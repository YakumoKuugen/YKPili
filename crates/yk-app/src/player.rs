//! 播放浮窗（M3）。
//!
//! 绘制顺序 = 这个文件里 child 的排列顺序（GPUI 没有 z-index）：
//!
//! ```text
//! ① 视频 surface      absolute inset_0       最先挂，铺满
//! ② 底部渐变遮罩      absolute bottom        让控制栏文字在亮画面上可读
//! ③ 控制栏 88px       deferred(priority 30)  弹幕输入 38 + 操作条 50
//! ④ 信息面板          deferred(priority 60)  展开 400 / 收起 200
//! ⑤ 骑边界按钮        deferred(priority 61)  唯一常驻的唤回入口
//! ```
//!
//! **D-05 规格（用户已调好，不要改）**：
//! 收起态是 **200px 半宽仍可见**，不是消失；且**面板本体即 hover 热区**。

use gpui::component::input::{Input, InputState};
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::danmaku::{Danmaku, DanmakuSet, LaneAllocator};
use yk_core::player::PlayerState;
use yk_ui::theme::*;

use crate::video::TestFrame;

/// 真实视频源（libmpv 解一帧）。拿到就用它，拿不到退回测试帧。
#[cfg(target_os = "macos")]
pub struct MpvFrameSource {
    player: yk_media::MpvPlayer,
    buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    tick: u64,
}

#[cfg(target_os = "macos")]
impl MpvFrameSource {
    /// 用 libmpv 解一帧并转成 CVPixelBuffer。失败时返回 None，调用方退回测试帧。
    pub fn open(video: &str, w: u32, h: u32) -> Option<Self> {
        let mut mpv = yk_media::MpvPlayer::new(w, h).ok()?;
        mpv.load_file(video).ok()?;
        // 给解码器一点时间产出首帧
        std::thread::sleep(std::time::Duration::from_millis(350));
        let rgba = mpv.render_frame().ok()?;
        let nv12 = yk_media::convert::rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
        Some(Self {
            player: mpv,
            buffer: yk_media::pixel_buffer::nv12_to_pixel_buffer(&nv12),
            tick: 0,
        })
    }

    /// 取一帧最新画面。播放循环每帧调一次。
    ///
    /// 每次都重新建 CVPixelBuffer —— 验证阶段够用，但它是一帧一次分配，
    /// 真机上应该改成「复用 buffer + 只更新两个平面」。
    /// 取一帧最新画面，返回当前播放位置（秒）。播放循环每帧调一次。
    pub fn pump(&mut self) -> f64 {
        self.player.drain_events();
        let t = self.player.get_property_double("time-pos").unwrap_or(0.0);
        // 诊断：时间轴到底有没有在走
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 60 == 1 {
            let t = self.player.get_property_double("time-pos").unwrap_or(-1.0);
            let aid = self.player.get_property_double("aid").unwrap_or(-1.0);
            let vol = self.player.get_property_double("volume").unwrap_or(-1.0);
            eprintln!(
                "[mpv] time-pos={t:.3}s aid={aid} volume={vol} tick={}",
                self.tick
            );
        }
        if let Ok(rgba) = self.player.render_frame() {
            let nv12 = yk_media::convert::rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
            self.buffer = yk_media::pixel_buffer::nv12_to_pixel_buffer(&nv12);
        }
        t
    }

    pub fn buffer(&self) -> objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer> {
        self.buffer.clone()
    }
}

/// 控制栏总高（唯一口径）。
const CTRL_H: f32 = PLAYER_CONTROL_BAR_HEIGHT; // 88
/// 弹幕输入条高。
const DANMAKU_INPUT_H: f32 = 38.0;
const INFO_PANEL_EXPANDED: f32 = PLAYER_INFO_PANEL_WIDTH; // 400
const INFO_PANEL_COLLAPSED: f32 = PLAYER_INFO_PANEL_COLLAPSED_WIDTH; // 200

