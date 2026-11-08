//! 消息页的纯渲染辅助：未读角标、内联错误、骨架列表。
//!
//! 只吃主题与基础数据，不接触 `AppRoot`。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::assets::IconName;
use gpui::component::input::Textarea;
use gpui::component::v_virtual_list;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_infra::api::client::now_wts;
use yk_infra::messages::{
    chat_time_label, needs_time_divider, ChatMessage, Conversation, MessageChannel, MessageEntry,
    DM_MAX_BYTES,
};
use yk_components::status;
use yk_theme::theme::*;

use crate::image::ImageResidency;
use crate::messages::{
    ChatState, MessageRow, MessageState, MessagesHost, RemoveConfirm, BUBBLE_MAX_W, COMPOSER_H, CONTENT_MAX_W,
    DM_ROW_H, FOOTER_H, INBOX_ROW_H, LIKE_DETAIL_H, UNREAD_DIVIDER_H,
};

pub fn count_badge(t: &YkTheme, count: u64, active: bool) -> AnyElement {
    let p = &t.palette;
    let text = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    div()
        .h(px(16.))
        .min_w(px(16.))
        .px(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(if active {
            p.theme_primary
        } else {
            p.status_error
        })
        .text_size(px(FONT_SIZE_MICRO))
        .line_height(px(16.))
        .text_color(rgb(0xFFFFFF))
        .child(text)
        .into_any_element()
}

pub fn inline_error(t: &YkTheme, error: &str) -> AnyElement {
    let p = &t.palette;
    div()
        .w_full()
        .mt(px(SPACE_2))
        .px(px(SPACE_3))
        .py(px(SPACE_2))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .rounded(px(RADIUS_MD))
        .bg(p.status_error_10)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(p.status_error)
        .child(div().flex_1().child(error.to_string()))
        .into_any_element()
    // 调用方会在下次刷新 / 操作时清掉错误；不放一个点了没反应的 ×。
}

pub fn skeleton_list(t: &YkTheme, rows: usize, height: f32) -> AnyElement {
    let brush = t.surfaces.skeleton_brush;
    let border = t.surfaces.border_subtle;
    div()
        .flex()
        .flex_col()
        .w_full()
        .with_animation(
            "msg-skeleton-pulse",
            Animation::new(Duration::from_millis(yk_components::status::SKELETON_PULSE_MS)).repeat(),
            move |el, delta| {
                let phase = if delta < 0.5 {
                    delta * 2.0
                } else {
                    (1.0 - delta) * 2.0
                };
                el.opacity(1.0 - 0.55 * phase)
                    .children((0..rows).map(move |index| {
                        div()
                            .h(px(height))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(SPACE_3))
                            .border_b_1()
                            .border_color(border)
                            .child(
                                div()
                                    .w(px(40.))
                                    .h(px(40.))
                                    .rounded_full()
                                    .bg(brush)
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(SPACE_2))
                                    .child(
                                        div()
                                            .h(px(12.))
                                            .w(px(120. + index as f32 * 18.))
                                            .rounded(px(RADIUS_SM))
                                            .bg(brush),
                                    )
                                    .child(
                                        div()
                                            .h(px(10.))
                                            .w(px(260.))
                                            .rounded(px(RADIUS_SM))
                                            .bg(brush),
                                    )
                                    .child(
                                        div()
                                            .h(px(10.))
                                            .w(px(180.))
                                            .rounded(px(RADIUS_SM))
                                            .bg(brush),
                                    ),
                            )
                    }))
            },
        )
        .into_any_element()
}

pub fn header<V: MessagesHost + 'static>(t: &YkTheme, state: &MessageState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(SPACE_4))
        .w_full()
        .h(px(52.))
        .flex_shrink_0()
        .child(
            div()
                .text_size(px(FONT_SIZE_HEADING))
                .line_height(px(LINE_HEIGHT_HEADING))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .child("消息中心"),
        );

    // 通道切换（回复 / @ / 赞 / 私信）已经搬到 Dock 的二级分页：
    // 页头只留筛选与批量动作，避免同一组胶囊在页内和 Dock 各画一份。
    row =
        row
            .child(div().flex_1())
            .children((state.channel != MessageChannel::Dm).then(|| {
                div()
                    .id("msg-only-unread")
                    .h(px(30.))
                    .px(px(SPACE_3))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_1))
                    .rounded_full()
                    .bg(if state.only_unread {
                        p.theme_primary_20
                    } else {
                        s.fill_1
                    })
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(if state.only_unread {
                        p.theme_primary
                    } else {
                        s.text_secondary
                    })
                    .cursor_pointer()
                    .hover(move |style| style.bg(s.fill_2))
                    .child(Icon::new(IconName::Funnel).size(px(14.)).text_color(
                        if state.only_unread {
                            p.theme_primary
                        } else {
                            s.text_secondary
                        },
                    ))
                    .child("只看未读")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.messages_toggle_only_unread(cx);
                        window.refresh();
                    }))
            }))
            .children((state.channel == MessageChannel::Dm).then(|| {
                div()
                    .id("msg-only-mutual")
                    .h(px(30.))
                    .px(px(SPACE_3))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_1))
                    .rounded_full()
                    .bg(if state.only_mutual {
                        p.theme_primary_20
                    } else {
                        s.fill_1
                    })
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(if state.only_mutual {
                        p.theme_primary
                    } else {
                        s.text_secondary
                    })
                    .cursor_pointer()
                    .hover(move |style| style.bg(s.fill_2))
                    .child(Icon::new(IconName::Users).size(px(14.)).text_color(
                        if state.only_mutual {
                            p.theme_primary
                        } else {
                            s.text_secondary
                        },
                    ))
                    .child("只看互关")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.messages_toggle_only_mutual(cx);
                        window.refresh();
                    }))
            }));
    // 私信通道额外的「全部已读」：只有私信有接口（update_ack），
    // 互动通道没有可清零的接口，不放假按钮。
    if state.channel == MessageChannel::Dm && !state.dm.conversations.is_empty() {
        row = row.child(
            div()
                .id("msg-read-all")
                .h(px(30.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded_full()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_secondary)
                .cursor_pointer()
                .hover(move |style| style.bg(s.fill_2).text_color(s.text_primary))
                .child("全部已读")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.messages_mark_all_dm_read(cx);
                    window.refresh();
                })),
        );
    }

    row.into_any_element()
}

