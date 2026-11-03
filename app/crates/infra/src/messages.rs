//! 消息中心：四通道模型、未读口径、赞聚合、时间与摘要格式化、私信会话。
//!
//! 对应设计文档 12.5.4（消息中心 · 通道与准入 / 详设）。
//! 这里只放**纯数据与文案规则**，不依赖 gpui：
//! - 未读判定以接口的 `reply_time > last_view_at` 为准（E-26），
//!   本地只保留「用户刚点过」的乐观覆盖，不自己编第二套已读状态；
//! - 赞按「被赞内容 + 业务类型」聚合，窗口 24 小时（E-25：不能假设服务端已聚合）；
//! - @ / 赞列表未被公开文档收录，字段缺失时降级成通用文案，不让整页失败；
//! - 互动没有可清零的已读接口，因此不提供「全部已读」，只提供「只看未读」；
//! - 私信只收互关会话：互关判定用 `/x/relation/friends` 白名单（E-24），
//!   单条 `is_follow` 只作提示，不单独当互关依据。

use std::collections::HashMap;

use chrono::{Datelike, Local, TimeZone, Timelike};

use crate::api::models::{
    MsgfeedBodyRaw, MsgfeedItemRaw, MsgfeedUnreadData, SessionMsgRaw, SessionRaw,
};
use crate::format;

/// 赞聚合窗口：24 小时。跨窗口不合并（设计文档 12.5.4 条目解剖 · 赞）。
pub const LIKE_AGG_WINDOW_SECS: u64 = 24 * 60 * 60;
/// 私信正文上限（接口按字节算）。
pub const DM_MAX_BYTES: usize = 2000;
/// 互关白名单缓存 TTL（E-24 的退路：5 分钟）。
pub const MUTUAL_TTL_SECS: u64 = 5 * 60;
/// 一条气泡最大宽度（逻辑像素，仅文案层用于截断判断）。
pub const BUBBLE_MAX_WIDTH: f32 = 320.0;

/// 消息中心的四个通道。只是页内状态，不是路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MessageChannel {
    /// 回复我的 —— 默认通道。
    #[default]
    Reply,
    /// @我的。
    Mention,
    /// 收到的赞（必须客户端聚合）。
    Like,
    /// 私信（只有互关会话）。
    Dm,
}

impl MessageChannel {
    /// 互动三通道（去掉私信）。分段控件顺序与文档一致。
    pub const INBOX: [Self; 3] = [Self::Reply, Self::Mention, Self::Like];
    pub const ALL: [Self; 4] = [Self::Reply, Self::Mention, Self::Like, Self::Dm];

    pub fn label(self) -> &'static str {
        match self {
            Self::Reply => "回复我的",
            Self::Mention => "@我的",
            Self::Like => "收到的赞",
            Self::Dm => "私信",
        }
    }

    /// 空态：原因 + 可执行动作，不写「暂无数据」（附录 C.5）。
    pub fn empty_title(self) -> &'static str {
        match self {
            Self::Reply => "还没有人回复你的评论",
            Self::Mention => "还没有人 @ 你",
            Self::Like => "还没有收到赞",
            Self::Dm => "这里只会出现互相关注好友的消息",
        }
    }

    pub fn empty_desc(self) -> &'static str {
        match self {
            Self::Reply | Self::Like => "去首页逛逛，参与讨论后消息会出现在这里",
            Self::Mention => "被 @ 时，这里会留下一条记录",
            Self::Dm => "未互关的人无法给你发私信，首次联系请走评论区",
        }
    }

    pub fn error_title(self) -> &'static str {
        match self {
            Self::Reply => "回复加载失败",
            Self::Mention => "@ 消息加载失败",
            Self::Like => "赞消息加载失败",
            Self::Dm => "私信加载失败",
        }
    }

    /// 是否是需要 `v_virtual_list` 定长行的互动通道（互动 116 / 会话 72）。
    pub fn is_inbox(self) -> bool {
        self != Self::Dm
    }
}

/// 四个通道的未读数。顶栏 / Dock 只决定红点亮不亮，数字只出现在消息中心。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnreadCounts {
    pub reply: u64,
    pub mention: u64,
    pub like: u64,
    pub dm: u64,
}

impl UnreadCounts {
    /// 从 `/x/msgfeed/unread` 映射。
    ///
    /// `recv_reply = reply + at` 只用于总数，分通道仍各取各的；
    /// `like` 是旧字段、`recv_like` 是新字段，两个都读、取较大值。
    pub fn from_unread_data(data: &MsgfeedUnreadData) -> Self {
        Self {
            reply: data.reply.max(0) as u64,
            mention: data.at.max(0) as u64,
            like: data.recv_like.max(data.like).max(0) as u64,
            dm: 0,
        }
    }

    pub fn channel(self, channel: MessageChannel) -> u64 {
        match channel {
            MessageChannel::Reply => self.reply,
            MessageChannel::Mention => self.mention,
            MessageChannel::Like => self.like,
            MessageChannel::Dm => self.dm,
        }
    }

