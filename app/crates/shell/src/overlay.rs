//! 全局轻提示层：滚动刷新 / 加载更多 / 退出确认。
//!
//! 这些提示压在所有页面之上、但不属于某个能力页面，因此由 shell 提供
//! 统一的摆位与材质；文案和显示时机由组装点决定。

use gpui::prelude::*;
use gpui::*;
use yk_theme::theme::*;

/// 内容区上下的轻量状态胶囊。
///
/// `at_top = true` 时贴在顶栏下方（回顶刷新），否则贴在 Dock 上方（触底加载）。
pub fn status_pill(t: &YkTheme, text: &str, at_top: bool) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut layer = div()
        .absolute()
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .flex_shrink_0();
    layer = if at_top {
        layer.top(px(TOP_BAR_HEIGHT + SPACE_3))
    } else {
        layer.bottom(px(DOCK_RESERVED + SPACE_2))
    };
    layer
        .child(
            div()
                .h(px(30.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded_full()
                .bg(s.surface_elevated_strong)
                .border_1()
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_2.clone())
                .occlude()
                .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(p.theme_primary))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_secondary)
                        .child(text.to_string()),
                ),
        )
        .into_any_element()
}

/// 退出确认提示：压在播放器 / 登录 / 设置之上，只有视觉层，不挡点击。
pub fn quit_hint(t: &YkTheme, text: &str) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .min_w(px(520.))
                .px(px(SPACE_8))
                .py(px(SPACE_5))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_2XL))
                .bg(rgba(0x1E1E1EF2))
                .border_1()
                .border_color(rgba(0xFFFFFF14))
                .shadow(t.shadows.shadow_glass_elevated.clone())
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .text_center()
                        .child(text.to_string()),
                ),
        )
        .into_any_element()
}


/// 顶部离线横幅：文案与重试动作由组装点提供，shell 只负责摆位与材质。
pub fn offline_banner<V: 'static>(
    t: &YkTheme,
    text: &str,
    retrying: bool,
    cx: &mut Context<V>,
    on_retry: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .w_full()
        .h(px(36.))
        .px(px(SPACE_6))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .bg(s.surface_elevated_strong)
        .border_b_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(p.status_warning),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_secondary)
                .child(text.to_string()),
        )
        .child(
            div()
                .id("offline-retry")
                .h(px(24.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded(px(RADIUS_SM))
                .bg(if retrying { s.fill_2 } else { p.theme_primary })
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(if retrying {
                    s.text_tertiary
                } else {
                    rgb(0xFFFFFF).into()
                })
                .cursor_pointer()
                .child(if retrying { "重试中…" } else { "重试" })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    on_retry(this, window, cx);
                })),
        )
        .into_any_element()
}
