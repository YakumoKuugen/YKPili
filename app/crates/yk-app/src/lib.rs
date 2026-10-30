//! YKPili 的 GPUI 外壳（M2）。
//!
//! 对应旧 `MainView.axaml` 的四层结构，但换成 GPUI 的**绘制顺序**表达：
//!
//! ```text
//! Root
//!   ├─ ① 背景渐变        （最先画，铺满）
//!   ├─ ② 主题色光晕      （叠在渐变上）
//!   ├─ ③ 页面内容        （顶栏 → 分类 → 网格）
//!   ├─ ④ 播放浮窗        （独立 View，整场景压在页面上；M3 再接）
//!   ├─ ⑤ 模态遮罩/对话框  （M4 接）
//!   └─ ⑥ Toast           （deferred，永远最上；M4 接）
//! ```
//!
//! GPUI 没有 z-index，「谁在上」只由 child 的先后顺序决定（见设计文档附录 D · D.1 事实 2）。
//! 因此这个文件里 child 的排列顺序**就是**层级契约本身，改动顺序等于改动层级。

use gpui::assets::IconName;
use gpui::component::{v_virtual_list, Icon, VirtualListScrollHandle};
use gpui::prelude::*;
use gpui::{ImageSource, InteractiveElement, StatefulInteractiveElement, *};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};
use yk_core::api::models::{RecommendVideoItem, VideoOwner, VideoStat};
use yk_core::dynamic::DynamicItem;
use yk_core::models::CoverItem;
use yk_core::Feed;

pub mod account_view;
pub mod components;
pub mod dynamic_feed;
pub mod dynamic_view;
pub mod favorites_view;
pub mod history_view;
pub mod http;
pub mod login_view;
pub mod messages_view;
pub mod page_image_cache;
pub mod pages;
pub mod player;
pub mod rank_view;
pub mod search_view;
pub mod settings_view;
pub mod video;
pub mod video_worker;
use page_image_cache::PageImageCache;
use yk_ui::status::SKELETON_PULSE_MS;
use yk_ui::theme::*;

/// 退出确认提示文案。
///
/// 行为是「再次按下」而不是「按住」：第一次只提示，3 秒内再按一次才退出。
/// Windows 用 Alt+Q（Win+Q 被系统搜索占用），macOS 用 ⌘Q。
pub const QUIT_HINT_TEXT: &str = if cfg!(target_os = "windows") {
    "再次按下 Alt+Q 键即可退出"
} else {
    "再次按下 ⌘Q 键即可退出"
};

/// 一次 ⌘Q 的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitDecision {
    /// 第一次按：展示提示。
    Show,
    /// 按住产生的重复键 / 提示刚出现：忽略。
    Wait,
    /// 提示期内再次按：退出。
    Quit,
}

/// 当前帧实际挂出的卡片图片 URL。
///
/// 卡片渲染时写入；下一帧开始时与页面图片缓存做差集，滚出
/// 「可视 + 一行缓冲」的图片会被同时移出 CPU 缓存与 GPU 图集。
#[derive(Default)]
pub(crate) struct ImageResidency {
    pub current: HashSet<String>,
}

impl ImageResidency {
    pub fn mark(&mut self, url: &str) {
        if !url.is_empty() {
            self.current.insert(url.to_string());
        }
    }
}

/// 应用根视图。
pub struct AppRoot {
    theme: YkTheme,
    feed: Feed,
    /// Some = 正在播放（整场景压在页面上）
    playing: Option<gpui::Entity<player::PlayerView>>,
    /// M4 组件验收页
    gallery: Option<gpui::Entity<components::ComponentGallery>>,
    /// 当前页面；None = 首页（卡片网格）
    page: Option<pages::PageKind>,
    page_state: pages::PageState,
    /// 当前展示的是占位数据还是真实接口数据（用于顶栏角标）
    using_mock: bool,
    /// 首页首屏是否正在拉取推荐（显示「卡片状态实测」里的骨架卡）。
    feed_loading: bool,
    /// 首屏拉取是否已经启动，避免窗口重建时重复发请求。
    initial_load_started: bool,
    /// PlayerView 请求关闭：下一次 render 再移除实体，避免在它自己的回调里析构。
    close_player_requested: bool,
    /// 播放器入场完成后，把被它完全遮住的内容区停绘。
    ///
    /// 不移除布局（保滚动位置/网格几何），只跳过首页网格与其中图片的绘制，
    /// 避免视频每帧重绘时连带为背后不可见的页面付 Layout / Paint 成本。
    /// 打开时先 false（入场动画需要真实页面背景），关闭缩回开始时立刻恢复 false。
    player_page_hidden: bool,
    /// 当前登录会话（启动时从 session.json 恢复）。
    session: Option<yk_core::models::UserSession>,
    /// 登录卡片；None = 未打开。
    login: Option<Entity<login_view::LoginView>>,
    /// 已登录账号面板；None = 未打开。
    account: Option<Entity<account_view::AccountView>>,
    /// 设置浮层（缓存管理）；None = 未打开。
    settings: Option<Entity<settings_view::SettingsView>>,
    /// 页面级图片缓存：只绑定在内容区，顶栏/浮层不受影响。
    page_image_cache: Option<Entity<PageImageCache>>,
    /// 本帧实际加载的卡片图 URL。
    active_images: Rc<RefCell<ImageResidency>>,
    /// 滚动监测：滚轮不会触发 AppRoot render，需要轮询偏移量后 notify。
    scroll_watchdog_started: bool,
    last_scroll_offset_y: f32,
    /// 最近一次真实滚动的时间。
    ///
    /// 滚轮 / 触控板滚动时，卡片会在指针下方移动，GPUI 会连续派发
    /// `on_hover(true/false)`。若照单全收，每次 hover 变化都会让
    /// `hover_generation + 1`，封面缩放动画就被反复重置、重播。
    /// 滚动期间及其后一小段时间内丢弃 hover 事件，只保留一次收起动画。
    last_scroll_at: Option<std::time::Instant>,
    /// 滚动停止后是否还需要补一帧。
    ///
    /// 滚动期间卡片 hover 视觉被暂停；停下后如果没有鼠标事件，画面会停在
    /// 「没有 hover」的状态。watchdog 在 `scroll_settling()` 结束后补一帧，
    /// 让指针下的卡片重新拿到 hover 样式（只补一次）。
    scroll_render_pending: bool,
    /// 动态详情浮层；None = 未打开。
    dynamic_detail: Option<Entity<dynamic_view::DynamicDetailView>>,
    /// 详情浮层完成缩回：下一次 render 再移除实体，避免在它自己的回调里析构。
    dynamic_detail_close_requested: bool,
    /// 「正在关注」瀑布流数据（关注 UP 主的动态）。
    following: Vec<DynamicItem>,
    following_loading: bool,
    following_loading_more: bool,
    following_loaded: bool,
    following_error: Option<String>,
    following_offset: String,
    following_has_more: bool,
    /// 关注流请求代际：登录 / 退出 / 刷新时只接受最后一次结果。
    following_generation: u64,
    /// 观看历史页状态（数据、筛选、批量选择、搜索框）。
    history: history_view::HistoryState,
    /// 收藏夹页状态（夹子列表、夹内视频、分页与本地排序）。
    favorites: favorites_view::FavoritesState,
    /// 排行榜页状态（分区、整榜数据与请求代际）。
    rank: rank_view::RankState,
    /// 搜索：顶栏输入、联想面板与结果页共享同一份状态。
    search: search_view::SearchState,
    /// 消息中心：四通道列表、私信会话与未读角标。
    messages: messages_view::MessageState,
    /// 启动后是否已经自动弹过一次登录；取消后不再打扰。
    login_prompted: bool,
    /// 推荐流请求代际：登录后的带 Cookie 请求必须覆盖启动时的匿名请求。
    feed_generation: u64,
    /// 首页推荐流的下一页序号（fresh_idx）；刷新归零，触底 +1。
    feed_page: u32,
    /// 是否正在追加下一页推荐。
    feed_loading_more: bool,
    /// 是否正在重新拉取当前推荐流（回顶刷新，不清空旧内容）。
    feed_refreshing: bool,
    /// 推荐流是否还值得继续翻页（达到上限 / 返回空 / 全是重复）。
    feed_has_more: bool,
    /// 关注流回顶刷新中（保留旧列表，避免闪骨架）。
    following_refreshing: bool,
    /// 用户已经向下滚过一段距离，回到顶部时触发一次刷新。
    top_refresh_armed: bool,
    /// 顶部下拉过阈值后只触发一次，回弹到 0 后复位。
    pull_refresh_triggered: bool,
    /// 回顶/下拉刷新的冷却时间，避免回弹过程连续触发两次。
    last_refresh_at: Option<std::time::Instant>,
    /// 内容区滚动句柄：首页虚拟列表用它，页面容器复用它的 base handle
    content_scroll: VirtualListScrollHandle,
    /// 离散滚轮平滑滚动：当前目标区域（1 = 内容区，2 = 历史左栏）。
    smooth_scroll_key: Option<u8>,
    /// 平滑滚动要驱动的句柄。
    smooth_scroll_handle: Option<ScrollHandle>,
    /// 平滑滚动的目标偏移（负数向下）。
    smooth_scroll_target: f32,
    /// 平滑滚动代际：新目标/取消时递增，旧动画循环自行退出。
    smooth_scroll_generation: u64,
    /// 是否已有平滑滚动循环在跑。
    smooth_scroll_animating: bool,
    /// 窗口坐标 → 页面坐标；点击封面时在播放浮窗展开之前快照。
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 当前是否离线（启动时拉取失败 / 显式 YK_OFFLINE）
    offline: bool,
    /// 离线时展示的缓存时间（Unix 秒）；None = 没有可用缓存
    offline_updated_at: Option<u64>,
    /// 当前 hover 的卡片（CoverItem::index）
    hovered_card: Option<usize>,
    /// 刚离开的卡片：只为它播放 1→0 的收尾动画，避免所有旧卡一起重播
    leaving_card: Option<usize>,
    /// 离线条上的「重试」是否正在进行
    offline_retrying: bool,
    /// 最近一次重试失败的原因（只用于日志/提示）
    offline_error: Option<String>,
    /// hover 进入/离开时 +1：让 with_animation 的 id 变化、动画重新开始
    hover_generation: u64,
    /// 退出确认提示的展示时间；None = 没显示。
    ///
    /// 第一次 ⌘Q 只展示提示，提示期内再按一次才真正退出（见
    /// [`AppRoot::request_quit`]）。
    quit_hint_shown_at: Option<std::time::Instant>,
    /// 退出提示代际：重复按 ⌘Q / 超时清除时丢弃旧定时器。
    quit_hint_generation: u64,
    /// 系统「减少动效」轮询是否已启动（macOS/Windows 没有订阅通知）
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    motion_watchdog_started: bool,
}

impl AppRoot {
    /// 用真实接口数据构造（含离线缓存）；拉取失败时退回占位数据。
    pub fn with_real_data(items: &[RecommendVideoItem]) -> Self {
        let mut app = Self::new();
        app.feed.load(items);
        app.using_mock = false;
        app
    }

    /// 首屏加载态：推荐流为空，首页画骨架卡，等 `start_initial_load` 回填。
    ///
    /// 只在「没有缓存、在线启动」时使用；有缓存就先把缓存铺上，
    /// 不为了展示骨架而让用户多等一次网络往返。
    pub fn loading() -> Self {
        let mut app = Self::new();
        app.feed = Feed::new();
        app.using_mock = false;
        app.feed_loading = true;
        app
    }

    /// 由外部（open_window 的 build 闭包）注入已创建好的播放器。
    /// 这样 PlayerView 只在窗口创建时构造一次，不会因为 render 重入而重复 load_file。
    pub fn with_player(mut self, player: gpui::Entity<player::PlayerView>) -> Self {
        self.playing = Some(player);
        self
    }

    /// 启动时直接落到指定页面（验收用，`YK_PAGE=following`）。
    pub fn with_page(mut self, page: pages::PageKind) -> Self {
        self.page = Some(page);
        self
    }

    /// 验收入口：启动即落到某个关键词的结果页（`YK_SEARCH=鬼灭`）。
    pub fn with_search(mut self, query: &str) -> Self {
        let query = query.trim();
        if !query.is_empty() {
            self.page = Some(pages::PageKind::Search);
            self.search.query = query.to_string();
            self.search.draft = query.to_string();
        }
        self
    }

    /// 验收入口：启动即展开顶栏联想面板（`YK_SEARCH_SUGGEST=test`）。
    pub fn with_search_suggest(mut self, term: &str) -> Self {
        self.search.draft = term.to_string();
        self.search.open = true;
        self
    }

    /// 测试 / 截图入口：不自动弹登录卡片。
    pub fn without_login_prompt(mut self) -> Self {
        self.login_prompted = true;
        self
    }

    /// 测试 / 截图入口：装一个假的已登录会话（不发请求，只让页面走登录态分支）。
    pub fn with_signed_in(mut self) -> Self {
        let mut session = yk_core::models::UserSession::default();
        session.dede_user_id = 1;
        session.sess_data = "test-session".into();
        session.bili_jct = "test-csrf".into();
        session.nickname = "测试账号".into();
        session.is_logged_in = true;
        self.session = Some(session);
        self
    }

    /// 测试 / 截图入口：固定一张封面处于 hover 态，用于检查卡片材质。
    pub fn with_hovered_card(mut self, index: usize) -> Self {
        self.hovered_card = Some(index);
        self
    }

    /// 测试 / 截图入口：注入历史页数据并进入批量模式，检查选中卡片的材质。
    pub fn with_history_entries(mut self, entries: Vec<yk_core::history::HistoryEntry>) -> Self {
        self.page = Some(pages::PageKind::History);
        self.history.entries = entries;
        self.history.loaded = true;
        self.history.batch = true;
        let kids: Vec<i64> = self.history.entries.iter().take(2).map(|e| e.kid).collect();
        self.history.selected = kids.into_iter().collect();
        self
    }

    /// 测试 / 截图入口：直接注入一份结果，跳过网络。
    pub fn with_search_results(
        mut self,
        query: &str,
        results: yk_core::search::SearchResults,
        total: u64,
    ) -> Self {
        self.page = Some(pages::PageKind::Search);
        self.search.query = query.to_string();
        self.search.draft = query.to_string();
        self.search.loading = false;
        self.search.error = None;
        self.search.page = 1;
        self.search.num_pages = 1;
        self.search.num_results = total;
        self.search.has_more = false;
        self.search.results = results;
        self
    }

    /// 测试 / 截图入口：注入联想面板内容。
    pub fn with_search_suggestions(
        mut self,
        term: &str,
        items: Vec<yk_core::search::SuggestItem>,
        hotwords: Vec<yk_core::search::Hotword>,
    ) -> Self {
        self.search.draft = term.to_string();
        self.search.open = true;
        self.search.suggestions = items;
        self.search.hotwords = hotwords;
        self.search.hotwords_loaded = true;
        self
    }

    /// 测试 / 截图入口：直接注入互动通道数据，跳过网络。
    pub fn with_message_inbox(
        mut self,
        channel: yk_core::messages::MessageChannel,
        entries: Vec<yk_core::messages::MessageEntry>,
        unread: yk_core::messages::UnreadCounts,
    ) -> Self {
        self.page = Some(pages::PageKind::Messages);
        self.messages.channel = channel;
        let inbox = self.messages.inbox_mut();
        inbox.entries = entries;
        inbox.loaded = true;
        inbox.loading = false;
        inbox.has_more = false;
        self.messages.unread = unread;
        self.messages.unread_loaded = true;
        self
    }

    /// 测试 / 截图入口：直接显示 ⌘Q 退出确认提示。
    pub fn with_quit_hint(mut self) -> Self {
        self.quit_hint_shown_at = Some(std::time::Instant::now());
        self
    }

    /// 测试 / 验收：⌘Q 退出提示当前是否可见。
    pub fn quit_hint_visible(&self) -> bool {
        self.quit_hint_shown_at.is_some()
    }