    /// 总数。用于顶栏 / Dock 红点，不显示数字。
    pub fn total(self) -> u64 {
        self.reply + self.mention + self.like + self.dm
    }

    pub fn has_any(self) -> bool {
        self.total() > 0
    }

    /// 点铃铛时落点：第一个有未读的通道；全无未读落「回复我的」。
    /// 顺序固定为 回复 → @ → 赞 → 私信。
    pub fn first_unread_channel(self) -> MessageChannel {
        MessageChannel::ALL
            .into_iter()
            .find(|channel| self.channel(*channel) > 0)
            .unwrap_or(MessageChannel::Reply)
    }

    pub fn set_channel(&mut self, channel: MessageChannel, value: u64) {
        match channel {
            MessageChannel::Reply => self.reply = value,
            MessageChannel::Mention => self.mention = value,
            MessageChannel::Like => self.like = value,
            MessageChannel::Dm => self.dm = value,
        }
    }
}

/// 互动列表的游标：`cursor.id` + `cursor.time`；`is_end` 判到底。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboxCursor {
    pub id: i64,
    pub time: i64,
    pub is_end: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageUser {
    pub mid: i64,
    pub nickname: String,
    pub avatar: String,
}

impl MessageUser {
    /// 头像缩略图：40px 显示，Retina 取 64。
    pub fn face_thumb_url(&self) -> String {
        thumb_url(&self.avatar, "@64w_64h_1c.webp")
    }

    pub fn initial(&self) -> String {
        self.nickname.chars().next().unwrap_or('?').to_string()
    }
}

/// 消息条目的跳转目标。**只解析 `uri`**，不解析 `native_uri`
/// （那是 B 站 App 的 scheme；客户端只用自己的路由）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MessageTarget {
    Video {
        bvid: String,
    },
    Dynamic {
        id: String,
    },
    /// 有 uri 但认不出目标，或 uri 为空。
    #[default]
    Unknown,
}

impl MessageTarget {
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// 一条互动消息（回复 / @ / 赞）。
#[derive(Debug, Clone, Default)]
pub struct MessageEntry {
    /// 条目主键，用于本地已读覆盖与选择。
    pub id: i64,
    pub channel: MessageChannel,
    pub user: MessageUser,
    /// 动作文案：「回复了你的评论」「在楼中楼回复了你」「在评论中 @ 了你」…
    pub action: String,
    /// 引用摘要（2 行截断由视图做）。
    pub summary: String,
    /// 目标标题（视频标题 / 「动态」）。
    pub target_title: String,
    pub uri: String,
    /// 业务类型；分支一律走它 + uri，不按 business_id 硬编码。
    pub business: String,
    pub business_id: i64,
    /// 赞聚合键的一半：被赞内容的 subject_id。
    pub subject_id: i64,
    /// 时间（Unix 秒）。
    pub time: u64,
    /// 已读基准；`time > last_view_at` 即未读。
    pub last_view_at: u64,
    /// 本地乐观已读（用户刚点开 / 点了「标为已读」）。
    pub read_override: bool,
    pub counts_reply: u64,
    pub is_up: bool,
    /// 赞聚合的头像堆叠（最多展示 3 个，全部保留供展开）。
    pub likers: Vec<MessageUser>,
    /// 赞的总人数（接口 `counts`），可能大于本地拿到的 `users` 数。
    pub like_count: u64,
    pub target: MessageTarget,
}

impl MessageEntry {
    /// 从接口条目映射。`fallback_last_view_at` 是响应顶层的 `last_view_at`
    /// （部分响应把已读时间放在顶层）。
    pub fn from_raw(
        channel: MessageChannel,
        raw: MsgfeedItemRaw,
        fallback_last_view_at: i64,
    ) -> Self {
        let body: MsgfeedBodyRaw = raw.item;
        let business = body.business.to_ascii_lowercase();
        let kind = body.kind.to_ascii_lowercase();
        let summary = first_non_empty(&[
            &body.source_content,
            &body.target_reply_content,
            &body.root_reply_content,
            &body.title,
        ]);
        let action = action_text(channel, &kind, &business, &body);
        let last_view_at = if raw.last_view_at > 0 {
            raw.last_view_at as u64
        } else {
            fallback_last_view_at.max(0) as u64
        };
        let time = raw.reply_time.max(0) as u64;
        let count = raw.counts.reply.max(0) as u64;
        let is_up = raw.counts.is_up != 0;
        // 赞列表只有 `users`；回复 / @ 只有单个 `user`。
        let mut user = MessageUser {
            mid: raw.user.mid,
            nickname: raw.user.nickname.trim().to_string(),
            avatar: raw.user.avatar,
        };
        let mut likers: Vec<MessageUser> = raw
            .users
            .into_iter()
            .map(|user| MessageUser {
                mid: user.mid,
                nickname: user.nickname.trim().to_string(),
                avatar: user.avatar,
            })
            .collect();
        if user.nickname.is_empty() {
            if let Some(first) = likers.first() {
                user = first.clone();
            }
        }
        if channel == MessageChannel::Like && likers.is_empty() {
            likers.push(user.clone());
        }
        let id = if raw.id > 0 {
            raw.id
        } else if body.source_id > 0 {
            body.source_id
        } else {
            body.target_id
        };
        let like_count = if channel == MessageChannel::Like {
            count.max(likers.len() as u64)
        } else {
            0
        };
        Self {
            id,
            channel,
            user: user.clone(),
            action,
            summary,
            target_title: format::html_decode(body.title.trim()),
            target: parse_target(&body.uri),
            uri: body.uri,
            business,
            business_id: body.business_id,
            subject_id: if body.subject_id > 0 {
                body.subject_id
            } else {
                body.item_id
            },
            time,
            last_view_at,
            read_override: false,
            counts_reply: count,
            is_up,
            likers,
            like_count,
        }
        .normalized()
    }

