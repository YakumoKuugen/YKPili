//! 搜索页的纯渲染辅助：快捷键提示、骨架行、分区标题等。
//!
//! 只吃主题与静态数据，不接触 `AppRoot` / 搜索状态。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::input::Input;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_domain::format;
use yk_components::status;
use yk_theme::theme::*;

use crate::image::ImageResidency;
use yk_infra::search::{
    Hotword, SearchArticle, SearchLive, SearchMedia, SearchSort, SearchTab, SearchUser, SearchVideo,
    SuggestItem,
};

use crate::card::card;
use crate::search::{DurationFilter, SearchHost, SearchState};

/// 顶栏搜索框最大宽。
pub const SEARCH_BAR_WIDTH: f32 = TOP_BAR_SEARCH_WIDTH;

/// 综合页「影视」大卡的海报尺寸。
pub const MEDIA_POSTER_WIDTH: f32 = 190.0;
pub const MEDIA_POSTER_HEIGHT: f32 = 268.0;

/// 联想面板最大高度；超出滚动。
pub const SUGGEST_MAX_HEIGHT: f32 = 500.0;

/// 联想 / 历史 / 热搜行高。
pub const SUGGEST_ROW_HEIGHT: f32 = 44.0;
/// 结果缩略图（结果条目骨架与列表用）。
pub const RESULT_THUMB_WIDTH: f32 = 200.0;
pub const RESULT_THUMB_HEIGHT: f32 = 112.0;



pub fn kbd_hint(t: &YkTheme, key: &'static str, action: &'static str) -> AnyElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_quaternary)
        .child(
            div()
                .px(px(5.))
                .h(px(18.))
                .flex()
                .items_center()
                .rounded(px(RADIUS_SM))
                .bg(s.fill_2)
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_tertiary)
                .child(key),
        )
        .child(action)
        .into_any_element()
}

pub fn skeleton_row(t: &YkTheme, index: usize) -> AnyElement {
    let s = &t.surfaces;
    div()
        .h(px(SUGGEST_ROW_HEIGHT))
        .px(px(SPACE_5))
        .flex()
        .items_center()
        .child(
            div()
                .h(px(13.))
                .w(px(if index % 2 == 0 { 220. } else { 160. }))
                .rounded(px(RADIUS_SM))
                .bg(s.skeleton_brush),
        )
        .into_any_element()
}



pub fn section_title(t: &YkTheme, title: &'static str, meta: Option<String>) -> AnyElement {
    let s = &t.surfaces;
    div()
        .mt(px(SPACE_2))
        .flex()
        .items_baseline()
        .gap(px(SPACE_2))
        .w_full()
        .child(
            div()
                .text_size(px(FONT_SIZE_SECTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .child(title),
        )
        .children(meta.map(|meta| {
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child(meta)
        }))
        .into_any_element()
}

pub fn loading_rows(t: &YkTheme) -> AnyElement {
    let s = &t.surfaces;
    let mut col = div().flex().flex_col().gap(px(SPACE_3)).w_full();
    for _ in 0..3 {
        col = col.child(
            div()
                .flex()
                .gap(px(SPACE_4))
                .p(px(SPACE_3))
                .w_full()
                .max_w(px(760.))
                .rounded(px(CARD_RADIUS))
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .w(px(RESULT_THUMB_WIDTH))
                        .h(px(RESULT_THUMB_HEIGHT))
                        .rounded(px(MEDIA_RADIUS))
                        .bg(s.skeleton_brush),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_2))
                        .flex_1()
                        .child(
                            div()
                                .h(px(16.))
                                .w(relative(0.7))
                                .rounded(px(RADIUS_SM))
                                .bg(s.skeleton_brush),
                        )
                        .child(
                            div()
                                .h(px(12.))
                                .w(relative(0.4))
                                .rounded(px(RADIUS_SM))
                                .bg(s.skeleton_brush),
                        )
                        .child(
                            div()
                                .h(px(12.))
                                .w(relative(0.55))
                                .rounded(px(RADIUS_SM))
                                .bg(s.skeleton_brush),
                        ),
                ),
        );
    }
    col.into_any_element()
}

