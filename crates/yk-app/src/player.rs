//! 播放浮窗（M3）。
//!
//! 绘制顺序 = 这个文件里 child 的排列顺序（GPUI 没有 z-index）：
//!
//! ```text
//! ⓪ 浮窗外侧点击层    topbar 之下、整页      点击关闭（顶栏保持可用）
//! ① 浮窗 600 高       顶栏下方 16px          圆角 + shadow_glass_modal
//!    ⓪ 入场          封面 bounds → 目标框    280ms 单段曲线 + 轻回弹
//!    ①a 视频 surface   absolute inset_0     铺满窗口
//!    ①b 弹幕层 / 取流状态 / 底部渐变
//!    ①c 控制栏 88px     absolute bottom
//!    ①d 信息面板       展开 400 / 收起 200
//!    ①e 骑边界按钮
//!    ①f Esc 提示       第一次 Esc 显示
//! ```
//!
//! **D-05 规格（用户已调好，不要改）**：
//! 收起态是 **200px 半宽仍可见**，不是消失；且**面板本体即 hover 热区**。

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::prelude::*;
use gpui::{FocusHandle, InteractiveElement, StatefulInteractiveElement, *};
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
        let t0 = std::time::Instant::now();
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
        let cost = t0.elapsed();
        if cost.as_millis() >= 8 {
            eprintln!(
                "[player] pump 耗时 {}ms（>8ms 会拖慢 UI）",
                cost.as_millis()
            );
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
/// 浮窗高度（设计文档第 06 节：600）。
const PLAYER_FLOAT_HEIGHT: f32 = 600.0;
/// 从封面展开：一条连续曲线走完，避免两段缓动在交界处出现速度断点。
/// 280ms 向 macOS Quick Look 的快速预览节奏靠拢，尾部只留很短的过冲回弹。
const PLAYER_ENTRANCE_MS: u64 = 280;
/// 关闭缩回封面；Quick Look 收回通常比展开更短，这里 260ms。
const PLAYER_EXIT_MS: u64 = 260;

/// 点击卡片时冻结的封面几何与视觉信息。
///
/// 坐标已经换算成 AppRoot 页面坐标；`root_size` 用来在播放器窗口里
/// 重新算目标尺寸，窗口 resize 时不会用旧宽度。
#[derive(Clone)]
pub struct CoverSource {
    pub bounds: Bounds<Pixels>,
    pub root_size: Size<Pixels>,
    pub bvid: String,
    pub title: String,
    pub image_url: String,
    pub placeholder_color: String,
}

/// 一次播放的观看历史上报上下文。
///
/// 只在登录态下创建；`cookie` / `csrf` 在开始时冻结，避免中途退出登录
/// 把上报发到空会话上。
#[derive(Clone)]
struct HistoryReport {
    aid: i64,
    bvid: String,
    cid: i64,
    cookie: String,
    csrf: String,
    /// 本轮打开时刻（单调时钟，用于 5 秒冷却）。
    opened_at: std::time::Instant,
}

pub struct PlayerView {
    theme: YkTheme,
    frame: TestFrame,
    /// 视频渲染线程（解码与转换都在它上面，UI 线程只收帧）
    #[cfg(target_os = "macos")]
    worker: Option<crate::video_worker::FrameWorker>,
    /// 最近一帧（由 worker 产出）
    #[cfg(target_os = "macos")]
    current: Option<objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>>,
    /// 播放循环是否已启动（worker 创建后同帧启动一次，不能等下一次 render）
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
    /// 输入框按了 Enter（`InputEvent::PressEnter`），下一轮 render 里发送。
    /// 订阅回调拿不到 `Window`，而清空输入框需要，所以用 defer 转一手。
    submit_requested: bool,
    /// 是否已尝试启动渲染线程（换源时重置）
    tried_worker: bool,
    /// 当前卡片的播放源；None = 还没点卡片 / 正在请求。
    source: Option<yk_core::PlaybackSource>,
    /// 正在后台取播放地址。
    loading_source: bool,
    /// 取流失败原因（卡片没有 bvid / 需要登录 / 视频受限）。
    source_error: Option<String>,
    /// 当前视频的上报上下文；未登录或非投稿视频为 None。
    history_report: Option<HistoryReport>,
    /// 取流代次：连续点不同卡片时只接受最后一次结果。
    load_generation: u64,
    /// 输入框是否处于聚焦。聚焦时暂停渲染循环 ——
    /// 因为 pump() 在 UI 线程要做 10-25ms 的转换，会把输入事件和音频都拖卡。
    input_focused: bool,
    /// 浮窗根焦点：Esc 两次关闭。
    focus_handle: FocusHandle,
    /// 下一次 render 把焦点交给浮窗根（打开 / 换源时置位）。
    needs_focus: bool,
    /// 第一次 Esc 后显示「再次按 Esc 关闭」。
    escape_hint: bool,
    /// 提示代际：5 秒自动收回 / 关闭 / 换源时递增，旧定时器不能误关新提示。
    escape_hint_generation: u64,
    /// 点封面打开时的起始几何；None = 程序化打开（YK_BVID / YK_VIDEO）。
    entrance_source: Option<CoverSource>,
    /// 入场动画代际：只换 id，不在 render 里生成新时钟。
    entrance_generation: u64,
    /// 正在播放关闭缩回动画（此期间不再响应第二次关闭）。
    closing: bool,
    /// 关闭动画代际：旧定时器不能结束新一次关闭。
    closing_generation: u64,
    /// 关闭回调：`false` = 开始缩回，`true` = 缩回结束可移除实体。
    /// AppRoot 在开始时先恢复分类条/网格，让卡片回到封面快照的坐标。
    close_handler: Option<Box<dyn Fn(&mut App, bool)>>,
}

/// 进程内 `PlayerView` 实例数。
///
/// 设计上**只允许 1 个**：输入框的 `InputState` 挂在实例上，重建就等于换一个空输入框
/// （附录 F L-1）。`AppRoot::ensure_player` 已经保证复用，这里是第二道防线 ——
/// 以后谁再写一条 `cx.new(|_| PlayerView::new(..))`，会在这里立刻暴露。
static LIVE_PLAYERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 入场动画 id 的进程内序号：关闭再打开也拿新 id，避免元素状态复用导致直接跳到终点。
static ENTRANCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// 关闭缩回动画 id 的进程内序号：同一窗口反复开关也不会复用旧动画状态。
static EXIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Drop for PlayerView {
    fn drop(&mut self) {
        // 兜底：正常关闭 / 换源已经在 begin_close / open_bvid 里补过上报；
        // 直接 drop（退出登录、窗口销毁）时这里再补最后一拍。
        if let Some(mut report) = self.history_report.take() {
            if report.opened_at.elapsed() >= std::time::Duration::from_secs(5) {
                let position = self.play_time.max(0.0) as u64;
                dispatch_history_report(&report, position);
            }
        }
        LIVE_PLAYERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl PlayerView {
    pub fn new(theme: YkTheme, cx: &mut Context<Self>) -> Self {
        let n = LIVE_PLAYERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        assert!(
            n <= 1,
            "PlayerView 只能有一个实例（当前 {n} 个）：重建会换掉 InputState，\
             输入内容随之丢失，见附录 F L-1 / 附录 D 事实 19"
        );
        eprintln!("[player] PlayerView::new（实例数 {n}）");
        Self {
            theme,
            frame: TestFrame::new(1280, 720),
            // worker 不在这里启动：构造只做纯赋值，启动线程/打开文件这类副作用
            // 放到首次 render（见 render 里的 tried_worker）。
            // 动机是「构造保持无副作用」这条约束，而不是 cx.new 会重试 ——
            // `Context::new` 收的是 FnOnce，只调用一次。
            #[cfg(target_os = "macos")]
            worker: None,
            tried_worker: false,
            source: None,
            loading_source: false,
            source_error: None,
            history_report: None,
            load_generation: 0,
            #[cfg(target_os = "macos")]
            current: None,
            #[cfg(target_os = "macos")]
            loop_started: false,
            // 沉浸优先：播放浮窗默认收起信息面板（200px）。
            expanded: false,
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
            submit_requested: false,
            input_focused: false,
            focus_handle: cx.focus_handle().tab_stop(true),
            needs_focus: true,
            escape_hint: false,
            escape_hint_generation: 0,
            entrance_source: None,
            entrance_generation: 0,
            closing: false,
            closing_generation: 0,
            close_handler: None,
            state: {
                let mut st = PlayerState::new();
                st.duration_secs = 754.0;
                st.position_secs = 256.0;
                st.playing = true;
                st
            },
        }
    }

    /// 卡片点击时注入起始封面；必须在 `open_bvid` 之前调用。
    pub fn set_entrance(&mut self, source: CoverSource, cx: &mut Context<Self>) {
        eprintln!(
            "[player] 封面展开：{}（{:.0}x{:.0} → 浮窗）",
            source.title,
            source.bounds.size.width.as_f32(),
            source.bounds.size.height.as_f32()
        );
        self.entrance_source = Some(source);
        self.entrance_generation = ENTRANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        cx.notify();
    }

    /// 卡片点击入口：按 bvid 取详情 + playurl，回填后由下一次 render 启动 mpv。
    ///
    /// 请求在后台 executor 跑；用 `load_generation` 丢弃过期结果，
    /// 连续点两张卡时不会出现旧视频覆盖新视频。
    pub fn open_bvid(&mut self, bvid: String, cx: &mut Context<Self>) {
        // 换源前先把上一条的关闭上报补掉，否则连续点两个视频时
        // 第一个只留下打开时的 0s 那一拍。
        self.flush_history_report();
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.loading_source = true;
        self.source_error = None;
        self.source = None;
        #[cfg(target_os = "macos")]
        {
            self.worker = None;
            self.current = None;
        }
        self.tried_worker = false;
        self.play_time = 0.0;
        self.state.position_secs = 0.0;
        self.state.playing = true;
        self.needs_focus = true;
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);
        eprintln!("[player] 请求播放：{bvid}");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let cookie = current_session_cookie();
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_playback(&client, &bvid, &cookie)
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                view.loading_source = false;
                match result {
                    Ok(src) => {
                        eprintln!(
                            "[player] 取流成功：{}（{}s，音轨 {}）",
                            src.title,
                            src.duration_secs,
                            if src.audio_url.is_some() {
                                "有"
                            } else {
                                "无"
                            }
                        );
                        view.state.duration_secs = src.duration_secs as f64;
                        view.state.position_secs = 0.0;
                        view.state.playing = true;
                        let (aid, bvid, cid) = (src.aid, src.bvid.clone(), src.cid);
                        view.source = Some(src);
                        view.begin_history_report(aid, bvid, cid);
                    }
                    Err(e) => {
                        eprintln!("[player] 取流失败：{e}");
                        view.source_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开视频时上报一次历史（进度 0，状态「已观看」）。
    ///
    /// 不做周期心跳：只在打开 / 关闭两个时机各报一次。关闭时是否再报，
    /// 由 [`Self::flush_history_report`] 判断「打开是否已满 5 秒」。
    fn begin_history_report(&mut self, aid: i64, bvid: String, cid: i64) {
        let Some(session) = current_session() else {
            self.history_report = None;
            return;
        };
        if aid <= 0 || cid <= 0 || bvid.trim().is_empty() {
            self.history_report = None;
            eprintln!("[history-report] 缺少 aid/cid/bvid，跳过：aid={aid} cid={cid} bvid={bvid}");
            return;
        }
        let report = HistoryReport {
            aid,
            bvid: bvid.clone(),
            cid,
            cookie: session.cookie_header(),
            csrf: session.bili_jct.clone(),
            opened_at: std::time::Instant::now(),
        };
        eprintln!("[history-report] 打开上报：{bvid}（aid={aid} cid={cid}）");
        dispatch_history_report(&report, 0);
        self.history_report = Some(report);
    }

    /// 关闭 / 换源时补报一次；打开不足 5 秒则只保留打开时的那一次。
    fn flush_history_report(&mut self) {
        let Some(mut report) = self.history_report.take() else {
            return;
        };
        if report.opened_at.elapsed() < std::time::Duration::from_secs(5) {
            eprintln!(
                "[history-report] 打开不足 5s，跳过关闭上报：{}（{:.1}s）",
                report.bvid,
                report.opened_at.elapsed().as_secs_f32()
            );
            return;
        }
        let position = self.play_time.max(0.0) as u64;
        eprintln!("[history-report] 关闭上报：{} @ {position}s", report.bvid);
        dispatch_history_report(&report, position);
    }

    /// AppRoot 注入：关闭时把自己的 `playing` 置空。
    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App, bool) + 'static) {
        self.close_handler = Some(Box::new(handler));
    }

    fn escape_hint_overlay(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.escape_hint {
            return None;
        }
        Some(
            div()
                .id("player-esc-hint")
                .absolute()
                // 顶栏右侧：在浮窗/视频之外，不遮画面。
                .top(px((TOP_BAR_HEIGHT - 32.) / 2.))
                .right(px(SPACE_4))
                .occlude()
                .h(px(32.))
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(t.surfaces.surface_elevated_strong)
                .border_1()
                .border_color(t.surfaces.border_popover)
                .shadow(t.shadows.shadow_2.clone())
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_SM))
                        .bg(t.palette.status_error)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child("×"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(t.surfaces.text_secondary)
                        .child("再次按 Esc 关闭"),
                )
                .child(
                    div()
                        .px(px(4.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .border_1()
                        .border_color(t.surfaces.border_strong)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(t.surfaces.text_tertiary)
                        .child("Esc"),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.request_close(window, cx);
                }))
                .into_any_element(),
        )
    }

    /// 第一次 Esc：显示提示，5 秒后自动收回；期间再按 Esc 才关闭。
    fn arm_escape_hint(&mut self, cx: &mut Context<Self>) {
        self.escape_hint = true;
        let generation = self.escape_hint_generation.wrapping_add(1);
        self.escape_hint_generation = generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.escape_hint_generation == generation && view.escape_hint {
                    view.escape_hint = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn request_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.begin_close(cx);
    }

    /// 关闭状态机本体；不依赖 Window，方便关闭请求从 Esc/遮罩/按钮统一进入。
    fn begin_close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        // 关闭前补最后一拍；打开不足 5s 的会在 flush 里被冷却拦掉。
        self.flush_history_report();
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);

        // 先恢复页面布局：关闭起点是“打开前”的封面坐标，分类条必须回来，
        // 否则网格还压在上移后的位置，缩回终点会对不上卡片。
        if let Some(handler) = self.close_handler.as_ref() {
            handler(cx, false);
        }

        // 没有封面快照（YK_BVID/YK_VIDEO 程序化打开）就直接收掉。
        if self.entrance_source.is_none() {
            if let Some(handler) = self.close_handler.as_ref() {
                handler(cx, true);
            }
            return;
        }

        self.closing = true;
        self.closing_generation = EXIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.closing_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(PLAYER_EXIT_MS + 30))
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.closing || view.closing_generation != generation {
                    return;
                }
                view.closing = false;
                if let Some(handler) = view.close_handler.as_ref() {
                    handler(cx, true);
                }
            });
        })
        .detach();
    }

    fn panel_width(&self) -> f32 {
        if self.expanded {
            INFO_PANEL_EXPANDED
        } else {
            INFO_PANEL_COLLAPSED
        }
    }
}

