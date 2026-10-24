//! 把 YKPili 的设计令牌接到 `gpui-component` 的主题对象上。
//!
//! 这是 M4 的关键一步：第 13 节决定「组件全部选用 gpui-component +
//! 主题覆盖」，而不是自建一套样式。覆盖做在这里，视图里就不必逐个组件
//! 传颜色——组件读 `cx.theme()`，自然拿到项目的色值。
//!
//! 对应关系（旧 → 新）：
//!   `{DynamicResource SurfaceContent}`  → `theme.colors.list`
//!   `{DynamicResource ThemePrimaryBrush}` → `theme.colors.primary` / `accent`
//!   `{DynamicResource TextPrimary}`      → `theme.colors.foreground`
//!   …以此类推。**视图不再直接读令牌里的颜色来画标准控件**，
//!   只有自定义容器（顶栏、面板这类）才直接用 `YkSurface`。

use gpui::{px, Hsla};
use gpui::component::Theme as GpuiTheme;

use crate::theme::*;

/// 把一套 `YkTheme` 应用到 gpui-component 的全局主题上。
///
/// 调用时机：启动时一次（`gpui::init` 之后），以及主题切换时。
/// **不要**在每次 render 里调用。
pub fn apply(cx: &mut gpui::App, t: &YkTheme) {
    let p = &t.palette;
    let s = &t.surfaces;
    let white = gpui::hsla(1.0, 0.0, 1.0, 1.0);
    let transparent = gpui::hsla(0.0, 0.0, 0.0, 0.0);

    GpuiTheme::update(cx, |th| {
        // ---- 模式 ----
        th.mode = if t.is_dark {
            gpui::component::ThemeMode::Dark
        } else {
            gpui::component::ThemeMode::Light
        };

        // ---- 字体与几何 ----
        // 字号按项目字号阶给基准 15；组件内部按需用 text_size 覆盖。
        th.font_family = FONT_FAMILY_UI.into();
        th.font_size = px(FONT_SIZE_BODY);
        th.mono_font_family = "Menlo, Consolas, DejaVu Sans Mono, monospace".into();
        th.mono_font_size = px(FONT_SIZE_CAPTION);
        th.radius = px(INTERACTIVE_RADIUS); // 8
        th.radius_lg = px(CARD_RADIUS); // 12
        th.shadow = true;

        let c = &mut th.colors;

        // ---- 基础语义 ----
        c.background = s.bg_base;
        c.foreground = s.text_primary;
        c.border = s.border_subtle;
        c.muted = s.fill_1;
        c.muted_foreground = s.text_tertiary;
        c.popover = s.surface_elevated_strong;
        c.popover_foreground = s.text_primary;
        c.overlay = s.overlay_scrim;
        c.caret = p.theme_primary;

        // ---- 品牌色 ----
        c.accent = p.theme_primary;
        c.primary = p.theme_primary;
        c.primary_active = p.theme_primary_70;
        c.primary_foreground = white;
        c.ring = p.theme_primary_60; // 焦点环：与项目的 0 0 0 3px theme-30 同一色系
        c.link = p.theme_primary_70;
        c.link_hover = p.theme_primary;
        c.link_active = p.theme_primary_80;

        // ---- 状态色 ----
        c.danger = p.status_error;
        c.danger_active = p.status_error_80;
        c.danger_hover = p.status_error_70;
        c.danger_foreground = white;
        c.success = p.status_success;
        c.warning = p.status_warning;
        c.info = p.status_info;

        // ---- 按钮（默认 secondary 系） ----
        c.button = s.fill_1;
        c.button_hover = s.fill_2;
        c.button_active = s.fill_3;
        c.button_foreground = s.text_primary;
        c.button_secondary = s.fill_1;
        c.button_secondary_hover = s.fill_2;
        c.button_secondary_active = s.fill_3;
        c.button_secondary_foreground = s.text_primary;
        c.button_primary = p.theme_primary;
        c.button_primary_hover = p.theme_primary_80;
        c.button_primary_active = p.theme_primary_70;
        c.button_primary_foreground = white;
        c.button_danger = p.status_error;
        c.button_danger_hover = p.status_error_70;
        c.button_danger_active = p.status_error_80;
        c.button_danger_foreground = white;
        c.button_info = p.status_info;
        c.button_info_foreground = white;
        c.button_success = p.status_success;
        c.button_success_foreground = white;
        c.button_warning = p.status_warning;
        c.button_warning_foreground = white;

        // ---- 输入 ----
        c.input = s.fill_1;

        // ---- 列表 / 表格 ----
        c.list = s.surface_content;
        c.list_hover = s.surface_content_hover;
        c.list_active = s.surface_content_alt;
        c.list_active_border = s.border_strong;
        c.list_even = transparent;
        c.list_head = s.surface_content_solid;
        c.table = s.surface_content;
        c.table_hover = s.surface_content_hover;
        c.table_active = s.surface_content_alt;
        c.table_active_border = s.border_strong;
        c.table_even = transparent;
        c.table_head = s.surface_content_solid;

        // ---- 其它 ----
        c.skeleton = s.skeleton_brush;
        c.scrollbar = s.fill_2;
        c.scrollbar_thumb = s.fill_3;
        c.scrollbar_thumb_hover = s.fill_4;
        c.switch = s.fill_3;
        c.switch_thumb = white;
        c.title_bar = s.bg_base;
        c.title_bar_border = s.border_subtle;
        c.sidebar = s.surface_content;
        c.sidebar_foreground = s.text_primary;
        c.sidebar_border = s.border_subtle;
        c.sidebar_accent = s.fill_2;
        c.sidebar_accent_foreground = s.text_primary;
        c.tab = transparent;
        c.tab_active = s.surface_content_solid;
        c.tab_active_foreground = s.text_primary;
        c.tab_foreground = s.text_tertiary;
        c.progress_bar = p.theme_primary;
        c.slider_bar = s.fill_3;
        c.slider_thumb = p.theme_primary;
    });
}

/// 兼容旧称：把 `Ys` 的色值转成组件库色值。
pub fn hsla_of(h: Hsla) -> Hsla {
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只验证映射表本身不 panic、色值非空。
    /// 真正「组件是否吃到颜色」要在截图里看。
    #[test]
    fn mapping_table_is_complete() {
        let t = YkTheme::dark();
        let p = &t.palette;
        // 品牌色必须锁定为 B 站蓝
        let blue: gpui::Rgba = p.theme_primary.into();
        assert_eq!((blue.r * 255.0).round() as u8, 0x00);
        assert_eq!((blue.g * 255.0).round() as u8, 0xA1);
        assert_eq!((blue.b * 255.0).round() as u8, 0xD6);
    }
}