pub fn search_bar<V: SearchHost + 'static>(
    t: &YkTheme,
    state: &SearchState,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let Some(input) = state.input.as_ref() else {
        return div()
            .w_full()
            .max_w(px(SEARCH_BAR_WIDTH))
            .h(px(TOP_BAR_CONTROL_HEIGHT))
            .rounded_full()
            .bg(s.surface_elevated_solid)
            .border_1()
            .border_color(s.border_popover)
            .into_any_element();
    };
    let handle = input.read(cx).focus_handle(cx);
    let focused = handle.is_focused(window) || state.open;

    let mut shadow = t.shadows.shadow_1.clone();
    shadow.extend(t.shadows.edge_glow_1.clone());
    if focused {
        // 聚焦态：参考稿的「亮蓝色粗环 + 外圈柔光」。
        // 边框只有 1px，靠两段 BoxShadow 把环撑到约 2.5px：
        // 内段 1.5px 硬边跟着圆角走，外段 14px 柔光负责发光。
        shadow.push(BoxShadow::new(px(0.), px(0.), p.theme_primary).spread_radius(px(1.5)));
        shadow.push(
            BoxShadow::new(px(0.), px(0.), p.theme_primary_40)
                .blur_radius(px(14.))
                .spread_radius(px(1.)),
        );
    }

    let mut bar = div()
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .w_full()
        .max_w(px(SEARCH_BAR_WIDTH))
        .h(px(TOP_BAR_CONTROL_HEIGHT))
        .pl(px(SPACE_4))
        .pr(px(SPACE_2))
        .rounded_full()
        // 搜索框是顶栏唯一的大件：用语义上的「抬升实心面」，比工具栏底色亮一档。
        // 不借用 hover 色 —— 亮色主题下 hover 是 #D9D9D9，会让输入框发灰。
        // 聚焦时不再换底色（换色会让整块「跳」一下），只加主题色粗环 + 外圈柔光。
        .bg(s.surface_elevated_solid)
        .border_1()
        // 未聚焦时保留一圈很浅的高光边（不能像白色贴片，但要在深色工具栏上
        // 看得出这是可输入的控件）；聚焦时交给主题色粗环去抢注意力。
        .border_color(if focused {
            p.theme_primary
        } else {
            s.border_popover
        })
        .shadow(shadow)
        .occlude()
        .track_focus(&handle)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let handle = handle.clone();
                move |view, _: &MouseDownEvent, window, cx| {
                    if !handle.is_focused(window) {
                        window.focus(&handle, cx);
                    }
                    if !view.search_state_mut().open {
                        view.search_state_mut().open = true;
                        view.search_ensure_hotwords(cx);
                        if !view.search_state_mut().draft.trim().is_empty() {
                            view.search_schedule_suggest(cx);
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }),
        )
        // ↑↓ / Esc 由输入框外层先截住：光标不动，只移动联想高亮。
        .capture_key_down(cx.listener(|view, event: &KeyDownEvent, _window, cx| {
            if !view.search_state_mut().open {
                return;
            }
            let key = event.keystroke.key.as_str();
            if key == "escape" {
                if view.search_state_mut().selected.is_some() {
                    view.search_state_mut().selected = None;
                } else {
                    view.search_state_mut().open = false;
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if key != "down" && key != "up" {
                return;
            }
            let len = view.search_state_mut().suggestions.len();
            if len == 0 {
                return;
            }
            let next = match (key, view.search_state_mut().selected) {
                ("down", Some(index)) => (index + 1).min(len - 1),
                ("down", None) => 0,
                ("up", Some(0)) => 0,
                ("up", Some(index)) => index - 1,
                ("up", None) => len - 1,
                _ => 0,
            };
            view.search_state_mut().selected = Some(next);
            cx.stop_propagation();
            cx.notify();
        }));

    bar = bar.child(
        div()
            .flex_1()
            .min_w(px(0.))
            .child(Input::new(input).appearance(false).bordered(false).w_full()),
    );
    if !state.draft.is_empty() {
        bar = bar.child(
            div()
                .id("search-clear")
                .w(px(26.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2))
                .child(
                    Icon::new(IconName::X)
                        .size(px(14.))
                        .text_color(s.text_tertiary),
                )
                .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                    if let Some(input) = view.search_state_mut().input.clone() {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    view.search_state_mut().draft.clear();
                    view.search_state_mut().suggestions.clear();
                    view.search_state_mut().selected = None;
                    view.search_state_mut().open = true;
                    view.search_ensure_hotwords(cx);
                    cx.stop_propagation();
                    cx.notify();
                })),
        );
    }
    bar = bar.child(
        div()
            .id("search-go")
            .w(px(32.))
            .h(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|style| style.bg(p.theme_primary_20))
            .child(
                Icon::new(IconName::Search)
                    .size(px(19.))
                    .text_color(if focused {
                        p.theme_primary
                    } else {
                        s.text_secondary
                    }),
            )
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.search_submit_from_input(cx);
                cx.stop_propagation();
            })),
    );
    let bounds = state.bar_bounds.clone();
    div()
        .w_full()
        .flex()
        .justify_center()
        .on_children_prepainted(move |children, _window, _cx| {
            if let Some(bar) = children.first() {
                bounds.set(Some(*bar));
            }
        })
        .child(bar)
        .into_any_element()
}

pub fn suggest_overlay<V: SearchHost + 'static>(
    t: &YkTheme,
    state: &SearchState,
    window: &Window,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    if !state.open {
        return None;
    }
    let s = &t.surfaces;

    // 面板是一个挂在根层的浮层：贴着搜索框下沿 6px，宽度与搜索框一致。
    // 搜索框在顶栏里是被 logo / 头像夹住的 flex 居中，不是窗口正中；
    // 用 prepaint 记下的实际矩形定位，窗口缩放也不会错位。
    let bar = state.bar_bounds.get();
    let left = bar
        .map(|b| b.origin.x.as_f32())
        .unwrap_or_else(|| (window.viewport_size().width.as_f32() - SEARCH_BAR_WIDTH) / 2.0);
    let top = bar
        .map(|b| b.origin.y.as_f32() + b.size.height.as_f32() + 6.0)
        .unwrap_or(TOP_BAR_HEIGHT - 2.0);
    let width = bar
        .map(|b| b.size.width.as_f32())
        .unwrap_or(SEARCH_BAR_WIDTH)
        .max(320.0);

    let mut panel = div()
        .id("search-suggest-panel")
        .w(px(width))
        .max_h(px(SUGGEST_MAX_HEIGHT))
        .flex()
        .flex_col()
        .rounded(px(POPOVER_SURFACE_RADIUS))
        .bg(s.surface_elevated_solid)
        .border_1()
        .border_color(s.border_popover)
        .shadow(t.shadows.shadow_glass_modal.clone())
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
            }),
        );

    let body = if state.draft.trim().is_empty() {
        empty_query_body(t, state, cx)
    } else {
        suggestions_body(t, state, window, cx)
    };
    panel = panel.child(body);

    // 快捷键提示条：给「↑↓ / Enter / Esc」一个可见入口。
    panel = panel.child(
        div()
            .flex()
            .items_center()
            .gap(px(SPACE_4))
            .h(px(34.))
            .px(px(SPACE_4))
            .border_t_1()
            .border_color(s.border_subtle)
            .child(kbd_hint(t, "↑↓", "选择"))
            .child(kbd_hint(t, "Enter", "搜索"))
            .child(kbd_hint(t, "Esc", "关闭")),
    );

    Some(
        div()
            .absolute()
            .top(px(top))
            .left(px(left))
            .child(panel)
            .into_any_element(),
    )
}

