//! 观看历史页。
//!
//! 版式参考 B 站收藏夹页（2026-10-26 参考稿）：
//! 左侧是大封面卡片网格 + 顶部筛选 / 搜索 / 批量管理，右侧是一条固定的
//! 「日期分组」栏——收藏夹用文件夹分栏，历史没有文件夹，最自然的同构就是
//! 按天分组：`今天 / 昨天 / 10月21日 周二`。点日期只筛当前列表，不重新请求。
//!
//! 与首页卡片的差异：
//! - 首页卡关心播放量 / 弹幕数；历史卡关心「看到哪」——左下进度、右下时长，
//!   标题前是「已观看 / 已看完 / 直播」状态标签；
//! - 历史列表不需要虚拟列表：首屏 20 条、按需加载更多，行数远小于推荐流；
//! - 页面自带左右分栏，右栏不随左栏滚动，所以它**不接**外壳的
//!   `content-scroll`，由本模块自己管高度（见 `content_area` 的特判）。

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::input::{Input, InputState};
use gpui::component::{v_virtual_list, Icon, VirtualListScrollHandle};
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::history::{group_days, HistoryCursor, HistoryDay, HistoryEntry};
use yk_ui::status;
use yk_ui::theme::*;

use crate::{AppRoot, ImageResidency, DOCK_RESERVED};

/// 右侧日期栏宽度。参考稿是 320，这里取 300：1280 窗口下左栏还能放下 3 列。
const RAIL_W: f32 = 300.0;
/// 左栏卡片间距。与首页封面网格同口径（208 单元格 + 16 间距）。
const GRID_GAP: f32 = SPACE_4;
/// 行间距。
const ROW_GAP: f32 = SPACE_4;
/// 右栏封面拼贴高度。
const RAIL_BANNER_H: f32 = 112.0;
/// 批量操作条高度。
const BATCH_BAR_H: f32 = 52.0;
/// 搜索框高度。
const SEARCH_H: f32 = 38.0;

/// 历史页的内容类型筛选。右栏分段控件与顶部数据共用一套。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryFilter {
    All,
    Video,
    Bangumi,
    Live,
}

impl Default for HistoryFilter {
    fn default() -> Self {
        Self::All
    }
}

impl HistoryFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Video, Self::Bangumi, Self::Live];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Video => "视频",
            Self::Bangumi => "番剧",
            Self::Live => "直播",
        }
    }

    pub fn matches(self, entry: &HistoryEntry) -> bool {
        match self {
            Self::All => true,
            Self::Video => entry.business == "archive",
            Self::Bangumi => entry.business == "pgc",
            Self::Live => entry.business == "live",
        }
    }
}

/// 批量 / 清空操作的确认目标。
///
/// 打开确认框时就把 `kids` 冻结下来，避免确认期间列表刷新导致「删错一批」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryConfirm {
    DeleteSelected { kids: Vec<i64> },
    DeleteOne { kid: i64, title: String },
    ClearAll { count: usize },
}

impl HistoryConfirm {
    pub fn title(&self) -> String {
        match self {
            Self::DeleteSelected { kids, .. } => format!("删除选中的 {} 条记录？", kids.len()),
            Self::DeleteOne { .. } => "删除这条观看记录？".into(),
            Self::ClearAll { .. } => "清空全部观看历史？".into(),
        }
    }

    pub fn body(&self) -> String {
        match self {
            Self::DeleteSelected { kids } => {
                format!(
                    "将删除选中的 {} 条记录，并同步删除 B 站账号中的历史。此操作不可撤销。",
                    kids.len()
                )
            }
            Self::DeleteOne { title, .. } => {
                let title = if title.chars().count() > 42 {
                    let short: String = title.chars().take(42).collect();
                    format!("{short}…")
                } else {
                    title.clone()
                };
                format!("《{title}》将从历史中移除，并同步删除 B 站账号中的记录。此操作不可撤销。")
            }
            Self::ClearAll { count } => format!(
                "将删除 B 站账号中的全部观看历史（当前已加载 {count} 条）。此操作不可撤销。"
            ),
        }
    }

    fn kids(&self) -> Vec<i64> {
        match self {
            Self::DeleteSelected { kids, .. } => kids.clone(),
            Self::DeleteOne { kid, .. } => vec![*kid],
            Self::ClearAll { .. } => Vec::new(),
        }
    }
}

/// 历史页全部可变状态。放在 `AppRoot` 上，切页不丢筛选与滚动数据。
pub struct HistoryState {
    pub entries: Vec<HistoryEntry>,
    pub loading: bool,
    /// 是否成功拉过至少一页；区分「空历史」与「还没开始」。
    pub loaded: bool,
    pub error: Option<String>,
    pub cursor: Option<HistoryCursor>,
    pub has_more: bool,
    pub loading_more: bool,
    /// 请求代际：刷新 / 重新登录时只接受最后一次结果。
    pub generation: u64,
    pub filter: HistoryFilter,
    /// true = 最近观看在前；false = 最早观看在前。
    pub sort_desc: bool,
    pub query: String,
    /// 选中的日期分组键；None = 全部历史。
    pub day: Option<String>,
    pub batch: bool,
    pub selected: HashSet<i64>,
    pub search_input: Option<Entity<InputState>>,
    pub confirm: Option<HistoryConfirm>,
    pub delete_busy: bool,
    pub action_error: Option<String>,
    /// 左栏网格自己的滚动句柄；历史页不接外壳的 content-scroll。
    pub scroll: VirtualListScrollHandle,
}

impl Default for HistoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            loading: false,
            loaded: false,
            error: None,
            cursor: None,
            has_more: false,
            loading_more: false,
            generation: 0,
            filter: HistoryFilter::All,
            sort_desc: true,
            query: String::new(),
            day: None,
            batch: false,
            selected: HashSet::new(),
            search_input: None,
            confirm: None,
            delete_busy: false,
            action_error: None,
            scroll: VirtualListScrollHandle::new(),
        }
    }

    pub fn reset(&mut self) {
        let search_input = self.search_input.take();
        *self = Self::new();
        self.search_input = search_input;
    }

    /// 类型 / 搜索变化后，原来选中的日期可能已经不存在，回退到全部。
    fn reset_day_if_empty(&mut self) {
        let Some(day) = self.day.as_ref() else {
            return;
        };
        let still_present = self
            .entries
            .iter()
            .any(|e| &e.day_key() == day && self.filter.matches(e) && e.matches_query(&self.query));
        if !still_present {
            self.day = None;
        }
    }
}

