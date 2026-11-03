//! UP 主浮窗：点 UP 主头像展开的「他人空间」，设计口径见 UX 文档 12.9 节。
//!
//! 形态与播放浮窗同族：压暗底层、280ms 弹出 / 260ms 回收、`Esc` 或点遮罩关闭。
//! 入场曲线用 `motion::ease_entrance()`（尾部约 0.9% 过冲），退场用 `ease_standard()`；
//! 与 `account_view` 一样，把外壳从「触发元素的位置」插值到「面板位置」，
//! 内容在 p > 0.55 之后才淡入，避免被拉扁的中间帧露出来。
//!
//! 数据：`yk_infra::space` 一次拿资料 + 标签 + 播放/获赞 + 关注数，投稿单独一次。

use gpui::component::menu::ContextMenuExt as _;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_infra::dynamic::{asset_url, fitted_width};
use yk_domain::format;
use yk_infra::space::{SpaceOrder, SpaceProfile, SpaceVideo};
use yk_theme::theme::*;

/// 动画 id 的进程内序号：复用旧 id 会让重开「顿住」。
static UP_PROFILE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const OPEN_MS: u64 = 280;
const CLOSE_MS: u64 = 260;
const PANEL_W: f32 = 1000.0;
const PANEL_H: f32 = 660.0;
const BANNER_H: f32 = 176.0;
const HEADER_H: f32 = 230.0;
const TABS_H: f32 = 44.0;
const AVATAR: f32 = 72.0;
const PAD_X: f32 = 20.0;
/// 数据行 / 资料行距 header 底边多少：负值表示压到头图下沿以外，
/// 与 Tab 条（HEADER_H + 10 起）之间仍留 2px。
const STATS_BOTTOM: f32 = 2.0;
const IDENT_LEFT: f32 = 104.0; // 20 + 72 + 12
const GRID_COLS: usize = 4;
/// 视频投稿 / 专栏投稿每页取多少：12 条正好铺满三行 4 列网格，
/// 再多就是在为一个屏幕外的内容付请求与渲染的钱。
const VIDEOS_PAGE_SIZE: u32 = 12;
const ARTICLES_PAGE_SIZE: u32 = 12;
const GRID_GAP: f32 = 12.0;
const CARD_W: f32 =
    (PANEL_W - PAD_X * 2.0 - GRID_GAP * (GRID_COLS as f32 - 1.0)) / GRID_COLS as f32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// 动态（第一个标签，打开就看这个）
    Feed,
    /// 视频投稿
    Videos,
    /// 专栏投稿
    Articles,
}

pub struct UpProfileView {
    theme: YkTheme,
    mid: i64,
    /// 打开瞬间就能显示的兜底信息（来自触发元素），资料回来后被覆盖。
    name: String,
    face: String,
    profile: Option<SpaceProfile>,
    videos: Vec<SpaceVideo>,
    total: u64,
    /// 投稿是否已经请求过（点了「投稿」Tab 才拉，见 12.9 的懒加载约定）。
    videos_requested: bool,
    videos_page: u32,
    videos_has_more: bool,
    videos_loading: bool,
    videos_error: Option<String>,
    /// 动态：5 条一页，滚到底再拉。
    dynamics: Vec<yk_infra::space::SpaceDynamic>,
    /// 已取回但还没上屏的动态：接口一页给十几条，这里按 5 条一批往外放。
    dynamics_buffer: Vec<yk_infra::space::SpaceDynamic>,
    dynamics_offset: String,
    dynamics_has_more: bool,
    dynamics_loading: bool,
    dynamics_error: Option<String>,
    dynamics_started: bool,
    /// 专栏投稿。
    articles: Vec<yk_infra::space::SpaceArticle>,
    articles_page: u32,
    articles_has_more: bool,
    articles_loading: bool,
    articles_error: Option<String>,
    articles_started: bool,
    tab: Tab,
    loading: bool,
    error: Option<String>,
    started: bool,
    closing: bool,
    closing_generation: u64,
    open_generation: u64,
    /// 触发元素的屏幕位置：动画从它长出来，关闭时缩回去。
    opener: Bounds<Pixels>,
    viewport: Size<Pixels>,
    scroll: ScrollHandle,
    followed: bool,
    follow_busy: bool,
    close_handler: Option<Box<dyn Fn(&mut App)>>,
    /// 点封面开播：把封面几何交给 AppRoot，让它走统一的「封面长成播放器」入口。
    /// 焦点句柄：面板要参与焦点系统，和播放器面板一致。
    ///
    /// 没有 `track_focus` 的元素树里，`SelectableText` 的鼠标按下会被
    /// 「抢焦点」那一拍吃掉（或者选区根本起不来），而右键菜单开关一次
    /// 又会把选区的按下/松开计数搞乱——表现就是「刚进来选不中，右键一次
    /// 之后松手还在选」。
    focus_handle: FocusHandle,
    /// 首帧渲染后再交焦点（和播放器同一处理）：元素树还没挂 `track_focus`
    /// 时直接 focus 会丢，表现为「第一次拖选无效，得先点一下」。
    needs_focus: bool,
    play_handler: Option<Box<dyn Fn(&mut App, &mut Window, String, yk_player::CoverSource)>>,
    /// 点开图片预览：交给 AppRoot 的 `open_image_viewer`——
    /// 全站只有这一个预览实现，动态、评论、播放器面板都走它。
    image_handler: Option<Box<dyn Fn(&mut App, &mut Window, Vec<String>, usize)>>,
}

impl UpProfileView {
    pub fn new(
        theme: YkTheme,
        mid: i64,
        name: String,
        face: String,
        opener: Bounds<Pixels>,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx;
        Self {
            theme,
            mid,
            name,
            face,
            profile: None,
            videos: Vec::new(),
            total: 0,
            videos_requested: false,
            videos_page: 0,
            videos_has_more: true,
            videos_loading: false,
            videos_error: None,
            dynamics: Vec::new(),
            dynamics_buffer: Vec::new(),
            dynamics_offset: String::new(),
            dynamics_has_more: true,
            dynamics_loading: false,
            dynamics_error: None,
            dynamics_started: false,
            articles: Vec::new(),
            articles_page: 0,
            articles_has_more: true,
            articles_loading: false,
            articles_error: None,
            articles_started: false,
            tab: Tab::Feed,
            loading: false,
            error: None,
            started: false,
            closing: false,
            closing_generation: 0,
            open_generation: UP_PROFILE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            opener,
            viewport,
            scroll: ScrollHandle::new(),
            followed: false,
            follow_busy: false,
            close_handler: None,
            focus_handle: cx.focus_handle(),
            needs_focus: true,
            play_handler: None,
            image_handler: None,
        }
    }

    pub fn set_close_handler(&mut self, close: impl Fn(&mut App) + 'static) {
        self.close_handler = Some(Box::new(close));
    }