/// 入场/退场共用一条平滑曲线：EaseStandard 起手，尾部约 0.9% 过冲后
/// 自然回到 1。单段曲线没有速度断点，缩放过程不会在 300ms 附近「顿一下」。
fn entrance_progress(raw: f32) -> f32 {
    ease_entrance()(raw.clamp(0.0, 1.0))
}

fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |a: Pixels, b: Pixels| px(a.as_f32() + (b.as_f32() - a.as_f32()) * t);
    Bounds {
        origin: point(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        size: size(
            mix(a.size.width, b.size.width),
            mix(a.size.height, b.size.height),
        ),
    }
}

/// 入场期的 Esc 也从同一个焦点句柄进；用弱实体回到 PlayerView 的状态机。
fn player_escape_handler(
    weak: WeakEntity<PlayerView>,
) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    move |event, window, cx| {
        if event.keystroke.key != "escape" {
            return;
        }
        let _ = weak.update(cx, |view, cx| {
            if view.escape_hint {
                view.request_close(window, cx);
            } else {
                view.arm_escape_hint(cx);
            }
        });
        cx.stop_propagation();
    }
}

/// 入场/退场共用的封面海报：渐变兜底 + 标题垫底 + 真实缩略图。
fn cover_poster(theme: &YkTheme, source: &CoverSource) -> AnyElement {
    let mut poster = div().absolute().inset_0().bg(crate::placeholder_gradient(
        &source.placeholder_color,
        theme.surfaces.fill_3,
    ));
    if !source.title.is_empty() {
        poster = poster.child(
            div()
                .absolute()
                .inset_0()
                .p(px(SPACE_3))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(16.))
                .line_height(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .text_center()
                .line_clamp(3)
                .child(source.title.clone()),
        );
    }
    if !source.image_url.is_empty() {
        poster = poster.child(
            img(ImageSource::from(source.image_url.clone()))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    poster.into_any_element()
}

/// 从封面几何展开到播放器目标尺寸。
///
/// 过渡期只挂封面海报，到 `progress == 1` 才挂完整播放器内容：
/// 控制栏/信息面板不会被塞进 192×108 的盒子，也不会让 deferred 的
/// 面板唤回按钮从窗口边缘跑出去。
#[derive(gpui::IntoElement)]
struct PlayerEntrance {
    theme: YkTheme,
    source: CoverSource,
    content: Option<AnyElement>,
    focus_handle: FocusHandle,
    weak: WeakEntity<PlayerView>,
    progress: f32,
}

impl RenderOnce for PlayerEntrance {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PlayerEntrance {
            theme,
            source,
            content,
            focus_handle,
            weak,
            progress,
        } = self;
        let p = entrance_progress(progress);
        let root_w = source.root_size.width.as_f32().max(2.0 * SPACE_4 + 1.0);
        let destination = Bounds {
            origin: point(px(SPACE_4), px(TOP_BAR_HEIGHT + SPACE_4)),
            size: size(px(root_w - 2.0 * SPACE_4), px(PLAYER_FLOAT_HEIGHT)),
        };
        let bounds = lerp_bounds(source.bounds, destination, p);

        let shell = div()
            .id("player-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(0x0B0F14))
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude()
            .key_context("Player")
            .track_focus(&focus_handle)
            .on_key_down(player_escape_handler(weak));

        if progress >= 1.0 {
            return shell.child(content.expect("入场结束时必须挂上播放器内容"));
        }

        shell.child(cover_poster(&theme, &source))
    }
}

/// 关闭缩回：从当前播放器目标框缩回点击时的封面几何。
///
/// 过渡期只保留当前视频帧（没有帧时用封面海报），不挂控制栏/信息面板，
/// 避免窗口缩小时控件被重排得乱跳。
#[derive(gpui::IntoElement)]
struct PlayerExit {
    theme: YkTheme,
    source: CoverSource,
    visual: Option<AnyElement>,
    progress: f32,
}

impl RenderOnce for PlayerExit {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PlayerExit {
            theme,
            source,
            visual,
            progress,
        } = self;
        let p = entrance_progress(progress);
        let q = 1.0 - p;
        let root_w = source.root_size.width.as_f32().max(2.0 * SPACE_4 + 1.0);
        let destination = Bounds {
            origin: point(px(SPACE_4), px(TOP_BAR_HEIGHT + SPACE_4)),
            size: size(px(root_w - 2.0 * SPACE_4), px(PLAYER_FLOAT_HEIGHT)),
        };
        let bounds = lerp_bounds(source.bounds, destination, q);
        let shell = div()
            .id("player-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(0x0B0F14))
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude();
        shell.children(visual)
    }
}

/// 当前登录会话；未登录/旧文件损坏返回 None。
fn current_session() -> Option<yk_core::models::UserSession> {
    let path = yk_core::session::session_path();
    yk_core::session::load(&path).filter(|s| s.is_valid())
}

/// 登录会话的 Cookie 头；未登录/旧文件损坏返回空串（仍可取 480P）。
fn current_session_cookie() -> String {
    current_session()
        .map(|s| s.cookie_header())
        .unwrap_or_default()
}

/// 把一次历史上报快照丢到后台线程发送。
///
/// 不挂在 `cx.spawn` 上：关闭动画结束时 `PlayerView` 会被回收，
/// 挂在实体上的任务可能一起被取消；最后一次上报必须独立于实体生命周期。
fn dispatch_history_report(report: &HistoryReport, progress: u64) {
    let payload = yk_core::api::client::HistoryProgress {
        aid: report.aid,
        cid: report.cid,
        progress,
    };
    let cookie = report.cookie.clone();
    let csrf = report.csrf.clone();
    eprintln!(
        "[history-report] 上报 {}：progress={}s",
        report.bvid, payload.progress
    );
    std::thread::spawn(move || {
        let client = match yk_core::api::client::ReqwestClient::new() {
            Ok(client) => client,
            Err(error) => {
                eprintln!("[history-report] 创建客户端失败：{error}");
                return;
            }
        };
        if let Err(error) = yk_core::api::client::report_history(&client, &cookie, &csrf, &payload)
        {
            eprintln!("[history-report] 上报失败：{error}");
        }
    });
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        eprintln!("[M3] PlayerView::render 被调用");
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.focus_handle.clone();
            // 等这一帧的元素树挂上 track_focus 后再交焦点，避免首帧焦点丢失。
            cx.defer_in(window, move |_view, window, cx| {
                window.focus(&handle, cx);
            });
        }
        if self.submit_requested {
            self.submit_requested = false;
            cx.defer_in(window, |view, window, cx| view.send_danmaku(window, cx));
        }
        // 渲染线程：有播放源才启动；换源时 open_bvid 会把 tried_worker 复位。
        // 没有播放源时只有 YK_VIDEO 验收入口会退回本地测试文件，
        // 正常点卡片期间（loading_source）绝不先播测试视频。
        #[cfg(target_os = "macos")]
        if !self.tried_worker {
            let source = self
                .source
                .as_ref()
                .map(|s| (s.video_url.clone(), s.audio_url.clone()));
            let fallback =
                if source.is_none() && !self.loading_source && self.source_error.is_none() {
                    std::env::var("YK_VIDEO").ok()
                } else {
                    None
                };
            if let Some((video, audio)) = source.or_else(|| fallback.map(|v| (v, None))) {
                self.tried_worker = true;
                self.worker =
                    crate::video_worker::FrameWorker::spawn_with_audio(video, audio, 1280, 720);
                // 取流期间用户可能已经按过暂停：新 worker 启动后补发一次状态。
                if let Some(w) = self.worker.as_ref() {
                    w.set_paused(!self.state.playing);
                }
                // 渲染循环已在同一个 render 里、worker 创建之后立即启动；
                // 这里再 notify 一次，让首帧尽快重绘，而不是等下一次用户事件。
                if self.worker.is_some() {
                    cx.notify();
                }
            }
        }

        // 播放循环：每 16ms 取一帧最新画面并重绘（≈60fps）。
        // **必须放在 worker 创建之后**：以前放在 render 开头，靠 cx.notify()
        // 等下一轮 render 才启动；窗口未激活/被遮挡时下一轮可能永远不来，
        // 表现就是音频在播、画面不出，点一下播放器触发 render 后才出首帧。
        // libmpv 已在自动播放，这里只负责「把当前帧搬到 surface 上」。
        #[cfg(target_os = "macos")]
        if !self.loop_started && self.worker.is_some() {
            self.loop_started = true;
            cx.spawn(async move |this, cx| {
                let mut received = 0u64;
                let mut idle_ticks = 0u32;
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    // UI 线程只做「取一帧 + 通知重绘」，不碰任何解码/转换。
                    // 这就是输入与播放互不耦合的关键：即使正在打字，
                    // 渲染线程照常出帧，UI 也只是轻量地更新一下。
                    let alive = this.update(cx, |view, cx| {
                        let mut got = false;
                        if let Some(w) = view.worker.as_ref() {
                            if let Some((buf, t)) = w.latest() {
                                view.current = Some(buf);
                                view.play_time = t;
                                view.state.position_secs = t;
                                got = true;
                            }
                        }
                        if got {
                            received += 1;
                            idle_ticks = 0;
                            if received == 1 {
                                eprintln!("[player] 收到首帧");
                            }
                            cx.notify();
                        } else {
                            idle_ticks += 1;
                            if idle_ticks == 125 {
                                idle_ticks = 0;
                                eprintln!("[player] 连续 2s 没有新帧");
                            }
                        }
                        got
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
            let state = cx.new(|cx| InputState::new(window, cx).placeholder("发一条友善的弹幕吧"));
            // Enter 走 Input 自己的 Enter action（外层补了 key_context("Input") 才匹配），
            // 由它发出 PressEnter。Shift+Enter 不发送。
            // 中文输入法组合中的 Enter 应由输入法先消费（用于选词）；
            // 这条依赖平台 IME 行为，真机选词场景还要再验一次。
            cx.subscribe(&state, |view, _state, event: &InputEvent, cx| {
                if let InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } = event
                {
                    eprintln!("[input] Enter → 发送");
                    view.submit_requested = true;
                    cx.notify();
                }
            })
            .detach();
            eprintln!("[input] InputState 已创建");
            self.danmaku_input = Some(state);
        }

        let t = self.theme.clone();
        // 入场目标宽度跟随当前窗口；封面起点仍是点击时冻结的那一份。
        if let Some(source) = self.entrance_source.as_mut() {
            source.root_size = window.viewport_size();
        }
        // 浮窗内部：绝对铺满窗口。窗口本身负责圆角/裁剪/阴影/遮挡。
        let content = div()
            .absolute()
            .inset_0()
            .bg(rgb(0x0B0F14))
            // ① 视频层（弹幕作为它的子元素，避免形成独立的合成层把 surface 遮掉）
            .child({
                #[cfg(target_os = "macos")]
                {
                    video_layer(&self.frame, self.current.as_ref())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    video_layer(&self.frame)
                }
            })
            // ①b 弹幕层：gpui-fast 已解决 surface 与文字的合成冲突，
            // 直接 canvas 绘制即可（见附录 D 事实 16）。
            .child(self.danmaku_layer())
            // ①c 取流状态：加载中 / 失败说明，不遮挡控制栏与右侧面板
            .children(
                if self.loading_source {
                    Some("正在获取播放地址…".to_string())
                } else {
                    self.source_error.clone()
                }
                .map(|text| {
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(rgb(0xFFFFFF))
                        .child(text)
                        .into_any_element()
                }),
            )
            // ② 底部渐变遮罩：控制栏压在画面上时必须靠它保证文字可读
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(CTRL_H + 120.))
                    .bg(linear_gradient(
                        0.0,
                        linear_color_stop(hsla(0.0, 0.0, 0.0, 0.0), 0.0),
                        linear_color_stop(hsla(0.0, 0.0, 0.0, 0.72), 1.0),
                    )),
            )
            // ③ 控制栏（deferred priority 30）
            .child(
                // gpui-fast 下不再需要 deferred（合成问题已解决）。
                // 而且 deferred 元素的 hit-test 有问题：输入框收不到点击。
                control_bar(
                    &t,
                    &self.state,
                    self.danmaku_input.as_ref(),
                    cx,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        // 点播放按钮 = 放弃输入焦点，恢复渲染循环
                        this.input_focused = false;
                        this.state.toggle_play();
                        #[cfg(target_os = "macos")]
                        if let Some(w) = this.worker.as_ref() {
                            w.set_paused(!this.state.playing);
                        }
                        window.refresh();
                        cx.notify();
                    }),
                ),
            )
            // ④ 信息面板（deferred priority 60）
            .child(info_panel(
                &t,
                self.expanded,
                self.tab,
                self.panel_width(),
                cx,
            ))
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
            .into_any_element();

        // 浮窗外侧点击层：从顶栏下方开始，顶栏本身保持可用。
        let backdrop = div()
            .id("player-backdrop")
            .absolute()
            .top(px(TOP_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            // 极低透明度背景：确保这层一定参与命中测试（纯 alpha=0 的 div
            // 在部分路径下会被当成不绘制元素，点击直接落到下层封面卡）。
            .bg(rgba(0x00000001))
            // 挡住下层卡片/页面：点浮窗外侧只关闭，不触发新卡片。
            .occlude()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                // 关闭浮窗的这一拍不要再把 click 传给下层封面卡。
                cx.stop_propagation();
                this.request_close(window, cx);
            }));

        // 关闭缩回时只保留画面：当前视频帧优先，没有帧就用封面海报。
        // 控制栏/面板不挂树，窗口缩小时不会被重排得乱跳。
        let closing_visual: Option<AnyElement> = if self.closing {
            #[cfg(target_os = "macos")]
            {
                if self.current.is_some() {
                    Some(video_layer(&self.frame, self.current.as_ref()).into_any_element())
                } else {
                    self.entrance_source
                        .as_ref()
                        .map(|source| cover_poster(&t, source))
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                self.entrance_source
                    .as_ref()
                    .map(|source| cover_poster(&t, source))
            }
        } else {
            None
        };

        // 卡片打开：用同一套窗口外壳，从点击时的封面几何展开到目标尺寸。
        // 动画进度来自稳定 id 的 Animation 时钟；render 里绝不新建起点。
        let window: AnyElement = if self.closing {
            let source = self
                .entrance_source
                .clone()
                .expect("关闭缩回必须有封面快照");
            let exit = PlayerExit {
                theme: t.clone(),
                source,
                visual: closing_visual,
                progress: 0.0,
            };
            exit.with_animation(
                format!("player-exit-{}", self.closing_generation),
                Animation::new(std::time::Duration::from_millis(PLAYER_EXIT_MS)).with_easing(|t| t),
                |el, progress| PlayerExit { progress, ..el },
            )
            .into_any_element()
        } else if let Some(source) = self.entrance_source.clone() {
            let entrance = PlayerEntrance {
                theme: t.clone(),
                source,
                content: Some(content),
                focus_handle: self.focus_handle.clone(),
                weak: cx.entity().downgrade(),
                progress: 0.0,
            };
            entrance
                .with_animation(
                    format!("player-entrance-{}", self.entrance_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_ENTRANCE_MS))
                        .with_easing(|t| t),
                    |el, progress| PlayerEntrance { progress, ..el },
                )
                .into_any_element()
        } else {
            div()
                .id("player-window")
                .absolute()
                .top(px(TOP_BAR_HEIGHT + SPACE_4))
                .left(px(SPACE_4))
                .right(px(SPACE_4))
                .h(px(PLAYER_FLOAT_HEIGHT))
                .rounded(px(CARD_RADIUS))
                .overflow_hidden()
                .bg(rgb(0x0B0F14))
                .border_1()
                .border_color(t.surfaces.border_popover)
                .shadow(t.shadows.shadow_glass_modal.clone())
                // 浮窗本体截断命中的穿透；子控件仍在更上层。
                .occlude()
                .key_context("Player")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key != "escape" {
                        return;
                    }
                    if this.escape_hint {
                        this.request_close(window, cx);
                    } else {
                        this.arm_escape_hint(cx);
                    }
                    cx.stop_propagation();
                }))
                .child(content)
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .child(backdrop)
            .child(window)
            // 第一次 Esc 提示放在浮窗之外：顶栏右侧。
            .children(self.escape_hint_overlay(&t, cx))
    }
}

