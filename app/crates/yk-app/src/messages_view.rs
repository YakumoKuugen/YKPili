//! 消息中心（设计文档 12.5.4）。
//!
//! 版式：标题与四通道分段同一行，下面是单列列表，内容列最大宽 960 居中。
//! - 互动三通道（回复 / @ / 赞）固定行高 116，走 `v_virtual_list`；
//! - 私信会话行高 72；点开进入会话页（气泡 / 时间分组 / 撤回占位 / 吸底输入）；
//! - 未读分隔线「以上为已读」只在首次进入、且存在未读时出现一次；
//! - 赞在客户端按「被赞内容 + 业务」聚合，24 小时窗口；
//! - 私信只收互关：`session_type=9` + `/x/relation/friends` 白名单，
//!   打开会话时再用单条 relation 校验（E-24）；
//! - 互动没有可清零的已读接口，所以不提供「全部已读」，只有「只看未读」（E-26）。
//!
//! 状态放在 `AppRoot` 上（与历史页同款），切页不丢通道、筛选与滚动位置。

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui::assets::IconName;
use gpui::component::input::{InputEvent, Textarea, TextareaState};
use gpui::component::{v_virtual_list, Icon, VirtualListScrollHandle};
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::api::client::now_wts;
use yk_core::api::client::{
    fetch_chat_messages, fetch_conversations, fetch_dm_unread_total, fetch_msgfeed_inbox,
    fetch_msgfeed_unread, fetch_mutual_ids, fetch_relation_attribute, mark_conversation_read,
    remove_conversation, send_dm, ReqwestClient,
};
use yk_core::messages::{
    aggregate_likes, chat_time_label, needs_time_divider, ChatMessage, Conversation, InboxCursor,
    MessageChannel, MessageEntry, MessageTarget, UnreadCounts, DM_MAX_BYTES, MUTUAL_TTL_SECS,
};
use yk_ui::status;
use yk_ui::theme::*;

use crate::{AppRoot, ImageResidency, DOCK_RESERVED};

/// 互动条目固定行高（--s12-inbox-row-h）。
pub(crate) const INBOX_ROW_H: f32 = 116.0;
/// 私信会话固定行高（--s12-msg-row-h）。
pub(crate) const DM_ROW_H: f32 = 72.0;
/// 未读分隔线高。
const UNREAD_DIVIDER_H: f32 = 28.0;
/// 赞展开后每一行的高。
const LIKE_DETAIL_H: f32 = 44.0;
/// 列表底部「加载中 / 没有更多了」。
const FOOTER_H: f32 = 44.0;
/// 内容列最大宽（--s12-inbox-w）。960 ≈ 70 个汉字。
const CONTENT_MAX_W: f32 = 960.0;
/// 私信气泡最大宽（--s12-bubble-w）。
const BUBBLE_MAX_W: f32 = 320.0;
/// 私信输入区高（--s12-composer-h）。
const COMPOSER_H: f32 = 56.0;
/// 消息页自己的轮询间隔（进页 30s）。
const POLL_INTERVAL: Duration = Duration::from_secs(30);

// ================= 页内状态 =================

/// 一个互动通道的一页状态。四个通道（含私信）各自一份 generation。
#[derive(Default)]
pub struct InboxState {
    pub entries: Vec<MessageEntry>,
    pub cursor: Option<InboxCursor>,
    pub has_more: bool,
    pub loading: bool,
    pub loading_more: bool,
    /// 是否成功拉过至少一页；区分「空」与「还没开始」。
    pub loaded: bool,
    pub error: Option<String>,
    pub generation: u64,
}

impl InboxState {
    pub fn reset(&mut self) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            generation,
            ..Self::default()
        };
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = None;
        self.has_more = false;
        self.loaded = true;
        self.error = None;
    }
}

/// 私信会话列表状态。
#[derive(Default)]
pub struct DmState {
    pub conversations: Vec<Conversation>,
    /// 下一页 `begin_ts`（微秒）；None = 到底。
    pub begin_ts: Option<i64>,
    pub has_more: bool,
    pub loading: bool,
    pub loading_more: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub generation: u64,
}

impl DmState {
    pub fn reset(&mut self) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            generation,
            ..Self::default()
        };
    }
}

/// 打开中的私信会话。
pub struct ChatState {
    pub talker_id: i64,
    pub nickname: String,
    pub avatar: String,
    pub mutual: Option<bool>,
    pub messages: Vec<ChatMessage>,
    pub loading: bool,
    pub loading_more: bool,
    pub has_more: bool,
    /// 更早一页的 begin_seqno。
    pub begin_seqno: i64,
    pub error: Option<String>,
    pub sending: bool,
    /// 正在发送的正文（气泡右下角转圈）。
    pub pending: Option<String>,
    /// 发送失败的正文；气泡保留，点击重发。
    pub failed: Option<String>,
    /// 打开前的未读数，用于本地乐观扣减。
    pub unread_before: u64,
    pub scroll: ScrollHandle,
    /// 拉完最新一页后滚到底（只做一次）。
    pub pending_bottom: bool,
    pub generation: u64,
}

impl ChatState {
    pub fn new(conversation: &Conversation) -> Self {
        Self {
            talker_id: conversation.talker_id,
            nickname: conversation.nickname.clone(),
            avatar: conversation.avatar.clone(),
            mutual: conversation.mutual,
            messages: Vec::new(),
            loading: true,
            loading_more: false,
            has_more: false,
            begin_seqno: 0,
            error: None,
            sending: false,
            pending: None,
            failed: None,
            unread_before: conversation.unread,
            scroll: ScrollHandle::new(),
            pending_bottom: false,
            generation: 0,
        }
    }

    pub fn can_send(&self) -> bool {
        self.mutual != Some(false)
    }
}

/// 移除会话的模态确认。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveConfirm {
    pub talker_id: i64,
    pub nickname: String,
}

/// 消息页全部可变状态。
pub struct MessageState {
    pub channel: MessageChannel,
    pub reply: InboxState,
    pub mention: InboxState,
    pub like: InboxState,
    pub dm: DmState,
    pub unread: UnreadCounts,
    pub unread_loaded: bool,
    pub unread_loading: bool,
    pub only_unread: bool,
    /// 私信列表「只看互关」：隐藏已解除互关的历史会话。
    pub only_mutual: bool,
    /// 键盘 / 点击选中的条目 id。
    pub selected: Option<i64>,
    /// 首屏未读锚点只做一次；切通道重置。
    pub anchored: bool,
    pub inbox_scroll: VirtualListScrollHandle,
    pub dm_scroll: VirtualListScrollHandle,
    /// 展开点赞列表的条目 id。
    pub expanded_like: Option<i64>,
    pub chat: Option<ChatState>,
    pub composer: Option<Entity<TextareaState>>,
    pub composer_draft: String,
    /// 移除会话确认框。
    pub confirm: Option<RemoveConfirm>,
    pub confirm_busy: bool,
    pub action_error: Option<String>,
    pub notice: Option<String>,
    pub notice_generation: u64,
    /// 互关白名单（E-24 的退路：5 分钟 TTL）。
    pub whitelist: HashSet<i64>,
    pub whitelist_loaded: bool,
    pub whitelist_loading: bool,
    pub whitelist_checked_at: Option<std::time::Instant>,
    /// 列表键盘焦点。首帧 render 时创建（FocusHandle 需要 cx）。
    pub focus: Option<FocusHandle>,
    pub focus_requested: bool,
    /// 本地「标为已读」的条目 id（E-26：msgfeed 没有可核实的单条已读接口）。
    pub read_ids: HashSet<i64>,
    /// 首次进入页面时拉一次未读 / 白名单。
    pub poll_started: bool,
    /// 输入框里按 Enter：下一帧 render 再发送（需要 Window 清空输入框）。
    pub send_requested: bool,
    /// 窗口是否在前台：失焦时暂停 30s 轮询（设计文档 12.5.4）。
    pub window_active: bool,
}

