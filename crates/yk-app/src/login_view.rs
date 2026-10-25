//! 登录卡片：扫码 / 账号密码 / 手机验证码。
//!
//! UI 只负责输入与状态机；网络、Cookie 解析与 RSA 加密都在 `yk_core::auth`。
//! 登录成功通过 `success_handler` 把 `UserSession` 交回 AppRoot，由它落盘并刷新推荐流。

use gpui::component::input::{Input, InputState};
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::auth::{self, QrLogin, QrPoll};
use yk_core::models::UserSession;
use yk_ui::theme::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginTab {
    Qr,
    Password,
    Sms,
}

pub struct LoginView {
    theme: YkTheme,
    tab: LoginTab,
    username: Option<Entity<InputState>>,
    password: Option<Entity<InputState>>,
    phone: Option<Entity<InputState>>,
    sms_code: Option<Entity<InputState>>,
    qr: Option<QrLogin>,
    qr_loading: bool,
    qr_started: bool,
    qr_scanned: bool,
    qr_expired: bool,
    qr_generation: u64,
    busy: bool,
    status: String,
    status_error: bool,
    close_handler: Option<Box<dyn Fn(&mut Window, &mut App)>>,
    success_handler: Option<Box<dyn Fn(&mut App, UserSession)>>,
}