pub fn error_state<V: MessagesHost + 'static>(t: &YkTheme, title: &str, error: &str, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex_1()
        .min_h(px(240.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .child(
            div()
                .text_size(px(28.))
                .text_color(p.status_error)
                .child("!"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(error.to_string()),
        )
        .child(
            div()
                .id("msg-retry")
                .mt(px(SPACE_2))
                .h(px(CONTROL_HEIGHT))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child("重试")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.messages_refresh(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

pub fn empty_state<V: MessagesHost + 'static>(t: &YkTheme, channel: MessageChannel, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut col = div()
        .flex_1()
        .min_h(px(280.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .child(
            div()
                .text_size(px(30.))
                .text_color(s.text_quaternary)
                .child(if channel == MessageChannel::Dm {
                    "✉"
                } else {
                    "○"
                }),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child(channel.empty_title()),
        )
        .child(
            div()
                .max_w(px(460.))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(channel.empty_desc()),
        );
    if channel == MessageChannel::Dm {
        // 私信空态写清楚「为什么陌生人不在这里」，并给一个可执行出口。
        col = col.child(
            div()
                .id("msg-go-following")
                .mt(px(SPACE_2))
                .h(px(CONTROL_HEIGHT))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(p.theme_primary_40)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(p.theme_primary)
                .cursor_pointer()
                .hover(|style| style.bg(p.theme_primary_10))
                .child("查看我的关注")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.messages_go_following(cx);
                    window.refresh();
                    cx.notify();
                })),
        );
    }
    col.into_any_element()
}

/// 行宽：虚拟列表每一行的实际宽度。取内容列宽（窗口不足时用可用宽）。
pub fn row_width(window: &Window) -> f32 {
    // 页面内容列最大 960，左右各 24 内边距；行宽必须等于内宽，
    // 不能用 960（会超出内容列 48）。
    (window.viewport_size().width.as_f32() - SPACE_6 * 2.0).clamp(320.0, CONTENT_MAX_W)
}

pub fn divider_row(t: &YkTheme) -> AnyElement {
    let s = &t.surfaces;
    div()
        .h(px(UNREAD_DIVIDER_H))
        .w_full()
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .child(div().flex_1().h(px(1.)).bg(s.border_subtle))
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child("以上为已读"),
        )
        .child(div().flex_1().h(px(1.)).bg(s.border_subtle))
        .into_any_element()
}

pub fn like_more_row(t: &YkTheme, remaining: u64) -> AnyElement {
    let s = &t.surfaces;
    div()
        .h(px(LIKE_DETAIL_H))
        .w_full()
        .pl(px(68.))
        .pr(px(SPACE_4))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(s.border_subtle)
        .bg(s.fill_1)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_quaternary)
        .child(format!("还有 {remaining} 人赞了这条内容"))
        .into_any_element()
}

pub fn like_detail_row(t: &YkTheme, nickname: String, face: String) -> AnyElement {
    let s = &t.surfaces;
    let initial = nickname.chars().next().unwrap_or('?').to_string();
    div()
        .h(px(LIKE_DETAIL_H))
        .w_full()
        .pl(px(68.))
        .pr(px(SPACE_4))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .border_b_1()
        .border_color(s.border_subtle)
        .bg(s.fill_1)
        .child(avatar(t, &face, &initial, 24.0))
        .child(
            div()
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_secondary)
                .child(nickname),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child("赞了这条内容"),
        )
        .into_any_element()
}

pub fn avatar(t: &YkTheme, url: &str, initial: &str, size: f32) -> AnyElement {
    yk_components::avatar::Avatar::new(size)
        .url(url)
        .initial(initial)
        .background(t.surfaces.fill_2.into())
        .text_color(t.surfaces.text_tertiary)
        .build()
}

pub fn icon_button(t: &YkTheme, id: impl Into<ElementId>, icon: IconName) -> Stateful<Div> {
    let s = &t.surfaces;
    let hover = s.fill_2;
    div()
        .id(id)
        .w(px(26.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(Icon::new(icon).size(px(14.)).text_color(s.text_secondary))
}

pub fn face_thumb(url: &str) -> String {
    if url.trim().is_empty() {
        return String::new();
    }
    format!("{}@64w_64h_1c.webp", url.trim())
}

pub fn chat_message_row(t: &YkTheme, message: &ChatMessage) -> AnyElement {
    use yk_infra::messages::ChatKind;
    match message.kind {
        ChatKind::System | ChatKind::Revoked => div()
            .self_center()
            .max_w(px(460.))
            .px(px(SPACE_3))
            .py(px(3.))
            .rounded_full()
            .bg(t.surfaces.fill_1)
            .text_size(px(FONT_SIZE_CAPTION))
            .text_color(t.surfaces.text_quaternary)
            .child(message.text.clone())
            .into_any_element(),
        ChatKind::Image | ChatKind::Emoji | ChatKind::Video => {
            bubble(t, &message.text, message.mine, false).into_any_element()
        }
        _ => bubble(t, &message.text, message.mine, false).into_any_element(),
    }
}

pub fn bubble(t: &YkTheme, text: &str, mine: bool, pending: bool) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut el = div()
        .max_w(px(BUBBLE_MAX_W))
        .px(px(SPACE_3))
        .py(px(SPACE_2))
        .rounded(px(RADIUS_LG))
        .text_size(px(FONT_SIZE_CONTROL))
        .line_height(px(LINE_HEIGHT_BODY))
        .text_color(s.text_primary)
        .bg(if mine { p.theme_primary_20 } else { s.fill_2 });
    el = if mine { el.self_end() } else { el.self_start() };
    if pending {
        el = el.opacity(0.7);
    }
    el.child(text.to_string()).into_any_element()
}

pub fn initial(nickname: &str) -> String {
    yk_components::avatar::initial_of(nickname)
}

/// 消息页的轻提示（复制成功 / 已读未同步 / 动态未接入）。
pub fn notice(t: &YkTheme, text: &str) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(DOCK_RESERVED + SPACE_4))
        .flex()
        .justify_center()
        .child(status::toast_pill(t, text))
        .into_any_element()
}