/// 渲染历史页。`window` 只用来取视口宽（列数）与创建输入框焦点。
pub(crate) fn page(
    t: &YkTheme,
    state: &HistoryState,
    signed_in: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    // 历史页没有走首页那套「可视范围」计算，直接把这批卡片实际挂出的图
    // 全部登记为驻留：首屏 20 条 + 头像 + 右栏拼贴，量级可控。
    // 不登记的话，下一帧 `prune_page_images` 会把这些图立刻 drop 掉，
    // 表现为「日志里图片 200，界面上只有占位色」。
    {
        let mut residency = active_images.borrow_mut();
        for entry in &state.entries {
            residency.mark(&entry.cover_thumb_url());
            let face = entry.face_thumb_url();
            if !face.is_empty() {
                residency.mark(&face);
            }
        }
    }

    let viewport_w = window.viewport_size().width.as_f32();
    let content_w = (viewport_w - SPACE_6 * 2.0 - RAIL_W - SPACE_5).max(COVER_CELL_WIDTH);
    // 与首页同一套几何：列数 = 可用宽能放下几个 208 + 16 的槽位。
    let cols = (((content_w + GRID_GAP) / (COVER_CELL_WIDTH + GRID_GAP)).floor() as usize).max(1);
    let card_w = COVER_CELL_WIDTH;

    // 显式算高度，不依赖 flex 的隐式收缩：历史页左栏是滚动容器，
    // 一旦主行拿到的是内容高度，卡片就会一直铺到 Dock 底下。
    // 固定项：上内边距 16 + 标题 28 + 工具条上间距 16 + 工具条 38
    // + 主行上间距 16 + 底部 Dock 占位 96。
    let page_h = (window.viewport_size().height.as_f32() - TOP_BAR_HEIGHT).max(320.0);
    let main_h = (page_h
        - SPACE_4
        - LINE_HEIGHT_HEADING
        - SPACE_4
        - SEARCH_H
        - SPACE_4
        - DOCK_RESERVED)
        .max(240.0);

    // 过滤：类型 + 搜索 + 日期。日期栏的 counts 只算类型 + 搜索，
    // 这样「今天 5 条」不会被已经选中的日期影响。
    let base: Vec<&HistoryEntry> = state
        .entries
        .iter()
        .filter(|e| state.filter.matches(e) && e.matches_query(&state.query))
        .collect();
    let mut visible: Vec<&HistoryEntry> = base
        .iter()
        .copied()
        .filter(|e| state.day.as_ref().map_or(true, |day| &e.day_key() == day))
        .collect();
    visible.sort_by(|a, b| {
        if state.sort_desc {
            b.view_at.cmp(&a.view_at)
        } else {
            a.view_at.cmp(&b.view_at)
        }
    });
    let days = group_days(
        &base
            .iter()
            .map(|e| (*e).clone())
            .collect::<Vec<HistoryEntry>>(),
    );

    let root = div()
        .relative()
        .h(px(page_h))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .px(px(SPACE_6))
        .pt(px(SPACE_4))
        .child(header(t, state))
        .child(toolbar(t, state, window, cx))
        .child(
            div()
                .flex()
                .h(px(main_h))
                .flex_shrink_0()
                .min_h(px(0.))
                .gap(px(SPACE_5))
                .mt(px(SPACE_4))
                .child(
                    // 父层再夹一次：GPUI 的滚动容器在部分情况下会把背景/内容
                    // 画到布局框之外，只靠 `overflow_y_scroll` 会漏到 Dock 下面。
                    // 这里用固定高的 `overflow_hidden` 做硬裁剪。
                    div()
                        .flex_1()
                        .h(px(main_h))
                        .min_w(px(0.))
                        .overflow_hidden()
                        .child(grid_column(
                            t, state, &visible, cols, card_w, content_w, main_h, signed_in,
                            cx,
                        )),
                )
                .child(rail(t, state, &days, &base, signed_in, cx)),
        )
        // 显式占位而不是 padding：padding 在 size_full 的容器上可能把内容
        // 顶出可视区，最后一行会钻到 Dock 底下。用一个固定高度 child 收口。
        .child(div().h(px(DOCK_RESERVED)));

    // 批量条浮在左栏底部、Dock 之上。右栏有自己的底部操作，不被它盖住。
    let root = if state.batch {
        root.child(
            div()
                .absolute()
                .left(px(SPACE_6))
                .right(px(RAIL_W + SPACE_5 + SPACE_6))
                .bottom(px(DOCK_RESERVED - 12.0))
                .flex()
                .justify_center()
                .child(batch_bar(t, state, cx)),
        )
    } else {
        root
    };
    root.into_any_element()
}

// ================= 头部 / 工具条 =================

fn header(t: &YkTheme, state: &HistoryState) -> AnyElement {
    let s = &t.surfaces;
    let total = state.entries.len();
    let sync = state
        .entries
        .iter()
        .max_by_key(|e| e.view_at)
        .map(|e| e.watched_text())
        .unwrap_or_default();
    let mut title = div().flex().items_end().gap(px(SPACE_2)).child(
        div()
            .text_size(px(FONT_SIZE_DISPLAY))
            .line_height(px(LINE_HEIGHT_HEADING))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(s.text_primary)
            .child("观看历史"),
    );
    if total > 0 {
        title = title.child(
            div()
                .pb(px(1.))
                .text_size(px(FONT_SIZE_HEADING))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_quaternary)
                .child(format!("({total})")),
        );
    }
    div()
        .flex()
        .items_end()
        .justify_between()
        .w_full()
        .child(title)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(
                    Icon::new(IconName::Cloud)
                        .size(px(14.))
                        .text_color(s.text_quaternary),
                )
                .child(if sync.is_empty() {
                    "登录后与 B 站同步".to_string()
                } else {
                    format!("已同步 B 站 · 最近 {sync}")
                }),
        )
        .into_any_element()
}