    /// 未读：接口时间优先；用户刚点过则本地覆盖。
    pub fn is_unread(&self) -> bool {
        !self.read_override && self.time > self.last_view_at
    }

    /// 标为已读（本地乐观；msgfeed 没有可核实的单条已读接口，见 E-26）。
    pub fn mark_read(&mut self) {
        self.read_override = true;
        self.last_view_at = self.last_view_at.max(self.time);
    }

    /// 赞聚合键。subject_id 优先；缺失时退回 business_id / 条目 id，
    /// 保证「认不出目标」时不会把无关内容误合并。
    pub fn like_key(&self) -> (String, i64) {
        if self.subject_id > 0 {
            (self.business.clone(), self.subject_id)
        } else if self.business_id > 0 {
            (self.business.clone(), self.business_id)
        } else {
            (self.business.clone(), self.id)
        }
    }

    pub fn same_like_subject(&self, other: &Self) -> bool {
        self.channel == MessageChannel::Like
            && other.channel == MessageChannel::Like
            && self.like_key() == other.like_key()
    }

    fn merge_like(&mut self, other: MessageEntry) {
        // 分组保留较新的时间，头像按时间倒序；同 mid 去重。
        if other.time > self.time {
            self.time = other.time;
            self.last_view_at = other.last_view_at;
            self.uri = other.uri.clone();
            self.target = other.target.clone();
            self.target_title = other.target_title.clone();
            self.business_id = other.business_id;
            self.id = other.id;
        }
        for liker in other.likers {
            if !liker.nickname.is_empty()
                && !self.likers.iter().any(|existing| {
                    (liker.mid > 0 && existing.mid == liker.mid)
                        || (liker.mid <= 0 && existing.nickname == liker.nickname)
                })
            {
                self.likers.push(liker);
            }
        }
        // 聚合后只要有一条未读，整组就算未读；只有全部读过才保持已读。
        self.read_override &= other.read_override;
        self.like_count = self
            .like_count
            .max(other.like_count)
            .max(self.likers.len() as u64);
    }

    /// 聚合后横幅上的「谁赞了你」：≤3 人全列，>3 人只列前两位 + 总数。
    pub fn like_actor_label(&self) -> String {
        let names: Vec<&str> = self
            .likers
            .iter()
            .map(|user| user.nickname.as_str())
            .filter(|name| !name.is_empty())
            .collect();
        match names.len() {
            0 => {
                if self.user.nickname.is_empty() {
                    "有人".into()
                } else {
                    self.user.nickname.clone()
                }
            }
            1..=3 => names.join("、"),
            _ => format!(
                "{}、{} 等 {} 人",
                names[0],
                names[1],
                self.like_count.max(names.len() as u64)
            ),
        }
    }

    /// 行首第一行要展示的「主体」：赞是聚合后的名字，其余是单个昵称。
    pub fn actor_label(&self) -> String {
        if self.channel == MessageChannel::Like {
            self.like_actor_label()
        } else if self.user.nickname.is_empty() {
            "有人".into()
        } else {
            self.user.nickname.clone()
        }
    }

    pub fn is_like(&self) -> bool {
        self.channel == MessageChannel::Like
    }

    /// 时间文案：今天给时刻，更早给日期（列表口径）。
    pub fn time_label(&self, now: u64) -> String {
        message_time(self.time, now)
    }

    /// 是否「原内容已删除 / 认不出目标」。有 uri 但认不出时仍保留链接，
    /// 只是不提供站内跳转。
    pub fn target_missing(&self) -> bool {
        self.uri.trim().is_empty()
    }