pub struct PlayerView {
    theme: YkTheme,
    frame: TestFrame,
    /// 真实视频帧（libmpv 解出）。None 时视频层显示测试帧。
    #[cfg(target_os = "macos")]
    mpv: Option<MpvFrameSource>,
    /// 播放循环是否已启动（render 首帧时启动一次）
    #[cfg(target_os = "macos")]
    loop_started: bool,
    /// 面板是否展开（hover 唤起）。
    expanded: bool,
    /// 当前 Tab：0 评论 / 1 弹幕 / 2 选集。
    tab: usize,
    /// 播放状态（音量/倍速/进度/全屏）全部来自 yk-core 的 PlayerState。
    state: PlayerState,
    /// 弹幕：已按时间排序，且每条都预分配了轨道
    danmaku: Vec<(Danmaku, usize)>,
    lanes: LaneAllocator,
    /// 最近一次从 mpv 读到的播放位置（秒），弹幕位置靠它算
    play_time: f64,
    /// 弹幕输入框状态（首次 render 时创建，那时才有 window）
    danmaku_input: Option<Entity<InputState>>,
}

impl PlayerView {
    pub fn new(theme: YkTheme) -> Self {
        Self {
            theme,
            frame: TestFrame::new(1280, 720),
            #[cfg(target_os = "macos")]
            mpv: {
                // M8 验证：先固定解一帧，确认 libmpv → NV12 → CVPixelBuffer → surface 全链路。
                let path = std::env::var("YK_VIDEO")
                    .unwrap_or_else(|_| "/tmp/mpv-test.mp4".to_string());
                match MpvFrameSource::open(&path, 1280, 720) {
                    Some(s) => {
                        eprintln!("[mpv] 已解码首帧：{path}");
                        Some(s)
                    }
                    None => {
                        eprintln!("[mpv] 解码失败，退回测试帧：{path}");
                        None
                    }
                }
            },
            #[cfg(target_os = "macos")]
            loop_started: false,
            expanded: true,
            tab: 0,
            danmaku: {
                let set = mock_danmaku();
                // 预分配轨道：一次算完，播放时只查时间
                let mut lanes = LaneAllocator::new(1280.0, 220.0);
                let mut out = Vec::with_capacity(set.len());
                for d in set.items() {
                    if d.mode.is_scroll() {
                        let lane = lanes.assign(d);
                        out.push((d.clone(), lane));
                    }
                }
                out
            },
            lanes: LaneAllocator::new(1280.0, 220.0),
            play_time: 0.0,
            danmaku_input: None,
            state: {
                let mut st = PlayerState::new();
                st.duration_secs = 754.0;
                st.position_secs = 256.0;
                st.playing = true;
                st
            },
        }
    }

    /// 外部强制取一帧（截图验证用）。
    #[cfg(target_os = "macos")]
    pub fn pump_now(&mut self) {
        if let Some(m) = self.mpv.as_mut() {
            self.play_time = m.pump();
        }
    }