pub fn friendly_error(error: &str) -> String {
    if error.contains("-101") {
        "登录态已失效，请重新登录".into()
    } else if error.contains("-352") {
        "请求被风控拦截，请重新登录后再试".into()
    } else if error.contains("没有 data") {
        format!("{error}（接口返回结构可能已变）")
    } else {
        error.to_string()
    }
}

pub fn chat_page<V: MessagesHost + 'static>(
    t: &YkTheme,
    state: &MessageState,
    chat: &ChatState,
    now: u64,
    _window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut col =
        div()
            .id("msg-chat-page")
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            // 底部 Dock 是浮层：输入区必须停在它上方，不能钻到 Dock 背后。
            .pb(px(DOCK_RESERVED))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.messages_close_conversation(cx);
                    window.refresh();
                    cx.stop_propagation();
                }
            }))
            .child(
                // 会话头：返回 + 头像 32 + 昵称 + 关系标 + 「更多」。
                div()
                    .h(px(56.))
                    .px(px(SPACE_4))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .border_b_1()
                    .border_color(s.border_subtle)
                    .flex_shrink_0()
                    .child(
                        icon_button(t, "msg-chat-back", IconName::ArrowLeft).on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.messages_close_conversation(cx);
                                window.refresh();
                            },
                        )),
                    )
                    .child(avatar(
                        t,
                        &face_thumb(&chat.avatar),
                        &initial(&chat.nickname),
                        32.0,
                    ))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(s.text_primary)
                            .child(chat.nickname.clone()),
                    )
                    .children(chat.mutual.map(|mutual| {
                        div()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(RADIUS_SM))
                            .bg(if mutual { p.theme_primary_10 } else { s.fill_1 })
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(if mutual {
                                p.theme_primary
                            } else {
                                s.text_quaternary
                            })
                            .child(if mutual { "互关" } else { "已解除互关" })
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("msg-chat-read")
                            .h(px(30.))
                            .px(px(SPACE_3))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_secondary)
                            .cursor_pointer()
                            .hover(move |style| style.bg(s.fill_2))
                            .child("标为已读")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.messages_mark_chat_read(cx);
                                window.refresh();
                            })),
                    )
                    .child(icon_button(t, "msg-chat-remove", IconName::Trash).on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.messages_ask_remove_chat(cx);
                            window.refresh();
                        }),
                    )),
            );

    if !chat.can_send() {
        col = col.child(
            div()
                .flex_shrink_0()
                .mx(px(SPACE_6))
                .mt(px(SPACE_3))
                .px(px(SPACE_3))
                .py(px(SPACE_2))
                .rounded(px(RADIUS_MD))
                .bg(p.status_warning_10)
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(p.status_warning)
                .child(format!(
                    "你与「{}」已不是互相关注好友，无法继续发送私信 · 历史消息仍可查看",
                    chat.nickname
                )),
        );
    }
    if let Some(error) = chat.error.as_deref() {
        col = col.child(
            div()
                .mx(px(SPACE_6))
                .mt(px(SPACE_2))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(p.status_error)
                .child(friendly_error(error)),
        );
    }

    // 消息区：自己滚，不跟页面走。
    let mut body = div()
        .id("msg-chat-body")
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .overflow_y_scroll()
        .track_scroll(&chat.scroll)
        .px(px(SPACE_6))
        .py(px(SPACE_4))
        .flex()
        .flex_col()
        .gap(px(SPACE_3));

    if chat.loading && chat.messages.is_empty() {
        body = body.child(skeleton_list(t, 4, 54.0));
    } else if chat.messages.is_empty() {
        body = body.child(
            div()
                .flex_1()
                .min_h(px(200.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("还没有消息，打个招呼吧"),
        );
    } else {
        if chat.has_more {
            body = body.child(
                div()
                    .id("msg-chat-older")
                    .h(px(28.))
                    .self_center()
                    .px(px(SPACE_3))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(p.theme_primary)
                    .cursor_pointer()
                    .hover(|style| style.bg(p.theme_primary_10))
                    .child(if chat.loading_more {
                        "加载中…"
                    } else {
                        "加载更早的消息"
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.messages_load_older_chat(cx);
                        window.refresh();
                    })),
            );
        }
        let mut previous_ts: Option<u64> = None;
        for message in &chat.messages {
            if needs_time_divider(previous_ts, message.timestamp) {
                body = body.child(
                    div()
                        .self_center()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .child(chat_time_label(message.timestamp, now)),
                );
            }
            previous_ts = Some(message.timestamp);
            body = body.child(chat_message_row(t, message));
        }
    }

    if let Some(pending) = chat.pending.as_deref() {
        body = body.child(bubble(t, pending, true, true));
    }
    if let Some(failed) = chat.failed.as_deref() {
        let retry = failed.to_string();
        body = body.child(
            div()
                .id("msg-chat-retry")
                .self_end()
                .flex()
                .items_end()
                .gap(px(SPACE_1))
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded_full()
                        .bg(p.status_error)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child("!"),
                )
                .child(bubble(t, failed, true, false))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.messages_resend_failed(retry.clone(), window, cx);
                })),
        );
    }

    col = col.child(body);
    col.child(composer(t, state, chat, cx)).into_any_element()
}