fn toolbar(
    t: &YkTheme,
    state: &HistoryState,
    window: &Window,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let sort_label = if state.sort_desc {
        "最近观看"
    } else {
        "最早观看"
    };
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .mt(px(SPACE_4))
        .w_full()
        .child(
            div()
                .id("hist-sort")
                .h(px(SEARCH_H))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(t.surfaces.surface_elevated)
                .border_1()
                .border_color(t.surfaces.border_subtle)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(t.surfaces.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(t.surfaces.surface_elevated_hover))
                .child(sort_label)
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(px(14.))
                        .text_color(t.surfaces.text_quaternary),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.history.sort_desc = !this.history.sort_desc;
                    this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    window.refresh();
                    cx.notify();
                })),
        )
        .child(search_box(t, state, window, cx))
        .child(
            div()
                .id("hist-search")
                .w(px(SEARCH_H))
                .h(px(SEARCH_H))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(t.surfaces.surface_elevated)
                .border_1()
                .border_color(t.surfaces.border_subtle)
                .cursor_pointer()
                .hover(|style| style.bg(t.surfaces.surface_elevated_hover))
                .child(
                    Icon::new(IconName::Search)
                        .size(px(16.))
                        .text_color(t.surfaces.text_tertiary),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    if let Some(input) = this.history.search_input.clone() {
                        let handle = input.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
                    }
                })),
        )
        .child(div().flex_1())
        .child(batch_button(t, state, cx))
        .into_any_element()
}

fn search_box(
    t: &YkTheme,
    state: &HistoryState,
    _window: &Window,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let Some(input) = state.search_input.as_ref() else {
        return div()
            .flex_1()
            .max_w(px(360.))
            .h(px(SEARCH_H))
            .rounded(px(INTERACTIVE_RADIUS))
            .bg(s.surface_elevated)
            .border_1()
            .border_color(s.border_subtle)
            .into_any_element();
    };
    let handle = input.read(cx).focus_handle(cx);
    let mut wrapper = div()
        .flex_1()
        .max_w(px(360.))
        .h(px(SEARCH_H))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(s.surface_elevated)
        .border_1()
        .border_color(s.border_subtle)
        .track_focus(&handle)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let handle = handle.clone();
                move |_view, _: &MouseDownEvent, window, cx| {
                    if !handle.is_focused(window) {
                        window.focus(&handle, cx);
                    }
                }
            }),
        )
        .child(
            div().flex_1().min_w(px(0.)).h_full().child(
                Input::new(input)
                    .appearance(false)
                    .bordered(false)
                    .w_full()
                    .h_full(),
            ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_quaternary)
                .when(!state.query.is_empty(), |el| el.child("实时筛选")),
        );
    // 清除按钮：有内容时才出现，避免空搜索框右边挂一个死按钮。
    if !state.query.trim().is_empty() {
        wrapper = wrapper.child(
            div()
                .id("hist-search-clear")
                .w(px(18.))
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2))
                .child(
                    Icon::new(IconName::X)
                        .size(px(12.))
                        .text_color(s.text_tertiary),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    if let Some(input) = this.history.search_input.clone() {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    this.history.query.clear();
                    this.history.selected.clear();
                    this.history.reset_day_if_empty();
                    this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    window.refresh();
                    cx.notify();
                })),
        );
    }
    wrapper.into_any_element()
}

fn batch_button(t: &YkTheme, state: &HistoryState, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let active = state.batch;
    div()
        .id("hist-batch")
        .h(px(SEARCH_H))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(if active {
            p.theme_primary_20
        } else {
            s.surface_elevated
        })
        .border_1()
        .border_color(if active {
            p.theme_primary_50
        } else {
            s.border_subtle
        })
        .text_size(px(FONT_SIZE_CONTROL))
        .text_color(if active {
            p.theme_primary
        } else {
            s.text_secondary
        })
        .cursor_pointer()
        .hover(|style| style.bg(s.surface_elevated_hover))
        .child(
            Icon::new(if active {
                IconName::Check
            } else {
                IconName::ListChecks
            })
            .size(px(15.))
            .text_color(if active {
                p.theme_primary
            } else {
                s.text_tertiary
            }),
        )
        .child(if active { "完成" } else { "批量管理" })
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.history.batch = !this.history.batch;
            this.history.selected.clear();
            window.refresh();
            cx.notify();
        }))
        .into_any_element()
}

// ================= 左栏：卡片网格 =================

fn grid_column(
    t: &YkTheme,
    state: &HistoryState,
    visible: &[&HistoryEntry],
    cols: usize,
    card_w: f32,
    width: f32,
    height: f32,
    signed_in: bool,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    if state.loading && !state.loaded {
        return div()
            .flex()
            .flex_col()
            .flex_1()
            .h(px(height))
            .min_w(px(0.))
            .gap(px(ROW_GAP))
            .children((0..2).map(|row| {
                div()
                    .flex()
                    .gap(px(GRID_GAP))
                    .children((0..cols).map(|col| skeleton_card(t, card_w, row * cols + col)))
            }))
            .into_any_element();
    }

    if !signed_in {
        return div()
            .flex()
            .flex_col()
            .flex_1()
            .h(px(height))
            .min_w(px(0.))
            .child(status::empty(
                t,
                "登录后查看观看历史",
                "登录 B 站账号后，这里会同步你在各端看过的视频",
            ))
            .into_any_element();
    }

    if let Some(error) = state.error.as_deref() {
        if state.entries.is_empty() {
            return error_state(t, error, cx);
        }
    }

    if visible.is_empty() {
        let filtered = !state.query.trim().is_empty()
            || state.filter != HistoryFilter::All
            || state.day.is_some();
        return empty_state(t, filtered, cx);
    }

    let rows = visible.len().div_ceil(cols);
    let refresh_error = state.error.as_deref().filter(|_| !state.entries.is_empty());
    // 虚拟列表：历史页左栏是独立滚动区，按行给尺寸，只画可视行。
    // 固定行高 = 卡片 184 + 行距 16；最后一项是「加载更早」footer。
    let row_h = COVER_CELL_HEIGHT + ROW_GAP;
    let footer_h = 76.0;
    let mut sizes = Vec::with_capacity(rows + 1);
    for _ in 0..rows {
        sizes.push(size(px(width), px(row_h)));
    }
    sizes.push(size(px(width), px(footer_h)));

    let entries: Rc<Vec<HistoryEntry>> = Rc::new(visible.iter().map(|e| (*e).clone()).collect());
    let theme = t.clone();
    let scroll = state.scroll.clone();
    let entity = cx.entity();
    let list = v_virtual_list(
        entity,
        "history-grid",
        Rc::new(sizes),
        move |this, visible_rows, _window, cx| {
            visible_rows
                .map(|row| {
                    if row >= rows {
                        return footer(&theme, &this.history, cx);
                    }
                    let start = row * cols;
                    let end = (start + cols).min(entries.len());
                    div()
                        .flex()
                        .justify_center()
                        .gap(px(GRID_GAP))
                        .w_full()
                        .h(px(row_h))
                        .children((start..end).map(|i| {
                            card(&theme, &this.history, &entries[i], i, card_w, cx)
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        },
    )
    .track_scroll(&scroll)
    .flex_1()
    .min_h(px(0.))
    .w_full();

    div()
        .flex()
        .flex_col()
        .h(px(height))
        .w(px(width))
        .children(refresh_error.map(|error| error_banner(t, error)))
        .child(list)
        .into_any_element()
}

fn error_banner(t: &YkTheme, error: &str) -> AnyElement {
    let p = &t.palette;
    div()
        .h(px(36.))
        .w_full()
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(p.status_error_10)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(p.status_error)
        .child(
            Icon::new(IconName::CircleAlert)
                .size(px(14.))
                .text_color(p.status_error),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(format!("刷新失败：{error}（列表保留上次结果）")),
        )
        .into_any_element()
}

fn footer(t: &YkTheme, state: &HistoryState, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let mut row = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(SPACE_2))
        .pt(px(SPACE_5))
        .pb(px(SPACE_5));
    if state.has_more {
        row = row.child(
            div()
                .id("hist-load-more")
                .h(px(36.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(SPACE_2))
                .rounded_full()
                .bg(s.surface_elevated)
                .border_1()
                .border_color(s.border_subtle)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(s.surface_elevated_hover))
                .child(if state.loading_more {
                    "正在加载…"
                } else {
                    "加载更早的记录"
                })
                .when(!state.loading_more, |el| {
                    el.child(
                        Icon::new(IconName::ChevronDown)
                            .size(px(14.))
                            .text_color(s.text_quaternary),
                    )
                })
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.load_more_history(cx);
                    window.refresh();
                })),
        );
    } else {
        row = row.child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child("已经到底了"),
        );
    }
    row.into_any_element()
}

