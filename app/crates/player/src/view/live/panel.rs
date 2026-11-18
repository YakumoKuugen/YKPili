//! 直播面板与中央浮层。
//!
//! 面板 Tab 由 `mode::PanelSpec` 决定（弹幕 / 直播信息），内容在这里分派；
//! 断流 / 取流失败用同一个中央浮层，按钮只有「重新连接」与「关闭」。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_domain::format;
use yk_theme::theme::*;

use crate::PlayerView;

impl PlayerView {
    /// 断流 / 取流失败浮层。`None` = 一切正常，不挂树。
    pub(crate) fn live_overlay(
        &self,
        t: &YkTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let live = self.live.as_ref()?;
        let (title, desc) = if let Some(outage) = live.outage.as_deref() {
            ("直播已结束或连接中断", outage.to_string())
        } else if let Some(error) = live.error.as_deref() {
            ("直播加载失败", error.to_string())
        } else {
            return None;
        };
        let s = &t.surfaces;
        let p = &t.palette;
        Some(
            div()
                .id("live-outage")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000066))
                .child(
                    div()
                        .w(px(360.))
                        .rounded(px(MODAL_RADIUS))
                        .bg(s.surface_popover)
                        .border_1()
                        .border_color(s.border_popover)
                        .shadow(t.shadows.shadow_2.clone())
                        .p(px(SPACE_5))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(SPACE_3))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(40.))
                                .h(px(40.))
                                .rounded_full()
                                .bg(p.bili_pink.opacity(0.16))
                                .child(
                                    Icon::new(IconName::Radio)
                                        .size(px(20.))
                                        .text_color(p.bili_pink),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(s.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .line_height(px(18.))
                                .text_color(s.text_tertiary)
                                .text_center()
                                .child(desc),
                        )
                        .child(
                            div()
                                .mt(px(SPACE_1))
                                .flex()
                                .items_center()
                                .gap(px(SPACE_3))
                                .child(
                                    div()
                                        .id("live-retry")
                                        .h(px(32.))
                                        .px(px(SPACE_5))
                                        .flex()
                                        .items_center()
                                        .rounded_full()
                                        .bg(p.theme_primary)
                                        .text_size(px(FONT_SIZE_CAPTION))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .cursor_pointer()
                                        .hover(|style| style.opacity(0.9))
                                        .child("重新连接")
                                        .on_click(cx.listener(
                                            |view, _: &ClickEvent, _window, cx| {
                                                view.retry_live(cx);
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id("live-close")
                                        .h(px(32.))
                                        .px(px(SPACE_4))
                                        .flex()
                                        .items_center()
                                        .rounded_full()
                                        .bg(s.fill_2)
                                        .text_size(px(FONT_SIZE_CAPTION))
                                        .text_color(s.text_secondary)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(s.fill_3))
                                        .child("关闭")
                                        .on_click(cx.listener(
                                            move |view, _: &ClickEvent, window, cx| {
                                                view.request_close(window, cx);
                                            },
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 直播面板内容：0 弹幕（右侧聊天列表）/ 1 直播信息。
    pub(crate) fn live_tab_content(
        &self,
        t: &YkTheme,
        active: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match active {
            0 => self.live_chat_tab(t, cx),
            _ => self.live_info_tab(t),
        }
    }

    /// 右侧聊天列表：连接状态 + 最近消息（环形缓冲最多 200 条）。
    fn live_chat_tab(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let Some(live) = self.live.as_ref() else {
            return div().into_any_element();
        };
        let (dot, label): (Hsla, String) = match &live.chat.connection {
            super::state::ChatConnection::Connected => {
                (rgb(0x4CD964).into(), "已连接".to_string())
            }
            super::state::ChatConnection::Connecting => {
                (rgb(0xFFB020).into(), "连接中…".to_string())
            }
            super::state::ChatConnection::Reconnecting { attempt, .. } => {
                (rgb(0xFFB020).into(), format!("重连中（第 {attempt} 次）"))
            }
            super::state::ChatConnection::Failed(error) => {
                (p.status_error, format!("连接失败：{error}"))
            }
            super::state::ChatConnection::Idle => (s.text_quaternary, "未连接".to_string()),
        };
        let failed = matches!(
            live.chat.connection,
            super::state::ChatConnection::Failed(_)
        );
        let header = div()
            .w_full()
            .px(px(SPACE_4))
            .py(px(SPACE_2))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(s.border_subtle)
            .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(dot))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_tertiary)
                    .child(label),
            )
            .when(failed, |el| {
                el.child(
                    div()
                        .id("live-chat-retry")
                        .h(px(22.))
                        .px(px(SPACE_2))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(s.fill_2)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_secondary)
                        .cursor_pointer()
                        .hover(|style| style.bg(s.fill_3))
                        .child("重新连接")
                        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.start_live_chat(cx);
                        })),
                )
            });

        let mut list = div()
            .id("live-chat-list")
            .w_full()
            .h(px(420.))
            .overflow_y_scroll()
            .track_scroll(&live.chat.scroll)
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(SPACE_3))
            .py(px(SPACE_2));
        if live.chat.messages.is_empty() {
            list = list.child(
                div()
                    .py(px(SPACE_6))
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_quaternary)
                    .text_center()
                    .child(match live.chat.connection {
                        super::state::ChatConnection::Connected => "还没有人说话",
                        _ => "正在连接直播聊天…",
                    }),
            );
        } else {
            // 只渲染最近 80 条：环形缓冲保留 200 条，但不必每帧全量排版。
            let start = live.chat.messages.len().saturating_sub(80);
            for message in live.chat.messages.iter().skip(start) {
                list = list.child(chat_row(t, message));
            }
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .into_any_element()
    }

    fn live_info_tab(&self, t: &YkTheme) -> AnyElement {
        let s = &t.surfaces;
        let Some(live) = self.live.as_ref() else {
            return div().into_any_element();
        };
        let mut rows = div()
            .w_full()
            .px(px(SPACE_4))
            .py(px(SPACE_3))
            .flex()
            .flex_col()
            .gap(px(SPACE_2));
        let status = if live.outage.is_some() {
            "已断开"
        } else if live.loading {
            "正在连接…"
        } else {
            "直播中"
        };
        for (label, value) in [
            ("主播", live.uname.clone()),
            ("分区", live.area_name.clone()),
            ("直播间", format!("#{}", live.room_id)),
            ("协议", if live.is_flv { "FLV" } else { "HLS" }.to_string()),
            ("人气", format::count(live.online)),
            ("看过", live.watched_text.clone()),
            ("状态", status.to_string()),
        ] {
            let value = if value.trim().is_empty() {
                "未知".to_string()
            } else {
                value
            };
            rows = rows.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(SPACE_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .child(
                        div()
                            .w(px(56.))
                            .flex_shrink_0()
                            .text_color(s.text_quaternary)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_color(s.text_secondary)
                            .child(value),
                    ),
            );
        }
        rows.into_any_element()
    }
}