pub fn composer<V: MessagesHost + 'static>(
    t: &YkTheme,
    state: &MessageState,
    chat: &ChatState,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let enabled = chat.can_send() && !chat.sending;
    let draft = state.composer_draft.clone();
    let too_long = draft.len() > DM_MAX_BYTES;
    let can_send = enabled && !draft.trim().is_empty() && !too_long;

    let mut bar = div()
        .h(px(COMPOSER_H))
        .flex_shrink_0()
        .px(px(SPACE_6))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .border_t_1()
        .border_color(s.border_subtle);

    if let Some(input) = state.composer.as_ref() {
        bar = bar.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .h(px(36.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded_full()
                .bg(s.fill_2)
                .when(!enabled, |el| el.opacity(0.5))
                .child(
                    Textarea::new(input)
                        .appearance(false)
                        .bordered(false)
                        .disabled(!enabled)
                        .h(px(34.)),
                ),
        );
    } else {
        bar = bar.child(
            div()
                .flex_1()
                .h(px(36.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded_full()
                .bg(s.fill_2)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_quaternary)
                .child("发消息…"),
        );
    }

    if too_long {
        bar = bar.child(
            div()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(p.status_error)
                .child(format!("{} / {} 字节", draft.len(), DM_MAX_BYTES)),
        );
    }

    bar.child(
        div()
            .id("msg-chat-send")
            .h(px(CONTROL_HEIGHT))
            .px(px(SPACE_4))
            .flex()
            .items_center()
            .rounded_full()
            .bg(if can_send { p.theme_primary } else { s.fill_3 })
            .text_size(px(FONT_SIZE_CONTROL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if can_send {
                rgb(0xFFFFFF).into()
            } else {
                s.text_quaternary
            })
            .when(can_send, |el| {
                el.cursor_pointer().hover(|style| style.opacity(0.9))
            })
            .child(if chat.sending {
                "发送中…"
            } else {
                "发送"
            })
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.messages_send_chat(window, cx);
            })),
    )
    .into_any_element()
}

pub fn dm_body<V: MessagesHost + 'static>(
    t: &YkTheme,
    state: &MessageState,
    now: u64,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let dm = &state.dm;
    if dm.loading && dm.conversations.is_empty() {
        return skeleton_list(t, 5, DM_ROW_H);
    }
    if let Some(error) = dm.error.as_deref() {
        if dm.conversations.is_empty() {
            return error_state(
                t,
                MessageChannel::Dm.error_title(),
                &friendly_error(error),
                cx,
            );
        }
    }
    let visible = state.visible_conversation_indices();
    if visible.is_empty() {
        if state.only_mutual && !dm.conversations.is_empty() {
            return dm_mutual_empty_state(t, cx);
        }
        return empty_state(t, MessageChannel::Dm, cx);
    }

    let rows = Rc::new(state.dm_rows());
    let sizes = Rc::new(
        rows.iter()
            .map(|row| size(px(row_width(window)), px(row.height())))
            .collect::<Vec<_>>(),
    );
    let rows_for_render = rows.clone();
    let theme = t.clone();
    let selected = state.selected;
    let entity = cx.entity();
    let list = v_virtual_list(
        entity,
        "messages-dm",
        sizes,
        move |this, range, _window, cx| {
            range
                .filter_map(|index| {
                    rows_for_render.get(index).copied().map(|row| match row {
                        MessageRow::Conversation(index) => {
                            let Some(conversation) =
                                this.messages_mut().dm.conversations.get(index).cloned()
                            else {
                                return div().into_any_element();
                            };
                            conversation_row(
                                &theme,
                                &conversation,
                                now,
                                selected == Some(conversation.talker_id),
                                cx,
                            )
                        }
                        MessageRow::Footer => footer_row(
                            &theme,
                            this.messages_mut().dm.loading_more,
                            this.messages_mut().dm.has_more,
                            cx,
                        ),
                        _ => div().into_any_element(),
                    })
                })
                .collect::<Vec<_>>()
        },
    );
    let list = list.track_scroll(&state.dm_scroll);
    list.into_any_element()
}