fn error_state(t: &YkTheme, error: &str, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex()
        .flex_col()
        .flex_1()
        .h_full()
        .min_w(px(0.))
        .items_center()
        .justify_center()
        .gap(px(SPACE_3))
        .child(
            div()
                .w(px(44.))
                .h(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(p.status_error_10)
                .child(
                    Icon::new(IconName::CircleAlert)
                        .size(px(22.))
                        .text_color(p.status_error),
                ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_primary)
                .child("观看历史加载失败"),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .text_center()
                .child(error.to_string()),
        )
        .child(
            div()
                .id("hist-retry")
                .h(px(34.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child("重试")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.load_history(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn empty_state(t: &YkTheme, filtered: bool, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let (title, desc, action) = if filtered {
        (
            "没有匹配的观看记录",
            "换个关键词，或点右侧的日期与类型看看",
            "清除筛选",
        )
    } else {
        (
            "还没有观看记录",
            "看过的视频会按天出现在这里，方便随时接着看",
            "去首页逛逛",
        )
    };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .child(
            div()
                .w(px(52.))
                .h(px(52.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(s.fill_1)
                .child(
                    Icon::new(IconName::Clock)
                        .size(px(24.))
                        .text_color(s.text_quaternary),
                ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_secondary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(desc),
        )
        .child(
            div()
                .id("hist-empty-action")
                .mt(px(SPACE_2))
                .h(px(32.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(s.surface_elevated)
                .border_1()
                .border_color(s.border_subtle)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(s.surface_elevated_hover))
                .child(action)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if filtered {
                        this.history.filter = HistoryFilter::All;
                        this.history.query.clear();
                        this.history.day = None;
                        this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        if let Some(input) = this.history.search_input.clone() {
                            input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    } else {
                        this.page = None;
                        this.scroll_content_to_top();
                    }
                    window.refresh();
                    cx.notify();
                })),
        )
        .into_any_element()
}

// ================= 单张历史卡 =================

fn card(
    t: &YkTheme,
    state: &HistoryState,
    entry: &HistoryEntry,
    index: usize,
    card_w: f32,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let cover_h = COVER_ITEM_HEIGHT;
    let kid = entry.kid;
    let bvid = entry.bvid.clone();
    let title = entry.title.clone();
    let cover_url = entry.cover_thumb_url();
    let selected = state.selected.contains(&kid);
    let playable = entry.is_playable();
    let batch = state.batch;
    let progress = entry.progress_percent();
    let state_label = entry.state_label();
    let progress_label = entry.progress_label();
    let duration = entry.duration_text();
    let live = entry.is_live();
    let watched = entry.watched_text();
    let author = entry.author_name.clone();
    let face = entry.face_thumb_url();
    let shadow = t.shadows.shadow_2.clone();
    let hover_border = s.border_popover;
    let hover_bg = s.surface_content;
    let title_color = s.text_primary;

    let mut cover = div()
        .relative()
        .w_full()
        .h(px(cover_h))
        .rounded(px(MEDIA_RADIUS))
        .overflow_hidden()
        .bg(s.fill_3)
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(SPACE_3))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(15.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .text_center()
                        .line_clamp(3)
                        .opacity(0.85)
                        .child(title.clone()),
                ),
        );
    if !cover_url.is_empty() {
        cover = cover.child(
            img(ImageSource::from(cover_url))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }

    // 底部压暗：数据层在亮封面上也要读得清。
    cover = cover.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(52.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgba(0x00000000), 0.),
                linear_color_stop(rgba(0x000000C4), 1.),
            )),
    );

    if live {
        cover = cover.child(
            div()
                .absolute()
                .top(px(8.))
                .left(px(8.))
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(3.))
                .rounded(px(RADIUS_SM))
                .bg(p.bili_pink)
                .text_size(px(FONT_SIZE_MICRO))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .child("直播"),
        );
    }

    // 进度条：只画在封面内侧，避免和圆角打架。
    if progress > 0 && !live {
        cover = cover.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(3.))
                .bg(rgba(0xFFFFFF33))
                .child(div().h_full().w(relative(progress as f32 / 100.0)).bg(
                    if progress >= 100 {
                        p.status_success
                    } else {
                        p.theme_primary
                    },
                )),
        );
    }

    if !progress_label.is_empty() {
        cover = cover.child(
            div()
                .absolute()
                .left(px(SPACE_2))
                .bottom(px(7.))
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .child(
                    Icon::new(IconName::Clock)
                        .size(px(11.))
                        .text_color(rgba(0xFFFFFFCC)),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_DATA_OVERLAY))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0xFFFFFF))
                        .child(progress_label),
                ),
        );
    }
    cover = cover.child(
        div()
            .absolute()
            .right(px(SPACE_2))
            .bottom(px(7.))
            .text_size(px(FONT_SIZE_DATA_OVERLAY))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(0xFFFFFF))
            .child(duration),
    );

    // 批量模式：左上角圆形复选框。盖在封面上，点击只切换选中。
    if batch {
        let selected_bg = if selected {
            p.theme_primary
        } else {
            rgba(0x00000080).into()
        };
        cover = cover.child(
            div()
                .id(("hist-check", index))
                .absolute()
                .top(px(8.))
                .left(px(8.))
                .w(px(22.))
                .h(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(selected_bg)
                .border_1()
                .border_color(rgba(0xFFFFFFCC))
                .cursor_pointer()
                .when(selected, |el| {
                    el.child(
                        Icon::new(IconName::Check)
                            .size(px(13.))
                            .text_color(rgb(0xFFFFFF)),
                    )
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.toggle_history_selection(kid);
                    window.refresh();
                    cx.notify();
                })),
        );
    }

    let title_for_more = title.clone();
    let more = div()
        .id(("hist-more", index))
        .w(px(24.))
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2))
        .child(
            Icon::new(IconName::EllipsisVertical)
                .size(px(16.))
                .text_color(s.text_tertiary),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            this.ask_delete_history(HistoryConfirm::DeleteOne {
                kid,
                title: title_for_more.clone(),
            });
            window.refresh();
            cx.notify();
        }));

    let tag_pill = div()
        .mt(px(1.))
        .h(px(18.))
        .px(px(6.))
        .flex()
        .items_center()
        .flex_shrink_0()
        .rounded(px(RADIUS_SM))
        .bg(if live {
            p.bili_pink
        } else if entry.is_finished() {
            p.status_success_10
        } else {
            s.fill_2
        })
        .text_size(px(FONT_SIZE_MICRO))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if live {
            rgb(0xFFFFFF).into()
        } else if entry.is_finished() {
            p.status_success
        } else {
            s.text_tertiary
        })
        .child(state_label);

    let card_el = div()
        .id(("hist-card", index))
        .w(px(card_w))
        .h(px(COVER_CELL_HEIGHT))
        .p(px(SPACE_2))
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(rgba(0x00000000))
        .flex()
        .flex_col()
        .when(batch || playable, |el| el.cursor_pointer())
        .hover(move |style| {
            style
                .bg(hover_bg)
                .border_color(hover_border)
                .shadow(shadow.clone())
        })
        .when(batch && selected, |el| el.bg(s.fill_1))
        .child(cover)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(SPACE_1))
                .mt(px(SPACE_1))
                .child(tag_pill)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(title_color)
                        .line_clamp(2)
                        .child(title.clone()),
                )
                .child(more),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .mt(px(2.))
                .h(px(18.))
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(s.fill_2)
                        .when(!face.is_empty(), |el| {
                            el.child(
                                img(ImageSource::from(face.clone()))
                                    .w_full()
                                    .h_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(author),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .child("·"),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .child(watched),
                ),
        );

    let card_el = if batch {
        card_el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.toggle_history_selection(kid);
            window.refresh();
            cx.notify();
        }))
    } else if playable {
        let bvid_for_click = bvid.clone();
        let title_for_click = title.clone();
        // 续播：这条历史记录本身就写着上次看到哪（秒）。
        let progress_for_click = entry.progress_secs;
        card_el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if this.playing.is_some() || this.close_player_requested {
                return;
            }
            eprintln!("[history] 打开：{title_for_click}（{bvid_for_click}）");
            this.open_history_video(
                bvid_for_click.clone(),
                title_for_click.clone(),
                progress_for_click,
                cx,
            );
            window.refresh();
            cx.notify();
        }))
    } else {
        card_el
    };

    card_el.into_any_element()
}