    fn normalized(mut self) -> Self {
        if self.summary.trim().is_empty() {
            self.summary = if self.target_title.is_empty() {
                "（内容为空）".into()
            } else {
                self.target_title.clone()
            };
        } else {
            self.summary = format::html_decode(self.summary.trim());
        }
        if self.target_title.is_empty() {
            self.target_title = match self.target {
                MessageTarget::Video { .. } => "视频".into(),
                MessageTarget::Dynamic { .. } => "动态".into(),
                MessageTarget::Unknown => String::new(),
            };
        }
        self
    }
}

/// 统一入口：按通道聚合赞，其他通道原样返回。
pub fn normalize_inbox(channel: MessageChannel, items: Vec<MessageEntry>) -> Vec<MessageEntry> {
    if channel == MessageChannel::Like {
        aggregate_likes(items)
    } else {
        items
    }
}

/// 客户端赞聚合（设计文档要求「必须聚合」）。
///
/// 列表本身按时间倒序；同一「被赞内容 + 业务」在 24 小时内合并成一条。
/// 窗口外的旧赞单列，避免把半年前的赞也算进同一堆。
pub fn aggregate_likes(items: Vec<MessageEntry>) -> Vec<MessageEntry> {
    let mut out: Vec<MessageEntry> = Vec::new();
    for entry in items {
        let merged = out.iter().position(|existing| {
            existing.same_like_subject(&entry)
                && existing.time.abs_diff(entry.time) <= LIKE_AGG_WINDOW_SECS
        });
        match merged {
            Some(index) => out[index].merge_like(entry),
            None => out.push(entry),
        }
    }
    out
}

/// 一条私信会话。
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub talker_id: i64,
    pub nickname: String,
    pub avatar: String,
    /// 互关状态：`None` = 白名单还没拿到（此时不锁发送，交给服务端判定）。
    pub mutual: Option<bool>,
    pub unread: u64,
    pub last_msg: String,
    pub last_msg_ts: u64,
    pub last_msg_mine: bool,
    /// 微秒时间戳换算后的秒。
    pub session_ts: u64,
    pub last_seqno: i64,
    pub is_follow: bool,
}

impl Conversation {
    /// 从会话列表原始项 + 用户名片映射构造。
    ///
    /// `mutual` 来自 `/x/relation/friends` 白名单；白名单没拿到时传 `None`。
    pub fn from_raw(
        raw: SessionRaw,
        my_mid: i64,
        cards: &HashMap<i64, MessageUser>,
        mutual: Option<bool>,
    ) -> Self {
        let card = cards.get(&raw.talker_id);
        let last = raw.last_msg.unwrap_or_default();
        let last_mine = last.sender_uid == my_mid;
        let last_msg = if last.msg_seqno == 0 && last.content.is_empty() {
            String::new()
        } else {
            decode_last_message(last.msg_type, last.msg_status, &last.content)
        };
        Self {
            talker_id: raw.talker_id,
            nickname: card
                .map(|user| user.nickname.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("用户 {}", raw.talker_id)),
            avatar: card.map(|user| user.avatar.clone()).unwrap_or_default(),
            mutual,
            unread: raw.unread_count.max(0) as u64,
            last_msg,
            last_msg_ts: last.timestamp.max(0) as u64,
            last_msg_mine: last_mine,
            session_ts: session_seconds(raw.session_ts),
            last_seqno: last.msg_seqno,
            is_follow: raw.is_follow != 0,
        }
    }

    /// 会话行摘要：我方发送带「我：」前缀；空会话给一句占位。
    pub fn preview(&self) -> String {
        if self.last_msg.is_empty() {
            return "还没有消息".into();
        }
        if self.last_msg_mine {
            format!("我：{}", self.last_msg)
        } else {
            self.last_msg.clone()
        }
    }

    pub fn time_label(&self, now: u64) -> String {
        message_time(self.session_ts.max(self.last_msg_ts), now)
    }

    pub fn face_thumb_url(&self) -> String {
        thumb_url(&self.avatar, "@64w_64h_1c.webp")
    }

    pub fn initial(&self) -> String {
        self.nickname.chars().next().unwrap_or('?').to_string()
    }

    /// 未互关确认后才禁用发送；白名单未知时允许尝试，失败再给可读文案。
    pub fn can_send(&self) -> bool {
        self.mutual != Some(false)
    }

    pub fn relation_label(&self) -> Option<&'static str> {
        match self.mutual {
            Some(true) => Some("互关"),
            Some(false) => Some("已解除互关"),
            None => None,
        }
    }
}

/// 私信正文的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatKind {
    #[default]
    Text,
    Image,
    Emoji,
    Video,
    /// 撤回：只渲染占位，**不读取被撤回的正文**。
    Revoked,
    /// 系统提示（互关欢迎语等）。
    System,
    Other,
}

/// 聊天页的一条消息。
#[derive(Debug, Clone, Default)]
pub struct ChatMessage {
    pub seqno: i64,
    pub sender_uid: i64,
    pub timestamp: u64,
    pub kind: ChatKind,
    pub text: String,
    pub mine: bool,
}

