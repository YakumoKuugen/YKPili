//! 消息中心状态：互动通道、私信会话、未读与选择。
//!
//! 只放状态与纯计算，不依赖 `AppRoot`；加载、轮询、发送仍由组装点负责。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::component::input::{InputEvent, TextareaState};
use gpui::AppContext as _;
use gpui::component::VirtualListScrollHandle;
use gpui::{ClipboardItem, Entity, FocusHandle, ScrollHandle, ScrollStrategy};
use yk_infra::api::client::{
    fetch_chat_messages, fetch_conversations, fetch_dm_unread_total,
    fetch_msgfeed_inbox, fetch_msgfeed_unread, fetch_mutual_ids, fetch_relation_attribute,
    mark_conversation_read, remove_conversation, send_dm,
    ReqwestClient,
};
use yk_infra::messages::{aggregate_likes, MessageTarget, DM_MAX_BYTES, MUTUAL_TTL_SECS, 
    ChatMessage, Conversation, InboxCursor, MessageChannel, MessageEntry, UnreadCounts,
};

/// 互动条目固定行高（--s12-inbox-row-h）。
pub const INBOX_ROW_H: f32 = 116.0;
/// 私信会话固定行高（--s12-msg-row-h）。
pub const DM_ROW_H: f32 = 72.0;
/// 未读分隔线高。
pub const UNREAD_DIVIDER_H: f32 = 28.0;
/// 赞展开后每一行的高。
pub const LIKE_DETAIL_H: f32 = 44.0;
/// 列表底部「加载中 / 没有更多了」。
pub const FOOTER_H: f32 = 44.0;
/// 内容列最大宽（--s12-inbox-w）。960 ≈ 70 个汉字。
pub const CONTENT_MAX_W: f32 = 960.0;
/// 私信气泡最大宽（--s12-bubble-w）。
pub const BUBBLE_MAX_W: f32 = 320.0;
/// 私信输入区高（--s12-composer-h）。
pub const COMPOSER_H: f32 = 56.0;
/// 消息页自己的轮询间隔（进页 30s）。
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

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

impl MessageState {
    pub fn entries_mut(&mut self) -> &mut Vec<MessageEntry> {
        &mut self.inbox_mut().entries
    }

    pub fn error_clear(&mut self) {
        self.action_error = None;
        self.inbox_mut().error = None;
        self.dm.error = None;
        if let Some(chat) = self.chat.as_mut() {
            chat.error = None;
        }
    }
}

