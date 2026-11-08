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
use gpui::component::VirtualListScrollHandle;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};
use yk_infra::api::models::RecommendVideoItem;
use yk_infra::dynamic::{DynamicItem, DynamicKind};
use yk_domain::models::CoverItem;
use yk_infra::Feed;

pub use yk_library::card::{animate_cover_zoom, card, CardHost};
pub use yk_library::cover::{cover_art, cover_visual, CoverVisual};
pub use yk_library::image::ImageResidency;
pub use yk_library::favorites as favorites_view;
pub use yk_library::history as history_view;
pub use yk_components::gallery as components;
pub use yk_infra::http;
#[cfg(target_os = "windows")]
pub use yk_infra::win_clipboard;
pub use yk_library::pages;
pub use yk_library::{image_viewer, settings_view};
pub use yk_library::page_image_cache;
pub use yk_library::rank as rank_view;
pub use yk_library::up_profile;
pub use yk_shell::{account_view, login_view};
pub use yk_player as player;
use crate::dynamic_view::{CardOrigin, DynamicDetailView};
use crate::player::CoverSource;
use yk_shell::dock::{DockItem, DockSelect};
use yk_library::messages::MessagesHost;
pub use yk_library::history::HistoryHost;
use yk_domain::models::UserSession;
pub use yk_library::favorites::{FavoritesHost, FavoritesState};
pub use yk_library::rank::{RankHost, RankState};
pub use yk_library::search::SearchHost;
pub use yk_shell::route::Route;
pub use yk_player::{comment_section, comment_view, video, video_worker};

pub mod dynamic_view;
mod mock;
mod history_actions;
use page_image_cache::PageImageCache;
use yk_theme::theme::*;

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

/// 顶栏搜索浮层的开 / 合代际：每次切换都换一个动画 id，GPUI 才会重新播。
static SEARCH_ANIM_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 应用根视图。
pub struct AppRoot {
    theme: YkTheme,
    /// 根节点自己的焦点句柄。
    ///
    /// ⌘Q 挂在根节点的 capture 阶段；没有焦点时 GPUI 只把按键派发到
    /// `base::Root` 这一层，AppRoot 的监听器根本不在路径上。启动时先让根
    /// 节点拿到焦点，键盘事件才有稳定的落点（打开播放器后能退出，就是
    /// 因为播放器刚好把焦点放进了 AppRoot 子树）。
    root_focus: Option<FocusHandle>,
    /// 是否已经排了一次「把焦点交回根节点」的延迟任务。
    root_focus_pending: bool,
    feed: Feed,
    /// Some = 正在播放（整场景压在页面上）
    playing: Option<gpui::Entity<player::PlayerView>>,
    /// M4 组件验收页
    gallery: Option<gpui::Entity<components::ComponentGallery>>,
    /// 当前页面；None = 首页（卡片网格）
    page: Option<Route>,
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
    session: Option<yk_domain::models::UserSession>,
    /// 登录卡片；None = 未打开。
    login: Option<Entity<login_view::LoginView>>,
    /// 已登录账号面板；None = 未打开。
    account: Option<Entity<account_view::AccountView>>,
    /// UP 主浮窗（他人空间）；None = 未打开。点 UP 主头像打开，Esc 或点遮罩回收。
    up_profile: Option<Entity<up_profile::UpProfileView>>,
    /// 播放器是否压在 UP 主浮窗之上。
    ///
    /// 两种情况天然矛盾，只能按来源决定谁在上：
    /// - 从播放器面板点 UP 主头像进来 → 浮窗要在播放器之上（否则被盖住）；
    /// - 在浮窗里点封面起播 → 播放器要在浮窗之上（否则「在动态层上面开播放器」就是空话）。
    player_on_top: bool,
    /// 设置浮层（缓存管理）；None = 未打开。
    settings: Option<Entity<settings_view::SettingsView>>,
    /// 页面级图片缓存：只绑定在内容区，顶栏/浮层不受影响。
    page_image_cache: Option<Entity<PageImageCache>>,
    /// 根节点远程图缓存：顶栏头像、播放器面板、账号 / 动态详情 / 图片查看器
    /// 都在内容区之外，必须有一个 ImageCache 才会加载。
    ///
    /// 它只增不减（GPUI 的 `RetainAllImageCache` 语义），所以播放器关闭时
    /// 整体清空一次 —— 否则每看一个视频留下的评论头像会一直堆到窗口销毁。
    remote_image_cache: Option<Entity<RetainAllImageCache>>,
    /// 本帧实际加载的卡片图 URL。
    active_images: Rc<RefCell<ImageResidency>>,
    /// 上一次 prune 时的页面。页面切换后的第一帧不能拿上一页的
    /// `active_images` 当 keep，否则旧页面的封面会被一直钉在缓存和图集里。
    page_images_route: Option<Route>,
    page_images_route_known: bool,
    /// 滚动监测：滚轮不会触发 AppRoot render，需要轮询偏移量后 notify。
    scroll_watchdog_started: bool,
    /// 窗口可见性状态机（失焦与不可见要分开，见 `player::VisibilityGate`）。
    ///
    /// 不可见时：页面动画、平滑滚动、滚动补帧这些「没人看」的活全部停掉，
    /// 只保留播放器的音频与进度。
    visibility: player::VisibilityGate,
    /// 窗口可见性观察者是否已注册。
    visibility_observed: bool,
    /// 窗口可见性订阅句柄。
    visibility_observer: Option<gpui::Subscription>,
    /// 图片查看器（点击关注页图片打开；支持滚轮缩放 / 拖动 / 多图切换）。
    /// 首次打开时才创建，避免没点过图片也挂一个空视图。
    image_viewer: Option<gpui::Entity<crate::image_viewer::ImageViewer>>,
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
    search: yk_library::search::SearchState,
    /// 搜索浮层正在播「收回」动画：此时 `search.open` 已经是 false，
    /// 但浮层还要留在树上把动画播完。
    search_closing: bool,
    /// 展开动画已经落地。落地后才显示联想面板，避免面板先于搜索框冒出来。
    search_bar_expanded: bool,
    /// 展开动画代际（`with_animation` 的 id，变了才重播）。
    search_open_generation: u64,
    /// 收回动画代际。
    search_close_generation: u64,
    /// 消息中心：四通道列表、私信会话与未读角标。
    messages: yk_library::messages::MessageState,
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
    pub fn with_page(mut self, page: Route) -> Self {
        self.page = Some(page);
        self
    }