    pub fn set_play_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, String, yk_player::CoverSource) + 'static,
    ) {
        self.play_handler = Some(Box::new(handler));
    }

    /// 测试 / 截图入口：直接灌一批动态（不走网络），走真实渲染路径。
    #[doc(hidden)]
    pub fn with_dynamics(mut self, items: Vec<yk_infra::space::SpaceDynamic>) -> Self {
        self.dynamics = items;
        self.dynamics_buffer.clear();
        self.dynamics_loading = false;
        self.dynamics_has_more = false;
        self.tab = Tab::Feed;
        self
    }

    /// 测试用：替换动态列表（不走网络）。
    #[doc(hidden)]
    pub fn set_dynamics(&mut self, items: Vec<yk_infra::space::SpaceDynamic>) {
        self.dynamics = items;
        self.dynamics_buffer.clear();
        self.dynamics_loading = false;
        self.dynamics_has_more = false;
        self.tab = Tab::Feed;
    }

    /// 测试用：当前动态条数（确认注入生效）。
    #[doc(hidden)]
    pub fn test_dynamics_len(&self) -> usize {
        self.dynamics.len()
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn set_image_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, Vec<String>, usize) + 'static,
    ) {
        self.image_handler = Some(Box::new(handler));
    }

    pub fn mid(&self) -> i64 {
        self.mid
    }

    /// 分阶段取数，顺序按「屏幕从上往下的可见顺序」排：
    ///
    /// 1. `x/web-interface/card` —— **一个请求**拿到人 + 粉丝/获赞/投稿数 + 关注状态；
    /// 2. `acc/info` —— 只补卡里没有的头图与加入时间；
    /// 3. 动态首页 5 条 —— 第一个标签的正文。
    ///
    /// 视频 / 专栏不在这里拉，点了对应标签再说。
    fn start_fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.profile.is_some() {
            return;
        }
        self.loading = true;
        let mid = self.mid;
        let cookie = yk_player::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();

        // 阶段 1：名片。
        let card_cookie = cookie.clone();
        cx.spawn(async move |this, cx| {
            let card = cx
                .background_executor()
                .spawn({
                    let cookie = card_cookie.clone();
                    async move {
                        let client = yk_infra::api::client::ReqwestClient::new()
                            .map_err(|error| error.to_string())?;
                        yk_infra::space::fetch_space_card(&client, &cookie, mid)
                    }
                })
                .await;
            match card {
                Ok(card) => {
                    let _ = this.update(cx, |view, cx| {
                        view.loading = false;
                        view.apply_card(card, cx);
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |view, cx| view.set_error(error, cx));
                    return;
                }
            }
        })
        .detach();

        // 阶段 2：头图 + 加入时间（card 里没有）。
        let basic_cookie = cookie.clone();
        cx.spawn(async move |this, cx| {
            let basic = cx
                .background_executor()
                .spawn({
                    let cookie = basic_cookie.clone();
                    async move {
                        let client = yk_infra::api::client::ReqwestClient::new()
                            .map_err(|error| error.to_string())?;
                        yk_infra::space::fetch_space_basic(&client, &cookie, mid)
                    }
                })
                .await;
            if let Ok(basic) = basic {
                let _ = this.update(cx, |view, cx| view.apply_top_photo(basic, cx));
            }
        })
        .detach();

        // 阶段 3：总播放。`card` 只带获赞，`acc/info` 只带注册时间/头图，
        // 播放只有 `x/space/upstat` 有；未登录时它不返回，界面就不显示这一项。
        let upstat_cookie = cookie.clone();
        cx.spawn(async move |this, cx| {
            let plays = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>(yk_infra::space::fetch_space_upstat(&client, &upstat_cookie, mid))
                })
                .await;
            if let Ok((plays, _likes)) = plays {
                let _ = this.update(cx, |view, cx| {
                    if let Some(profile) = view.profile.as_mut() {
                        profile.plays = plays;
                    }
                    cx.notify();
                });
            }
        })
        .detach();

        // 阶段 4：动态首页。
        self.start_dynamics_fetch(cx);
    }

    /// 拉一页动态（5 条）。第一次进面板自动拉一次，之后滚到底再续。
    fn start_dynamics_fetch(&mut self, cx: &mut Context<Self>) {
        if self.dynamics_loading || !self.dynamics_has_more {
            return;
        }
        self.dynamics_loading = true;
        self.dynamics_error = None;
        self.dynamics_started = true;
        let mid = self.mid;
        let offset = self.dynamics_offset.clone();
        let cookie = yk_player::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::space::fetch_space_dynamics(&client, &cookie, mid, &offset)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.dynamics_loading = false;
                match page {
                    Ok((items, next, more)) => {
                        view.dynamics_buffer.extend(items);
                        view.dynamics_offset = next;
                        view.dynamics_has_more = more;
                        view.reveal_dynamics();
                    }
                    Err(error) => view.dynamics_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 一次只上屏 5 条动态；缓冲空了才去要下一页。
    fn reveal_dynamics(&mut self) {
        let take = self.dynamics_buffer.len().min(5);
        if take > 0 {
            self.dynamics.extend(self.dynamics_buffer.drain(..take));
        }
    }

    /// 还有没有未上屏的内容（缓冲或接口游标任一非空）。
    fn dynamics_remaining(&self) -> bool {
        !self.dynamics_buffer.is_empty() || self.dynamics_has_more
    }

    /// 视频投稿：点标签拉第一页（30 条），滚到底续下一页。
    fn start_videos_fetch(&mut self, cx: &mut Context<Self>) {
        if self.videos_loading || !self.videos_has_more {
            return;
        }
        self.videos_requested = true;
        self.videos_loading = true;
        self.videos_error = None;
        let mid = self.mid;
        let pn = self.videos_page + 1;
        let cookie = yk_player::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::space::fetch_space_videos(
                        &client,
                        &cookie,
                        mid,
                        pn,
                        VIDEOS_PAGE_SIZE,
                        SpaceOrder::Newest,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.videos_loading = false;
                match page {
                    Ok(page) => {
                        let got = page.items.len() as u64;
                        view.videos_page = pn;
                        if pn <= 1 {
                            view.videos = page.items;
                        } else {
                            view.videos.extend(page.items);
                        }
                        view.total = page.total.max(view.videos.len() as u64);
                        // 拿满一页就认为还有下一页；总数已知时以总数为准。
                        // 只认「这一页有没有内容」：`page.count` 有时是 0 或者
                        // 不准确的旧值，用它算 has_more 会让列表在几页后突然停住。
                        view.videos_has_more = got > 0;
                        view.videos_error = None;
                    }
                    Err(error) => view.videos_error = Some(error),
                }
                cx.notify();

            });
        })
        .detach();
    }

    /// 专栏投稿：点了「专栏投稿」标签才拉，一次 20 篇。
    fn start_articles_fetch(&mut self, cx: &mut Context<Self>) {
        if self.articles_loading || !self.articles_has_more {
            return;
        }
        self.articles_loading = true;
        self.articles_error = None;
        self.articles_started = true;
        let mid = self.mid;
        let pn = self.articles_page + 1;
        let cookie = yk_player::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::space::fetch_space_articles(
                        &client,
                        &cookie,
                        mid,
                        pn,
                        ARTICLES_PAGE_SIZE,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.articles_loading = false;
                match page {
                    Ok((items, total)) => {
                        view.articles_page = pn;
                        let got = items.len();
                        view.articles.extend(items);
                        // 拿满一页就认为还有下一页；总数已知时以总数为准。
                        view.articles_has_more = got > 0;
                    }
                    Err(error) => view.articles_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }

    /// 名片落地：一个请求就把整块头部（人 + 数字 + 关注状态）画出来。
    fn apply_card(&mut self, card: yk_infra::space::SpaceCard, cx: &mut Context<Self>) {
        self.name = card.name.clone();
        self.face = card.face.clone();
        self.followed = card.followed;
        self.total = card.archive_count;
        let mut profile = self.profile.take().unwrap_or_default();
        profile.mid = card.mid;
        profile.name = card.name;
        profile.face = card.face;
        profile.sign = card.sign;
        profile.level = card.level;
        profile.official_title = card.official_title;
        profile.vip_label = card.vip_label;
        profile.fans = card.fans;
        profile.follows = card.follows;
        profile.likes = card.likes;
        self.profile = Some(profile);
        self.error = None;
        cx.notify();
    }

    /// 第二帧补头图与加入时间——这两项只有 `acc/info` 有。
    fn apply_top_photo(&mut self, basic: yk_infra::space::SpaceBasic, cx: &mut Context<Self>) {
        if basic.joined_at == 0 {
            eprintln!("[space] acc/info 没有返回 jointime（注册时间未知），这一项不显示");
        }
        if let Some(profile) = self.profile.as_mut() {
            if !basic.top_photo.is_empty() {
                profile.top_photo = basic.top_photo;
            }
            profile.joined_at = basic.joined_at;
            if profile.sign.is_empty() {
                profile.sign = basic.sign;
            }
        }
        cx.notify();
    }

    /// AppRoot 收到 Esc 时直接调它，走同一套回收动画。
    pub fn begin_close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.closing_generation =
            UP_PROFILE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.closing_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(CLOSE_MS + 30))
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.closing || view.closing_generation != generation {
                    return;
                }
                view.closing = false;
                if let Some(handler) = view.close_handler.as_ref() {
                    handler(cx);
                }
            });
        })
        .detach();
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        if self.follow_busy || self.closing {
            return;
        }
        let Some(session) = yk_player::comment_section::current_session() else {
            self.error = Some("请先登录后再关注".into());
            cx.notify();
            return;
        };
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let mid = self.mid;
        let next = !self.followed;
        self.follow_busy = true;
        // 乐观更新：失败再回滚。
        self.followed = next;
        if let Some(profile) = self.profile.as_mut() {
            profile.fans = if next {
                profile.fans.saturating_add(1)
            } else {
                profile.fans.saturating_sub(1)
            };
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::follow_user(&client, &cookie, &csrf, mid, next)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.follow_busy = false;
                if let Err(error) = result {
                    view.followed = !next;
                    if let Some(profile) = view.profile.as_mut() {
                        profile.fans = if view.followed {
                            profile.fans.saturating_add(1)
                        } else {
                            profile.fans.saturating_sub(1)
                        };
                    }
                    view.error = Some(format!("关注失败：{error}"));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn target_bounds(&self) -> Bounds<Pixels> {
        let w = PANEL_W.min(self.viewport.width.as_f32() - 48.0).max(360.0);
        let h = PANEL_H.min(self.viewport.height.as_f32() - 48.0).max(360.0);
        let x = ((self.viewport.width.as_f32() - w) / 2.0).max(24.0);
        let y = ((self.viewport.height.as_f32() - h) / 2.0).max(24.0);
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    // ---------------------------------------------------------------- 渲染

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.started {
            self.started = true;
            cx.defer_in(window, |view, _window, cx| view.start_fetch(cx));
        }
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.focus_handle.clone();
            // 等这一帧的元素树挂上 `track_focus` 之后再交焦点，
            // 否则首帧的 focus 会被丢掉，用户得先点一下才能拖选。
            cx.defer_in(window, move |_view, window, cx| {
                window.focus(&handle, cx);
            });
        }
        let t = self.theme.clone();
        let scale = window.scale_factor();
        let content = self.panel(&t, scale, cx);
        let target = self.target_bounds();
        let shell = UpProfileShell {
            theme: t.clone(),
            face: self.face.clone(),
            content: Some(content),
            source: self.opener,
            target,
            progress: 0.0,
        };
        // 对照实验开关：跳过入场/退场动画包装，直接出终态。
        // 用来验证「动画包装（RenderOnce + lerp_bounds）是否打断了文本选区」。
        const SKIP_SHELL_ANIMATION: bool = false;
        let animated = if SKIP_SHELL_ANIMATION {
            // 完全不包动画元素：直接出终态（progress=1 → 目标 bounds）。
            UpProfileShell { progress: 1.0, ..shell }.into_any_element()
        } else if self.closing {
            shell
                .with_animation(
                    format!("up-profile-close-{}", self.closing_generation),
                    Animation::new(std::time::Duration::from_millis(CLOSE_MS)).with_easing(|t| t),
                    |el, p| UpProfileShell {
                        progress: 1.0 - p,
                        ..el
                    },
                )
                .into_any_element()
        } else {
            shell
                .with_animation(
                    format!("up-profile-open-{}", self.open_generation),
                    Animation::new(std::time::Duration::from_millis(OPEN_MS)).with_easing(|t| t),
                    |el, p| UpProfileShell { progress: p, ..el },
                )
                .into_any_element()
        };

        // 遮罩：0.55 黑，与浮窗同一帧起收（12.9.3 的时间线）。
        let scrim = div()
            .absolute()
            .inset_0()
            .bg(rgba(0x0000008C));
        let scrim = if self.closing {
            scrim
                .with_animation(
                    format!("up-profile-scrim-close-{}", self.closing_generation),
                    Animation::new(std::time::Duration::from_millis(CLOSE_MS)),
                    |el, p| el.opacity(1.0 - p),
                )
                .into_any_element()
        } else {
            scrim
                .with_animation(
                    format!("up-profile-scrim-{}", self.open_generation),
                    Animation::new(std::time::Duration::from_millis(OPEN_MS)),
                    |el, p| el.opacity(p),
                )
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("up-profile-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.begin_close(cx);
                    }))
                    .child(scrim),
            )
            .child(animated)
    }

    fn panel(&self, t: &YkTheme, scale: f32, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let profile = self.profile.as_ref();
        let name = profile
            .map(|i| i.name.clone())
            .unwrap_or_else(|| self.name.clone());
        let sign = profile.map(|i| i.sign.clone()).unwrap_or_default();
        let tags: Vec<String> = profile.map(|i| i.tags.clone()).unwrap_or_default();
        let official = profile.map(|i| i.official_title.clone()).unwrap_or_default();
        let vip = profile.map(|i| i.vip_label.clone()).unwrap_or_default();
        let level = profile.map(|i| i.level).unwrap_or(0);
        let fans = profile
            .map(|i| format::count(i.fans))
            .unwrap_or_else(|| "—".into());
        let follows = profile
            .map(|i| format::count(i.follows))
            .unwrap_or_else(|| "—".into());
        let likes = profile
            .map(|i| format::count(i.likes))
            .unwrap_or_else(|| "—".into());
        let plays = profile
            .map(|i| format::count(i.plays))
            .unwrap_or_else(|| "—".into());
        // `jointime` 为 0 = 这个账号该字段没返回（风控 / 老号 / 接口降级），
        // 与其显示一个空值，不如整项不出现。
        let joined = profile
            .map(|i| i.joined_at)
            .filter(|ts| *ts > 0)
            .map(format::date_time);
        // 「最近更新」只能从最新一条投稿推；投稿没拉过就先不显示这一项，
        // 宁缺勿假（12.9 的约定：没有数据就整项不出现，不写「—」）。
        let updated = profile
            .and_then(|i| i.pinned.as_ref().map(|v| v.created))
            .or_else(|| self.videos.first().map(|v| v.created))
            .map(format::relative_time);

        // ---- 头图：铺满宽度，底部遮罩给白字兜底 ----
        // 头图用 UP 主自己设的那张（`acc/info` 的 top_photo），
        // 取不到 / 加载失败时底下这层品牌渐变就是兜底。
        let top_photo = profile.map(|i| i.top_photo.clone()).unwrap_or_default();
        let mut banner = div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .right(px(0.0))
            .h(px(BANNER_H))
            .bg(linear_gradient(
                118.0,
                linear_color_stop(rgb(0x0E3A4A), 0.0),
                linear_color_stop(rgb(0x3D2E5C), 1.0),
            ));
        if !top_photo.is_empty() {
            let url = asset_url(&top_photo, fitted_width(PANEL_W, scale, 0));
            banner = banner.child(
                img(ImageSource::from(url))
                    .absolute()
                    .inset_0()
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Cover),
            );
        }
        // 压暗层永远压在最上面：白字要靠它拿对比度。
        let banner = banner.child(
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    0.0,
                    linear_color_stop(rgba(0x000000C2), 0.0),
                    linear_color_stop(rgba(0x00000000), 1.0),
                )),
        );

        // ---- 头像：圆心压在头图下沿 ----
        let face_url = if self.face.is_empty() {
            String::new()
        } else {
            asset_url(&self.face, fitted_width(AVATAR, scale, 0))
        };
        let avatar = div()
            .absolute()
            .left(px(PAD_X - 2.0))
            .top(px(BANNER_H - AVATAR / 2.0 - 2.0))
            .w(px(AVATAR + 4.0))
            .h(px(AVATAR + 4.0))
            .rounded_full()
            .bg(rgba(0xFFFFFF6B))
            .child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .top(px(2.0))
                    .w(px(AVATAR))
                    .h(px(AVATAR))
                    .rounded_full()
                    .overflow_hidden()
                    .child(
                        yk_components::avatar::Avatar::new(AVATAR)
                            .url(face_url.clone())
                            .background(linear_gradient(
                                135.0,
                                linear_color_stop(p.bili_blue, 0.0),
                                linear_color_stop(p.bili_pink, 1.0),
                            ))
                            .build(),
                    ),
            );

        // ---- 昵称 + 徽标 ----
        let mut name_row = div()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .h(px(22.0))
            .child(
                div()
                    .max_w(px(300.0))
                    .truncate()
                    .text_size(px(FONT_SIZE_HEADING))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(name),
            );
        if !official.is_empty() {
            name_row = name_row.child(on_image_chip("✓", &official, p.bili_blue));
        }
        if level > 0 {
            name_row = name_row.child(on_image_chip("", &format!("LV{level}"), p.bili_blue));
        }
        if !vip.is_empty() {
            name_row = name_row.child(on_image_chip("♛", &vip, p.bili_pink));
        }

        // ---- 标签 ----
        let mut tag_row = div().flex().items_center().gap(px(6.0)).h(px(20.0));
        for tag in tags.iter().take(5) {
            tag_row = tag_row.child(tag_chip(tag));
        }
        let has_tags = !tags.is_empty();

        let sign_el = div()
            .h(px(20.0))
            .text_size(px(FONT_SIZE_CONTROL))
            .text_color(rgba(0xFFFFFFD1))
            .truncate()
            .child(sign);

        // ---- 关注 / 消息 ----
        let follow_label = if self.follow_busy {
            "处理中…".to_string()
        } else if self.followed {
            "已关注".to_string()
        } else {
            "+ 关注".to_string()
        };
        let followed = self.followed;
        let follow = div()
            .id("up-profile-follow")
            .h(px(CONTROL_HEIGHT))
            .px(px(SPACE_5))
            .flex()
            .items_center()
            .rounded(px(RADIUS_FULL))
            .cursor_pointer()
            .when(followed, |el| {
                el.bg(rgba(0x0000006B))
                    .border_1()
                    .border_color(rgba(0xFFFFFF47))
            })
            .when(!followed, |el| el.bg(p.theme_primary))
            .text_size(px(FONT_SIZE_CONTROL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0xFFFFFF))
            .hover(|style| style.opacity(0.9))
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                view.toggle_follow(cx);
            }))
            .child(follow_label);

        let message = div()
            .h(px(CONTROL_HEIGHT))
            .px(px(SPACE_4))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(RADIUS_FULL))
            .bg(rgba(0x0000006B))
            .border_1()
            .border_color(rgba(0xFFFFFF38))
            .text_size(px(FONT_SIZE_CONTROL))
            .text_color(rgb(0xFFFFFF))
            .child("发消息");

        let ident = div()
            .absolute()
            .left(px(IDENT_LEFT))
            // 从「底边距」锚定而不是固定 top：没有标签时这一块会自己往下坐，
            // 不会在下面留一大块空白（有标签时向上长，仍然贴着头图）。
            .bottom(px(HEADER_H - BANNER_H + 12.0))
            .right(px(PAD_X))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(name_row)
            .child(sign_el)
            .when(has_tags, |el| el.child(tag_row));

        let actions = div()
            .absolute()
            .right(px(PAD_X))
            .top(px(134.0))
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .child(follow)
            .child(message);

        // ---- 数据 | 资料：落在头图下方的底色上 ----
        let plays_value = profile.map(|i| i.plays).unwrap_or(0);
        let mut stats = div()
            .absolute()
            .left(px(IDENT_LEFT))
            .bottom(px(STATS_BOTTOM))
            .flex()
            .items_end()
            .gap(px(SPACE_5))
            .child(stat_col(&follows, "关注", s))
            .child(stat_col(&fans, "粉丝", s))
            .child(stat_col(&likes, "获赞", s));
        // `upstat` 需要登录态，未登录 / 失败时恒为 0——显示 0 会被当成
        // 「这个 UP 主一部都没人看」，比缺一项更有害。
        if plays_value > 0 {
            stats = stats.child(stat_col(&plays, "播放", s));
        }

        let mid_text = self.mid.to_string();
        let mut facts = div()
            .absolute()
            .right(px(PAD_X))
            .bottom(px(STATS_BOTTOM))
            .flex()
            .items_end()
            .gap(px(SPACE_5))
            .child(fact_col(&mid_text, "UID", s));
        if let Some(joined) = joined.as_ref() {
            facts = facts.child(fact_col(joined, "加入时间", s));
        }
        if let Some(updated) = updated.as_ref() {
            facts = facts.child(fact_col(updated, "最近更新", s));
        }

        let header = div()
            .relative()
            .h(px(HEADER_H))
            .flex_shrink_0()
            // 头部实色：面板底下是任意页面，半透明会让文字底噪不可控。
            .bg(s.surface_elevated_solid)
            .child(banner)
            .child(ident)
            .child(actions)
            .child(stats)
            .child(facts)
            .child(avatar);

        // ---- Tab ----
        let tabs = div()
            .h(px(TABS_H))
            .flex_shrink_0()
            // 与上面的数据行再拉开 10px：贴着数据行时整块显得挤。
            .mt(px(10.0))
            // Tab 条比头部提亮一档：设计稿里「数据区 / 分页区」是两个色块。
            .bg(s.surface_elevated_solid_hover)
            .px(px(PAD_X))
            .flex()
            .items_center()
            .gap(px(SPACE_5))
            .border_b_1()
            .border_color(s.border_subtle)
            .child(self.tab_button(Tab::Feed, "动态", 0, t, cx))
            .child(self.tab_button(Tab::Videos, "视频投稿", self.total, t, cx))
            .child(self.tab_button(Tab::Articles, "专栏投稿", 0, t, cx));

        // ---- 内容 ----
        let body = match self.tab {
            Tab::Feed => self.feed_body(t, cx),
            Tab::Videos => self.videos_body(t, scale, cx),
            Tab::Articles => self.articles_body(t, cx),
        };

        div()
            .id("up-profile-panel")
            .w(px(PANEL_W))
            .h(px(PANEL_H))
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            // **别用 on_click + stop_propagation**：`ClickEvent` 是在 mouse-up 阶段
            // 派发的，这里一 stop，窗口级的选区监听就收不到松开事件，
            // `WindowSelectionState::is_selecting` 会永远停在 true ——
            // 于是之后每次按下都被当成「已经在选」，正文再也选不中。
            // 想让点击不漏到底下的遮罩，用 `occlude()`（挡住更下层的命中框，
            // 但不吞事件）才是对的。
            .occlude()
            .child(header)
            .child(tabs)
            .child(
                div()
                    .id("up-profile-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .bg(s.bg_base)
                    .track_scroll(&self.scroll)
                    .on_scroll_wheel(cx.listener(
                        |view, _: &ScrollWheelEvent, _window, cx| {
                            view.maybe_load_more(cx);
                        },
                    ))
                    .child(body),
            )
            .into_any_element()
    }

    fn tab_button(
        &self,
        tab: Tab,
        label: &str,
        count: u64,
        t: &YkTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let active = self.tab == tab;
        let mut el = div()
            .id(SharedString::from(format!("up-profile-tab-{label}")))
            .relative()
            .h(px(TABS_H))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .text_size(px(FONT_SIZE_CONTROL))
            .text_color(if active {
                s.text_primary
            } else {
                s.text_tertiary
            })
            .when(active, |el| el.font_weight(FontWeight::SEMIBOLD))
            .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                view.tab = tab;
                match tab {
                    Tab::Feed => view.start_dynamics_fetch(cx),
                    Tab::Videos => {
                        if view.videos_page == 0 {
                            view.start_videos_fetch(cx);
                        }
                    }
                    Tab::Articles => view.start_articles_fetch(cx),
                }
                cx.notify();
            }))
            .child(label.to_string());
        if count > 0 {
            el = el.child(
                div()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(if active {
                        p.theme_primary
                    } else {
                        s.text_quaternary
                    })
                    .child(format::count(count)),
            );
        }
        if active {
            el = el.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .h(px(2.0))
                    .rounded(px(2.0))
                    .bg(p.theme_primary),
            );
        }
        el
    }

    /// 第一个标签：动态。5 条一页，滚到尾部的哨兵进入视口就续下一页。
    fn feed_body(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        if let Some(error) = self.dynamics_error.as_ref() {
            return self.state_block(t, "动态加载失败", error, false, cx);
        }
        if self.dynamics.is_empty() {
            if self.dynamics_loading {
                return self.state_block(t, "正在拉取动态…", "先出人，再出动态", false, cx);
            }
            return self.state_block(t, "TA 还没有发过动态", "投稿、转发、图文都会出现在这里", false, cx);
        }
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .p(px(PAD_X))
            .pb(px(SPACE_6));
        for (index, item) in self.dynamics.iter().enumerate() {
            col = col.child(self.dynamic_card(index, item, t, cx));
        }
        if self.dynamics_loading {
            col = col.child(
                div()
                    .py(px(SPACE_3))
                    .text_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_quaternary)
                    .child("正在加载更多…"),
            );
        } else if !self.dynamics_remaining() {
            col = col.child(
                div()
                    .py(px(SPACE_3))
                    .text_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_quaternary)
                    .child("没有更多动态了"),
            );
        }
        col.into_any_element()
    }

    fn dynamic_card(
        &self,
        index: usize,
        item: &yk_infra::space::SpaceDynamic,
        t: &YkTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let time = format::relative_time(item.pub_ts);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .min_w(px(0.0))
            .flex_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_quaternary)
                    .child(time)
                    .when(!item.kind.is_empty(), |el| {
                        el.child(
                            div()
                                .px(px(6.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .rounded(px(BADGE_RADIUS))
                                .bg(p.theme_primary_10)
                                .text_color(p.theme_primary)
                                .child(kind_label(&item.kind)),
                        )
                    }),
            );
        if !item.text.is_empty() {
            // 与评论区同一份实现：拖选 + 右键「复制文本」。
            let text = item.text.clone();
            let notice: std::rc::Rc<dyn Fn(&mut App)> = std::rc::Rc::new(|_: &mut App| {});
            body = body.child(yk_player::comment_view::selectable_text(
                SharedString::from(format!("up-profile-dynamic-text-{index}")),
                ("up-dynamic-select", index),
                // 与评论区一样给确定宽度：面板宽 − 内容内边距 − 行内边距。
                PANEL_W - PAD_X * 2.0 - SPACE_3 * 2.0,
                // 和评论区完全同一条路径：右键弹「复制文本」菜单。
                true,
                text.clone(),
                gpui::TextStyleRefinement {
                    font_size: Some(px(FONT_SIZE_CONTROL).into()),
                    line_height: Some(px(21.0).into()),
                    color: Some(s.text_primary),
                    ..Default::default()
                },
                text,
                notice,
            ));
        }
        if !item.images.is_empty() {
            // 图文动态：最多 9 张，按 3 列铺；单张时按 16:9 大图。
            let cols: usize = if item.images.len() == 1 { 1 } else { 3 };
            let width: f32 = if cols == 1 { 240.0 } else { 96.0 };
            // 高度按「行数 × 单元高 + 间距」显式算出来：
            // item 的高度必须把媒体算进去，否则图片会把下面的统计行挤出去。
            let cell_h = (width * 9.0 / 16.0).round();
            let rows = item.images.len().div_ceil(cols);
            let grid_h = rows as f32 * cell_h + (rows.saturating_sub(1) as f32) * 6.0;
            let all_images = item.images.clone();
            let mut grid = div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .max_w(px(if cols == 1 { 240.0 } else { 300.0 }))
                .h(px(grid_h));
            for (i, src) in item.images.iter().take(9).enumerate() {
                let thumb = asset_url(src, fitted_width(width, 2.0, 0));
                let urls = all_images.clone();
                // 与评论区图片条同一份实现（`comment_view::media_tile`）。
                let weak = cx.entity().downgrade();
                let on_click = std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |view, cx| {
                        if let Some(handler) = view.image_handler.as_ref() {
                            handler(cx, window, urls.clone(), i);
                        }
                    });
                }) as std::rc::Rc<dyn Fn(&mut Window, &mut App)>;
                grid = grid.child(yk_components::media::media_tile(
                    SharedString::from(format!("up-profile-dynamic-img-{index}-{i}")),
                    s.surface_content,
                    thumb,
                    width,
                    cell_h,
                    MEDIA_RADIUS,
                    Some(on_click),
                ));
            }
            body = body.child(grid);
        } else if !item.cover.is_empty() {
            let cover = asset_url(&item.cover, fitted_width(240.0, 2.0, 0));
            if item.kind == "video" {
                // 视频动态：直接复用首页推荐卡片的封面（播放/弹幕/时长都在），
                // 不另做一套小缩略图——同一条视频在首页和这里长得一样。
                let art =
                    crate::cover::cover_art(&item.title, &cover, None, s.fill_3).into_any_element();
                let visual = crate::cover::cover_visual(
                    t,
                    crate::cover::CoverVisual {
                        art,
                        views: item.views,
                        danmaku: item.danmaku,
                        duration_secs: item.duration_secs,
                        is_live: false,
                        show_stats: true,
                    },
                    16.0 / 9.0,
                )
                .into_any_element();
                body = body.child(
                    div().w(px(240.0)).child(playable_cover(
                        SharedString::from(format!("up-profile-dynamic-play-{index}")),
                        item.bvid.clone(),
                        item.title.clone(),
                        cover.clone(),
                        self.viewport,
                        visual,
                        cx,
                    )),
                );
            } else {
                body = body.child(
                    div()
                        .flex()
                        .gap(px(SPACE_3))
                        .items_center()
                        .child(
                            div()
                                .relative()
                                .w(px(120.0))
                                .h(px(68.0))
                                .flex_shrink_0()
                                .rounded(px(MEDIA_RADIUS))
                                .overflow_hidden()
                                .bg(s.surface_content)
                                .child(
                                    match yk_components::media::cover_image(&cover) {
                                Some(image) => image,
                                None => div().into_any_element(),
                            },
                                ),
                        )
                        .when(!item.title.is_empty(), |el| {
                            el.child(
                                div()
                                    .line_clamp(2)
                                    .text_size(px(FONT_SIZE_CONTROL))
                                    .text_color(s.text_secondary)
                                    .child(item.title.clone()),
                            )
                        }),
                );
            }
        }
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_4))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child(format!("{} 赞", format::count(item.likes)))
                .child(format!("{} 评论", format::count(item.comments)))
                .child(format!("{} 转发", format::count(item.forwards))),
        );

        // 不画作者头像：这一页所有动态都是同一个 UP 主，
        // 每条重复一枚头像等于把「署名」变成噪音（同 12.9「卡片去掉署名行」）。
        div()
            .id(SharedString::from(format!("up-profile-dynamic-{index}")))
            .p(px(SPACE_3))
            .rounded(px(RADIUS_LG))
            .border_b_1()
            .border_color(s.border_subtle)
            .child(body)
            .into_any_element()
    }

    /// 第三个标签：专栏投稿。
    fn articles_body(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        if let Some(error) = self.articles_error.as_ref() {
            return self.state_block(t, "专栏加载失败", error, false, cx);
        }
        if self.articles.is_empty() {
            if self.articles_loading {
                return self.state_block(t, "正在拉取专栏…", "只在这一页付请求的时间", false, cx);
            }
            return self.state_block(t, "TA 还没写过专栏", "长篇内容会出现在这里", false, cx);
        }
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .p(px(PAD_X))
            .pb(px(SPACE_6));
        for (index, article) in self.articles.iter().enumerate() {
            let cover = if article.cover.is_empty() {
                String::new()
            } else {
                asset_url(&article.cover, fitted_width(160.0, 2.0, 0))
            };
            let mut row = div()
                .id(SharedString::from(format!("up-profile-article-{index}")))
                .flex()
                .gap(px(SPACE_3))
                .items_center()
                .p(px(SPACE_3))
                .rounded(px(RADIUS_LG))
                .border_b_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w(px(0.0))
                        .flex_1()
                        .child(
                            div()
                                .line_clamp(2)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(s.text_primary)
                                .child(article.title.clone()),
                        )
                        .child(
                            div()
                                .line_clamp(1)
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_tertiary)
                                .child(article.summary.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(SPACE_4))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .child(format::relative_time(article.published_at))
                                .child(format!("{} 阅读", format::count(article.views)))
                                .child(format!("{} 评论", format::count(article.comments))),
                        ),
                );
            if !cover.is_empty() {
                row = row.child(
                    div()
                        .relative()
                        .w(px(104.0))
                        .h(px(66.0))
                        .flex_shrink_0()
                        .rounded(px(MEDIA_RADIUS))
                        .overflow_hidden()
                        .bg(s.surface_content)
                        .child(
                            match yk_components::media::cover_image(&cover) {
                                Some(image) => image,
                                None => div().into_any_element(),
                            },
                        ),
                );
            }
            col = col.child(row);
        }
        if self.articles_loading {
            col = col.child(
                div()
                    .py(px(SPACE_3))
                    .text_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_quaternary)
                    .child("正在加载更多…"),
            );
        }
        col.into_any_element()
    }

    /// 滚到底部附近就续页（与评论区同一套 ScrollHandle 判据）。
    fn maybe_load_more(&mut self, cx: &mut Context<Self>) {
        let offset = self.scroll.offset().y.as_f32();
        let max = self.scroll.max_offset().y.as_f32();
        let viewport = self.scroll.bounds().size.height.as_f32();
        // GPUI 的两个值都是**负数**：`offset` 向下滚越来越负，
        // `max_offset` 是它能到的最负值（顶部为 0）。
        // 因此「离底部的距离」= offset - max（到底时正好是 0），
        // 之前写成 max - offset 等于恒为正、永远判不到底——这就是「怎么滚都不加载」。
        // 判据与评论区 `needs_more` 完全一致。
        // 只认「滚到边界」：距底 60px 以内才算。之前用一整屏的提前量，
        // 加上加载后的自检，表现就是「一进页面就一路拉到底」。
        let near_bottom = max <= -1.0 && offset - max <= 60.0;
        // 内容撑不满一屏时没有滚动事件，滚轮也到不了边界——这种极端情况
        // 允许直接补一页，避免停在半屏。
        let short_content = max > -1.0;
        if !(near_bottom || short_content) {
            return;
        }
        match self.tab {
            Tab::Feed => {
                if !self.dynamics_buffer.is_empty() {
                    self.reveal_dynamics();
                    cx.notify();
                } else if !self.dynamics_loading {
                    self.start_dynamics_fetch(cx);
                }
            }
            Tab::Articles => self.start_articles_fetch(cx),
            Tab::Videos => self.start_videos_fetch(cx),
        }
    }

    fn videos_body(&self, t: &YkTheme, scale: f32, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        // 只有「一条都没加载出来」时才整块报错；翻页失败要保住已有内容，
        // 否则用户滚了三页碰到一次超时，列表就被整段清空了。
        if let Some(error) = self.videos_error.as_ref().filter(|_| self.videos.is_empty()) {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(SPACE_2))
                .p(px(SPACE_10))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("投稿加载失败"),
                )
                .child(
                    div()
                        .max_w(px(420.0))
                        .text_center()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(error.clone()),
                )
                .child(
                    div()
                        .id("up-profile-videos-retry")
                        .mt(px(SPACE_2))
                        .h(px(CONTROL_HEIGHT))
                        .px(px(SPACE_4))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_FULL))
                        .bg(s.surface_content)
                        .border_1()
                        .border_color(s.border_popover)
                        .cursor_pointer()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(s.text_primary)
                        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.videos_requested = false;
                            view.videos_error = None;
                            view.start_videos_fetch(cx);
                        }))
                        .child("重试"),
                )
                .into_any_element();
        }
        // **只有一条都没有**时才让位给加载态：翻页加载时如果也走这条分支，
        // 已上屏的网格会被整块换掉，用户看到的就是「清空重填」。
        if self.videos_loading && self.videos.is_empty() {
            return self.state_block(t, "正在拉取投稿…", "只在这一页付请求的时间", false, cx);
        }
        if self.videos.is_empty() {
            return self.state_block(
                t,
                "TA 还没有投稿",
                "关注后 TA 发了新视频会出现在「正在关注」",
                false,
                cx,
            );
        }
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .p(px(PAD_X))
            .pb(px(SPACE_6))
            .child(self.grid(t, scale, cx));
        if self.videos_loading {
            col = col.child(
                div()
                    .py(px(SPACE_3))
                    .text_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(t.surfaces.text_quaternary)
                    .child("正在加载更多…"),
            );
        } else if let Some(error) = self.videos_error.as_ref() {
            col = col.child(
                div()
                    .id("up-profile-videos-retry")
                    .mx_auto()
                    .my(px(SPACE_2))
                    .h(px(CONTROL_HEIGHT))
                    .px(px(SPACE_4))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(t.surfaces.surface_content)
                    .border_1()
                    .border_color(t.palette.status_error_30)
                    .cursor_pointer()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(t.surfaces.text_primary)
                    .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                        view.videos_error = None;
                        view.start_videos_fetch(cx);
                    }))
                    .child(format!("加载失败，点此重试（{error}）")),
            );
        } else if !self.videos_has_more {
            col = col.child(
                div()
                    .py(px(SPACE_3))
                    .text_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(t.surfaces.text_quaternary)
                    .child("没有更多投稿了"),
            );
        }
        col.into_any_element()
    }

    fn grid(&self, t: &YkTheme, scale: f32, cx: &mut Context<Self>) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(GRID_GAP));
        for (index, video) in self.videos.iter().enumerate() {
            grid = grid.child(self.video_card(video, index, t, scale, cx));
        }
        grid.into_any_element()
    }

    fn video_card(
        &self,
        video: &SpaceVideo,
        index: usize,
        t: &YkTheme,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let cover = asset_url(&video.cover, fitted_width(CARD_W, scale, 0));
        // 封面复用首页推荐卡片的实现：同一个 `cover_art` + `cover_visual`，
        // 播放/弹幕/时长与首页卡片完全一致，不另做一套。
        let art = crate::cover::cover_art(&video.title, &cover, None, s.fill_3).into_any_element();
        let thumbnail = playable_cover(
            SharedString::from(format!("up-profile-video-{index}")),
            video.bvid.clone(),
            video.title.clone(),
            cover.clone(),
            self.viewport,
            crate::cover::cover_visual(
                t,
                crate::cover::CoverVisual {
                    art,
                    views: video.views,
                    danmaku: video.danmaku,
                    duration_secs: video.duration_secs,
                    is_live: false,
                    show_stats: true,
                },
                16.0 / 9.0,
            )
            .into_any_element(),
            cx,
        );
        div()
            .id(SharedString::from(format!("up-profile-video-cell-{index}")))
            .w(px(CARD_W))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(4.0))
            .rounded(px(CARD_RADIUS))
            .cursor_pointer()
            .hover(|style| style.bg(s.surface_content_hover))
            .child(thumbnail)
            .child(
                // 两行标题的高度必须用「行高 × 2」，不能用「字号 × 2 + 4」：
                // 13 号字的行高是 18，30px 的盒子装不下两行，第二行会被裁掉。
                div()
                    .mt(px(SPACE_1))
                    .h(px(LINE_HEIGHT_CONTROL * 2.0))
                    .line_clamp(2)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .line_height(px(LINE_HEIGHT_CONTROL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(s.text_primary)
                    .child(video.title.clone()),
            )
            .into_any_element()
    }

    fn state_block(
        &self,
        t: &YkTheme,
        title: &str,
        detail: &str,
        show_retry: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let mut block = div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(SPACE_2))
            .p(px(SPACE_10))
            .child(
                div()
                    .text_size(px(FONT_SIZE_TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .max_w(px(420.0))
                    .text_center()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_tertiary)
                    .child(detail.to_string()),
            );
        if show_retry {
            block = block.child(
                div()
                    .id("up-profile-retry")
                    .mt(px(SPACE_2))
                    .h(px(CONTROL_HEIGHT))
                    .px(px(SPACE_4))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS_FULL))
                    .bg(s.surface_content)
                    .border_1()
                    .border_color(s.border_popover)
                    .cursor_pointer()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(s.text_primary)
                    .hover(|style| style.bg(s.surface_content_hover))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        view.error = None;
                        view.loading = false;
                        view.profile = None;
                        view.started = false;
                        cx.notify();
                    }))
                    .child("重试"),
            );
        }
        block.into_any_element()
    }
}