impl Default for MessageState {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageState {
    pub fn new() -> Self {
        Self {
            channel: MessageChannel::Reply,
            reply: InboxState::default(),
            mention: InboxState::default(),
            like: InboxState::default(),
            dm: DmState::default(),
            unread: UnreadCounts::default(),
            unread_loaded: false,
            unread_loading: false,
            only_unread: false,
            only_mutual: false,
            selected: None,
            anchored: false,
            inbox_scroll: VirtualListScrollHandle::new(),
            dm_scroll: VirtualListScrollHandle::new(),
            expanded_like: None,
            chat: None,
            composer: None,
            composer_draft: String::new(),
            confirm: None,
            confirm_busy: false,
            action_error: None,
            notice: None,
            notice_generation: 0,
            whitelist: HashSet::new(),
            whitelist_loaded: false,
            whitelist_loading: false,
            whitelist_checked_at: None,
            read_ids: HashSet::new(),
            focus: None,
            focus_requested: false,
            poll_started: false,
            send_requested: false,
            window_active: true,
        }
    }

    pub fn reset(&mut self) {
        let focus = self.focus.clone();
        let composer = self.composer.take();
        *self = Self::new();
        self.focus = focus;
        self.composer = composer;
    }

    /// 重新登录 / 换账号时连本地已读一起清掉。
    pub fn reset_for_account(&mut self) {
        self.reset();
        self.read_ids.clear();
    }

    pub fn inbox(&self) -> &InboxState {
        match self.channel {
            MessageChannel::Reply => &self.reply,
            MessageChannel::Mention => &self.mention,
            MessageChannel::Like => &self.like,
            MessageChannel::Dm => &self.reply,
        }
    }

    pub fn inbox_mut(&mut self) -> &mut InboxState {
        match self.channel {
            MessageChannel::Reply => &mut self.reply,
            MessageChannel::Mention => &mut self.mention,
            MessageChannel::Like => &mut self.like,
            MessageChannel::Dm => &mut self.reply,
        }
    }

    pub fn entries(&self) -> &[MessageEntry] {
        &self.inbox().entries
    }

    /// 切换通道：重置锚点与选中，保留已加载的数据。
    pub fn switch_channel(&mut self, channel: MessageChannel) {
        if self.channel == channel {
            return;
        }
        self.channel = channel;
        self.anchored = false;
        self.selected = None;
        self.expanded_like = None;
        self.only_unread = self.only_unread && channel.is_inbox();
        self.action_error = None;
    }

    /// 「只看未读」筛选后，在当前列表里的原始下标。
    pub fn visible_indices(&self) -> Vec<usize> {
        self.entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| !self.only_unread || entry.is_unread())
            .map(|(index, _)| index)
            .collect()
    }

    /// 互动列表的行模型。未读分隔线只在存在未读时插在第一条未读之前。
    pub fn inbox_rows(&self) -> Vec<MessageRow> {
        let visible = self.visible_indices();
        let first_unread = self.entries().iter().position(|entry| entry.is_unread());
        let mut rows = Vec::new();
        for index in visible {
            if !self.only_unread && first_unread == Some(index) {
                rows.push(MessageRow::Divider);
            }
            rows.push(MessageRow::Entry(index));
            let entry = &self.entries()[index];
            if entry.is_like() && self.expanded_like == Some(entry.id) {
                for liker in 0..entry.likers.len() {
                    rows.push(MessageRow::LikeDetail {
                        entry: index,
                        liker,
                    });
                }
                let remaining = entry.like_count.saturating_sub(entry.likers.len() as u64);
                if remaining > 0 {
                    rows.push(MessageRow::LikeMore {
                        entry: index,
                        remaining,
                    });
                }
            }
        }
        if !self.entries().is_empty() {
            rows.push(MessageRow::Footer);
        }
        rows
    }

    /// 某条目在行模型中的下标（键盘滚动用）。
    pub fn row_of_entry(&self, id: i64) -> Option<usize> {
        self.inbox_rows().iter().position(|row| match row {
            MessageRow::Entry(index) | MessageRow::LikeDetail { entry: index, .. } => {
                self.entries().get(*index).map(|entry| entry.id) == Some(id)
            }
            _ => false,
        })
    }

    /// 第一条未读所在行；没有未读返回 None。
    pub fn first_unread_row(&self) -> Option<usize> {
        self.inbox_rows().iter().position(|row| match row {
            MessageRow::Entry(index) => self
                .entries()
                .get(*index)
                .map(|entry| entry.is_unread())
                .unwrap_or(false),
            _ => false,
        })
    }

    pub fn selected_id(&self) -> Option<i64> {
        self.selected
    }

    pub fn active_entries(&self) -> usize {
        self.entries().len()
    }

    pub fn dm_unread_total(&self) -> u64 {
        self.dm
            .conversations
            .iter()
            .filter(|conversation| conversation.mutual != Some(false))
            .map(|conversation| conversation.unread)
            .sum()
    }

    /// 已经打开的会话列表（给渲染层只读访问）。
    pub fn chat(&self) -> Option<&ChatState> {
        self.chat.as_ref()
    }

    /// 消息页自己的滚动句柄：互动走 inbox，私信列表走 dm，聊天页走 chat。
    pub fn active_scroll(&self) -> Option<&VirtualListScrollHandle> {
        if self.chat.is_some() {
            return None;
        }
        Some(match self.channel {
            MessageChannel::Dm => &self.dm_scroll,
            _ => &self.inbox_scroll,
        })
    }

    pub fn active_has_more(&self) -> bool {
        if self.chat.is_some() {
            return false;
        }
        match self.channel {
            MessageChannel::Dm => self.dm.has_more,
            _ => self.inbox().has_more,
        }
    }

    /// 「只看互关」筛选后的会话原始下标。
    pub fn visible_conversation_indices(&self) -> Vec<usize> {
        self.dm
            .conversations
            .iter()
            .enumerate()
            .filter(|(_, conversation)| !self.only_mutual || conversation.mutual == Some(true))
            .map(|(index, _)| index)
            .collect()
    }

    /// 只保留当前筛选下仍可见的选中会话；没有就落第一条。
    pub fn ensure_dm_selection(&mut self) {
        let visible = self.visible_conversation_indices();
        let still_visible = self.selected.is_some_and(|id| {
            visible
                .iter()
                .any(|index| self.dm.conversations[*index].talker_id == id)
        });
        if !still_visible {
            self.selected = visible
                .first()
                .map(|index| self.dm.conversations[*index].talker_id);
        }
    }

    /// 切换「只看互关」。切换后重新落选中，并回到列表顶部。
    pub fn toggle_only_mutual(&mut self) {
        self.only_mutual = !self.only_mutual;
        self.ensure_dm_selection();
    }

    /// 会话列表的行模型：会话 + 底部加载提示。
    pub fn dm_rows(&self) -> Vec<MessageRow> {
        let visible = self.visible_conversation_indices();
        let mut rows: Vec<MessageRow> = visible.into_iter().map(MessageRow::Conversation).collect();
        if !rows.is_empty() {
            rows.push(MessageRow::Footer);
        }
        rows
    }

