//! 已登录后的账号面板：资料、硬币/B币、最近登录、关注/粉丝/动态、退出账号。
//!
//! 资料由 `yk_core::auth::fetch_account` 在后台拉取；面板先显示会话里的
//! 昵称/头像，数据回来后再补全，避免点头像时空白等待。

use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::auth::AccountInfo;
use yk_core::models::UserSession;
use yk_ui::theme::*;

/// 账号面板动画 id 的进程内序号，避免反复开关复用旧动画状态。
static ACCOUNT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
const ACCOUNT_OPEN_MS: u64 = 280;
const ACCOUNT_CLOSE_MS: u64 = 260;
const ACCOUNT_PANEL_W: f32 = 360.0;
const ACCOUNT_PANEL_H: f32 = 400.0;

pub struct AccountView {
    theme: YkTheme,
    session: UserSession,
    info: Option<AccountInfo>,
    started: bool,
    loading: bool,
    error: Option<String>,
    closing: bool,
    closing_generation: u64,
    open_generation: u64,
    pending_logout: bool,
    pending_settings: bool,
    close_handler: Option<Box<dyn Fn(&mut App)>>,
    logout_handler: Option<Box<dyn Fn(&mut App)>>,
    settings_handler: Option<Box<dyn Fn(&mut App)>>,
    info_handler: Option<Box<dyn Fn(&mut App, AccountInfo)>>,
}

impl AccountView {
    pub fn new(
        theme: YkTheme,
        session: UserSession,
        initial: Option<AccountInfo>,
        refresh_on_open: bool,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            session,
            info: initial,
            started: !refresh_on_open,
            loading: false,
            error: None,
            closing: false,
            closing_generation: 0,
            open_generation: ACCOUNT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            pending_logout: false,
            pending_settings: false,
            close_handler: None,
            logout_handler: None,
            settings_handler: None,
            info_handler: None,
        }
    }

    pub fn set_handlers(
        &mut self,
        close: impl Fn(&mut App) + 'static,
        logout: impl Fn(&mut App) + 'static,
        settings: impl Fn(&mut App) + 'static,
        info: impl Fn(&mut App, AccountInfo) + 'static,
    ) {
        self.close_handler = Some(Box::new(close));
        self.logout_handler = Some(Box::new(logout));
        self.settings_handler = Some(Box::new(settings));
        self.info_handler = Some(Box::new(info));
    }

    fn start_fetch(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let cookie = self.session.cookie_header();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { yk_core::auth::fetch_account(&cookie) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.loading = false;
                match result {
                    Ok(info) => {
                        view.info = Some(info.clone());
                        if let Some(handler) = view.info_handler.as_ref() {
                            handler(cx, info);
                        }
                    }
                    Err(e) => {
                        if view.info.is_none() {
                            view.error = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_close(&mut self, logout_after: bool, cx: &mut Context<Self>) {
        self.begin_close_action(false, logout_after, cx);
    }

    fn begin_settings(&mut self, cx: &mut Context<Self>) {
        self.begin_close_action(true, false, cx);
    }

    fn begin_close_action(
        &mut self,
        settings_after: bool,
        logout_after: bool,
        cx: &mut Context<Self>,
    ) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.pending_logout = logout_after;
        self.pending_settings = settings_after;
        self.closing_generation = ACCOUNT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.closing_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(ACCOUNT_CLOSE_MS + 30))
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.closing || view.closing_generation != generation {
                    return;
                }
                view.closing = false;
                if view.pending_logout {
                    view.pending_logout = false;
                    if let Some(handler) = view.logout_handler.as_ref() {
                        handler(cx);
                    }
                } else if view.pending_settings {
                    view.pending_settings = false;
                    if let Some(handler) = view.settings_handler.as_ref() {
                        handler(cx);
                    }
                } else if let Some(handler) = view.close_handler.as_ref() {
                    handler(cx);
                }
            });
        })
        .detach();
    }

    fn format_amount(value: f64) -> String {
        if (value.fract()).abs() < 0.001 {
            format!("{value:.0}")
        } else {
            format!("{value:.1}")
        }
    }

    fn avatar(&self, face: &str) -> AnyElement {
        let t = &self.theme;
        let p = &t.palette;
        let mut el = div()
            .w(px(88.))
            .h(px(88.))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(p.theme_primary_60)
            .bg(linear_gradient(
                135.,
                linear_color_stop(p.bili_blue, 0.),
                linear_color_stop(p.bili_pink, 1.),
            ));
        if !face.is_empty() {
            el = el.child(
                img(ImageSource::from(face.to_string()))
                    .w(px(88.))
                    .h(px(88.))
                    .object_fit(ObjectFit::Cover),
            );
        }
        el.into_any_element()
    }

    fn stat(&self, value: String, label: &str) -> AnyElement {
        let s = &self.theme.surfaces;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(SPACE_1))
            .child(
                div()
                    .text_size(px(FONT_SIZE_DISPLAY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child(value),
            )
            .child(
                div()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_tertiary)
                    .child(label.to_string()),
            )
            .into_any_element()
    }
}