pub fn suggestions_body<V: SearchHost + 'static>(
    t: &YkTheme,
    state: &SearchState,
    _window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let mut col = div()
        .id("search-suggest-list")
        .flex()
        .flex_col()
        .py(px(SPACE_2))
        .max_h(px(SUGGEST_MAX_HEIGHT - 34.))
        .overflow_y_scroll();

    if state.suggest_loading {
        col = col.children((0..3).map(|i| skeleton_row(t, i)));
        return col.into_any_element();
    }

    if let Some(error) = state.suggest_error.as_ref() {
        let _ = error;
        col = col.child(
            div()
                .px(px(SPACE_5))
                .py(px(SPACE_3))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("联想不可用，按 Enter 直接搜索"),
        );
        return col.into_any_element();
    }

    if state.suggestions.is_empty() {
        let query = state.draft.trim().to_string();
        let mut row = div()
            .id("search-suggest-empty")
            .h(px(SUGGEST_ROW_HEIGHT))
            .px(px(SPACE_5))
            .flex()
            .items_center()
            .gap(px(SPACE_3))
            .cursor_pointer()
            .hover(|style| style.bg(s.fill_1))
            .child(
                Icon::new(IconName::Search)
                    .size(px(16.))
                    .text_color(s.text_quaternary),
            )
            .child(
                div()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(s.text_secondary)
                    .child(format!("没有联想词，搜索「{query}」")),
            );
        row = row.on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
            if let Some(input) = view.search_state_mut().input.clone() {
                let query = query.clone();
                input.update(cx, |input, cx| input.set_value(query, window, cx));
            }
            view.search_submit(query.clone(), cx);
            window.refresh();
        }));
        return col.child(row).into_any_element();
    }

    for (index, item) in state.suggestions.iter().enumerate() {
        col = col.child(suggest_row(
            t,
            item,
            index,
            state.selected == Some(index),
            cx,
        ));
    }
    col.into_any_element()
}

pub fn suggest_row<V: SearchHost + 'static>(
    t: &YkTheme,
    item: &SuggestItem,
    index: usize,
    selected: bool,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let plain = item.plain();
    let value = item.value.clone();
    let runs: Vec<TextRun> = item
        .spans
        .iter()
        .map(|span| TextRun {
            len: span.text.len(),
            font: gpui::Font {
                weight: if span.highlighted {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                },
                ..Default::default()
            },
            color: if span.highlighted {
                p.bili_blue
            } else {
                s.text_secondary
            },
            ..Default::default()
        })
        .collect();

    let mut row = div()
        .id(("search-suggest", index))
        .relative()
        .h(px(SUGGEST_ROW_HEIGHT))
        .px(px(SPACE_5))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .cursor_pointer()
        .text_size(px(FONT_SIZE_BODY))
        .text_color(s.text_secondary);
    if selected {
        row = row
            .bg(s.fill_2)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(8.))
                    .bottom(px(8.))
                    .w(px(3.))
                    .rounded_full()
                    .bg(p.theme_primary),
            )
            .font_weight(FontWeight::SEMIBOLD);
    }
    row.hover(|style| style.bg(s.fill_1))
        .child(StyledText::new(plain).with_runs(runs))
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
            if let Some(input) = view.search_state_mut().input.clone() {
                let value = value.clone();
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            view.search_submit(value.clone(), cx);
            window.refresh();
        }))
        .into_any_element()
}

pub fn empty_query_body<V: SearchHost + 'static>(t: &YkTheme, state: &SearchState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let mut col = div()
        .id("search-suggest-empty-query")
        .flex()
        .flex_col()
        .py(px(SPACE_2))
        .max_h(px(SUGGEST_MAX_HEIGHT - 34.))
        .overflow_y_scroll();

    if !state.history.entries().is_empty() {
        col = col.child(section_head(
            t,
            IconName::Clock,
            "搜索历史",
            Some(("清空", 0)),
            cx,
        ));
        for entry in state.history.entries() {
            let term = entry.term.clone();
            col = col.child(history_row(t, &term, cx));
        }
    }

    if !state.hotwords.is_empty() {
        col = col.child(section_head(
            t,
            IconName::Flame,
            "bilibili 热搜",
            Some(("换一批", 1)),
            cx,
        ));
        for (index, hot) in state.hotwords.iter().enumerate() {
            col = col.child(hotword_row(t, hot, index, cx));
        }
    } else if state.hotwords_loading {
        col = col.child(
            div()
                .px(px(SPACE_5))
                .py(px(SPACE_3))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child("热搜加载中…"),
        );
    }

    if state.history.entries().is_empty() && state.hotwords.is_empty() && !state.hotwords_loading {
        col = col.child(
            div()
                .px(px(SPACE_5))
                .py(px(SPACE_4))
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_tertiary)
                .child("输入关键词开始搜索"),
        );
    }
    col.into_any_element()
}

pub fn history_row<V: SearchHost + 'static>(t: &YkTheme, term: &str, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let term_for_click = term.to_string();
    let term_for_delete = term.to_string();
    div()
        .id(SharedString::from(format!("search-history-{term}")))
        .h(px(SUGGEST_ROW_HEIGHT))
        .px(px(SPACE_5))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_1))
        .child(
            Icon::new(IconName::Clock)
                .size(px(15.))
                .text_color(s.text_quaternary),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child(term.to_string()),
        )
        .child(
            div()
                .id(SharedString::from(format!("search-history-del-{term}")))
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_3))
                .child(
                    Icon::new(IconName::X)
                        .size(px(12.))
                        .text_color(s.text_quaternary),
                )
                .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                    view.search_state_mut().history.remove(&term_for_delete);
                    window.refresh();
                    cx.stop_propagation();
                    cx.notify();
                })),
        )
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
            if let Some(input) = view.search_state_mut().input.clone() {
                let term = term_for_click.clone();
                input.update(cx, |input, cx| input.set_value(term, window, cx));
            }
            view.search_submit(term_for_click.clone(), cx);
            window.refresh();
        }))
        .into_any_element()
}