impl ChatMessage {
    pub fn from_raw(raw: SessionMsgRaw, my_mid: i64) -> Self {
        let revoked = raw.msg_status == 1;
        let system = raw.msg_source == 17
            || raw.msg_source == 18
            || raw.msg_type == 10
            || raw.msg_type == 18;
        let (kind, text) = if revoked {
            (ChatKind::Revoked, "消息已撤回".to_string())
        } else if system {
            let text = decode_content(raw.msg_type, &raw.content);
            (
                ChatKind::System,
                if text.trim().is_empty() {
                    "你们已互相关注，开始聊天吧".to_string()
                } else {
                    text
                },
            )
        } else {
            let (kind, text) = decode_raw_content(raw.msg_type, &raw.content);
            (kind, text)
        };
        Self {
            seqno: raw.msg_seqno,
            sender_uid: raw.sender_uid,
            timestamp: raw.timestamp.max(0) as u64,
            kind,
            text,
            mine: raw.sender_uid == my_mid,
        }
    }
}

/// 会话列表的最后一条：正文 + 类型标记。图片 / 表情 / 撤回只给占位。
pub fn decode_last_message(msg_type: i64, msg_status: i64, content: &str) -> String {
    if msg_status == 1 {
        return "消息已撤回".into();
    }
    decode_content(msg_type, content)
}

fn decode_raw_content(msg_type: i64, content: &str) -> (ChatKind, String) {
    match msg_type {
        1 => (ChatKind::Text, decode_content(msg_type, content)),
        2 => (ChatKind::Image, "[图片]".into()),
        3 => (ChatKind::Emoji, "[表情]".into()),
        5 => (ChatKind::Video, "[视频]".into()),
        11 | 12 => (ChatKind::Other, decode_content(msg_type, content)),
        _ => {
            let text = decode_content(msg_type, content);
            if text.trim().is_empty() {
                (ChatKind::Other, "[消息]".into())
            } else {
                (ChatKind::Other, text)
            }
        }
    }
}

/// 解析接口里的 JSON 正文字符串。取不到就退回原文，不让气泡空白。
pub fn decode_content(msg_type: i64, content: &str) -> String {
    if msg_type == 2 {
        return "[图片]".into();
    }
    if msg_type == 3 {
        return "[表情]".into();
    }
    let value: serde_json::Value = serde_json::from_str(content).unwrap_or(serde_json::Value::Null);
    // 18 = 会话内的系统提示（对方主动回复前最多 1 条）：正文是 JSON 数组字符串。
    if msg_type == 18 {
        if let Some(inner) = value.get("content").and_then(|v| v.as_str()) {
            if let Ok(array) = serde_json::from_str::<serde_json::Value>(inner) {
                if let Some(text) = array
                    .get(0)
                    .and_then(|item| item.get("text"))
                    .and_then(|v| v.as_str())
                {
                    if !text.trim().is_empty() {
                        return format::html_decode(text);
                    }
                }
            }
        }
    }
    let text = value.get("content").and_then(|v| v.as_str());
    if let Some(text) = text {
        if !text.trim().is_empty() {
            return format::html_decode(text);
        }
    }
    // 10 = 系统 / 登录通知；11 = 视频分享；12 = 专栏分享。
    // 这些卡片的正文在 text / title 里，不能一律当「[消息]」。
    if msg_type == 10 {
        return value
            .get("text")
            .or_else(|| value.get("title"))
            .and_then(|v| v.as_str())
            .map(format::html_decode)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "系统通知".into());
    }
    if msg_type == 11 || msg_type == 12 {
        let label = if msg_type == 11 {
            "[视频]"
        } else {
            "[专栏]"
        };
        let title = value
            .get("title")
            .and_then(|v| v.as_str())
            .map(format::html_decode)
            .unwrap_or_default();
        return if title.trim().is_empty() {
            label.into()
        } else {
            format!("{label} {title}")
        };
    }
    // 图片 / 表情卡的 JSON 里没有 content
    if value.get("url").is_some() {
        return "[图片]".into();
    }
    if value.get("type").is_some() && value.get("content").is_none() {
        return "[消息]".into();
    }
    format::html_decode(content.trim())
}

/// 列表时间：今天显示 `20:14`，昨天 `昨天`，今年 `10-21`，跨年 `2025-12-01`。
pub fn message_time(ts: u64, now: u64) -> String {
    if ts == 0 {
        return String::new();
    }
    let Some(local) = to_local(ts) else {
        return String::new();
    };
    let Some(now_local) = to_local(now) else {
        return format::date_time(ts);
    };
    let today = now_local.date_naive();
    let day = local.date_naive();
    if day == today {
        format!("{:02}:{:02}", local.hour(), local.minute())
    } else if day == today.pred_opt().unwrap_or(today) {
        "昨天".into()
    } else if day.year() == today.year() {
        format!("{:02}-{:02}", local.month(), local.day())
    } else {
        format!("{}-{:02}-{:02}", local.year(), local.month(), local.day())
    }
}