    /// @ 通道的响应没有 `last_view_at`（E-26），只给了总数。
    /// 退路：按时间倒序把最新 N 条标成未读，其余算已读；本地已读覆盖优先。
    pub fn reconcile_mention_unread(&mut self) {
        if self.unread_loaded {
            let count = self.unread.mention as usize;
            for (index, entry) in self.mention.entries.iter_mut().enumerate() {
                if self.read_ids.contains(&entry.id) {
                    entry.mark_read();
                } else if index < count {
                    entry.read_override = false;
                    // 客户端默认把 last_view_at 设成「现在」，这里显式往回拨 1 秒，
                    // `is_unread()` 的 `time > last_view_at` 才成立。
                    entry.last_view_at = entry.time.saturating_sub(1);
                } else {
                    entry.mark_read();
                }
            }
        }
    }

    /// 白名单应用后重新标注会话的互关状态。
    pub fn apply_whitelist(&mut self) {
        if !self.whitelist_loaded {
            return;
        }
        for conversation in &mut self.dm.conversations {
            conversation.mutual = Some(self.whitelist.contains(&conversation.talker_id));
        }
        self.unread.dm = self.dm_unread_total();
        self.ensure_dm_selection();
    }
}

/// 列表行。固定高度的行一起喂给 `v_virtual_list` 的 `item_sizes`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRow {
    /// 「以上为已读」分隔线。
    Divider,
    /// 互动条目（下标指向当前通道 entries）。
    Entry(usize),
    /// 赞展开后的单个点赞者。
    LikeDetail { entry: usize, liker: usize },
    /// 接口只回传最多 5 个头像；聚合后人数更多时补一行「还有 N 人」。
    LikeMore { entry: usize, remaining: u64 },
    /// 私信会话（下标指向 dm.conversations）。
    Conversation(usize),
    /// 加载中 / 加载更多 / 没有更多了。
    Footer,
}

impl MessageRow {
    pub fn height(self) -> f32 {
        match self {
            Self::Divider => UNREAD_DIVIDER_H,
            Self::Entry(_) => INBOX_ROW_H,
            Self::LikeDetail { .. } | Self::LikeMore { .. } => LIKE_DETAIL_H,
            Self::Conversation(_) => DM_ROW_H,
            Self::Footer => FOOTER_H,
        }
    }
}

// ================= 页面 =================