pub fn hotword_row<V: SearchHost + 'static>(t: &YkTheme, hot: &Hotword, index: usize, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let keyword = hot.keyword.clone();
    let number_color = match index {
        0 => p.status_error,
        1 => p.status_warning,
        2 => p.bili_pink,
        _ => s.text_quaternary,
    };
    let mut row = div()
        .id(("search-hot", index))
        .h(px(SUGGEST_ROW_HEIGHT))
        .px(px(SPACE_5))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_1))
        .child(
            div()
                .w(px(20.))
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(number_color)
                .child(hot.pos.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child(keyword.clone()),
        );
    if let Some(tag) = hot.tag() {
        row = row.child(
            div()
                .px(px(5.))
                .h(px(17.))
                .flex()
                .items_center()
                .rounded(px(RADIUS_SM))
                .bg(match hot.word_type {
                    4 => p.status_error_20,
                    7 => p.bili_pink,
                    _ => p.theme_primary_20,
                })
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(if hot.word_type == 7 {
                    rgb(0xFFFFFF).into()
                } else {
                    p.bili_pink
                })
                .child(tag),
        );
    }
    row.on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
        if let Some(input) = view.search_state_mut().input.clone() {
            let keyword = keyword.clone();
            input.update(cx, |input, cx| input.set_value(keyword, window, cx));
        }
        view.search_submit(keyword.clone(), cx);
        window.refresh();
    }))
    .into_any_element()
}

pub fn section_head<V: SearchHost + 'static>(
    t: &YkTheme,
    icon: IconName,
    title: &'static str,
    action: Option<(&'static str, u8)>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let mut head = div()
        .h(px(34.))
        .px(px(SPACE_5))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .child(Icon::new(icon).size(px(14.)).text_color(s.text_quaternary))
        .child(
            div()
                .flex_1()
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_tertiary)
                .child(title),
        );
    if let Some((label, kind)) = action {
        head = head.child(
            div()
                .id(SharedString::from(format!("search-section-action-{kind}")))
                .h(px(24.))
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .rounded(px(RADIUS_SM))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2).text_color(s.text_secondary))
                .child(label)
                .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                    match kind {
                        0 => {
                            view.search_state_mut().history.clear();
                        }
                        _ => view.search_reload_hotwords(cx),
                    }
                    cx.stop_propagation();
                    cx.notify();
                })),
        );
    }
    head.into_any_element()
}

pub fn page<V: SearchHost + 'static>(
    t: &YkTheme,
    state: &SearchState,
    hovered: Option<usize>,
    leaving: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(tabs_row(t, state, cx))
        .child(sort_row(t, state, cx));

    if state.filters_open {
        col = col.child(filters_row(t, state, cx));
    }

    if state.loading {
        col = col.child(loading_rows(t));
    } else if let Some(error) = state.error.as_ref() {
        col = col.child(error_state(t, error, &state.query, cx));
    } else if state.visible_empty() {
        col = col.child(empty_state(t, state, cx));
    } else {
        match state.tab {
            SearchTab::All => {
                if let Some(media) = state
                    .results
                    .films
                    .first()
                    .or_else(|| state.results.bangumi.first())
                {
                    col = col.child(section_title(t, "影视", None));
                    col = col.child(media_hero(t, media, state, active_images.clone(), cx));
                }
                if let Some(user) = state.results.users.first() {
                    col = col.child(section_title(t, "用户", None));
                    col = col.child(user_card(t, user, state, active_images.clone(), cx));
                }
                let videos = state.visible_videos();
                if !videos.is_empty() {
                    col = col.child(section_title(
                        t,
                        "视频",
                        Some(format!(
                            "共 {} 条",
                            state.num_results.max(videos.len() as u64)
                        )),
                    ));
                    col = col.child(video_grid(
                        t,
                        &videos,
                        hovered,
                        leaving,
                        hover_generation,
                        layout_bounds,
                        active_images.clone(),
                        window,
                        cx,
                    ));
                }
            }
            SearchTab::Video => {
                let videos = state.visible_videos();
                col = col.child(video_grid(
                    t,
                    &videos,
                    hovered,
                    leaving,
                    hover_generation,
                    layout_bounds,
                    active_images.clone(),
                    window,
                    cx,
                ));
            }
            SearchTab::Bangumi => {
                col = col.child(media_grid(
                    t,
                    &state.results.bangumi,
                    state,
                    active_images.clone(),
                    cx,
                ));
            }
            SearchTab::Film => {
                col = col.child(media_grid(
                    t,
                    &state.results.films,
                    state,
                    active_images.clone(),
                    cx,
                ));
            }
            SearchTab::User => {
                let mut list = div().flex().flex_col().gap(px(SPACE_3)).w_full();
                for user in &state.results.users {
                    list = list.child(user_card(t, user, state, active_images.clone(), cx));
                }
                col = col.child(list);
            }
            SearchTab::Live => {
                col = col.child(live_grid(t, state, active_images.clone(), cx));
            }
            SearchTab::Article => {
                col = col.child(article_list(t, state, active_images.clone(), cx));
            }
        }

        let paginated_tab = matches!(state.tab, SearchTab::All | SearchTab::Video);
        if paginated_tab && (state.has_more || state.loading_more || state.more_error.is_some()) {
            col = col.child(load_more(t, state, cx));
        } else if paginated_tab && !state.results.is_empty() {
            col = col.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(t.surfaces.text_quaternary)
                    .child(format!(
                        "已经到底了 · 共 {} 条",
                        state.num_results.max(state.results.videos.len() as u64)
                    )),
            );
        }
    }

    col.into_any_element()
}