/// 「只看互关」筛出空结果时的空态：说明筛选原因，并给一键退出。
pub fn dm_mutual_empty_state<V: MessagesHost + 'static>(t: &YkTheme, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex_1()
        .min_h(px(280.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .child(
            div()
                .text_size(px(30.))
                .text_color(s.text_quaternary)
                .child("◎"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("没有互相关注好友的会话"),
        )
        .child(
            div()
                .max_w(px(460.))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("关掉「只看互关」可以查看已解除互关的历史会话"),
        )
        .child(
            div()
                .id("msg-show-all-dm")
                .mt(px(SPACE_2))
                .h(px(CONTROL_HEIGHT))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(p.theme_primary_40)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(p.theme_primary)
                .cursor_pointer()
                .hover(|style| style.bg(p.theme_primary_10))
                .child("显示全部会话")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.messages_toggle_only_mutual(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

pub fn conversation_row<V: MessagesHost + 'static>(
    t: &YkTheme,
    conversation: &Conversation,
    now: u64,
    selected: bool,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let id = conversation.talker_id;
    let hover_bg = s.fill_1;
    let base_bg = if selected {
        p.theme_primary_10
    } else {
        hsla(0., 0., 0., 0.)
    };
    let unread = conversation.unread > 0;
    let face = conversation.face_thumb_url();
    let initial = conversation.initial();
    let time = conversation.time_label(now);
    let preview = conversation.preview();
    let relation = conversation.relation_label();

    div()
        .id(("msg-dm-row", id as u64))
        .group("dm-row")
        .relative()
        .h(px(DM_ROW_H))
        .w_full()
        .px(px(SPACE_2))
        .flex()
        .items_center()
        .gap(px(SPACE_3))
        .border_b_1()
        .border_color(s.border_subtle)
        .bg(base_bg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover_bg))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.messages_open_conversation(id, window, cx);
            window.refresh();
        }))
        .child(V::card_up_avatar(
            SharedString::from(format!("msg-dm-up-{}", id as u64)),
            conversation.talker_id,
            conversation.nickname.clone(),
            conversation.avatar.clone(),
            40.0,
            avatar(t, &face, &initial, 40.0).into_any_element(),
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_2))
                        .child(
                            div()
                                .min_w(px(0.))
                                .line_clamp(1)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .font_weight(if unread {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(s.text_primary)
                                .child(conversation.nickname.clone()),
                        )
                        .children(relation.map(|label| {
                            let mutual = label == "互关";
                            div()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(RADIUS_SM))
                                .bg(if mutual { p.theme_primary_10 } else { s.fill_1 })
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(if mutual {
                                    p.theme_primary
                                } else {
                                    s.text_quaternary
                                })
                                .child(label)
                        })),
                )
                .child(
                    div()
                        .line_clamp(1)
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(preview),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(SPACE_1))
                .flex_shrink_0()
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(time),
                )
                .children((conversation.unread > 0).then(|| {
                    let count = conversation.unread;
                    div()
                        .h(px(16.))
                        .min_w(px(16.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(p.status_error)
                        .text_size(px(FONT_SIZE_MICRO))
                        .line_height(px(16.))
                        .text_color(rgb(0xFFFFFF))
                        .child(if count > 99 {
                            "99+".to_string()
                        } else {
                            count.to_string()
                        })
                })),
        )
        .child(
            div()
                .absolute()
                .right(px(SPACE_2))
                .top(px(SPACE_1))
                .flex()
                .items_center()
                .gap(px(2.))
                .opacity(0.)
                .group_hover("dm-row", |style| style.opacity(1.0))
                .occlude()
                .px(px(2.))
                .rounded(px(RADIUS_SM))
                .bg(s.surface_elevated_strong)
                .child(
                    icon_button(t, ("msg-dm-read", id as u64), IconName::Check).on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.messages_mark_conversation_read(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        }),
                    ),
                )
                .child(
                    icon_button(t, ("msg-dm-remove", id as u64), IconName::Trash).on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.messages_ask_remove_conversation(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        }),
                    ),
                ),
        )
        .into_any_element()
}

pub fn footer_row<V: MessagesHost + 'static>(
    t: &YkTheme,
    loading_more: bool,
    has_more: bool,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let text = if loading_more {
        "加载中…"
    } else if has_more {
        "加载更早的消息"
    } else {
        "没有更多了"
    };
    let mut row = div()
        .id("msg-footer")
        .h(px(FOOTER_H))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_quaternary);
    if has_more && !loading_more {
        row = row
            .cursor_pointer()
            .hover(move |style| style.text_color(s.text_secondary))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.messages_load_more(cx);
                window.refresh();
            }));
    }
    row.child(text).into_any_element()
}

/// 渲染消息页。`window` 用来取视口宽、创建输入框焦点。
pub fn page<V: MessagesHost + 'static>(
    t: &YkTheme,
    state: &MessageState,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    // 头像走页面图片缓存：只把当前实际会画的头像登记为驻留，
    // 否则下一帧 `prune_page_images` 会把它们 drop 掉（历史页踩过同一个坑）。
    {
        let mut residency = active_images.borrow_mut();
        for entry in state.entries() {
            let face = entry.user.face_thumb_url();
            if !face.is_empty() {
                residency.mark(&face);
            }
            for liker in &entry.likers {
                let face = liker.face_thumb_url();
                if !face.is_empty() {
                    residency.mark(&face);
                }
            }
        }
        for conversation in &state.dm.conversations {
            let face = conversation.face_thumb_url();
            if !face.is_empty() {
                residency.mark(&face);
            }
        }
        if let Some(chat) = state.chat() {
            let face = face_thumb(&chat.avatar);
            if !face.is_empty() {
                residency.mark(&face);
            }
        }
    }

    let now = now_wts();
    let content = if let Some(chat) = state.chat() {
        chat_page(t, state, chat, now, window, cx)
    } else {
        inbox_page(t, state, now, window, cx)
    };

    div()
        .size_full()
        .flex()
        .justify_center()
        .bg(t.surfaces.bg_base)
        .child(
            div()
                .size_full()
                // 960 是内容列（主列表）宽，外面还有左右各 24 内边距。
                .max_w(px(CONTENT_MAX_W + SPACE_6 * 2.0))
                .flex()
                .flex_col()
                .child(content),
        )
        .into_any_element()
}