    /// 测试入口：把焦点放到顶栏搜索框，让窗口有真实的键盘焦点。
    pub fn focus_search_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_search_input(window, cx);
        if let Some(input) = self.search.input.clone() {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// 测试 / 截图入口：直接注入一张榜单。
    pub fn with_rank_page(mut self, rid: u32, page: yk_core::ranking::RankPage) -> Self {
        self.page = Some(pages::PageKind::Rank);
        self.rank.zone = yk_core::ranking::ZONES
            .iter()
            .position(|zone| zone.rid == rid)
            .unwrap_or(0);
        self.rank.items = page.items;
        if !page.note.trim().is_empty() {
            self.rank.note = page.note;
        }
        self.rank.loaded = true;
        self.rank.loading = false;
        self
    }

    /// 测试 / 截图入口：直接注入收藏夹列表与夹内视频。
    pub fn with_favorites(
        mut self,
        folders: Vec<yk_core::interaction::FavFolder>,
        items: Vec<yk_core::favorites::FavVideo>,
        total: u64,
    ) -> Self {
        self.page = Some(pages::PageKind::Favorites);
        self.favorites.folders = folders;
        self.favorites.folders_loaded = true;
        self.favorites.folders_loading = false;
        self.favorites.selected = self.favorites.folders.first().map(|folder| folder.id);
        self.favorites.items = items;
        self.favorites.total = total;
        self.favorites.loaded = true;
        self.favorites.loading = false;
        self.favorites.has_more = false;
        self
    }

    /// 测试 / 截图入口：私信列表直接开启「只看互关」。
    pub fn with_message_only_mutual(mut self, only: bool) -> Self {
        self.messages.only_mutual = only;
        self.messages.ensure_dm_selection();
        self
    }

    /// 测试 / 截图入口：展开某条赞消息的点赞者列表。
    pub fn with_message_expanded_like(mut self, id: i64) -> Self {
        self.messages.expanded_like = Some(id);
        self
    }

    /// 测试 / 截图入口：直接注入私信会话列表。
    pub fn with_message_conversations(
        mut self,
        conversations: Vec<yk_core::messages::Conversation>,
        unread: yk_core::messages::UnreadCounts,
    ) -> Self {
        self.page = Some(pages::PageKind::Messages);
        self.messages.channel = yk_core::messages::MessageChannel::Dm;
        self.messages.dm.conversations = conversations;
        self.messages.dm.loaded = true;
        self.messages.dm.loading = false;
        self.messages.apply_whitelist();
        self.messages.unread = unread;
        self.messages.unread_loaded = true;
        self
    }

    /// 测试 / 截图入口：直接打开一个私信会话。
    pub fn with_message_chat(
        mut self,
        conversation: yk_core::messages::Conversation,
        messages: Vec<yk_core::messages::ChatMessage>,
    ) -> Self {
        self.page = Some(pages::PageKind::Messages);
        self.messages.channel = yk_core::messages::MessageChannel::Dm;
        self.messages.dm.conversations = vec![conversation.clone()];
        self.messages.dm.loaded = true;
        self.messages.apply_whitelist();
        let mut chat = messages_view::ChatState::new(&conversation);
        chat.messages = messages;
        chat.loading = false;
        chat.has_more = false;
        self.messages.chat = Some(chat);
        self.messages.unread_loaded = true;
        self
    }

    /// 首帧后补齐「当前页面所需的异步数据」。
    ///
    /// 关注页与历史页都需要：`with_page` 只切路由，数据仍要等窗口建好后拉。
    pub fn load_current_page(&mut self, cx: &mut Context<Self>) {
        // 验收入口可能带着展开的联想面板启动；先补一次联想请求。
        if self.search.open && !self.search.draft.trim().is_empty() {
            self.schedule_suggest(cx);
        }
        match self.page {
            Some(pages::PageKind::Following) => self.ensure_following(cx),
            Some(pages::PageKind::History) => self.ensure_history(cx),
            Some(pages::PageKind::Favorites) => self.ensure_favorites(cx),
            Some(pages::PageKind::Rank) => self.ensure_rank(cx),
            Some(pages::PageKind::Messages) => self.ensure_messages(cx),
            Some(pages::PageKind::Search) => {
                if !self.search.query.trim().is_empty() {
                    self.load_search_page(false, cx);
                }
            }
            _ => {}
        }
    }

    /// 启动时恢复登录会话；无效会话被忽略。
    pub fn with_session(mut self, session: Option<yk_core::models::UserSession>) -> Self {
        self.session = session.filter(|s| s.is_valid());
        self
    }

    fn cookie(&self) -> String {
        self.session
            .as_ref()
            .map(|s| s.cookie_header())
            .unwrap_or_default()
    }

    /// 切页 / 换 tab / 排序后清空卡片 hover：卡片索引属于上一组结果，
    /// 不清的话新页面第 N 张卡会沿用旧 hover 态。
    pub(crate) fn reset_card_hover(&mut self) {
        self.hovered_card = None;
        self.leaving_card = None;
        self.hover_generation = self.hover_generation.wrapping_add(1);
    }

    /// 处理一次 ⌘Q。
    ///
    /// 第一次只展示「再次按下 ⌘Q / Alt+Q 键即可退出」提示，返回 `false`；
    /// 提示还在（3 秒内）且不是按住产生的首个重复键，再按一次返回 `true`，
    /// 由调用方执行 `cx.quit()`。这样误触不会直接杀掉窗口。
    pub fn request_quit(&mut self, cx: &mut Context<Self>) -> bool {
        let now = std::time::Instant::now();
        match self.quit_decision(now) {
            QuitDecision::Quit => {
                eprintln!("[quit] 再次按下退出快捷键，正式退出");
                true
            }
            QuitDecision::Wait => false,
            QuitDecision::Show => {
                eprintln!("[quit] 第一次按下退出快捷键：显示退出提示");
                self.quit_hint_generation = self.quit_hint_generation.wrapping_add(1);
                let generation = self.quit_hint_generation;
                cx.notify();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(3))
                        .await;
                    let _ = this.update(cx, |view, cx| {
                        if view.quit_hint_generation == generation {
                            view.quit_hint_shown_at = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
                false
            }
        }
    }

    /// 纯状态机：决定这次 ⌘Q 是「显示提示 / 忽略 / 退出」。
    ///
    /// 不依赖 `Context`，因此可以单测 300ms 防重复、3s 提示窗口两个边界。
    fn quit_decision(&mut self, now: std::time::Instant) -> QuitDecision {
        if let Some(shown_at) = self.quit_hint_shown_at {
            let elapsed = now.saturating_duration_since(shown_at);
            // 300ms 内视为同一次按住产生的重复键：只展示提示，不立刻退。
            if elapsed < std::time::Duration::from_millis(300) {
                return QuitDecision::Wait;
            }
            // 提示只保留 3 秒；定时器没来得及清掉时，超时也按「第一次」处理。
            if elapsed <= std::time::Duration::from_secs(3) {
                return QuitDecision::Quit;
            }
            self.quit_hint_shown_at = None;
        }
        self.quit_hint_shown_at = Some(now);
        QuitDecision::Show
    }

    /// 退出确认提示：压在播放器 / 登录 / 设置之上。
    ///
    /// 只有视觉层，没有 hitbox，不会挡住下面的点击。
    fn quit_hint(&self, t: &YkTheme) -> Option<AnyElement> {
        self.quit_hint_shown_at?;
        Some(
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
                                .child(QUIT_HINT_TEXT),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 更新卡片 hover 状态；返回是否发生变化（调用方据此决定要不要重绘）。
    ///
    /// `animate = false` 用于滚动期间：状态照常跟踪，但不推进
    /// `hover_generation`，配合渲染层的暂停，封面缩放不会被反复重置。
    fn update_card_hover(&mut self, idx: usize, hovered: bool, animate: bool) -> bool {
        if hovered {
            if self.hovered_card == Some(idx) {
                return false;
            }
            // 从上一张卡移进来：让上一张播放收尾动画。
            if let Some(prev) = self.hovered_card {
                self.leaving_card = Some(prev);
            }
            // 重新进入正在收尾的自己：取消收尾，直接反向。
            if self.leaving_card == Some(idx) {
                self.leaving_card = None;
            }
            self.hovered_card = Some(idx);
            if animate {
                self.hover_generation = self.hover_generation.wrapping_add(1);
            }
            true
        } else if self.hovered_card == Some(idx) {
            self.hovered_card = None;
            self.leaving_card = Some(idx);
            if animate {
                self.hover_generation = self.hover_generation.wrapping_add(1);
            }
            true
        } else {
            false
        }
    }

    /// 记录一次滚动活动。只刷新时间戳 / 补帧标记，不改 hover 状态 ——
    /// 滚轮事件本身可能被边界 clamp 掉（content 没动），此时不应把
    /// 用户停在卡片上的 hover 收掉。
    fn mark_scrolling(&mut self) {
        self.last_scroll_at = Some(std::time::Instant::now());
        self.scroll_render_pending = true;
    }

    /// 滚动是否仍在「活跃期」。
    ///
    /// 平滑滚动动画本身全程算活跃；触控板 / 原生滚动用时间窗兜底。
    fn scroll_settling(&self) -> bool {
        self.smooth_scroll_animating
            || self
                .last_scroll_at
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(250))
    }

    /// 打开播放器。**只在没有实例时创建，已有就复用。**
    ///
    /// 这是附录 F L-1 的修复：`PlayerView` 里唯一的 `InputState` 跟着实例走，
    /// 一旦重建，输入框内容就换成新的空 state（表现为「打字打一半没了」）。
    /// 所以所有入口都走这里，绝不无条件 `cx.new`。
    /// `PlayerView::new` 里另有单实例断言兜底，防止以后又冒出新路径。
    fn ensure_player(&mut self, cx: &mut Context<Self>) {
        if self.playing.is_some() {
            return;
        }
        // 顶栏下拉不能横跨在播放浮窗之上（12.1 行为红线）。
        self.search.open = false;
        self.reset_card_hover();
        self.cancel_smooth_scroll();
        let theme = self.theme.clone();
        self.playing = Some(cx.new(|cx| player::PlayerView::new(theme, cx)));
        self.install_player_close_handler(cx);
        // 入场动画（280ms）结束、播放器完全盖住内容区后，把背后的页面停绘。
        // 入场期间仍要真实网格做展开背景，所以延迟到动画结束后再置位。
        self.player_page_hidden = false;
        let player_id = self.playing.as_ref().map(|player| player.entity_id());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(PLAYER_PAGE_HIDE_DELAY_MS))
                .await;
            let _ = this.update(cx, |root, cx| {
                // 只对「发起这次延迟的那个播放器实例」生效：快速开关/换实例时，
                // 旧定时器不能把新播放器入场期间的页面提前停绘。
                if root.playing.as_ref().map(|player| player.entity_id()) == player_id
                    && !root.close_player_requested
                {
                    root.player_page_hidden = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 打开登录卡片（扫码 / 密码 / 验证码）。
    fn ensure_login(&mut self, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        self.search.open = false;
        let theme = self.theme.clone();
        let login = cx.new(|cx| login_view::LoginView::new(theme, cx));
        let app_for_close = cx.entity().downgrade();
        let app_for_success = cx.entity().downgrade();
        let _ = login.update(cx, |view, _| {
            view.set_handlers(
                move |window, cx| {
                    let _ = app_for_close.update(cx, |root, cx| {
                        root.login = None;
                        window.refresh();
                        cx.notify();
                    });
                },
                move |cx, session| {
                    let _ = app_for_success.update(cx, |root, cx| {
                        root.apply_login(session, cx);
                    });
                },
            );
        });
        self.login = Some(login);
        cx.notify();
    }

    fn cached_account_info(&self) -> Option<yk_core::auth::AccountInfo> {
        let session = self.session.as_ref()?;
        if session.account_updated_at == 0 {
            return None;
        }
        Some(yk_core::auth::AccountInfo {
            mid: session.dede_user_id,
            name: session.nickname.clone(),
            face: session.face_url.clone(),
            coins: session.coins,
            bcoin: session.bcoin,
            level: session.level,
            following: session.following,
            followers: session.followers,
            dynamics: session.dynamics,
            last_login_time: session.last_login_time.clone(),
            last_login_geo: session.last_login_geo.clone(),
        })
    }

    fn store_account(&mut self, info: yk_core::auth::AccountInfo, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.coins = info.coins;
        session.bcoin = info.bcoin;
        session.level = info.level;
        session.following = info.following;
        session.followers = info.followers;
        session.dynamics = info.dynamics;
        session.last_login_time = info.last_login_time;
        session.last_login_geo = info.last_login_geo;
        session.account_updated_at = yk_core::api::client::now_wts();
        let session = session.clone();
        if let Err(e) = yk_core::session::save(&yk_core::session::session_path(), &session) {
            eprintln!("[login] 账号面板缓存写入失败：{e}");
        }
        cx.notify();
    }

    /// 已登录时点头像：打开账号面板。
    fn ensure_account(&mut self, cx: &mut Context<Self>) {
        if self.account.is_some() {
            return;
        }
        self.search.open = false;
        let Some(session) = self.session.clone() else {
            self.ensure_login(cx);
            return;
        };
        let theme = self.theme.clone();
        let cached = self.cached_account_info();
        // 有缓存就立即显示，不再每次打开都请求；只有缓存缺失时才后台加载。
        let needs_refresh = cached.is_none();
        let account =
            cx.new(|cx| account_view::AccountView::new(theme, session, cached, needs_refresh, cx));
        let app_for_close = cx.entity().downgrade();
        let app_for_logout = cx.entity().downgrade();
        let app_for_settings = cx.entity().downgrade();
        let app_for_info = cx.entity().downgrade();
        let _ = account.update(cx, |view, _| {
            view.set_handlers(
                move |cx| {
                    let _ = app_for_close.update(cx, |root, cx| {
                        root.account = None;
                        cx.notify();
                    });
                },
                move |cx| {
                    let _ = app_for_logout.update(cx, |root, cx| {
                        root.logout(cx);
                    });
                },
                move |cx| {
                    let _ = app_for_settings.update(cx, |root, cx| {
                        root.ensure_settings(cx);
                    });
                },
                move |cx, info| {
                    let _ = app_for_info.update(cx, |root, cx| {
                        root.store_account(info, cx);
                    });
                },
            );
        });
        self.account = Some(account);
        cx.notify();
    }

    /// 退出账号：清 session.json / 内存会话，回到匿名推荐并打开登录卡片。
    fn logout(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.account = None;
        self.settings = None;
        self.playing = None;
        self.close_player_requested = false;
        self.player_page_hidden = false;
        self.dynamic_detail = None;
        self.dynamic_detail_close_requested = false;
        self.reset_following();
        self.history.reset();
        self.favorites.reset();
        self.messages.reset_for_account();
        self.login_prompted = true;
        if let Err(e) = yk_core::session::clear(&yk_core::session::session_path()) {
            eprintln!("[login] 清除 session.json 失败：{e}");
        }
        self.refresh_feed_anonymous(cx);
        self.ensure_login(cx);
        cx.notify();
    }

    /// 退出登录后重新拉匿名推荐。
    fn refresh_feed_anonymous(&mut self, cx: &mut Context<Self>) {
        self.reset_feed_paging();
        self.feed_generation = self.feed_generation.wrapping_add(1);
        let generation = self.feed_generation;
        // 整页替换期间挡住「自动补页」，否则两边会互相顶掉 generation。
        self.feed_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend(
                        &client,
                        "",
                        30,
                        yk_core::api::client::now_wts(),
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.feed_generation != generation {
                    return;
                }
                view.feed_loading = false;
                match result {
                    Ok(items) => {
                        eprintln!("[login] 已退出，匿名推荐：{} 条", items.len());
                        view.feed.load(&items);
                        view.feed_page = 0;
                        view.feed_has_more = true;
                        view.feed_loading_more = false;
                        view.feed_refreshing = false;
                        view.using_mock = false;
                        view.offline = false;
                        view.offline_updated_at = None;
                        view.offline_error = None;
                        if let Err(e) = yk_core::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 匿名推荐写入失败：{e}");
                        }
                    }
                    Err(e) => eprintln!("[login] 退出后匿名推荐拉取失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 登录成功：落盘、关闭卡片、用新 Cookie 重新拉推荐。
    fn apply_login(&mut self, session: yk_core::models::UserSession, cx: &mut Context<Self>) {
        if !session.is_valid() {
            return;
        }
        let path = yk_core::session::session_path();
        if let Err(e) = yk_core::session::save(&path, &session) {
            eprintln!("[login] session.json 写入失败：{e}");
        }
        self.session = Some(session);
        self.login = None;
        self.login_prompted = true;
        // 换账号后旧的关注流 / 消息中心一定不再适用。
        self.reset_following();
        self.messages.reset_for_account();
        self.refresh_unread(cx);
        self.refresh_feed_authenticated(cx);
        if self.page == Some(pages::PageKind::Following) {
            self.load_following_page(false, false, cx);
        }
        if self.page == Some(pages::PageKind::History) {
            self.load_history(cx);
        }
        if self.page == Some(pages::PageKind::Favorites) {
            // 换账号后旧收藏夹不再适用：先清空，再按新 Cookie 重拉。
            self.favorites.reset();
            self.load_favorites(cx);
        }
        if self.page == Some(pages::PageKind::Messages) {
            self.ensure_messages(cx);
        }
        cx.notify();
    }

    /// 用当前会话 Cookie 拉一页新的推荐流（登录成功后调用）。
    fn refresh_feed_authenticated(&mut self, cx: &mut Context<Self>) {
        self.reset_feed_paging();
        let cookie = self.cookie();
        self.feed_generation = self.feed_generation.wrapping_add(1);
        let generation = self.feed_generation;
        // 整页替换期间挡住「自动补页」，否则两边会互相顶掉 generation。
        self.feed_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend(
                        &client,
                        &cookie,
                        30,
                        yk_core::api::client::now_wts(),
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.feed_generation != generation {
                    return;
                }
                view.feed_loading = false;
                match result {
                    Ok(items) => {
                        eprintln!("[login] 登录后推荐流已更新：{} 条", items.len());
                        view.feed.load(&items);
                        view.feed_page = 0;
                        view.feed_has_more = true;
                        view.feed_loading_more = false;
                        view.feed_refreshing = false;
                        view.using_mock = false;
                        view.offline = false;
                        view.offline_updated_at = None;
                        view.offline_error = None;
                        if let Err(e) = yk_core::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 登录后推荐流写入失败：{e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("[login] 登录后推荐流拉取失败，保留当前列表：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ================= 正在关注：动态瀑布流 =================

    /// 首次进入「正在关注」时拉一页；已加载 / 正在加载都不重复请求。
    fn ensure_following(&mut self, cx: &mut Context<Self>) {
        if self.following_loaded
            || self.following_loading
            || self.following_refreshing
            || self.session.is_none()
        {
            return;
        }
        self.load_following_page(false, false, cx);
    }

    /// 手动刷新/回顶刷新：保留旧列表，成功后再替换。
    fn reload_following(&mut self, cx: &mut Context<Self>) {
        self.load_following_page(false, true, cx);
    }

    /// 拉一页关注动态。`append = true` 接在现有列表后面；`refresh = true` 重新拉第一页但不清空旧内容。
    ///
    /// 关注流接口是 offset 分页；成功后把 `offset` / `has_more` 存回 AppRoot。
    /// 请求期间用代际号丢弃过期结果（退出登录 / 刷新）。
    fn load_following_page(&mut self, append: bool, refresh: bool, cx: &mut Context<Self>) {
        if self.following_loading || self.following_loading_more || self.following_refreshing {
            return;
        }
        if append && !self.following_has_more {
            return;
        }
        if !append && !refresh && self.following_loaded {
            return;
        }
        let cookie = self.cookie();
        if cookie.is_empty() {
            self.following_error = Some("登录后查看关注动态".into());
            cx.notify();
            return;
        }

        self.following_generation = self.following_generation.wrapping_add(1);
        let generation = self.following_generation;
        let offset = if append {
            self.following_offset.clone()
        } else {
            String::new()
        };
        if append {
            self.following_loading_more = true;
        } else if refresh {
            self.following_refreshing = true;
            self.following_error = None;
        } else {
            self.following_loading = true;
            self.following_error = None;
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_following_dynamics(&client, &cookie, &offset)
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.following_generation != generation {
                    return;
                }
                view.following_loading = false;
                view.following_loading_more = false;
                view.following_refreshing = false;
                match result {
                    Ok(page) => {
                        let mut items = page.items;
                        let added;
                        if append {
                            let start = view.following.len();
                            for (i, item) in items.iter_mut().enumerate() {
                                item.index = start + i;
                            }
                            // 按动态 id 去重，防止翻页边界重复。
                            items.retain(|item| {
                                !view.following.iter().any(|existing| existing.id == item.id)
                            });
                            added = items.len();
                            view.following.extend(items);
                        } else {
                            added = items.len();
                            view.following = items;
                        }
                        // 仅调试构建：注入一条纯文字动态，验收「左正文右评论」分支。
                        #[cfg(debug_assertions)]
                        if std::env::var("YK_DYNAMIC_TEXT_ONLY").is_ok() {
                            view.following.insert(0, dynamic_feed::debug_text_item(0));
                            for (index, item) in view.following.iter_mut().enumerate() {
                                item.index = index;
                            }
                        }
                        view.following_offset = page.offset;
                        view.following_has_more = page.has_more && added > 0;
                        view.following_loaded = true;
                        view.following_error = None;
                        eprintln!(
                            "[dynamic] 关注流加载成功：{} 条（has_more={}）",
                            view.following.len(),
                            page.has_more
                        );
                        if std::env::var("YK_DYNAMIC_DEBUG").is_ok() {
                            for item in &view.following {
                                eprintln!(
                                    "  #{} {:?} {} imgs={} text={}",
                                    item.index,
                                    item.kind,
                                    item.author_name(),
                                    item.images.len(),
                                    item.text.chars().take(18).collect::<String>()
                                );
                                if let Some(origin) = item.origin.as_ref() {
                                    eprintln!(
                                        "      orig {:?} imgs={} video={} text={}",
                                        origin.kind,
                                        origin.images.len(),
                                        origin.video.is_some(),
                                        origin.text.chars().take(18).collect::<String>()
                                    );
                                }
                            }
                        }
                        // 验收快捷入口：YK_DYNAMIC_VIDEO=1 自动展开第一条视频动态。
                        if !append
                            && std::env::var("YK_DYNAMIC_VIDEO").is_ok()
                            && view.playing.is_none()
                        {
                            if let Some(item) =
                                view.following.iter().find(|i| i.video.is_some()).cloned()
                            {
                                view.open_dynamic_video(&item, None, cx);
                            }
                        }
                        // 验收快捷入口：YK_DYNAMIC_DETAIL=<下标> 自动打开一条图文/文字动态。
                        if !append && view.dynamic_detail.is_none() {
                            if let Some(index) = std::env::var("YK_DYNAMIC_DETAIL")
                                .ok()
                                .and_then(|value| value.parse::<usize>().ok())
                            {
                                if let Some(item) = view.following.get(index).cloned() {
                                    view.open_dynamic_detail(&item, None, cx);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[dynamic] 关注流加载失败：{e}");
                        if append {
                            // 翻页失败不清空已有内容，只停止继续加载。
                            view.following_has_more = false;
                        } else if refresh && !view.following.is_empty() {
                            // 回顶刷新失败：保留旧列表，不切到错误页。
                        } else {
                            view.following_error = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn reset_following(&mut self) {
        self.following.clear();
        self.following_loading = false;
        self.following_loading_more = false;
        self.following_refreshing = false;
        self.following_loaded = false;
        self.following_error = None;
        self.following_offset.clear();
        self.following_has_more = false;
        self.following_generation = self.following_generation.wrapping_add(1);
    }

    /// 从个人面板进入设置浮层：账号面板已先播完缩回动画。
    fn ensure_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        self.search.open = false;
        self.account = None;
        let theme = self.theme.clone();
        let settings = cx.new(|cx| settings_view::SettingsView::new(theme, cx));
        let app = cx.entity().downgrade();
        let _ = settings.update(cx, |view, _| {
            view.set_close_handler(move |cx| {
                let _ = app.update(cx, |root, cx| {
                    root.settings = None;
                    cx.notify();
                });
            });
        });
        self.settings = Some(settings);
        cx.notify();
    }

    /// 注入页面级图片缓存。由 main 在窗口 build 时创建，保证整个页面共用同一个
    /// 缓存实体，才能按 URL 精确驱逐。
    pub fn set_page_image_cache(&mut self, cache: Entity<PageImageCache>) {
        self.page_image_cache = Some(cache);
    }

    /// 丢掉滚出可视区 + 一行缓冲的卡片图。
    ///
    /// 使用上一帧实际挂出的 URL 作为 keep 集合；当前帧渲染时卡片会重新标记。
    /// 播放器/详情浮层打开期间跳过，避免误删它们正在使用的同一张封面。
    fn prune_page_images(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.playing.is_some() || self.dynamic_detail.is_some() {
            return;
        }
        let Some(cache) = self.page_image_cache.clone() else {
            return;
        };
        let keep = std::mem::take(&mut self.active_images.borrow_mut().current);
        cache.update(cx, |cache, cx| cache.prune(&keep, window, cx));
    }

    /// 把「关闭浮窗」的回调注入 PlayerView。
    ///
    /// 播放器自己不知道 AppRoot 的存在；Esc 两次 / 点浮窗外侧时
    /// 通过弱引用把 `playing` 置空，实体 drop 后渲染线程随之停止。
    pub fn install_player_close_handler(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.playing.clone() else {
            return;
        };
        let app = cx.entity().downgrade();
        let _ = player.update(cx, |view, _| {
            view.set_close_handler(move |cx, finished| {
                let _ = app.update(cx, |root, cx| {
                    if !finished {
                        // 关闭缩回开始：先把停绘的页面放回来，缩回动画
                        // 需要真实网格/封面作为背景。
                        root.player_page_hidden = false;
                        cx.notify();
                        return;
                    }
                    root.player_page_hidden = false;
                    root.close_player_requested = true;
                    cx.notify();
                });
            });
        });
    }

    /// 标记为离线，并记录缓存时间（None = 没有可用缓存，只能展示占位内容）。
    pub fn with_offline(mut self, updated_at: Option<u64>) -> Self {
        self.offline = true;
        self.offline_updated_at = updated_at;
        self.feed_loading = false;
        self
    }

    /// 任何一次「整页替换」的推荐请求开始时，先把分页状态复位，
    /// 避免旧的触底/刷新标志卡住后续滚动。
    fn reset_feed_paging(&mut self) {
        self.feed_page = 0;
        self.feed_has_more = true;
        self.feed_loading_more = false;
        self.feed_refreshing = false;
    }

    /// 首屏推荐流：后台拉一页；成功回填并写缓存，失败则退回缓存 / 占位数据。
    ///
    /// 骨架只覆盖「真的没有内容可先显示」的启动；有缓存时直接显示缓存，
    /// 请求结果回来后原地替换（设计文档第 05 节：骨架只在首次加载出现）。
    pub fn start_initial_load(&mut self, cx: &mut Context<Self>) {
        if self.initial_load_started {
            return;
        }
        self.initial_load_started = true;
        // 首屏请求期间挡住触底加载：有缓存时 feed 非空、`feed_loading` 原本是
        // false，大窗口的「自动补页」会和首屏请求并发，两者会互相顶掉 generation。
        self.feed_loading = true;
        // 顶栏 / Dock 的红点要在首页就正确，不能等用户先进消息页。
        if self.session.is_some() {
            self.refresh_unread(cx);
        }
        self.reset_feed_paging();
        let cookie = self.cookie();
        self.feed_generation = self.feed_generation.wrapping_add(1);
        let generation = self.feed_generation;

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend(
                        &client,
                        &cookie,
                        30,
                        yk_core::api::client::now_wts(),
                    )
                    .map_err(|e| e.to_string())
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.feed_generation != generation {
                    return;
                }
                view.feed_loading = false;
                match result {
                    Ok(items) => {
                        eprintln!("[api] 首屏推荐拉取成功：{} 条", items.len());
                        view.feed.load(&items);
                        view.feed_page = 0;
                        view.feed_has_more = true;
                        view.feed_loading_more = false;
                        view.feed_refreshing = false;
                        view.using_mock = false;
                        view.offline = false;
                        view.offline_updated_at = None;
                        view.offline_error = None;
                        if let Err(e) = yk_core::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 推荐流写入失败：{e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("[api] 首屏推荐拉取失败，退回缓存 / 占位数据：{e}");
                        view.offline = true;
                        if view.feed.is_empty() {
                            match yk_core::feed_cache::FeedCache::new().load() {
                                Some(cached) => {
                                    eprintln!(
                                        "[offline] 使用缓存：{} 条（{}）",
                                        cached.items.len(),
                                        yk_core::feed_cache::age_text(cached.updated_at)
                                    );
                                    view.feed.load(&cached.items);
                                    view.using_mock = false;
                                    view.offline_updated_at = Some(cached.updated_at);
                                }
                                None => {
                                    eprintln!("[offline] 没有可用缓存，使用占位数据");
                                    view.feed.load(&mock_recommend(24));
                                    view.using_mock = true;
                                    view.offline_updated_at = None;
                                }
                            }
                        } else if let Some(cached) = yk_core::feed_cache::FeedCache::new().load() {
                            // 启动时已经铺了缓存：保留当前列表，只补离线条的缓存时间。
                            view.offline_updated_at = Some(cached.updated_at);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 离线时点「重试」：后台重新拉一页推荐，成功则清掉离线条并更新缓存。
    fn retry_online(&mut self, cx: &mut Context<Self>) {
        if self.offline_retrying {
            return;
        }
        self.offline_retrying = true;
        self.offline_error = None;
        self.reset_feed_paging();
        let cookie = self.cookie();
        self.feed_generation = self.feed_generation.wrapping_add(1);
        let generation = self.feed_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend(
                        &client,
                        &cookie,
                        30,
                        yk_core::api::client::now_wts(),
                    )
                    .map_err(|e| e.to_string())
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.feed_generation != generation {
                    return;
                }
                view.offline_retrying = false;
                match result {
                    Ok(items) => {
                        eprintln!("[offline] 重试成功：{} 条", items.len());
                        view.feed.load(&items);
                        view.feed_page = 0;
                        view.feed_has_more = true;
                        view.feed_loading_more = false;
                        view.feed_refreshing = false;
                        view.using_mock = false;
                        view.offline = false;
                        view.offline_updated_at = None;
                        view.offline_error = None;
                        if let Err(e) = yk_core::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 重试后写入失败：{e}");
                        }
                        view.content_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    }
                    Err(e) => {
                        eprintln!("[offline] 重试失败：{e}");
                        view.offline_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ================= 无限滚动 / 回到顶部刷新 =================

    /// 首页触底：追加下一批推荐（fresh_idx 递增）。
    fn load_more_feed(&mut self, cx: &mut Context<Self>) {
        if self.feed_loading
            || self.feed_loading_more
            || self.feed_refreshing
            || self.offline
            || !self.feed_has_more
            || self.feed.is_empty()
            || self.feed.is_full()
        {
            return;
        }
        let page = self.feed_page.saturating_add(1).max(1);
        let cookie = self.cookie();
        self.feed_loading_more = true;
        self.feed_generation = self.feed_generation.wrapping_add(1);
        let generation = self.feed_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend_page(
                        &client,
                        &cookie,
                        30,
                        yk_core::api::client::now_wts(),
                        page,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.feed_generation != generation {
                    return;
                }
                view.feed_loading_more = false;
                match result {
                    Ok(items) => {
                        let added = view.feed.append(&items);
                        view.feed_page = page;
                        view.feed_has_more = added > 0 && !items.is_empty() && !view.feed.is_full();
                        eprintln!(
                            "[feed] 触底加载第 {page} 页：+{added} 条（共 {} 条）",
                            view.feed.len()
                        );
                    }
                    Err(e) => {
                        eprintln!("[feed] 触底加载第 {page} 页失败：{e}");
                        // 避免 watchdog 每 60ms 重试一次；重新触发用回顶刷新。
                        view.feed_has_more = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 首页回到顶部：重新拉第 0 页并替换当前内容；不先清空，避免闪空。
    fn refresh_feed(&mut self, cx: &mut Context<Self>) {
        if self.feed_loading || self.feed_loading_more || self.feed_refreshing {
            return;
        }
        let cookie = self.cookie();
        self.feed_refreshing = true;
        self.feed_generation = self.feed_generation.wrapping_add(1);
        let generation = self.feed_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_core::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend_page(
                        &client,
                        &cookie,
                        30,
                        yk_core::api::client::now_wts(),
                        0,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.feed_generation != generation {
                    return;
                }
                view.feed_refreshing = false;
                match result {
                    Ok(items) if !items.is_empty() => {
                        eprintln!("[feed] 回顶刷新：{} 条", items.len());
                        view.feed.load(&items);
                        view.feed_page = 0;
                        view.feed_has_more = true;
                        view.feed_loading_more = false;
                        view.using_mock = false;
                        view.offline = false;
                        view.offline_updated_at = None;
                        view.offline_error = None;
                        if let Err(e) = yk_core::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 刷新后写入失败：{e}");
                        }
                    }
                    Ok(_) => eprintln!("[feed] 回顶刷新返回空列表，保留当前内容"),
                    Err(e) => eprintln!("[feed] 回顶刷新失败，保留当前内容：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 当前页面回到顶部后按页面类型刷新（手势入口，带冷却防抖）。
    fn refresh_current_page(&mut self, cx: &mut Context<Self>) {
        // 一次回弹可能同时满足「回到顶部」和「下拉过阈值」，加一道冷却。
        if let Some(last) = self.last_refresh_at {
            if last.elapsed() < std::time::Duration::from_millis(1200) {
                return;
            }
        }
        self.last_refresh_at = Some(std::time::Instant::now());
        self.refresh_page_now(cx);
    }

    /// 按当前页面类型发起刷新：推荐流重拉第 0 页，关注流重拉第一页，
    /// 历史 / 收藏夹重拉第一页，排行榜重拉当前分区。
    ///
    /// 回顶手势与手动入口共用这一层；1.2s 冷却只加在手势入口上 —— 用户明确
    /// 点了按钮，就该真的重新发一次请求，而不是被防抖吞掉。
    ///
    /// 返回是否发起了刷新：热门 / 分区等没有可重拉数据的页面返回 `false`，
    /// 调用方据此决定要不要把滚动位置拉回顶部。
    fn refresh_page_now(&mut self, cx: &mut Context<Self>) -> bool {
        match self.page {
            None => {
                self.refresh_feed(cx);
                true
            }
            Some(pages::PageKind::Following) => {
                // 未登录没有关注流；登录卡片由入口负责带出，这里不重复弹。
                if self.session.is_none() {
                    return false;
                }
                self.load_following_page(false, true, cx);
                true
            }
            Some(pages::PageKind::History) => {
                if self.session.is_none() {
                    return false;
                }
                self.load_history(cx);
                true
            }
            Some(pages::PageKind::Favorites) => {
                if self.session.is_none() {
                    return false;
                }
                self.load_favorites(cx);
                true
            }
            Some(pages::PageKind::Rank) => {
                self.load_rank(cx);
                true
            }
            Some(pages::PageKind::Messages) => {
                if self.session.is_none() {
                    return false;
                }
                self.refresh_messages(cx);
                true
            }
            Some(pages::PageKind::Search) => {
                if self.search.query.trim().is_empty() {
                    return false;
                }
                // 结果页刷新 = 用同一关键词重拉第一页。
                self.load_search_page(false, cx);
                true
            }
            _ => false,
        }
    }

    /// 手动刷新入口（Dock 的「刷新」按钮 / 再点一次当前一级入口）。
    ///
    /// 先清掉刷新手势的中间状态：刷新后会把内容区拉回顶部，而滚动看门狗
    /// 如果还留着 `top_refresh_armed`，会把这次程序化回顶再判成一次「下拉刷新」。
    /// 返回是否发起了刷新。
    fn refresh_page_manually(&mut self, cx: &mut Context<Self>) -> bool {
        eprintln!(
            "[refresh] 手动刷新当前页：page={:?} signed_in={}",
            self.page,
            self.session.is_some()
        );
        self.top_refresh_armed = false;
        self.pull_refresh_triggered = false;
        let started = self.refresh_page_now(cx);
        if started {
            self.scroll_content_to_top();
        }
        started
    }

    /// 切页 / 刷新后把内容区拉回顶部。
    fn scroll_content_to_top(&mut self) {
        self.cancel_smooth_scroll();
        self.content_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.content_scroll.base_handle().scroll_to_top_of_item(0);
    }

    /// 取消离散滚轮的平滑滚动（页面切换 / 打开播放器 / 触控板事件时）。
    fn cancel_smooth_scroll(&mut self) {
        self.smooth_scroll_generation = self.smooth_scroll_generation.wrapping_add(1);
        self.smooth_scroll_animating = false;
        self.smooth_scroll_key = None;
        self.smooth_scroll_handle = None;
    }

    /// 当前指针位置该驱动哪个滚动句柄。
    ///
    /// 首页 / 关注 / 搜索等页面共用外壳 `content_scroll`；历史页左栏用自己的
    /// `history.scroll`，右栏是日期导航、不滚左栏。消息页有自己的虚拟列表，
    /// 浮层打开时也不接管。
    fn smooth_scroll_target_for(
        &self,
        position: Point<Pixels>,
        window: &Window,
    ) -> Option<(u8, ScrollHandle)> {
        if self.playing.is_some()
            || self.search.open
            || self.account.is_some()
            || self.settings.is_some()
            || self.login.is_some()
            || self.dynamic_detail.is_some()
        {
            return None;
        }
        let viewport = window.viewport_size();
        if position.y <= px(TOP_BAR_HEIGHT)
            || position.y >= viewport.height - px(DOCK_RESERVED)
        {
            return None;
        }
        match self.page {
            // 只接管明确需要的三个滚动面板；搜索/消息等页面内部还有
            // 自己的滚动区，继续交给原生处理，避免误抢。
            None | Some(pages::PageKind::Following) => {
                Some((1, self.content_scroll.base_handle().clone()))
            }
            Some(pages::PageKind::History) => {
                // 右栏宽 RAIL_W，滚轮停在日期栏上时不滚左侧网格。
                let left_limit = viewport.width - px(history_view::RAIL_W + SPACE_5 + SPACE_6);
                if position.x < left_limit {
                    Some((2, self.history.scroll.base_handle().clone()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 捕获阶段处理离散滚轮：不交给 GPUI 的「一格一跳」，而是写入目标偏移，
    /// 由 8ms 动画循环逐帧推进。
    fn handle_smooth_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // 滚轮 → 卡片在指针下滑动 → GPUI 连发 hover 事件。
        // 先记下滚动时间，`on_hover` 在活跃期内直接忽略，避免动画反复重置。
        self.mark_scrolling();
        if event.delta.precise() {
            // 触控板 / 高精度滚轮是像素级增量，交给原生滚动更跟手；
            // 若平滑动画正在跑，先停掉，避免两边抢 offset。
            self.cancel_smooth_scroll();
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height()).y.as_f32();
        if delta.abs() < 0.01 {
            return;
        }
        let Some((key, handle)) = self.smooth_scroll_target_for(event.position, window) else {
            return;
        };
        let same = self.smooth_scroll_key == Some(key);
        let base = if same && self.smooth_scroll_animating {
            self.smooth_scroll_target
        } else {
            handle.offset().y.as_f32()
        };
        let max = handle.max_offset().y.as_f32().max(0.0);
        let target = (base + delta).clamp(-max, 0.0);
        if !same {
            self.smooth_scroll_generation = self.smooth_scroll_generation.wrapping_add(1);
            self.smooth_scroll_animating = false;
            self.smooth_scroll_key = Some(key);
        }
        self.smooth_scroll_handle = Some(handle);
        self.smooth_scroll_target = target;
        // 阻止 GPUI 内置的滚轮 handler 直接把 offset 跳过去。
        cx.stop_propagation();
        if !self.smooth_scroll_animating {
            self.start_smooth_scroll(cx);
        }
    }

    /// 8ms 一步，把 offset 以指数缓动推向目标；到点后停。
    fn start_smooth_scroll(&mut self, cx: &mut Context<Self>) {
        self.smooth_scroll_animating = true;
        let generation = self.smooth_scroll_generation;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(8))
                .await;
            let keep = this.update(cx, |view, cx| {
                if view.smooth_scroll_generation != generation {
                    return false;
                }
                let Some(handle) = view.smooth_scroll_handle.clone() else {
                    view.smooth_scroll_animating = false;
                    return false;
                };
                let current = handle.offset().y.as_f32();
                let target = view.smooth_scroll_target;
                let diff = target - current;
                if diff.abs() < 0.5 {
                    handle.set_offset(point(px(0.), px(target)));
                    view.smooth_scroll_animating = false;
                    cx.notify();
                    return false;
                }
                handle.set_offset(point(px(0.), px(current + diff * 0.22)));
                cx.notify();
                true
            });
            if !matches!(keep, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// 滚到底部时按当前页面追加数据。
    fn auto_load_more(&mut self, cx: &mut Context<Self>) {
        match self.page {
            None => self.load_more_feed(cx),
            Some(pages::PageKind::Following) => self.load_following_page(true, false, cx),
            Some(pages::PageKind::Messages) => self.messages_load_more(cx),
            Some(pages::PageKind::Favorites) => self.load_more_favorites(cx),
            _ => {}
        }
    }

    pub fn new() -> Self {
        eprintln!("[app] AppRoot::new 被调用");
        Self {
            theme: YkTheme::dark(),
            feed: {
                let mut f = Feed::new();
                f.load(&mock_recommend(24));
                f
            },
            playing: None,
            gallery: None,
            page: None,
            page_state: pages::PageState::Ready,
            using_mock: true,
            feed_loading: false,
            initial_load_started: false,
            close_player_requested: false,
            player_page_hidden: false,
            session: None,
            login: None,
            account: None,
            settings: None,
            page_image_cache: None,
            active_images: Rc::new(RefCell::new(ImageResidency::default())),
            scroll_watchdog_started: false,
            last_scroll_offset_y: 0.0,
            last_scroll_at: None,
            scroll_render_pending: false,
            dynamic_detail: None,
            dynamic_detail_close_requested: false,
            following: Vec::new(),
            following_loading: false,
            following_loading_more: false,
            following_loaded: false,
            following_error: None,
            following_offset: String::new(),
            following_has_more: false,
            following_generation: 0,
            history: history_view::HistoryState::new(),
            favorites: favorites_view::FavoritesState::new(),
            rank: rank_view::RankState::new(),
            search: search_view::SearchState::new(),
            messages: messages_view::MessageState::new(),
            login_prompted: false,
            feed_generation: 0,
            feed_page: 0,
            feed_loading_more: false,
            feed_refreshing: false,
            feed_has_more: true,
            following_refreshing: false,
            top_refresh_armed: false,
            pull_refresh_triggered: false,
            last_refresh_at: None,
            content_scroll: VirtualListScrollHandle::new(),
            smooth_scroll_key: None,
            smooth_scroll_handle: None,
            smooth_scroll_target: 0.0,
            smooth_scroll_generation: 0,
            smooth_scroll_animating: false,
            layout_bounds: Rc::new(Cell::new(None)),
            offline: false,
            offline_updated_at: None,
            hovered_card: None,
            leaving_card: None,
            offline_retrying: false,
            offline_error: None,
            hover_generation: 0,
            quit_hint_shown_at: None,
            quit_hint_generation: 0,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            motion_watchdog_started: false,
        }
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.close_player_requested {
            self.close_player_requested = false;
            self.player_page_hidden = false;
            self.playing = None;
            self.hovered_card = None;
            self.leaving_card = None;
        }
        if self.dynamic_detail_close_requested {
            self.dynamic_detail_close_requested = false;
            self.dynamic_detail = None;
        }
        self.prune_page_images(window, cx);
        if !self.scroll_watchdog_started {
            self.scroll_watchdog_started = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(60))
                        .await;
                    let alive = this.update(cx, |view, cx| {
                        // 滚动停下后补一帧：滚动期间卡片 hover 视觉被暂停，
                        // 若没有后续鼠标事件，指针下的卡片会一直停在静态态。
                        // 同时清掉收尾态，避免滚回来的卡片重新挂载时重播 1→0。
                        if view.scroll_render_pending && !view.scroll_settling() {
                            view.scroll_render_pending = false;
                            view.leaving_card = None;
                            cx.notify();
                        }
                        // 消息页用自己的虚拟列表 / 私信列表，滚动偏移不在 content_scroll 上。
                        if view.page == Some(pages::PageKind::Messages) {
                            if let Some(handle) = view
                                .messages
                                .active_scroll()
                                .map(|scroll| scroll.base_handle().clone())
                            {
                                let y = handle.offset().y.as_f32();
                                let max = handle.max_offset().y.as_f32();
                                if max > 0.0 && y < -80.0 && -y >= max - 240.0 {
                                    view.messages_load_more(cx);
                                }
                            }
                            return;
                        }
                        let handle = view.content_scroll.base_handle().clone();
                        let y = handle.offset().y.as_f32();
                        let max = handle.max_offset().y.as_f32();

                        // 大窗口 / 高分辨率：一页 30 条可能连一屏都填不满，
                        // 内容高度不够 → max_offset 恒为 0 → 下面的「触底加载」
                        // 永远不触发，表现就是没有滚动条、也拉不到更多封面。
                        // 这里按视口几何算出「一屏至少需要多少张」，不足就补页；
                        // 请求中 / 离线 / 已达上限由 `load_more_feed` 自己挡。
                        if view.page.is_none()
                            && view.initial_load_started
                            && !view.feed_loading
                            && view.feed_has_more
                        {
                            if let Some(bounds) = view.layout_bounds.get() {
                                let need = home_min_items(
                                    bounds.size.width.as_f32(),
                                    bounds.size.height.as_f32(),
                                );
                                if view.feed.len() < need {
                                    view.load_more_feed(cx);
                                }
                            }
                        }

                        // 回顶刷新：先向下滚过一段距离，再回到顶部才触发一次。
                        if y < -240.0 {
                            view.top_refresh_armed = true;
                        }
                        if y > -2.0 && view.top_refresh_armed {
                            view.top_refresh_armed = false;
                            // 同时压住这次回弹里的下拉手势，避免同一个动作发两次请求。
                            view.pull_refresh_triggered = true;
                            if view.playing.is_none()
                                && view.dynamic_detail.is_none()
                                && view.settings.is_none()
                                && view.account.is_none()
                                && view.login.is_none()
                            {
                                view.refresh_current_page(cx);
                            }
                        }
                        // 顶部继续下拉（触控板 overscroll）也当作刷新手势；
                        // 必须重新向下滚过一点，才允许下一次下拉刷新。
                        if y < -1.0 {
                            view.pull_refresh_triggered = false;
                        }
                        if y > 72.0
                            && !view.pull_refresh_triggered
                            && view.playing.is_none()
                            && view.dynamic_detail.is_none()
                            && view.settings.is_none()
                            && view.account.is_none()
                            && view.login.is_none()
                        {
                            view.pull_refresh_triggered = true;
                            view.top_refresh_armed = false;
                            view.refresh_current_page(cx);
                        }

                        // 触底自动加载：距离底部 240px 内触发；请求中会被上层 guard 挡住。
                        if max > 0.0
                            && y < -80.0
                            && -y >= max - 240.0
                            && view.playing.is_none()
                            && view.dynamic_detail.is_none()
                            && view.settings.is_none()
                            && view.account.is_none()
                            && view.login.is_none()
                        {
                            view.auto_load_more(cx);
                        }

                        if (y - view.last_scroll_offset_y).abs() > 0.5 {
                            view.last_scroll_offset_y = y;
                            // 记录滚动时间；hover 状态仍在更新（指针下的卡片
                            // 会变），但渲染时暂停封面动画，等滚动停下再恢复。
                            view.mark_scrolling();
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        // 系统「减少动效」：macOS / Windows 没有可订阅的通知，
        // 启动时 gpui-base 已经读过一次；运行中改开关只能轮询重读。
        // Linux 由 gpui-base 跟随 portal 信号，不需要这里再轮。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if !self.motion_watchdog_started {
            self.motion_watchdog_started = true;
            cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let alive = this.update(cx, |_view, cx| {
                    let before = cx.reduce_motion();
                    gpui::base::apply_system_reduce_motion(cx);
                    let after = cx.reduce_motion();
                    if before != after {
                        eprintln!("[motion] reduce_motion {before} -> {after}");
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            })
            .detach();
        }
        // 没有恢复会话时，首帧后自动弹一次登录卡片；取消后不再打扰。
        // 不在 render 里直接 cx.new，避免递归渲染。
        if self.session.is_none() && self.login.is_none() && !self.login_prompted {
            self.login_prompted = true;
            cx.defer_in(window, |view, _window, cx| view.ensure_login(cx));
        }

        // 历史页的搜索框必须在首帧 render 里懒建（InputState 需要 Window）。
        if self.page == Some(pages::PageKind::History) {
            self.ensure_history_search_input(window, cx);
        }
        // 顶栏搜索框常驻，同样只能在拿到 Window 之后创建。
        self.ensure_search_input(window, cx);
        self.messages.window_active = window.is_window_active();
        // 消息页：补焦点 / 私信输入框、处理延迟滚动与 Enter 发送。
        if self.page == Some(pages::PageKind::Messages) {
            self.messages_prepare_frame(window, cx);
        }

        let t = &self.theme;

        let layout_bounds = self.layout_bounds.clone();
        let weak_root = cx.entity().downgrade();
        div()
            .relative()
            .size_full()
            // 顶栏 / 播放浮窗 / 账号面板 / 动态详情都在页面图片缓存范围之外；
            // GPUI 的 `img()` 没有 `ImageCache` 就不会发请求（评论头像、顶栏头像
            // 都只会剩渐变占位）。这里给整棵树挂一个 retain-all 缓存兜底，
            // 内容区仍由内层 `PageImageCache` 按视口淘汰。
            .image_cache(retain_all("app-remote-images"))
            // 点空白处收起搜索联想面板；搜索框与面板自己会 stop_propagation。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                    if view.search.open {
                        view.search.open = false;
                        view.search.selected = None;
                        cx.notify();
                    }
                }),
            )
            .capture_key_down(cx.listener(
                |view, event: &KeyDownEvent, _window, cx| {
                    let keystroke = &event.keystroke;
                    // macOS 的「平台键」是 Command；Windows 用 Alt+Q：
                    // Win+Q 会被系统搜索抢走，Ctrl+Q 又容易和输入框快捷键混淆。
                    // 两边都走同一套「第一次提示、3 秒内再按一次才退出」的状态机。
                    #[cfg(target_os = "windows")]
                    let quit_modifier = keystroke.modifiers.alt;
                    #[cfg(not(target_os = "windows"))]
                    let quit_modifier = keystroke.modifiers.platform;
                    if quit_modifier && keystroke.key.eq_ignore_ascii_case("q") {
                        // 按住不放产生的自动重复不算「再按一次」：忽略，
                        // 必须松开后重新按下才确认退出。
                        if !event.is_held && view.request_quit(cx) {
                            cx.quit();
                        }
                        cx.stop_propagation();
                    }
                },
            ))
            // ⓪ 页面原点探针：封面快照用它把窗口坐标换算成页面坐标。
            //    canvas 自己不可见、不吃点击，每帧 prepaint 更新。
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        layout_bounds.set(Some(bounds));
                    },
                    move |_, _, window, _cx| {
                        let weak = weak_root.clone();
                        window.on_mouse_event(
                            move |event: &ScrollWheelEvent, phase, window, cx| {
                                if phase != DispatchPhase::Capture {
                                    return;
                                }
                                let _ = weak.update(cx, |root, cx| {
                                    root.handle_smooth_scroll(event, window, cx)
                                });
                            },
                        );
                    },
                )
                .absolute()
                .inset_0(),
            )
            // ① 背景渐变
            // GPUI 的 linear_gradient 只吃 2 个色标（[LinearColorStop; 2]），
            // 而设计稿是三段（#DCF1F9 / #EFF2F7 / #F2F2F8）。
            // 中间段与两端差异很小，这里取首尾两段近似；如需精确三段落差，
            // 要在上层再叠一条只覆盖中段的渐变。
            .bg(linear_gradient(
                135.0,
                linear_color_stop(t.bg_gradient_stops()[0].0, 0.0),
                linear_color_stop(t.bg_gradient_stops()[2].0, 1.0),
            ))
            // ② 主题色光晕（径向）
            .child(glow_layer(t))
            // ③ 页面内容
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(top_bar(
                        t,
                        self.session.as_ref().map(|s| s.face_url.clone()),
                        self.account.is_some(),
                        &self.search,
                        window,
                        cx,
                    ))
                    // 顶栏下方直接是内容区：分类条已删除（与底部 Dock 重复），
                    // 页面切换统一由 Dock 承担。
                    .children(if self.offline {
                        Some(offline_banner(
                            t,
                            self.offline_updated_at,
                            self.feed.items().len(),
                            self.offline_retrying,
                            self.offline_error.is_some(),
                            cx,
                        ))
                    } else {
                        None
                    })
                    .child({
                        let content = content_area(
                            t,
                            self.feed.items(),
                            self.feed_loading,
                            self.player_page_hidden && self.playing.is_some(),
                            self.page,
                            self.page_state,
                            self.session.is_some(),
                            &self.history,
                            &self.favorites,
                            &self.rank,
                            &self.search,
                            &self.messages,
                            self.hovered_card,
                            self.leaving_card,
                            self.hover_generation,
                            self.scroll_settling(),
                            dynamic_feed::FeedState {
                                items: &self.following,
                                loading: self.following_loading,
                                error: self.following_error.as_deref(),
                                loaded: self.following_loaded,
                                has_more: self.following_has_more,
                                loading_more: self.following_loading_more,
                                signed_in: self.session.is_some(),
                            },
                            self.layout_bounds.clone(),
                            self.active_images.clone(),
                            window,
                            &self.content_scroll,
                            cx,
                        );
                        if let Some(cache) = self.page_image_cache.clone() {
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .w_full()
                                .min_h(px(0.))
                                .image_cache(cache)
                                .child(content)
                                .into_any_element()
                        } else {
                            content
                        }
                    }),
            )
            // Dock 是「页面场景内的浮层」，挂在内容之后，横向浮在所有封面之上
            .child(dock(
                t,
                self.page,
                self.session.is_some(),
                self.messages.unread.total(),
                cx,
            ))
            // 搜索联想面板：挂在根层，天然压在内容与 Dock 之上、播放浮窗之下。
            .children(search_view::suggest_overlay(t, &self.search, window, cx))
            // 尚未接入的动作给一个明确提示，避免「点了没反应」。
            .children(
                self.search
                    .notice
                    .as_deref()
                    .map(|text| search_view::notice(t, text)),
            )
            // 历史页的删除确认：盖住内容与 Dock，但仍在播放浮窗之下。
            .children(self.history.confirm.as_ref().map(|kind| {
                history_view::confirm_overlay(
                    t,
                    kind,
                    self.history.delete_busy,
                    self.history.action_error.as_deref(),
                    cx,
                )
            }))
            // 消息页的移除会话确认 / 轻提示。
            .children(self.messages.confirm.as_ref().map(|confirm| {
                messages_view::confirm_overlay(
                    t,
                    confirm,
                    self.messages.confirm_busy,
                    self.messages.action_error.as_deref(),
                    cx,
                )
            }))
            .children(
                self.messages
                    .notice
                    .as_deref()
                    .map(|text| messages_view::notice(t, text)),
            )
            // 刷新 / 触底加载的浮层提示
            .children(
                (self.feed_refreshing || self.following_refreshing)
                    .then(|| status_pill(t, "正在刷新当前页面…", true)),
            )
            .children(
                (self.feed_loading_more || self.following_loading_more)
                    .then(|| status_pill(t, "正在加载更多…", false)),
            )
            // ④ 播放浮窗：独立场景，整块压在页面之上。
            //    它必须挂在页面内容**之后**，且不与页面内容平铺成兄弟层。
            .children(self.gallery.clone())
            .children(self.playing.clone())
            // 动态详情浮层压在页面与播放器之上；点外部缩回原卡片。
            .children(self.dynamic_detail.clone())
            // 登录卡片在播放器之上，保证登录时不会被视频窗口盖住。
            .children(self.login.clone())
            // 已登录账号面板压在页面之上；点顶栏头像打开。
            .children(self.account.clone())
            // 设置浮层再压一层；从个人面板进入。
            .children(self.settings.clone())
            // ⌘Q 退出确认压在全部内容之上。
            .children(self.quit_hint(t))
    }
}

/// 滚动触发的轻量提示：回顶刷新在顶部，触底加载在 Dock 上方。
fn status_pill(t: &YkTheme, text: &str, at_top: bool) -> AnyElement {
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

// ================= ② 光晕 =================

fn glow_layer(t: &YkTheme) -> impl IntoElement {
    let glow = t.bg_accent_glow_stops();
    div().absolute().inset_0().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(760.))
            .h(px(520.))
            // GPUI 只有线性渐变，没有 `radial_gradient`。
            // 设计稿是左上角为中心的径向光晕（70% × 55% 椭圆）；
            // 这里用一条垂直渐变从主题色衰减到透明来近似 —— 视觉上最接近，
            // 真正的径向衰减需要自定义 Element 画（列为已知限制）。
            .bg(linear_gradient(
                160.0,
                linear_color_stop(glow[0].0, 0.0),
                linear_color_stop(glow[1].0, 1.0),
            )),
    )
}

// ================= 顶栏（64px） =================

/// 当前页面是否有可重拉的数据。
///
/// 顶栏刷新与 Dock「刷新」共用这一份判断：没有数据可拉的页面
/// （如热门 / 分区 / 我的）把入口置灰，而不是点了没反应。
fn can_refresh_page(page: Option<pages::PageKind>, signed_in: bool) -> bool {
    page.is_none()
        || page == Some(pages::PageKind::Search)
        || page == Some(pages::PageKind::Rank)
        || (matches!(
            page,
            Some(pages::PageKind::Following)
                | Some(pages::PageKind::History)
                | Some(pages::PageKind::Favorites)
                | Some(pages::PageKind::Messages)
        ) && signed_in)
}

/// 顶栏（64px）。
///
/// 视觉口径参考 macOS 浏览器工具栏：深色半透明底、两侧成组的胶囊按钮、
/// 中间一根更宽的搜索框。功能上不新增导航入口 —— 页面切换仍只由底部
/// Dock 承担；刷新也仍然只在 Dock 里，避免同一条顶栏出现两个刷新。
fn top_bar(
    t: &YkTheme,
    face_url: Option<String>,
    hide_avatar: bool,
    search: &search_view::SearchState,
    window: &Window,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let bar = div()
        .id("topbar-drag")
        .relative()
        // 整条顶栏都能按住拖动窗口。控件组各自截住 mouse_down，
        // 所以点头像 / 输入框 / 窗口按钮不会误拖窗口。
        .on_mouse_down(MouseButton::Left, |_, window, _| begin_window_drag(window))
        .flex()
        .items_center()
        .w_full()
        .h(px(TOP_BAR_HEIGHT))
        // 窗口再矮也不能把工具栏压扁：内容区自己滚动 / 留白。
        .flex_shrink_0()
        // macOS 透明标题栏的红黄绿压在顶栏左上：按钮占 20..72，头像胶囊从 92
        // 开始，留出呼吸位。Windows 没有红黄绿，头像直接贴左内边距。
        .pl(px(if cfg!(target_os = "macos") {
            SPACE_5 + 76.0
        } else {
            SPACE_5
        }))
        // Windows 的三个窗口按钮贴右边缘，方便按角落关闭；macOS 保留右边距。
        .pr(px(if cfg!(target_os = "windows") {
            0.0
        } else {
            SPACE_5
        }))
        .gap(px(SPACE_4))
        // 比卡片 / 浮层更暗、带一点透明：工具栏是「窗口的外壳」，不抢内容的视觉重量。
        // 内容不与顶栏重叠（顶栏是 flex 列的第一个 child），所以这里可以放心用
        // 半透明表面；GPUI 没有元素级 backdrop blur，透出的是页面底色与光晕。
        .bg(s.surface_toolbar)
        .border_b_1()
        .border_color(s.border_subtle)
        // 头像从右上角挪到原来品牌胶囊的位置；右上角让给窗口按钮。
        .child(toolbar_group(t, face_url, hide_avatar, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .child(search_view::search_bar(t, search, window, cx)),
        );

    #[cfg(target_os = "windows")]
    let bar = bar.child(window_controls(t, window, cx));

    bar
}

/// 左上角头像胶囊。
///
/// 暂时只保留头像，不放假设置入口；胶囊只作为材质容器，
/// 等有真实功能（设置 / 通知等）再往里加图标。容器截住 mouse_down，
/// 点头像不会误拖窗口。
fn toolbar_group(
    t: &YkTheme,
    face_url: Option<String>,
    hide_avatar: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_0_5))
        .p(px(6.))
        .h(px(TOP_BAR_CONTROL_HEIGHT))
        .flex_shrink_0()
        .rounded_full()
        .bg(s.fill_2)
        .border_1()
        .border_color(s.border_strong)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(avatar(t, face_url, hide_avatar, cx))
}

// ================= Windows 自绘标题栏 =================

/// 取当前窗口的 HWND（Win32）。
#[cfg(target_os = "windows")]
fn window_hwnd(window: &Window) -> Option<*mut core::ffi::c_void> {
    use raw_window_handle::RawWindowHandle;

    // `Window` 自己也有 `window_handle()`（返回 `AnyWindowHandle`），
    // 必须走 trait 的全限定调用。
    let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get() as *mut core::ffi::c_void),
        _ => None,
    }
}

/// 按住顶栏拖动窗口。
///
/// Windows 的 GPUI 后端没有实现 `start_window_move`，这里用 Win32 标准做法：
/// 释放鼠标捕获后向窗口发 `WM_NCLBUTTONDOWN` + `HTCAPTION`，
/// 由系统进入原生的窗口移动循环（和标题栏拖动完全一致）。
fn begin_window_drag(window: &Window) {
    #[cfg(target_os = "windows")]
    {
        if let Some(hwnd) = window_hwnd(window) {
            unsafe {
                ReleaseCapture();
                // 必须用 PostMessage：SendMessage 会在当前 mouse_down 处理函数里
                // 同步进入系统移动循环，GPUI 的窗口过程被重入，App 的 RefCell
                // 会直接 panic（already borrowed）。Post 出去等这一拍处理完再拖动。
                PostMessageW(hwnd, 0x00A1, 2, 0);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    window.start_window_move();
}

#[cfg(target_os = "windows")]
fn is_window_zoomed(window: &Window) -> bool {
    window_hwnd(window).is_some_and(|hwnd| unsafe { IsZoomed(hwnd) != 0 })
}

#[cfg(target_os = "windows")]
fn toggle_maximize(window: &Window) {
    if let Some(hwnd) = window_hwnd(window) {
        // 同样不能同步 ShowWindow：它会在 on_click 里立刻发 WM_SIZE，
        // GPUI 重入后拿不到新视口，画面会停在旧的 1280x820（右侧留白）。
        // 交给消息队列，等当前事件处理完再最大化 / 还原。
        let command = if is_window_zoomed(window) {
            0xF120 // SC_RESTORE
        } else {
            0xF030 // SC_MAXIMIZE
        };
        unsafe {
            PostMessageW(hwnd, 0x0112, command, 0);
        }
    }
}

/// 标题栏右侧的最小化 / 最大化（还原）/ 关闭。
#[cfg(target_os = "windows")]
fn window_controls(t: &YkTheme, window: &Window, cx: &mut Context<AppRoot>) -> impl IntoElement {
    let s = &t.surfaces;
    let close_hover: Hsla = rgb(0xC42B1C).into();
    let maximize_icon = if is_window_zoomed(window) {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };

    div()
        .flex()
        .items_center()
        .h_full()
        .flex_shrink_0()
        .child(caption_button(
            "win-minimize",
            IconName::WindowMinimize,
            s.fill_3,
            s.text_primary,
            cx,
            |window, _| window.minimize_window(),
        ))
        .child(caption_button(
            "win-maximize",
            maximize_icon,
            s.fill_3,
            s.text_primary,
            cx,
            |window, _| toggle_maximize(window),
        ))
        .child(caption_button(
            "win-close",
            IconName::WindowClose,
            close_hover,
            s.text_primary,
            cx,
            |_, cx| cx.quit(),
        ))
}

#[cfg(target_os = "windows")]
fn caption_button(
    id: &'static str,
    icon: IconName,
    hover_bg: Hsla,
    icon_color: Hsla,
    cx: &mut Context<AppRoot>,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .occlude()
        // 点按钮不能触发顶栏拖动。
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |_view, _: &ClickEvent, window, cx| {
            eprintln!("[window] 标题栏按钮：{id}");
            action(window, cx);
            // 最大化 / 还原后重绘，图标与悬停态立即切过去。
            cx.notify();
            cx.stop_propagation();
        }))
        .hover(move |style| style.bg(hover_bg))
        .child(Icon::new(icon).size(px(13.)).text_color(icon_color))
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn ReleaseCapture() -> i32;
    fn PostMessageW(
        hwnd: *mut core::ffi::c_void,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> i32;
    fn IsZoomed(hwnd: *mut core::ffi::c_void) -> i32;
}

fn avatar(
    t: &YkTheme,
    face_url: Option<String>,
    hidden: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let p = &t.palette;
    let face = face_url.filter(|u| !u.is_empty());
    div()
        .id("topbar-avatar")
        .w(px(34.))
        .h(px(34.))
        .rounded_full()
        .overflow_hidden()
        .border_1()
        .border_color(t.surfaces.border_popover)
        .bg(linear_gradient(
            135.,
            linear_color_stop(p.bili_blue, 0.),
            linear_color_stop(p.bili_pink, 1.),
        ))
        .cursor_pointer()
        // 头像点击是真实交互，必须挡住顶栏拖拽，否则一点就变成拖窗口。
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
            if view.session.is_some() {
                view.ensure_account(cx);
            } else {
                view.ensure_login(cx);
            }
        }))
        .when_some(face, |el, url| {
            el.child(
                img(ImageSource::from(url))
                    .w(px(34.))
                    .h(px(34.))
                    .object_fit(ObjectFit::Cover),
            )
        })
        // 账号面板展开时隐藏原位头像；面板缩回完成后 AppRoot 移除 account，
        // 这一帧头像重新出现，正好接住缩回动画。
        .when(hidden, |el| el.invisible())
}

// ================= 离线条 =================

/// 顶部离线条：不遮挡内容，只说明「现在是离线 + 缓存时间」，并给一个重试入口。
fn offline_banner(
    t: &YkTheme,
    updated_at: Option<u64>,
    count: usize,
    retrying: bool,
    retry_failed: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut text = match updated_at {
        Some(ts) => format!(
            "离线 · 缓存时间：{} · {} 条",
            yk_core::feed_cache::age_text(ts),
            count
        ),
        None => "离线 · 暂无可用缓存，正在展示占位内容".to_string(),
    };
    if retrying {
        text = "正在重新连接…".to_string();
    } else if retry_failed {
        text.push_str(" · 重试失败");
    }

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
                .child(text),
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
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.retry_online(cx);
                    window.refresh();
                })),
        )
}

// ================= 内容区 =================

fn content_area(
    t: &YkTheme,
    items: &[CoverItem],
    loading: bool,
    behind_player: bool,
    page: Option<pages::PageKind>,
    state: pages::PageState,
    signed_in: bool,
    history: &history_view::HistoryState,
    favorites: &favorites_view::FavoritesState,
    rank: &rank_view::RankState,
    search: &search_view::SearchState,
    messages: &messages_view::MessageState,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    scroll_settling: bool,
    following: dynamic_feed::FeedState<'_>,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    scroll: &VirtualListScrollHandle,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    // 滚动期间暂停卡片的 hover / 收尾视觉：卡片在指针下滑过时 GPUI 会连发
    // hover 事件，若每次都按新代际重锚，封面缩放会反复重置、重播。
    // 状态照常更新（指针最后停在哪张卡上不能丢），只是这一帧不画动画。
    let (hovered_card, leaving_card) = if scroll_settling {
        (None, None)
    } else {
        (hovered_card, leaving_card)
    };
    // Dock 现在是底部横向浮层：横向不再占位，内容区左右各留 24；
    // 底部留出 `DOCK_RESERVED`，否则最后一行封面会被 Dock 压住 ——
    // 浮层「在上」不等于内容可以钻到它背后。
    if behind_player {
        return div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .bg(t.surfaces.bg_base)
            .into_any_element();
    }

    // 消息页自带虚拟列表与输入框，滚动由页内自己管，不接外壳的 content-scroll。
    if page == Some(pages::PageKind::Messages) {
        return div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .bg(t.surfaces.bg_base)
            .child(messages_view::page(t, messages, active_images, window, cx))
            .into_any_element();
    }

    // 历史页自带左右分栏 + 左栏独立滚动，不接外壳的 `content-scroll`：
    // 右栏必须固定，跟着整页滚就失去「日期导航」的意义。
    if page == Some(pages::PageKind::History) {
        return div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .bg(t.surfaces.bg_base)
            .child(history_view::page(
                t,
                history,
                signed_in,
                active_images,
                layout_bounds.clone(),
                window,
                cx,
            ))
            .into_any_element();
    }

    if let Some(kind) = page {
        // 「正在关注」是瀑布流，需要窗口宽度和 AppRoot 的加载回调；
        // 其余页面仍交给 pages 模块。
        let body: AnyElement = if kind == pages::PageKind::Following {
            let offset_y = scroll.base_handle().offset().y.as_f32();
            let viewport_h = scroll.base_handle().bounds().size.height.as_f32();
            let visible_y = if viewport_h > 0.0 {
                (-offset_y)..(-offset_y + viewport_h)
            } else {
                0.0..window.viewport_size().height.as_f32()
            };
            dynamic_feed::page(
                t,
                following,
                visible_y,
                active_images.clone(),
                layout_bounds,
                window,
                cx,
            )
        } else if kind == pages::PageKind::Search {
            search_view::page(
                t,
                search,
                hovered_card,
                leaving_card,
                hover_generation,
                layout_bounds,
                active_images.clone(),
                window,
                cx,
            )
        } else if kind == pages::PageKind::Favorites {
            favorites_view::page(
                t,
                favorites,
                signed_in,
                hovered_card,
                leaving_card,
                hover_generation,
                layout_bounds,
                active_images.clone(),
                window,
                cx,
            )
        } else if kind == pages::PageKind::Rank {
            rank_view::page(
                t,
                rank,
                hovered_card,
                leaving_card,
                hover_generation,
                layout_bounds,
                window,
                scroll,
                active_images.clone(),
                cx,
            )
        } else {
            pages::render(t, kind, state, signed_in).into_any_element()
        };
        // 页面容器不透明铺满，否则底层光晕会从内容区两侧透上来。
        return div()
            .id("content-scroll")
            .flex_1()
            // flex 子项的 min-height 默认是 auto，不压到 0 的话内容会撑开父级，
            // overflow_y_scroll 就不会真正滚动。
            .min_h(px(0.))
            .track_scroll(scroll.base_handle())
            .overflow_y_scroll()
            .bg(t.surfaces.bg_base)
            .child(
                div()
                    .px(px(SPACE_6))
                    .pt(px(SPACE_4))
                    .pb(px(DOCK_RESERVED))
                    .w_full()
                    .child(body),
            )
            .into_any_element();
    }

    // ---- 首页：按视口宽把卡片分组为「行」，交给 v_virtual_list ----
    // 列数 = 可用宽度能放下几个 208 + 16 的槽位；至少 1 列。
    // 窗口缩放时列数会变，item_sizes 每帧重算（行数很少，成本可忽略）。
    let available = (window.viewport_size().width.as_f32() - SPACE_6 * 2.0).max(COVER_CELL_WIDTH);
    let cols = home_columns(window.viewport_size().width.as_f32());
    // 行高 = 卡片高 + 行间距；行内只画卡片，多出来的 16 就是行距。
    let row_h = COVER_CELL_HEIGHT + SPACE_4;

    // 首屏加载：用与真实卡片完全同几何的骨架卡，避免数据回来时网格跳动。
    // 只用一个 repeat 动画驱动整块网格的 opacity（1 → 0.45 → 1，1.5s），
    // 不在每张骨架卡上各排一条动画，也不引入任何额外颜色。
    if loading && items.is_empty() {
        let viewport_h = window.viewport_size().height.as_f32();
        let available_h = (viewport_h - TOP_BAR_HEIGHT - SPACE_4 - DOCK_RESERVED).max(row_h);
        let skeleton_rows = ((available_h / row_h).floor() as usize).max(2);
        let ease = ease_standard();
        return div()
            .flex()
            .flex_col()
            .gap(px(SPACE_4))
            .w_full()
            .px(px(SPACE_6))
            .pt(px(SPACE_4))
            .pb(px(DOCK_RESERVED))
            .children((0..skeleton_rows).map(|_| {
                div()
                    .flex()
                    .justify_center()
                    .gap(px(SPACE_4))
                    .w_full()
                    .h(px(COVER_CELL_HEIGHT))
                    .children((0..cols).map(|_| skeleton_card(t)))
            }))
            .with_animation(
                "home-card-skeleton-pulse",
                Animation::new(std::time::Duration::from_millis(SKELETON_PULSE_MS)).repeat(),
                move |el, delta| {
                    // 三角波：0 → 0.5 → 1 对应 1 → 0.45 → 1。
                    let phase = if delta < 0.5 {
                        delta * 2.0
                    } else {
                        (1.0 - delta) * 2.0
                    };
                    el.opacity(1.0 - 0.55 * ease(phase))
                },
            )
            .into_any_element();
    }

    let rows = items.len().div_ceil(cols);
    let sizes = Rc::new(vec![size(px(available), px(row_h)); rows]);

    // 首页的真实可视范围从滚动偏移算，而不是用虚拟列表的 overdraw 范围：
    // 后者会多给 2 行，拿它当加载范围会让最后一行「可见但没图」。
    let viewport_h = (window.viewport_size().height.as_f32() - TOP_BAR_HEIGHT).max(row_h);
    let offset_y = scroll.base_handle().offset().y.as_f32();
    let first_visible = (((-offset_y) - SPACE_4).max(0.0) / row_h).floor() as usize;
    let last_visible = (((-offset_y) + viewport_h - SPACE_4).max(0.0) / row_h).ceil() as usize;
    let load_start = first_visible.min(rows);
    let load_end = (last_visible + 1).min(rows).max(load_start);

    // 即使虚拟列表这一帧没有调用 render_items，也要让缓存知道当前该保留哪些图。
    for row in load_start..load_end {
        let start = row * cols;
        let end = (start + cols).min(items.len());
        for item in &items[start..end] {
            active_images.borrow_mut().mark(&item.cover_thumb_url());
            if !item.up_face.is_empty() {
                active_images.borrow_mut().mark(&item.up_face);
            }
        }
    }

    let entity = cx.entity();
    let theme = t.clone();
    let active_images_for_grid = active_images.clone();
    v_virtual_list(
        entity,
        "home-grid",
        sizes,
        move |this, visible_rows, _window, cx| {
            visible_rows
                .map(|row| {
                    let start = row * cols;
                    let all = this.feed.items();
                    let end = (start + cols).min(all.len());
                    // 卡片行整体居中：窗口变宽时多出来的空间均分到两侧，
                    // 而不是全堆在右边（否则网格看起来永远偏左上）。
                    let mut row_el = div()
                        .flex()
                        .justify_center()
                        .gap(px(SPACE_4))
                        .w_full()
                        .h(px(COVER_CELL_HEIGHT));
                    let hovered_idx = if this.scroll_settling() {
                        None
                    } else {
                        this.hovered_card
                    };
                    let leaving_idx = if this.scroll_settling() {
                        None
                    } else {
                        this.leaving_card
                    };
                    let hover_gen = this.hover_generation;
                    let root_bounds = this.layout_bounds.clone();
                    let load_cover = row >= load_start && row < load_end;
                    for item in &all[start..end] {
                        let hovered = hovered_idx == Some(item.index);
                        let leaving = leaving_idx == Some(item.index);
                        row_el = row_el.child(card(
                            &theme,
                            item,
                            hovered,
                            leaving,
                            hover_gen,
                            root_bounds.clone(),
                            load_cover,
                            active_images_for_grid.clone(),
                            cx,
                        ));
                    }
                    row_el
                })
                .collect::<Vec<_>>()
        },
    )
    .track_scroll(scroll)
    .px(px(SPACE_6))
    .pt(px(SPACE_4))
    .pb(px(DOCK_RESERVED))
    .into_any_element()
}

/// 封面卡片：208 × 184（COVER_CELL_WIDTH / COVER_CELL_HEIGHT）。
///
/// 结构对应设计文档第 04 节「封面卡片解剖」：
///
/// ```text
/// 208×184
///   ├─ 8 padding
///   ├─ 封面 192×108（16:9，圆角 12）
///   │    ├─ 渐变卡面 + 大标题（真实封面加载前/失败时的兜底）
///   │    ├─ 真实封面图
///   │    ├─ 底部压暗渐变（保证数据层可读）
///   │    ├─ 左下 播放量 + 弹幕数 · 右下 时长
///   │    ├─ 悬停操作（稍后再看 / 下载 / 更多）
///   │    └─ 观看进度条（仅看过的视频）
///   ├─ 标题 2 行（13 / 18）
///   └─ 署名行：UP 头像 18 + 昵称 + 相对时间
/// ```
///
/// 渐变卡面是**自绘**的：主题令牌只有单色，设计稿的 135° 两色渐变
/// 由占位色派生暗/亮两档后用 `linear_gradient` 画出来。
fn card(
    t: &YkTheme,
    item: &CoverItem,
    hovered: bool,
    leaving: bool,
    hover_generation: u64,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    load_cover: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let title = item.title.clone();
    let bvid = item.bvid.clone();
    let idx = item.index;
    // 与顶栏胶囊同一套「玻璃」：fill_2 底 + border_strong 边 + 带内高光的阴影。
    let hover_shadow = t.shadows.shadow_glass_card.clone();
    let hover_border = s.border_strong;
    let hover_bg = s.fill_2;
    // 当前帧封面几何：prepaint 写入，点击时读取；不 notify。
    let cover_source: Rc<RefCell<Option<player::CoverSource>>> = Rc::new(RefCell::new(None));
    let cover_image_url = item.cover_thumb_url();
    let cover_placeholder = item.placeholder_color.clone();

    // 封面只有「卡面」参与 1.04 缓推：渐变/真实封面在 art 层，
    // 数据层、时长、操作按钮、进度条都留在固定层上，不跟着放大。
    let cover_bg = placeholder_gradient(&item.placeholder_color, s.fill_3);
    let mut art = div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h_full()
        .bg(cover_bg)
        // 渐变卡面中央的大标题（设计稿 `.thumb .art`）：
        // 真实封面加载后盖在上面；加载中 / 失败时这里就是卡面主视觉。
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(SPACE_3))
                .child(
                    div()
                        .text_size(px(16.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .text_center()
                        .line_clamp(3)
                        .child(title.clone()),
                ),
        );

    let cover_url = item.cover_thumb_url();
    if load_cover && !cover_url.is_empty() {
        active_images.borrow_mut().mark(&cover_url);
        art = art.child(
            // 封面图：B 站图床走 384×216 缩略图（内存/解码开销），
            // 显式 100% 宽高 + Cover 才能铺满封面；只写 inset 会退回原图尺寸，
            // 表现为「只看见左上角」。
            img(ImageSource::from(cover_url))
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }

    let art = animate_cover_zoom(art, hovered, leaving, idx, hover_generation);
    let mut cover = div()
        .relative()
        // 封面宽 = 卡片内容宽（192）；卡片 hover 时内容层外扩，
        // 封面跟着变宽，而不是只平移。高用同一比例的 108/168。
        .w_full()
        .h(relative(
            COVER_ITEM_HEIGHT / (COVER_CELL_HEIGHT - 2.0 * SPACE_2),
        ))
        .rounded(px(MEDIA_RADIUS))
        .overflow_hidden()
        .child(art);

    // 直播角标：粉色实心胶囊 + 白字 + 18 高（设计文档 13.9）。
    if item.is_live {
        cover = cover.child(live_badge(t));
    }

    // 底部压暗：设计稿要求数据层文字在任何封面上都可读（GPUI 没有
    // text-shadow，用一层透明→黑的线性渐变兜底，位置固定在封面下沿 40px）。
    cover = cover.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(40.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgba(0x00000000), 0.),
                linear_color_stop(rgba(0x000000B3), 1.),
            )),
    );

    // 数据层：左下 播放量 + 弹幕数，右下 时长。
    cover = cover
        .child(
            div()
                .absolute()
                .left(px(SPACE_2))
                .bottom(px(6.))
                .flex()
                .items_center()
                .gap(px(SPACE_3))
                .child(cover_stat(
                    IconName::Play,
                    yk_core::format::count(item.view_count),
                ))
                .child(cover_stat(
                    IconName::MessageCircle,
                    yk_core::format::count(item.danmaku_count),
                )),
        )
        .when(item.duration_secs > 0, |el| {
            el.child(
                div()
                    .absolute()
                    .right(px(SPACE_2))
                    .bottom(px(6.))
                    .text_size(px(FONT_SIZE_DATA_OVERLAY))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(0xFFFFFF))
                    .child(item.duration_text()),
            )
        });

    // 悬停操作：进入 150ms 浮出（opacity 0→1 + 4px 下移到位）；
    // 离开也保留 150ms 收尾。只在 hovered / leaving 时挂进树里。
    if hovered || leaving {
        let actions = div()
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .flex()
            .gap(px(SPACE_1))
            .child(hover_action(t, idx, "later", IconName::Clock, cx))
            .child(hover_action(t, idx, "download", IconName::Download, cx))
            .child(hover_action(t, idx, "more", IconName::Ellipsis, cx));
        cover = cover.child(animate_hover_actions(
            actions,
            hovered,
            leaving,
            idx,
            hover_generation,
        ));
    }

    // 观看进度：仅当本地有进度时出现（数据字段已在 CoverItem 里）。
    if item.progress_secs > 0 && item.duration_secs > 0 {
        let ratio = (item.progress_secs as f32 / item.duration_secs as f32).clamp(0., 1.);
        cover = cover.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(3.))
                .bg(rgba(0xFFFFFF40))
                .child(div().h_full().w(relative(ratio)).bg(p.bili_pink)),
        );
    }

    let source_bvid = bvid.clone();
    let source_title = title.clone();
    let source_for_prepaint = cover_source.clone();
    let root_for_prepaint = root_bounds;
    let content = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        // 8 padding：192 封面 + 184 单元格几何都从这里推出来。
        .p(px(SPACE_2))
        .child(cover)
        .child(
            div()
                // 标题/署名固定 192 宽：卡片 hover 放大时文本不再重排。
                .w(px(COVER_ITEM_WIDTH))
                .flex()
                .flex_col()
                .child(
                    div()
                        .mt(px(SPACE_1))
                        .h(px(LINE_HEIGHT_CONTROL * 2.))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(2)
                        .child(title.clone()),
                )
                .child(byline(t, item, load_cover, active_images.clone())),
        )
        // 子元素 0 是封面。记录实际布局（含 hover 放大后的 inset），
        // 点击时用它作为浮窗展开的起点；这里只写 Rc，不触发重绘。
        .on_children_prepainted(move |children, _window, _cx| {
            let Some(cover) = children.first().copied() else {
                return;
            };
            let Some(root) = root_for_prepaint.get() else {
                return;
            };
            *source_for_prepaint.borrow_mut() = Some(player::CoverSource {
                bounds: Bounds {
                    origin: point(
                        cover.origin.x - root.origin.x,
                        cover.origin.y - root.origin.y,
                    ),
                    size: cover.size,
                },
                root_size: root.size,
                bvid: source_bvid.clone(),
                title: source_title.clone(),
                image_url: cover_image_url.clone(),
                placeholder_color: cover_placeholder.clone(),
            });
        });

    // 卡片整体 1.03：设计稿 150ms / EaseStandard，用内层负 inset 近似。
    // 封面 1.04 缓推在 art 层单独做（300ms），互不抢一条动画。
    let content = animate_card_scale(content, hovered, leaving, idx, hover_generation);

    // **固定热区**：hover 挂在这个 208×184 的透明层上，点击挂在同尺寸外框上；
    // 两者都不参与缩放动画。内容层可以放大、封面可以缓推，但鼠标的命中范围始终不变，
    // 不会出现「鼠标停在边界 → 动画放大 → 再次命中 → 反复触发」的循环。
    let source_for_click = cover_source.clone();
    let hit_layer = div()
        .id(("cover-hit", idx))
        .absolute()
        .inset_0()
        .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
            // 播放浮窗打开期间，不允许下层封面响应 hover。
            if this.playing.is_some() {
                return;
            }
            // 滚动期间卡片从指针下滑过，GPUI 会连续给出 true/false。
            // 状态仍然要跟（指针最后停在谁上面），但**不推进动画代际**：
            // 渲染侧同时把 hover/leaving 视觉暂停，滚动时就不会反复重置缩放。
            let animate = !this.scroll_settling();
            if this.update_card_hover(idx, *hovered, animate) {
                cx.notify();
            }
        }));

    div()
        .id(("cover", idx))
        .relative()
        .w(px(COVER_CELL_WIDTH))
        .h(px(COVER_CELL_HEIGHT))
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
        // 默认态透明（设计稿 `.card` 默认无底色），悬停时才浮出表面色。
        .bg(rgba(0x00000000))
        .border_1()
        .border_color(rgba(0x00000000))
        // 阴影/描边/背景随 hovered 状态切；缩放/缓推/操作由三条独立动画负责。
        .when(hovered, move |el| {
            el.bg(hover_bg)
                .shadow(hover_shadow)
                .border_color(hover_border)
        })
        // 点击挂在固定外框上（动作按钮是它的后代，点按钮时 stop_propagation）。
        // 这样整卡点击不依赖会缩放的内容层。
        // 已存在就复用：重建会换掉 PlayerView 里的 InputState，弹幕输入跟着丢（附录 F L-1）。
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            // 播放浮窗/遮罩已经打开时，落到下层的点击一律忽略；
            // 否则点浮窗外侧关闭的同一拍可能又打开一张封面卡。
            if this.playing.is_some() || this.close_player_requested {
                return;
            }
            // 必须在 ensure_player 展开浮窗、网格重排之前冻结封面几何。
            let mut entrance = source_for_click
                .borrow()
                .clone()
                .filter(|source| source.bvid == bvid);
            if let (Some(source), Some(root)) = (entrance.as_mut(), this.layout_bounds.get()) {
                // 窗口在 home 渲染后 resize 过：目标宽度取最新根尺寸。
                source.root_size = root.size;
            }
            eprintln!("[M3] 进入播放浮窗: {title}（{bvid}）");
            this.ensure_player(cx);
            if let Some(player) = this.playing.clone() {
                // 先给入场起点，再换播放源；播放器实例本身不重建。
                if let Some(source) = entrance {
                    let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
                }
                let _ = player.update(cx, |view, cx| view.open_bvid(bvid.clone(), cx));
            }
            window.refresh();
            cx.notify();
        }))
        // 先挂固定热区，再挂会动画的内容；热区在下、动作按钮在上。
        .child(hit_layer)
        .child(content)
}

/// 卡片整体 1.03：208×3%÷2 ≈ 3.12，150ms / EaseStandard。
const CARD_HOVER_INSET: f32 = 3.12;
/// 封面缓推 1.04：宽高各放大 4%，300ms / EaseStandard。
const COVER_HOVER_GROW: f32 = 0.04;

fn card_scale(el: Div, t: f32) -> Div {
    el.inset(px(-CARD_HOVER_INSET * t))
}

fn cover_scale(el: Div, t: f32) -> Div {
    let grow = COVER_HOVER_GROW * t;
    el.w(relative(1.0 + grow))
        .h(relative(1.0 + grow))
        .left(relative(-grow / 2.0))
        .top(relative(-grow / 2.0))
}

fn actions_scale(el: Div, t: f32, entering: bool) -> Div {
    let opacity = if entering { t } else { 1.0 - t };
    let el = el.top(px(2.0 + 4.0 * opacity)).opacity(opacity);
    // 收尾动画结束时隐藏，避免透明按钮继续吃点击。
    if opacity <= 0.001 {
        el.invisible()
    } else {
        el
    }
}

fn animate_card_scale(
    el: Div,
    hovered: bool,
    leaving: bool,
    idx: usize,
    generation: u64,
) -> AnyElement {
    if hovered {
        el.with_animation(
            format!("card-scale-in-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| card_scale(el, t),
        )
        .into_any_element()
    } else if leaving {
        el.with_animation(
            format!("card-scale-out-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| card_scale(el, 1.0 - t),
        )
        .into_any_element()
    } else {
        card_scale(el, 0.0).into_any_element()
    }
}

fn animate_cover_zoom(
    el: Div,
    hovered: bool,
    leaving: bool,
    idx: usize,
    generation: u64,
) -> AnyElement {
    if hovered {
        el.with_animation(
            format!("cover-zoom-in-{idx}-{generation}"),
            Animation::new(DURATION_MODERATE).with_easing(ease_standard()),
            |el, t| cover_scale(el, t),
        )
        .into_any_element()
    } else if leaving {
        el.with_animation(
            format!("cover-zoom-out-{idx}-{generation}"),
            Animation::new(DURATION_MODERATE).with_easing(ease_standard()),
            |el, t| cover_scale(el, 1.0 - t),
        )
        .into_any_element()
    } else {
        cover_scale(el, 0.0).into_any_element()
    }
}

fn animate_hover_actions(
    el: Div,
    hovered: bool,
    leaving: bool,
    idx: usize,
    generation: u64,
) -> AnyElement {
    if hovered {
        el.with_animation(
            format!("card-actions-in-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| actions_scale(el, t, true),
        )
        .into_any_element()
    } else if leaving {
        el.with_animation(
            format!("card-actions-out-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| actions_scale(el, t, false),
        )
        .into_any_element()
    } else {
        el.invisible().into_any_element()
    }
}

/// 首屏骨架卡：与真实卡片同几何（208×184 / 封面 192×108 / 两行标题 / 署名行）。
///
/// 颜色只用 `SkeletonBrush`，脉冲由父级网格统一驱动。
fn skeleton_card(t: &YkTheme) -> impl IntoElement {
    let brush = t.surfaces.skeleton_brush;
    div()
        .w(px(COVER_CELL_WIDTH))
        .h(px(COVER_CELL_HEIGHT))
        .flex()
        .flex_col()
        .p(px(SPACE_2))
        .child(
            div()
                .w(px(COVER_ITEM_WIDTH))
                .h(px(COVER_ITEM_HEIGHT))
                .rounded(px(MEDIA_RADIUS))
                .bg(brush),
        )
        .child(
            div()
                .mt(px(SPACE_1))
                .h(px(LINE_HEIGHT_CONTROL * 2.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .h(px(11.))
                        .w(relative(0.92))
                        .rounded(px(RADIUS_SM))
                        .bg(brush),
                )
                .child(
                    div()
                        .h(px(11.))
                        .w(relative(0.64))
                        .rounded(px(RADIUS_SM))
                        .bg(brush),
                ),
        )
        .child(
            div()
                .mt(px(2.))
                .h(px(18.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(18.)).h(px(18.)).rounded_full().bg(brush))
                .child(div().h(px(11.)).flex_1().rounded(px(RADIUS_SM)).bg(brush))
                .child(div().h(px(11.)).w(px(32.)).rounded(px(RADIUS_SM)).bg(brush)),
        )
}

/// 封面数据层的一个计数：图标 + 数值，白字压在封面下沿。
fn cover_stat(icon: IconName, value: String) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .child(Icon::new(icon).size(px(14.)).text_color(rgb(0xFFFFFF)))
        .child(
            div()
                .text_size(px(FONT_SIZE_DATA_OVERLAY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xFFFFFF))
                .child(value),
        )
}

/// 直播角标：粉色实心胶囊 + 白字 + 18 高。
fn live_badge(t: &YkTheme) -> impl IntoElement {
    let p = &t.palette;
    div()
        .absolute()
        .top(px(SPACE_2))
        .left(px(SPACE_2))
        .h(px(18.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(3.))
        .rounded_full()
        .bg(p.bili_pink)
        .text_size(px(FONT_SIZE_MICRO))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0xFFFFFF))
        .child(
            Icon::new(IconName::Radio)
                .size(px(12.))
                .text_color(rgb(0xFFFFFF)),
        )
        .child("直播中")
}

/// 封面右上角的悬停操作（稍后再看 / 下载 / 更多）。
///
/// 三个动作的**后端**还没接；这里先把点击从整卡手势里截走，
/// 避免点「下载」却打开播放浮窗。接入真实动作时只改这个函数。
fn hover_action(
    t: &YkTheme,
    idx: usize,
    kind: &'static str,
    icon: IconName,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let primary = t.palette.theme_primary;
    div()
        .id((kind, idx))
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_MD))
        .bg(rgba(0x0000008C))
        .cursor_pointer()
        .hover(move |style| style.bg(primary))
        .child(
            Icon::new(icon)
                .size(px(ICON_SIZE_SM))
                .text_color(rgb(0xFFFFFF)),
        )
        .on_click(cx.listener(move |_this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            eprintln!("[card] 悬停操作 {kind} · 卡片 #{idx}（后端动作未接）");
        }))
}

/// 署名行：UP 头像 18 + 昵称 + 相对时间。
fn byline(
    t: &YkTheme,
    item: &CoverItem,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let face_url = if load_image {
        item.up_face.clone()
    } else {
        String::new()
    };
    if !face_url.is_empty() {
        active_images.borrow_mut().mark(&face_url);
    }
    let name = if item.up_name.is_empty() {
        "未知 UP 主".to_string()
    } else {
        item.up_name.clone()
    };
    let published = item.published_text();

    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(18.))
        .mt(px(2.))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_tertiary)
        .child(
            // 头像兜底：蓝→粉渐变圆；有 up_face 时真图盖在上面。
            div()
                .relative()
                .w(px(18.))
                .h(px(18.))
                .rounded_full()
                .overflow_hidden()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .when(!face_url.is_empty(), |el| {
                    el.child(
                        img(ImageSource::from(face_url.clone()))
                            .w(px(18.))
                            .h(px(18.))
                            .object_fit(ObjectFit::Cover),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(s.text_tertiary)
                .child(name),
        )
        .when(!published.is_empty(), |el| {
            el.child(
                div()
                    .flex_shrink_0()
                    .text_color(s.text_quaternary)
                    .child("·"),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(s.text_quaternary)
                    .child(published),
            )
        })
}

/// 自绘封面兜底渐变：主题令牌只有单色，设计稿的 135° 双色卡面
/// 用占位色派生「暗 → 中」两档自己画。
pub(crate) fn placeholder_gradient(hex: &str, fallback: Hsla) -> Background {
    let base = Hsla::from(parse_placeholder(hex));
    let base = if base.a < 0.05 { fallback } else { base };
    let start = Hsla {
        h: base.h,
        s: base.s,
        l: (base.l * 0.35).clamp(0.08, 0.32),
        a: 1.0,
    };
    let end = Hsla {
        h: base.h,
        s: base.s,
        l: (base.l * 0.30 + 0.24).clamp(0.16, 0.52),
        a: 1.0,
    };
    linear_gradient(
        135.,
        linear_color_stop(start, 0.),
        linear_color_stop(end, 1.),
    )
}

// ================= Dock（页面场景内浮层） =================

/// 底部 Dock 的几何契约：Dock 浮在页面上，内容区底部必须留出
/// `DOCK_RESERVED`，否则最后一行封面会被压住。
const DOCK_BOTTOM_MARGIN: f32 = 20.0;
const DOCK_ITEM_HEIGHT: f32 = 40.0;
/// Dock 本体高 52（40 + 6×2）+ 下边距 20 + 呼吸 24。
const DOCK_RESERVED: f32 = 96.0;

/// 播放器入场动画结束后，再延迟一点停绘背后页面。
/// 入场动画 280ms，留 40ms 余量，避免最后一帧还在展开时页面先消失。
const PLAYER_PAGE_HIDE_DELAY_MS: u64 = 320;

/// 首页网格列数：可用宽度能放下几个「卡片 + 间距」槽位，至少 1 列。
///
/// 网格绘制（[`content_area`]）和「一屏需要多少张封面」的判断共用这一份几何，
/// 避免两处公式漂移。
fn home_columns(viewport_width: f32) -> usize {
    let available = (viewport_width - SPACE_6 * 2.0).max(COVER_CELL_WIDTH);
    (((available + SPACE_4) / (COVER_CELL_WIDTH + SPACE_4)).floor() as usize).max(1)
}

/// 首页填满一屏所需的最少卡片数：列数 ×（整行数 + 1 行）。
///
/// 推荐接口一页固定 30 条；窗口大到一页填不满时，内容高度不足 → 滚动条
/// 的 `max_offset` 为 0 → 「触底加载」永远不触发，表现就是没有滚动条、
/// 也拉不到更多封面。这里按视口几何算出「至少几行才超出一屏」，不够就补页。
///
/// 网格容器上内边距 16 + 下内边距 `DOCK_RESERVED`，滚动视口高 = 窗口高 − 顶栏；
/// 所以内容高 `rows × row_h + 112 > 视口高` 等价于 `rows × row_h > available_h`，
/// 即整行数 + 1 行就够了（刚好多出不到一行，保证一定滚得动）。
fn home_min_items(viewport_width: f32, viewport_height: f32) -> usize {
    let cols = home_columns(viewport_width);
    let row_h = COVER_CELL_HEIGHT + SPACE_4;
    let available_h = (viewport_height - TOP_BAR_HEIGHT - SPACE_4 - DOCK_RESERVED).max(row_h);
    let full_rows = (available_h / row_h).floor() as usize;
    cols * (full_rows + 1)
}

struct DockEntry {
    label: &'static str,
    icon: IconName,
    page: Option<pages::PageKind>,
}

/// 底部横向 Dock：三个一级入口。
///
/// 左侧竖排 Dock（首/热/区/排/直/我）已整体删除，功能收敛到这里：
/// 一级入口不再占用内容区左边距，改为底部浮层，横向浮在所有封面之上。
///
/// 「热门 / 分区 / 直播 / 我的」不在这里处理，已从 Dock 移除；
/// 对应 `PageKind` 仍保留，深链等入口还能打开。
const DOCK_ENTRIES: [DockEntry; 3] = [
    DockEntry {
        label: "个性推荐",
        icon: IconName::Sparkles,
        page: None,
    },
    DockEntry {
        label: "正在关注",
        icon: IconName::Users,
        page: Some(pages::PageKind::Following),
    },
    DockEntry {
        label: "视频排行榜",
        icon: IconName::Trophy,
        page: Some(pages::PageKind::Rank),
    },
];

/// Dock 里的动作项：与导航项同高同字号，图标 + 文字。
///
/// 顶栏原来的四个图标移进来时只有图标，和左侧三个导航项不一致；
/// 这里统一成「16 图标 + 12 文字」，消息保留未读红点。
/// `enabled = false` 用来表示「当前页面没有这个动作可做的事」（如非首页点刷新）：
/// 置灰、不给 hover 和手型，而不是点了没反应。
fn dock_action(
    t: &YkTheme,
    icon: IconName,
    label: &'static str,
    unread: bool,
    enabled: bool,
    active: bool,
) -> Div {
    let s = &t.surfaces;
    let p = &t.palette;
    let fg = if !enabled {
        s.text_quaternary
    } else if active {
        s.text_on_accent
    } else {
        s.text_secondary
    };
    let hover_bg = if active { p.theme_primary } else { s.fill_2 };
    let hover_fg = if active {
        s.text_on_accent
    } else {
        s.text_primary
    };
    let mut action = div()
        .h(px(DOCK_ITEM_HEIGHT))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded_full()
        .bg(if active {
            p.theme_primary
        } else {
            hsla(0., 0., 0., 0.)
        })
        .text_size(px(FONT_SIZE_CAPTION))
        .font_weight(if active {
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
                .child(Icon::new(icon).size(px(16.)).text_color(fg))
                .when(unread, |el| {
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
        .child(label);
    if enabled {
        action = action
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg).text_color(hover_fg));
    }
    action
}

/// 底部 Dock。
///
/// `msg_unread` 是消息中心的总未读，**只在这里画红点**：顶栏已经不放消息铃铛，
/// 未读状态全部由 Dock「消息」按钮承接（数字仍只在消息中心分段上显示）。
fn dock(
    t: &YkTheme,
    page: Option<pages::PageKind>,
    signed_in: bool,
    msg_unread: u64,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
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

    for (i, entry) in DOCK_ENTRIES.iter().enumerate() {
        let active = page == entry.page;
        let fg = if active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let bg = if active {
            p.theme_primary
        } else {
            hsla(0., 0., 0., 0.)
        };
        let hover_bg = if active { p.theme_primary } else { s.fill_2 };
        let target = entry.page;
        row = row.child(
            div()
                .id(("dock", i))
                .h(px(DOCK_ITEM_HEIGHT))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded_full()
                .bg(bg)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .child(Icon::new(entry.icon).size(px(16.)).text_color(fg))
                .child(entry.label)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    // 再点一次当前入口 = 刷新当前页（B 站习惯：点当前 tab 重拉数据）。
                    if this.page == target {
                        if target == Some(pages::PageKind::Following) && !signed_in {
                            // 未登录：带出登录卡片，而不是刷新一个空页。
                            this.ensure_login(cx);
                        } else {
                            this.refresh_page_manually(cx);
                        }
                        window.refresh();
                        cx.notify();
                        return;
                    }

                    // 未登录点「正在关注」：直接把登录卡片带到前面，
                    // 而不是给一个只能干瞪眼的空页。
                    if target == Some(pages::PageKind::Following) {
                        if signed_in {
                            this.ensure_following(cx);
                        } else {
                            this.ensure_login(cx);
                        }
                    }
                    if target == Some(pages::PageKind::Rank) {
                        this.ensure_rank(cx);
                    }
                    this.page = target;
                    this.reset_card_hover();
                    this.top_refresh_armed = false;
                    this.pull_refresh_triggered = false;
                    // 切页回到顶部：滚动位置属于「上一页」，不该带到新页面。
                    this.scroll_content_to_top();
                    window.refresh();
                    cx.notify();
                })),
        );
    }

    // 顶栏原来的四个快捷动作挪进 Dock：导航与动作之间用 1px 分隔线。
    // 刷新只在有数据的页面上可用：「个性推荐」、登录后的「正在关注」与「历史」；
    // 其余页面没有可重拉的数据，按钮置灰，不假装刷新。
    let can_refresh = can_refresh_page(page, signed_in);
    row = row
        .child(
            div()
                .w(px(1.))
                .h(px(20.))
                .mx(px(SPACE_1))
                .bg(s.border_popover),
        )
        // 顺序：历史 / 收藏 / 消息 / 刷新；与导航项一样带文字
        .child(
            dock_action(
                t,
                IconName::Clock,
                "历史",
                false,
                true,
                page == Some(pages::PageKind::History),
            )
            .id("dock-history")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                // 再点一次「历史」= 刷新第一页（与一级入口的习惯一致）。
                if this.page == Some(pages::PageKind::History) {
                    this.refresh_page_manually(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                this.page = Some(pages::PageKind::History);
                this.reset_card_hover();
                this.top_refresh_armed = false;
                this.pull_refresh_triggered = false;
                if this.session.is_none() {
                    this.ensure_login(cx);
                }
                // 从其他页面进入历史：重拉第一页，确保刚看完的视频、
                // 其他设备产生的记录能立刻出现；不是只在首次进入时加载。
                this.load_history(cx);
                this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                window.refresh();
                cx.notify();
            })),
        )
        .child(
            dock_action(
                t,
                IconName::Bookmark,
                "收藏",
                false,
                true,
                page == Some(pages::PageKind::Favorites),
            )
            .id("dock-favorites")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                if this.page == Some(pages::PageKind::Favorites) {
                    this.refresh_page_manually(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                this.page = Some(pages::PageKind::Favorites);
                this.reset_card_hover();
                this.top_refresh_armed = false;
                this.pull_refresh_triggered = false;
                if this.session.is_none() {
                    this.ensure_login(cx);
                } else {
                    // 每次从其他页面进入都重新拉夹子列表，刚在播放页收藏的视频
                    // 才能立刻出现在正确的夹子里。
                    this.load_favorites(cx);
                }
                this.scroll_content_to_top();
                window.refresh();
                cx.notify();
            })),
        )
        .child(
            dock_action(
                t,
                IconName::Bell,
                "消息",
                signed_in && msg_unread > 0,
                true,
                page == Some(pages::PageKind::Messages),
            )
            .id("dock-messages")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                if this.session.is_none() {
                    this.ensure_login(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                if this.page == Some(pages::PageKind::Messages) {
                    this.refresh_messages(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                this.page = Some(pages::PageKind::Messages);
                this.reset_card_hover();
                this.top_refresh_armed = false;
                this.pull_refresh_triggered = false;
                this.messages_land_on_first_unread(cx);
                this.ensure_messages(cx);
                this.scroll_content_to_top();
                window.refresh();
                cx.notify();
            })),
        )
        .child(
            dock_action(t, IconName::RefreshCw, "刷新", false, can_refresh, false)
                .id("dock-refresh")
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    // 刷新是从第一页重拉，滚动位置留在旧内容中间没有意义。
                    if this.refresh_page_manually(cx) {
                        window.refresh();
                        cx.notify();
                    }
                })),
        );

    div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .bottom(px(DOCK_BOTTOM_MARGIN))
        .px(px(SPACE_4))
        .flex()
        .justify_center()
        .child(row)
}

// ================= 工具 =================

/// 把 `#RRGGBB` / `#RRGGBBAA` / `#AARRGGBB` 解析成背景色。
///
/// 占位色来自数据层（`CoverItem::placeholder_color`），是字符串。
/// 这里按设计稿的口径：`#AARRGGBB`（与 XAML 一致）。
fn parse_placeholder(hex: &str) -> Rgba {
    let h = hex.trim_start_matches('#');
    match h.len() {
        6 => {
            let v = u32::from_str_radix(h, 16).unwrap_or(0x555555);
            rgb(v)
        }
        8 => {
            let v = u32::from_str_radix(h, 16).unwrap_or(0xFF555555);
            rgba(v)
        }
        _ => rgb(0x555555),
    }
}

/// 占位数据。真实数据在 M1 的 `CoverItem::from_recommend` 里，
/// 外壳阶段先用与设计稿一致的样例值，避免网络依赖挡住视觉验证。
///
/// 注意：这里刻意产出 **接口原始模型**（`RecommendVideoItem`）而不是 `CoverItem`，
/// 让样例数据也走一遍真实的映射路径 —— 否则 `from_recommend` 丢字段的 bug
/// 在 mock 阶段就发现不了。
fn mock_recommend(n: usize) -> Vec<RecommendVideoItem> {
    const TITLES: [&str; 8] = [
        "生肉烹饪师和他的猫：一顿饭的成本到底有多少",
        "一碗甜子的深夜食堂 · 第 12 期",
        "班长captain 的极限运动合集",
        "用 Rust 从零写一个视频播放器",
        "当我把整个 B 站的热门都下载下来",
        "三分钟看懂 GPUI 的渲染管线",
        "这个夏天最值得去的十个地方",
        "为什么你的代码总是越写越乱",
    ];
    const UPS: [&str; 4] = ["生肉烹饪师", "一碗甜子", "班长captain", "技术宅小明"];
    const VIEWS: [u64; 6] = [124_000, 2_600, 3_000_000, 862, 112_000, 15_190];
    const DANMAKU: [u64; 6] = [144, 0, 1_200, 3, 124, 61];
    const DURATION: [u64; 6] = [754, 1_320, 245, 3_725, 96, 480];
    let now = yk_core::api::client::now_wts();

    (0..n)
        .map(|i| RecommendVideoItem {
            id: i as i64,
            bvid: format!("BV1xx411c7m{i}"),
            title: TITLES[i % TITLES.len()].to_string(),
            pic: String::new(),
            desc: String::new(),
            duration: DURATION[i % 6],
            pubdate: now.saturating_sub((i as u64 % 8) * 3_600 + 1_800),
            goto: "av".into(),
            owner: Some(VideoOwner {
                name: UPS[i % UPS.len()].to_string(),
                face: String::new(),
                ..Default::default()
            }),
            stat: Some(VideoStat {
                view: VIEWS[i % 6],
                danmaku: DANMAKU[i % 6],
                ..Default::default()
            }),
        })
        .collect()
}

#[cfg(test)]
mod quit_tests {
    use super::{AppRoot, QuitDecision};
    use std::time::{Duration, Instant};

    #[test]
    fn quit_needs_a_second_press_inside_the_hint_window() {
        let mut app = AppRoot::new();
        let t0 = Instant::now();

        assert_eq!(app.quit_decision(t0), QuitDecision::Show);
        assert!(app.quit_hint_shown_at.is_some());

        // 按住 ⌘Q 产生的首个重复键：忽略，避免提示刚出现就退出。
        assert_eq!(
            app.quit_decision(t0 + Duration::from_millis(100)),
            QuitDecision::Wait
        );

        // 明确的第二次按键：正式退出。
        assert_eq!(
            app.quit_decision(t0 + Duration::from_millis(500)),
            QuitDecision::Quit
        );
    }

    #[test]
    fn expired_hint_requires_another_first_press() {
        let mut app = AppRoot::new();
        let t0 = Instant::now();
        assert_eq!(app.quit_decision(t0), QuitDecision::Show);

        // 超过 3 秒：提示已过期，这次又按「第一次」处理。
        let later = t0 + Duration::from_millis(3_200);
        assert_eq!(app.quit_decision(later), QuitDecision::Show);
        assert_eq!(
            app.quit_decision(later + Duration::from_millis(400)),
            QuitDecision::Quit
        );
    }
}

#[cfg(test)]
mod scroll_hover_tests {
    use super::AppRoot;

    #[test]
    fn scroll_hover_updates_state_without_restarting_animation() {
        let mut app = AppRoot::new();
        let generation = app.hover_generation;

        // 模拟滚动中滚轮连续划过卡片：状态要跟着走（指针最后停在哪张卡上
        // 不能丢），但不能推进动画代际，否则封面缩放每帧重置。
        assert!(app.update_card_hover(3, true, false));
        assert_eq!(app.hovered_card, Some(3));
        assert_eq!(app.hover_generation, generation);

        assert!(app.update_card_hover(3, false, false));
        assert_eq!(app.hovered_card, None);
        assert_eq!(app.leaving_card, Some(3));
        assert_eq!(app.hover_generation, generation);

        assert!(app.update_card_hover(4, true, false));
        assert_eq!(app.hovered_card, Some(4));
        assert_eq!(app.hover_generation, generation);

        // 滚动结束后用户真的把鼠标移到卡上：这时才换代，播一次进入动画。
        assert!(app.update_card_hover(5, true, true));
        assert_eq!(app.hover_generation, generation + 1);
    }

    #[test]
    fn scroll_settling_windows_cover_native_scroll() {
        let mut app = AppRoot::new();
        assert!(!app.scroll_settling());
        app.mark_scrolling();
        assert!(app.scroll_settling(), "刚滚过：hover 动画应当暂停");
        assert!(app.scroll_render_pending, "滚动停止后需要补一帧恢复视觉");
    }
}

#[cfg(test)]
mod home_grid_tests {
    use super::{home_columns, home_min_items};

    #[test]
    fn columns_follow_window_width() {
        // 1280 可用宽 1232 → 5 列；2560 可用宽 2512 → 11 列。
        assert_eq!(home_columns(1280.0), 5);
        assert_eq!(home_columns(2560.0), 11);
        // 极窄窗口至少保留 1 列。
        assert_eq!(home_columns(200.0), 1);
    }

    #[test]
    fn normal_window_fits_in_one_page() {
        // 一页 30 条：普通窗口够用，不该无谓地多请求。
        assert!(home_min_items(1280.0, 820.0) <= 30);
        assert!(home_min_items(1440.0, 900.0) <= 30);
    }

    #[test]
    fn large_window_needs_more_than_one_page() {
        // 大窗口下一页 30 条填不满一屏，必须继续补页，否则 max_offset 恒为 0。
        assert!(home_min_items(2560.0, 1440.0) > 30);
        assert!(home_min_items(1920.0, 1200.0) > 30);
        // 补页目标不能超过 Feed 的容量上限。
        assert!(home_min_items(2560.0, 1440.0) <= yk_core::feed::MAX_ITEMS);
    }

    #[test]
    fn min_items_grows_with_viewport() {
        let base = home_min_items(1920.0, 1080.0);
        assert!(home_min_items(1920.0, 1440.0) > base, "更高 → 需要更多行");
        assert!(
            home_min_items(2560.0, 1080.0) > base,
            "更宽 → 每行更多，同样高度需要更多条"
        );
    }
}