/// 聊天页时间分组：`今天 20:14` / `昨天 20:14` / `10月21日 20:14`。
pub fn chat_time_label(ts: u64, now: u64) -> String {
    if ts == 0 {
        return String::new();
    }
    let Some(local) = to_local(ts) else {
        return String::new();
    };
    let Some(now_local) = to_local(now) else {
        return format::date_time(ts);
    };
    let today = now_local.date_naive();
    let day = local.date_naive();
    let clock = format!("{:02}:{:02}", local.hour(), local.minute());
    if day == today {
        format!("今天 {clock}")
    } else if day == today.pred_opt().unwrap_or(today) {
        format!("昨天 {clock}")
    } else if day.year() == today.year() {
        format!("{}月{}日 {clock}", local.month(), local.day())
    } else {
        format!(
            "{}年{}月{}日 {clock}",
            local.year(),
            local.month(),
            local.day()
        )
    }
}

/// 相邻消息间隔是否要插一行时间（> 5 分钟）。
pub fn needs_time_divider(prev: Option<u64>, current: u64) -> bool {
    match prev {
        None => current > 0,
        Some(prev) => prev == 0 || current.saturating_sub(prev) > 5 * 60,
    }
}

/// 会话列表分页：`begin_ts` 是**微秒**，这里转秒给 UI / 下一请求。
pub fn session_seconds(ts: i64) -> u64 {
    if ts <= 0 {
        return 0;
    }
    // 1e12 以下视为已经是秒（接口历史行为），不硬除。
    if ts > 1_000_000_000_000 {
        (ts / 1_000_000) as u64
    } else {
        ts as u64
    }
}

/// 私信正文是否可发送：非空、≤2000 字节。
pub fn validate_dm_text(text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("消息不能为空".into());
    }
    if text.len() > DM_MAX_BYTES {
        return Err(format!("消息超过 {} 字节，请缩短后再发送", DM_MAX_BYTES));
    }
    Ok(())
}

/// 未读数对不上时（E-26）退化成「有互动」红点：只保留是否非零。
pub fn dots_only(counts: UnreadCounts) -> UnreadCounts {
    UnreadCounts {
        reply: counts.reply.min(1),
        mention: counts.mention.min(1),
        like: counts.like.min(1),
        dm: counts.dm.min(1),
    }
}

fn action_text(
    channel: MessageChannel,
    kind: &str,
    business: &str,
    body: &MsgfeedBodyRaw,
) -> String {
    match channel {
        MessageChannel::Reply => {
            let nested = !body.target_reply_content.trim().is_empty()
                && !body.source_content.trim().is_empty()
                && body.root_reply_content.trim().is_empty();
            if nested {
                "在楼中楼回复了你".into()
            } else if is_dynamic(kind, business) {
                "回复了你的动态评论".into()
            } else {
                "回复了你的评论".into()
            }
        }
        MessageChannel::Mention => {
            if is_dynamic(kind, business) {
                "在动态中 @ 了你".into()
            } else {
                "在评论中 @ 了你".into()
            }
        }
        MessageChannel::Like => {
            if is_dynamic(kind, business) {
                "赞了你的动态".into()
            } else if is_comment(kind, business) {
                "赞了你的评论".into()
            } else {
                "赞了你的内容".into()
            }
        }
        MessageChannel::Dm => String::new(),
    }
}

fn is_comment(kind: &str, business: &str) -> bool {
    kind == "reply"
        || business.contains("comment")
        || business.contains("reply")
        || business.contains("评论")
}

fn is_dynamic(kind: &str, business: &str) -> bool {
    kind == "dynamic" || business.contains("dynamic") || business.contains("动态")
}

fn parse_target(uri: &str) -> MessageTarget {
    if let Some(bvid) = find_bvid(uri) {
        return MessageTarget::Video { bvid };
    }
    if let Some(id) = find_digits_after(uri, &["t.bilibili.com/", "/opus/", "/dynamic/"]) {
        return MessageTarget::Dynamic { id };
    }
    MessageTarget::Unknown
}

fn find_bvid(uri: &str) -> Option<String> {
    for (index, _) in uri.match_indices("BV") {
        let rest: String = uri[index + 2..].chars().take(10).collect();
        if rest.len() == 10 && rest.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(format!("BV{rest}"));
        }
    }
    None
}