fn chat_row(t: &YkTheme, message: &yk_infra::live_chat::LiveChatMessage) -> AnyElement {
    use yk_infra::live_chat::LiveChatKind;
    let s = &t.surfaces;
    let p = &t.palette;
    let name_color: Hsla = if message.is_admin {
        rgb(0xFFD700).into()
    } else if message.guard_level > 0 {
        p.bili_pink
    } else {
        s.text_quaternary
    };
    let text_color: Hsla = match message.color {
        Some((r, g, b)) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into(),
        None => s.text_secondary,
    };
    let (bg, text_color): (Hsla, Hsla) = match message.kind {
        LiveChatKind::Gift | LiveChatKind::Guard => (p.theme_primary_20, s.text_primary),
        LiveChatKind::SuperChat => (rgba(0xF7BA2A33).into(), s.text_primary),
        _ => (rgba(0x00000000).into(), text_color),
    };
    // 舰队 / 房管用小徽标区分身份，文字与徽标双编码（不只靠颜色）。
    let badge: Option<(&str, Hsla)> = if message.is_admin {
        Some(("房管", rgb(0xFFD700).into()))
    } else if message.guard_level >= 3 {
        Some(("舰长", p.bili_pink))
    } else if message.guard_level == 2 {
        Some(("提督", p.bili_pink))
    } else if message.guard_level == 1 {
        Some(("总督", p.bili_pink))
    } else {
        None
    };
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(4.))
        .rounded(px(6.))
        .px(px(4.))
        .py(px(2.))
        .bg(bg)
        .children(badge.map(|(label, color)| {
            div()
                .flex_shrink_0()
                .px(px(3.))
                .rounded(px(3.))
                .bg(color.opacity(0.18))
                .text_size(px(9.))
                .text_color(color)
                .child(label)
        }))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(name_color)
                .child(format!("{}：", message.uname)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(FONT_SIZE_MICRO))
                .line_height(px(16.))
                .text_color(text_color)
                .child(message.text.clone()),
        )
        .into_any_element()
}