impl Render for AccountView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 不能在 render 里直接 cx.spawn：延到这一帧结束再拉资料。
        if !self.started {
            self.started = true;
            cx.defer_in(window, |view, _window, cx| view.start_fetch(cx));
        }
        let t = self.theme.clone();
        let s = &t.surfaces;
        let p = &t.palette;

        let name = self
            .info
            .as_ref()
            .map(|i| i.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.session.nickname.clone());
        let face = self
            .info
            .as_ref()
            .map(|i| i.face.clone())
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| self.session.face_url.clone());
        let coins = self
            .info
            .as_ref()
            .map(|i| Self::format_amount(i.coins))
            .unwrap_or_else(|| "—".into());
        let bcoin = self
            .info
            .as_ref()
            .map(|i| Self::format_amount(i.bcoin))
            .unwrap_or_else(|| "—".into());
        let level = self.info.as_ref().map(|i| i.level).unwrap_or(0);
        let following = self
            .info
            .as_ref()
            .map(|i| i.following.to_string())
            .unwrap_or_else(|| "—".into());
        let followers = self
            .info
            .as_ref()
            .map(|i| i.followers.to_string())
            .unwrap_or_else(|| "—".into());
        let dynamics = self
            .info
            .as_ref()
            .and_then(|i| i.dynamics)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into());
        let last_time = self
            .info
            .as_ref()
            .map(|i| i.last_login_time.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "—".into());
        let last_geo = self
            .info
            .as_ref()
            .map(|i| i.last_login_geo.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "—".into());

        let content = div()
            .id("account-panel")
            .w(px(ACCOUNT_PANEL_W))
            .h(px(ACCOUNT_PANEL_H))
            .p(px(SPACE_6))
            .flex()
            .flex_col()
            .gap(px(SPACE_4))
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation()))
            // 顶部：昵称 + 大头像（头像与卡片顶边重叠一点，贴近设计稿）
            .child(
                div()
                    .relative()
                    .h(px(96.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(SPACE_2))
                            .w(px(210.))
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_DISPLAY))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(s.text_primary)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(SPACE_5))
                                    .text_size(px(FONT_SIZE_CONTROL))
                                    .text_color(s.text_secondary)
                                    .child(format!("硬币：{coins}"))
                                    .child(format!("B币：{bcoin}")),
                            ),
                    )
                    .child(div().absolute().top_0().right_0().child(self.avatar(&face))),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(SPACE_3))
                    .child(
                        div()
                            .mt(px(2.))
                            .h(px(18.))
                            .px(px(5.))
                            .flex()
                            .items_center()
                            .rounded(px(RADIUS_SM))
                            .bg(p.status_error)
                            .text_size(px(FONT_SIZE_MICRO))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child(format!("LV{level}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_CONTROL))
                                    .text_color(s.text_secondary)
                                    .child(format!("最后登录：{last_time}")),
                            )
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_CONTROL))
                                    .text_color(s.text_tertiary)
                                    .truncate()
                                    .child(format!("登录地点：{last_geo}")),
                            ),
                    ),
            )
            .when(self.loading, |el| {
                el.child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .child("正在加载账号信息…"),
                )
            })
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(p.status_error)
                        .child(error),
                )
            })
            .child(div().h(px(1.)).w_full().bg(s.border_subtle))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(self.stat(following, "关注"))
                    .child(div().w(px(1.)).h(px(36.)).bg(s.border_subtle))
                    .child(self.stat(followers, "粉丝"))
                    .child(div().w(px(1.)).h(px(36.)).bg(s.border_subtle))
                    .child(self.stat(dynamics, "动态")),
            )
            .child(
                div()
                    .id("account-settings")
                    .h(px(38.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(s.fill_1)
                    .border_1()
                    .border_color(s.border_subtle)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(s.text_secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.begin_settings(cx);
                    }))
                    .child("设置"),
            )
            .child(
                div()
                    .id("account-logout")
                    .h(px(38.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(s.fill_1)
                    .border_1()
                    .border_color(p.status_error_30)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.status_error)
                    .cursor_pointer()
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.begin_close(true, cx);
                    }))
                    .child("退出账号"),
            );

        let width = window.viewport_size().width.as_f32();
        let source = Bounds {
            origin: point(px(width - SPACE_5 - 34.), px((TOP_BAR_HEIGHT - 34.) / 2.)),
            size: size(px(34.), px(34.)),
        };
        let target = Bounds {
            origin: point(
                px(width - SPACE_4 - ACCOUNT_PANEL_W),
                px(TOP_BAR_HEIGHT + SPACE_4),
            ),
            size: size(px(ACCOUNT_PANEL_W), px(ACCOUNT_PANEL_H)),
        };
        let popover = AccountPopover {
            theme: t.clone(),
            face,
            content: Some(content.into_any_element()),
            source,
            target,
            progress: 0.0,
        };
        let animated = if self.closing {
            popover
                .with_animation(
                    format!("account-close-{}", self.closing_generation),
                    Animation::new(std::time::Duration::from_millis(ACCOUNT_CLOSE_MS))
                        .with_easing(|t| t),
                    |el, p| AccountPopover {
                        progress: 1.0 - p,
                        ..el
                    },
                )
                .into_any_element()
        } else {
            popover
                .with_animation(
                    format!("account-open-{}", self.open_generation),
                    Animation::new(std::time::Duration::from_millis(ACCOUNT_OPEN_MS))
                        .with_easing(|t| t),
                    |el, p| AccountPopover { progress: p, ..el },
                )
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("account-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x00000001))
                    .occlude()
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.begin_close(false, cx);
                    })),
            )
            .child(animated)
    }
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

/// 从头像圆点展开成面板；关闭时反向缩回同一个头像位置。
#[derive(gpui::IntoElement)]
struct AccountPopover {
    theme: YkTheme,
    face: String,
    content: Option<AnyElement>,
    source: Bounds<Pixels>,
    target: Bounds<Pixels>,
    progress: f32,
}

impl RenderOnce for AccountPopover {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let AccountPopover {
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
        let p_theme = &theme.palette;
        let bg = if face.is_empty() {
            linear_gradient(
                135.,
                linear_color_stop(p_theme.bili_blue, 0.),
                linear_color_stop(p_theme.bili_pink, 1.),
            )
        } else {
            // 同一个字段类型：用渐变/实色都由 Background 承载。
            linear_gradient(
                0.,
                linear_color_stop(s.surface_elevated_solid, 0.),
                linear_color_stop(s.surface_elevated_solid, 1.),
            )
        };
        let mut shell = div()
            .id("account-shell")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(POPOVER_RADIUS))
            .overflow_hidden()
            .bg(bg)
            .border_1()
            .border_color(s.border_popover)
            .shadow(theme.shadows.shadow_2.clone())
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
                .child(content.expect("账号面板内容")),
        )
    }
}