/// 消息页需要宿主提供的动作与状态访问。
pub trait MessagesHost: crate::card::CardHost + gpui::Render + Sized {
    fn messages(&self) -> &MessageState;
    fn messages_mut(&mut self) -> &mut MessageState;
    fn messages_refresh_unread(&mut self, cx: &mut gpui::Context<Self>) {
        if self.messages_session().is_none() || self.messages_mut().unread_loading {
            return;
        }
        self.messages_mut().unread_loading = true;
        let cookie = self.messages_cookie();
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
                view.messages_mut().unread_loading = false;
                if let Ok((mut counts, dm)) = result {
                    counts.dm = dm.unwrap_or_else(|| view.messages_mut().dm_unread_total());
                    let changed = counts != view.messages_mut().unread;
                    if changed {
                        eprintln!(
                            "[messages] 未读：回复 {} / @ {} / 赞 {} / 私信 {}",
                            counts.reply, counts.mention, counts.like, counts.dm
                        );
                    }
                    view.messages_mut().unread = counts;
                    view.messages_mut().unread_loaded = true;
                    // @ 通道靠总数推断未读；数字到了要回头重算一次。
                    view.messages_mut().reconcile_mention_unread();
                    if changed {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    fn messages_notice(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        self.messages_mut().notice = Some(text.to_string());
        self.messages_mut().notice_generation = self.messages_mut().notice_generation.wrapping_add(1);
        let generation = self.messages_mut().notice_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2400))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.messages_mut().notice_generation == generation {
                    view.messages_mut().notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn messages_load_dm(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        if !force && (self.messages().dm.loaded || self.messages().dm.loading) {
            return;
        }
        if self.messages_session().is_none() {
            let state = self.messages_mut();
            state.dm.conversations.clear();
            state.dm.loaded = true;
            state.dm.error = None;
            cx.notify();
            return;
        }
        if self.messages().dm.loading {
            return;
        }
        let generation = {
            let state = self.messages_mut();
            state.dm.generation = state.dm.generation.wrapping_add(1);
            state.dm.loading = true;
            state.dm.error = None;
            state.dm.begin_ts = None;
            state.selected = None;
            state.dm.generation
        };
        let cookie = self.messages_cookie();
        let my_mid = self
            .messages_session()
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
                if view.messages().dm.generation != generation {
                    return;
                }
                {
                    let state = view.messages_mut();
                    state.dm.loading = false;
                    state.dm.loaded = true;
                }
                match result {
                    Ok((conversations, next)) => {
                        let state = view.messages_mut();
                        state.dm.conversations = conversations;
                        state.dm.has_more = next.is_some();
                        state.dm.begin_ts = next;
                        state.dm.error = None;
                        state.apply_whitelist();
                        let dm_total = state.dm_unread_total();
                        state.unread.dm = dm_total;
                        state.ensure_dm_selection();
                    }
                    Err(error) => {
                        view.messages_mut().dm.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn messages_mark_read_background(
        &mut self,
        talker_id: i64,
        unread_before: u64,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(session) = self.messages_session() else {
            return;
        };
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        self.messages_mut().unread.dm = self.messages_mut().unread.dm.saturating_sub(unread_before);
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
                    view.messages_notice("已读状态未同步", cx);
                    // 失败以服务端为准：重新拉一次未读与会话。
                    view.messages_refresh_unread(cx);
                    view.messages_load_dm(true, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn messages_mark_read_local(&mut self, talker_id: i64) {
        let state = self.messages_mut();
        let mut removed = 0;
        for conversation in &mut state.dm.conversations {
            if conversation.talker_id == talker_id {
                removed = conversation.unread;
                conversation.unread = 0;
            }
        }
        state.unread.dm = state.unread.dm.saturating_sub(removed);
    }
    fn messages_session(&self) -> Option<yk_domain::models::UserSession>;
    fn messages_cookie(&self) -> String;

    fn messages_action_error(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        self.messages_mut().action_error = Some(text.to_string());
        cx.notify();
    }
    fn messages_switch_channel(
        &mut self,
        channel: MessageChannel,
        cx: &mut gpui::Context<Self>,
    ) {
        self.messages_mut().switch_channel(channel);
        self.messages_mut().focus_requested = true;
        match channel {
            MessageChannel::Dm => {
                self.messages_mut()
                    .dm_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                self.messages_refresh_whitelist(false, cx);
                self.messages_load_dm(false, cx);
            }
            other => {
                self.messages_mut()
                    .inbox_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                self.messages_load_inbox(other, false, cx);
            }
        }
        cx.notify();
    }
    fn messages_refresh(&mut self, cx: &mut gpui::Context<Self>) {
        self.messages_mut().action_error = None;
        self.messages_refresh_unread(cx);
        if self.messages_mut().chat.is_some() {
            self.messages_load_chat_latest(cx);
            return;
        }
        self.messages_load_channel(true, cx);
    }
    fn messages_go_following(&mut self, cx: &mut gpui::Context<Self>);
    fn messages_toggle_only_unread(&mut self, cx: &mut gpui::Context<Self>) {
        let state = self.messages_mut();
        state.only_unread = !state.only_unread;
        state.selected = None;
        state.inbox_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }
    fn messages_toggle_only_mutual(&mut self, cx: &mut gpui::Context<Self>) {
        self.messages_mut().toggle_only_mutual();
        self.messages_mut().dm_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }
    fn messages_mark_all_dm_read(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(session) = self.messages_session() else {
            return;
        };
        let targets: Vec<i64> = self
            .messages_mut()
            .dm
            .conversations
            .iter()
            .filter(|conversation| conversation.unread > 0)
            .map(|conversation| conversation.talker_id)
            .collect();
        if targets.is_empty() {
            self.messages_notice("没有未读会话", cx);
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
                        for conversation in &mut view.messages_mut().dm.conversations {
                            conversation.unread = 0;
                        }
                        view.messages_mut().unread.dm = 0;
                        let text = if failed == 0 {
                            format!("已标记 {ok} 个会话")
                        } else {
                            format!("已标记 {ok} 个会话，{failed} 个失败")
                        };
                        view.messages_notice(&text, cx);
                        if failed > 0 {
                            // 失败的那些不能本地清零：重拉会话列表，以服务端为准。
                            view.messages_load_dm(true, cx);
                        }
                    }
                    Err(error) => {
                        view.messages_notice(&crate::messages_ui::friendly_error(&error), cx);
                    }
                }
                view.messages_refresh_unread(cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn messages_load_more(&mut self, cx: &mut gpui::Context<Self>) {
        if self.messages().chat.is_some() {
            return;
        }
        match self.messages().channel {
            MessageChannel::Dm => self.load_more_dm(cx),
            channel => self.load_more_inbox(channel, cx),
        }
    }

    fn load_more_inbox(&mut self, channel: MessageChannel, cx: &mut gpui::Context<Self>) {
        let (generation, cursor) = {
            let state = self.messages();
            let inbox = match channel {
                MessageChannel::Reply => &state.reply,
                MessageChannel::Mention => &state.mention,
                MessageChannel::Like => &state.like,
                MessageChannel::Dm => return,
            };
            if inbox.loading_more || inbox.loading || !inbox.has_more {
                return;
            }
            let Some(cursor) = inbox.cursor.clone() else {
                return;
            };
            (inbox.generation, cursor)
        };
        {
            let state = self.messages_mut();
            match channel {
                MessageChannel::Reply => state.reply.loading_more = true,
                MessageChannel::Mention => state.mention.loading_more = true,
                MessageChannel::Like => state.like.loading_more = true,
                MessageChannel::Dm => return,
            }
        }
        let cookie = self.messages_cookie();
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
                let read_ids = view.messages().read_ids.clone();
                let error_text = {
                    let state = view.messages_mut();
                    let inbox = match channel {
                        MessageChannel::Reply => &mut state.reply,
                        MessageChannel::Mention => &mut state.mention,
                        MessageChannel::Like => &mut state.like,
                        MessageChannel::Dm => return,
                    };
                    if inbox.generation != generation {
                        return;
                    }
                    inbox.loading_more = false;
                    match result {
                        Ok((items, next)) => {
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
                            if channel == MessageChannel::Like {
                                inbox.entries = aggregate_likes(std::mem::take(&mut inbox.entries));
                            }
                            inbox.has_more = next.is_some();
                            inbox.cursor = next;
                            None
                        }
                        Err(error) => Some(crate::messages_ui::friendly_error(&error)),
                    }
                };
                if let Some(text) = error_text {
                    view.messages_mut().action_error = Some(text);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn messages_mark_read(&mut self, id: i64, cx: &mut gpui::Context<Self>) {
        let state = self.messages_mut();
        let Some(entry) = state
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
        state.read_ids.insert(id);
        let now = state.unread.channel(channel).saturating_sub(1);
        state.unread.set_channel(channel, now);
        cx.notify();
    }
    fn messages_move_selection(&mut self, delta: i32, cx: &mut gpui::Context<Self>) {
        let state = self.messages_mut();
        if state.channel == MessageChannel::Dm {
            let visible = state.visible_conversation_indices();
            let len = visible.len();
            if len == 0 {
                return;
            }
            let current = state
                .selected
                .and_then(|id| {
                    visible
                        .iter()
                        .position(|index| state.dm.conversations[*index].talker_id == id)
                })
                .map(|index| index as i32)
                .unwrap_or(-1);
            let next = (current + delta).clamp(0, len as i32 - 1) as usize;
            let id = state.dm.conversations[visible[next]].talker_id;
            state.selected = Some(id);
            state
                .dm_scroll
                .scroll_to_item(next, ScrollStrategy::Top);
        } else {
            let visible = state.visible_indices();
            if visible.is_empty() {
                return;
            }
            let current = state
                .selected
                .and_then(|id| {
                    visible
                        .iter()
                        .position(|index| state.entries()[*index].id == id)
                })
                .map(|index| index as i32)
                .unwrap_or(-1);
            let next = (current + delta).clamp(0, visible.len() as i32 - 1) as usize;
            let id = state.entries()[visible[next]].id;
            state.selected = Some(id);
            if let Some(row) = state.row_of_entry(id) {
                state
                    .inbox_scroll
                    .scroll_to_item(row, ScrollStrategy::Top);
            }
        }
        cx.notify();
    }
    fn messages_open_entry(&mut self, id: i64, cx: &mut gpui::Context<Self>) {
        self.messages_mark_read(id, cx);
        let Some(entry) = self
            .messages()
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
                self.messages_open_video(bvid, cx);
            }
            MessageTarget::Dynamic { id: _ } => {
                // 动态详情按 id 打开还没有接（现有详情只接受 feed 实体）。
                // 不假装成功：把链接交给用户，并明确说明页面未接入。
                let uri = entry.uri.clone();
                if !uri.trim().is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(uri));
                }
                self.messages_notice("动态详情页暂未接入，链接已复制", cx);
            }
            MessageTarget::Unknown => {
                if entry.target_missing() {
                    self.messages_notice("原内容已删除或不可见", cx);
                } else {
                    self.messages_notice("只能标记已读，目标链接暂不支持站内跳转", cx);
                }
            }
        }
        cx.notify();
    }
    fn messages_copy_link(&mut self, id: i64, cx: &mut gpui::Context<Self>) {
        let Some(uri) = self
            .messages()
            .entries()
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.uri.clone())
        else {
            return;
        };
        if uri.trim().is_empty() {
            self.messages_notice("这条消息没有可复制的链接", cx);
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(uri));
        self.messages_notice("已复制", cx);
    }
    /// 打开视频（组装点覆盖）。
    fn messages_open_video(&mut self, _bvid: String, _cx: &mut gpui::Context<Self>) {}

    fn ensure_chat_composer(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.messages_mut().composer.is_some() {
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
                view.messages_mut().composer_draft = state.read(cx).value().to_string();
                cx.notify();
            }
            InputEvent::PressEnter { shift, .. } if !shift => {
                // 订阅回调里拿不到 Window；置位后由下一帧 render 发送。
                view.messages_mut().send_requested = true;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        self.messages_mut().composer = Some(state);
    }
    fn messages_ensure_composer(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        self.ensure_chat_composer(window, cx);
    }

    fn messages_load_chat_latest(&mut self, cx: &mut gpui::Context<Self>) {
        let Some((talker_id, generation)) = self.messages_mut().chat.as_mut().map(|chat| {
            // 每次重拉都 +1：旧请求回来时（尤其重新打开同一个会话）会被丢弃。
            chat.generation = chat.generation.wrapping_add(1);
            chat.loading = true;
            chat.error = None;
            chat.pending_bottom = true;
            (chat.talker_id, chat.generation)
        }) else {
            return;
        };
        let cookie = self.messages_cookie();
        let my_mid = self
            .messages_session()
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
                let Some(chat) = view.messages_mut().chat.as_mut() else {
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
    fn messages_verify_relation(&mut self, talker_id: i64, cx: &mut gpui::Context<Self>) {
        let Some(session) = self.messages_session() else {
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
                } else if view.messages_mut().whitelist_loaded {
                    Some(view.messages_mut().whitelist.contains(&talker_id))
                } else {
                    view.messages()
                        .chat
                        .as_ref()
                        .filter(|chat| chat.talker_id == talker_id)
                        .and_then(|chat| chat.mutual)
                };
                if let Some(chat) = view.messages_mut().chat.as_mut() {
                    if chat.talker_id == talker_id {
                        chat.mutual = mutual;
                    }
                }
                for conversation in &mut view.messages_mut().dm.conversations {
                    if conversation.talker_id == talker_id {
                        conversation.mutual = mutual;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn messages_open_conversation(
        &mut self,
        talker_id: i64,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(conversation) = self
            .messages_mut()
            .dm
            .conversations
            .iter()
            .find(|conversation| conversation.talker_id == talker_id)
            .cloned()
        else {
            return;
        };
        self.messages_mut().selected = Some(talker_id);
        self.messages_mut().chat = Some(ChatState::new(&conversation));
        self.messages_mut().action_error = None;
        self.messages_mut().composer_draft.clear();
        self.messages_ensure_composer(window, cx);
        if let Some(input) = self.messages_mut().composer.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.messages_load_chat_latest(cx);
        self.messages_mark_read_local(talker_id);
        self.messages_mark_read_background(talker_id, 0, cx);
        self.messages_verify_relation(talker_id, cx);
        cx.notify();
    }
    fn messages_toggle_like(&mut self, id: i64, cx: &mut gpui::Context<Self>) {
        let state = self.messages_mut();
        state.expanded_like = if state.expanded_like == Some(id) {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }
    fn messages_close_conversation(&mut self, cx: &mut gpui::Context<Self>) {
        self.messages_mut().chat = None;
        self.messages_mut().composer_draft.clear();
        self.messages_mut().focus_requested = true;
        self.messages_mut().action_error = None;
        self.messages_mut()
            .dm_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }
    fn messages_resend_failed(
        &mut self,
        text: String,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let has_chat = self.messages_mut().chat.is_some();
        if !has_chat {
            return;
        }
        if let Some(chat) = self.messages_mut().chat.as_mut() {
            chat.failed = None;
        }
        if let Some(input) = self.messages().composer.clone() {
            input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        }
        self.messages_mut().composer_draft = text;
        self.messages_send_chat(window, cx);
    }
    fn messages_mark_chat_read(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(talker_id) = self.messages_mut().chat.as_ref().map(|chat| chat.talker_id) else {
            return;
        };
        self.messages_mark_read_background(talker_id, 0, cx);
    }
    fn messages_load_older_chat(&mut self, cx: &mut gpui::Context<Self>) {
        let Some((talker_id, generation, begin_seqno)) =
            self.messages_mut().chat.as_ref().and_then(|chat| {
                (chat.has_more && !chat.loading_more && chat.begin_seqno > 0).then_some((
                    chat.talker_id,
                    chat.generation,
                    chat.begin_seqno,
                ))
            })
        else {
            return;
        };
        if let Some(chat) = self.messages_mut().chat.as_mut() {
            chat.loading_more = true;
        }
        let cookie = self.messages_cookie();
        let my_mid = self
            .messages_session()
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
                let Some(chat) = view.messages_mut().chat.as_mut() else {
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
    fn messages_ask_remove_chat(&mut self, cx: &mut gpui::Context<Self>) {
        let Some((talker_id, nickname)) = self
            .messages()
            .chat
            .as_ref()
            .map(|chat| (chat.talker_id, chat.nickname.clone()))
        else {
            return;
        };
        let state = self.messages_mut();
        state.confirm = Some(RemoveConfirm { talker_id, nickname });
        state.confirm_busy = false;
        state.action_error = None;
        cx.notify();
    }
    fn messages_send_chat(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let Some((talker_id, generation, can_send, sending)) =
            self.messages_mut().chat.as_ref().map(|chat| {
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
        let text = self.messages_mut().composer_draft.clone();
        if text.trim().is_empty() {
            return;
        }
        if text.len() > DM_MAX_BYTES {
            self.messages_action_error("消息超过 2000 字节，请缩短后再发送", cx);
            return;
        }
        let Some(session) = self.messages_session() else {
            self.messages_action_error("登录态已失效，请重新登录", cx);
            return;
        };
        if let Some(chat) = self.messages_mut().chat.as_mut() {
            chat.sending = true;
            chat.pending = Some(text.clone());
            chat.failed = None;
            chat.pending_bottom = true;
        }
        if let Some(input) = self.messages_mut().composer.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.messages_mut().composer_draft.clear();
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
                let Some(chat) = view.messages_mut().chat.as_mut() else {
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
                        view.messages_mark_read_local(talker_id);
                        view.messages_mark_read_background(talker_id, 0, cx);
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
    fn messages_ask_remove_conversation(
        &mut self,
        talker_id: i64,
        cx: &mut gpui::Context<Self>,
    ) {
        let state = self.messages_mut();
        let Some(conversation) = state
            .dm
            .conversations
            .iter()
            .find(|conversation| conversation.talker_id == talker_id)
        else {
            return;
        };
        state.confirm = Some(RemoveConfirm {
            talker_id,
            nickname: conversation.nickname.clone(),
        });
        state.confirm_busy = false;
        state.action_error = None;
        cx.notify();
    }
    fn messages_confirm_remove(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(confirm) = self.messages_mut().confirm.clone() else {
            return;
        };
        if self.messages_mut().confirm_busy {
            return;
        }
        let Some(session) = self.messages_session() else {
            self.messages_mut().action_error = Some("登录态已失效，请重新登录".into());
            cx.notify();
            return;
        };
        self.messages_mut().confirm_busy = true;
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
                view.messages_mut().confirm_busy = false;
                match result {
                    Ok(()) => {
                        view.messages_mut()
                            .dm
                            .conversations
                            .retain(|conversation| conversation.talker_id != talker_id);
                        view.messages_mut().confirm = None;
                        if view.messages_mut().chat.as_ref().map(|chat| chat.talker_id) == Some(talker_id)
                        {
                            view.messages_mut().chat = None;
                        }
                        view.messages_mut().error_clear();
                        view.messages_notice("会话已移除", cx);
                    }
                    Err(error) => {
                        // 失败保留条目，不做本地假删除。
                        view.messages_mut().action_error = Some(crate::messages_ui::friendly_error(&error));
                    }
                }
                view.messages_refresh_unread(cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn messages_cancel_remove(&mut self, cx: &mut gpui::Context<Self>) {
        self.messages_mut().confirm = None;
        self.messages_mut().action_error = None;
        cx.notify();
    }
    fn messages_mark_conversation_read(
        &mut self,
        talker_id: i64,
        cx: &mut gpui::Context<Self>,
    ) {
        self.messages_mark_read_local(talker_id);
        self.messages_mark_read_background(talker_id, 0, cx);
        cx.notify();
    }

    fn messages_prepare_frame(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.messages().focus.is_none() {
            self.messages_mut().focus = Some(cx.focus_handle().tab_stop(true));
        }
        if self.messages().chat.is_some() {
            self.ensure_chat_composer(window, cx);
        }
        if std::mem::take(&mut self.messages_mut().send_requested) {
            self.messages_send_chat(window, cx);
        }
        if self.messages().focus_requested {
            self.messages_mut().focus_requested = false;
            if let Some(handle) = self.messages().focus.clone() {
                cx.defer_in(window, move |_view, window, cx| {
                    window.focus(&handle, cx);
                });
            }
        }
        let bottom = self
            .messages_mut()
            .chat
            .as_mut()
            .map(|chat| std::mem::take(&mut chat.pending_bottom))
            .unwrap_or(false);
        if bottom {
            if let Some(chat) = self.messages().chat.as_ref() {
                chat.scroll.scroll_to_bottom();
                let handle = chat.scroll.clone();
                cx.defer_in(window, move |_view, _window, cx| {
                    handle.scroll_to_bottom();
                    cx.notify();
                });
            }
        }
    }

    fn load_more_dm(&mut self, cx: &mut gpui::Context<Self>) {
        let (generation, begin_ts) = {
            let state = self.messages_mut();
            if state.dm.loading_more || state.dm.loading || !state.dm.has_more {
                return;
            }
            let Some(begin_ts) = state.dm.begin_ts else {
                return;
            };
            state.dm.loading_more = true;
            (state.dm.generation, begin_ts)
        };
        let cookie = self.messages_cookie();
        let my_mid = self
            .messages_session()
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
                if view.messages().dm.generation != generation {
                    return;
                }
                view.messages_mut().dm.loading_more = false;
                match result {
                    Ok((conversations, next)) => {
                        let state = view.messages_mut();
                        for conversation in conversations {
                            if !state
                                .dm
                                .conversations
                                .iter()
                                .any(|existing| existing.talker_id == conversation.talker_id)
                            {
                                state.dm.conversations.push(conversation);
                            }
                        }
                        state.dm.has_more = next.is_some();
                        state.dm.begin_ts = next;
                        state.apply_whitelist();
                        let total = state.dm_unread_total();
                        state.unread.dm = total;
                    }
                    Err(error) => {
                        view.messages_mut().action_error =
                            Some(crate::messages_ui::friendly_error(&error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn ensure_messages(&mut self, cx: &mut gpui::Context<Self>) {
        self.messages_mut().focus_requested = true;
        self.messages_refresh_unread(cx);
        if self.messages().chat.is_none() {
            self.messages_load_channel(false, cx);
        }
        self.messages_start_poll(cx);
    }

    fn messages_load_channel(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        match self.messages().channel {
            MessageChannel::Dm => self.messages_load_dm(force, cx),
            channel => self.messages_load_inbox(channel, force, cx),
        }
    }

    fn messages_load_inbox(
        &mut self,
        channel: MessageChannel,
        force: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        if channel == MessageChannel::Dm {
            return;
        }
        let started = {
            let state = self.messages();
            let inbox = match channel {
                MessageChannel::Reply => &state.reply,
                MessageChannel::Mention => &state.mention,
                MessageChannel::Like => &state.like,
                MessageChannel::Dm => return,
            };
            !inbox.loaded && !inbox.loading
        };
        if !started && !force {
            return;
        }
        if self.messages_session().is_none() {
            let state = self.messages_mut();
            let inbox = match channel {
                MessageChannel::Reply => &mut state.reply,
                MessageChannel::Mention => &mut state.mention,
                MessageChannel::Like => &mut state.like,
                MessageChannel::Dm => return,
            };
            inbox.clear();
            cx.notify();
            return;
        }

        let (generation, first_load) = {
            let state = self.messages_mut();
            let inbox = match channel {
                MessageChannel::Reply => &mut state.reply,
                MessageChannel::Mention => &mut state.mention,
                MessageChannel::Like => &mut state.like,
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
        if first_load {
            let state = self.messages_mut();
            state.anchored = false;
            state.selected = None;
        }
        let cookie = self.messages_cookie();
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
                let read_ids = view.messages().read_ids.clone();
                let unread_loaded = view.messages().unread_loaded;
                let mention_unread = view.messages().unread.mention as usize;
                let mut selected_id = None;
                let error_text = {
                    let state = view.messages_mut();
                    let inbox = match channel {
                        MessageChannel::Reply => &mut state.reply,
                        MessageChannel::Mention => &mut state.mention,
                        MessageChannel::Like => &mut state.like,
                        MessageChannel::Dm => return,
                    };
                    if inbox.generation != generation {
                        return;
                    }
                    inbox.loading = false;
                    inbox.loaded = true;
                    match result {
                        Ok((items, cursor)) => {
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
                            selected_id = inbox
                                .entries
                                .iter()
                                .find(|entry| entry.is_unread())
                                .or_else(|| inbox.entries.first())
                                .map(|entry| entry.id);
                            None
                        }
                        Err(error) => {
                            inbox.error = Some(error.clone());
                            Some(error)
                        }
                    }
                };
                let _ = error_text;
                let state = view.messages_mut();
                state.selected = selected_id;
                if !state.anchored {
                    state.anchored = true;
                    if let Some(row) = state.first_unread_row() {
                        state
                            .inbox_scroll
                            .scroll_to_item(row, ScrollStrategy::Center);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn messages_start_poll(&mut self, cx: &mut gpui::Context<Self>) {
        if self.messages().poll_started {
            return;
        }
        self.messages_mut().poll_started = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            let alive = this.update(cx, |view, cx| {
                if !view.messages_poll_allowed()
                    || view.messages().chat.is_some()
                    || !view.messages().window_active
                    || view.messages_session().is_none()
                {
                    return;
                }
                view.messages_refresh_unread(cx);
                if view.messages().channel == MessageChannel::Dm {
                    view.messages_load_dm(true, cx);
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }

    fn messages_poll_allowed(&self) -> bool {
        true
    }
    fn messages_land_on_first_unread(&mut self, cx: &mut gpui::Context<Self>) {
        let channel = self.messages_mut().unread.first_unread_channel();
        self.messages_mut().switch_channel(channel);
        self.messages_mut().focus_requested = true;
        self.messages_mut()
            .inbox_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.messages_mut()
            .dm_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        if channel == MessageChannel::Dm {
            self.messages_refresh_whitelist(false, cx);
            self.messages_load_dm(false, cx);
        } else {
            self.messages_load_inbox(channel, false, cx);
        }
    }

    fn messages_refresh_whitelist(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        if self.messages_mut().whitelist_loading {
            return;
        }
        if !force && self.messages_mut().whitelist_loaded {
            if let Some(checked) = self.messages_mut().whitelist_checked_at {
                if checked.elapsed().as_secs() < MUTUAL_TTL_SECS {
                    return;
                }
            }
        }
        let Some(session) = self.messages_session() else {
            return;
        };
        self.messages_mut().whitelist_loading = true;
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
                view.messages_mut().whitelist_loading = false;
                match result {
                    Ok(ids) => {
                        eprintln!("[messages] 互关白名单：{} 人", ids.len());
                        view.messages_mut().whitelist = ids;
                        view.messages_mut().whitelist_loaded = true;
                        view.messages_mut().apply_whitelist();
                    }
                    Err(error) => {
                        eprintln!("[messages] 互关白名单失败：{error}");
                    }
                }
                view.messages_mut().whitelist_checked_at = Some(std::time::Instant::now());
                cx.notify();
            });
        })
        .detach();
    }
}

pub fn merge_chat(existing: &mut Vec<ChatMessage>, incoming: Vec<ChatMessage>) {
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