pub fn tabs_row<V: SearchHost + 'static>(t: &YkTheme, state: &SearchState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let icons = [
        IconName::Search,
        IconName::CirclePlay,
        IconName::Tv,
        IconName::Film,
        IconName::User,
        IconName::Radio,
        IconName::Newspaper,
    ];
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(SPACE_2))
        .w_full();
    for (index, tab) in SearchTab::ALL.iter().enumerate() {
        let active = state.tab == *tab;
        let fg = if active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let bg = if active { p.theme_primary } else { s.fill_2 };
        let hover = if active { p.theme_primary } else { s.fill_3 };
        row = row.child(
            div()
                .id(("search-tab", index))
                .h(px(38.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded_full()
                .bg(bg)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(Icon::new(icons[index]).size(px(16.)).text_color(fg))
                .child(tab.label())
                .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                    if view.search_state_mut().tab != *tab {
                        view.search_state_mut().tab = *tab;
                        view.search_state_mut().filters_open = false;
                        view.search_reset_hover();
                        view.search_scroll_top();
                        // 切到「用户」才需要这批关注状态（综合页只露第一个用户）。
                        view.search_prefetch_follows(cx);
                        window.refresh();
                        cx.notify();
                    }
                })),
        );
    }
    row.into_any_element()
}

pub fn sort_row<V: SearchHost + 'static>(t: &YkTheme, state: &SearchState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut row = div().flex().items_center().gap(px(SPACE_2)).w_full().child(
        div()
            .mr(px(SPACE_1))
            .text_size(px(FONT_SIZE_CAPTION))
            .text_color(s.text_tertiary)
            .child("排序"),
    );
    for (index, sort) in SearchSort::ALL.iter().enumerate() {
        let active = state.sort == *sort;
        let fg = if active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let bg = if active { p.theme_primary } else { s.fill_2 };
        let hover = if active { p.theme_primary } else { s.fill_3 };
        row = row.child(
            div()
                .id(("search-sort", index))
                .h(px(30.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded(px(RADIUS_HALF))
                .bg(bg)
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(sort.label())
                .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                    view.search_state_mut().sort = *sort;
                    view.search_reset_hover();
                    view.search_scroll_top();
                    window.refresh();
                    cx.notify();
                })),
        );
    }
    row = row.child(div().flex_1()).child(
        div()
            .id("search-filters")
            .h(px(30.))
            .px(px(SPACE_3))
            .flex()
            .items_center()
            .gap(px(SPACE_1))
            .rounded(px(RADIUS_HALF))
            .bg(if state.filters_open {
                p.theme_primary_20
            } else {
                s.fill_2
            })
            .text_size(px(FONT_SIZE_CAPTION))
            .text_color(if state.filters_open {
                p.theme_primary
            } else {
                s.text_secondary
            })
            .cursor_pointer()
            .hover(|style| style.bg(s.fill_3))
            .child(
                Icon::new(IconName::SlidersHorizontal)
                    .size(px(14.))
                    .text_color(if state.filters_open {
                        p.theme_primary
                    } else {
                        s.text_tertiary
                    }),
            )
            .child("更多筛选")
            .child(
                Icon::new(if state.filters_open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronDown
                })
                .size(px(13.))
                .text_color(s.text_quaternary),
            )
            .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                view.search_state_mut().filters_open = !view.search_state_mut().filters_open;
                window.refresh();
                cx.notify();
            })),
    );
    row.into_any_element()
}

pub fn filters_row<V: SearchHost + 'static>(t: &YkTheme, state: &SearchState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(SPACE_2))
        .w_full()
        .p(px(SPACE_3))
        .rounded(px(CARD_RADIUS))
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .mr(px(SPACE_1))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("时长"),
        );
    for (index, filter) in DurationFilter::ALL.iter().enumerate() {
        let active = state.duration == *filter;
        row = row.child(
            div()
                .id(("search-duration", index))
                .h(px(28.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded_full()
                .bg(if active { p.theme_primary_20 } else { s.fill_1 })
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(if active {
                    p.theme_primary
                } else {
                    s.text_secondary
                })
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2))
                .child(filter.label())
                .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                    view.search_state_mut().duration = *filter;
                    view.search_reset_hover();
                    window.refresh();
                    cx.notify();
                })),
        );
    }
    row = row.child(div().flex_1()).child(
        div()
            .id("search-filters-reset")
            .h(px(28.))
            .px(px(SPACE_2))
            .flex()
            .items_center()
            .rounded(px(RADIUS_SM))
            .text_size(px(FONT_SIZE_CAPTION))
            .text_color(s.text_tertiary)
            .cursor_pointer()
            .hover(|style| style.bg(s.fill_1).text_color(s.text_secondary))
            .child("清除筛选")
            .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                view.search_state_mut().duration = DurationFilter::All;
                view.search_reset_hover();
                window.refresh();
                cx.notify();
            })),
    );
    row.into_any_element()
}

pub fn video_grid<V: SearchHost + 'static>(
    t: &YkTheme,
    videos: &[&SearchVideo],
    hovered: Option<usize>,
    leaving: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let _ = window;
    let mut grid = div().flex().flex_wrap().gap(px(SPACE_4)).w_full();
    for (index, video) in videos.iter().enumerate() {
        let item = video.to_cover_item(index);
        if !item.image_url.is_empty() {
            active_images.borrow_mut().mark(&item.cover_thumb_url());
        }
        grid = grid.child(card(
            t,
            &item,
            hovered == Some(index),
            leaving == Some(index),
            hover_generation,
            layout_bounds.clone(),
            true,
            active_images.clone(),
            cx,
        ));
    }
    grid.into_any_element()
}