fn find_digits_after(uri: &str, needles: &[&str]) -> Option<String> {
    for needle in needles {
        if let Some(index) = uri.find(needle) {
            let rest = &uri[index + needle.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    None
}

fn first_non_empty(candidates: &[&String]) -> String {
    candidates
        .iter()
        .map(|text| text.trim())
        .find(|text| !text.is_empty())
        .unwrap_or("")
        .to_string()
}

fn to_local(ts: u64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_opt(ts as i64, 0).single()
}

fn thumb_url(url: &str, suffix: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    let url = url
        .strip_prefix("http://")
        .map_or_else(|| url.to_string(), |rest| format!("https://{rest}"));
    if url.contains('@') {
        url
    } else {
        format!("{url}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{MsgfeedCountsRaw, MsgfeedUserRaw};

    fn entry(
        channel: MessageChannel,
        id: i64,
        time: u64,
        subject: i64,
        nick: &str,
    ) -> MessageEntry {
        MessageEntry {
            id,
            channel,
            user: MessageUser {
                mid: id,
                nickname: nick.into(),
                avatar: String::new(),
            },
            action: String::new(),
            summary: "内容".into(),
            target_title: "视频".into(),
            uri: String::new(),
            business: "comment".into(),
            business_id: 1,
            subject_id: subject,
            time,
            last_view_at: 0,
            read_override: false,
            counts_reply: 1,
            is_up: false,
            likers: vec![MessageUser {
                mid: id,
                nickname: nick.into(),
                avatar: String::new(),
            }],
            like_count: 1,
            target: MessageTarget::Unknown,
        }
    }

    #[test]
    fn unread_counts_map_channels_and_fallbacks() {
        let data = MsgfeedUnreadData {
            reply: 3,
            at: 1,
            like: 0,
            recv_reply: 4,
            recv_like: 3,
        };
        let counts = UnreadCounts::from_unread_data(&data);
        assert_eq!(counts.reply, 3);
        assert_eq!(counts.mention, 1);
        assert_eq!(counts.like, 3, "旧 like 为 0 时用 recv_like");
        assert_eq!(counts.dm, 0);
        assert_eq!(counts.total(), 7);
        assert_eq!(counts.first_unread_channel(), MessageChannel::Reply);

        let only_at = UnreadCounts {
            reply: 0,
            mention: 2,
            like: 0,
            dm: 1,
        };
        assert_eq!(only_at.first_unread_channel(), MessageChannel::Mention);
        let none = UnreadCounts::default();
        assert_eq!(none.first_unread_channel(), MessageChannel::Reply);
    }

    #[test]
    fn like_entries_aggregate_on_subject_and_window() {
        let now = 1_700_000_000;
        let items = vec![
            entry(MessageChannel::Like, 1, now, 42, "A"),
            entry(MessageChannel::Like, 2, now - 60, 42, "B"),
            entry(MessageChannel::Like, 3, now - 120, 42, "C"),
            entry(MessageChannel::Like, 4, now - 180, 42, "D"),
            // 不同目标不合并
            entry(MessageChannel::Like, 5, now - 30, 43, "E"),
            // 超过 24 小时不合并
            entry(
                MessageChannel::Like,
                6,
                now - LIKE_AGG_WINDOW_SECS - 1,
                42,
                "F",
            ),
        ];
        let out = aggregate_likes(items);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].likers.len(), 4);
        assert_eq!(out[0].like_actor_label(), "A、B 等 4 人");
        assert_eq!(out[1].like_actor_label(), "E");
        assert_eq!(out[2].like_actor_label(), "F");
    }

    #[test]
    fn like_actor_label_uses_all_names_up_to_three() {
        let out = aggregate_likes(vec![
            entry(MessageChannel::Like, 1, 100, 1, "A"),
            entry(MessageChannel::Like, 2, 99, 1, "B"),
            entry(MessageChannel::Like, 3, 98, 1, "C"),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].like_actor_label(), "A、B、C");
    }

    #[test]
    fn read_override_wins_over_server_time() {
        let mut item = entry(MessageChannel::Reply, 1, 100, 0, "A");
        item.last_view_at = 0;
        assert!(item.is_unread());
        item.mark_read();
        assert!(!item.is_unread());
        assert_eq!(item.last_view_at, 100);
    }

    #[test]
    fn target_parses_video_and_dynamic_uri_only() {
        assert_eq!(
            parse_target("https://www.bilibili.com/video/BV1xx411c7mD?comment_on=1"),
            MessageTarget::Video {
                bvid: "BV1xx411c7mD".into()
            }
        );
        assert_eq!(
            parse_target("https://t.bilibili.com/123456789012"),
            MessageTarget::Dynamic {
                id: "123456789012".into()
            }
        );
        // native_uri 不参与解析；没有可识别 id 就是 Unknown
        assert_eq!(
            parse_target("bilibili://video/12345?native_uri=bilibili://x"),
            MessageTarget::Unknown
        );
    }

    #[test]
    fn message_time_formats_today_yesterday_and_dates() {
        let now = 1_700_000_000; // 本地时区下的某个时刻
        let today = message_time(now, now);
        assert_eq!(today.len(), 5, "今天给 HH:MM");
        let yesterday = message_time(now - 24 * 3600, now);
        assert_eq!(yesterday, "昨天");
        let earlier = message_time(now - 10 * 24 * 3600, now);
        assert!(earlier.contains('-'), "更早给日期：{earlier}");
    }

    #[test]
    fn chat_time_grouping() {
        assert!(needs_time_divider(None, 100));
        assert!(!needs_time_divider(Some(100), 100 + 300));
        assert!(needs_time_divider(Some(100), 100 + 301));
        let label = chat_time_label(1_700_000_000, 1_700_000_000);
        assert!(label.starts_with("今天 "), "{label}");
    }

    #[test]
    fn session_timestamp_microseconds_convert_to_seconds() {
        assert_eq!(session_seconds(1_700_000_000_123_456), 1_700_000_000);
        assert_eq!(session_seconds(1_700_000_000), 1_700_000_000);
        assert_eq!(session_seconds(0), 0);
    }

    #[test]
    fn content_decoding_covers_text_image_emoji_and_revoke() {
        assert_eq!(
            decode_content(1, r#"{"content":"你好 &amp; 世界"}"#),
            "你好 & 世界"
        );
        assert_eq!(decode_content(2, r#"{"url":"https://x/y.jpg"}"#), "[图片]");
        assert_eq!(decode_content(3, r#"{"url":"https://x/e.png"}"#), "[表情]");
        assert_eq!(
            decode_last_message(1, 1, r#"{"content":"被撤回的正文"}"#),
            "消息已撤回"
        );
        // 解析失败也不吞正文
        assert_eq!(decode_content(1, "纯文本"), "纯文本");
    }

    #[test]
    fn conversation_preview_prefixes_own_messages() {
        let raw = SessionRaw {
            talker_id: 7,
            unread_count: 2,
            session_ts: 1_700_000_000_000_000,
            is_follow: 1,
            last_msg: Some(SessionMsgRaw {
                sender_uid: 7,
                msg_type: 1,
                content: r#"{"content":"在吗"}"#.into(),
                msg_seqno: 9,
                timestamp: 1_700_000_000,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut cards = HashMap::new();
        cards.insert(
            7,
            MessageUser {
                mid: 7,
                nickname: "阿茶".into(),
                avatar: String::new(),
            },
        );
        let conv = Conversation::from_raw(raw, 1, &cards, Some(true));
        assert_eq!(conv.nickname, "阿茶");
        assert_eq!(conv.preview(), "在吗");
        assert_eq!(conv.session_ts, 1_700_000_000);
        assert!(conv.can_send());
        assert_eq!(conv.relation_label(), Some("互关"));

        let mut own = conv.clone();
        own.last_msg_mine = true;
        assert_eq!(own.preview(), "我：在吗");

        let mut broken = conv.clone();
        broken.mutual = Some(false);
        assert!(!broken.can_send());
        assert_eq!(broken.relation_label(), Some("已解除互关"));
    }

    #[test]
    fn dm_text_validation_uses_byte_limit() {
        assert!(validate_dm_text("  ").is_err());
        assert!(validate_dm_text("hi").is_ok());
        assert!(validate_dm_text(&"a".repeat(DM_MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn reply_action_text_handles_nested_and_dynamic() {
        let nested = MsgfeedBodyRaw {
            source_content: "回复".into(),
            target_reply_content: "被回复".into(),
            ..Default::default()
        };
        assert_eq!(
            action_text(MessageChannel::Reply, "reply", "comment", &nested),
            "在楼中楼回复了你"
        );
        let dynamic = MsgfeedBodyRaw::default();
        assert_eq!(
            action_text(MessageChannel::Reply, "dynamic", "dynamic", &dynamic),
            "回复了你的动态评论"
        );
        assert_eq!(
            action_text(MessageChannel::Mention, "reply", "comment", &dynamic),
            "在评论中 @ 了你"
        );
        assert_eq!(
            action_text(MessageChannel::Like, "reply", "comment", &dynamic),
            "赞了你的评论"
        );
    }

    #[test]
    fn raw_mapping_survives_missing_fields() {
        let raw = MsgfeedItemRaw {
            id: 5,
            user: MsgfeedUserRaw {
                mid: 1,
                nickname: "A".into(),
                avatar: "http://i0.hdslb.com/a.jpg".into(),
            },
            item: MsgfeedBodyRaw {
                source_content: "你好".into(),
                uri: "https://www.bilibili.com/video/BV1xx411c7mD".into(),
                title: "视频标题".into(),
                ..Default::default()
            },
            counts: MsgfeedCountsRaw {
                reply: 1,
                ..Default::default()
            },
            reply_time: 100,
            last_view_at: 90,
            ..Default::default()
        };
        let item = MessageEntry::from_raw(MessageChannel::Reply, raw, 0);
        assert_eq!(item.id, 5);
        assert_eq!(item.summary, "你好");
        assert_eq!(item.target_title, "视频标题");
        assert!(item.is_unread());
        assert_eq!(
            item.user.face_thumb_url(),
            "https://i0.hdslb.com/a.jpg@64w_64h_1c.webp"
        );
        assert!(matches!(item.target, MessageTarget::Video { .. }));
    }
}
