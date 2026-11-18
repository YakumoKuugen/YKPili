//! 直播动作：打开 / 重新连接 / 切画质 / 断流。
//!
//! 所有异步结果都先比 `load_generation`（首屏 / 重连）与
//! `quality_generation`（切画质）再落状态；关闭或换房间后旧响应一律丢弃。

use gpui::component::input::InputState;
use gpui::{px, Context};
use yk_domain::models::UserSession;
use yk_domain::playback::{PlaybackQuality, PlaybackSource};
use yk_infra::live::{fetch_live_playback, fetch_live_room_info, LivePlayback, LiveRoom};
use yk_infra::live_chat::{connect_live_chat, LiveChatEvent};

use crate::view::live::LiveState;
use super::state::ChatConnection;
use crate::PlayerView;

/// 默认请求原画。
const LIVE_DEFAULT_QN: i32 = 10000;

impl PlayerView {
    /// 打开直播间：重置共享运行时 → 建立直播会话 → 后台取流。
    pub fn open_live(&mut self, room: LiveRoom, cx: &mut Context<Self>) {
        self.begin_new_source();
        let generation = self.load_generation;
        eprintln!(
            "[live] 打开直播间：room={} title={}",
            room.room_id, room.title
        );
        self.live = Some(Box::new(LiveState::from_room(&room)));
        // 关注直播列表里的房间天然已关注：直接点亮按钮，免一次 `x/relation`。
        // 来源未知（None）时才在取流成功后补查。
        self.followed = room.followed.unwrap_or(false);
        self.follow_known = room.followed.is_some();
        self.loading_source = true;
        self.model.clear_error();
        self.model.state.playing = true;
        self.model.state.duration_secs = 0.0;
        self.model.state.position_secs = 0.0;
        self.play_time = 0.0;
        self.start_position = 0.0;
        self.expanded = false;
        self.panel_scroll.set_offset(gpui::point(px(0.), px(0.)));
        // 直播不画画面滚动弹幕：只有右侧列表（P0）。
        self.danmaku_visible = false;
        cx.notify();
        self.fetch_live_source(LIVE_DEFAULT_QN, generation, None, cx);
        // 聊天等取流成功后再连：避免「取流 + 房间信息 + 弹幕 token」同一秒齐发。
    }