    fn panel_width(&self) -> f32 {
        if self.expanded {
            INFO_PANEL_EXPANDED
        } else {
            INFO_PANEL_COLLAPSED
        }
    }
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        eprintln!("[M3] PlayerView::render 被调用");
        // 播放循环：每 16ms 取一帧最新画面并重绘（≈60fps）。
        // libmpv 已在自动播放，这里只负责「把当前帧搬到 surface 上」。
        #[cfg(target_os = "macos")]
        if !self.loop_started && self.mpv.is_some() {
            self.loop_started = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    let alive = this.update(cx, |view, cx| {
                        if let Some(m) = view.mpv.as_mut() {
                            view.play_time = m.pump();
                        }
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        // 弹幕输入框：首次 render 才有 window，所以在这里创建。
        // 用 gpui-component 的 Input —— 它内部走 InputHandler，
        // 支持 IME 组合（marked text），中文输入必需。
        if self.danmaku_input.is_none() {
            let state = cx.new(|cx| {
                InputState::new(window, cx).placeholder("发一条友善的弹幕吧")
            });
            self.danmaku_input = Some(state);
        }

        let t = self.theme.clone();

        // 浮窗是「独立场景」，必须绝对定位铺满最近的定位祖先（AppRoot 根 div 是 relative）。
        // 只写 size_full 不行：作为 children 加入时外层没有尺寸约束，会退化成 0×0，
        // 表现就是「渲染日志有、画面上什么都没有」。
        div()
            .absolute()
            .inset_0()
            .bg(rgb(0x000000))
            // ① 视频层（弹幕作为它的子元素，避免形成独立的合成层把 surface 遮掉）
            .child({
                #[cfg(target_os = "macos")]
                {
                    video_layer(&self.frame, self.mpv.as_ref())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    video_layer(&self.frame)
                }
            })
            // ①b 弹幕层 —— 暂不启用。
            //
            // 实测：只要在同一棵元素树上叠加**文字**，surface 就会整块变黑；
            // 换成同尺寸的**色块**则正常。三种挂法都试过，全部黑：
            //   1. 作为根节点的普通 absolute 兄弟
            //   2. 嵌在 video_layer 内部
            //   3. deferred(priority 10)
            //
            // 形态上不是尺寸或定位问题，是「surface 与文本渲染的合成冲突」。
            // 之后按文档方案 ② 改用 canvas + window.text_system().shape_line()
            // 直接 paint 文字（不经过 div+text），**结果同样是黑屏**。
            //
            // 综合四次实验，根因确定：**只要向 surface 之上绘制 glyph，
            // surface 就被破坏**。同尺寸色块则完全正常。
            // 这是 GPUI Metal renderer 的合成问题，不是用法问题。
            //
            // 绕过方向（下一步）：
            //   a. 文字先渲染到离屏纹理，再当图片叠加（牺牲锐度）
            //   b. 弹幕用系统原生覆盖层（NSView），脱离 GPUI 合成
            //   c. 上报 GPUI，等 surface 与 glyph 的纹理槽冲突修复
            //
            .child(deferred(self.danmaku_layer()).with_priority(10usize))
            // ② 底部渐变遮罩：控制栏压在画面上时必须靠它保证文字可读
            .child(
                div().absolute().left_0().right_0().bottom_0().h(px(CTRL_H + 120.)).bg(
                    linear_gradient(
                        0.0,
                        linear_color_stop(hsla(0.0, 0.0, 0.0, 0.0), 0.0),
                        linear_color_stop(hsla(0.0, 0.0, 0.0, 0.72), 1.0),
                    ),
                ),
            )
            // ③ 控制栏（deferred priority 30）
            .child(
                deferred(control_bar(&t, &self.state, self.danmaku_input.as_ref(), cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.state.toggle_play();
                        window.refresh();
                        cx.notify();
                    },
                )))
                .with_priority(30usize),
            )
            // ④ 信息面板（deferred priority 60）
            .child(
                deferred(info_panel(&t, self.expanded, self.tab, self.panel_width(), cx))
                    .with_priority(60usize),
            )
            // ⑤ 骑边界按钮（deferred priority 61）：面板收起时的常驻唤回入口
            .child(
                deferred(
                    div()
                        .absolute()
                        .top(px(0.))
                        .right(px(self.panel_width() - 14.))
                        .w(px(28.))
                        .h(px(56.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_SM))
                        .bg(t.surfaces.surface_elevated_strong)
                        .border_1()
                        .border_color(t.surfaces.border_popover)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(t.surfaces.text_secondary)
                        .child(if self.expanded { "»" } else { "«" }),
                )
                .with_priority(61usize),
            )
    }
}

// ================= 弹幕层 =================

