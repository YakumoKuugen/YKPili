//! 状态四态的统一实现（M6）。
//!
//! 第 05 节要求每个页面 / 每个列表 / 每张卡片都有「加载中 / 有内容 / 空 / 错误」。
//! M5 时四态是各页面各写一份，M6 把它收成公共组件，避免十个页面十种骨架。
//!
//! **骨架脉冲用 1.5s，不用组件库的 `Skeleton`**：
//! `gpui_component::skeleton` 把脉冲写死成 2s bounce 且只暴露 `secondary()`，
//! 而 1.5s 是设计节奏。详见设计文档附录 E · E-16。

use gpui::prelude::*;
use gpui::*;
use std::time::Duration;

use crate::theme::*;

/// 骨架脉冲周期（毫秒）。项目规范，不要改成组件库的 2000。
pub const SKELETON_PULSE_MS: u64 = 1500;

// 编译期断言：组件库的 Skeleton 写死 2000ms，项目是 1500ms。
// 用 const 断言而不是 #[test]，是为了避免 #[test] 宏在这个 crate 里展开过深。
const _: () = assert!(SKELETON_PULSE_MS == 1500, "骨架脉冲必须保持 1.5s");

/// 一条骨架行。
pub fn skeleton_row(theme: &YkTheme, height: f32, phase: f32) -> impl IntoElement {
    let base = theme.surfaces.skeleton_brush;
    // 用一个静态相位做错峰，避免所有行同频闪烁（观感很廉价）。
    let opacity = 0.35 + 0.30 * ((phase * std::f32::consts::PI).sin().abs());
    div()
        .h(px(height))
        .w_full()
        .rounded(px(CARD_RADIUS))
        .bg(base)
        .opacity(opacity)
}

/// 列表/页面的加载态。
pub fn skeleton(theme: &YkTheme, rows: usize) -> impl IntoElement {
    let brush = theme.surfaces.skeleton_brush;
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_3))
        .w_full()
        .with_animation(
            "yk-skeleton-pulse",
            Animation::new(Duration::from_millis(SKELETON_PULSE_MS)).repeat(),
            move |el, delta| {
                // delta 在 0..1 之间循环；用整体相位驱动各行
                el.children((0..rows).map(move |i| {
                    let phase = (delta + i as f32 * 0.15).fract();
                    let opacity = 0.35 + 0.30 * ((phase * std::f32::consts::PI).sin().abs());
                    div()
                        .h(px(56.))
                        .w_full()
                        .rounded(px(CARD_RADIUS))
                        .bg(brush)
                        .opacity(opacity)
                        .into_any_element()
                }))
            },
        )
}

/// 空态。
pub fn empty(theme: &YkTheme, title: &str, desc: &str) -> impl IntoElement {
    let s = &theme.surfaces;
    let title = title.to_string();
    let desc = desc.to_string();
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(320.))
        .w_full()
        .child(
            div()
                .text_size(px(28.))
                .text_color(s.text_quaternary)
                .child("○"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(desc),
        )
}

/// 错误态。
pub fn error(theme: &YkTheme, title: &str, desc: &str) -> impl IntoElement {
    let s = &theme.surfaces;
    let p = &theme.palette;
    let title = title.to_string();
    let desc = desc.to_string();
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(320.))
        .w_full()
        .child(
            div()
                .text_size(px(28.))
                .text_color(p.status_error)
                .child("!"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(desc),
        )
}

/// 内联加载指示（卡片级 / 局部刷新用）。
pub fn spinner(theme: &YkTheme) -> impl IntoElement {
    let color = theme.palette.theme_primary;
    div()
        .w(px(20.))
        .h(px(20.))
        .rounded_full()
        .bg(color)
        .opacity(0.2)
        .with_animation(
            "yk-spinner",
            Animation::new(Duration::from_millis(900)).repeat(),
            |el, delta| el.opacity(0.2 + 0.8 * delta),
        )
}

/// Toast 胶囊的统一高度。
///
/// 旧版各页面各写一份 30–34px，播放器那条还带缩放动画，视觉尺度不一致；
/// 统一成 48 高后面积约为旧版的 2 倍，字号也抬到正文档。
pub const TOAST_HEIGHT: f32 = 48.0;
/// Toast 胶囊的左右内边距。
pub const TOAST_PADDING_X: f32 = SPACE_4 + SPACE_2;
/// Toast 胶囊的最大宽度（超长文案先换行，不横向撑爆）。
pub const TOAST_MAX_WIDTH: f32 = 520.0;

/// 全局统一的 toast 胶囊：播放器互动反馈、搜索 / 消息提示、详情复制提示共用。
pub fn toast_pill(theme: &YkTheme, text: impl Into<String>) -> Div {
    let s = &theme.surfaces;
    div()
        .min_h(px(TOAST_HEIGHT))
        .max_w(px(TOAST_MAX_WIDTH))
        .px(px(TOAST_PADDING_X))
        .py(px(SPACE_2))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(s.surface_elevated_strong)
        .border_1()
        .border_color(s.border_popover)
        .shadow(theme.shadows.shadow_2.clone())
        .text_size(px(FONT_SIZE_BODY))
        .text_color(s.text_secondary)
        .child(text.into())
}

/// 评论「置顶」徽标：播放器评论区与动态详情评论区共用。
pub fn pinned_badge(theme: &YkTheme) -> Div {
    div()
        .h(px(14.))
        .px(px(4.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(theme.palette.theme_primary_20)
        .text_size(px(FONT_SIZE_MICRO))
        .line_height(px(14.))
        .text_color(theme.palette.theme_primary)
        .child("置顶")
}
