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

use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::player::PlayerState;
use yk_ui::theme::*;

use crate::video::TestFrame;

/// 控制栏总高（唯一口径）。
const CTRL_H: f32 = PLAYER_CONTROL_BAR_HEIGHT; // 88
/// 弹幕输入条高。
const DANMAKU_INPUT_H: f32 = 38.0;
const INFO_PANEL_EXPANDED: f32 = PLAYER_INFO_PANEL_WIDTH; // 400
const INFO_PANEL_COLLAPSED: f32 = PLAYER_INFO_PANEL_COLLAPSED_WIDTH; // 200

pub struct PlayerView {
    theme: YkTheme,
    frame: TestFrame,
    /// 面板是否展开（hover 唤起）。
    expanded: bool,
    /// 当前 Tab：0 评论 / 1 弹幕 / 2 选集。
    tab: usize,
    /// 播放状态（音量/倍速/进度/全屏）全部来自 yk-core 的 PlayerState。
    state: PlayerState,
}

impl PlayerView {
    pub fn new(theme: YkTheme) -> Self {
        Self {
            theme,
            frame: TestFrame::new(1280, 720),
            expanded: true,
            tab: 0,
            state: {
                let mut st = PlayerState::new();
                st.duration_secs = 754.0;
                st.position_secs = 256.0;
                st.playing = true;
                st
            },
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        eprintln!("[M3] PlayerView::render 被调用");
        let t = self.theme.clone();

        // 浮窗是「独立场景」，必须绝对定位铺满最近的定位祖先（AppRoot 根 div 是 relative）。
        // 只写 size_full 不行：作为 children 加入时外层没有尺寸约束，会退化成 0×0，
        // 表现就是「渲染日志有、画面上什么都没有」。
        div()
            .absolute()
            .inset_0()
            .bg(rgb(0x000000))
            // ① 视频层：CVPixelBuffer 交给 GPUI 的 surface，与控件同管线合成
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(surface(self.frame.buffer()).size_full()),
            )
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
                deferred(control_bar(&t, &self.state, cx.listener(
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

// ================= 控制栏 =================

fn control_bar(
    t: &YkTheme,
    state: &PlayerState,
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
                .child(
                    div()
                        .flex_1()
                        .h(px(26.))
                        .px(px(SPACE_2))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0xFFFFFF1A))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(hsla(1.0, 0.0, 1.0, 0.55))
                        .child("发一条友善的弹幕吧"),
                )
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