fn skeleton_card(t: &YkTheme, card_w: f32, index: usize) -> AnyElement {
    let s = &t.surfaces;
    let cover_h = COVER_ITEM_HEIGHT;
    div()
        .w(px(card_w))
        .p(px(SPACE_2))
        .flex()
        .flex_col()
        .gap(px(SPACE_2))
        .rounded(px(CARD_RADIUS))
        .child(
            div()
                .w_full()
                .h(px(cover_h))
                .rounded(px(MEDIA_RADIUS))
                .bg(s.skeleton_brush)
                .opacity(0.75),
        )
        .child(
            div()
                .w(relative(if index % 2 == 0 { 0.92 } else { 0.64 }))
                .h(px(13.))
                .rounded(px(RADIUS_SM))
                .bg(s.skeleton_brush)
                .opacity(0.6),
        )
        .child(
            div()
                .w(relative(0.45))
                .h(px(10.))
                .rounded(px(RADIUS_SM))
                .bg(s.skeleton_brush)
                .opacity(0.45),
        )
        .into_any_element()
}

// ================= 右栏：日期分组 =================

fn rail(
    t: &YkTheme,
    state: &HistoryState,
    days: &[HistoryDay],
    base: &[&HistoryEntry],
    signed_in: bool,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let total = base.len();
    let latest: Vec<String> = state
        .entries
        .iter()
        .filter(|e| !e.cover.trim().is_empty())
        .take(3)
        .map(|e| e.cover_thumb_url())
        .collect();
    let mut banner = div()
        .relative()
        .w_full()
        .h(px(RAIL_BANNER_H))
        .flex()
        .bg(s.fill_3)
        .overflow_hidden();
    for i in 0..3 {
        let url = latest.get(i).cloned().unwrap_or_default();
        banner = banner.child(div().relative().flex_1().h_full().overflow_hidden().when(
            !url.is_empty(),
            |el| {
                el.child(
                    img(ImageSource::from(url))
                        .absolute()
                        .inset_0()
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Cover),
                )
            },
        ));
    }
    banner = banner
        .child(div().absolute().inset_0().bg(linear_gradient(
            180.,
            linear_color_stop(rgba(0x00000000), 0.0),
            linear_color_stop(rgba(0x1A1C20D9), 1.0),
        )))
        .child(
            div()
                .absolute()
                .left(px(SPACE_3))
                .top(px(SPACE_3))
                .h(px(22.))
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .rounded_full()
                .bg(rgba(0x00000066))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(rgba(0xFFFFFFE6))
                .child(
                    Icon::new(IconName::Cloud)
                        .size(px(12.))
                        .text_color(rgba(0xFFFFFFE6)),
                )
                .child(if signed_in {
                    "云端同步"
                } else {
                    "本地记录"
                }),
        )
        .child(
            div()
                .absolute()
                .left(px(SPACE_3))
                .bottom(px(SPACE_2))
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgba(0xFFFFFFE6))
                .child(if signed_in {
                    "B 站观看历史"
                } else {
                    "登录后同步"
                }),
        );

    let mut list = div()
        .id("hist-day-list")
        .flex()
        .flex_col()
        .gap(px(2.))
        .min_h(px(0.))
        .flex_1()
        .overflow_y_scroll()
        .child(day_row(
            t,
            None,
            "全部历史",
            total,
            state.day.is_none(),
            IconName::Clock,
            cx,
        ));
    for day in days {
        list = list.child(day_row(
            t,
            Some(day.key.clone()),
            &day.label,
            day.count,
            state.day.as_deref() == Some(day.key.as_str()),
            IconName::CalendarDays,
            cx,
        ));
    }

    let mut continue_label = "继续观看".to_string();
    let mut continue_playable = false;
    if let Some(entry) = base
        .iter()
        .copied()
        .filter(|e| e.is_playable())
        .max_by_key(|e| e.view_at)
    {
        continue_playable = true;
        if entry.progress_percent() >= 100 {
            continue_label = "重看最近一条".into();
        }
    }

    let mut body = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .p(px(SPACE_4))
        .gap(px(SPACE_3));
    body = body
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("观看历史"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(if total == state.entries.len() {
                            format!("共 {} 条", total)
                        } else {
                            format!("筛选出 {} / {} 条", total, state.entries.len())
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(2.))
                .p(px(2.))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(s.fill_1)
                .children(HistoryFilter::ALL.iter().map(|filter| {
                    let active = state.filter == *filter;
                    let target = *filter;
                    div()
                        .id(format!("hist-filter-{}", target.label()))
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_HALF))
                        .bg(if active {
                            p.theme_primary
                        } else {
                            rgba(0x00000000).into()
                        })
                        .text_size(px(FONT_SIZE_CAPTION))
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if active {
                            s.text_on_accent
                        } else {
                            s.text_tertiary
                        })
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .child(filter.label())
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.history.filter = target;
                            this.history.selected.clear();
                            this.history.reset_day_if_empty();
                            this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                            window.refresh();
                            cx.notify();
                        }))
                })),
        )
        .child(
            div()
                .id("hist-continue")
                .h(px(36.))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(s.surface_elevated_strong)
                .border_1()
                .border_color(s.border_subtle)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if continue_playable {
                    s.text_secondary
                } else {
                    s.text_quaternary
                })
                .cursor_pointer()
                .hover(|style| style.bg(s.surface_elevated_hover))
                .child(
                    Icon::new(IconName::Play)
                        .size(px(14.))
                        .text_color(if continue_playable {
                            s.text_secondary
                        } else {
                            s.text_quaternary
                        }),
                )
                .child(continue_label)
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    if let Some(entry) = this
                        .history
                        .entries
                        .iter()
                        .filter(|e| e.is_playable())
                        .max_by_key(|e| e.view_at)
                        .cloned()
                    {
                        this.open_history_video(
                            entry.bvid,
                            entry.title,
                            entry.progress_secs,
                            cx,
                        );
                        window.refresh();
                        cx.notify();
                    }
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child("按日期"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(format!("{} 天", days.len())),
                ),
        )
        .child(list)
        .child(div().h(px(1.)).w_full().bg(s.border_subtle).my(px(2.)))
        .child(
            div()
                .id("hist-clear")
                .h(px(32.))
                .w_full()
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(p.status_error)
                .cursor_pointer()
                .hover(|style| style.bg(p.status_error_10))
                .child(
                    Icon::new(IconName::Trash)
                        .size(px(14.))
                        .text_color(p.status_error),
                )
                .child("清空观看历史")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    if this.history.entries.is_empty() {
                        return;
                    }
                    this.ask_delete_history(HistoryConfirm::ClearAll {
                        count: this.history.entries.len(),
                    });
                    window.refresh();
                    cx.notify();
                })),
        );

    div()
        .w(px(RAIL_W))
        .h_full()
        .flex_shrink_0()
        .rounded(px(CARD_RADIUS_CONTINUOUS))
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(s.surface_elevated)
        .border_1()
        .border_color(s.border_subtle)
        .shadow(t.shadows.shadow_2.clone())
        .child(banner)
        .child(body)
        .into_any_element()
}