    /// 验收入口：启动即落到某个关键词的结果页（`YK_SEARCH=鬼灭`）。
    pub fn with_search(mut self, query: &str) -> Self {
        let query = query.trim();
        if !query.is_empty() {
            self.page = Some(Route::Search);
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
    /// 测试入口：直接打开某个 UP 主的浮窗（不走网络，动态由
    /// [`up_profile::UpProfileView::with_dynamics`] 注入）。
    #[doc(hidden)]
    pub fn open_up_profile_for_test(
        &mut self,
        mid: i64,
        name: String,
        face: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opener = gpui::Bounds {
            origin: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            size: gpui::size(gpui::px(200.0), gpui::px(120.0)),
        };
        self.ensure_up_profile(mid, name, face, opener, window, cx);
    }

    /// 测试入口：往已打开的浮窗里灌动态。
    #[doc(hidden)]
    pub fn inject_up_profile_dynamics_for_test(
        &mut self,
        items: Vec<yk_infra::space::SpaceDynamic>,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.up_profile.clone() else {
            return;
        };
        view.update(cx, |view, cx| {
            view.set_dynamics(items);
            cx.notify();
        });
    }

    pub fn without_login_prompt(mut self) -> Self {
        self.login_prompted = true;
        self
    }

    /// 测试 / 截图入口：装一个假的已登录会话（不发请求，只让页面走登录态分支）。
    pub fn with_signed_in(mut self) -> Self {
        let mut session = yk_domain::models::UserSession::default();
        session.dede_user_id = 1;
        session.sess_data = "test-session".into();
        session.bili_jct = "test-csrf".into();
        session.nickname = "测试账号".into();
        session.is_logged_in = true;
        self.session = Some(session);
        self
    }

    /// 测试 / 截图入口：注入关注流数据并落到「正在关注」页，不发请求。
    pub fn with_following(mut self, items: Vec<DynamicItem>) -> Self {
        self.page = Some(Route::Following);
        self.following = items;
        self.following_loaded = true;
        self.following_loading = false;
        self.following_loading_more = false;
        self.following_has_more = false;
        self
    }

    /// 测试入口：动态详情浮层是否已打开。
    #[doc(hidden)]
    pub fn test_dynamic_detail_open(&self) -> bool {
        self.dynamic_detail.is_some()
    }

    /// 测试入口：图片查看器是否已打开。
    #[doc(hidden)]
    pub fn test_image_viewer_open(&self, cx: &App) -> bool {
        self.image_viewer
            .as_ref()
            .is_some_and(|viewer| viewer.read(cx).is_open())
    }

    /// 测试 / 截图入口：固定一张封面处于 hover 态，用于检查卡片材质。
    pub fn with_hovered_card(mut self, index: usize) -> Self {
        self.hovered_card = Some(index);
        self
    }

    /// 测试 / 截图入口：注入历史页数据并进入批量模式，检查选中卡片的材质。
    pub fn with_history_entries(mut self, entries: Vec<yk_infra::history::HistoryEntry>) -> Self {
        self.page = Some(Route::History);
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
        results: yk_infra::search::SearchResults,
        total: u64,
    ) -> Self {
        self.page = Some(Route::Search);
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
        items: Vec<yk_infra::search::SuggestItem>,
        hotwords: Vec<yk_infra::search::Hotword>,
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
        channel: yk_infra::messages::MessageChannel,
        entries: Vec<yk_infra::messages::MessageEntry>,
        unread: yk_infra::messages::UnreadCounts,
    ) -> Self {
        self.page = Some(Route::Messages);
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

    /// 测试 / 验收：顶栏搜索浮层当前是否展开。
    pub fn search_open_for_test(&self) -> bool {
        self.search.open
    }

    /// 测试 / 验收：搜索浮层是否正在播收回动画。
    pub fn search_closing_for_test(&self) -> bool {
        self.search_closing
    }

    /// 展开顶栏搜索浮层：收起态胶囊 → 浮在最上层的完整搜索框。
    ///
    /// 代际 +1 换动画 id；`search_bar_expanded` 等动画落地后才置 true，
    /// 联想面板不会先于搜索框冒出来。
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.open = true;
        self.search_closing = false;
        self.search_bar_expanded = false;
        self.search.selected = None;
        self.search_open_generation =
            SEARCH_ANIM_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.search_ensure_hotwords(cx);
        if let Some(input) = self.search.input.clone() {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        let generation = self.search_open_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(
                    yk_library::search_ui::SEARCH_GROW_MS + 20,
                ))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.search.open
                    && !view.search_closing
                    && view.search_open_generation == generation
                {
                    view.search_bar_expanded = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 收起顶栏搜索浮层：先播收回动画，动画结束再卸掉浮层。
    ///
    /// 只管「本来展开着」的情况；提交搜索 / 打开播放器等路径直接置
    /// `open = false`，浮层下一帧消失，不需要动画。
    fn begin_search_close(&mut self, cx: &mut Context<Self>) {
        if !self.search.open || self.search_closing {
            return;
        }
        self.search.open = false;
        self.search.selected = None;
        self.search_bar_expanded = false;
        self.search_closing = true;
        self.search_close_generation =
            SEARCH_ANIM_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.search_close_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(
                    yk_library::search_ui::SEARCH_CLOSE_MS + 20,
                ))
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                if view.search_closing && view.search_close_generation == generation {
                    view.search_closing = false;
                    // 浮层里的输入框随动画卸载：焦点不能悬在不在树上的元素上，
                    // 否则 ⌘Q 的 capture 监听不在派发路径上，退出提示会失灵。
                    let handle = match view.root_focus.clone() {
                        Some(handle) => handle,
                        None => {
                            let handle = cx.focus_handle();
                            view.root_focus = Some(handle.clone());
                            handle
                        }
                    };
                    window.focus(&handle, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 搜索浮层：一份透明遮罩 + 展开 / 收回中的搜索框 + 联想面板。
    ///
    /// 挂在根节点 children 的**最后**，永远压在播放器 / 账号面板等所有
    /// 浮层之上；遮罩负责「点外面收起」，避免这一击同时落到下面的卡片上。
    fn search_overlay(
        &self,
        t: &YkTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport_width = window.viewport_size().width.as_f32();
        let source = yk_library::search_ui::search_collapsed_bounds(viewport_width);
        let target = yk_library::search_ui::search_expanded_bounds(viewport_width);
        // 收回时由浮层自己画渐隐的聚焦环（见 SearchPopover），
        // 这里把搜索框自带的环关掉，避免结束时硬消失。
        let bar = yk_library::search_ui::search_bar(
            t,
            &self.search,
            window,
            cx,
            !self.search_closing,
        );
        // 验收注入（YK_SEARCH_SUGGEST / with_search_suggestions）在第一帧就是
        // 展开态：代际为 0，直接按终点画，不播动画。
        let static_open =
            self.search.open && !self.search_closing && self.search_open_generation == 0;
        let popover = yk_library::search_ui::SearchPopover {
            theme: t.clone(),
            bar,
            source,
            target,
            progress: if static_open { 1.0 } else { 0.0 },
            closing: self.search_closing,
        };
        let popover = if static_open {
            popover.into_any_element()
        } else if self.search_closing {
            popover
                .with_animation(
                    format!("search-close-{}", self.search_close_generation),
                    Animation::new(std::time::Duration::from_millis(
                        yk_library::search_ui::SEARCH_CLOSE_MS,
                    ))
                    .with_easing(|t| t),
                    // 收回从 1 → 0：先快后慢，最后几帧贴回胶囊。
                    |el, p| yk_library::search_ui::SearchPopover {
                        progress: 1.0 - ease_entrance()(p),
                        ..el
                    },
                )
                .into_any_element()
        } else {
            popover
                .with_animation(
                    format!("search-open-{}", self.search_open_generation),
                    Animation::new(std::time::Duration::from_millis(
                        yk_library::search_ui::SEARCH_OPEN_MS,
                    ))
                    .with_easing(|t| t),
                    // 长到目标 + 150ms 缓动落稳（曲线自带过冲，见 search_ui）。
                    |el, p| yk_library::search_ui::SearchPopover {
                        progress: yk_library::search_ui::search_open_progress(p),
                        ..el
                    },
                )
                .into_any_element()
        };
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("search-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x00000001))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                            view.begin_search_close(cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(popover)
            // 联想面板必须和浮层一起放在遮罩之上，否则会被遮罩挡住点击。
            .children(
                (self.search.open
                    && (self.search_bar_expanded || self.search_open_generation == 0))
                    .then(|| {
                        yk_library::search_ui::suggest_overlay(t, &self.search, window, cx)
                    })
                    .flatten(),
            )
            .into_any_element()
    }

    /// 测试入口：把焦点放到顶栏搜索框，让窗口有真实的键盘焦点。
    ///
    /// 搜索框现在只在展开态存在，这里顺便把浮层置成「静态展开」
    /// （代际 0，不播动画）。调用方随后要画一帧，键事件才能沿着
    /// 输入框所在的元素树找到根节点。
    pub fn focus_search_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_search_input(window, cx);
        self.search.open = true;
        self.search_closing = false;
        self.search_bar_expanded = true;
        if let Some(input) = self.search.input.clone() {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 测试 / 截图入口：直接注入一张榜单。
    pub fn with_rank_page(mut self, rid: u32, page: yk_infra::ranking::RankPage) -> Self {
        self.page = Some(Route::Rank);
        self.rank.zone = yk_infra::ranking::ZONES
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
        folders: Vec<yk_infra::interaction::FavFolder>,
        items: Vec<yk_infra::favorites::FavVideo>,
        total: u64,
    ) -> Self {
        self.page = Some(Route::Favorites);
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
        conversations: Vec<yk_infra::messages::Conversation>,
        unread: yk_infra::messages::UnreadCounts,
    ) -> Self {
        self.page = Some(Route::Messages);
        self.messages.channel = yk_infra::messages::MessageChannel::Dm;
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
        conversation: yk_infra::messages::Conversation,
        messages: Vec<yk_infra::messages::ChatMessage>,
    ) -> Self {
        self.page = Some(Route::Messages);
        self.messages.channel = yk_infra::messages::MessageChannel::Dm;
        self.messages.dm.conversations = vec![conversation.clone()];
        self.messages.dm.loaded = true;
        self.messages.apply_whitelist();
        let mut chat = yk_library::messages::ChatState::new(&conversation);
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
            Some(Route::Following) => self.ensure_following(cx),
            Some(Route::History) => self.ensure_history(cx),
            Some(Route::Favorites) => self.ensure_favorites(cx),
            Some(Route::Rank) => self.ensure_rank(cx),
            Some(Route::Messages) => self.ensure_messages(cx),
            Some(Route::Search) => {
                if !self.search.query.trim().is_empty() {
                    self.load_search_page(false, cx);
                }
            }
            _ => {}
        }
    }

    /// 启动时恢复登录会话；无效会话被忽略。
    pub fn with_session(mut self, session: Option<yk_domain::models::UserSession>) -> Self {
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
        self.install_player_up_profile_handler(cx);
        self.install_player_image_handler(cx);
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

    fn cached_account_info(&self) -> Option<yk_infra::auth::AccountInfo> {
        let session = self.session.as_ref()?;
        if session.account_updated_at == 0 {
            return None;
        }
        Some(yk_infra::auth::AccountInfo {
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

    fn store_account(&mut self, info: yk_infra::auth::AccountInfo, cx: &mut Context<Self>) {
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
        session.account_updated_at = yk_infra::api::client::now_wts();
        let session = session.clone();
        if let Err(e) = yk_infra::session::save(&yk_infra::session::session_path(), &session) {
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

    /// 从任意一处封面打开播放浮窗：封面几何 → 播放器入场动画。
    ///
    /// 首页卡片、UP 主浮窗的投稿网格、视频类动态都走这一个入口，
    /// 保证「点了封面就长成播放器」只有一份逻辑。
    pub(crate) fn play_from_cover(
        &mut self,
        mut source: player::CoverSource,
        bvid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if bvid.is_empty() {
            eprintln!("[play] 跳过：封面没有 bvid");
            return;
        }
        eprintln!(
            "[play] 从封面起播：{bvid}（{}）",
            if self.up_profile.is_some() {
                "压在浮窗之上"
            } else {
                "独立打开"
            }
        );
        // 从浮窗里起播：播放器要压在 UP 主浮窗之上（「在动态层上面再开一个播放器」），
        // 浮窗保持打开，关掉播放器还能回到 TA 的主页。
        self.player_on_top = self.up_profile.is_some();
        if let Some(root) = self.layout_bounds.get() {
            source.root_size = root.size;
        }

        // **不能重建 PlayerView**：它带单实例断言（构造 +1 / Drop −1），
        // 而旧实例在这一帧的元素树里还挂着引用，`self.playing = None` 只是减一次
        // 引用计数、并不会立刻 Drop —— 直接 `PlayerView::new` 会撞断言，
        // panic 穿过 ObjC 事件回调就是 SIGABRT（2026-11-02 那次崩溃）。
        // 正确做法是复用同一个实例：`set_entrance` 会递增 entrance_generation，
        // 入场动画因此重播；`open_bvid` 负责换源（内部会先补上一条的观看上报）。
        if self.playing.is_none() {
            self.ensure_player(cx);
        } else {
            self.close_player_requested = false;
            self.player_page_hidden = false;
        }
        if let Some(player) = self.playing.clone() {
            let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
            let _ = player.update(cx, |view, cx| view.open_bvid(bvid, cx));
        }
        window.refresh();
        cx.notify();
    }

    /// 点 UP 主头像：打开 UP 主浮窗（他人空间，12.9 节）。
    ///
    /// 同一个人已经开着就复用，避免重复请求；换人则重建实体。
    /// 入场起点先用「顶栏下方 34 圆点」近似——头像的精确 bounds 要 canvas 探测，
    /// 动画观感一致，锚点可以后面再精修。
    pub fn ensure_up_profile(
        &mut self,
        mid: i64,
        name: String,
        face: String,
        opener: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mid <= 0 {
            return;
        }
        if let Some(existing) = self.up_profile.as_ref() {
            if existing.read(cx).mid() == mid {
                return;
            }
            self.up_profile = None;
        }
        self.search.open = false;
        // 从播放器面板 / 卡片署名进来：浮窗压在播放器之上（否则会被视频窗口盖住）。
        self.player_on_top = false;
        let theme = self.theme.clone();
        let viewport = window.viewport_size();
        let view = cx.new(|cx| {
            up_profile::UpProfileView::new(theme, mid, name, face, opener, viewport, cx)
        });
        let root = cx.entity().downgrade();
        let root_for_play = cx.entity().downgrade();
        let root_for_image = cx.entity().downgrade();
        let _ = view.update(cx, |view, _| {
            view.set_close_handler(move |cx| {
                let _ = root.update(cx, |root, cx| {
                    root.up_profile = None;
                    cx.notify();
                });
            });
            view.set_play_handler(move |cx, window, bvid, source| {
                let _ = root_for_play.update(cx, |root, cx| {
                    root.play_from_cover(source, bvid, window, cx);
                });
            });
            view.set_image_handler(move |cx, window, urls, index| {
                let _ = root_for_image.update(cx, |root, cx| {
                    root.open_image_viewer(urls, index, window, cx);
                });
            });
        });
        // 打开浮窗前清一次全局选区状态。
        //
        // 窗口级选区有个坑：只要某次按下之后「松开」被别处吞掉（遮挡层、
        // 关闭按钮、`on_click` 里的 stop_propagation 都可能），
        // `WindowSelectionState::is_selecting` 就会永久停在 true，
        // 之后所有按下都被当成「已经在选」而跳过，正文一次也选不中。
        // 打开浮窗属于全新一轮交互，这里直接清干净。
        gpui::base::TextSelection::clear(window, cx);
        self.up_profile = Some(view);
        cx.notify();
    }

    /// 给任意头像挂上「点击打开 UP 主浮窗」：canvas 探针记下元素的屏幕 bounds，
    /// 点击时把 mid / 昵称 / 头像 / bounds 一起交给本实体。
    ///
    /// 探针必须是这一帧新建的 `Rc`：元素树每帧重建，点击处理器捕获的就是
    /// 同一帧 prepaint 写进来的那组 bounds，所以不会读到上一帧的旧坐标。
    pub(crate) fn up_profile_avatar(
        id: SharedString,
        mid: i64,
        name: String,
        face: String,
        size_px: f32,
        child: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let sink = bounds.clone();
        div()
            .id(id)
            .relative()
            .w(px(size_px))
            .h(px(size_px))
            .flex_shrink_0()
            .cursor_pointer()
            .hover(|style| style.opacity(0.86))
            .child(
                canvas(
                    move |element_bounds, _window, _cx| sink.set(Some(element_bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(child)
            .on_click(cx.listener(move |root, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                if mid <= 0 {
                    return;
                }
                let opener = bounds.get().unwrap_or_else(|| Bounds {
                    origin: point(px(0.0), px(TOP_BAR_HEIGHT)),
                    size: size(px(size_px), px(size_px)),
                });
                root.ensure_up_profile(mid, name.clone(), face.clone(), opener, window, cx);
            }))
            .into_any_element()
    }

    /// 把「打开图片预览」的回调注入 PlayerView。
    ///
    /// 全站只有 `AppRoot::open_image_viewer` 一份预览实现；播放器以前自己
    /// `cx.new` 了一个 ImageViewer，同一张图在两处打开是两套缩放/切图状态。
    pub fn install_player_image_handler(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.playing.clone() else {
            return;
        };
        let app = cx.entity().downgrade();
        let _ = player.update(cx, |view, _| {
            view.set_image_handler(move |cx, window, urls, index| {
                let _ = app.update(cx, |root, cx| {
                    root.open_image_viewer(urls, index, window, cx);
                });
            });
        });
    }

    /// 把「打开 UP 主浮窗」的回调注入 PlayerView。
    pub fn install_player_up_profile_handler(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.playing.clone() else {
            return;
        };
        let app = cx.entity().downgrade();
        let _ = player.update(cx, |view, _| {
            view.set_up_profile_handler(move |cx, window, mid, name, face, opener| {
                let _ = app.update(cx, |root, cx| {
                    root.ensure_up_profile(mid, name, face, opener, window, cx);
                });
            });
        });
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
        if let Err(e) = yk_infra::session::clear(&yk_infra::session::session_path()) {
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
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_recommend(
                        &client,
                        "",
                        30,
                        yk_infra::api::client::now_wts(),
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
                        if let Err(e) = yk_infra::feed_cache::FeedCache::new().save(&items) {
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
    fn apply_login(&mut self, session: yk_domain::models::UserSession, cx: &mut Context<Self>) {
        if !session.is_valid() {
            return;
        }
        let path = yk_infra::session::session_path();
        if let Err(e) = yk_infra::session::save(&path, &session) {
            eprintln!("[login] session.json 写入失败：{e}");
        }
        self.session = Some(session);
        self.login = None;
        self.login_prompted = true;
        // 换账号后旧的关注流 / 消息中心一定不再适用。
        self.reset_following();
        self.messages.reset_for_account();
        self.messages_refresh_unread(cx);
        self.refresh_feed_authenticated(cx);
        if self.page == Some(Route::Following) {
            self.load_following_page(false, false, cx);
        }
        if self.page == Some(Route::History) {
            self.load_history(cx);
        }
        if self.page == Some(Route::Favorites) {
            // 换账号后旧收藏夹不再适用：先清空，再按新 Cookie 重拉。
            self.favorites.reset();
            self.load_favorites(cx);
        }
        if self.page == Some(Route::Messages) {
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
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_recommend(
                        &client,
                        &cookie,
                        30,
                        yk_infra::api::client::now_wts(),
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
                        if let Err(e) = yk_infra::feed_cache::FeedCache::new().save(&items) {
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
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_following_dynamics(&client, &cookie, &offset)
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
                            view.following.insert(0, debug_text_item(0));
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

    /// 注入根节点远程图缓存。由 main 创建，保证整窗共用一个实体；
    /// 播放器关闭时才有地方把它整体清空。
    pub fn set_remote_image_cache(&mut self, cache: Entity<RetainAllImageCache>) {
        self.remote_image_cache = Some(cache);
    }

    /// 清空根节点远程图缓存。
    ///
    /// 评论头像、播放器封面海报、UP 主 / 账号面板图片都挂在这个缓存上；
    /// 播放器关闭后它们不再被任何元素引用，留下来只会让 CPU 解码缓冲和
    /// DirectX 图集纹理随播放次数线性增长。内容区的封面由内层
    /// `PageImageCache` 按视口管理，不受这里影响。
    fn clear_remote_images(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cache) = self.remote_image_cache.clone() else {
            return;
        };
        cache.update(cx, |cache, cx| cache.clear(window, cx));
    }

    /// 丢掉滚出可视区 + 一行缓冲的卡片图。
    ///
    /// 使用上一帧实际挂出的 URL 作为 keep 集合；当前帧渲染时卡片会重新标记。
    /// 播放器/详情浮层打开期间跳过，避免误删它们正在使用的同一张封面。
    fn prune_page_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.playing.is_some() || self.dynamic_detail.is_some() {
            return;
        }
        let Some(cache) = self.page_image_cache.clone() else {
            return;
        };
        // 页面切换后的第一帧：`current` 里还是上一页的驻留集合。拿它当 keep
        // 会把旧页面的封面一直钉在缓存里；而这一帧之后如果没有第二次重绘
        // （停在消息页这类静态页面很常见），就再没有机会淘汰。
        let route_changed = !self.page_images_route_known || self.page_images_route != self.page;
        self.page_images_route = self.page;
        self.page_images_route_known = true;

        let keep = if route_changed {
            self.active_images.borrow_mut().current.clear();
            HashSet::new()
        } else {
            std::mem::take(&mut self.active_images.borrow_mut().current)
        };
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
            self.messages_refresh_unread(cx);
        }
        self.reset_feed_paging();
        let cookie = self.cookie();
        self.feed_generation = self.feed_generation.wrapping_add(1);
        // 顺手预热 WBI keys：所有 wbi 接口（推荐 / 详情 / 取流 / 空间）都要它，
        // 而它是一次 `nav`。以前第一打开播要先付这一趟（Nav 桶 1s 间隔），
        // 表现就是「第一次播放特别慢」。这里在启动时先把缓存填上。
        {
            let cookie = cookie.clone();
            std::thread::spawn(move || {
                if let Ok(client) = yk_infra::api::client::ReqwestClient::new() {
                    if let Err(error) = yk_infra::api::client::fetch_wbi_keys(&client, &cookie) {
                        eprintln!("[wbi] 预热失败：{error}");
                    }
                }
            });
        }
        let generation = self.feed_generation;

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client =
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_recommend(
                        &client,
                        &cookie,
                        30,
                        yk_infra::api::client::now_wts(),
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
                        if let Err(e) = yk_infra::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 推荐流写入失败：{e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("[api] 首屏推荐拉取失败，退回缓存 / 占位数据：{e}");
                        view.offline = true;
                        if view.feed.is_empty() {
                            match yk_infra::feed_cache::FeedCache::new().load() {
                                Some(cached) => {
                                    eprintln!(
                                        "[offline] 使用缓存：{} 条（{}）",
                                        cached.items.len(),
                                        yk_infra::feed_cache::age_text(cached.updated_at)
                                    );
                                    view.feed.load(&cached.items);
                                    view.using_mock = false;
                                    view.offline_updated_at = Some(cached.updated_at);
                                }
                                None => {
                                    eprintln!("[offline] 没有可用缓存，使用占位数据");
                                    view.feed.load(&crate::mock::recommend(24));
                                    view.using_mock = true;
                                    view.offline_updated_at = None;
                                }
                            }
                        } else if let Some(cached) = yk_infra::feed_cache::FeedCache::new().load() {
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
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_recommend(
                        &client,
                        &cookie,
                        30,
                        yk_infra::api::client::now_wts(),
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
                        if let Err(e) = yk_infra::feed_cache::FeedCache::new().save(&items) {
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
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_recommend_page(
                        &client,
                        &cookie,
                        30,
                        yk_infra::api::client::now_wts(),
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
                        yk_infra::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
                    yk_infra::api::client::fetch_recommend_page(
                        &client,
                        &cookie,
                        30,
                        yk_infra::api::client::now_wts(),
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
                        if let Err(e) = yk_infra::feed_cache::FeedCache::new().save(&items) {
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
            Some(Route::Following) => {
                // 未登录没有关注流；登录卡片由入口负责带出，这里不重复弹。
                if self.session.is_none() {
                    return false;
                }
                self.load_following_page(false, true, cx);
                true
            }
            Some(Route::History) => {
                if self.session.is_none() {
                    return false;
                }
                self.load_history(cx);
                true
            }
            Some(Route::Favorites) => {
                if self.session.is_none() {
                    return false;
                }
                self.load_favorites(cx);
                true
            }
            Some(Route::Rank) => {
                self.load_rank(cx);
                true
            }
            Some(Route::Messages) => {
                if self.session.is_none() {
                    return false;
                }
                self.messages_refresh(cx);
                true
            }
            Some(Route::Search) => {
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
            None | Some(Route::Following) => {
                Some((1, self.content_scroll.base_handle().clone()))
            }
            Some(Route::History) => {
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
        // **浮层自己滚**：指针在 UP 主浮窗 / 动态详情 / 图片查看器 / 播放器里时，
        // 这一层不能把滚轮截走（下面会 stop_propagation 并驱动页面网格）——
        // 否则表现为「在浮窗里滚，背后首页在滚，浮窗纹丝不动」。
        if self.up_profile.is_some()
            || self.dynamic_detail.is_some()
            || self.image_viewer.is_some()
            || self.playing.is_some()
        {
            self.cancel_smooth_scroll();
            return;
        }
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

    /// 窗口可见性变化。
    ///
    /// 不可见（最小化 / 被完全遮挡）时把平滑滚动动画停掉：它的每一步都会
    /// `notify()` 触发整窗重绘，而没人看画面。恢复可见后重新滚一次即可。
    ///
    /// **不管失焦**：失焦时窗口还在屏幕上，内容必须照常刷新。
    fn set_window_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !self.visibility.set(visible) {
            return;
        }
        if !visible {
            self.cancel_smooth_scroll();
        } else {
            // 恢复可见：补一帧，把停住期间的画面拉回来。
            cx.notify();
        }
    }

    /// 打开图片查看器。
    ///
    /// `urls` 是这一组图片（按展示顺序），`index` 是点中的那张。
    /// 查看器自己处理缩放 / 平移 / 左右切换 / 还原，宿主只需在元素树末尾挂它。
    pub(crate) fn open_image_viewer(
        &mut self,
        urls: Vec<String>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewer = match self.image_viewer.as_ref() {
            Some(viewer) => viewer.clone(),
            None => {
                let viewer = cx.new(crate::image_viewer::ImageViewer::new);
                self.image_viewer = Some(viewer.clone());
                viewer
            }
        };
        viewer.update(cx, |viewer, cx| viewer.open(urls, index, window, cx));
        cx.notify();
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
                // 窗口不可见时不重绘：offset 照样推进，恢复可见后一次性补上。
                if view.visibility.is_visible() {
                    cx.notify();
                }
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
            Some(Route::Following) => self.load_following_page(true, false, cx),
            Some(Route::Messages) => self.messages_load_more(cx),
            Some(Route::Favorites) => self.load_more_favorites(cx),
            _ => {}
        }
    }

    pub fn new() -> Self {
        eprintln!("[app] AppRoot::new 被调用");
        Self {
            theme: YkTheme::dark(),
            root_focus: None,
            root_focus_pending: false,
            feed: {
                let mut f = Feed::new();
                f.load(&crate::mock::recommend(24));
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
            up_profile: None,
            player_on_top: false,
            settings: None,
            page_image_cache: None,
            remote_image_cache: None,
            active_images: Rc::new(RefCell::new(ImageResidency::default())),
            page_images_route: None,
            page_images_route_known: false,
            scroll_watchdog_started: false,
            visibility: player::VisibilityGate::default(),
            visibility_observed: false,
            visibility_observer: None,
            image_viewer: None,
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
            search: yk_library::search::SearchState::new(),
            search_closing: false,
            search_bar_expanded: false,
            search_open_generation: 0,
            search_close_generation: 0,
            messages: yk_library::messages::MessageState::new(),
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

/// shell 路由 → library 页面内容。只覆盖 `pages` 负责的五个页面；
/// 其余路由在 `content_area` 里由各自的 screen 实现处理。
fn screen_of(route: Route) -> pages::Screen {
    match route {
        Route::Hot => pages::Screen::Hot,
        Route::Category => pages::Screen::Category,
        Route::Live => pages::Screen::Live,
        Route::Mine => pages::Screen::Mine,
        Route::Settings => pages::Screen::Settings,
        _ => pages::Screen::Hot,
    }
}

impl yk_library::dynamic_card::DynamicCardHost for AppRoot {
    fn dynamic_open_images(
        &mut self,
        urls: Vec<String>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_image_viewer(urls, index, window, cx);
    }

    fn dynamic_open_card(
        &mut self,
        item: &DynamicItem,
        origin: yk_library::dynamic_card::CardOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 播放器 / 详情已经开着一层时，卡片点击直接丢弃：动画期间不允许叠新的层。
        if self.playing.is_some() || self.dynamic_detail.is_some() || self.close_player_requested {
            return;
        }
        if item.kind == yk_infra::dynamic::DynamicKind::Video {
            self.open_dynamic_video(item, Some(origin), cx);
        } else {
            self.open_dynamic_detail(item, Some(origin), cx);
        }
        window.refresh();
    }
}

impl yk_library::following::FollowingHost for AppRoot {
    fn following_state(&self) -> yk_library::following::FeedState<'_> {
        yk_library::following::FeedState {
            items: &self.following,
            loading: self.following_loading,
            error: self.following_error.as_deref(),
            loaded: self.following_loaded,
            has_more: self.following_has_more,
            loading_more: self.following_loading_more,
            signed_in: self.session.is_some(),
        }
    }

    fn following_refresh(&mut self, cx: &mut Context<Self>) {
        self.reload_following(cx);
    }

    fn following_load_more(&mut self, cx: &mut Context<Self>) {
        if !self.following_loading_more {
            self.load_following_page(true, false, cx);
        }
    }

    fn following_sign_in(&mut self, cx: &mut Context<Self>) {
        self.ensure_login(cx);
    }
}

impl yk_library::home::HomeHost for AppRoot {
    fn home_state(&self) -> yk_library::home::HomeState<'_> {
        yk_library::home::HomeState {
            items: self.feed.items(),
            loading: self.feed_loading,
            hovered: self.hovered_card,
            leaving: self.leaving_card,
            hover_generation: self.hover_generation,
        }
    }
}

impl CardHost for AppRoot {
    fn card_is_playing(&self) -> bool {
        self.playing.is_some()
    }

    fn card_close_requested(&self) -> bool {
        self.close_player_requested
    }

    fn card_scroll_settling(&self) -> bool {
        self.scroll_settling()
    }

    fn card_update_hover(&mut self, idx: usize, hovered: bool, animate: bool) -> bool {
        self.update_card_hover(idx, hovered, animate)
    }

    fn card_open(
        &mut self,
        bvid: String,
        title: String,
        entrance: Option<player::CoverSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        eprintln!("[M3] 进入播放浮窗: {title}（{bvid}）");
        self.ensure_player(cx);
        if let Some(player) = self.playing.clone() {
            // 先给入场起点，再换播放源；播放器实例本身不重建。
            if let Some(source) = entrance {
                let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
            }
            let _ = player.update(cx, |view, cx| view.open_bvid(bvid, cx));
        }
        window.refresh();
        cx.notify();
    }

    fn card_up_avatar(
        id: SharedString,
        mid: i64,
        name: String,
        face: String,
        size_px: f32,
        child: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        AppRoot::up_profile_avatar(id, mid, name, face, size_px, child, cx)
    }
}

impl AppRoot {
    /// Dock 的统一入口：导航项在前、动作项在后，序号由 shell 保证稳定。
    fn dock_select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let signed_in = self.session.is_some();
        match index {
            0..=2 => {
                let target = DOCK_ENTRIES[index].page;
                if self.page == target {
                    if target == Some(Route::Following) && !signed_in {
                        self.ensure_login(cx);
                    } else {
                        self.refresh_page_manually(cx);
                    }
                    window.refresh();
                    cx.notify();
                    return;
                }
                if target == Some(Route::Following) {
                    if signed_in {
                        self.ensure_following(cx);
                    } else {
                        self.ensure_login(cx);
                    }
                }
                if target == Some(Route::Rank) {
                    self.ensure_rank(cx);
                }
                self.page = target;
                self.reset_card_hover();
                self.top_refresh_armed = false;
                self.pull_refresh_triggered = false;
                self.scroll_content_to_top();
            }
            3 => {
                if self.page == Some(Route::History) {
                    self.refresh_page_manually(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                self.page = Some(Route::History);
                self.reset_card_hover();
                self.top_refresh_armed = false;
                self.pull_refresh_triggered = false;
                if self.session.is_none() {
                    self.ensure_login(cx);
                }
                self.load_history(cx);
                self.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            4 => {
                if self.page == Some(Route::Favorites) {
                    self.refresh_page_manually(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                self.page = Some(Route::Favorites);
                self.reset_card_hover();
                self.top_refresh_armed = false;
                self.pull_refresh_triggered = false;
                if self.session.is_none() {
                    self.ensure_login(cx);
                } else {
                    self.load_favorites(cx);
                }
                self.scroll_content_to_top();
            }
            5 => {
                if self.session.is_none() {
                    self.ensure_login(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                if self.page == Some(Route::Messages) {
                    self.messages_refresh(cx);
                    window.refresh();
                    cx.notify();
                    return;
                }
                self.page = Some(Route::Messages);
                self.reset_card_hover();
                self.top_refresh_armed = false;
                self.pull_refresh_triggered = false;
                self.messages_land_on_first_unread(cx);
                self.ensure_messages(cx);
                self.scroll_content_to_top();
            }
            6 => {
                if self.refresh_page_manually(cx) {
                    window.refresh();
                    cx.notify();
                }
                return;
            }
            _ => return,
        }
        window.refresh();
        cx.notify();
    }
}


impl yk_library::search::SearchHost for AppRoot {
    fn search_state(&self) -> &yk_library::search::SearchState {
        &self.search
    }

    fn search_state_mut(&mut self) -> &mut yk_library::search::SearchState {
        &mut self.search
    }

    fn search_cookie(&self) -> String {
        self.cookie()
    }

    fn search_request_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search(window, cx);
    }

    fn search_request_close(&mut self, cx: &mut Context<Self>) {
        self.begin_search_close(cx);
    }

    fn search_session(&self) -> Option<UserSession> {
        self.session.clone()
    }

    fn search_open_player(&mut self, bvid: String, cx: &mut Context<Self>) {
        self.ensure_player(cx);
        if let Some(player) = self.playing.clone() {
            let _ = player.update(cx, |view, cx| view.open_bvid(bvid, cx));
        }
    }

    fn search_route_to_results(&mut self, cx: &mut Context<Self>) {
        self.page = Some(crate::Route::Search);
        self.page_state = crate::pages::PageState::Ready;
        self.reset_card_hover();
        self.top_refresh_armed = false;
        self.pull_refresh_triggered = false;
        cx.notify();
    }

    fn search_ensure_hotwords(&mut self, cx: &mut Context<Self>) {
        self.ensure_hotwords(cx);
    }

    fn search_schedule_suggest(&mut self, cx: &mut Context<Self>) {
        self.schedule_suggest(cx);
    }

    fn search_reload_hotwords(&mut self, cx: &mut Context<Self>) {
        self.reload_hotwords(cx);
    }

    fn search_submit_from_input(&mut self, cx: &mut Context<Self>) {
        self.submit_search_from_input(cx);
    }

    fn search_submit(&mut self, query: String, cx: &mut Context<Self>) {
        self.submit_search(query, cx);
    }

    fn search_load_more(&mut self, cx: &mut Context<Self>) {
        self.load_search_page(true, cx);
    }

    fn search_reload(&mut self, cx: &mut Context<Self>) {
        self.load_search_page(false, cx);
    }

    fn search_go_category(&mut self, cx: &mut Context<Self>) {
        self.page = Some(crate::Route::Category);
        self.reset_card_hover();
        self.scroll_content_to_top();
        cx.notify();
    }

    fn search_open_video(&mut self, bvid: String, cx: &mut Context<Self>) {
        self.open_search_video(bvid, cx);
    }



    fn search_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_search_notice(text, cx);
    }

    fn search_reset_hover(&mut self) {
        self.reset_card_hover();
    }

    fn search_scroll_top(&mut self) {
        self.scroll_content_to_top();
    }
}


impl yk_library::messages::MessagesHost for AppRoot {
    fn messages(&self) -> &yk_library::messages::MessageState {
        &self.messages
    }

    fn messages_mut(&mut self) -> &mut yk_library::messages::MessageState {
        &mut self.messages
    }







    fn messages_session(&self) -> Option<yk_domain::models::UserSession> {
        self.session.clone()
    }











    fn messages_open_video(&mut self, bvid: String, cx: &mut Context<Self>) {
        self.ensure_player(cx);
        if let Some(player) = self.playing.clone() {
            player.update(cx, |view, cx| view.open_bvid(bvid, cx));
        }
    }


    fn messages_ensure_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_chat_composer(window, cx);
    }

    fn messages_cookie(&self) -> String {
        self.cookie()
    }


    fn messages_poll_allowed(&self) -> bool {
        self.page == Some(crate::Route::Messages)
    }













    fn messages_go_following(&mut self, cx: &mut Context<Self>) {
        self.page = Some(crate::Route::Following);
        self.reset_card_hover();
        self.load_current_page(cx);
        self.scroll_content_to_top();
        cx.notify();
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
            // 播放器实体已经移除：评论头像 / 封面海报等根缓存图不再需要，
            // 趁这一拍把 CPU 缓冲和 GPU 图集一起回收。
            self.clear_remote_images(window, cx);
        }
        if self.dynamic_detail_close_requested {
            self.dynamic_detail_close_requested = false;
            self.dynamic_detail = None;
        }
        self.prune_page_images(window, cx);

        // 根节点焦点：保证 ⌘Q 永远能进到根节点的 capture 监听器。
        // 已有焦点（搜索框、播放器、输入框等）时不抢，只在「没人持有焦点」时兜底。
        let focus_handle = match self.root_focus.as_ref() {
            Some(handle) => handle.clone(),
            None => {
                let handle = cx.focus_handle();
                self.root_focus = Some(handle.clone());
                handle
            }
        };
        if !self.root_focus_pending && window.focused(cx).is_none() {
            self.root_focus_pending = true;
            let handle = focus_handle.clone();
            cx.defer_in(window, move |view, window, cx| {
                view.root_focus_pending = false;
                if window.focused(cx).is_none() {
                    window.focus(&handle, cx);
                }
            });
        }

        // 搜索浮层被直接置为关闭（提交搜索、打开播放器等）时，输入框会随
        // 浮层下树；焦点若还挂在它身上，⌘Q 的 capture 就不在派发路径上。
        // 这类路径不播收回动画，这里把焦点还给根节点兜底。
        if !self.search.open && !self.search_closing {
            if let Some(input) = self.search.input.as_ref() {
                let input_focus = input.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&input_focus) {
                    window.focus(&focus_handle, cx);
                }
            }
        }

        // 窗口可见性：最小化 / 被完全遮挡时停掉页面侧的动画与补帧。
        // 失焦不算 —— 窗口还在屏幕上，内容该照常刷新。
        if !self.visibility_observed {
            self.visibility_observed = true;
            cx.defer_in(window, |view, window, cx| {
                view.visibility_observer =
                    Some(cx.observe_window_visibility(window, |view, visibility, _window, cx| {
                        view.set_window_visible(visibility.is_visible(), cx);
                    }));
            });
        }
        if !self.scroll_watchdog_started {
            self.scroll_watchdog_started = true;
            cx.spawn(async move |this, cx| {
                loop {
                    // 窗口不可见时没必要 60ms 醒一次：那一圈本来就会立刻返回，
                    // 只是白白唤醒 UI 线程。降到 500ms，恢复可见后再切回来。
                    let tick = this
                        .update(cx, |view, _| {
                            if view.visibility.is_visible() {
                                60
                            } else {
                                500
                            }
                        })
                        .unwrap_or(60);
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(tick))
                        .await;
                    let alive = this.update(cx, |view, cx| {
                        // 窗口不可见（最小化 / 被完全遮挡）：这一圈做的全是
                        // 「给人看的」事（补帧、触底加载、回顶刷新），先跳过。
                        // 消息页的自动加载更多保留 —— 那是数据，不是画面。
                        if !view.visibility.is_visible()
                            && view.page != Some(Route::Messages)
                        {
                            return;
                        }
                        // 滚动停下后补一帧：滚动期间卡片 hover 视觉被暂停，
                        // 若没有后续鼠标事件，指针下的卡片会一直停在静态态。
                        // 同时清掉收尾态，避免滚回来的卡片重新挂载时重播 1→0。
                        if view.scroll_render_pending && !view.scroll_settling() {
                            view.scroll_render_pending = false;
                            view.leaving_card = None;
                            cx.notify();
                        }
                        // 消息页用自己的虚拟列表 / 私信列表，滚动偏移不在 content_scroll 上。
                        if view.page == Some(Route::Messages) {
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
                                let need = yk_library::home::min_items(
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
        if self.page == Some(Route::History) {
            self.ensure_history_search_input(window, cx);
        }
        // 顶栏搜索框常驻，同样只能在拿到 Window 之后创建。
        self.ensure_search_input(window, cx);
        self.messages.window_active = window.is_window_active();
        // 消息页：补焦点 / 私信输入框、处理延迟滚动与 Enter 发送。
        if self.page == Some(Route::Messages) {
            self.messages_prepare_frame(window, cx);
        }

        let t = &self.theme;

        // 搜索浮层要挂在根层（压在内容 / Dock 之上），先在这里备好。
        let search_overlay = if self.search.open || self.search_closing {
            Some(self.search_overlay(t, window, cx))
        } else {
            None
        };

        let layout_bounds = self.layout_bounds.clone();
        let weak_root = cx.entity().downgrade();
        // 顶栏 / 播放浮窗 / 账号面板 / 动态详情都在页面图片缓存范围之外；
        // GPUI 的 `img()` 没有 `ImageCache` 就不会发请求（评论头像、顶栏头像
        // 都只会剩渐变占位）。这里给整棵树挂一个缓存：正常由 main 注入的
        // 实体（关闭播放器时可整体清空），测试 / 未注入时退回 retain-all，
        // 内容区仍由内层 `PageImageCache` 按视口淘汰。
        let root = div()
            .relative()
            .size_full()
            .track_focus(&focus_handle)
            // 点空白处收起搜索联想面板；搜索框与面板自己会 stop_propagation。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                    if view.search.open {
                        view.begin_search_close(cx);
                    }
                }),
            )
            .capture_key_down(cx.listener(
                |view, event: &KeyDownEvent, _window, cx| {
                    let keystroke = &event.keystroke;
                    // Esc 只关最上面一层：UP 主浮窗优先于播放器的两次 Esc 退出。
                    if keystroke.key.eq_ignore_ascii_case("escape")
                        && view.up_profile.is_some()
                        && !view.player_on_top
                    {
                        if let Some(up) = view.up_profile.clone() {
                            let _ = up.update(cx, |view, cx| view.begin_close(cx));
                        }
                        cx.stop_propagation();
                        return;
                    }
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
            // ①+② 背景渐变与主题色光晕（shell 根布局）
            

            // 根布局：背景 + 内容列 + Dock，由 shell 统一编排。
            .child(
                yk_shell::frame::ShellFrame {
                    top_bar: (yk_shell::top_bar::top_bar(
                        t,
                        yk_shell::top_bar::toolbar_group(
                            t,
                            avatar(
                                t,
                                self.session.as_ref().map(|s| s.face_url.clone()),
                                self.account.is_some(),
                                cx,
                            )
                            .into_any_element(),
                        ),
                        // 收起态是顶栏里的小胶囊；展开 / 收回过程中由根层浮层
                        // 接管，这里留空，避免中间同时出现两份搜索框。
                        if self.search.open || self.search_closing {
                            div().into_any_element()
                        } else {
                            yk_library::search_ui::search_pill(t, &self.search, window, cx)
                        },
                        window,
                        cx,
                    )).into_any_element(),
                    body: (div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    
                    // 顶栏下方直接是内容区：分类条已删除（与底部 Dock 重复），
                    // 页面切换统一由 Dock 承担。
                    .children(if self.offline {
                        let mut text = match self.offline_updated_at {
                            Some(ts) => format!(
                                "离线 · 缓存时间：{} · {} 条",
                                yk_infra::feed_cache::age_text(ts),
                                self.feed.items().len()
                            ),
                            None => "离线 · 暂无可用缓存，正在展示占位内容".to_string(),
                        };
                        if self.offline_retrying {
                            text = "正在重新连接…".to_string();
                        } else if self.offline_error.is_some() {
                            text.push_str(" · 重试失败");
                        }
                        Some(yk_shell::overlay::offline_banner(
                            t,
                            &text,
                            self.offline_retrying,
                            cx,
                            |this, window, cx| {
                                this.retry_online(cx);
                                window.refresh();
                            },
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
                            yk_library::following::FeedState {
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
                    })).into_any_element(),
                    dock: ({
                let signed_in = self.session.is_some();
                let can_refresh = can_refresh_page(self.page, signed_in);
                let nav: Vec<DockItem> = DOCK_ENTRIES
                    .iter()
                    .map(|entry| {
                        DockItem::nav(entry.label, entry.label, entry.icon, self.page == entry.page)
                    })
                    .collect();
                let actions = vec![
                    DockItem::action(
                        "dock-history",
                        "历史",
                        IconName::Clock,
                        self.page == Some(Route::History),
                        true,
                        false,
                    ),
                    DockItem::action(
                        "dock-favorites",
                        "收藏",
                        IconName::Bookmark,
                        self.page == Some(Route::Favorites),
                        true,
                        false,
                    ),
                    DockItem::action(
                        "dock-messages",
                        "消息",
                        IconName::Bell,
                        self.page == Some(Route::Messages),
                        true,
                        signed_in && self.messages.unread.total() > 0,
                    ),
                    DockItem::action(
                        "dock-refresh",
                        "刷新",
                        IconName::RefreshCw,
                        false,
                        can_refresh,
                        false,
                    ),
                ];
                let on_select: DockSelect<AppRoot> =
                    Rc::new(|this, index, window, cx| this.dock_select(index, window, cx));
                yk_shell::dock::dock(t, &nav, &actions, cx, on_select)
            }).into_any_element(),
                }
                .render(t),
            )
            // 尚未接入的动作给一个明确提示，避免「点了没反应」。
            .children(
                self.search
                    .notice
                    .as_deref()
                    .map(|text| yk_library::search_ui::notice(t, text)),
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
                yk_library::messages_ui::confirm_overlay(
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
                    .map(|text| yk_library::messages_ui::notice(t, text)),
            )
            // 刷新 / 触底加载的浮层提示
            .children(
                (self.feed_refreshing || self.following_refreshing)
                    .then(|| yk_shell::overlay::status_pill(t, "正在刷新当前页面…", true)),
            )
            .children(
                (self.feed_loading_more || self.following_loading_more)
                    .then(|| yk_shell::overlay::status_pill(t, "正在加载更多…", false)),
            )
            // ④ 播放浮窗：独立场景，整块压在页面之上。
            //    它必须挂在页面内容**之后**，且不与页面内容平铺成兄弟层。
            .children(self.gallery.clone())
            // 播放器与 UP 主浮窗的上下关系按来源决定（见 `player_on_top`）：
            // 从播放器开浮窗 → 浮窗在上；在浮窗里点封面 → 播放器在上。
            .children(if self.player_on_top {
                self.up_profile.clone().map(IntoElement::into_any_element)
            } else {
                self.playing.clone().map(IntoElement::into_any_element)
            })
            // 动态详情浮层压在页面与播放器之上；点外部缩回原卡片。
            .children(self.dynamic_detail.clone())
            .children(if self.player_on_top {
                self.playing.clone().map(IntoElement::into_any_element)
            } else {
                self.up_profile.clone().map(IntoElement::into_any_element)
            })
            // 登录卡片在播放器之上，保证登录时不会被视频窗口盖住。
            .children(self.login.clone())
            // 已登录账号面板压在页面之上；点顶栏头像打开。
            .children(self.account.clone())
            // 设置浮层再压一层；从个人面板进入。
            .children(self.settings.clone())
            // 图片查看器压在页面 / 播放器之上，但仍在退出确认之下。
            .children(self.image_viewer.clone())
            // ⌘Q 退出确认压在全部内容之上。
            .children(
                self.quit_hint_shown_at
                    .is_some()
                    .then(|| yk_shell::overlay::quit_hint(t, QUIT_HINT_TEXT)),
            )
            // 搜索浮层永远最上层：播放器 / 账号面板 / 退出提示都压在它之下，
            // 开启时连遮罩一起盖住整窗。
            .children(search_overlay);

        match self.remote_image_cache.clone() {
            Some(cache) => root.image_cache(cache),
            None => root.image_cache(retain_all("app-remote-images")),
        }
    }
}


// ================= ② 光晕 =================


// ================= 顶栏（64px） =================

/// 当前页面是否有可重拉的数据。
///
/// 顶栏刷新与 Dock「刷新」共用这一份判断：没有数据可拉的页面
/// （如热门 / 分区 / 我的）把入口置灰，而不是点了没反应。
fn can_refresh_page(page: Option<Route>, signed_in: bool) -> bool {
    page.is_none()
        || page == Some(Route::Search)
        || page == Some(Route::Rank)
        || (matches!(
            page,
            Some(Route::Following)
                | Some(Route::History)
                | Some(Route::Favorites)
                | Some(Route::Messages)
        ) && signed_in)
}



// ================= Windows 自绘标题栏 =================








fn avatar(
    t: &YkTheme,
    face_url: Option<String>,
    hidden: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let p = &t.palette;
    let face = if hidden {
        None
    } else {
        face_url.filter(|u| !u.is_empty())
    };
    div()
        .id("topbar-avatar")
        .w(px(TOP_BAR_AVATAR_SIZE))
        .h(px(TOP_BAR_AVATAR_SIZE))
        .rounded_full()
        .border_1()
        .border_color(t.surfaces.border_popover)
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
        .when(hidden, |el| el.invisible())
        .child(
            yk_components::avatar::Avatar::new(TOP_BAR_AVATAR_SIZE)
                .url(face.map(|f| yk_domain::models::face_thumb_url(&f)).unwrap_or_default())
                .background(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .build(),
        )
}

// ================= 离线条 =================


// ================= 内容区 =================

fn content_area(
    t: &YkTheme,
    items: &[CoverItem],
    loading: bool,
    behind_player: bool,
    page: Option<Route>,
    state: pages::PageState,
    signed_in: bool,
    history: &history_view::HistoryState,
    favorites: &favorites_view::FavoritesState,
    rank: &rank_view::RankState,
    search: &yk_library::search::SearchState,
    messages: &yk_library::messages::MessageState,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    scroll_settling: bool,
    following: yk_library::following::FeedState<'_>,
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
    if page == Some(Route::Messages) {
        return div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .bg(t.surfaces.bg_base)
            .child(yk_library::messages_ui::page(t, messages, active_images, window, cx))
            .into_any_element();
    }

    // 历史页自带左右分栏 + 左栏独立滚动，不接外壳的 `content-scroll`：
    // 右栏必须固定，跟着整页滚就失去「日期导航」的意义。
    if page == Some(Route::History) {
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
        let body: AnyElement = if kind == Route::Following {
            let offset_y = scroll.base_handle().offset().y.as_f32();
            let viewport_h = scroll.base_handle().bounds().size.height.as_f32();
            let visible_y = if viewport_h > 0.0 {
                (-offset_y)..(-offset_y + viewport_h)
            } else {
                0.0..window.viewport_size().height.as_f32()
            };
            yk_library::following::page(
                t,
                following,
                visible_y,
                active_images.clone(),
                layout_bounds,
                window,
                cx,
            )
        } else if kind == Route::Search {
            yk_library::search_ui::page(
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
        } else if kind == Route::Favorites {
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
        } else if kind == Route::Rank {
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
            pages::render(t, screen_of(kind), state, signed_in).into_any_element()
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

    // ---- 首页：按视口宽切行，虚拟列表只画可见行（实现在 `yk_library::home`）----
    yk_library::home::grid(
        t,
        yk_library::home::HomeState {
            items,
            loading,
            hovered: hovered_card,
            leaving: leaving_card,
            hover_generation,
        },
        layout_bounds,
        active_images,
        scroll,
        window,
        cx,
    )
}

// ================= Dock（页面场景内浮层） =================

/// 底部 Dock 的几何契约：Dock 浮在页面上，内容区底部必须留出
/// `DOCK_RESERVED`，否则最后一行封面会被压住。

/// 播放器入场动画结束后，再延迟一点停绘背后页面。
/// 入场动画 280ms，留 40ms 余量，避免最后一帧还在展开时页面先消失。
const PLAYER_PAGE_HIDE_DELAY_MS: u64 = 320;

struct DockEntry {
    label: &'static str,
    icon: IconName,
    page: Option<Route>,
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
        page: Some(Route::Following),
    },
    DockEntry {
        label: "视频排行榜",
        icon: IconName::Trophy,
        page: Some(Route::Rank),
    },
];



// ================= 工具 =================


/// 验收用：一条纯文字动态。
///
/// 关注流里不一定随时有纯文字动态，而「只有正文 → 左正文右评论」是本次需求
/// 的独立布局分支。`YK_DYNAMIC_TEXT_ONLY=1` 时插到列表最前面，方便截图核对。
#[cfg(debug_assertions)]
pub(crate) fn debug_text_item(index: usize) -> DynamicItem {
    DynamicItem {
        index,
        id: "debug-word-1".to_string(),
        kind: DynamicKind::Text,
        author: yk_infra::dynamic::DynamicAuthor {
            mid: 42,
            name: "一句话的 UP 主".to_string(),
            face: String::new(),
        },
        pub_ts: yk_infra::api::client::now_wts(),
        pub_time: "刚刚".to_string(),
        action: String::new(),
        title: String::new(),
        text: "今天路过一条很窄的巷子，风从两边灌进来，像有人把夏天折成了一封信。\n\n走到巷尾才发现，尽头是一棵很大的梧桐树。树叶把光筛成碎金，落在一只睡着的橘猫身上。\n\n它眯着眼看我，像在说：别急，慢慢来。".to_string(),
        images: Vec::new(),
        video: None,
        link: None,
        stats: yk_infra::dynamic::DynamicStats {
            forward: 3,
            comment: 12,
            like: 128,
        },
        comment_target: Some(yk_infra::dynamic::CommentTarget {
            oid: "1263000000000000000".to_string(),
            kind: 17,
        }),
        origin: None,
        placeholder_color: "#4DB6AC".to_string(),
    }
}

// ================= AppRoot 入口 =================

impl AppRoot {
    /// 打开视频动态：和首页封面卡走同一条播放器入场动画。
    pub(crate) fn open_dynamic_video(
        &mut self,
        item: &DynamicItem,
        origin: Option<CardOrigin>,
        cx: &mut Context<Self>,
    ) {
        let Some(video) = item.video.as_ref() else {
            return;
        };
        if video.bvid.trim().is_empty() {
            eprintln!("[dynamic] 视频动态缺少 bvid：{}", item.id);
            return;
        }
        eprintln!("[dynamic] 展开视频动态：{}（{}）", video.title, video.bvid);
        self.ensure_player(cx);
        if let Some(player) = self.playing.clone() {
            if let Some(origin) = origin {
                let bounds = origin.cover_bounds.unwrap_or(origin.bounds);
                let source = CoverSource {
                    bounds,
                    root_size: origin.root_size,
                    bvid: video.bvid.clone(),
                    title: video.title.clone(),
                    image_url: video.cover_url(640.0, 1.0),
                    placeholder_color: item.placeholder_color.clone(),
                };
                let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
            }
            let _ = player.update(cx, |view, cx| view.open_bvid(video.bvid.clone(), cx));
        }
        cx.notify();
    }

    /// 打开图文 / 文字动态详情浮层。
    pub(crate) fn open_dynamic_detail(
        &mut self,
        item: &DynamicItem,
        origin: Option<CardOrigin>,
        cx: &mut Context<Self>,
    ) {
        if self.dynamic_detail.is_some() {
            return;
        }
        eprintln!(
            "[dynamic] 展开动态详情：{} / {} 张图",
            item.author_name(),
            item.images.len()
        );
        let theme = self.theme.clone();
        let item = item.clone();
        let cookie = self.cookie();
        let detail = cx.new(|cx| DynamicDetailView::new(theme, item, origin, cookie, cx));
        let app = cx.entity().downgrade();
        let _ = detail.update(cx, |view, _| {
            view.set_close_handler(move |cx, finished| {
                if !finished {
                    return;
                }
                let _ = app.update(cx, |root, cx| {
                    root.dynamic_detail_close_requested = true;
                    cx.notify();
                });
            });
        });
        // 点详情页大图 → AppRoot 的查看器（缩放 / 拖动 / 左右切换）。
        let app = cx.entity().downgrade();
        let _ = detail.update(cx, |view, _| {
            view.set_image_open_handler(move |cx, window, urls, index| {
                let _ = app.update(cx, |root, cx| {
                    root.open_image_viewer(urls, index, window, cx);
                });
            });
        });
        self.dynamic_detail = Some(detail.clone());
        // 验收用：YK_DYNAMIC_AUTOCLOSE_MS=1000 在展开后自动触发一次关闭，
        // 用来核对 260ms 缩回原卡片与实体回收。
        if let Some(ms) = std::env::var("YK_DYNAMIC_AUTOCLOSE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            cx.spawn(async move |_this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(ms))
                    .await;
                let _ = detail.update(cx, |view, cx| view.begin_close(cx));
            })
            .detach();
        }
        cx.notify();
    }
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

impl RankHost for AppRoot {
    fn rank_state(&self) -> &RankState {
        &self.rank
    }

    fn rank_state_mut(&mut self) -> &mut RankState {
        &mut self.rank
    }

    fn rank_layout_bounds(&self) -> Option<Bounds<Pixels>> {
        self.layout_bounds.get()
    }
}

impl FavoritesHost for AppRoot {
    fn favorites(&self) -> &FavoritesState {
        &self.favorites
    }

    fn favorites_mut(&mut self) -> &mut FavoritesState {
        &mut self.favorites
    }

    fn favorites_session(&self) -> Option<&UserSession> {
        self.session.as_ref()
    }

    fn favorites_cookie(&self) -> String {
        self.cookie()
    }

    fn favorites_open_login(&mut self, cx: &mut Context<Self>) {
        self.ensure_login(cx);
    }

    fn favorites_reset_hover(&mut self) {
        self.reset_card_hover();
    }
}
