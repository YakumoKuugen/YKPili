//! 搜索：顶栏输入 + 联想面板 + 独立结果页。
//!
//! 版式对齐 B 站网页版搜索（参考稿 2026-10-27）：
//! 顶栏输入框聚焦后直接浮出联想面板；回车进入结果页，结果页第一层是
//! 分类 tab + 排序条，综合页按「影视 / 用户 / 视频」分段。
//!
//! 与设计文档 12.1.2 的差异是**故意的**：文档早期方案是「左侧 200 筛选栏
//! + 两列列表」，参考稿实际是「顶部分类 tab + 横向结果卡」，这里按参考稿走。
//! 分类数据仍来自同一个 `all/v2` 响应，切 tab 只在本地过滤，不重发请求。

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::assets::IconName;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::format;
use yk_core::search::{
    self, Hotword, SearchHistory, SearchMedia, SearchResults, SearchSort, SearchTab, SearchUser,
    SearchVideo, SuggestItem,
};
use yk_ui::theme::*;

use crate::{AppRoot, ImageResidency, DOCK_RESERVED};

/// 顶栏搜索框最大宽（令牌在 `yk-ui`；这里保留常量名给联想面板做兜底定位）。
pub const SEARCH_BAR_WIDTH: f32 = TOP_BAR_SEARCH_WIDTH;
/// 联想面板最大高度；超出滚动。
const SUGGEST_MAX_HEIGHT: f32 = 500.0;
/// 联想 / 历史 / 热搜行高。
const SUGGEST_ROW_HEIGHT: f32 = 44.0;
/// 结果缩略图（结果条目骨架与列表用）。
const RESULT_THUMB_WIDTH: f32 = 200.0;
const RESULT_THUMB_HEIGHT: f32 = 112.0;
/// 综合页「影视」大卡的海报尺寸。
const MEDIA_POSTER_WIDTH: f32 = 190.0;
const MEDIA_POSTER_HEIGHT: f32 = 268.0;

/// 视频时长筛选（本地过滤，不发请求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DurationFilter {
    #[default]
    All,
    Short,
    Medium,
    Long,
    Extra,
}

impl DurationFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Short,
        Self::Medium,
        Self::Long,
        Self::Extra,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部时长",
            Self::Short => "10 分钟以下",
            Self::Medium => "10–30 分钟",
            Self::Long => "30–60 分钟",
            Self::Extra => "60 分钟以上",
        }
    }

    pub fn matches(self, secs: u64) -> bool {
        match self {
            Self::All => true,
            Self::Short => secs > 0 && secs < 600,
            Self::Medium => (600..1800).contains(&secs),
            Self::Long => (1800..3600).contains(&secs),
            Self::Extra => secs >= 3600,
        }
    }
}

/// 搜索结果页 / 顶栏搜索框的完整状态。
pub struct SearchState {
    /// 顶栏输入框；首帧 render 时懒建（`InputState` 需要 Window）。
    pub input: Option<Entity<InputState>>,
    /// 输入框里的原文（未提交）。
    pub draft: String,
    /// 当前结果对应的关键词（已提交）。
    pub query: String,
    /// 联想面板是否展开。
    pub open: bool,
    pub tab: SearchTab,
    pub sort: SearchSort,
    pub duration: DurationFilter,
    pub filters_open: bool,

    // ---- 结果 ----
    pub results: SearchResults,
    pub loading: bool,
    pub loading_more: bool,
    pub error: Option<String>,
    pub more_error: Option<String>,
    pub page: u32,
    pub num_pages: u32,
    pub num_results: u64,
    pub has_more: bool,
    pub generation: u64,

    // ---- 联想 ----
    pub suggestions: Vec<SuggestItem>,
    pub suggest_loading: bool,
    pub suggest_error: Option<String>,
    pub suggest_generation: u64,
    pub selected: Option<usize>,
    pub history: SearchHistory,
    pub hotwords: Vec<Hotword>,
    pub hotwords_loading: bool,
    pub hotwords_loaded: bool,
    pub hotwords_generation: u64,

