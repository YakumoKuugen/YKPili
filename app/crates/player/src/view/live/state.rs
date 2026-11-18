//! 直播会话状态（纯数据）。
//!
//! 只服务直播模式；点播字段仍在 `PlayerView` 上（迁移分步进行）。
//! 这里不持有 `PlaybackSource`：取流结果统一放在 `PlayerView.source`，
//! 渲染与控制栏通过 [`crate::PlayerView::mode_kind`] 判断差异。

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;

use gpui::ScrollHandle;
use yk_infra::live::LiveRoom;
use yk_infra::live_chat::{LiveChatHandle, LiveChatMessage};

/// 聊天连接状态（驱动面板顶部的小圆点与文案）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatConnection {
    Idle,
    Connecting,
    Connected,
    Reconnecting { attempt: u32, delay_secs: u64 },
    Failed(String),
}

impl ChatConnection {
    pub fn label(&self) -> String {
        match self {
            ChatConnection::Idle => "未连接".to_string(),
            ChatConnection::Connecting => "连接中…".to_string(),
            ChatConnection::Connected => "已连接".to_string(),
            ChatConnection::Reconnecting { attempt, .. } => {
                format!("重连中（第 {attempt} 次）")
            }
            ChatConnection::Failed(error) => format!("连接失败：{error}"),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, ChatConnection::Connected)
    }
}

/// 直播聊天：环形缓冲 + 连接句柄 + 滚动。
pub struct LiveChatState {
    pub connection: ChatConnection,
    /// 最多 200 条（doc 6.8 的环形缓冲）。
    pub messages: VecDeque<LiveChatMessage>,
    pub handle: Option<LiveChatHandle>,
    pub rx: Option<Receiver<yk_infra::LiveChatEvent>>,
    pub scroll: ScrollHandle,
    /// 上一次发送尝试的时间：本地限速 1.5s 一次，避免连点撞 `10031` / 风控。
    pub last_sent_at: Option<std::time::Instant>,
    /// 每次开始 / 停止连接 +1；旧的轮询任务据此退出。
    pub generation: u64,
}

impl Default for LiveChatState {
    fn default() -> Self {
        Self {
            connection: ChatConnection::Idle,
            messages: VecDeque::new(),
            handle: None,
            rx: None,
            scroll: ScrollHandle::new(),
            last_sent_at: None,
            generation: 0,
        }
    }
}

impl LiveChatState {
    /// 停止连接并清空缓冲（换房间 / 关闭播放器）。
    pub fn stop(&mut self) {
        self.handle = None;
        self.rx = None;
        self.connection = ChatConnection::Idle;
        self.messages.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn begin_connect(&mut self) {
        self.handle = None;
        self.rx = None;
        self.connection = ChatConnection::Connecting;
        self.messages.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn push(&mut self, message: LiveChatMessage) {
        if self.messages.len() >= 200 {
            self.messages.pop_front();
        }
        self.messages.push_back(message);
    }

    /// 测试 / 本地回显：把连接标记为已连接。
    pub(crate) fn mark_connected(&mut self) {
        self.connection = ChatConnection::Connected;
    }
}

/// 一次直播播放的会话。
pub(crate) struct LiveState {
    pub room_id: i64,
    pub title: String,
    pub uid: i64,
    pub uname: String,
    pub face: String,
    pub area_name: String,
    /// 人气 / 看过（来自房间信息接口）。
    pub online: u64,
    pub watched_text: String,
    /// 房间信息（标题 / 分区 / 人气）是否已拉到。按需加载：只有用户
    /// 点开「直播信息」Tab 才请求，减少开播瞬间的接口齐发。
    pub room_info_loaded: bool,
    pub room_info_loading: bool,

    /// 取流代际：重新连接 / 切画质 / 关闭后旧响应直接丢弃。
    pub loading: bool,
    pub error: Option<String>,

    /// 画质切换代际（与首屏取流分开：切画质不重置会话）。
    pub quality_generation: u64,
    pub quality_switching: bool,

    /// 断流浮层：Some = 已停止推进，等用户「重新连接」或「关闭」。
    pub outage: Option<String>,

    /// 面板 Tab：0 弹幕 / 1 直播信息。
    pub tab: usize,
    /// 选中协议（仅日志 / 诊断用）。
    pub is_flv: bool,
    pub chat: LiveChatState,
}

impl LiveState {
    pub(crate) fn from_room(room: &LiveRoom) -> Self {
        Self {
            room_id: room.room_id,
            title: room.title.clone(),
            uid: room.uid,
            uname: room.uname.clone(),
            face: room.face.clone(),
            area_name: room.area_name.clone(),
            online: room.online,
            watched_text: room.watched_label(),
            room_info_loaded: false,
            room_info_loading: false,
            loading: true,
            error: None,
            quality_generation: 0,
            quality_switching: false,
            outage: None,
            tab: 0,
            is_flv: true,
            chat: LiveChatState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yk_infra::live_chat::{LiveChatKind, LiveChatMessage};

    fn message(uid: i64) -> LiveChatMessage {
        LiveChatMessage {
            uid,
            uname: format!("u{uid}"),
            text: "hi".into(),
            kind: LiveChatKind::Danmaku,
            color: None,
            guard_level: 0,
            is_admin: false,
            ts: 0,
        }
    }

    #[test]
    fn ring_buffer_keeps_latest_200() {
        let mut chat = LiveChatState::default();
        for uid in 0..250 {
            chat.push(message(uid));
        }
        assert_eq!(chat.messages.len(), 200);
        assert_eq!(chat.messages.front().map(|m| m.uid), Some(50));
        assert_eq!(chat.messages.back().map(|m| m.uid), Some(249));
    }

    #[test]
    fn stop_and_reconnect_advance_generation() {
        let mut chat = LiveChatState::default();
        let first = chat.generation;
        chat.begin_connect();
        assert!(chat.generation > first);
        assert_eq!(chat.connection, ChatConnection::Connecting);
        chat.push(message(1));
        chat.stop();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.connection, ChatConnection::Idle);
    }

    #[test]
    fn connection_labels_are_human_readable() {
        assert_eq!(ChatConnection::Connected.label(), "已连接");
        assert!(ChatConnection::Failed("x".into()).label().contains("连接失败"));
    }
}