impl PlayerView {
    /// 当前时刻该显示的滚动弹幕。
    ///
    /// 每帧只为「可见窗口」内的弹幕生成元素，不是全量 —— 一场直播可能有上万条，
    /// 全量遍历会直接掉帧。窗口前半段留 15 秒是因为一条弹幕最长要飞这么久。
    fn danmaku_layer(&self) -> impl IntoElement {
        let now = self.play_time;

        // 先在普通 Rust 里把「这一帧要画什么」算好，交给 paint 闭包。
        // 闭包必须 'static，所以数据要 move 进去。
        struct Drawn {
            x: f32,
            y: f32,
            text: String,
            size: f32,
            color: gpui::Hsla,
        }
        let drawn: Vec<Drawn> = self
            .danmaku
            .iter()
            .filter(|(d, _)| self.lanes.visible_at(d, now))
            .take(80)
            .map(|(d, lane)| Drawn {
                x: self.lanes.x_at(d, now),
                y: 8.0 + *lane as f32 * 30.0,
                text: d.text.clone(),
                size: d.font_size,
                color: rgb(d.rgb()).into(),
            })
            .collect();

        // canvas：直接走底层 paint，不经过 div + text 的常规元素路径。
        // 常规路径下文字会破坏 surface 的合成（实测画面全黑），
        // 这条是文档第 14 节指定的方案 ②。
        canvas(
            |_bounds, _window, _cx| (),
            move |_bounds, _state, window, cx| {
                for d in &drawn {
                    let run = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: d.color,
                        ..Default::default()
                    };
                    // shape_line 在 WindowTextSystem 上，不是 TextSystem
                    let line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[run],
                        None,
                    );
                    let _ = line.paint(
                        point(px(d.x), px(d.y)),
                        px(d.size * 1.25),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            },
        )
        .absolute()
        .inset_0()
    }
}

/// 占位弹幕（真实数据要从 `/x/v1/dm/list.so` 拉 XML，属于后续）。
fn mock_danmaku() -> DanmakuSet {
    const TEXTS: [&str; 12] = [
        "前方高能", "这也太真实了", "哈哈哈哈", "有一说一，确实", "第一次看到这种",
        "好家伙", "这个可以", "泪目", "up主加油", "妙啊", "学到了", "名场面",
    ];
    let raw = (0..300).map(|i| {
        // 从 -6 秒开始：这样首帧（play_time≈0）就有弹幕在屏幕上，
        // 便于截图验收；真实数据里时间是 0 起。
        let t = (i as f64) * 0.22 - 6.0;
        let color = if i % 7 == 0 { 16777215u32 } else { 65280 };
        let p = format!("{t},1,25,{color},0,0,0,0");
        (p, TEXTS[i % TEXTS.len()].to_string())
    });
    DanmakuSet::from_raw(raw)
}

// ================= 视频层 =================

/// macOS：CVPixelBuffer 交给 GPUI 的 `surface()`，与控件同管线合成。
///
/// 其它平台（含 iOS）：`surface()` 在 gpui-pre 0.3.9 里只在 Apple 桌面端可用，
/// 这里先渲染占位。iOS 的真实链路（libmpv 解码 → 纹理）属于 M8 的后续工作，
/// 解码方案已定：**libmpv**（跨桌面 + 移动端同一套）。
#[cfg(target_os = "macos")]
fn video_layer(frame: &TestFrame, mpv: Option<&MpvFrameSource>) -> impl IntoElement {
    // 有真实帧就用真实的，否则退回 spike 的测试帧
    let buffer = match mpv {
        Some(m) => m.buffer(),
        None => frame.buffer(),
    };
    div()
        .absolute()
        .inset_0()
        .child(surface(buffer).size_full())
}

