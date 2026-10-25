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
pub mod history_view;
pub mod http;
pub mod login_view;
pub mod page_image_cache;
pub mod pages;
pub mod player;
pub mod settings_view;
pub mod video;
pub mod video_worker;
use page_image_cache::PageImageCache;
use yk_ui::status::SKELETON_PULSE_MS;
use yk_ui::theme::*;

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

    /// 首帧后补齐「当前页面所需的异步数据」。
    ///
    /// 关注页与历史页都需要：`with_page` 只切路由，数据仍要等窗口建好后拉。
    pub fn load_current_page(&mut self, cx: &mut Context<Self>) {
        match self.page {
            Some(pages::PageKind::Following) => self.ensure_following(cx),
            Some(pages::PageKind::History) => self.ensure_history(cx),
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
        self.hovered_card = None;
        self.leaving_card = None;
        self.hover_generation = self.hover_generation.wrapping_add(1);
        let theme = self.theme.clone();
        self.playing = Some(cx.new(|cx| player::PlayerView::new(theme, cx)));
        self.install_player_close_handler(cx);
    }

    /// 打开登录卡片（扫码 / 密码 / 验证码）。
    fn ensure_login(&mut self, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
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
        self.dynamic_detail = None;
        self.dynamic_detail_close_requested = false;
        self.reset_following();
        self.history.reset();
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
        // 换账号后旧的关注流一定不再适用。
        self.reset_following();
        self.refresh_feed_authenticated(cx);
        if self.page == Some(pages::PageKind::Following) {
            self.load_following_page(false, false, cx);
        }
        if self.page == Some(pages::PageKind::History) {
            self.load_history(cx);
        }
        cx.notify();
    }

    /// 用当前会话 Cookie 拉一页新的推荐流（登录成功后调用）。
    fn refresh_feed_authenticated(&mut self, cx: &mut Context<Self>) {
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
                        if !append && std::env::var("YK_DYNAMIC_VIDEO").is_ok() && view.playing.is_none() {
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
                if !finished {
                    return;
                }
                let _ = app.update(cx, |root, cx| {
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
                        view.feed_has_more =
                            added > 0 && !items.is_empty() && !view.feed.is_full();
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
    /// 历史页重拉 cursor 第一页。
    ///
    /// 回顶手势与手动入口共用这一层；1.2s 冷却只加在手势入口上 —— 用户明确
    /// 点了按钮，就该真的重新发一次请求，而不是被防抖吞掉。
    ///
    /// 返回是否发起了刷新：其它页面（热门 / 分区 / 排行 / 我的…）没有可重拉的数据，
    /// 返回 `false`，调用方据此决定要不要把滚动位置拉回顶部。
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
    fn scroll_content_to_top(&self) {
        self.content_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.content_scroll.base_handle().scroll_to_top_of_item(0);
    }

    /// 滚到底部时按当前页面追加数据。
    fn auto_load_more(&mut self, cx: &mut Context<Self>) {
        match self.page {
            None => self.load_more_feed(cx),
            Some(pages::PageKind::Following) => self.load_following_page(true, false, cx),
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
            session: None,
            login: None,
            account: None,
            settings: None,
            page_image_cache: None,
            active_images: Rc::new(RefCell::new(ImageResidency::default())),
            scroll_watchdog_started: false,
            last_scroll_offset_y: 0.0,
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
            layout_bounds: Rc::new(Cell::new(None)),
            offline: false,
            offline_updated_at: None,
            hovered_card: None,
            leaving_card: None,
            offline_retrying: false,
            offline_error: None,
            hover_generation: 0,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            motion_watchdog_started: false,
        }
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.close_player_requested {
            self.close_player_requested = false;
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
                        let handle = view.content_scroll.base_handle().clone();
                        let y = handle.offset().y.as_f32();
                        let max = handle.max_offset().y.as_f32();

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

        let t = &self.theme;

        let layout_bounds = self.layout_bounds.clone();
        div()
            .relative()
            .size_full()
            // ⓪ 页面原点探针：封面快照用它把窗口坐标换算成页面坐标。
            //    canvas 自己不可见、不吃点击，每帧 prepaint 更新。
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        layout_bounds.set(Some(bounds));
                    },
                    |_, _, _, _| {},
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
                            self.page,
                            self.page_state,
                            self.session.is_some(),
                            &self.history,
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
            .child(dock(t, self.page, self.session.is_some(), cx))
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
                .child(
                    div()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(p.theme_primary),
                )
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

fn top_bar(
    t: &YkTheme,
    face_url: Option<String>,
    hide_avatar: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .id("topbar-drag")
        .relative()
        // 整条顶栏都能按住拖动窗口。原生透明标题栏只让顶部那条 titlebar
        // 高度参与拖动，顶栏下半部分实测按不住 —— 在用户眼里就是 bug。
        // 这里显式接管：按下即发起系统窗口拖拽，顶栏里的控件用自己的
        // `occlude()` 把事件截住，所以点输入框 / 图标 / 头像不会误拖窗口。
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move()
        })
        .flex()
        .items_center()
        .w_full()
        .h(px(TOP_BAR_HEIGHT))
        // 设计稿 03：窗口内边距 20，搜索框居中最大 520，右侧 38 圆形图标组。
        // macOS 标题栏透明后红黄绿按钮压在顶栏左上，logo 再让出 64px。
        .pl(px(if cfg!(target_os = "macos") {
            SPACE_5 + 64.0
        } else {
            SPACE_5
        }))
        .pr(px(SPACE_5))
        .gap(px(SPACE_4))
        // 顶栏压在内容上：必须用 Strong 变体（浮层压在内容之上，
        // 而 GPUI 没有元素级 backdrop blur，只提高自身不透明度）
        .bg(s.surface_elevated_strong)
        .border_b_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .flex_shrink_0()
                .child(
                    Icon::new(IconName::Tv)
                        .size(px(24.))
                        .text_color(p.theme_primary),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(s.text_primary)
                        .child("YKPili"),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .child(search_bar(t)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .flex_shrink_0()
                // 历史 / 收藏 / 消息 / 刷新 四个快捷动作已挪进底部 Dock，
                // 顶栏右侧只保留账号头像。
                .child(avatar(t, face_url, hide_avatar, cx)),
        )
}

fn search_bar(t: &YkTheme) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut shadow = t.shadows.shadow_1.clone();
    shadow.extend(t.shadows.edge_glow_1.clone());
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .w_full()
        .max_w(px(520.))
        .h(px(TOP_BAR_CONTROL_HEIGHT))
        // 胶囊搜索框：左 16、右 8，按钮离边 8。
        .pl(px(SPACE_4))
        .pr(px(SPACE_2))
        .rounded_full()
        .bg(s.surface_elevated)
        .border_1()
        .border_color(s.border_popover)
        .shadow(shadow)
        // 搜索框整体是控件，不参与顶栏拖动：挡住顶栏的拖拽 hitbox。
        .occlude()
        .child(
            Icon::new(IconName::Search)
                .size(px(18.))
                .text_color(s.text_quaternary),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_quaternary)
                .child("搜索视频、UP 主、番剧"),
        )
        .child(
            div()
                .id("topbar-search-go")
                .h(px(34.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .child("搜索"),
        )
}

fn top_icon_button(t: &YkTheme, icon: IconName, unread: bool) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let bg = s.fill_2;
    let fg = s.text_primary;
    let mut button = div()
        .relative()
        .w(px(38.))
        .h(px(38.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_color(s.text_secondary)
        .cursor_pointer()
        .hover(move |style| style.bg(bg).text_color(fg))
        // 图标按钮是控件，不能把按下事件冒泡给顶栏的拖拽层。
        .occlude()
        .child(Icon::new(icon).size(px(18.)));
    if unread {
        button = button.child(
            div()
                .absolute()
                .top(px(8.))
                .right(px(9.))
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(p.bili_pink),
        );
    }
    button
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
    page: Option<pages::PageKind>,
    state: pages::PageState,
    signed_in: bool,
    history: &history_view::HistoryState,
    following: dynamic_feed::FeedState<'_>,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    scroll: &VirtualListScrollHandle,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    // Dock 现在是底部横向浮层：横向不再占位，内容区左右各留 24；
    // 底部留出 `DOCK_RESERVED`，否则最后一行封面会被 Dock 压住 ——
    // 浮层「在上」不等于内容可以钻到它背后。

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
    let cols = (((available + SPACE_4) / (COVER_CELL_WIDTH + SPACE_4)).floor() as usize).max(1);
    // 行高 = 卡片高 + 行间距；行内只画卡片，多出来的 16 就是行距。
    let row_h = COVER_CELL_HEIGHT + SPACE_4;

    // 首屏加载：用与真实卡片完全同几何的骨架卡，避免数据回来时网格跳动。
    // 只用一个 repeat 动画驱动整块网格的 opacity（1 → 0.45 → 1，1.5s），
    // 不在每张骨架卡上各排一条动画，也不引入任何额外颜色。
    if loading {
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
    let viewport_h =
        (window.viewport_size().height.as_f32() - TOP_BAR_HEIGHT).max(row_h);
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
                    let hovered_idx = this.hovered_card;
                    let leaving_idx = this.leaving_card;
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
    let hover_shadow = t.shadows.shadow_card_hover.clone();
    let hover_border = s.border_popover;
    let hover_bg = s.surface_content;
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
                .child(byline(
                    t,
                    item,
                    load_cover,
                    active_images.clone(),
                )),
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
            if *hovered {
                if this.hovered_card != Some(idx) {
                    // 从上一张卡移进来：让上一张播放收尾动画。
                    if let Some(prev) = this.hovered_card {
                        this.leaving_card = Some(prev);
                    }
                    // 重新进入正在收尾的自己：取消收尾，直接反向。
                    if this.leaving_card == Some(idx) {
                        this.leaving_card = None;
                    }
                    this.hovered_card = Some(idx);
                    this.hover_generation = this.hover_generation.wrapping_add(1);
                    cx.notify();
                }
            } else if this.hovered_card == Some(idx) {
                this.hovered_card = None;
                this.leaving_card = Some(idx);
                this.hover_generation = this.hover_generation.wrapping_add(1);
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
) -> Div {
    let s = &t.surfaces;
    let p = &t.palette;
    let hover_bg = s.fill_1;
    let hover_fg = s.text_primary;
    let fg = if enabled {
        s.text_secondary
    } else {
        s.text_quaternary
    };
    let mut action = div()
        .h(px(DOCK_ITEM_HEIGHT))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded_full()
        .text_size(px(FONT_SIZE_CAPTION))
        .font_weight(FontWeight::MEDIUM)
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

fn dock(
    t: &YkTheme,
    page: Option<pages::PageKind>,
    signed_in: bool,
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
        .bg(s.surface_elevated_strong)
        .border_1()
        .border_color(s.border_subtle)
        // 浮层必须挡住下层的封面：胶囊的 padding / 按钮间隙没有自己的
        // 点击处理器，不 occlude 的话点缝里会直接点开背后的封面。
        .occlude()
        .shadow(t.shadows.shadow_3.clone());

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
        let hover_bg = if active { p.theme_primary } else { s.fill_1 };
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
                    this.page = target;
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
    let can_refresh = page.is_none()
        || (matches!(
            page,
            Some(pages::PageKind::Following) | Some(pages::PageKind::History)
        ) && signed_in);
    row = row
        .child(
            div()
                .w(px(1.))
                .h(px(20.))
                .mx(px(SPACE_1))
                .bg(s.border_subtle),
        )
        // 顺序：历史 / 收藏 / 消息 / 刷新；与导航项一样带文字
        .child(
            dock_action(t, IconName::Clock, "历史", false, true)
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
                    this.top_refresh_armed = false;
                    this.pull_refresh_triggered = false;
                    if this.session.is_none() {
                        this.ensure_login(cx);
                    }
                    this.ensure_history(cx);
                    this.history.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    window.refresh();
                    cx.notify();
                })),
        )
        .child(dock_action(t, IconName::Bookmark, "收藏", false, true))
        .child(dock_action(t, IconName::Bell, "消息", true, true))
        .child(
            dock_action(t, IconName::RefreshCw, "刷新", false, can_refresh)
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
            }),
            stat: Some(VideoStat {
                view: VIEWS[i % 6],
                danmaku: DANMAKU[i % 6],
            }),
        })
        .collect()
}