pub fn media_hero<V: SearchHost + 'static>(
    t: &YkTheme,
    media: &SearchMedia,
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let selected = state
        .episode_selected
        .get(&media.media_id)
        .copied()
        .unwrap_or(0)
        .min(media.episodes.len().saturating_sub(1));
    let media_id = media.media_id;
    let has_episodes = !media.episodes.is_empty();
    let episode_label = media
        .episodes
        .get(selected)
        .map(|ep| {
            let index = ep.index_title.trim();
            if index.is_empty() {
                format!("第 {} 集", selected + 1)
            } else if index.chars().all(|c| c.is_ascii_digit()) {
                format!("第 {index} 集")
            } else {
                index.to_string()
            }
        })
        .unwrap_or_else(|| "选集".to_string());

    let poster = if let Some(image) = {
        if !media.cover_url.is_empty() {
            active_images.borrow_mut().mark(&media.cover_url);
        }
        yk_components::media::sized_image(
            &media.cover_url,
            MEDIA_POSTER_WIDTH,
            MEDIA_POSTER_HEIGHT,
        )
    } {
        image
    } else {
        div()
            .w(px(MEDIA_POSTER_WIDTH))
            .h(px(MEDIA_POSTER_HEIGHT))
            .bg(s.fill_3)
            .into_any_element()
    };

    let mut meta = media.meta_line();
    if let Some(score) = media.score {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&format!("{score:.1} 分"));
    }
    let mut right = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .gap(px(SPACE_2))
        .child(
            div()
                .text_size(px(FONT_SIZE_SECTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .line_clamp(2)
                .child(media.title.clone()),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(meta),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(FONT_SIZE_CONTROL))
                .line_height(px(LINE_HEIGHT_CONTROL))
                .text_color(s.text_secondary)
                .line_clamp(4)
                .child(media.description.clone()),
        );

    // 选集：按钮展开后在卡片内部铺一行集数 chip，不做浮层（GPUI 没有 z-index，
    // 浮层越出卡片会被下一段内容盖住）。
    let button_label = if media.episodes.is_empty() {
        media.index_show.clone()
    } else {
        episode_label
    };
    let open = state.episode_open == Some(media.media_id);
    right = right.child(
        div()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .child(
                div()
                    .id(("search-media-eps", media_id.max(0) as usize))
                    .w(px(240.))
                    .h(px(36.))
                    .px(px(SPACE_3))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(INTERACTIVE_RADIUS))
                    .bg(s.fill_1)
                    .border_1()
                    .border_color(s.border_subtle)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(s.text_secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_2))
                    .when(!has_episodes, |el| el.child(media.index_show.clone()))
                    .when(has_episodes, |el| el.child(button_label))
                    .child(
                        Icon::new(if open {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .size(px(14.))
                        .text_color(s.text_quaternary),
                    )
                    .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                        if view.search_state_mut().episode_open == Some(media_id) {
                            view.search_state_mut().episode_open = None;
                        } else {
                            view.search_state_mut().episode_open = Some(media_id);
                        }
                        window.refresh();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(("search-media-watch", media_id.max(0) as usize))
                    .h(px(36.))
                    .px(px(SPACE_5))
                    .flex()
                    .items_center()
                    .rounded(px(INTERACTIVE_RADIUS))
                    .bg(p.theme_primary)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_on_accent)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .child("立即观看")
                    .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                        view.search_notice("番剧 / 影视播放接口尚未接入".to_string(), cx);
                        cx.notify();
                    })),
            )
            .child(div().flex_1()),
    );

    if open && !media.episodes.is_empty() {
        let mut eps = div()
            .id("search-media-eps-list")
            .flex()
            .flex_wrap()
            .gap(px(SPACE_1))
            .max_h(px(96.))
            .overflow_y_scroll();
        for (index, ep) in media.episodes.iter().enumerate() {
            let active = index == selected;
            let label = if ep.index_title.trim().is_empty() {
                format!("{}", index + 1)
            } else {
                ep.index_title.clone()
            };
            eps = eps.child(
                div()
                    .id(SharedString::from(format!(
                        "search-ep-{}-{index}",
                        media_id
                    )))
                    .h(px(28.))
                    .px(px(SPACE_2))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS_SM))
                    .bg(if active { p.theme_primary_20 } else { s.fill_1 })
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(if active {
                        p.theme_primary
                    } else {
                        s.text_secondary
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_2))
                    .child(label)
                    .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                        view.search_state_mut().episode_selected.insert(media_id, index);
                        view.search_state_mut().episode_open = None;
                        window.refresh();
                        cx.notify();
                    })),
            );
        }
        right = right.child(eps);
    }

    div()
        .flex()
        .gap(px(SPACE_4))
        .p(px(SPACE_3))
        .w_full()
        .max_w(px(760.))
        .rounded(px(CARD_RADIUS))
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .relative()
                .w(px(MEDIA_POSTER_WIDTH))
                .h(px(MEDIA_POSTER_HEIGHT))
                .rounded(px(MEDIA_RADIUS))
                .overflow_hidden()
                .child(poster)
                .child(
                    div()
                        .absolute()
                        .left(px(SPACE_2))
                        .top(px(SPACE_2))
                        .px(px(SPACE_2))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0x000000B3))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child(if media.season_type_name.trim().is_empty() {
                            "影视".to_string()
                        } else {
                            media.season_type_name.clone()
                        }),
                ),
        )
        .child(right)
        .into_any_element()
}

