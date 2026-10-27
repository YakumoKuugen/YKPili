//! M4 组件展示页：验证「选用 gpui-component + 主题覆盖」这条路走通了。
//!
//! 这一页**不写任何颜色字面量** —— 所有颜色都来自组件读到的
//! `cx.theme()`，而 `cx.theme()` 已被 `yk_ui::gpui_theme::apply` 覆盖成项目令牌。
//! 所以：如果这页的颜色和设计稿一致，说明主题桥接是对的；
//! 如果出现了组件库默认的紫/蓝，说明某个字段没覆盖到。

use gpui::component::badge::Badge;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::tag::Tag;
use gpui::component::Disableable;
use gpui::component::Theme as GpuiTheme;
use gpui::prelude::*;
use gpui::*;
use yk_ui::theme::*;

pub struct ComponentGallery {
    theme: YkTheme,
}

impl ComponentGallery {
    pub fn new(theme: YkTheme) -> Self {
        Self { theme }
    }
}

impl Render for ComponentGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = GpuiTheme::global(cx);
        let muted_fg = t.colors.muted_foreground;
        let fg = t.colors.foreground;
        let p = &self.theme.palette;

        // 同 M3 的坑：作为 children 加入的子视图必须绝对定位，
        // 只写 size_full 会退化成 0×0（渲染日志正常、画面空白）。
        div()
            .absolute()
            .inset_0()
            .bg(t.colors.background)
            .p(px(SPACE_8))
            .flex()
            .flex_col()
            .gap(px(SPACE_6))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_1))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_HEADING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child("组件库 · M4 验收"),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(muted_fg)
                            .child("全部来自 gpui-component；颜色由主题桥接注入，本页无字面量色值"),
                    ),
            )
            // ---- 按钮 ----
            .child(group(
                "按钮",
                muted_fg,
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_3))
                    .child(Button::new("b-primary").label("主要按钮").primary())
                    .child(Button::new("b-secondary").label("次要按钮").secondary())
                    .child(Button::new("b-danger").label("危险操作").danger())
                    .child(Button::new("b-ghost").label("幽灵按钮").ghost())
                    .child(Button::new("b-outline").label("描边按钮").outline())
                    .child(Button::new("b-disabled").label("禁用按钮").disabled(true)),
            ))
            // ---- 状态色 ----
            .child(group(
                "状态色（徽标 / 标签）",
                muted_fg,
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_3))
                    .child(Badge::new().count(3).color(p.status_error))
                    .child(Badge::new().count(12).color(p.theme_primary))
                    .child(
                        div()
                            .px(px(SPACE_2))
                            .py(px(1.))
                            .rounded(px(RADIUS_SM))
                            .bg(p.status_info_10)
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(p.status_info)
                            .child("信息"),
                    )
                    .child(
                        div()
                            .px(px(SPACE_2))
                            .py(px(1.))
                            .rounded(px(RADIUS_SM))
                            .bg(p.status_success_10)
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(p.status_success)
                            .child("成功"),
                    )
                    .child(
                        div()
                            .px(px(SPACE_2))
                            .py(px(1.))
                            .rounded(px(RADIUS_SM))
                            .bg(p.status_warning_10)
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(p.status_warning)
                            .child("警告"),
                    )
                    .child(
                        div()
                            .px(px(SPACE_2))
                            .py(px(1.))
                            .rounded(px(RADIUS_SM))
                            .bg(p.status_error_10)
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(p.status_error)
                            .child("危险"),
                    )
                    .child(Tag::color(gpui::component::ColorName::Blue).child("分区")),
            ))
            // ---- 色板对照（验证令牌值本身） ----
            .child(group(
                "令牌色板",
                muted_fg,
                div().flex().flex_row().gap(px(SPACE_2)).children(
                    [
                        ("品牌蓝", p.theme_primary),
                        ("品牌粉", p.bili_pink),
                        ("信息", p.status_info),
                        ("成功", p.status_success),
                        ("警告", p.status_warning),
                        ("错误", p.status_error),
                    ]
                    .into_iter()
                    .map(|(name, color)| {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(SPACE_1))
                            .child(div().w(px(56.)).h(px(36.)).rounded(px(RADIUS_MD)).bg(color))
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_MICRO))
                                    .text_color(muted_fg)
                                    .child(name),
                            )
                    }),
                ),
            ))
            // ---- 表面层级 ----
            .child(group(
                "表面层级（Fill1–4 / 表面变体）",
                muted_fg,
                div().flex().flex_row().gap(px(SPACE_2)).children(
                    [
                        ("Fill1", self.theme.surfaces.fill_1),
                        ("Fill2", self.theme.surfaces.fill_2),
                        ("Fill3", self.theme.surfaces.fill_3),
                        ("Fill4", self.theme.surfaces.fill_4),
                        ("Content", self.theme.surfaces.surface_content),
                        ("Strong", self.theme.surfaces.surface_elevated_strong),
                    ]
                    .into_iter()
                    .map(|(name, color)| {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(SPACE_1))
                            .child(
                                div()
                                    .w(px(72.))
                                    .h(px(36.))
                                    .rounded(px(RADIUS_MD))
                                    .bg(color)
                                    .border_1()
                                    .border_color(self.theme.surfaces.border_subtle),
                            )
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_MICRO))
                                    .text_color(muted_fg)
                                    .child(name),
                            )
                    }),
                ),
            ))
    }
}

fn group(title: &str, muted_fg: Hsla, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_2))
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(muted_fg)
                .child(title.to_string()),
        )
        .child(body)
}