impl Render for UpProfileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_view(window, cx)
    }
}

/// 压在图上的胶囊：黑底 + 白字，品牌色只留给图标。
fn on_image_chip(icon: &str, label: &str, accent: Hsla) -> impl IntoElement {
    let mut el = div()
        .h(px(20.0))
        .px(px(7.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded(px(BADGE_RADIUS))
        .bg(rgba(0x00000061))
        .border_1()
        .border_color(rgba(0xFFFFFF2E))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(rgb(0xFFFFFF));
    if !icon.is_empty() {
        el = el.child(
            div()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(accent)
                .child(icon.to_string()),
        );
    }
    el.child(label.to_string())
}

fn tag_chip(label: &str) -> impl IntoElement {
    div()
        .h(px(20.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .rounded(px(BADGE_RADIUS))
        .bg(rgba(0x00000061))
        .border_1()
        .border_color(rgba(0xFFFFFF24))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(rgba(0xFFFFFFE6))
        .child(label.to_string())
}

fn stat_col(value: &str, label: &str, s: &YkSurfaces) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(
            div()
                .text_size(px(FONT_SIZE_TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .child(value.to_string()),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child(label.to_string()),
        )
}

fn fact_col(value: &str, label: &str, s: &YkSurfaces) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_primary)
                .child(value.to_string()),
        )
}

fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |a: Pixels, b: Pixels| px(a.as_f32() + (b.as_f32() - a.as_f32()) * t);
    Bounds {
        origin: point(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        size: size(
            mix(a.size.width, b.size.width),
            mix(a.size.height, b.size.height),
        ),
    }
}

/// 从触发元素长成面板；关闭时反向缩回。内容在 p > 0.55 才淡入，
/// 这样被压扁的中间帧不会露出重排后的文字。
#[derive(gpui::IntoElement)]
struct UpProfileShell {
    theme: YkTheme,
    face: String,
    content: Option<AnyElement>,
    source: Bounds<Pixels>,
    target: Bounds<Pixels>,
    progress: f32,
}

impl RenderOnce for UpProfileShell {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let UpProfileShell {
            theme,
            face,
            content,
            source,
            target,
            progress,
        } = self;
        let p = ease_entrance()(progress.clamp(0.0, 1.0));
        let bounds = lerp_bounds(source, target, p);
        let content_alpha = ((p - 0.55) / 0.45).clamp(0.0, 1.0);
        let s = &theme.surfaces;
        let pal = &theme.palette;
        let mut shell = div()
            .id("up-profile-shell")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(POPOVER_RADIUS))
            .overflow_hidden()
            // 不透明材质：这是压在任意页面之上的浮窗，半透明会把底下的
            // 封面、文字透进来，正文对比度不可控。
            .bg(if face.is_empty() {
                linear_gradient(
                    135.0,
                    linear_color_stop(pal.bili_blue, 0.0),
                    linear_color_stop(pal.bili_pink, 1.0),
                )
            } else {
                linear_gradient(
                    0.0,
                    linear_color_stop(s.surface_elevated_solid, 0.0),
                    linear_color_stop(s.surface_elevated_solid, 1.0),
                )
            })
            .border_1()
            .border_color(s.border_popover)
            .shadow(theme.shadows.shadow_4.clone());
        // **不要在这层 occlude**：`occlude()` 是 `HitboxBehavior::BlockMouse`，
        // 它会盖在整块内容之上参与鼠标命中——评论区的面板没有这一层，
        // 所以那边的 `SelectableText` 能正常按/拖/松手；
        // 浮窗里这层挡在中间，`SelectableText` 的松手判定就收不到。
        // 点击不外泄由内层 `up-profile-panel` 的 `on_click(stop_propagation)` 保证。
        if !face.is_empty() {
            shell = shell.child(
                img(ImageSource::from(face))
                    .absolute()
                    .inset_0()
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Cover)
                    .opacity(1.0 - content_alpha),
            );
        }
        shell.child(
            div()
                .absolute()
                .inset_0()
                .opacity(content_alpha)
                .child(content.expect("UP 主浮窗内容")),
        )
    }
}