fn day_row(
    t: &YkTheme,
    key: Option<String>,
    label: &str,
    count: usize,
    selected: bool,
    icon: IconName,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let key_for_click = key.clone();
    let icon_color = if selected {
        p.theme_primary
    } else {
        s.text_tertiary
    };
    div()
        .id(format!("hist-day-{label}"))
        .h(px(36.))
        .w_full()
        .px(px(SPACE_2))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(if selected {
            p.theme_primary_20
        } else {
            rgba(0x00000000).into()
        })
        .text_size(px(FONT_SIZE_CONTROL))
        .text_color(if selected {
            s.text_primary
        } else {
            s.text_secondary
        })
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_1))
        .child(Icon::new(icon).size(px(15.)).text_color(icon_color))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(if selected {
                    p.theme_primary
                } else {
                    s.text_quaternary
                })
                .child(count.to_string()),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.history.day = key_for_click.clone();
            this.history.selected.clear();
            this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
            window.refresh();
            cx.notify();
        }))
        .into_any_element()
}

// ================= 批量条 / 确认框 =================

fn batch_bar(t: &YkTheme, state: &HistoryState, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let count = state.selected.len();
    let visible_count = state
        .entries
        .iter()
        .filter(|e| state.filter.matches(e) && e.matches_query(&state.query))
        .filter(|e| state.day.as_ref().map_or(true, |day| &e.day_key() == day))
        .count();
    div()
        .h(px(BATCH_BAR_H))
        .px(px(SPACE_4))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded_full()
        .bg(s.surface_elevated_strong)
        .border_1()
        .border_color(s.border_popover)
        .shadow(t.shadows.shadow_3.clone())
        .occlude()
        .child(
            div()
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_primary)
                .child(format!("已选 {count} 项")),
        )
        .child(
            div()
                .w(px(1.))
                .h(px(18.))
                .bg(s.border_subtle)
                .mx(px(SPACE_1)),
        )
        .child(action_button(t, "全选", false, cx, {
            move |this: &mut AppRoot| {
                let kids: Vec<i64> = this
                    .history
                    .entries
                    .iter()
                    .filter(|e| {
                        this.history.filter.matches(e)
                            && e.matches_query(&this.history.query)
                            && this
                                .history
                                .day
                                .as_ref()
                                .map_or(true, |day| &e.day_key() == day)
                    })
                    .map(|e| e.kid)
                    .collect();
                this.history.selected = kids.into_iter().collect();
            }
        }))
        .child(
            div()
                .id("hist-batch-delete")
                .h(px(30.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(if count > 0 { p.status_error } else { s.fill_1 })
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if count > 0 {
                    rgb(0xFFFFFF).into()
                } else {
                    s.text_quaternary
                })
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child(
                    Icon::new(IconName::Trash)
                        .size(px(13.))
                        .text_color(if count > 0 {
                            rgb(0xFFFFFF).into()
                        } else {
                            s.text_quaternary
                        }),
                )
                .child(if count > 0 {
                    format!("删除选中（{count}）")
                } else {
                    "删除选中".to_string()
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if this.history.selected.is_empty() {
                        return;
                    }
                    let kids: Vec<i64> = this.history.selected.iter().copied().collect();
                    this.ask_delete_history(HistoryConfirm::DeleteSelected { kids });
                    window.refresh();
                    cx.notify();
                })),
        )
        .child(action_button(
            t,
            "取消",
            false,
            cx,
            move |this: &mut AppRoot| {
                this.history.batch = false;
                this.history.selected.clear();
            },
        ))
        .child(div().flex_1())
        .when(visible_count == 0, |el| {
            el.child(
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_quaternary)
                    .child("没有可选的记录"),
            )
        })
        .into_any_element()
}