/// 渲染消息页。`window` 用来取视口宽、创建输入框焦点。
pub(crate) fn page(
    t: &YkTheme,
    state: &MessageState,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<AppRoot>,
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

fn inbox_page(
    t: &YkTheme,
    state: &MessageState,
    now: u64,
    window: &Window,
    cx: &mut Context<AppRoot>,
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
                if this.messages.chat.is_some() {
                    return;
                }
                let key = event.keystroke.key.as_str();
                match key {
                    "down" | "j" => this.messages_move_selection(1, cx),
                    "up" | "k" => this.messages_move_selection(-1, cx),
                    "enter" => {
                        let Some(id) = this.messages.selected else {
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
                        if this.messages.only_unread {
                            this.messages.only_unread = false;
                        } else if this.messages.channel == MessageChannel::Dm
                            && this.messages.only_mutual
                        {
                            this.messages_toggle_only_mutual(cx);
                        } else {
                            this.messages.selected = None;
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

fn header(t: &YkTheme, state: &MessageState, cx: &mut Context<AppRoot>) -> AnyElement {
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

    // 四通道分段控件：按钮 + 数字角标；数字只在这里出现。
    let mut seg = div()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(3.))
        .rounded_full()
        .bg(s.fill_2);
    for channel in MessageChannel::ALL {
        let active = state.channel == channel;
        let count = state.unread.channel(channel);
        let accent = p.theme_primary;
        let muted = s.text_tertiary;
        let active_bg = s.surface_content_solid;
        seg = seg.child(
            div()
                .id(("msg-channel", channel as usize))
                .h(px(30.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded_full()
                .bg(if active {
                    active_bg
                } else {
                    hsla(0., 0., 0., 0.)
                })
                .shadow(if active {
                    t.shadows.shadow_1.clone()
                } else {
                    Vec::new()
                })
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(if active { accent } else { muted })
                .cursor_pointer()
                .hover(move |style| style.text_color(accent))
                .child(channel.label())
                .children((count > 0).then(|| count_badge(t, count, active)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.messages_switch_channel(channel, cx);
                    window.refresh();
                })),
        );
    }

    row =
        row.child(seg)
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
            }))
            .child(
                div()
                    .id("msg-refresh")
                    .w(px(30.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |style| style.bg(s.fill_2))
                    .child(
                        Icon::new(IconName::RefreshCw)
                            .size(px(15.))
                            .text_color(s.text_secondary),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.refresh_messages(cx);
                        window.refresh();
                    })),
            );

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

fn count_badge(t: &YkTheme, count: u64, active: bool) -> AnyElement {
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

fn inline_error(t: &YkTheme, error: &str) -> AnyElement {
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

fn error_state(t: &YkTheme, title: &str, error: &str, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    this.refresh_messages(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn empty_state(t: &YkTheme, channel: MessageChannel, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    this.page = Some(crate::pages::PageKind::Following);
                    this.reset_card_hover();
                    this.load_current_page(cx);
                    this.scroll_content_to_top();
                    window.refresh();
                    cx.notify();
                })),
        );
    }
    col.into_any_element()
}

fn skeleton_list(t: &YkTheme, rows: usize, height: f32) -> AnyElement {
    let brush = t.surfaces.skeleton_brush;
    let border = t.surfaces.border_subtle;
    div()
        .flex()
        .flex_col()
        .w_full()
        .with_animation(
            "msg-skeleton-pulse",
            Animation::new(Duration::from_millis(yk_ui::status::SKELETON_PULSE_MS)).repeat(),
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

/// 行宽：虚拟列表每一行的实际宽度。取内容列宽（窗口不足时用可用宽）。
fn row_width(window: &Window) -> f32 {
    // 页面内容列最大 960，左右各 24 内边距；行宽必须等于内宽，
    // 不能用 960（会超出内容列 48）。
    (window.viewport_size().width.as_f32() - SPACE_6 * 2.0).clamp(320.0, CONTENT_MAX_W)
}

fn inbox_body(
    t: &YkTheme,
    state: &MessageState,
    channel: MessageChannel,
    window: &Window,
    cx: &mut Context<AppRoot>,
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
                            this.messages.inbox().loading_more,
                            this.messages.inbox().has_more,
                            cx,
                        ),
                        MessageRow::Entry(entry_index) => {
                            let Some(entry) =
                                this.messages.inbox().entries.get(entry_index).cloned()
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
                            let Some(entry) = this.messages.inbox().entries.get(entry) else {
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

fn divider_row(t: &YkTheme) -> AnyElement {
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

fn footer_row(
    t: &YkTheme,
    loading_more: bool,
    has_more: bool,
    cx: &mut Context<AppRoot>,
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

fn entry_row(
    t: &YkTheme,
    entry: &MessageEntry,
    now: u64,
    selected: bool,
    expanded: bool,
    cx: &mut Context<AppRoot>,
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
        .child(AppRoot::up_profile_avatar(
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
fn mention_styled(t: &YkTheme, text: &str) -> Option<StyledText> {
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

fn like_entry_row(
    t: &YkTheme,
    entry: &MessageEntry,
    now: u64,
    selected: bool,
    expanded: bool,
    cx: &mut Context<AppRoot>,
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
        .child(AppRoot::up_profile_avatar(
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

fn like_more_row(t: &YkTheme, remaining: u64) -> AnyElement {
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

fn like_detail_row(t: &YkTheme, nickname: String, face: String) -> AnyElement {
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

fn dm_body(
    t: &YkTheme,
    state: &MessageState,
    now: u64,
    window: &Window,
    cx: &mut Context<AppRoot>,
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
                                this.messages.dm.conversations.get(index).cloned()
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
                            this.messages.dm.loading_more,
                            this.messages.dm.has_more,
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
fn dm_mutual_empty_state(t: &YkTheme, cx: &mut Context<AppRoot>) -> AnyElement {
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

fn conversation_row(
    t: &YkTheme,
    conversation: &Conversation,
    now: u64,
    selected: bool,
    cx: &mut Context<AppRoot>,
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
        .child(AppRoot::up_profile_avatar(
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

fn avatar(t: &YkTheme, url: &str, initial: &str, size: f32) -> AnyElement {
    let s = &t.surfaces;
    let mut el = div()
        .relative()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .bg(s.fill_2)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size * 0.4).max(10.)))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(s.text_tertiary)
        // 首字垫在底下：图片加载失败时仍然有可辨识的占位（设计 12.5.4 头像槽位）。
        .child(initial.to_string());
    if !url.trim().is_empty() {
        el = el.child(
            img(ImageSource::from(url.to_string()))
                .absolute()
                .top_0()
                .left_0()
                .w(px(size))
                .h(px(size))
                .object_fit(ObjectFit::Cover),
        );
    }
    el.into_any_element()
}

fn icon_button(t: &YkTheme, id: impl Into<ElementId>, icon: IconName) -> Stateful<Div> {
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

fn face_thumb(url: &str) -> String {
    if url.trim().is_empty() {
        return String::new();
    }
    format!("{}@64w_64h_1c.webp", url.trim())
}

// ================= 私信会话页 =================

fn chat_page(
    t: &YkTheme,
    state: &MessageState,
    chat: &ChatState,
    now: u64,
    _window: &Window,
    cx: &mut Context<AppRoot>,
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

fn chat_message_row(t: &YkTheme, message: &ChatMessage) -> AnyElement {
    use yk_core::messages::ChatKind;
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

fn bubble(t: &YkTheme, text: &str, mine: bool, pending: bool) -> AnyElement {
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

fn composer(
    t: &YkTheme,
    state: &MessageState,
    chat: &ChatState,
    cx: &mut Context<AppRoot>,
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

fn initial(nickname: &str) -> String {
    nickname.chars().next().unwrap_or('?').to_string()
}

/// 消息页的轻提示（复制成功 / 已读未同步 / 动态未接入）。
pub(crate) fn notice(t: &YkTheme, text: &str) -> AnyElement {
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

// ================= AppRoot：数据加载 =================

impl AppRoot {
    /// 首次进入消息页：建焦点、拉未读 / 当前通道 / 白名单。
    pub(crate) fn ensure_messages(&mut self, cx: &mut Context<Self>) {
        self.messages.focus_requested = true;
        self.refresh_unread(cx);
        // 互关名单只服务私信准入：它是一份可能分页的长列表，
        // 打开「回复我的」时去拉它纯属浪费。等真的切到私信通道再拉。
        if self.messages.chat.is_none() {
            self.load_current_message_channel(false, cx);
        }
        self.start_messages_poll(cx);
    }

    /// 进入页面时落在第一个有未读的通道；全无未读落「回复我的」。
    pub(crate) fn messages_land_on_first_unread(&mut self, cx: &mut Context<Self>) {
        let channel = self.messages.unread.first_unread_channel();
        self.messages.switch_channel(channel);
        self.messages.focus_requested = true;
        self.messages
            .inbox_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.messages
            .dm_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        if channel == MessageChannel::Dm {
            self.refresh_mutual_whitelist(false, cx);
            self.load_dm_conversations(false, cx);
        } else {
            self.load_inbox(channel, false, cx);
        }
    }

    pub(crate) fn messages_switch_channel(
        &mut self,
        channel: MessageChannel,
        cx: &mut Context<Self>,
    ) {
        self.messages.switch_channel(channel);
        self.messages.focus_requested = true;
        match channel {
            MessageChannel::Dm => {
                self.messages
                    .dm_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                self.refresh_mutual_whitelist(false, cx);
                self.load_dm_conversations(false, cx);
            }
            other => {
                self.messages
                    .inbox_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                self.load_inbox(other, false, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn messages_toggle_only_mutual(&mut self, cx: &mut Context<Self>) {
        self.messages.toggle_only_mutual();
        self.messages
            .dm_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn messages_toggle_only_unread(&mut self, cx: &mut Context<Self>) {
        self.messages.only_unread = !self.messages.only_unread;
        self.messages.selected = None;
        self.messages
            .inbox_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// 刷新当前通道（Dock / 工具条 / 重试共用）。
    pub(crate) fn refresh_messages(&mut self, cx: &mut Context<Self>) {
        self.messages.action_error = None;
        self.refresh_unread(cx);
        if self.messages.chat.is_some() {
            self.load_chat_latest(cx);
            return;
        }
        self.load_current_message_channel(true, cx);
    }

    pub(crate) fn load_current_message_channel(&mut self, force: bool, cx: &mut Context<Self>) {
        match self.messages.channel {
            MessageChannel::Dm => self.load_dm_conversations(force, cx),
            channel => self.load_inbox(channel, force, cx),
        }
    }

    /// 互动通道第一页。`force = false` 时已加载过就不重复请求。
    pub(crate) fn load_inbox(
        &mut self,
        channel: MessageChannel,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let started = {
            let inbox = match channel {
                MessageChannel::Reply => &self.messages.reply,
                MessageChannel::Mention => &self.messages.mention,
                MessageChannel::Like => &self.messages.like,
                MessageChannel::Dm => return,
            };
            !inbox.loaded && !inbox.loading
        };
        if !started && !force {
            return;
        }
        if self.session.is_none() {
            let inbox = match channel {
                MessageChannel::Reply => &mut self.messages.reply,
                MessageChannel::Mention => &mut self.messages.mention,
                MessageChannel::Like => &mut self.messages.like,
                MessageChannel::Dm => return,
            };
            inbox.clear();
            cx.notify();
            return;
        }

        let (generation, first_load) = {
            let inbox = match channel {
                MessageChannel::Reply => &mut self.messages.reply,
                MessageChannel::Mention => &mut self.messages.mention,
                MessageChannel::Like => &mut self.messages.like,
                MessageChannel::Dm => return,
            };
            if inbox.loading {
                return;
            }
            let first_load = !inbox.loaded;
            inbox.generation = inbox.generation.wrapping_add(1);
            inbox.loading = true;
            inbox.error = None;
            inbox.cursor = None;
            (inbox.generation, first_load)
        };
        // 首次进入才重置未读锚点；刷新保留当前滚动位置（设计文档 12.5.4）。
        if first_load {
            self.messages.anchored = false;
            self.messages.selected = None;
        }
        let cookie = self.cookie();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_msgfeed_inbox(&client, channel, None, &cookie)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let inbox = match channel {
                    MessageChannel::Reply => &mut view.messages.reply,
                    MessageChannel::Mention => &mut view.messages.mention,
                    MessageChannel::Like => &mut view.messages.like,
                    MessageChannel::Dm => return,
                };
                if inbox.generation != generation {
                    return;
                }
                inbox.loading = false;
                inbox.loaded = true;
                let mut selected_id = None;
                let read_ids = view.messages.read_ids.clone();
                let unread_loaded = view.messages.unread_loaded;
                let mention_unread = view.messages.unread.mention as usize;
                match result {
                    Ok((items, cursor)) => {
                        eprintln!("[messages] {} 第一页：{} 条", channel.label(), items.len());
                        inbox.has_more = cursor.is_some();
                        inbox.cursor = cursor;
                        inbox.entries = items;
                        for (index, entry) in inbox.entries.iter_mut().enumerate() {
                            if channel == MessageChannel::Mention && unread_loaded {
                                if index < mention_unread && !read_ids.contains(&entry.id) {
                                    entry.read_override = false;
                                    entry.last_view_at = entry.time.saturating_sub(1);
                                } else {
                                    entry.mark_read();
                                }
                            }
                            if read_ids.contains(&entry.id) {
                                entry.mark_read();
                            }
                        }
                        inbox.error = None;
                        // 首屏选中第一条未读；没有未读才落第一行。
                        selected_id = inbox
                            .entries
                            .iter()
                            .find(|entry| entry.is_unread())
                            .or_else(|| inbox.entries.first())
                            .map(|entry| entry.id);
                    }
                    Err(error) => {
                        eprintln!("[messages] {} 加载失败：{error}", channel.label());
                        inbox.error = Some(error);
                    }
                }
                view.messages.selected = selected_id;
                // 首次进入：第一条未读滚到视口（用 Center 近似 1/3）。
                if !view.messages.anchored {
                    view.messages.anchored = true;
                    if let Some(row) = view.messages.first_unread_row() {
                        view.messages
                            .inbox_scroll
                            .scroll_to_item(row, ScrollStrategy::Center);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn messages_load_more(&mut self, cx: &mut Context<Self>) {
        if self.messages.chat.is_some() {
            return;
        }
        match self.messages.channel {
            MessageChannel::Dm => self.load_more_dm(cx),
            channel => self.load_more_inbox(channel, cx),
        }
    }

    fn load_more_inbox(&mut self, channel: MessageChannel, cx: &mut Context<Self>) {
        let (generation, cursor) = {
            let inbox = match channel {
                MessageChannel::Reply => &self.messages.reply,
                MessageChannel::Mention => &self.messages.mention,
                MessageChannel::Like => &self.messages.like,
                MessageChannel::Dm => return,
            };
            if inbox.loading_more || inbox.loading || !inbox.has_more {
                return;
            }
            let Some(cursor) = inbox.cursor else {
                return;
            };
            (inbox.generation, cursor)
        };
        let inbox = match channel {
            MessageChannel::Reply => &mut self.messages.reply,
            MessageChannel::Mention => &mut self.messages.mention,
            MessageChannel::Like => &mut self.messages.like,
            MessageChannel::Dm => return,
        };
        inbox.loading_more = true;
        let cookie = self.cookie();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_msgfeed_inbox(&client, channel, Some(cursor), &cookie)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let inbox = match channel {
                    MessageChannel::Reply => &mut view.messages.reply,
                    MessageChannel::Mention => &mut view.messages.mention,
                    MessageChannel::Like => &mut view.messages.like,
                    MessageChannel::Dm => return,
                };
                if inbox.generation != generation {
                    return;
                }
                let read_ids = view.messages.read_ids.clone();
                inbox.loading_more = false;
                match result {
                    Ok((items, next)) => {
                        let before = inbox.entries.len();
                        for mut item in items {
                            if read_ids.contains(&item.id) {
                                item.mark_read();
                            }
                            let duplicate = inbox.entries.iter().any(|existing| {
                                existing.id == item.id && existing.user.mid == item.user.mid
                            });
                            if !duplicate {
                                inbox.entries.push(item);
                            }
                        }
                        // 赞聚合必须跨页：同一个 24h 窗口可能被分页切开。
                        if channel == MessageChannel::Like {
                            inbox.entries = aggregate_likes(std::mem::take(&mut inbox.entries));
                        }
                        eprintln!(
                            "[messages] {} 追加：{} 条（{} → {}）",
                            channel.label(),
                            inbox.entries.len() - before,
                            before,
                            inbox.entries.len()
                        );
                        inbox.has_more = next.is_some();
                        inbox.cursor = next;
                    }
                    Err(error) => {
                        eprintln!("[messages] {} 加载更多失败：{error}", channel.label());
                        view.messages.action_error = Some(friendly_error(&error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 私信会话第一页（或强制刷新）。昵称 / 头像由 user cards 补。
    pub(crate) fn load_dm_conversations(&mut self, force: bool, cx: &mut Context<Self>) {
        if !force && (self.messages.dm.loaded || self.messages.dm.loading) {
            return;
        }
        if self.session.is_none() {
            self.messages.dm.conversations.clear();
            self.messages.dm.loaded = true;
            self.messages.dm.error = None;
            cx.notify();
            return;
        }
        if self.messages.dm.loading {
            return;
        }
        self.messages.dm.generation = self.messages.dm.generation.wrapping_add(1);
        self.messages.dm.loading = true;
        self.messages.dm.error = None;
        self.messages.dm.begin_ts = None;
        self.messages.selected = None;
        let generation = self.messages.dm.generation;
        let cookie = self.cookie();
        let my_mid = self
            .session
            .as_ref()
            .map(|session| session.dede_user_id)
            .unwrap_or(0);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_conversations(&client, &cookie, my_mid, None)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.messages.dm.generation != generation {
                    return;
                }
                view.messages.dm.loading = false;
                view.messages.dm.loaded = true;
                match result {
                    Ok((conversations, next)) => {
                        eprintln!("[messages] 私信会话：{} 个", conversations.len());
                        view.messages.dm.conversations = conversations;
                        view.messages.dm.has_more = next.is_some();
                        view.messages.dm.begin_ts = next;
                        view.messages.dm.error = None;
                        view.messages.apply_whitelist();
                        view.messages.unread.dm = view.messages.dm_unread_total();
                        view.messages.ensure_dm_selection();
                    }
                    Err(error) => {
                        eprintln!("[messages] 私信会话失败：{error}");
                        view.messages.dm.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_more_dm(&mut self, cx: &mut Context<Self>) {
        if self.messages.dm.loading_more || self.messages.dm.loading || !self.messages.dm.has_more {
            return;
        }
        let Some(begin_ts) = self.messages.dm.begin_ts else {
            return;
        };
        let generation = self.messages.dm.generation;
        self.messages.dm.loading_more = true;
        let cookie = self.cookie();
        let my_mid = self
            .session
            .as_ref()
            .map(|session| session.dede_user_id)
            .unwrap_or(0);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_conversations(&client, &cookie, my_mid, Some(begin_ts))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.messages.dm.generation != generation {
                    return;
                }
                view.messages.dm.loading_more = false;
                match result {
                    Ok((conversations, next)) => {
                        for conversation in conversations {
                            if !view
                                .messages
                                .dm
                                .conversations
                                .iter()
                                .any(|existing| existing.talker_id == conversation.talker_id)
                            {
                                view.messages.dm.conversations.push(conversation);
                            }
                        }
                        view.messages.dm.has_more = next.is_some();
                        view.messages.dm.begin_ts = next;
                        view.messages.apply_whitelist();
                        view.messages.unread.dm = view.messages.dm_unread_total();
                    }
                    Err(error) => {
                        view.messages.action_error = Some(friendly_error(&error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 互关白名单：5 分钟 TTL（E-24）。私信页进门时刷新，打开会话再单条校验。
    pub(crate) fn refresh_mutual_whitelist(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.messages.whitelist_loading {
            return;
        }
        if !force && self.messages.whitelist_loaded {
            if let Some(checked) = self.messages.whitelist_checked_at {
                if checked.elapsed().as_secs() < MUTUAL_TTL_SECS {
                    return;
                }
            }
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        self.messages.whitelist_loading = true;
        let mid = session.dede_user_id;
        let cookie = session.cookie_header();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_mutual_ids(&client, &cookie, mid)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.messages.whitelist_loading = false;
                match result {
                    Ok(ids) => {
                        eprintln!("[messages] 互关白名单：{} 人", ids.len());
                        view.messages.whitelist = ids;
                        view.messages.whitelist_loaded = true;
                        view.messages.apply_whitelist();
                    }
                    Err(error) => {
                        eprintln!("[messages] 互关白名单失败：{error}");
                    }
                }
                view.messages.whitelist_checked_at = Some(std::time::Instant::now());
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开会话时单条校验互关：6 = 互粉、128 = 拉黑，其余保留白名单结论。
    fn verify_conversation_relation(&mut self, talker_id: i64, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let cookie = session.cookie_header();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_relation_attribute(&client, &cookie, talker_id)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let Ok(attribute) = result else {
                    return;
                };
                let mutual = if attribute == 6 {
                    Some(true)
                } else if attribute == 128 {
                    Some(false)
                } else if view.messages.whitelist_loaded {
                    Some(view.messages.whitelist.contains(&talker_id))
                } else {
                    view.messages
                        .chat
                        .as_ref()
                        .filter(|chat| chat.talker_id == talker_id)
                        .and_then(|chat| chat.mutual)
                };
                if let Some(chat) = view.messages.chat.as_mut() {
                    if chat.talker_id == talker_id {
                        chat.mutual = mutual;
                    }
                }
                for conversation in &mut view.messages.dm.conversations {
                    if conversation.talker_id == talker_id {
                        conversation.mutual = mutual;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 未读数：顶栏 / Dock 红点与分段角标共用。只有数字变化才 notify。
    pub(crate) fn refresh_unread(&mut self, cx: &mut Context<Self>) {
        if self.session.is_none() || self.messages.unread_loading {
            return;
        }
        self.messages.unread_loading = true;
        let cookie = self.cookie();
        // 顶栏 / Dock 的私信部分单独算：会话列表可能还没加载，
        // 也可能停在别的通道上没刷新；失败就退回本地列表之和。
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    let counts = fetch_msgfeed_unread(&client, &cookie)?;
                    let dm = fetch_dm_unread_total(&client, &cookie).ok();
                    Ok::<(UnreadCounts, Option<u64>), String>((counts, dm))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.messages.unread_loading = false;
                if let Ok((mut counts, dm)) = result {
                    counts.dm = dm.unwrap_or_else(|| view.messages.dm_unread_total());
                    let changed = counts != view.messages.unread;
                    if changed {
                        eprintln!(
                            "[messages] 未读：回复 {} / @ {} / 赞 {} / 私信 {}",
                            counts.reply, counts.mention, counts.like, counts.dm
                        );
                    }
                    view.messages.unread = counts;
                    view.messages.unread_loaded = true;
                    // @ 通道靠总数推断未读；数字到了要回头重算一次。
                    view.messages.reconcile_mention_unread();
                    if changed {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// 进页 30s 轮询；离开页面不请求（窗口失焦也暂停）。
    fn start_messages_poll(&mut self, cx: &mut Context<Self>) {
        if self.messages.poll_started {
            return;
        }
        self.messages.poll_started = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let alive = this.update(cx, |view, cx| {
                if view.page != Some(crate::pages::PageKind::Messages)
                    || view.messages.chat.is_some()
                    || !view.messages.window_active
                    || view.session.is_none()
                {
                    return;
                }
                view.refresh_unread(cx);
                if view.messages.channel == MessageChannel::Dm {
                    // 会话列表的未读在列表里，重新拉一次才能更新角标。
                    view.load_dm_conversations(true, cx);
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }

    // ---- 条目动作 ----

    pub(crate) fn messages_mark_read(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(entry) = self
            .messages
            .entries_mut()
            .iter_mut()
            .find(|entry| entry.id == id)
        else {
            return;
        };
        if !entry.is_unread() {
            return;
        }
        entry.mark_read();
        let channel = entry.channel;
        // E-26：msgfeed 没有可核实的单条已读接口。本地覆盖 + 角标 −1，
        // 不立刻回打 unread（服务端还没变，打回来只会把数字又弹回去）。
        self.messages.read_ids.insert(id);
        let now = self.messages.unread.channel(channel).saturating_sub(1);
        self.messages.unread.set_channel(channel, now);
        cx.notify();
    }

    pub(crate) fn messages_copy_link(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(uri) = self
            .messages
            .entries()
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.uri.clone())
        else {
            return;
        };
        if uri.trim().is_empty() {
            self.message_notice("这条消息没有可复制的链接", cx);
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(uri));
        self.message_notice("已复制", cx);
    }

    /// 打开条目：标为已读 + 按 `uri` 走自建路由（不解析 native_uri）。
    pub(crate) fn messages_open_entry(&mut self, id: i64, cx: &mut Context<Self>) {
        self.messages_mark_read(id, cx);
        let Some(entry) = self
            .messages
            .entries()
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
        else {
            return;
        };
        match entry.target.clone() {
            MessageTarget::Video { bvid } => {
                eprintln!("[messages] 跳转视频：{bvid}");
                self.ensure_player(cx);
                if let Some(player) = self.playing.clone() {
                    player.update(cx, |view, cx| view.open_bvid(bvid, cx));
                }
            }
            MessageTarget::Dynamic { id: _ } => {
                // 动态详情按 id 打开还没有接（现有详情只接受 feed 实体）。
                // 不假装成功：把链接交给用户，并明确说明页面未接入。
                let uri = entry.uri.clone();
                if !uri.trim().is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(uri));
                }
                self.message_notice("动态详情页暂未接入，链接已复制", cx);
            }
            MessageTarget::Unknown => {
                if entry.target_missing() {
                    self.message_notice("原内容已删除或不可见", cx);
                } else {
                    self.message_notice("只能标记已读，目标链接暂不支持站内跳转", cx);
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn messages_toggle_like(&mut self, id: i64, cx: &mut Context<Self>) {
        self.messages.expanded_like = if self.messages.expanded_like == Some(id) {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }

    /// 键盘 j/k、↑/↓：按可见（含「只看未读」）条目移动选中。
    pub(crate) fn messages_move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.messages.channel == MessageChannel::Dm {
            let visible = self.messages.visible_conversation_indices();
            let len = visible.len();
            if len == 0 {
                return;
            }
            let current = self
                .messages
                .selected
                .and_then(|id| {
                    visible
                        .iter()
                        .position(|index| self.messages.dm.conversations[*index].talker_id == id)
                })
                .map(|index| index as i32)
                .unwrap_or(-1);
            let next = (current + delta).clamp(0, len as i32 - 1) as usize;
            let id = self.messages.dm.conversations[visible[next]].talker_id;
            self.messages.selected = Some(id);
            self.messages
                .dm_scroll
                .scroll_to_item(next, ScrollStrategy::Top);
        } else {
            let visible = self.messages.visible_indices();
            if visible.is_empty() {
                return;
            }
            let current = self
                .messages
                .selected
                .and_then(|id| {
                    visible
                        .iter()
                        .position(|index| self.messages.entries()[*index].id == id)
                })
                .map(|index| index as i32)
                .unwrap_or(-1);
            let next = (current + delta).clamp(0, visible.len() as i32 - 1) as usize;
            let id = self.messages.entries()[visible[next]].id;
            self.messages.selected = Some(id);
            if let Some(row) = self.messages.row_of_entry(id) {
                self.messages
                    .inbox_scroll
                    .scroll_to_item(row, ScrollStrategy::Top);
            }
        }
        cx.notify();
    }

    // ---- 私信会话 ----

    pub(crate) fn messages_open_conversation(
        &mut self,
        talker_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = self
            .messages
            .dm
            .conversations
            .iter()
            .find(|conversation| conversation.talker_id == talker_id)
            .cloned()
        else {
            return;
        };
        self.messages.selected = Some(talker_id);
        self.messages.chat = Some(ChatState::new(&conversation));
        self.messages.action_error = None;
        self.messages.composer_draft.clear();
        self.ensure_chat_composer(window, cx);
        if let Some(input) = self.messages.composer.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        self.load_chat_latest(cx);
        self.mark_conversation_locally_read(talker_id);
        self.mark_conversation_read_background(talker_id, 0, cx);
        self.verify_conversation_relation(talker_id, cx);
        cx.notify();
    }

    pub(crate) fn messages_close_conversation(&mut self, cx: &mut Context<Self>) {
        self.messages.chat = None;
        self.messages.composer_draft.clear();
        self.messages.focus_requested = true;
        self.messages.action_error = None;
        self.messages
            .dm_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// 拉最新一页聊天记录；成功后滚到底（只滚一次）。
    pub(crate) fn load_chat_latest(&mut self, cx: &mut Context<Self>) {
        let Some((talker_id, generation)) = self.messages.chat.as_mut().map(|chat| {
            // 每次重拉都 +1：旧请求回来时（尤其重新打开同一个会话）会被丢弃。
            chat.generation = chat.generation.wrapping_add(1);
            chat.loading = true;
            chat.error = None;
            chat.pending_bottom = true;
            (chat.talker_id, chat.generation)
        }) else {
            return;
        };
        let cookie = self.cookie();
        let my_mid = self
            .session
            .as_ref()
            .map(|session| session.dede_user_id)
            .unwrap_or(0);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_chat_messages(&client, &cookie, talker_id, my_mid, None)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let Some(chat) = view.messages.chat.as_mut() else {
                    return;
                };
                if chat.generation != generation || chat.talker_id != talker_id {
                    return;
                }
                chat.loading = false;
                match result {
                    Ok((mut messages, has_more, begin_seqno)) => {
                        messages.sort_by_key(|message| message.seqno);
                        merge_chat(&mut chat.messages, messages);
                        chat.has_more = has_more;
                        chat.begin_seqno = begin_seqno;
                        chat.error = None;
                        chat.pending_bottom = true;
                    }
                    Err(error) => {
                        eprintln!("[messages] 聊天记录失败：{error}");
                        chat.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn messages_load_older_chat(&mut self, cx: &mut Context<Self>) {
        let Some((talker_id, generation, begin_seqno)) =
            self.messages.chat.as_ref().and_then(|chat| {
                (chat.has_more && !chat.loading_more && chat.begin_seqno > 0).then_some((
                    chat.talker_id,
                    chat.generation,
                    chat.begin_seqno,
                ))
            })
        else {
            return;
        };
        if let Some(chat) = self.messages.chat.as_mut() {
            chat.loading_more = true;
        }
        let cookie = self.cookie();
        let my_mid = self
            .session
            .as_ref()
            .map(|session| session.dede_user_id)
            .unwrap_or(0);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    fetch_chat_messages(&client, &cookie, talker_id, my_mid, Some(begin_seqno))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let Some(chat) = view.messages.chat.as_mut() else {
                    return;
                };
                if chat.generation != generation {
                    return;
                }
                chat.loading_more = false;
                match result {
                    Ok((messages, has_more, begin_seqno)) => {
                        merge_chat(&mut chat.messages, messages);
                        chat.has_more = has_more;
                        chat.begin_seqno = begin_seqno;
                    }
                    Err(error) => {
                        chat.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn messages_send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((talker_id, generation, can_send, sending)) =
            self.messages.chat.as_ref().map(|chat| {
                (
                    chat.talker_id,
                    chat.generation,
                    chat.can_send(),
                    chat.sending,
                )
            })
        else {
            return;
        };
        if !can_send || sending {
            return;
        }
        let text = self.messages.composer_draft.clone();
        if text.trim().is_empty() {
            return;
        }
        if text.len() > DM_MAX_BYTES {
            self.action_error("消息超过 2000 字节，请缩短后再发送", cx);
            return;
        }
        let Some(session) = self.session.clone() else {
            self.action_error("登录态已失效，请重新登录", cx);
            return;
        };
        if let Some(chat) = self.messages.chat.as_mut() {
            chat.sending = true;
            chat.pending = Some(text.clone());
            chat.failed = None;
            chat.pending_bottom = true;
        }
        if let Some(input) = self.messages.composer.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.messages.composer_draft.clear();
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let my_mid = session.dede_user_id;
        let keys = session
            .img_key
            .clone()
            .zip(session.sub_key.clone())
            .filter(|(img, sub)| !img.is_empty() && !sub.is_empty());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let send_result = cx
                .background_executor()
                .spawn({
                    let text = text.clone();
                    async move {
                        let client = ReqwestClient::new()?;
                        send_dm(&client, &cookie, &csrf, my_mid, talker_id, &text, keys)?;
                        let client = ReqwestClient::new()?;
                        fetch_chat_messages(&client, &cookie, talker_id, my_mid, None)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let Some(chat) = view.messages.chat.as_mut() else {
                    return;
                };
                if chat.generation != generation || chat.talker_id != talker_id {
                    return;
                }
                chat.sending = false;
                match send_result {
                    Ok((mut messages, has_more, begin_seqno)) => {
                        chat.pending = None;
                        chat.failed = None;
                        messages.sort_by_key(|message| message.seqno);
                        merge_chat(&mut chat.messages, messages);
                        chat.has_more = has_more;
                        chat.begin_seqno = begin_seqno;
                        chat.pending_bottom = true;
                        // 发送成功顺带把会话置已读并置顶。
                        view.mark_conversation_locally_read(talker_id);
                        view.mark_conversation_read_background(talker_id, 0, cx);
                    }
                    Err(error) => {
                        eprintln!("[messages] 发送失败：{error}");
                        chat.pending = None;
                        chat.failed = Some(text.clone());
                        chat.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn messages_resend_failed(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.messages.chat.as_mut() else {
            return;
        };
        chat.failed = None;
        if let Some(input) = self.messages.composer.clone() {
            input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        }
        self.messages.composer_draft = text;
        self.messages_send_chat(window, cx);
    }

    pub(crate) fn messages_mark_chat_read(&mut self, cx: &mut Context<Self>) {
        let Some(talker_id) = self.messages.chat.as_ref().map(|chat| chat.talker_id) else {
            return;
        };
        self.mark_conversation_read_background(talker_id, 0, cx);
    }

    /// 单会话标已读：先乐观清列表角标，接口失败时提示「已读状态未同步」。
    pub(crate) fn messages_mark_conversation_read(
        &mut self,
        talker_id: i64,
        cx: &mut Context<Self>,
    ) {
        self.mark_conversation_locally_read(talker_id);
        self.mark_conversation_read_background(talker_id, 0, cx);
        cx.notify();
    }

    fn mark_conversation_locally_read(&mut self, talker_id: i64) {
        let mut removed = 0;
        for conversation in &mut self.messages.dm.conversations {
            if conversation.talker_id == talker_id {
                removed = conversation.unread;
                conversation.unread = 0;
            }
        }
        self.messages.unread.dm = self.messages.unread.dm.saturating_sub(removed);
    }

    fn mark_conversation_read_background(
        &mut self,
        talker_id: i64,
        unread_before: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        self.messages.unread.dm = self.messages.unread.dm.saturating_sub(unread_before);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    mark_conversation_read(&client, &cookie, &csrf, talker_id)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Err(error) = result {
                    eprintln!("[messages] 标已读失败：{error}");
                    view.message_notice("已读状态未同步", cx);
                    // 失败以服务端为准：重新拉一次未读与会话。
                    view.refresh_unread(cx);
                    view.load_dm_conversations(true, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 全部已读：串行 update_ack，间隔 200ms，部分失败不中断。
    pub(crate) fn messages_mark_all_dm_read(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let targets: Vec<i64> = self
            .messages
            .dm
            .conversations
            .iter()
            .filter(|conversation| conversation.unread > 0)
            .map(|conversation| conversation.talker_id)
            .collect();
        if targets.is_empty() {
            self.message_notice("没有未读会话", cx);
            return;
        }
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    let mut ok = 0usize;
                    let mut failed = 0usize;
                    for (index, talker_id) in targets.iter().enumerate() {
                        if index > 0 {
                            // 串行 + 200ms 间隔：并发打会触发 -352。
                            std::thread::sleep(Duration::from_millis(200));
                        }
                        match mark_conversation_read(&client, &cookie, &csrf, *talker_id) {
                            Ok(()) => ok += 1,
                            Err(error) => {
                                eprintln!("[messages] 批量已读失败（{talker_id}）：{error}");
                                failed += 1;
                            }
                        }
                    }
                    Ok::<(usize, usize), String>((ok, failed))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok((ok, failed)) => {
                        for conversation in &mut view.messages.dm.conversations {
                            conversation.unread = 0;
                        }
                        view.messages.unread.dm = 0;
                        let text = if failed == 0 {
                            format!("已标记 {ok} 个会话")
                        } else {
                            format!("已标记 {ok} 个会话，{failed} 个失败")
                        };
                        view.message_notice(&text, cx);
                        if failed > 0 {
                            // 失败的那些不能本地清零：重拉会话列表，以服务端为准。
                            view.load_dm_conversations(true, cx);
                        }
                    }
                    Err(error) => {
                        view.message_notice(&friendly_error(&error), cx);
                    }
                }
                view.refresh_unread(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn messages_ask_remove_conversation(
        &mut self,
        talker_id: i64,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = self
            .messages
            .dm
            .conversations
            .iter()
            .find(|conversation| conversation.talker_id == talker_id)
        else {
            return;
        };
        self.messages.confirm = Some(RemoveConfirm {
            talker_id,
            nickname: conversation.nickname.clone(),
        });
        self.messages.confirm_busy = false;
        self.messages.action_error = None;
        cx.notify();
    }

    pub(crate) fn messages_ask_remove_chat(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.messages.chat.as_ref() else {
            return;
        };
        self.messages.confirm = Some(RemoveConfirm {
            talker_id: chat.talker_id,
            nickname: chat.nickname.clone(),
        });
        self.messages.confirm_busy = false;
        self.messages.action_error = None;
        cx.notify();
    }

    pub(crate) fn messages_confirm_remove(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.messages.confirm.clone() else {
            return;
        };
        if self.messages.confirm_busy {
            return;
        }
        let Some(session) = self.session.clone() else {
            self.messages.action_error = Some("登录态已失效，请重新登录".into());
            cx.notify();
            return;
        };
        self.messages.confirm_busy = true;
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let talker_id = confirm.talker_id;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    remove_conversation(&client, &cookie, &csrf, talker_id)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.messages.confirm_busy = false;
                match result {
                    Ok(()) => {
                        view.messages
                            .dm
                            .conversations
                            .retain(|conversation| conversation.talker_id != talker_id);
                        view.messages.confirm = None;
                        if view.messages.chat.as_ref().map(|chat| chat.talker_id) == Some(talker_id)
                        {
                            view.messages.chat = None;
                        }
                        view.messages.error_clear();
                        view.message_notice("会话已移除", cx);
                    }
                    Err(error) => {
                        // 失败保留条目，不做本地假删除。
                        view.messages.action_error = Some(friendly_error(&error));
                    }
                }
                view.refresh_unread(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn messages_cancel_remove(&mut self, cx: &mut Context<Self>) {
        self.messages.confirm = None;
        self.messages.action_error = None;
        cx.notify();
    }

    pub(crate) fn message_notice(&mut self, text: &str, cx: &mut Context<Self>) {
        self.messages.notice = Some(text.to_string());
        self.messages.notice_generation = self.messages.notice_generation.wrapping_add(1);
        let generation = self.messages.notice_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2400))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.messages.notice_generation == generation {
                    view.messages.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn action_error(&mut self, text: &str, cx: &mut Context<Self>) {
        self.messages.action_error = Some(text.to_string());
        cx.notify();
    }

    // ---- 首帧准备 ----

    /// 消息页每帧 render 前：补焦点、补输入框、处理延迟滚动。
    pub(crate) fn messages_prepare_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.messages.focus.is_none() {
            self.messages.focus = Some(cx.focus_handle().tab_stop(true));
        }
        if self.messages.chat.is_some() {
            self.ensure_chat_composer(window, cx);
        }
        if std::mem::take(&mut self.messages.send_requested) {
            self.messages_send_chat(window, cx);
        }
        if self.messages.focus_requested {
            self.messages.focus_requested = false;
            if let Some(handle) = self.messages.focus.clone() {
                cx.defer_in(window, move |_view, window, cx| {
                    window.focus(&handle, cx);
                });
            }
        }
        let bottom = self
            .messages
            .chat
            .as_mut()
            .map(|chat| std::mem::take(&mut chat.pending_bottom))
            .unwrap_or(false);
        if bottom {
            if let Some(chat) = self.messages.chat.as_ref() {
                chat.scroll.scroll_to_bottom();
                // 这一帧的内容高度还没算出来，下一帧（deferred）再滚一次才真正到底。
                let handle = chat.scroll.clone();
                cx.defer_in(window, move |_view, _window, cx| {
                    handle.scroll_to_bottom();
                    cx.notify();
                });
            }
        }
    }

    pub(crate) fn ensure_chat_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.messages.composer.is_some() {
            return;
        }
        let state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("发消息…")
                .submit_on_enter(true)
                .auto_grow(1, 3)
        });
        cx.subscribe(&state, |view, state, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                view.messages.composer_draft = state.read(cx).value().to_string();
                cx.notify();
            }
            InputEvent::PressEnter { shift, .. } if !shift => {
                // 订阅回调里拿不到 Window；置位后由下一帧 render 发送。
                view.messages.send_requested = true;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        self.messages.composer = Some(state);
    }
}

/// 把新拉到的消息按 seqno 合并进现有列表（去重、升序）。
fn merge_chat(existing: &mut Vec<ChatMessage>, incoming: Vec<ChatMessage>) {
    for message in incoming {
        if message.seqno <= 0 {
            continue;
        }
        match existing.binary_search_by_key(&message.seqno, |item| item.seqno) {
            Ok(_) => {}
            Err(index) => existing.insert(index, message),
        }
    }
}

/// 错误文案统一口径：-101 登录态失效；-352 风控；其余保留服务端原因。
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

/// 消息页的模态确认框（移除会话）。
pub(crate) fn confirm_overlay(
    t: &YkTheme,
    confirm: &RemoveConfirm,
    busy: bool,
    error: Option<&str>,
    cx: &mut Context<AppRoot>,
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

// MessageState 的两个小扩展：避免在调用点写一长串字段路径。
impl MessageState {
    fn entries_mut(&mut self) -> &mut Vec<MessageEntry> {
        &mut self.inbox_mut().entries
    }

    fn error_clear(&mut self) {
        self.action_error = None;
        self.inbox_mut().error = None;
        self.dm.error = None;
        if let Some(chat) = self.chat.as_mut() {
            chat.error = None;
        }
    }
}