pub fn media_grid<V: SearchHost + 'static>(
    t: &YkTheme,
    media: &[SearchMedia],
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut grid = div().flex().flex_wrap().gap(px(SPACE_4)).w_full();
    for item in media {
        let selected = state
            .episode_selected
            .get(&item.media_id)
            .copied()
            .unwrap_or(0);
        let cover = {
            if !item.cover_url.is_empty() {
                active_images.borrow_mut().mark(&item.cover_url);
            }
            yk_components::media::fill_image(&item.cover_url, 252.)
                .unwrap_or_else(|| div().w_full().h(px(252.)).bg(s.fill_3).into_any_element())
        };
        grid = grid.child(
            div()
                .id(SharedString::from(format!(
                    "search-media-{}",
                    item.media_id
                )))
                .w(px(180.))
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .cursor_pointer()
                .on_click(cx.listener({
                    let media_id = item.media_id;
                    move |view, _: &ClickEvent, _window, cx| {
                        view.search_state_mut().episode_selected.insert(media_id, selected);
                        view.search_notice("番剧 / 影视播放接口尚未接入".to_string(), cx);
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(252.))
                        .rounded(px(MEDIA_RADIUS))
                        .overflow_hidden()
                        .child(cover)
                        .child(
                            div()
                                .absolute()
                                .left(px(SPACE_2))
                                .top(px(SPACE_2))
                                .px(px(SPACE_2))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_SM))
                                .bg(rgba(0x000000B3))
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(rgb(0xFFFFFF))
                                .child(if item.is_film { "电影" } else { "番剧" }),
                        )
                        .children(item.score.map(|score| {
                            div()
                                .absolute()
                                .right(px(SPACE_2))
                                .bottom(px(SPACE_2))
                                .px(px(6.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_SM))
                                .bg(rgba(0x000000B3))
                                .text_size(px(FONT_SIZE_MICRO))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.bili_pink)
                                .child(format!("{score:.1}"))
                        })),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .text_color(s.text_primary)
                        .line_clamp(2)
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .line_clamp(1)
                        .child(item.episode_count_label()),
                ),
        );
    }
    grid.into_any_element()
}

pub fn user_card<V: SearchHost + 'static>(
    t: &YkTheme,
    user: &SearchUser,
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let followed = state.followed.contains(&user.mid);
    let follow_busy = state.follow_busy.contains(&user.mid);

    let avatar = {
        if !user.avatar_url.is_empty() {
            active_images.borrow_mut().mark(&user.avatar_url);
        }
        yk_components::avatar::Avatar::new(56.0)
            .url(user.avatar_url.clone())
            .initial(user.name.chars().next().map(|c| c.to_string()).unwrap_or_default())
            .background(p.theme_primary_20.into())
            .text_color(p.theme_primary)
            .build()
    };
    // 点头像进空间浮窗（12.9 节）：搜索结果里「这个人是谁」和「关注 TA」同样重要。
    let avatar = V::card_up_avatar(
        SharedString::from(format!("search-up-{}", user.mid)),
        user.mid,
        user.name.clone(),
        user.avatar_url.clone(),
        56.0,
        avatar,
        cx,
    );

    let head = div()
        .flex()
        .items_center()
        .gap(px(SPACE_3))
        .w_full()
        .child(avatar)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .min_w(px(0.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_1))
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(s.text_primary)
                                .child(user.name.clone()),
                        )
                        .child(
                            div()
                                .px(px(5.))
                                .h(px(17.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_SM))
                                .bg(p.bili_pink)
                                .text_size(px(FONT_SIZE_MICRO))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xFFFFFF))
                                .child(format!("LV{}", user.level.max(1))),
                        )
                        .when(!user.verify.is_empty(), |el| {
                            el.child(
                                div()
                                    .px(px(5.))
                                    .h(px(17.))
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .rounded(px(RADIUS_SM))
                                    .bg(p.theme_primary_20)
                                    .text_size(px(FONT_SIZE_MICRO))
                                    .text_color(p.theme_primary)
                                    .child(
                                        Icon::new(IconName::BadgeCheck)
                                            .size(px(11.))
                                            .text_color(p.theme_primary),
                                    )
                                    .child(user.verify.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_3))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(format!("粉丝：{}", format::count(user.fans)))
                        .child(format!("视频：{}", format::count(user.videos))),
                ),
        )
        .child(div().flex_1())
        .child(
            div()
                .id(("search-follow", user.mid.max(0) as usize))
                .h(px(34.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(if followed { s.fill_2 } else { p.theme_primary })
                .when(follow_busy, |el| el.opacity(0.55))
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if followed {
                    s.text_secondary
                } else {
                    s.text_on_accent
                })
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child(if follow_busy {
                    "处理中…"
                } else if followed {
                    "已关注"
                } else {
                    "+ 关注"
                })
                .on_click(cx.listener({
                    let mid = user.mid;
                    move |view, _: &ClickEvent, _window, cx| {
                        view.search_toggle_follow(mid, cx);
                    }
                })),
        );

    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(SPACE_3))
        .p(px(SPACE_4))
        .w_full()
        .rounded(px(CARD_RADIUS))
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        .child(head);

    if !user.sign.trim().is_empty() {
        card = card.child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .line_clamp(1)
                .child(user.sign.clone()),
        );
    }

    // 综合页的参考稿给 6 个最近投稿；接口的 `res` 只回 3 个，
    // 用本次搜索里同 mid 的视频补到 6 个（不额外发请求）。
    let mut previews: Vec<&SearchVideo> = user.previews.iter().collect();
    for video in &state.results.videos {
        if previews.len() >= 6 {
            break;
        }
        if video.mid == user.mid
            && !video.bvid.is_empty()
            && !previews.iter().any(|item| item.bvid == video.bvid)
        {
            previews.push(video);
        }
    }
    if !previews.is_empty() {
        let mut row = div().flex().flex_wrap().gap(px(SPACE_2)).w_full();
        for (index, video) in previews.iter().enumerate() {
            row = row.child(user_video_tile(
                t,
                video,
                user.mid,
                index,
                active_images.clone(),
                cx,
            ));
        }
        card = card.child(row);
    }

    card.into_any_element()
}