fn action_button(
    t: &YkTheme,
    label: &'static str,
    _danger: bool,
    cx: &mut Context<AppRoot>,
    action: impl Fn(&mut AppRoot) + 'static,
) -> AnyElement {
    let s = &t.surfaces;
    div()
        .id(label)
        .h(px(30.))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(s.fill_1)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_secondary)
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2))
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            action(this);
            window.refresh();
            cx.notify();
        }))
        .into_any_element()
}

/// 删除确认框。由 AppRoot 在 Dock 之后挂载，盖住内容与侧栏。
pub(crate) fn confirm_overlay(
    t: &YkTheme,
    kind: &HistoryConfirm,
    busy: bool,
    error: Option<&str>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let title = kind.title();
    let body = kind.body();
    let error = error.map(|e| e.to_string());
    div()
        .id("hist-confirm-scrim")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(s.overlay_scrim)
        .occlude()
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            if this.history.delete_busy {
                return;
            }
            this.history.confirm = None;
            this.history.action_error = None;
            window.refresh();
            cx.notify();
        }))
        .child(
            div()
                .id("hist-confirm-dialog")
                .w(px(380.))
                .p(px(SPACE_6))
                .flex()
                .flex_col()
                .gap(px(SPACE_3))
                .rounded(px(MODAL_RADIUS))
                .bg(s.surface_elevated_strong)
                .border_1()
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_glass_modal.clone())
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(40.))
                        .h(px(40.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(p.status_error_10)
                        .child(
                            Icon::new(IconName::Trash)
                                .size(px(20.))
                                .text_color(p.status_error),
                        ),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(20.))
                        .text_color(s.text_tertiary)
                        .child(body),
                )
                .children(error.map(|error| {
                    div()
                        .px(px(SPACE_3))
                        .py(px(SPACE_2))
                        .rounded(px(INTERACTIVE_RADIUS))
                        .bg(p.status_error_10)
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(p.status_error)
                        .child(error)
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(SPACE_2))
                        .mt(px(SPACE_1))
                        .child(
                            div()
                                .id("hist-confirm-cancel")
                                .h(px(34.))
                                .px(px(SPACE_4))
                                .flex()
                                .items_center()
                                .rounded(px(INTERACTIVE_RADIUS))
                                .bg(s.fill_1)
                                .border_1()
                                .border_color(s.border_strong)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .text_color(s.text_primary)
                                .cursor_pointer()
                                .hover(|style| style.bg(s.fill_2))
                                .child("取消")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    if this.history.delete_busy {
                                        return;
                                    }
                                    this.history.confirm = None;
                                    this.history.action_error = None;
                                    window.refresh();
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("hist-confirm-delete")
                                .h(px(34.))
                                .px(px(SPACE_4))
                                .flex()
                                .items_center()
                                .rounded(px(INTERACTIVE_RADIUS))
                                .bg(p.status_error)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0xFFFFFF))
                                .cursor_pointer()
                                .hover(|style| style.opacity(0.9))
                                .child(if busy { "删除中…" } else { "确认删除" })
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.confirm_history_delete(cx);
                                    window.refresh();
                                })),
                        ),
                ),
        )
        .into_any_element()
}

// ================= AppRoot 入口 =================

use gpui::component::input::InputEvent;
use yk_core::api::client::{clear_history, delete_history_entries, fetch_history, ReqwestClient};