pub fn inbox_page<V: MessagesHost + 'static>(
    t: &YkTheme,
    state: &MessageState,
    now: u64,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let mut col = div()
        .flex()
        .flex_col()
        .size_full()
        .min_h(px(0.))
        .px(px(SPACE_6))
        .pt(px(SPACE_4))
        .pb(px(DOCK_RESERVED))
        .child(header(t, state, cx));

    if let Some(error) = state.action_error.as_deref() {
        col = col.child(inline_error(t, error));
    }
    // 刷新失败但手里还有旧内容：顶部条保留内容可读，不做整页错误态。
    let retained_error = match state.channel {
        MessageChannel::Dm => state
            .dm
            .error
            .as_deref()
            .filter(|_| !state.dm.conversations.is_empty()),
        _ => state
            .inbox()
            .error
            .as_deref()
            .filter(|_| !state.inbox().entries.is_empty()),
    };
    if let Some(error) = retained_error {
        col = col.child(inline_error(t, &friendly_error(error)));
    }

    let body = match state.channel {
        MessageChannel::Dm => dm_body(t, state, now, window, cx),
        channel => inbox_body(t, state, channel, window, cx),
    };
    let mut list = div()
        .id("msg-list-focus")
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .child(body);
    if let Some(focus) = state.focus.clone() {
        list = list.track_focus(&focus).on_key_down(cx.listener(
            |this, event: &KeyDownEvent, window, cx| {
                if this.messages_mut().chat.is_some() {
                    return;
                }
                let key = event.keystroke.key.as_str();
                match key {
                    "down" | "j" => this.messages_move_selection(1, cx),
                    "up" | "k" => this.messages_move_selection(-1, cx),
                    "enter" => {
                        let Some(id) = this.messages_mut().selected else {
                            return;
                        };
                        if event.keystroke.modifiers.shift {
                            this.messages_mark_read(id, cx);
                        } else {
                            this.messages_open_entry(id, cx);
                        }
                        window.refresh();
                    }
                    "escape" => {
                        if this.messages_mut().only_unread {
                            this.messages_mut().only_unread = false;
                        } else if this.messages_mut().channel == MessageChannel::Dm
                            && this.messages_mut().only_mutual
                        {
                            this.messages_toggle_only_mutual(cx);
                        } else {
                            this.messages_mut().selected = None;
                        }
                        window.refresh();
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            },
        ));
    }
    col.child(list).into_any_element()
}

pub fn inbox_body<V: MessagesHost + 'static>(
    t: &YkTheme,
    state: &MessageState,
    channel: MessageChannel,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let inbox = state.inbox();
    if inbox.loading && inbox.entries.is_empty() {
        return skeleton_list(t, 3, INBOX_ROW_H);
    }
    if let Some(error) = inbox.error.as_deref() {
        if inbox.entries.is_empty() {
            return error_state(t, channel.error_title(), &friendly_error(error), cx);
        }
    }
    if state.only_unread && inbox.entries.iter().all(|entry| !entry.is_unread()) {
        return status::empty(t, "没有未读消息", "关掉「只看未读」可以看到全部记录")
            .into_any_element();
    }
    if inbox.entries.is_empty() {
        return empty_state(t, channel, cx);
    }

    let rows = Rc::new(state.inbox_rows());
    let sizes = Rc::new(
        rows.iter()
            .map(|row| size(px(row_width(window)), px(row.height())))
            .collect::<Vec<_>>(),
    );
    let rows_for_render = rows.clone();
    let theme = t.clone();
    let selected = state.selected;
    let expanded = state.expanded_like;
    let entity = cx.entity();
    let list = v_virtual_list(
        entity,
        "messages-inbox",
        sizes,
        move |this, range, _window, cx| {
            let now = now_wts();
            range
                .filter_map(|index| {
                    rows_for_render.get(index).copied().map(|row| match row {
                        MessageRow::Divider => divider_row(&theme),
                        MessageRow::Footer => footer_row(
                            &theme,
                            this.messages_mut().inbox().loading_more,
                            this.messages_mut().inbox().has_more,
                            cx,
                        ),
                        MessageRow::Entry(entry_index) => {
                            let Some(entry) =
                                this.messages_mut().inbox().entries.get(entry_index).cloned()
                            else {
                                return div().into_any_element();
                            };
                            entry_row(
                                &theme,
                                &entry,
                                now,
                                selected == Some(entry.id),
                                expanded == Some(entry.id),
                                cx,
                            )
                        }
                        MessageRow::LikeMore { entry, remaining } => {
                            let _ = entry;
                            like_more_row(&theme, remaining)
                        }
                        MessageRow::LikeDetail { entry, liker } => {
                            let Some(entry) = this.messages_mut().inbox().entries.get(entry) else {
                                return div().into_any_element();
                            };
                            let Some(liker) = entry.likers.get(liker) else {
                                return div().into_any_element();
                            };
                            like_detail_row(&theme, liker.nickname.clone(), liker.face_thumb_url())
                        }
                        MessageRow::Conversation(_) => div().into_any_element(),
                    })
                })
                .collect::<Vec<_>>()
        },
    );
    let list = list.track_scroll(&state.inbox_scroll);
    list.into_any_element()
}

