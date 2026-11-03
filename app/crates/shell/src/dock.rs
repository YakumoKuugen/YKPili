//! 底部 Dock：全局一级导航 + 快捷动作。
//!
//! Dock 只决定「长什么样、有几个入口」；每个入口点了之后去哪、要不要先登录，
//! 由组装点通过 `on_select(index)` 处理。这样 shell 不持有路由与业务状态。

use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};

use yk_theme::theme::*;

/// 一个 Dock 入口。`active` / `enabled` / `unread` 由组装点按当前页面算好。
#[derive(Clone, Debug)]
pub struct DockItem {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: IconName,
    pub active: bool,
    pub enabled: bool,
    pub unread: bool,
}

impl DockItem {
    pub fn nav(id: &'static str, label: &'static str, icon: IconName, active: bool) -> Self {
        Self {
            id,
            label,
            icon,
            active,
            enabled: true,
            unread: false,
        }
    }

    pub fn action(
        id: &'static str,
        label: &'static str,
        icon: IconName,
        active: bool,
        enabled: bool,
        unread: bool,
    ) -> Self {
        Self {
            id,
            label,
            icon,
            active,
            enabled,
            unread,
        }
    }
}

/// 点击回调：拿到的是「导航项在前、动作项在后」的全局序号。
pub type DockSelect<V> = Rc<dyn Fn(&mut V, usize, &mut Window, &mut Context<V>)>;

/// 画 Dock。返回的元素已经绝对定位到窗口底部。
pub fn dock<V: 'static>(
    t: &YkTheme,
    nav: &[DockItem],
    actions: &[DockItem],
    cx: &mut Context<V>,
    on_select: DockSelect<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(px(SPACE_1))
        .p(px(6.))
        .rounded_full()
        // 与顶栏同一套材质：SurfaceToolbar 底 + BorderStrong 边 + 玻璃投影/内高光。
        // Dock 浮在封面之上，底色必须足够实（0.95），否则亮封面会透上来。
        .bg(s.surface_toolbar)
        .border_1()
        .border_color(s.border_strong)
        // 浮层必须挡住下层的封面：胶囊的 padding / 按钮间隙没有自己的
        // 点击处理器，不 occlude 的话点缝里会直接点开背后的封面。
        .occlude()
        .shadow(t.shadows.shadow_glass_elevated.clone());

    for (index, item) in nav.iter().enumerate() {
        let fg = if item.active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let bg = if item.active {
            p.theme_primary
        } else {
            hsla(0., 0., 0., 0.)
        };
        let hover_bg = if item.active { p.theme_primary } else { s.fill_2 };
        let on_select = on_select.clone();
        row = row.child(
            div()
                .id(item.id)
                .h(px(DOCK_ITEM_HEIGHT))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded_full()
                .bg(bg)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(if item.active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .child(Icon::new(item.icon).size(px(16.)).text_color(fg))
                .child(item.label)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    on_select(this, index, window, cx);
                })),
        );
    }

    // 导航与动作之间用 1px 分隔线。
    row = row.child(
        div()
            .w(px(1.))
            .h(px(20.))
            .mx(px(SPACE_1))
            .bg(s.border_popover),
    );

    let base = nav.len();
    for (offset, item) in actions.iter().enumerate() {
        let fg = if !item.enabled {
            s.text_quaternary
        } else if item.active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let hover_bg = if item.active { p.theme_primary } else { s.fill_2 };
        let hover_fg = if item.active {
            s.text_on_accent
        } else {
            s.text_primary
        };
        let mut action = div()
            .id(item.id)
            .h(px(DOCK_ITEM_HEIGHT))
            .px(px(SPACE_3))
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .rounded_full()
            .bg(if item.active {
                p.theme_primary
            } else {
                hsla(0., 0., 0., 0.)
            })
            .text_size(px(FONT_SIZE_CAPTION))
            .font_weight(if item.active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .text_color(fg)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .child(Icon::new(item.icon).size(px(16.)).text_color(fg))
                    .when(item.unread, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-2.))
                                .w(px(6.))
                                .h(px(6.))
                                .rounded_full()
                                .bg(p.bili_pink),
                        )
                    }),
            )
            .child(item.label);
        if item.enabled {
            action = action
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg).text_color(hover_fg));
            let on_select = on_select.clone();
            let index = base + offset;
            action = action.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                on_select(this, index, window, cx);
            }));
        }
        row = row.child(action);
    }

    div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .bottom(px(DOCK_BOTTOM_MARGIN))
        .px(px(SPACE_4))
        .flex()
        .justify_center()
        .child(row)
        .into_any_element()
}