impl LoginView {
    pub fn new(theme: YkTheme, _cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            tab: LoginTab::Qr,
            username: None,
            password: None,
            phone: None,
            sms_code: None,
            qr: None,
            qr_loading: false,
            qr_started: false,
            qr_scanned: false,
            qr_expired: false,
            qr_generation: 0,
            busy: false,
            status: String::new(),
            status_error: false,
            close_handler: None,
            success_handler: None,
        }
    }

    pub fn set_handlers(
        &mut self,
        close: impl Fn(&mut Window, &mut App) + 'static,
        success: impl Fn(&mut App, UserSession) + 'static,
    ) {
        self.close_handler = Some(Box::new(close));
        self.success_handler = Some(Box::new(success));
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.username.is_some() {
            return;
        }
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("手机号 / 邮箱"));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("密码"));
        password.update(cx, |state, cx| state.set_masked(true, window, cx));
        let phone = cx.new(|cx| InputState::new(window, cx).placeholder("手机号"));
        let sms_code = cx.new(|cx| InputState::new(window, cx).placeholder("验证码"));
        self.username = Some(username);
        self.password = Some(password);
        self.phone = Some(phone);
        self.sms_code = Some(sms_code);
    }

    fn value(&self, field: Option<&Entity<InputState>>, cx: &App) -> String {
        field
            .map(|state| state.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    fn auth_error(message: String) -> String {
        if message.contains("-105") || message.contains("验证码") {
            "B 站要求完成极验安全验证，请改用扫码登录".into()
        } else {
            message
        }
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.status = text.into();
        self.status_error = error;
        cx.notify();
    }

    fn start_qr(&mut self, cx: &mut Context<Self>) {
        self.qr_generation = self.qr_generation.wrapping_add(1);
        let generation = self.qr_generation;
        self.qr = None;
        self.qr_loading = true;
        self.qr_scanned = false;
        self.qr_expired = false;
        self.status = "正在生成二维码…".into();
        self.status_error = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { auth::generate_qr() })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.qr_generation != generation {
                    return;
                }
                view.qr_loading = false;
                match result {
                    Ok(qr) => {
                        view.qr = Some(qr);
                        view.status = "请使用 B 站 App 扫码".into();
                        view.status_error = false;
                        view.start_poll(cx);
                    }
                    Err(e) => {
                        view.status = e;
                        view.status_error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        let Some(qr) = self.qr.clone() else {
            return;
        };
        let generation = self.qr_generation;
        let key = qr.key;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2000))
                .await;
            let key_for_poll = key.clone();
            let result = cx
                .background_executor()
                .spawn(async move { auth::poll_qr(&key_for_poll) })
                .await;
            let keep = this.update(cx, |view, cx| {
                if view.qr_generation != generation {
                    return false;
                }
                match result {
                    Ok(QrPoll::Waiting) => {
                        if view.tab == LoginTab::Qr && !view.qr_scanned {
                            view.status = "等待扫码…二维码 120 秒后失效".into();
                        }
                        cx.notify();
                        true
                    }
                    Ok(QrPoll::Scanned) => {
                        view.qr_scanned = true;
                        if view.tab == LoginTab::Qr {
                            view.status = "已扫码，请在手机上确认".into();
                            view.status_error = false;
                        }
                        cx.notify();
                        true
                    }
                    Ok(QrPoll::Expired) => {
                        view.qr_expired = true;
                        if view.tab == LoginTab::Qr {
                            view.status = "二维码已失效，点击刷新".into();
                            view.status_error = true;
                        }
                        cx.notify();
                        false
                    }
                    Ok(QrPoll::Success(session)) => {
                        view.finish(*session, cx);
                        false
                    }
                    Err(e) => {
                        if view.tab == LoginTab::Qr {
                            view.status = e;
                            view.status_error = true;
                        }
                        cx.notify();
                        true
                    }
                }
            });
            if !matches!(keep, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn finish(&mut self, session: UserSession, cx: &mut Context<Self>) {
        if !session.is_valid() {
            self.set_status("登录成功但会话无效，请重试", true, cx);
            return;
        }
        self.busy = false;
        self.status = "登录成功，正在刷新推荐…".into();
        self.status_error = false;
        if let Some(handler) = self.success_handler.as_ref() {
            handler(cx, session);
        }
        cx.notify();
    }

    fn submit_password(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let username = self.value(self.username.as_ref(), cx);
        let password = self.value(self.password.as_ref(), cx);
        if username.is_empty() || password.is_empty() {
            self.set_status("请输入账号和密码", true, cx);
            return;
        }
        self.busy = true;
        self.set_status("正在登录…", false, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { auth::password_login(&username, &password) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(session) => view.finish(session, cx),
                    Err(e) => {
                        view.status = Self::auth_error(e);
                        view.status_error = true;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn send_sms(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let phone = self.value(self.phone.as_ref(), cx);
        if phone.is_empty() {
            self.set_status("请输入手机号", true, cx);
            return;
        }
        self.busy = true;
        self.set_status("正在发送验证码…", false, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { auth::send_sms(&phone) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(()) => {
                        view.status = "验证码已发送，请查收短信".into();
                        view.status_error = false;
                    }
                    Err(e) => {
                        view.status = e;
                        view.status_error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_sms(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let phone = self.value(self.phone.as_ref(), cx);
        let code = self.value(self.sms_code.as_ref(), cx);
        if phone.is_empty() || code.is_empty() {
            self.set_status("请输入手机号和验证码", true, cx);
            return;
        }
        self.busy = true;
        self.set_status("正在登录…", false, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { auth::sms_login(&phone, &code) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(session) => view.finish(session, cx),
                    Err(e) => {
                        view.status = Self::auth_error(e);
                        view.status_error = true;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.qr_generation = self.qr_generation.wrapping_add(1);
        if let Some(handler) = self.close_handler.as_ref() {
            handler(window, cx);
        }
    }

    fn tab_button(&self, label: &str, tab: LoginTab, cx: &mut Context<Self>) -> AnyElement {
        let active = self.tab == tab;
        let s = &self.theme.surfaces;
        let p = &self.theme.palette;
        div()
            .id(label.to_string())
            .h(px(34.))
            .px(px(SPACE_4))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_size(px(FONT_SIZE_CONTROL))
            .font_weight(if active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .text_color(if active {
                rgb(0xFFFFFF).into()
            } else {
                s.text_tertiary
            })
            .when(active, |el| el.bg(p.theme_primary))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.tab = tab;
                this.status.clear();
                this.status_error = false;
                cx.notify();
            }))
            .child(label.to_string())
            .into_any_element()
    }

    fn field(&self, state: &Entity<InputState>, cx: &mut Context<Self>) -> AnyElement {
        let handle = state.read(cx).focus_handle(cx);
        let s = &self.theme.surfaces;
        div()
            .h(px(42.))
            .w_full()
            .px(px(SPACE_3))
            .flex()
            .items_center()
            .rounded(px(INTERACTIVE_RADIUS))
            .bg(s.fill_1)
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
                Input::new(state)
                    .appearance(false)
                    .bordered(false)
                    .w_full()
                    .h_full(),
            )
            .into_any_element()
    }

    fn primary_button(
        &self,
        id: &'static str,
        label: &str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut LoginView, &mut Window, &mut Context<LoginView>) + 'static,
    ) -> AnyElement {
        let p = &self.theme.palette;
        div()
            .id(id)
            .h(px(42.))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(if self.busy {
                p.theme_primary_60
            } else {
                p.theme_primary
            })
            .text_size(px(FONT_SIZE_CONTROL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0xFFFFFF))
            .cursor_pointer()
            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                if view.busy {
                    return;
                }
                on_click(view, window, cx);
            }))
            .child(label.to_string())
            .into_any_element()
    }

    fn qr_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.theme.surfaces;
        let mut qr_box = div()
            .w(px(220.))
            .h(px(220.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_MD))
            // 二维码底必须纯白，不能跟暗色主题走。
            .bg(rgb(0xFFFFFF));
        if self.qr_loading {
            qr_box = qr_box.child(
                div()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(rgb(0x666666))
                    .child("正在生成二维码…"),
            );
        } else if let Some(qr) = self.qr.as_ref() {
            qr_box = qr_box.child(
                img(ImageSource::from(auth::qr_image_url(&qr.url)))
                    .w(px(200.))
                    .h(px(200.))
                    .object_fit(ObjectFit::Contain),
            );
        } else {
            qr_box = qr_box.child(
                div()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(rgb(0x666666))
                    .child("二维码未生成"),
            );
        }
        let qr_box = div()
            .relative()
            .child(qr_box)
            .when(self.qr_scanned, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_MD))
                        .bg(rgba(0x000000AA))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(rgb(0xFFFFFF))
                        .child("请在手机上确认"),
                )
            })
            .when(self.qr_expired, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_MD))
                        .bg(rgba(0x000000CC))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(rgb(0xFFFFFF))
                        .child("二维码已失效"),
                )
            });

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(SPACE_4))
            .child(qr_box)
            .child(
                div()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(if self.status_error {
                        self.theme.palette.status_error
                    } else {
                        s.text_tertiary
                    })
                    .text_center()
                    .child(if self.status.is_empty() {
                        "请使用 B 站 App 扫码".to_string()
                    } else {
                        self.status.clone()
                    }),
            )
            .when(
                self.qr_expired || (self.qr.is_none() && !self.qr_loading),
                |el| {
                    el.child(
                        div()
                            .id("login-qr-refresh")
                            .h(px(34.))
                            .px(px(SPACE_4))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(s.fill_2)
                            .text_size(px(FONT_SIZE_CONTROL))
                            .text_color(s.text_primary)
                            .cursor_pointer()
                            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                                view.start_qr(cx);
                            }))
                            .child("刷新二维码"),
                    )
                },
            )
            .into_any_element()
    }
}