    // ---- 页面内交互 ----
    /// 顶栏搜索框在窗口里的实际矩形：联想面板要贴着它，而不是贴着窗口中线。
    pub bar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub episode_open: Option<i64>,
    pub episode_selected: HashMap<i64, usize>,
    pub followed: HashSet<i64>,
    pub notice: Option<String>,
    notice_generation: u64,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            input: None,
            draft: String::new(),
            query: String::new(),
            open: false,
            tab: SearchTab::All,
            sort: SearchSort::Total,
            duration: DurationFilter::All,
            filters_open: false,
            results: SearchResults::default(),
            loading: false,
            loading_more: false,
            error: None,
            more_error: None,
            page: 0,
            num_pages: 0,
            num_results: 0,
            has_more: false,
            generation: 0,
            suggestions: Vec::new(),
            suggest_loading: false,
            suggest_error: None,
            suggest_generation: 0,
            selected: None,
            history: SearchHistory::load_default(),
            hotwords: Vec::new(),
            hotwords_loading: false,
            hotwords_loaded: false,
            hotwords_generation: 0,
            bar_bounds: Rc::new(Cell::new(None)),
            episode_open: None,
            episode_selected: HashMap::new(),
            followed: HashSet::new(),
            notice: None,
            notice_generation: 0,
        }
    }

    /// 当前 tab 下应用排序 / 时长后的视频。
    pub fn visible_videos(&self) -> Vec<&SearchVideo> {
        let mut videos: Vec<&SearchVideo> = self
            .results
            .videos
            .iter()
            .filter(|v| self.duration.matches(v.duration_secs))
            .collect();
        match self.sort {
            SearchSort::Total => {}
            SearchSort::Play => videos.sort_by(|a, b| b.view_count.cmp(&a.view_count)),
            SearchSort::Newest => videos.sort_by(|a, b| b.pubdate.cmp(&a.pubdate)),
            SearchSort::Danmaku => videos.sort_by(|a, b| b.danmaku_count.cmp(&a.danmaku_count)),
            SearchSort::Favorite => videos.sort_by(|a, b| b.favorite_count.cmp(&a.favorite_count)),
        }
        videos
    }

    pub fn visible_empty(&self) -> bool {
        if self.duration != DurationFilter::All && !self.results.videos.is_empty() {
            return self.visible_videos().is_empty();
        }
        self.results.is_empty_for(self.tab)
    }
}

// ================= AppRoot 入口 =================