// ================= 弹幕层 =================

impl PlayerView {
    /// 当前时刻该显示的滚动弹幕。
    ///
    /// 每帧只为「可见窗口」内的弹幕生成元素，不是全量 —— 一场直播可能有上万条，
    /// 全量遍历会直接掉帧。窗口前半段留 15 秒是因为一条弹幕最长要飞这么久。
    /// 把输入框里的文本作为一条弹幕发出去（本地回显，不接服务端）。
    fn send_danmaku(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.danmaku_input.as_ref() else {
            return;
        };
        let text = st.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }

        // 时间取当前播放位置（稍加提前量，让它马上进入视野而不是刚好在右边缘外）
        let time = self.play_time + 0.05;
        let d = Danmaku {
            time,
            mode: yk_core::danmaku::DanmakuMode::Scroll,
            font_size: 25.0,
            color: (0xFF, 0xFF, 0xFF),
            width: Danmaku::estimate_width(&text, 25.0),
            text,
        };
        // 分配轨道后再入列；新弹幕的时间一定比已有的大（当前时刻），
        // 所以追加到末尾不会破坏「从早到晚」的顺序。
        let lane = self.lanes.assign(&d);
        self.danmaku.push((d, lane));
        eprintln!(
            "[danmaku] 已发送，轨道 {lane}，共 {} 条",
            self.danmaku.len()
        );

