//! 搜索页状态：顶栏输入框、联想面板与结果页共享同一份。
//!
//! 只放状态与纯计算（排序 / 时长过滤 / 空态判断）；输入订阅、请求、
//! 关注状态刷新都在组装点，避免把 `AppRoot` 带进 library。

use std::cell::Cell;
use std::time::Duration;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::component::input::InputState;
use gpui::component::input::InputEvent;
use gpui::AppContext as _;
use gpui::{Bounds, Context, Entity, Focusable as _, Pixels, Window};
use yk_infra::search::{
    Hotword, SearchHistory, SearchResults, SearchSort, SearchTab, SearchVideo, SuggestItem,
};

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
    /// 已关注（服务端确认过）的 uid 集合。
    pub followed: HashSet<i64>,
    /// 已经问过服务端、关注状态可信的 uid；不在里面 = 还没查过。
    pub follow_known: HashSet<i64>,
    /// 关注 / 取关请求进行中的 uid：按钮显示「处理中」并挡住连点。
    pub follow_busy: HashSet<i64>,
    /// 后台补查关注状态的 uid（只用来去重，不改按钮文案）。
    pub follow_prefetching: HashSet<i64>,
    pub notice: Option<String>,
    pub notice_generation: u64,
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
            follow_known: HashSet::new(),
            follow_busy: HashSet::new(),
            follow_prefetching: HashSet::new(),
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

/// 搜索页需要宿主提供的动作与状态访问。
///
/// 输入订阅、联想请求、分页与关注状态留在组装点；视图只通过这组方法触发。
pub trait SearchHost: crate::card::CardHost + Sized {
    fn search_state(&self) -> &SearchState;
    fn search_state_mut(&mut self) -> &mut SearchState;

    /// 会话 Cookie；搜索接口需要时由组装点提供。
    fn search_session(&self) -> Option<yk_domain::models::UserSession> {
        None
    }

    fn search_cookie(&self) -> String {
        String::new()
    }

    fn search_ensure_hotwords(&mut self, cx: &mut Context<Self>) { self.ensure_hotwords(cx); }
    fn search_schedule_suggest(&mut self, cx: &mut Context<Self>) { self.schedule_suggest(cx); }
    fn search_reload_hotwords(&mut self, cx: &mut Context<Self>) { self.reload_hotwords(cx); }
    fn search_submit_from_input(&mut self, cx: &mut Context<Self>) { self.submit_search_from_input(cx); }
    fn search_submit(&mut self, query: String, cx: &mut Context<Self>) { self.submit_search(query, cx); }
    fn search_load_more(&mut self, cx: &mut Context<Self>) { self.load_search_page(true, cx); }
    fn search_reload(&mut self, cx: &mut Context<Self>) { self.load_search_page(false, cx); }
    fn search_notice(&mut self, text: String, cx: &mut Context<Self>) { self.set_search_notice(text, cx); }