impl AppRoot {
    /// 首帧创建顶栏搜索框；订阅放在同一处，避免每次 render 重复订阅。
    pub(crate) fn ensure_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.input.is_some() {
            return;
        }
        let draft = self.search.draft.clone();
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索视频、UP 主、番剧")
                .default_value(draft.clone())
        });
        if !draft.is_empty() {
            // `default_value` 在部分版本里只改内部值、不刷新显示，显式再 set 一次。
            state.update(cx, |input, cx| input.set_value(draft, window, cx));
        }
        cx.subscribe(&state, |view, state, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                let value = state.read(cx).value().to_string();
                view.on_search_draft_changed(value, cx);
            }
            InputEvent::Focus => {
                view.search.open = true;
                view.ensure_hotwords(cx);
                if view.search.draft.trim().is_empty() {
                    view.search.suggestions.clear();
                    view.search.suggest_loading = false;
                    view.search.selected = None;
                } else {
                    view.schedule_suggest(cx);
                }
                cx.notify();
            }
            InputEvent::Blur => {
                let generation = view.search.suggest_generation;
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(160))
                        .await;
                    let _ = this.update(cx, |view, cx| {
                        if view.search.suggest_generation == generation && view.search.open {
                            view.search.open = false;
                            view.search.selected = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            InputEvent::PressEnter { shift, .. } if !shift => {
                view.submit_search_from_input(cx);
            }
            _ => {}
        })
        .detach();
        self.search.input = Some(state);
    }

    fn on_search_draft_changed(&mut self, value: String, cx: &mut Context<Self>) {
        if self.search.draft == value {
            return;
        }
        self.search.draft = value;
        self.search.selected = None;
        self.search.open = true;
        if self.search.draft.trim().is_empty() {
            self.search.suggest_generation = self.search.suggest_generation.wrapping_add(1);
            self.search.suggestions.clear();
            self.search.suggest_loading = false;
            self.search.suggest_error = None;
            self.ensure_hotwords(cx);
        } else {
            self.schedule_suggest(cx);
        }
        cx.notify();
    }

    /// 250ms 防抖后拉联想词；用代际号丢弃过期请求。
    pub(crate) fn schedule_suggest(&mut self, cx: &mut Context<Self>) {
        let term = self.search.draft.trim().to_string();
        self.search.suggest_generation = self.search.suggest_generation.wrapping_add(1);
        let generation = self.search.suggest_generation;
        if term.is_empty() {
            self.search.suggestions.clear();
            self.search.suggest_loading = false;
            self.search.suggest_error = None;
            return;
        }
        self.search.suggest_loading = true;
        self.search.suggest_error = None;
        self.search.selected = None;
        // 新词进来先清掉上一个词的候选，避免 loading 期间 ↑↓ 选到旧结果。
        self.search.suggestions.clear();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let still = this.update(cx, |view, _| {
                view.search.suggest_generation == generation && view.search.draft.trim() == term
            });
            if !matches!(still, Ok(true)) {
                return;
            }
            let result = cx
                .background_executor()
                .spawn({
                    let term = term.clone();
                    async move {
                        let client = yk_core::api::client::ReqwestClient::new()
                            .map_err(|e| e.to_string())?;
                        search::fetch_suggest(&client, &term)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search.suggest_generation != generation {
                    return;
                }
                view.search.suggest_loading = false;
                match result {
                    Ok(items) => {
                        eprintln!("[search] 联想词 {} 条：{}", items.len(), term);
                        view.search.suggestions = items;
                        view.search.suggest_error = None;
                    }
                    Err(error) => {
                        eprintln!("[search] 联想词拉取失败：{error}");
                        view.search.suggestions.clear();
                        view.search.suggest_error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_hotwords(&mut self, cx: &mut Context<Self>) {
        if self.search.hotwords_loaded || self.search.hotwords_loading {
            return;
        }
        self.reload_hotwords(cx);
    }

    fn reload_hotwords(&mut self, cx: &mut Context<Self>) {
        if self.search.hotwords_loading {
            return;
        }
        self.search.hotwords_loading = true;
        self.search.hotwords_generation = self.search.hotwords_generation.wrapping_add(1);
        let generation = self.search.hotwords_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    search::fetch_hotwords(&client)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search.hotwords_generation != generation {
                    return;
                }
                view.search.hotwords_loading = false;
                view.search.hotwords_loaded = true;
                match result {
                    Ok(words) => view.search.hotwords = words,
                    Err(error) => eprintln!("[search] 热搜拉取失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 从顶栏输入框提交（优先取键盘高亮的联想词）。
    pub(crate) fn submit_search_from_input(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .search
            .selected
            .and_then(|index| self.search.suggestions.get(index))
            .map(|item| item.value.clone());
        let text = self
            .search
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_else(|| self.search.draft.clone());
        let query = selected.unwrap_or(text);
        self.submit_search(query, cx);
    }

    /// 进入结果页并拉第一页。重复提交同一个词也会重新请求（相当于刷新）。
    pub(crate) fn submit_search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        if self.search.history.push(&query) {
            eprintln!("[search] 历史 +1：{query}");
        }
        self.search.draft = query.clone();
        self.search.query = query;
        self.search.open = false;
        self.search.selected = None;
        // 提交后取消还在防抖窗口里的联想请求；结果页不再需要它。
        self.search.suggest_generation = self.search.suggest_generation.wrapping_add(1);
        self.search.suggest_loading = false;
        self.search.tab = SearchTab::All;
        self.search.sort = SearchSort::Total;
        self.search.duration = DurationFilter::All;
        self.search.filters_open = false;
        self.search.episode_open = None;
        self.search.results = SearchResults::default();
        self.search.error = None;
        self.search.more_error = None;
        self.search.loading = true;
        self.search.loading_more = false;
        self.search.page = 0;
        self.search.num_pages = 0;
        self.search.num_results = 0;
        self.search.has_more = true;

        self.page = Some(crate::pages::PageKind::Search);
        self.page_state = crate::pages::PageState::Ready;
        self.reset_card_hover();
        self.top_refresh_armed = false;
        self.pull_refresh_triggered = false;
        self.scroll_content_to_top();
        self.load_search_page(false, cx);
        cx.notify();
    }

    /// 拉一页结果。`append = true` 接在现有结果后面。
    pub(crate) fn load_search_page(&mut self, append: bool, cx: &mut Context<Self>) {
        if append {
            if self.search.loading_more || self.search.loading || !self.search.has_more {
                return;
            }
        } else if self.search.loading && self.search.page > 0 {
            // 首次进入时 loading 已经置 true，这里不能把它当成「正在请求」挡掉。
            return;
        }
        let query = self.search.query.clone();
        if query.trim().is_empty() {
            self.search.loading = false;
            return;
        }
        let page = if append { self.search.page + 1 } else { 1 };
        self.search.generation = self.search.generation.wrapping_add(1);
        let generation = self.search.generation;
        if append {
            self.search.loading_more = true;
            self.search.more_error = None;
        } else {
            self.search.loading = true;
            self.search.error = None;
            self.search.more_error = None;
        }
        let cookie = self.cookie();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let query = query.clone();
                    let cookie = cookie.clone();
                    async move {
                        let client = yk_core::api::client::ReqwestClient::new()
                            .map_err(|e| e.to_string())?;
                        search::fetch_search_all(&client, &query, page, &cookie)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search.generation != generation || view.search.query != query {
                    return;
                }
                view.search.loading = false;
                view.search.loading_more = false;
                match result {
                    Ok(result) => {
                        let added = if append {
                            view.search.results.merge(result.results)
                        } else {
                            view.search.results = result.results;
                            view.reset_card_hover();
                            view.search.results.videos.len()
                        };
                        view.search.page = result.page.max(page);
                        view.search.num_pages = result.num_pages;
                        view.search.num_results = result.num_results;
                        let has_video_more = view.search.page < result.num_pages;
                        view.search.has_more = has_video_more && added > 0;
                        eprintln!(
                            "[search] {query} 第 {} 页：视频 {} / 用户 {} / 番剧 {} / 影视 {} / 直播 {} / 专栏 {}",
                            view.search.page,
                            view.search.results.videos.len(),
                            view.search.results.users.len(),
                            view.search.results.bangumi.len(),
                            view.search.results.films.len(),
                            view.search.results.live.len(),
                            view.search.results.articles.len(),
                        );
                    }
                    Err(error) => {
                        eprintln!("[search] 结果拉取失败：{error}");
                        if append {
                            view.search.more_error = Some(error);
                        } else if view.search.results.is_empty() {
                            view.search.error = Some(error);
                        } else {
                            // 刷新失败：保留旧结果，只给一条轻提示，不把页面打回错误态。
                            view.set_search_notice(format!("刷新失败：{error}"), cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_search_video(&mut self, bvid: String, cx: &mut Context<Self>) {
        if self.playing.is_some() || self.close_player_requested || bvid.trim().is_empty() {
            return;
        }
        if bvid.starts_with("BV") || bvid.starts_with("bv") {
            self.ensure_player(cx);
            if let Some(player) = self.playing.clone() {
                let _ = player.update(cx, |view, cx| view.open_bvid(bvid, cx));
            }
        }
    }

    pub(crate) fn set_search_notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        self.search.notice = Some(text.clone());
        self.search.notice_generation = self.search.notice_generation.wrapping_add(1);
        let generation = self.search.notice_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2_400))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search.notice_generation == generation {
                    view.search.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

// ================= 顶栏搜索框 =================

pub(crate) fn search_bar(
    t: &YkTheme,
    state: &SearchState,
    window: &Window,
    cx: &mut Context<AppRoot>,
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
                    if !view.search.open {
                        view.search.open = true;
                        view.ensure_hotwords(cx);
                        if !view.search.draft.trim().is_empty() {
                            view.schedule_suggest(cx);
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }),
        )
        // ↑↓ / Esc 由输入框外层先截住：光标不动，只移动联想高亮。
        .capture_key_down(cx.listener(|view, event: &KeyDownEvent, _window, cx| {
            if !view.search.open {
                return;
            }
            let key = event.keystroke.key.as_str();
            if key == "escape" {
                if view.search.selected.is_some() {
                    view.search.selected = None;
                } else {
                    view.search.open = false;
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if key != "down" && key != "up" {
                return;
            }
            let len = view.search.suggestions.len();
            if len == 0 {
                return;
            }
            let next = match (key, view.search.selected) {
                ("down", Some(index)) => (index + 1).min(len - 1),
                ("down", None) => 0,
                ("up", Some(0)) => 0,
                ("up", Some(index)) => index - 1,
                ("up", None) => len - 1,
                _ => 0,
            };
            view.search.selected = Some(next);
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
                    if let Some(input) = view.search.input.clone() {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    view.search.draft.clear();
                    view.search.suggestions.clear();
                    view.search.selected = None;
                    view.search.open = true;
                    view.ensure_hotwords(cx);
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
                view.submit_search_from_input(cx);
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

// ================= 联想面板 =================

pub(crate) fn suggest_overlay(
    t: &YkTheme,
    state: &SearchState,
    window: &Window,
    cx: &mut Context<AppRoot>,
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

fn kbd_hint(t: &YkTheme, key: &'static str, action: &'static str) -> AnyElement {
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

fn suggestions_body(
    t: &YkTheme,
    state: &SearchState,
    _window: &Window,
    cx: &mut Context<AppRoot>,
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
            if let Some(input) = view.search.input.clone() {
                let query = query.clone();
                input.update(cx, |input, cx| input.set_value(query, window, cx));
            }
            view.submit_search(query.clone(), cx);
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

fn suggest_row(
    t: &YkTheme,
    item: &SuggestItem,
    index: usize,
    selected: bool,
    cx: &mut Context<AppRoot>,
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
            if let Some(input) = view.search.input.clone() {
                let value = value.clone();
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            view.submit_search(value.clone(), cx);
            window.refresh();
        }))
        .into_any_element()
}

fn skeleton_row(t: &YkTheme, index: usize) -> AnyElement {
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

fn empty_query_body(t: &YkTheme, state: &SearchState, cx: &mut Context<AppRoot>) -> AnyElement {
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

fn section_head(
    t: &YkTheme,
    icon: IconName,
    title: &'static str,
    action: Option<(&'static str, u8)>,
    cx: &mut Context<AppRoot>,
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
                            view.search.history.clear();
                        }
                        _ => view.reload_hotwords(cx),
                    }
                    cx.stop_propagation();
                    cx.notify();
                })),
        );
    }
    head.into_any_element()
}

fn history_row(t: &YkTheme, term: &str, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    view.search.history.remove(&term_for_delete);
                    window.refresh();
                    cx.stop_propagation();
                    cx.notify();
                })),
        )
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
            if let Some(input) = view.search.input.clone() {
                let term = term_for_click.clone();
                input.update(cx, |input, cx| input.set_value(term, window, cx));
            }
            view.submit_search(term_for_click.clone(), cx);
            window.refresh();
        }))
        .into_any_element()
}

fn hotword_row(t: &YkTheme, hot: &Hotword, index: usize, cx: &mut Context<AppRoot>) -> AnyElement {
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
        if let Some(input) = view.search.input.clone() {
            let keyword = keyword.clone();
            input.update(cx, |input, cx| input.set_value(keyword, window, cx));
        }
        view.submit_search(keyword.clone(), cx);
        window.refresh();
    }))
    .into_any_element()
}

// ================= 结果页 =================

pub(crate) fn page(
    t: &YkTheme,
    state: &SearchState,
    hovered: Option<usize>,
    leaving: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<AppRoot>,
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

fn tabs_row(t: &YkTheme, state: &SearchState, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    if view.search.tab != *tab {
                        view.search.tab = *tab;
                        view.search.filters_open = false;
                        view.reset_card_hover();
                        view.scroll_content_to_top();
                        window.refresh();
                        cx.notify();
                    }
                })),
        );
    }
    row.into_any_element()
}

fn sort_row(t: &YkTheme, state: &SearchState, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    view.search.sort = *sort;
                    view.reset_card_hover();
                    view.scroll_content_to_top();
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
                view.search.filters_open = !view.search.filters_open;
                window.refresh();
                cx.notify();
            })),
    );
    row.into_any_element()
}