#[cfg(not(target_os = "macos"))]
fn video_layer(_frame: &TestFrame) -> impl IntoElement {
    div().absolute().inset_0().bg(rgb(0x101014)).child(
        div()
            .absolute()
            .left(px(12.))
            .bottom(px(12.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(RADIUS_SM))
            .bg(rgba(0x000000AA))
            .text_size(px(FONT_SIZE_MICRO))
            .text_color(rgb(0xFFFFFF))
            .child("视频层占位 · iOS 待接 libmpv"),
    )
}

// ================= 控制栏 =================

fn control_bar(
    t: &YkTheme,
    state: &PlayerState,
    danmaku_input: Option<&Entity<InputState>>,
    on_play: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (time_now, time_total) = state.time_texts();
    let progress = state.progress();
    let s = &t.surfaces;
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(CTRL_H))
        .flex()
        .flex_col()
        // ---- 上半：弹幕输入 38 ----
        .child(
            div()
                .h(px(DANMAKU_INPUT_H))
                .w_full()
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(match danmaku_input {
                    Some(st) => div()
                        .flex_1()
                        .h(px(26.))
                        .child(Input::new(st).appearance(false).bordered(false))
                        .into_any_element(),
                    None => div()
                        .flex_1()
                        .h(px(26.))
                        .px(px(SPACE_2))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0xFFFFFF1A))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(hsla(1.0, 0.0, 1.0, 0.55))
                        .child("发一条友善的弹幕吧")
                        .into_any_element(),
                })
                .child(
                    div()
                        .px(px(SPACE_2))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(palette().theme_primary)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child("发送"),
                ),
        )
        // ---- 下半：操作条 50 ----
        .child(
            div()
                .h(px(50.))
                .w_full()
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_3))
                .child(
                    div()
                        .id("play-toggle")
                        .w(px(32.))
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(32. / 2.))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(rgb(0xFFFFFF))
                        .child(state.play_pause_glyph())
                        .on_click(on_play),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(hsla(1.0, 0.0, 1.0, 0.85))
                        .child(time_now),
                )
                // 进度条
                .child(
                    div()
                        .flex_1()
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(rgba(0xFFFFFF33))
                        .child(
                            div()
                                .h(px(4.))
                                .w(px(((1280. - 400.) as f64 * progress) as f32))
                                .rounded(px(2.))
                                .bg(palette().theme_primary),
                        ),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(hsla(1.0, 0.0, 1.0, 0.85))
                        .child(time_total),
                )
                .child(bar_icon("弹"))
                .child(bar_icon("1080P"))
                .child(bar_icon(&state.rate_text()))
                .child(bar_icon("全屏")),
        )
        // 控制栏底边：靠遮罩承接，不加实线，避免在画面上出现硬边
        .text_color(s.text_on_accent)
}

fn bar_icon(label: &str) -> impl IntoElement {
    div()
        .px(px(SPACE_2))
        .h(px(26.))
        .flex()
        .items_center()
        .rounded(px(RADIUS_SM))
        .text_size(px(FONT_SIZE_MICRO))
        .text_color(hsla(1.0, 0.0, 1.0, 0.85))
        .child(label.to_string())
}

// ================= 信息面板 =================

const TABS: [&str; 3] = ["评论", "弹幕", "选集"];

fn info_panel(
    t: &YkTheme,
    expanded: bool,
    active_tab: usize,
    width: f32,
    cx: &mut Context<PlayerView>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let mut tab_row = div()
        .flex()
        .items_center()
        .gap(px(SPACE_4))
        .px(px(SPACE_4))
        .h(px(44.))
        .border_b_1()
        .border_color(s.border_subtle);
    for (i, name) in TABS.iter().enumerate() {
        let on = i == active_tab;
        tab_row = tab_row.child(
            div()
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(if on { s.text_primary } else { s.text_tertiary })
                .child(*name),
        );
    }

    div()
        .id("info-panel")
        .absolute()
        .top(px(0.))
        .right(px(0.))
        .bottom(px(CTRL_H))
        .w(px(width))
        .flex()
        .flex_col()
        // 浮在视频上的面板：必须用 Strong 变体。
        // GPUI 没有元素级 backdrop blur，所以不能靠模糊压暗背后的画面。
        .bg(s.surface_elevated_strong)
        .border_l_1()
        .border_color(s.border_subtle)
        // 面板本体即 hover 热区（D-05）：进入展开、离开收回
        .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
            let next = *hovered;
            if this.expanded != next {
                this.expanded = next;
                cx.notify();
            }
        }))
        .child(tab_row)
        .child(
            div()
                .flex_1()
                .p(px(SPACE_4))
                .flex()
                .flex_col()
                .gap(px(SPACE_3))
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_tertiary)
                .child(if expanded {
                    "展开态 400px：评论 / 弹幕 / 选集"
                } else {
                    "收起态 200px"
                }),
        )
}