    /// 打开顶栏搜索浮层。组装点覆盖成带动画的展开；默认直接展开。
    fn search_request_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_state_mut().open = true;
        self.search_state_mut().selected = None;
        self.search_ensure_hotwords(cx);
        if let Some(input) = self.search_state().input.clone() {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 收起顶栏搜索浮层。组装点覆盖成带动画的回收；默认直接收起。
    fn search_request_close(&mut self, cx: &mut Context<Self>) {
        self.search_state_mut().open = false;
        self.search_state_mut().selected = None;
        cx.notify();
    }

    /// 提交搜索后把页面切到结果页；组装点覆盖。
    fn search_route_to_results(&mut self, _cx: &mut Context<Self>) {}
    fn search_go_category(&mut self, cx: &mut gpui::Context<Self>);
    fn search_open_video(&mut self, bvid: String, cx: &mut gpui::Context<Self>);
    fn search_toggle_follow(&mut self, mid: i64, cx: &mut gpui::Context<Self>) {
        if mid <= 0 || self.search_state_mut().follow_busy.contains(&mid) {
            return;
        }
        let Some(session) = self.search_session() else {
            self.search_notice("请先登录后再关注".to_string(), cx);
            return;
        };
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let known = self.search_state_mut().follow_known.contains(&mid);
        let intended = !self.search_state_mut().followed.contains(&mid);
        self.search_state_mut().follow_busy.insert(mid);
        if known {
            // 乐观更新：失败再翻回来（见下方 Err 分支）。
            if intended {
                self.search_state_mut().followed.insert(mid);
            } else {
                self.search_state_mut().followed.remove(&mid);
            }
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    // 未知起点：先查真实状态，再决定写「关注」还是「取关」。
                    let next = if known {
                        intended
                    } else {
                        !yk_infra::api::client::fetch_followed(&client, &cookie, mid)?
                    };
                    yk_infra::interaction::follow_user(&client, &cookie, &csrf, mid, next)?;
                    Ok::<_, String>(next)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.search_state_mut().follow_busy.remove(&mid);
                match result {
                    Ok(next) => {
                        if next {
                            view.search_state_mut().followed.insert(mid);
                        } else {
                            view.search_state_mut().followed.remove(&mid);
                        }
                        view.search_state_mut().follow_known.insert(mid);
                        view.set_search_notice(
                            if next { "已关注" } else { "已取消关注" },
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[search] 关注失败：{error}");
                        if known {
                            // 乐观更新回滚。
                            if intended {
                                view.search_state_mut().followed.remove(&mid);
                            } else {
                                view.search_state_mut().followed.insert(mid);
                            }
                        }
                        view.set_search_notice(error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn search_prefetch_follows(&mut self, cx: &mut gpui::Context<Self>) {
        if !matches!(self.search_state_mut().tab, SearchTab::All | SearchTab::User) {
            return;
        }
        if self.search_cookie().trim().is_empty() {
            return;
        }
        // 综合页只露出第一个用户，补一个就够；「用户」tab 才铺开补全。
        let cap = if self.search_state_mut().tab == SearchTab::All {
            1
        } else {
            20
        };
        let mids: Vec<i64> = {
            let state = self.search_state_mut();
            state
                .results
                .users
                .iter()
                .map(|user| user.mid)
                .filter(|mid| {
                    *mid > 0
                        && !state.follow_known.contains(mid)
                        && !state.follow_prefetching.contains(mid)
                })
                .take(cap)
                .collect()
        };
        if mids.is_empty() {
            return;
        }
        let cookie = self.search_cookie();
        for mid in &mids {
            self.search_state_mut().follow_prefetching.insert(*mid);
        }
        cx.spawn(async move |this, cx| {
            let pending = mids.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    let mut states = Vec::with_capacity(mids.len());
                    for mid in mids {
                        // 单条失败只跳过这一条：不因为一个 -412 把整批丢掉。
                        if let Ok(followed) =
                            yk_infra::api::client::fetch_followed(&client, &cookie, mid)
                        {
                            states.push((mid, followed));
                        }
                    }
                    Ok::<_, String>(states)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                for mid in &pending {
                    view.search_state_mut().follow_prefetching.remove(mid);
                }
                if let Ok(states) = result {
                    for (mid, followed) in states {
                        // 用户在此期间点过（有过写操作）的，以写操作为准。
                        if view.search_state_mut().follow_known.contains(&mid) {
                            continue;
                        }
                        view.search_state_mut().follow_known.insert(mid);
                        if followed {
                            view.search_state_mut().followed.insert(mid);
                        } else {
                            view.search_state_mut().followed.remove(&mid);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn search_reset_hover(&mut self);
    fn search_scroll_top(&mut self);

    fn on_search_draft_changed(&mut self, value: String, cx: &mut Context<Self>) {
        if self.search_state_mut().draft == value {
            return;
        }
        self.search_state_mut().draft = value;
        self.search_state_mut().selected = None;
        self.search_state_mut().open = true;
        if self.search_state_mut().draft.trim().is_empty() {
            self.search_state_mut().suggest_generation = self.search_state_mut().suggest_generation.wrapping_add(1);
            self.search_state_mut().suggestions.clear();
            self.search_state_mut().suggest_loading = false;
            self.search_state_mut().suggest_error = None;
            self.ensure_hotwords(cx);
        } else {
            self.schedule_suggest(cx);
        }
        cx.notify();
    }

    /// 250ms 防抖后拉联想词；用代际号丢弃过期请求。
    fn schedule_suggest(&mut self, cx: &mut Context<Self>) {
        let term = self.search_state_mut().draft.trim().to_string();
        self.search_state_mut().suggest_generation = self.search_state_mut().suggest_generation.wrapping_add(1);
        let generation = self.search_state_mut().suggest_generation;
        if term.is_empty() {
            self.search_state_mut().suggestions.clear();
            self.search_state_mut().suggest_loading = false;
            self.search_state_mut().suggest_error = None;
            return;
        }
        self.search_state_mut().suggest_loading = true;
        self.search_state_mut().suggest_error = None;
        self.search_state_mut().selected = None;
        // 新词进来先清掉上一个词的候选，避免 loading 期间 ↑↓ 选到旧结果。
        self.search_state_mut().suggestions.clear();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let still = this.update(cx, |view, _| {
                view.search_state_mut().suggest_generation == generation && view.search_state_mut().draft.trim() == term
            });
            if !matches!(still, Ok(true)) {
                return;
            }
            let result = cx
                .background_executor()
                .spawn({
                    let term = term.clone();
                    async move {
                        let client = yk_infra::api::client::ReqwestClient::new()
                            .map_err(|e| e.to_string())?;
                        yk_infra::search::fetch_suggest(&client, &term)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search_state_mut().suggest_generation != generation {
                    return;
                }
                view.search_state_mut().suggest_loading = false;
                match result {
                    Ok(items) => {
                        eprintln!("[search] 联想词 {} 条：{}", items.len(), term);
                        view.search_state_mut().suggestions = items;
                        view.search_state_mut().suggest_error = None;
                    }
                    Err(error) => {
                        eprintln!("[search] 联想词拉取失败：{error}");
                        view.search_state_mut().suggestions.clear();
                        view.search_state_mut().suggest_error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_hotwords(&mut self, cx: &mut Context<Self>) {
        if self.search_state_mut().hotwords_loaded || self.search_state_mut().hotwords_loading {
            return;
        }
        self.reload_hotwords(cx);
    }

    fn reload_hotwords(&mut self, cx: &mut Context<Self>) {
        if self.search_state_mut().hotwords_loading {
            return;
        }
        self.search_state_mut().hotwords_loading = true;
        self.search_state_mut().hotwords_generation = self.search_state_mut().hotwords_generation.wrapping_add(1);
        let generation = self.search_state_mut().hotwords_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::search::fetch_hotwords(&client)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search_state_mut().hotwords_generation != generation {
                    return;
                }
                view.search_state_mut().hotwords_loading = false;
                view.search_state_mut().hotwords_loaded = true;
                match result {
                    Ok(words) => view.search_state_mut().hotwords = words,
                    Err(error) => eprintln!("[search] 热搜拉取失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 从顶栏输入框提交（优先取键盘高亮的联想词）。
    fn submit_search_from_input(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .search_state()
            .selected
            .and_then(|index| self.search_state_mut().suggestions.get(index))
            .map(|item| item.value.clone());
        let text = self
            .search_state()
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_else(|| self.search_state_mut().draft.clone());
        let query = selected.unwrap_or(text);
        self.submit_search(query, cx);
    }

    /// 进入结果页并拉第一页。重复提交同一个词也会重新请求（相当于刷新）。
    fn submit_search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        if self.search_state_mut().history.push(&query) {
            eprintln!("[search] 历史 +1：{query}");
        }
        self.search_state_mut().draft = query.clone();
        self.search_state_mut().query = query;
        // 走统一的收起入口：组装点会播回收动画，并把焦点还给根节点。
        self.search_request_close(cx);
        self.search_state_mut().selected = None;
        // 提交后取消还在防抖窗口里的联想请求；结果页不再需要它。
        self.search_state_mut().suggest_generation = self.search_state_mut().suggest_generation.wrapping_add(1);
        self.search_state_mut().suggest_loading = false;
        self.search_state_mut().tab = SearchTab::All;
        self.search_state_mut().sort = SearchSort::Total;
        self.search_state_mut().duration = DurationFilter::All;
        self.search_state_mut().filters_open = false;
        self.search_state_mut().episode_open = None;
        self.search_state_mut().results = SearchResults::default();
        self.search_state_mut().error = None;
        self.search_state_mut().more_error = None;
        self.search_state_mut().loading = true;
        self.search_state_mut().loading_more = false;
        self.search_state_mut().page = 0;
        self.search_state_mut().num_pages = 0;
        self.search_state_mut().num_results = 0;
        self.search_state_mut().has_more = true;

        self.search_route_to_results(cx);
        self.search_scroll_top();
        self.load_search_page(false, cx);
        cx.notify();
    }

    /// 拉一页结果。`append = true` 接在现有结果后面。
    fn load_search_page(&mut self, append: bool, cx: &mut Context<Self>) {
        if append {
            if self.search_state_mut().loading_more || self.search_state_mut().loading || !self.search_state_mut().has_more {
                return;
            }
        } else if self.search_state_mut().loading && self.search_state_mut().page > 0 {
            // 首次进入时 loading 已经置 true，这里不能把它当成「正在请求」挡掉。
            return;
        }
        let query = self.search_state_mut().query.clone();
        if query.trim().is_empty() {
            self.search_state_mut().loading = false;
            return;
        }
        let page = if append { self.search_state_mut().page + 1 } else { 1 };
        self.search_state_mut().generation = self.search_state_mut().generation.wrapping_add(1);
        let generation = self.search_state_mut().generation;
        if append {
            self.search_state_mut().loading_more = true;
            self.search_state_mut().more_error = None;
        } else {
            self.search_state_mut().loading = true;
            self.search_state_mut().error = None;
            self.search_state_mut().more_error = None;
        }
        let cookie = self.search_cookie();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let query = query.clone();
                    let cookie = cookie.clone();
                    async move {
                        let client = yk_infra::api::client::ReqwestClient::new()
                            .map_err(|e| e.to_string())?;
                        yk_infra::search::fetch_search_all(&client, &query, page, &cookie)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                {
                    let state = view.search_state();
                    if state.generation != generation || state.query != query {
                        return;
                    }
                }
                view.search_state_mut().loading = false;
                view.search_state_mut().loading_more = false;
                match result {
                    Ok(result) => {
                        let page_no = result.page.max(page);
                        let num_pages = result.num_pages;
                        let num_results = result.num_results;
                        let counts = (
                            result.results.videos.len(),
                            result.results.users.len(),
                            result.results.bangumi.len(),
                            result.results.films.len(),
                            result.results.live.len(),
                            result.results.articles.len(),
                        );
                        let added = if append {
                            view.search_state_mut().results.merge(result.results)
                        } else {
                            view.search_state_mut().results = result.results;
                            view.search_reset_hover();
                            view.search_state_mut().results.videos.len()
                        };
                        {
                            let state = view.search_state_mut();
                            state.page = page_no;
                            state.num_pages = num_pages;
                            state.num_results = num_results;
                            let has_video_more = page_no < num_pages;
                            state.has_more = has_video_more && added > 0;
                        }
                        eprintln!(
                            "[search] {query} 第 {page_no} 页：视频 {} / 用户 {} / 番剧 {} / 影视 {} / 直播 {} / 专栏 {}",
                            counts.0, counts.1, counts.2, counts.3, counts.4, counts.5,
                        );
                        // 用户卡上的「已关注」要真实数据：结果一落地就在后台补查。
                        view.search_prefetch_follows(cx);
                    }
                    Err(error) => {
                        eprintln!("[search] 结果拉取失败：{error}");
                        if append {
                            view.search_state_mut().more_error = Some(error);
                        } else if view.search_state_mut().results.is_empty() {
                            view.search_state_mut().error = Some(error);
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

    fn set_search_notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        self.search_state_mut().notice = Some(text.clone());
        self.search_state_mut().notice_generation = self.search_state_mut().notice_generation.wrapping_add(1);
        let generation = self.search_state_mut().notice_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2_400))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search_state_mut().notice_generation == generation {
                    view.search_state_mut().notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn ensure_search_input(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.search_state_mut().input.is_some() {
            return;
        }
        let draft = self.search_state_mut().draft.clone();
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
                view.search_state_mut().open = true;
                view.ensure_hotwords(cx);
                if view.search_state_mut().draft.trim().is_empty() {
                    view.search_state_mut().suggestions.clear();
                    view.search_state_mut().suggest_loading = false;
                    view.search_state_mut().selected = None;
                } else {
                    view.schedule_suggest(cx);
                }
                cx.notify();
            }
            InputEvent::Blur => {
                let generation = view.search_state_mut().suggest_generation;
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(160))
                        .await;
                    let _ = this.update(cx, |view, cx| {
                        if view.search_state_mut().suggest_generation == generation
                            && view.search_state_mut().open
                        {
                            // 失焦 = 点到了外面：走和 Esc 同一条收起路径，
                            // 组装点会播收回动画。
                            view.search_request_close(cx);
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
        self.search_state_mut().input = Some(state);
    }

    fn open_search_video(&mut self, bvid: String, cx: &mut gpui::Context<Self>) {
        if self.card_is_playing() || self.card_close_requested() || bvid.trim().is_empty() {
            return;
        }
        if bvid.starts_with("BV") || bvid.starts_with("bv") {
            self.search_open_player(bvid, cx);
        }
    }

    fn search_open_player(&mut self, _bvid: String, _cx: &mut gpui::Context<Self>) {}
}