impl AppRoot {
    /// 首次进入历史页时懒建搜索框：
    /// `InputState::new` 需要 `&mut Window`，而 AppRoot 的构造发生在窗口之前，
    /// 所以只能在首帧 render 时建。订阅放在同一处，避免每次 render 重复订阅。
    pub(crate) fn ensure_history_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history.search_input.is_some() {
            return;
        }
        let state = cx.new(|cx| InputState::new(window, cx).placeholder("搜索观看历史"));
        cx.subscribe(&state, |view, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.history.query = state.read(cx).value().to_string();
                view.history.selected.clear();
                view.history.reset_day_if_empty();
                view.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
        })
        .detach();
        self.history.search_input = Some(state);
    }

    /// 进入历史页：只在还没拉过时请求第一页。
    pub(crate) fn ensure_history(&mut self, cx: &mut Context<Self>) {
        if self.history.loaded || self.history.loading {
            return;
        }
        self.load_history(cx);
    }

    /// 重新拉第一页。保留旧内容，数据回来再替换（刷新不闪白）。
    pub(crate) fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history.loading {
            return;
        }
        if self.session.is_none() {
            self.history.loading = false;
            self.history.loaded = true;
            self.history.entries.clear();
            self.history.error = None;
            self.history.has_more = false;
            self.history.cursor = None;
            cx.notify();
            return;
        }

        self.history.generation = self.history.generation.wrapping_add(1);
        let generation = self.history.generation;
        self.history.loading = true;
        self.history.error = None;
        self.history.cursor = None;
        // 重拉第一页后旧的 kid 可能已经不在列表里，批量选择不再可信。
        self.history.batch = false;
        self.history.selected.clear();
        self.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
        let cookie = self.cookie();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_history(&client, &cookie, None, "")
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.history.generation != generation {
                    return;
                }
                view.history.loading = false;
                match result {
                    Ok((items, cursor)) => {
                        eprintln!("[history] 第一页：{} 条", items.len());
                        view.history.has_more = cursor.is_some() && !items.is_empty();
                        view.history.cursor = cursor;
                        view.history.entries = items;
                        view.history.loaded = true;
                        view.history.error = None;
                        view.history.reset_day_if_empty();
                    }
                    Err(error) => {
                        eprintln!("[history] 加载失败：{error}");
                        // 有旧内容就保留，顶部条提示；没有内容才进整页错误态。
                        view.history.error = Some(error);
                        view.history.loaded = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 触底 / 点「加载更早的记录」时追加一页。
    pub(crate) fn load_more_history(&mut self, cx: &mut Context<Self>) {
        if self.history.loading_more || self.history.loading || !self.history.has_more {
            return;
        }
        let Some(cursor) = self.history.cursor else {
            self.history.has_more = false;
            return;
        };
        let generation = self.history.generation;
        self.history.loading_more = true;
        self.history.error = None;
        let cookie = self.cookie();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_history(&client, &cookie, Some(cursor), "")
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.history.generation != generation {
                    return;
                }
                view.history.loading_more = false;
                match result {
                    Ok((items, next)) => {
                        let before = view.history.entries.len();
                        for item in items {
                            if !view.history.entries.iter().any(|e| e.kid == item.kid) {
                                view.history.entries.push(item);
                            }
                        }
                        eprintln!(
                            "[history] 追加：{} 条（{} → {}）",
                            view.history.entries.len() - before,
                            before,
                            view.history.entries.len()
                        );
                        view.history.has_more = next.is_some();
                        view.history.cursor = next;
                        view.history.error = None;
                        view.history.reset_day_if_empty();
                    }
                    Err(error) => {
                        eprintln!("[history] 加载更多失败：{error}");
                        view.history.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 历史卡点击 → 与首页 / 动态共用同一个播放器实例。
    ///
    /// `progress_secs` = 这条历史记录的已观看秒数，直接作为起播位置
    /// （续播）；没有有效进度时传 0，从头播。
    pub(crate) fn open_history_video(
        &mut self,
        bvid: String,
        title: String,
        progress_secs: u64,
        cx: &mut Context<Self>,
    ) {
        if bvid.trim().is_empty() {
            return;
        }
        eprintln!("[history] 进入播放浮窗：{title}（{bvid}）");
        self.ensure_player(cx);
        // 已看完的条目从头播：续播位置由播放器的统一规则再判一次。
        let seed = if progress_secs > 0 {
            Some(progress_secs as f64)
        } else {
            None
        };
        if let Some(player) = self.playing.clone() {
            let _ = player.update(cx, |view, cx| view.open_bvid_at(bvid, seed, cx));
        }
        cx.notify();
    }

    pub(crate) fn toggle_history_selection(&mut self, kid: i64) {
        if !self.history.batch {
            return;
        }
        if !self.history.selected.insert(kid) {
            self.history.selected.remove(&kid);
        }
    }

    pub(crate) fn ask_delete_history(&mut self, kind: HistoryConfirm) {
        self.history.confirm = Some(kind);
        self.history.action_error = None;
        self.history.delete_busy = false;
    }

    /// 确认删除：写操作在后台跑，成功后本地才移除；失败保留条目并展示原因。
    pub(crate) fn confirm_history_delete(&mut self, cx: &mut Context<Self>) {
        if self.history.delete_busy {
            return;
        }
        let Some(kind) = self.history.confirm.clone() else {
            return;
        };
        let Some(session) = self.session.clone() else {
            self.history.action_error = Some("登录态已失效，请重新登录后再删除".into());
            cx.notify();
            return;
        };
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let kind_for_task = kind.clone();
        let kind_for_update = kind.clone();
        self.history.delete_busy = true;
        self.history.action_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    match &kind_for_task {
                        HistoryConfirm::ClearAll { .. } => clear_history(&client, &cookie, &csrf),
                        other => delete_history_entries(&client, &cookie, &csrf, &other.kids()),
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.history.delete_busy = false;
                match result {
                    Ok(()) => {
                        eprintln!("[history] 删除成功：{:?}", kind_for_update);
                        match kind_for_update {
                            HistoryConfirm::ClearAll { .. } => {
                                view.history.entries.clear();
                                view.history.cursor = None;
                                view.history.has_more = false;
                                view.history.day = None;
                                view.history.loaded = true;
                            }
                            ref other => {
                                let kids: HashSet<i64> = other.kids().into_iter().collect();
                                view.history.entries.retain(|e| !kids.contains(&e.kid));
                                view.history.reset_day_if_empty();
                            }
                        }
                        view.history.confirm = None;
                        view.history.selected.clear();
                        view.history.batch = false;
                        view.history.action_error = None;
                        view.history.error = None;
                        // 删掉当前已加载的最后一页时，后面可能还有更早的记录；
                        // 自动补一页，避免把「暂时空了」误判成「历史清空」。
                        if view.history.entries.is_empty() && view.history.has_more {
                            view.load_more_history(cx);
                        }
                    }
                    Err(error) => {
                        eprintln!("[history] 删除失败：{error}");
                        view.history.action_error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