/// 给封面挂一层「点了就起播」：canvas 记下封面的屏幕几何，
/// 点击时连同 bvid 一起交回 AppRoot，走统一的「封面长成播放器」入口。
fn playable_cover(
    id: SharedString,
    bvid: String,
    title: String,
    image_url: String,
    viewport: Size<Pixels>,
    child: AnyElement,
    cx: &mut Context<UpProfileView>,
) -> AnyElement {
    let bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>> =
        std::rc::Rc::new(std::cell::Cell::new(None));
    let sink = bounds.clone();
    let playable = !bvid.is_empty();
    div()
        .id(id)
        .relative()
        .w_full()
        .when(playable, |el| el.cursor_pointer())
        .child(child)
        .child(
            canvas(
                move |element_bounds, _window, _cx| sink.set(Some(element_bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .when(playable, |el| {
            el.on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let Some(rect) = bounds.get() else {
                    return;
                };
                let Some(handler) = view.play_handler.as_ref() else {
                    return;
                };
                let source = yk_player::CoverSource {
                    bounds: rect,
                    root_size: viewport,
                    bvid: bvid.clone(),
                    title: title.clone(),
                    image_url: image_url.clone(),
                    placeholder_color: String::new(),
                };
                handler(cx, window, bvid.clone(), source);
            }))
        })
        .into_any_element()
}

/// 动态类型 → 中文标签。
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "video" => "视频",
        "article" => "专栏",
        "image" => "图文",
        "forward" => "转发",
        "live" => "直播",
        _ => "动态",
    }
}