impl Render for LoginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        if !self.qr_started && self.tab == LoginTab::Qr {
            self.qr_started = true;
            cx.defer_in(window, |view, _window, cx| view.start_qr(cx));
        }

        let t = self.theme.clone();
        let s = &t.surfaces;
        let title = match self.tab {
            LoginTab::Qr => "扫码登录 YKPili",
            LoginTab::Password => "账号密码登录",
            LoginTab::Sms => "手机验证码登录",
        };
        let subtitle = match self.tab {
            LoginTab::Qr => "登录后同步你的推荐、收藏与观看进度",
            LoginTab::Password => "使用 B 站账号登录，验证通过后同步推荐",
            LoginTab::Sms => "使用绑定手机号接收验证码登录",
        };

        let body: AnyElement = match self.tab {
            LoginTab::Qr => self.qr_card(cx),
            LoginTab::Password => {
                let user = self.username.as_ref().expect("inputs");
                let pass = self.password.as_ref().expect("inputs");
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_3))
                    .child(self.field(user, cx))
                    .child(self.field(pass, cx))
                    .child(self.primary_button(
                        "login-password-submit",
                        "登录",
                        cx,
                        |view, _, cx| {
                            view.submit_password(cx);
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .text_color(if self.status_error {
                                self.theme.palette.status_error
                            } else {
                                s.text_tertiary
                            })
                            .text_center()
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(s.text_quaternary)
                            .text_center()
                            .child("若 B 站要求安全验证，请改用扫码登录"),
                    )
                    .into_any_element()
            }
            LoginTab::Sms => {
                let phone = self.phone.as_ref().expect("inputs");
                let code = self.sms_code.as_ref().expect("inputs");
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_3))
                    .child(
                        div()
                            .flex()
                            .gap(px(SPACE_2))
                            .child(div().flex_1().child(self.field(phone, cx)))
                            .child(
                                div()
                                    .id("login-sms-send")
                                    .h(px(42.))
                                    .px(px(SPACE_3))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(INTERACTIVE_RADIUS))
                                    .bg(s.fill_2)
                                    .text_size(px(FONT_SIZE_MICRO))
                                    .text_color(s.text_primary)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                                        view.send_sms(cx)
                                    }))
                                    .child("发送验证码"),
                            ),
                    )
                    .child(self.field(code, cx))
                    .child(
                        self.primary_button("login-sms-submit", "登录", cx, |view, _, cx| {
                            view.submit_sms(cx);
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .text_color(if self.status_error {
                                self.theme.palette.status_error
                            } else {
                                s.text_tertiary
                            })
                            .text_center()
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(s.text_quaternary)
                            .text_center()
                            .child("若 B 站要求安全验证，请改用扫码登录"),
                    )
                    .into_any_element()
            }
        };

        let card = div()
            .id("login-card")
            .w(px(380.))
            .px(px(36.))
            .py(px(SPACE_8))
            .flex()
            .flex_col()
            .gap(px(SPACE_4))
            .rounded(px(MODAL_RADIUS))
            .bg(s.surface_elevated_strong)
            .border_1()
            .border_color(s.border_popover)
            .shadow(t.shadows.shadow_glass_modal.clone())
            .occlude()
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_1))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_HEADING))
                            .line_height(px(LINE_HEIGHT_HEADING))
                            .font_weight(FontWeight::BOLD)
                            .text_color(s.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_tertiary)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(SPACE_1))
                    .child(self.tab_button("扫码", LoginTab::Qr, cx))
                    .child(self.tab_button("密码", LoginTab::Password, cx))
                    .child(self.tab_button("验证码", LoginTab::Sms, cx)),
            )
            .child(body)
            .child(
                div()
                    .id("login-cancel")
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
                    .text_color(s.text_secondary)
                    .cursor_pointer()
                    .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                        view.request_close(window, cx);
                    }))
                    .child("取消登录"),
            );

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .occlude()
            .child(card)
    }
}