fn filters_row(t: &YkTheme, state: &SearchState, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    view.search.duration = *filter;
                    view.reset_card_hover();
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
                view.search.duration = DurationFilter::All;
                view.reset_card_hover();
                window.refresh();
                cx.notify();
            })),
    );
    row.into_any_element()
}

fn section_title(t: &YkTheme, title: &'static str, meta: Option<String>) -> AnyElement {
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

fn video_grid(
    t: &YkTheme,
    videos: &[&SearchVideo],
    hovered: Option<usize>,
    leaving: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let _ = window;
    let mut grid = div().flex().flex_wrap().gap(px(SPACE_4)).w_full();
    for (index, video) in videos.iter().enumerate() {
        let item = video.to_cover_item(index);
        if !item.image_url.is_empty() {
            active_images.borrow_mut().mark(&item.cover_thumb_url());
        }
        grid = grid.child(crate::card(
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

fn media_hero(
    t: &YkTheme,
    media: &SearchMedia,
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
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

    let poster = if !media.cover_url.is_empty() {
        active_images.borrow_mut().mark(&media.cover_url);
        img(ImageSource::from(media.cover_url.clone()))
            .w(px(MEDIA_POSTER_WIDTH))
            .h(px(MEDIA_POSTER_HEIGHT))
            .object_fit(ObjectFit::Cover)
            .into_any_element()
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
                        if view.search.episode_open == Some(media_id) {
                            view.search.episode_open = None;
                        } else {
                            view.search.episode_open = Some(media_id);
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
                        view.set_search_notice("番剧 / 影视播放接口尚未接入", cx);
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
                        view.search.episode_selected.insert(media_id, index);
                        view.search.episode_open = None;
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

fn media_grid(
    t: &YkTheme,
    media: &[SearchMedia],
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
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
        let cover = if item.cover_url.is_empty() {
            div().w_full().h(px(252.)).bg(s.fill_3).into_any_element()
        } else {
            active_images.borrow_mut().mark(&item.cover_url);
            img(ImageSource::from(item.cover_url.clone()))
                .w_full()
                .h(px(252.))
                .object_fit(ObjectFit::Cover)
                .into_any_element()
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
                        view.search.episode_selected.insert(media_id, selected);
                        view.set_search_notice("番剧 / 影视播放接口尚未接入", cx);
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

fn user_card(
    t: &YkTheme,
    user: &SearchUser,
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let followed = state.followed.contains(&user.mid);

    let avatar = if user.avatar_url.is_empty() {
        div()
            .w(px(56.))
            .h(px(56.))
            .rounded_full()
            .bg(p.theme_primary_20)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(20.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.theme_primary)
            .child(
                user.name
                    .chars()
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            )
            .into_any_element()
    } else {
        active_images.borrow_mut().mark(&user.avatar_url);
        img(ImageSource::from(user.avatar_url.clone()))
            .w(px(56.))
            .h(px(56.))
            .rounded_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    };

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
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if followed {
                    s.text_secondary
                } else {
                    s.text_on_accent
                })
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child(if followed { "已关注" } else { "+ 关注" })
                .on_click(cx.listener({
                    let mid = user.mid;
                    move |view, _: &ClickEvent, window, cx| {
                        if !view.search.followed.remove(&mid) {
                            view.search.followed.insert(mid);
                            view.set_search_notice("已在本地关注，账号关注接口尚未接入", cx);
                        }
                        window.refresh();
                        cx.notify();
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

fn user_video_tile(
    t: &YkTheme,
    video: &SearchVideo,
    owner_mid: i64,
    index: usize,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let cover = if video.cover_url.is_empty() {
        div().w_full().h(px(94.)).bg(s.fill_3).into_any_element()
    } else {
        active_images.borrow_mut().mark(&video.cover_url);
        img(ImageSource::from(video.cover_url.clone()))
            .w_full()
            .h(px(94.))
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    };
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
            view.open_search_video(bvid.clone(), cx);
            window.refresh();
            cx.notify();
        }))
        .child(
            div()
                .relative()
                .w_full()
                .h(px(94.))
                .rounded(px(RADIUS_HALF))
                .overflow_hidden()
                .child(cover)
                .child(
                    div()
                        .absolute()
                        .left(px(SPACE_1))
                        .bottom(px(SPACE_1))
                        .px(px(4.))
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0x000000B3))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child(format::count(video.view_count)),
                ),
        )
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

fn live_grid(
    t: &YkTheme,
    state: &SearchState,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
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
                    view.set_search_notice("直播间播放接口尚未接入", cx);
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

fn article_list(
    t: &YkTheme,
    state: &SearchState,
    _active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
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
                    view.set_search_notice("专栏阅读页尚未接入", cx);
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

fn load_more(t: &YkTheme, state: &SearchState, cx: &mut Context<AppRoot>) -> AnyElement {
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
            if view.search.loading_more {
                return;
            }
            view.load_search_page(true, cx);
            window.refresh();
        }))
        .into_any_element()
}

fn loading_rows(t: &YkTheme) -> AnyElement {
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

fn error_state(t: &YkTheme, error: &str, query: &str, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    view.load_search_page(false, cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn empty_state(t: &YkTheme, state: &SearchState, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    view.page = Some(crate::pages::PageKind::Category);
                    view.reset_card_hover();
                    view.scroll_content_to_top();
                    window.refresh();
                    cx.notify();
                })),
        )
        .into_any_element()
}

/// 全局轻提示：动作尚未接入时给一个明确出口，而不是点了没反应。
pub(crate) fn notice(t: &YkTheme, text: &str) -> AnyElement {
    let s = &t.surfaces;
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(DOCK_RESERVED + SPACE_4))
        .flex()
        .justify_center()
        .child(
            div()
                .h(px(34.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(s.surface_elevated_strong)
                .border_1()
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_2.clone())
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_secondary)
                .child(text.to_string()),
        )
        .into_any_element()
}
