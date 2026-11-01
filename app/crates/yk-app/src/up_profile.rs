//! UP 主浮窗：点 UP 主头像展开的「他人空间」，设计口径见 UX 文档 12.9 节。
//!
//! 形态与播放浮窗同族：压暗底层、280ms 弹出 / 260ms 回收、`Esc` 或点遮罩关闭。
//! 入场曲线用 `motion::ease_entrance()`（尾部约 0.9% 过冲），退场用 `ease_standard()`；
//! 与 `account_view` 一样，把外壳从「触发元素的位置」插值到「面板位置」，
//! 内容在 p > 0.55 之后才淡入，避免被拉扁的中间帧露出来。
//!
//! 数据：`yk_core::space` 一次拿资料 + 标签 + 播放/获赞 + 关注数，投稿单独一次。

use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::dynamic::{asset_url, fitted_width};
use yk_core::format;
use yk_core::space::{SpaceOrder, SpaceProfile, SpaceVideo};
use yk_ui::theme::*;

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
const IDENT_LEFT: f32 = 104.0; // 20 + 72 + 12
const GRID_COLS: usize = 4;
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
    videos_loading: bool,
    videos_error: Option<String>,
    /// 动态：5 条一页，滚到底再拉。
    dynamics: Vec<yk_core::space::SpaceDynamic>,
    /// 已取回但还没上屏的动态：接口一页给十几条，这里按 5 条一批往外放。
    dynamics_buffer: Vec<yk_core::space::SpaceDynamic>,
    dynamics_offset: String,
    dynamics_has_more: bool,
    dynamics_loading: bool,
    dynamics_error: Option<String>,
    dynamics_started: bool,
    /// 专栏投稿。
    articles: Vec<yk_core::space::SpaceArticle>,
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
    play_handler: Option<Box<dyn Fn(&mut App, &mut Window, String, crate::player::CoverSource)>>,
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
            play_handler: None,
        }
    }

    pub fn set_close_handler(&mut self, close: impl Fn(&mut App) + 'static) {
        self.close_handler = Some(Box::new(close));
    }

    pub fn set_play_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, String, crate::player::CoverSource) + 'static,
    ) {
        self.play_handler = Some(Box::new(handler));
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
        let cookie = crate::comment_section::current_session()
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
                        let client = yk_core::api::client::ReqwestClient::new()
                            .map_err(|error| error.to_string())?;
                        yk_core::space::fetch_space_card(&client, &cookie, mid)
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
                        let client = yk_core::api::client::ReqwestClient::new()
                            .map_err(|error| error.to_string())?;
                        yk_core::space::fetch_space_basic(&client, &cookie, mid)
                    }
                })
                .await;
            if let Ok(basic) = basic {
                let _ = this.update(cx, |view, cx| view.apply_top_photo(basic, cx));
            }
        })
        .detach();

        // 阶段 3：动态首页。
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
        let cookie = crate::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::space::fetch_space_dynamics(&client, &cookie, mid, &offset)
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

    /// 视频投稿：点了「视频投稿」标签才拉，一次 30 条。
    fn start_videos_fetch(&mut self, cx: &mut Context<Self>) {
        if self.videos_requested {
            return;
        }
        self.videos_requested = true;
        self.videos_loading = true;
        self.videos_error = None;
        let mid = self.mid;
        let cookie = crate::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::space::fetch_space_videos(
                        &client,
                        &cookie,
                        mid,
                        1,
                        30,
                        SpaceOrder::Newest,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.videos_loading = false;
                match page {
                    Ok(page) => {
                        view.videos = page.items;
                        view.total = page.total;
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
        let cookie = crate::comment_section::current_session()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::space::fetch_space_articles(&client, &cookie, mid, pn, 20)
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
                        view.articles_has_more = if total > 0 {
                            (view.articles.len() as u64) < total
                        } else {
                            got >= 20
                        };
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
    fn apply_card(&mut self, card: yk_core::space::SpaceCard, cx: &mut Context<Self>) {
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
    fn apply_top_photo(&mut self, basic: yk_core::space::SpaceBasic, cx: &mut Context<Self>) {
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
        let Some(session) = crate::comment_section::current_session() else {
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
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::follow_user(&client, &cookie, &csrf, mid, next)
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
        let animated = if self.closing {
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
        let joined = profile
            .map(|i| format::date_time(i.joined_at))
            .unwrap_or_else(|| "—".into());
        // 「最近更新」只能从最新一条投稿推；投稿没拉过就先不显示这一项，
        // 宁缺勿假（12.9 的约定：没有数据就整项不出现，不写「—」）。
        let updated = profile
            .and_then(|i| i.pinned.as_ref().map(|v| v.created))
            .or_else(|| self.videos.first().map(|v| v.created))
            .map(format::relative_time);

        // ---- 头图：铺满宽度，底部遮罩给白字兜底 ----
        let banner = div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .right(px(0.0))
            .h(px(BANNER_H))
            .bg(linear_gradient(
                118.0,
                linear_color_stop(rgb(0x0E3A4A), 0.0),
                linear_color_stop(rgb(0x3D2E5C), 1.0),
            ))
            .child(
                // 占位渐变 + 遮罩：接上 `profile.top_photo` 后换成 img，这一层改成压暗
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        0.0,
                        linear_color_stop(rgba(0x000000C2), 0.0),
                        linear_color_stop(rgba(0x00000000), 1.0),
                    )),
            )
            .child(
                // 文档标注：头图来自 UP 主空间设置，占位渐变只是兜底
                div()
                    .absolute()
                    .left(px(PAD_X))
                    .top(px(12.0))
                    .h(px(22.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(BADGE_RADIUS))
                    .border_1()
                    .border_color(rgba(0xFFFFFF6B))
                    .bg(rgba(0x0000006B))
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(rgba(0xFFFFFFEB))
                    .child("空间头图 top_photo · 未接入时回落品牌渐变"),
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
                    .bg(linear_gradient(
                        135.0,
                        linear_color_stop(p.bili_blue, 0.0),
                        linear_color_stop(p.bili_pink, 1.0),
                    ))
                    .when(!face_url.is_empty(), |el| {
                        el.child(
                            img(ImageSource::from(face_url.clone()))
                                .absolute()
                                .inset_0()
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    }),
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
            .top(px(94.0))
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
        let stats = div()
            .absolute()
            .left(px(IDENT_LEFT))
            .bottom(px(12.0))
            .flex()
            .items_end()
            .gap(px(SPACE_5))
            .child(stat_col(&follows, "关注", s))
            .child(stat_col(&fans, "粉丝", s))
            .child(stat_col(&likes, "获赞", s))
            .child(stat_col(&plays, "播放", s));

        let mid_text = self.mid.to_string();
        let mut facts = div()
            .absolute()
            .right(px(PAD_X))
            .bottom(px(12.0))
            .flex()
            .items_end()
            .gap(px(SPACE_5))
            .child(fact_col(&joined, "加入时间", s))
            .child(fact_col(&mid_text, "UID", s));
        if let Some(updated) = updated.as_ref() {
            facts = facts.child(fact_col(updated, "最近更新", s));
        }

        let header = div()
            .relative()
            .h(px(HEADER_H))
            .flex_shrink_0()
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
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation()))
            .child(header)
            .child(tabs)
            .child(
                div()
                    .id("up-profile-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
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
                    Tab::Videos => view.start_videos_fetch(cx),
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
        item: &yk_core::space::SpaceDynamic,
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
            body = body.child(
                div()
                    .line_clamp(4)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .line_height(px(21.0))
                    .text_color(s.text_primary)
                    .child(item.text.clone()),
            );
        }
        if !item.cover.is_empty() {
            let cover = asset_url(&item.cover, fitted_width(240.0, 2.0, 0));
            if item.kind == "video" {
                // 视频动态：直接复用首页推荐卡片的封面（播放/弹幕/时长都在），
                // 不另做一套小缩略图——同一条视频在首页和这里长得一样。
                let art =
                    crate::cover_art(&item.title, &cover, None, s.fill_3).into_any_element();
                let visual = crate::cover_visual(
                    t,
                    crate::CoverVisual {
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
                                    img(ImageSource::from(cover))
                                        .absolute()
                                        .inset_0()
                                        .w_full()
                                        .h_full()
                                        .object_fit(ObjectFit::Cover),
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

        div()
            .id(SharedString::from(format!("up-profile-dynamic-{index}")))
            .flex()
            .gap(px(SPACE_3))
            .p(px(SPACE_3))
            .rounded(px(RADIUS_LG))
            .border_b_1()
            .border_color(s.border_subtle)
            .child(
                div()
                    .w(px(36.0))
                    .h(px(36.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(linear_gradient(
                        135.0,
                        linear_color_stop(p.bili_blue, 0.0),
                        linear_color_stop(p.bili_pink, 1.0),
                    )),
            )
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
                            img(ImageSource::from(cover))
                                .absolute()
                                .inset_0()
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
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
        if max - offset > viewport + 240.0 {
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
            Tab::Videos => {}
        }
    }

    fn videos_body(&self, t: &YkTheme, scale: f32, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        if let Some(error) = self.videos_error.as_ref() {
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
        if self.videos_loading {
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
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .p(px(PAD_X))
            .child(self.grid(t, scale, cx))
            .into_any_element()
    }

    fn grid(&self, t: &YkTheme, scale: f32, cx: &mut Context<Self>) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(GRID_GAP));
        for (index, video) in self.videos.iter().take(8).enumerate() {
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
        let art = crate::cover_art(&video.title, &cover, None, s.fill_3).into_any_element();
        let thumbnail = playable_cover(
            SharedString::from(format!("up-profile-video-{index}")),
            video.bvid.clone(),
            video.title.clone(),
            cover.clone(),
            self.viewport,
            crate::cover_visual(
                t,
                crate::CoverVisual {
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
                div()
                    .h(px(FONT_SIZE_CONTROL * 2.0 + 4.0))
                    .line_clamp(2)
                    .text_size(px(FONT_SIZE_CONTROL))
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
            .bg(if face.is_empty() {
                linear_gradient(
                    135.0,
                    linear_color_stop(pal.bili_blue, 0.0),
                    linear_color_stop(pal.bili_pink, 1.0),
                )
            } else {
                linear_gradient(
                    0.0,
                    linear_color_stop(s.surface_elevated_strong, 0.0),
                    linear_color_stop(s.surface_elevated_strong, 1.0),
                )
            })
            .border_1()
            .border_color(s.border_popover)
            .shadow(theme.shadows.shadow_4.clone())
            .occlude();
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
                let source = crate::player::CoverSource {
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