pub fn user_video_tile<V: SearchHost + 'static>(
    t: &YkTheme,
    video: &SearchVideo,
    owner_mid: i64,
    index: usize,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    // 封面走公共组件（与首页卡片 / UP 主浮窗同一份）：
    // 渐变卡面 + 真实封面 + 播放 / 弹幕 + 时长都在 `cover_visual` 里。
    if !video.cover_url.is_empty() {
        active_images.borrow_mut().mark(&video.cover_url);
    }
    let art = crate::cover::cover_art(&video.title, &video.cover_url, None, s.fill_3).into_any_element();
    let cover = crate::cover::cover_visual(
        t,
        crate::cover::CoverVisual {
            art,
            views: video.view_count,
            danmaku: video.danmaku_count,
            duration_secs: video.duration_secs,
            is_live: false,
            show_stats: true,
        },
        168.0 / 94.0,
    );
    let bvid = video.bvid.clone();
    div()
        .id(SharedString::from(format!(
            "search-user-video-{owner_mid}-{index}"
        )))
        .w(px(168.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .cursor_pointer()
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
            view.search_open_video(bvid.clone(), cx);
            window.refresh();
            cx.notify();
        }))
        .child(cover)
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .line_height(px(LINE_HEIGHT_CAPTION))
                .text_color(s.text_secondary)
                .line_clamp(2)
                .child(video.title.clone()),
        )
        .into_any_element()
}

pub fn live_grid<V: SearchHost + 'static>(
    t: &YkTheme,
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut grid = div().flex().flex_wrap().gap(px(SPACE_4)).w_full();
    for (index, live) in state.results.live.iter().enumerate() {
        let cover = if live.cover_url.is_empty() {
            div().w_full().h(px(117.)).bg(s.fill_3).into_any_element()
        } else {
            active_images.borrow_mut().mark(&live.cover_url);
            img(ImageSource::from(live.cover_url.clone()))
                .w_full()
                .h(px(117.))
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        };
        grid = grid.child(
            div()
                .id(("search-live", index))
                .w(px(COVER_CELL_WIDTH))
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .cursor_pointer()
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.search_notice("直播间播放接口尚未接入".to_string(), cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(117.))
                        .rounded(px(MEDIA_RADIUS))
                        .overflow_hidden()
                        .child(cover)
                        .child(
                            div()
                                .absolute()
                                .left(px(SPACE_2))
                                .top(px(SPACE_2))
                                .px(px(SPACE_2))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_SM))
                                .bg(p.bili_pink)
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(rgb(0xFFFFFF))
                                .child("直播中"),
                        )
                        .child(
                            div()
                                .absolute()
                                .right(px(SPACE_2))
                                .bottom(px(SPACE_2))
                                .px(px(4.))
                                .rounded(px(RADIUS_SM))
                                .bg(rgba(0x000000B3))
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(rgb(0xFFFFFF))
                                .child(format!("{} 人气", format::count(live.online))),
                        ),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(1)
                        .child(live.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .line_clamp(1)
                        .child(format!("{} · {}", live.uname, live.area_name)),
                ),
        );
    }
    grid.into_any_element()
}

pub fn article_list<V: SearchHost + 'static>(
    t: &YkTheme,
    state: &SearchState,
    _active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut list = div().flex().flex_col().gap(px(SPACE_2)).w_full();
    for (index, article) in state.results.articles.iter().enumerate() {
        list = list.child(
            div()
                .id(("search-article", index))
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(SPACE_4))
                .w_full()
                .rounded(px(CARD_RADIUS))
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .cursor_pointer()
                .hover(|style| style.bg(s.surface_content_hover))
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.search_notice("专栏阅读页尚未接入".to_string(), cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(1)
                        .child(article.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .line_clamp(2)
                        .child(article.description.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_3))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(article.author.clone())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .child(
                                    Icon::new(IconName::Eye)
                                        .size(px(12.))
                                        .text_color(p.theme_primary),
                                )
                                .child(format::count(article.view)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .child(
                                    Icon::new(IconName::ThumbsUp)
                                        .size(px(12.))
                                        .text_color(p.theme_primary),
                                )
                                .child(format::count(article.like)),
                        ),
                ),
        );
    }
    list.into_any_element()
}

pub fn load_more<V: SearchHost + 'static>(t: &YkTheme, state: &SearchState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let label = if state.loading_more {
        "加载中…"
    } else if state.more_error.is_some() {
        "加载失败，点击重试"
    } else {
        "加载更多"
    };
    div()
        .id("search-load-more")
        .w_full()
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(s.fill_1)
        .border_1()
        .border_color(s.border_subtle)
        .text_size(px(FONT_SIZE_CONTROL))
        .text_color(if state.more_error.is_some() {
            t.palette.status_error
        } else {
            s.text_secondary
        })
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2))
        .child(label)
        .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
            if view.search_state_mut().loading_more {
                return;
            }
            view.search_load_more(cx);
            window.refresh();
        }))
        .into_any_element()
}

pub fn error_state<V: SearchHost + 'static>(t: &YkTheme, error: &str, query: &str, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_3))
        .w_full()
        .py(px(64.))
        .child(
            Icon::new(IconName::CircleAlert)
                .size(px(32.))
                .text_color(p.status_error),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .child("搜索失败"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(format!("「{query}」：{error}")),
        )
        .child(
            div()
                .id("search-retry")
                .h(px(34.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_on_accent)
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child("重试")
                .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                    view.search_reload(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

pub fn empty_state<V: SearchHost + 'static>(t: &YkTheme, state: &SearchState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let message = if state.duration != DurationFilter::All && !state.results.videos.is_empty() {
        "当前时长筛选下没有结果".to_string()
    } else {
        format!("没有找到「{}」相关内容", state.query)
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_3))
        .w_full()
        .py(px(72.))
        .child(
            Icon::new(IconName::SearchX)
                .size(px(32.))
                .text_color(s.text_quaternary),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_primary)
                .child(message),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("换个关键词，或到分区页看看"),
        )
        .child(
            div()
                .id("search-empty-category")
                .h(px(34.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_on_accent)
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child("去分区页")
                .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                    view.search_go_category(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

/// 全局轻提示：动作尚未接入时给一个明确出口，而不是点了没反应。
pub fn notice(t: &YkTheme, text: &str) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(DOCK_RESERVED + SPACE_4))
        .flex()
        .justify_center()
        .child(status::toast_pill(t, text))
        .into_any_element()
}