pub fn entry_row<V: MessagesHost + 'static>(
    t: &YkTheme,
    entry: &MessageEntry,
    now: u64,
    selected: bool,
    expanded: bool,
    cx: &mut Context<V>,
) -> AnyElement {
    if entry.is_like() {
        return like_entry_row(t, entry, now, selected, expanded, cx);
    }
    let s = &t.surfaces;
    let p = &t.palette;
    let unread = entry.is_unread();
    let id = entry.id;
    let hover_bg = s.fill_1;
    let selected_bg = p.theme_primary_10;
    let base_bg = if selected {
        selected_bg
    } else {
        hsla(0., 0., 0., 0.)
    };
    let actor = entry.actor_label();
    let action = entry.action.clone();
    let time = entry.time_label(now);
    let summary = entry.summary.clone();
    let target_title = entry.target_title.clone();
    let has_target = !target_title.is_empty();
    let face = entry.user.face_thumb_url();
    let initial = entry.user.initial();

    let row = div()
        .id(("msg-row", id as u64))
        .group("message-row")
        .relative()
        .h(px(INBOX_ROW_H))
        .w_full()
        .pl(px(14.))
        .pr(px(SPACE_2))
        .py(px(SPACE_3))
        .flex()
        .gap(px(SPACE_3))
        .border_b_1()
        .border_color(s.border_subtle)
        .bg(base_bg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover_bg))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.messages_open_entry(id, cx);
            window.refresh();
        }))
        .child(
            // 未读点：行首 4px 圆点；文案侧同时加粗，不只靠颜色。
            div()
                .absolute()
                .left(px(2.))
                .top(px(20.))
                .w(px(4.))
                .h(px(4.))
                .rounded_full()
                .bg(if unread {
                    p.status_error
                } else {
                    hsla(0., 0., 0., 0.)
                }),
        )
        .child(V::card_up_avatar(
            SharedString::from(format!("msg-up-{}", id as u64)),
            entry.user.mid,
            entry.user.nickname.clone(),
            entry.user.avatar.clone(),
            40.0,
            avatar(t, &face, &initial, 40.0).into_any_element(),
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(SPACE_2))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .line_clamp(1)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .text_color(s.text_primary)
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(px(SPACE_1))
                                        .child(
                                            div()
                                                .font_weight(if unread {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::MEDIUM
                                                })
                                                .child(actor),
                                        )
                                        .child(
                                            div()
                                                .font_weight(if unread {
                                                    FontWeight::MEDIUM
                                                } else {
                                                    FontWeight::NORMAL
                                                })
                                                .text_color(s.text_secondary)
                                                .child(action),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .child(time),
                        ),
                )
                .child(
                    // 引用块：左 2px 主题色边 + fill_1 底，2 行截断。
                    div()
                        .mt(px(6.))
                        .flex()
                        .rounded(px(RADIUS_HALF))
                        .overflow_hidden()
                        .bg(s.fill_1)
                        .child(div().w(px(2.)).flex_shrink_0().bg(p.theme_primary_30))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .px(px(SPACE_3))
                                .py(px(5.))
                                .text_size(px(FONT_SIZE_CONTROL))
                                .line_height(px(LINE_HEIGHT_CONTROL))
                                .text_color(s.text_tertiary)
                                .line_clamp(2)
                                .child(match mention_styled(t, &summary) {
                                    Some(styled) => styled.into_any_element(),
                                    None => summary.clone().into_any_element(),
                                }),
                        ),
                )
                .children(has_target.then(|| {
                    div()
                        .mt(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(SPACE_1))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .child(
                            Icon::new(IconName::Play)
                                .size(px(12.))
                                .text_color(s.text_quaternary),
                        )
                        .child(div().line_clamp(1).child(target_title))
                })),
        )
        .child(
            div()
                .absolute()
                .right(px(SPACE_2))
                .top(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(2.))
                .opacity(0.)
                .group_hover("message-row", |style| style.opacity(1.0))
                .occlude()
                .px(px(2.))
                .rounded(px(RADIUS_SM))
                .bg(s.surface_elevated_strong)
                .child(
                    icon_button(t, ("msg-read", id as u64), IconName::Check).on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.messages_mark_read(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        },
                    )),
                )
                .child(
                    icon_button(t, ("msg-copy", id as u64), IconName::Copy).on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.messages_copy_link(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        },
                    )),
                ),
        );

    row.into_any_element()
}

/// @ 通道的摘要里把 `@昵称` 染成主题色（设计文档 12.5.4 · 条目解剖）。
///
/// 只处理第一个 @ 片段：从 `@` 到空白 / 冒号；找不到就退回普通文本。
pub fn mention_styled(t: &YkTheme, text: &str) -> Option<StyledText> {
    let s = &t.surfaces;
    let p = &t.palette;
    let at = text.find('@')?;
    if text.len() <= at + 1 {
        return None;
    }
    let rest = &text[at + 1..];
    let end_rel = rest
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || *c == '：' || *c == ':' || *c == '，')
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    let end = at + 1 + end_rel;
    if end <= at + 1 {
        return None;
    }
    let base = TextRun {
        len: at,
        font: gpui::Font {
            weight: FontWeight::NORMAL,
            ..Default::default()
        },
        color: s.text_tertiary,
        ..Default::default()
    };
    let accent = TextRun {
        len: end - at,
        font: gpui::Font {
            weight: FontWeight::MEDIUM,
            ..Default::default()
        },
        color: p.theme_primary,
        ..Default::default()
    };
    let tail = TextRun {
        len: text.len() - end,
        font: gpui::Font {
            weight: FontWeight::NORMAL,
            ..Default::default()
        },
        color: s.text_tertiary,
        ..Default::default()
    };
    let mut runs = Vec::new();
    if at > 0 {
        runs.push(base);
    }
    runs.push(accent);
    if text.len() > end {
        runs.push(tail);
    }
    Some(StyledText::new(text.to_string()).with_runs(runs))
}