        // 清空输入框（set_value 在 gpui-base 的 InputState 上，三参）
        if let Some(st) = self.danmaku_input.as_ref() {
            st.update(cx, |st, cx| st.set_value("", window, cx));
        }
        cx.notify();
    }

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
                    // 设计稿要求「1px 深色描边 + 投影」，而 ShapedLine 的
                    // 颜色烘在 layout 里、paint 不能重新着色。这里把同一条文本
                    // shape 两次：一次深色（描边/投影），一次原色（字身）。
                    // 描边色必须是深色（#0B0F14），白字 + 白描边在白画面上会糊。
                    let stroke = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: rgba(0x0B0F14D9).into(),
                        ..Default::default()
                    };
                    let stroke_line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[stroke],
                        None,
                    );
                    let line_height = px(d.size * 1.25);

                    // 投影：右下 1,2 px 先画一层，再画 8 方向 1px 描边。
                    let _ = stroke_line.paint(
                        point(px(d.x + 1.0), px(d.y + 2.0)),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                    for (dx, dy) in [
                        (-1.0, 0.0),
                        (1.0, 0.0),
                        (0.0, -1.0),
                        (0.0, 1.0),
                        (-1.0, -1.0),
                        (1.0, -1.0),
                        (-1.0, 1.0),
                        (1.0, 1.0),
                    ] {
                        let _ = stroke_line.paint(
                            point(px(d.x + dx), px(d.y + dy)),
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }

                    // 字身
                    let run = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: d.color,
                        ..Default::default()
                    };
                    let line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[run],
                        None,
                    );
                    let _ = line.paint(
                        point(px(d.x), px(d.y)),
                        line_height,
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
        "前方高能",
        "这也太真实了",
        "哈哈哈哈",
        "有一说一，确实",
        "第一次看到这种",
        "好家伙",
        "这个可以",
        "泪目",
        "up主加油",
        "妙啊",
        "学到了",
        "名场面",
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
fn video_layer(
    frame: &TestFrame,
    current: Option<&objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>>,
) -> impl IntoElement {
    // 有真实帧就用真实的，否则退回 spike 的测试帧
    let buffer = match current {
        Some(b) => b.clone(),
        None => frame.buffer(),
    };
    div()
        .absolute()
        .inset_0()
        // 等比 letterbox（L-2）：`surface()` 的 object_fit 默认就是 Contain，
        // 按 CVPixelBuffer 的真实尺寸缩放并居中留黑边。这里显式写出来，
        // 避免以后有人改默认值或误用 Fill 把画面拉变形。
        // 实测：4:3 片源在 1280×820 窗口里保持 4:3，左右留黑边。
        .child(surface(buffer).object_fit(ObjectFit::Contain).size_full())
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
    cx: &mut Context<PlayerView>,
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
                    Some(st) => {
                        // Input 元素本身不 track_focus，键盘事件不会路由过去。
                        // 必须由外层接管焦点：track_focus 让元素参与焦点系统，
                        // 点击时显式 window.focus() 把输入焦点交给它。
                        let handle = st.read(cx).focus_handle(cx);
                        // 记录聚焦状态需要在 PlayerView 上，这里通过回调设置
                        div()
                            .id("danmaku-input")
                            .flex_1()
                            .h(px(26.))
                            // Enter 的绑定是 `KeyBinding::new("enter", Enter, Some("Input"))`；
                            // 焦点落在这个外层 div 上，必须补 key_context 才能匹配到 Input 的 action。
                            .key_context("Input")
                            .track_focus(&handle)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let handle = handle.clone();
                                    move |view, _ev: &MouseDownEvent, window, cx| {
                                        // 已经聚焦就什么都不做 ——
                                        // 重复 window.focus + notify 会让 Input 重建，
                                        // 表现为「点一下已输入内容的输入框，内容被清掉」。
                                        if handle.is_focused(window) {
                                            return;
                                        }
                                        view.input_focused = true;
                                        window.focus(&handle, cx);
                                        cx.notify();
                                    }
                                }),
                            )
                            // Input 自己不撑满时，hitbox 只有文字那么宽 ——
                            // 点击空白处不会聚焦。这里让它撑满整条。
                            .child(
                                Input::new(st)
                                    .appearance(false)
                                    .bordered(false)
                                    .w_full()
                                    .h_full(),
                            )
                            .into_any_element()
                    }
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
                        .id("danmaku-send")
                        .px(px(SPACE_2))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(palette().theme_primary)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .cursor_pointer()
                        .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                            view.send_danmaku(window, cx);
                        }))
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
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
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