    /// 断流浮层 / 取流失败里的「重新连接」：重拉一次播放地址。
    pub(crate) fn retry_live(&mut self, cx: &mut Context<Self>) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        let room_id = live.room_id;
        let qn = self
            .source
            .as_ref()
            .map(|source| source.quality_id)
            .filter(|qn| *qn > 0)
            .unwrap_or(LIVE_DEFAULT_QN);
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        if let Some(live) = self.live.as_mut() {
            live.loading = true;
            live.outage = None;
            live.error = None;
        }
        self.loading_source = true;
        self.model.clear_error();
        self.model.state.playing = true;
        self.source = None;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.worker = None;
            self.current = None;
        }
        #[cfg(target_os = "windows")]
        {
            self.needs_image_clear = true;
        }
        self.tried_worker = false;
        self.first_frame_seen = false;
        self.no_frame_logged = false;
        self.last_frame_at = std::time::Instant::now();
        self.finish_handled = false;
        eprintln!("[live] 重新连接：room={room_id} qn={qn}");
        self.fetch_live_source(qn, generation, None, cx);
    }

    /// 切直播画质：按新 qn 重取流，重建 worker。
    pub(crate) fn select_live_quality(&mut self, qn: i32, cx: &mut Context<Self>) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        if live.quality_switching {
            return;
        }
        if self
            .source
            .as_ref()
            .map(|source| source.quality_id == qn)
            .unwrap_or(false)
        {
            self.close_control_popovers(cx);
            return;
        }
        let generation = self.load_generation;
        let quality_generation = live.quality_generation.wrapping_add(1);
        if let Some(live) = self.live.as_mut() {
            live.quality_generation = quality_generation;
            live.quality_switching = true;
        }
        self.quality_popover_open = false;
        self.control_popover_bounds.set(None);
        eprintln!("[live] 切换画质：qn={qn}");
        cx.notify();
        self.fetch_live_source(qn, generation, Some(quality_generation), cx);
    }

    /// 发送直播弹幕：`POST /msg/send`，成功才清空输入框。
    pub(crate) fn send_live_danmaku(
        &mut self,
        room_id: i64,
        text: String,
        session: UserSession,
        input: gpui::Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let echo = text.clone();
        // 本地发送限速：连点 / 回车按住时不要打到服务端。
        if let Some(live) = self.live.as_mut() {
            if live
                .chat
                .last_sent_at
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(1500))
            {
                self.show_interaction_note("发送太快，稍等一下".into(), true, cx);
                return;
            }
            live.chat.last_sent_at = Some(std::time::Instant::now());
        }
        self.danmaku_sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::live::send_live_danmaku(&client, room_id, &text, &cookie, &csrf)
                })
                .await;
            let _ = this.update_in(cx, move |view, window, cx| {
                view.danmaku_sending = false;
                match result {
                    Ok(()) => {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                        // 本地回显：服务端通常也会把这条推回聊天流，按「自己发的」
                        // 先显示一次，用户不用等一个来回才看到自己说话。
                        if let Some(live) = view.live.as_mut() {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            live.chat.push(yk_infra::live_chat::LiveChatMessage {
                                uid: 0,
                                uname: "我".to_string(),
                                text: echo,
                                kind: yk_infra::live_chat::LiveChatKind::Danmaku,
                                color: None,
                                guard_level: 0,
                                is_admin: false,
                                ts: now,
                            });
                            live.chat.scroll.scroll_to_bottom();
                        }
                        view.show_interaction_note("弹幕已发送".into(), false, cx);
                    }
                    Err(error) => {
                        yk_infra::error_log::error("live-chat", format!("发送失败：{error}"));
                        // 失败不清输入框：用户改一改就能重发。
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 建立直播聊天连接，并启动 120ms 轮询任务把事件搬进 UI。
    pub(crate) fn start_live_chat(&mut self, cx: &mut Context<Self>) {
        let Some(room_id) = self.live.as_ref().map(|live| live.room_id) else {
            return;
        };
        let session = crate::controller::current_session();
        let cookie = session
            .as_ref()
            .map(|session| session.cookie_header())
            .unwrap_or_default();
        let uid = session
            .as_ref()
            .map(|session| session.dede_user_id)
            .unwrap_or(0);
        let generation = {
            let Some(live) = self.live.as_mut() else {
                return;
            };
            live.chat.begin_connect();
            let (handle, rx) = connect_live_chat(room_id, cookie, uid);
            live.chat.handle = Some(handle);
            live.chat.rx = Some(rx);
            live.chat.generation
        };
        eprintln!("[live-chat] 开始连接：room={room_id} uid={uid}");
        cx.notify();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let alive = this.update(cx, |view, cx| {
                let current = view
                    .live
                    .as_ref()
                    .map(|live| live.chat.generation);
                if current != Some(generation) {
                    return false;
                }
                view.pump_live_chat(cx);
                // 连接终止（Failed / stop）后不再轮询，用户点「重新连接」会开新任务。
                view.live
                    .as_ref()
                    .map(|live| live.chat.rx.is_some())
                    .unwrap_or(false)
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// 把通道里的聊天事件搬进环形缓冲。返回是否还有连接需要继续轮询。
    pub(crate) fn pump_live_chat(&mut self, cx: &mut Context<Self>) {
        let events: Vec<LiveChatEvent> = {
            let Some(live) = self.live.as_mut() else {
                return;
            };
            let Some(rx) = live.chat.rx.as_ref() else {
                return;
            };
            rx.try_iter().take(64).collect()
        };
        if events.is_empty() {
            return;
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let mut appended = false;
        for event in events {
            match event {
                LiveChatEvent::Connected => {
                    live.chat.connection = ChatConnection::Connected;
                }
                LiveChatEvent::Reconnecting {
                    attempt,
                    delay_secs,
                } => {
                    live.chat.connection = ChatConnection::Reconnecting {
                        attempt,
                        delay_secs,
                    };
                }
                LiveChatEvent::Failed(error) => {
                    live.chat.connection = ChatConnection::Failed(error);
                    // 终止轮询；用户可在面板里点「重新连接」。
                    live.chat.rx = None;
                    live.chat.handle = None;
                }
                LiveChatEvent::Message(message) => {
                    // 本地回显与服务端回显去重：自己刚发的那条文字相同、
                    // 15 秒内、uid=0，就把回显补成真实身份而不是再插一行。
                    let echo_index = if message.uid != 0 {
                        live.chat
                            .messages
                            .iter()
                            .enumerate()
                            .rev()
                            .take(6)
                            .find(|(_, existing)| {
                                existing.uid == 0
                                    && existing.text == message.text
                                    && message.ts.saturating_sub(existing.ts) < 15
                            })
                            .map(|(index, _)| index)
                    } else {
                        None
                    };
                    if let Some(index) = echo_index {
                        if let Some(existing) = live.chat.messages.get_mut(index) {
                            existing.uid = message.uid;
                            existing.uname = message.uname;
                            existing.color = message.color;
                            existing.guard_level = message.guard_level;
                            existing.is_admin = message.is_admin;
                        }
                    } else {
                        live.chat.push(message);
                        appended = true;
                    }
                }
            }
        }
        if appended {
            live.chat.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// 按需拉房间信息（标题 / 主播 / 分区 / 人气），只在用户打开
    /// 「直播信息」Tab 时发一次；失败不挡聊天与播放。
    pub(crate) fn ensure_live_room_info(&mut self, cx: &mut Context<Self>) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        if live.room_info_loaded || live.room_info_loading {
            return;
        }
        if let Some(live) = self.live.as_mut() {
            live.room_info_loading = true;
        }
        let generation = self.load_generation;
        self.spawn_live_room_info(generation, cx);
    }

    fn spawn_live_room_info(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(room_id) = self.live.as_ref().map(|live| live.room_id) else {
            return;
        };
        let cookie = crate::controller::current_session_cookie();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    fetch_live_room_info(&client, room_id, &cookie)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                if let Some(live) = view.live.as_mut() {
                    live.room_info_loading = false;
                }
                let Ok(info) = result else {
                    return;
                };
                let uid_changed = view
                    .live
                    .as_ref()
                    .map(|live| live.uid != info.uid && info.uid > 0)
                    .unwrap_or(false);
                if let Some(live) = view.live.as_mut() {
                    live.room_info_loaded = true;
                    if !info.title.is_empty() {
                        live.title = info.title.clone();
                    }
                    if !info.uname.is_empty() {
                        live.uname = info.uname.clone();
                    }
                    if !info.face.is_empty() {
                        live.face = info.face.clone();
                    }
                    if !info.area_name.is_empty() {
                        live.area_name = info.area_name.clone();
                    }
                    if info.uid > 0 {
                        live.uid = info.uid;
                    }
                    live.online = info.online;
                    if !info.watched_text.is_empty() {
                        live.watched_text = info.watched_text.clone();
                    }
                }
                if let Some(source) = view.source.as_mut() {
                    if source.is_live {
                        if !info.title.is_empty() {
                            source.title = info.title.clone();
                        }
                        if !info.uname.is_empty() {
                            source.owner_name = info.uname.clone();
                        }
                        if !info.face.is_empty() {
                            source.owner_face = info.face.clone();
                        }
                        if !info.area_name.is_empty() {
                            source.category = info.area_name.clone();
                        }
                        if info.uid > 0 {
                            source.owner_mid = info.uid;
                        }
                    }
                }
                if uid_changed {
                    view.spawn_live_follow_state(info.uid, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 直播断流 / 下播：停住画面，保留浮窗等用户选择，绝不自动关闭。
    pub(crate) fn on_live_finished(&mut self, cx: &mut Context<Self>) {        if self.finish_handled {
            return;
        }
        self.finish_handled = true;
        self.model.state.playing = false;
        self.model.controls_visible = true;
        self.last_activity = std::time::Instant::now();
        if let Some(live) = self.live.as_mut() {
            live.loading = false;
            live.outage = Some("直播已结束或连接中断".to_string());
        }
        eprintln!("[live] 断开：等待用户重新连接");
        cx.notify();
    }

    /// 取流公共流程。`quality_generation = Some(_)` 表示这次是换画质。
    fn fetch_live_source(
        &mut self,
        qn: i32,
        generation: u64,
        quality_generation: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        let room_id = live.room_id;
        self.loading_source = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let cookie = crate::controller::current_session_cookie();
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    fetch_live_playback(&client, room_id, qn, &cookie)
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                let Some(live) = view.live.as_mut() else {
                    return;
                };
                if let Some(quality_generation) = quality_generation {
                    if live.quality_generation != quality_generation {
                        return;
                    }
                }
                view.loading_source = false;
                live.loading = false;
                live.quality_switching = false;
                match result {
                    Ok(playback) => {
                        view.apply_live_playback(playback, quality_generation.is_some(), cx);
                    }
                    Err(error) => {
                        yk_infra::error_log::error("live", format!("取流失败：{error}"));
                        live.error = Some(error.clone());
                        view.model.set_error(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 取流成功：换成直播源并重建 worker。
    fn apply_live_playback(
        &mut self,
        playback: LivePlayback,
        quality_switch: bool,
        cx: &mut Context<Self>,
    ) {
        let uid = if playback.uid > 0 {
            playback.uid
        } else {
            self.live.as_ref().map(|live| live.uid).unwrap_or(0)
        };
        eprintln!(
            "[live] 取流成功：room={} 协议={} qn={} title={}",
            playback.room_id,
            if playback.is_flv { "http_stream" } else { "http_hls" },
            playback.quality_id,
            playback.title
        );
        if let Some(live) = self.live.as_mut() {
            if !playback.title.is_empty() {
                live.title = playback.title.clone();
            }
            if playback.uid > 0 {
                live.uid = playback.uid;
            }
            if !playback.uname.is_empty() {
                live.uname = playback.uname.clone();
            }
            if !playback.face.is_empty() {
                live.face = playback.face.clone();
            }
            if !playback.area_name.is_empty() {
                live.area_name = playback.area_name.clone();
            }
            live.is_flv = playback.is_flv;
            live.error = None;
            live.outage = None;
        }
        self.model.clear_error();
        self.model.state.duration_secs = 0.0;
        self.model.state.playing = true;
        self.finish_handled = false;
        if !quality_switch {
            self.play_time = 0.0;
            self.model.state.position_secs = 0.0;
        }
        self.start_position = 0.0;
        // 换源 / 换流：先停旧 worker，下一帧 render 用新 source 重建。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.worker = None;
            self.current = None;
        }
        #[cfg(target_os = "windows")]
        {
            self.needs_image_clear = true;
        }
        self.tried_worker = false;
        self.first_frame_seen = false;
        self.no_frame_logged = false;
        self.last_frame_at = std::time::Instant::now();
        let mut source = playback_source(&playback);
        // 取流接口可能不带主播资料：用列表 / 房间信息兜底，面板不出现空名字。
        if let Some(live) = self.live.as_ref() {
            if source.title.is_empty() {
                source.title = live.title.clone();
            }
            if source.owner_name.is_empty() {
                source.owner_name = live.uname.clone();
            }
            if source.owner_face.is_empty() {
                source.owner_face = live.face.clone();
            }
            if source.owner_mid <= 0 {
                source.owner_mid = live.uid;
            }
            if source.category.is_empty() {
                source.category = live.area_name.clone();
            }
        }
        self.source = Some(source);
        // 关注按钮 / 粉丝数：直播只保留「关注主播」这一项互动。
        self.spawn_live_follow_state(uid, cx);
        if !quality_switch {
            let need_chat = self
                .live
                .as_ref()
                .map(|live| live.chat.rx.is_none())
                .unwrap_or(false);
            if need_chat {
                // 取流成功之后再连聊天：getDanmuInfo 与取流至少隔一个节流窗口。
                self.start_live_chat(cx);
            }
        }
        cx.notify();
    }
}

/// 把取流结果收敛成共享播放源（供 worker / 面板 / 控制栏只读）。
fn playback_source(playback: &LivePlayback) -> PlaybackSource {
    PlaybackSource {
        video_url: playback.video_url.clone(),
        audio_url: None,
        title: playback.title.clone(),
        desc: String::new(),
        pubdate: 0,
        category: playback.area_name.clone(),
        quality: playback.quality_label.clone(),
        quality_id: playback.quality_id,
        qualities: playback
            .qualities
            .iter()
            .map(|quality| PlaybackQuality {
                id: quality.id,
                label: quality.label.clone(),
                vip_only: false,
            })
            .collect(),
        duration_secs: 0,
        video_width: playback.video_width,
        video_height: playback.video_height,
        aid: 0,
        bvid: String::new(),
        cid: 0,
        owner_mid: playback.uid,
        owner_name: playback.uname.clone(),
        owner_face: playback.face.clone(),
        owner_sign: String::new(),
        view_count: 0,
        danmaku_count: 0,
        reply_count: 0,
        like_count: 0,
        coin_count: 0,
        favorite_count: 0,
        share_count: 0,
        pages: Vec::new(),
        current_page: 1,
        is_live: true,
        live_room_id: playback.room_id,
    }
}

/// 直播没有「总时长」，已看时长用 `mm:ss`；超过一小时用 `h:mm:ss`。
pub(crate) fn watched_text(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