pub fn like_entry_row<V: MessagesHost + 'static>(
    t: &YkTheme,
    entry: &MessageEntry,
    now: u64,
    selected: bool,
    expanded: bool,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let unread = entry.is_unread();
    let id = entry.id;
    let hover_bg = s.fill_1;
    let base_bg = if selected {
        p.theme_primary_10
    } else {
        hsla(0., 0., 0., 0.)
    };
    let face = entry.user.face_thumb_url();
    let initial = entry.user.initial();
    let actor = entry.actor_label();
    let action = entry.action.clone();
    let summary = entry.summary.clone();
    let time = entry.time_label(now);

    let mut stack = div().flex_shrink_0().flex().items_center().mt(px(3.));
    for (index, liker) in entry.likers.iter().take(3).enumerate() {
        stack = stack.child(
            div()
                .when(index > 0, |el| el.ml(px(-8.)))
                .rounded_full()
                .border_2()
                .border_color(s.surface_content_solid)
                .child(avatar(t, &liker.face_thumb_url(), &liker.initial(), 26.0)),
        );
    }
    let total_likers = entry.like_count.max(entry.likers.len() as u64);
    if total_likers > 3 {
        stack = stack.child(
            div()
                .ml(px(-8.))
                .w(px(26.))
                .h(px(26.))
                .rounded_full()
                .border_2()
                .border_color(s.surface_content_solid)
                .bg(s.fill_3)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_secondary)
                .child(format!("+{}", total_likers - 3)),
        );
    }

    div()
        .id(("msg-like-row", id as u64))
        .group("message-row")
        .relative()
        .h(px(INBOX_ROW_H))
        .w_full()
        .pl(px(14.))
        .pr(px(SPACE_2))
        .py(px(SPACE_2))
        .flex()
        .gap(px(SPACE_3))
        .border_b_1()
        .border_color(s.border_subtle)
        .bg(base_bg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover_bg))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.messages_open_entry(id, cx);
            window.refresh();
        }))
        .child(
            div()
                .absolute()
                .left(px(2.))
                .top(px(20.))
                .w(px(4.))
                .h(px(4.))
                .rounded_full()
                .bg(if unread {
                    p.status_error
                } else {
                    hsla(0., 0., 0., 0.)
                }),
        )
        .child(V::card_up_avatar(
            SharedString::from(format!("msg-like-up-{}", id as u64)),
            entry.user.mid,
            entry.user.nickname.clone(),
            entry.user.avatar.clone(),
            40.0,
            avatar(t, &face, &initial, 40.0).into_any_element(),
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_baseline()
                        .gap(px(SPACE_2))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .line_clamp(1)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .text_color(s.text_primary)
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(px(SPACE_1))
                                        .child(
                                            div()
                                                .font_weight(if unread {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::MEDIUM
                                                })
                                                .child(actor),
                                        )
                                        .child(div().text_color(s.text_secondary).child(action)),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .child(time),
                        ),
                )
                .child(stack)
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .rounded(px(RADIUS_HALF))
                        .overflow_hidden()
                        .bg(s.fill_1)
                        .child(div().w(px(2.)).flex_shrink_0().bg(p.theme_primary_30))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .px(px(SPACE_3))
                                .py(px(5.))
                                .text_size(px(FONT_SIZE_CONTROL))
                                .line_height(px(LINE_HEIGHT_CONTROL))
                                .text_color(s.text_tertiary)
                                .line_clamp(1)
                                .child(summary),
                        ),
                ),
        )
        .child(
            div()
                .absolute()
                .right(px(SPACE_2))
                .top(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(2.))
                .opacity(0.)
                .group_hover("message-row", |style| style.opacity(1.0))
                .occlude()
                .px(px(2.))
                .rounded(px(RADIUS_SM))
                .bg(s.surface_elevated_strong)
                .child(
                    icon_button(t, ("msg-like-read", id as u64), IconName::Check).on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.messages_mark_read(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        }),
                    ),
                )
                .child(
                    icon_button(t, ("msg-like-copy", id as u64), IconName::Copy).on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.messages_copy_link(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        }),
                    ),
                )
                .child(
                    icon_button(t, ("msg-like-expand", id as u64), IconName::ChevronDown)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.messages_toggle_like(id, cx);
                            window.refresh();
                            cx.stop_propagation();
                        }))
                        .when(expanded, |el| el.bg(p.theme_primary_10)),
                ),
        )
        .into_any_element()
}

/// 消息页的模态确认框（移除会话）。
pub fn confirm_overlay<V: MessagesHost + 'static>(
    t: &YkTheme,
    confirm: &RemoveConfirm,
    busy: bool,
    error: Option<&str>,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.surfaces.overlay_scrim)
        .occlude()
        .child(
            div()
                .w(px(380.))
                .flex()
                .flex_col()
                .gap(px(SPACE_3))
                .p(px(SPACE_5))
                .rounded(px(MODAL_RADIUS))
                .bg(s.surface_content_solid)
                .border_1()
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_glass_modal.clone())
                .child(
                    div()
                        .text_size(px(FONT_SIZE_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("移除这个会话？"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(LINE_HEIGHT_BODY))
                        .text_color(s.text_secondary)
                        .child(format!(
                            "「{}」会从私信列表移除；之后对方再发消息时会重新出现。",
                            confirm.nickname
                        )),
                )
                .children(error.map(|error| {
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(p.status_error)
                        .child(friendly_error(error))
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(SPACE_2))
                        .child(
                            div()
                                .id("msg-remove-cancel")
                                .h(px(CONTROL_HEIGHT))
                                .px(px(SPACE_4))
                                .flex()
                                .items_center()
                                .rounded(px(INTERACTIVE_RADIUS))
                                .border_1()
                                .border_color(p.theme_primary_40)
                                .text_size(px(FONT_SIZE_CONTROL))
                                .text_color(p.theme_primary)
                                .cursor_pointer()
                                .hover(|style| style.bg(p.theme_primary_10))
                                .child("取消")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.messages_cancel_remove(cx);
                                    window.refresh();
                                })),
                        )
                        .child(
                            div()
                                .id("msg-remove-confirm")
                                .h(px(CONTROL_HEIGHT))
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
                                .child(if busy { "移除中…" } else { "确认移除" })
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.messages_confirm_remove(cx);
                                    window.refresh();
                                })),
                        ),
                ),
        )
        .into_any_element()
}


