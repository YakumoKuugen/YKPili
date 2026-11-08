//! 消息中心的离屏渲染验收：互动列表 / 私信列表 / 会话页各一张。
//!
//! 默认跳过；本地验收：`YK_VISUAL=1 cargo test -p yk-app --test messages_visual`。
//! 输出 `/tmp/ykpili-messages-*.ppm`，用 `sips -s format png` 转 PNG 目检。

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::Route;
use yk_app::AppRoot;
use yk_infra::messages::{
    ChatKind, ChatMessage, Conversation, MessageChannel, MessageEntry, MessageTarget, MessageUser,
    UnreadCounts,
};
use yk_domain::models::UserSession;

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn capture(app: impl FnOnce() -> AppRoot, path: &str, height: f32) {
    let mut cx = context();
    let (handle, _) = cx
        .update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1280.), px(height)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                    cx.new(|_| app())
                },
            )
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    let image = cx.capture_screenshot(handle).expect("headless screenshot");
    let (width, height) = image.dimensions();
    let raw = image.into_raw();
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in raw.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, ppm).unwrap();
    eprintln!("[visual] {path} {width}x{height}");
}

fn now() -> u64 {
    yk_infra::api::client::now_wts()
}

fn user(mid: i64, name: &str) -> MessageUser {
    MessageUser {
        mid,
        nickname: name.into(),
        avatar: String::new(),
    }
}

fn entry(
    id: i64,
    channel: MessageChannel,
    nick: &str,
    action: &str,
    summary: &str,
    time: u64,
    last_view_at: u64,
) -> MessageEntry {
    MessageEntry {
        id,
        channel,
        user: user(id, nick),
        action: action.into(),
        summary: summary.into(),
        target_title: "【4K】夜景延时 · 第一视角".into(),
        uri: "https://www.bilibili.com/video/BV1xx411c7mD".into(),
        business: "comment".into(),
        business_id: 1,
        subject_id: 0,
        time,
        last_view_at,
        read_override: false,
        counts_reply: 1,
        is_up: false,
        likers: Vec::new(),
        like_count: 0,
        target: MessageTarget::Video {
            bvid: "BV1xx411c7mD".into(),
        },
    }
}

fn reply_entries() -> Vec<MessageEntry> {
    let now = now();
    vec![
        entry(
            1,
            MessageChannel::Reply,
            "阿茶",
            "回复了你的评论",
            "这个转场其实在 12 秒那里就切了，4K 下能看清帧边界；不过 30 帧的素材补到 60 帧之后，暗部会有一点…",
            now - 86_400,
            now - 80_000,
        ),
        entry(
            2,
            MessageChannel::Reply,
            "-1℃",
            "回复了你的评论",
            "你说得对，但 4K 其实不是重点——重点是这版调色把暗部拉回来了，原始素材是 8bit 的…",
            now - 120,
            now - 3_600,
        ),
        entry(
            3,
            MessageChannel::Reply,
            "空投猪",
            "在楼中楼回复了你",
            "我试了一下 RIFE 4.6，光流跑 1/2 分辨率，1080p 大概 12 帧。",
            now - 900,
            now - 3_600,
        ),
        entry(
            4,
            MessageChannel::Reply,
            "量子菠萝",
            "回复了你的评论",
            "这篇的结论和我这边测的差不多，主要差在编码器参数上。",
            now - 7_200,
            now - 3_600,
        ),
    ]
}

fn mention_entries() -> Vec<MessageEntry> {
    let now = now();
    let mut item = entry(
        20,
        MessageChannel::Mention,
        "空投猪",
        "在评论中 @ 了你",
        "回复 @阿茶 :这个补帧参数我也想要，RIFE 4.6 的模型在哪下？",
        now - 60,
        0,
    );
    item.target_title = "【4K】夜景延时 · 第一视角".into();
    vec![item]
}

fn like_entries() -> Vec<MessageEntry> {
    let now = now();
    let mut like = entry(
        10,
        MessageChannel::Like,
        "阿茶",
        "赞了你的评论",
        "原来 4K 片源在这里，难怪我怎么压都不对…",
        now - 1_800,
        now - 3_600,
    );
    like.like_count = 8;
    like.likers = (0..4)
        .map(|index| user(index, ["阿茶", "-1℃", "空投猪", "量子菠萝"][index as usize]))
        .collect();
    like.action = "赞了你的评论".into();
    vec![like]
}

fn conversations() -> Vec<Conversation> {
    let now = now();
    vec![
        Conversation {
            talker_id: 1,
            nickname: "阿茶".into(),
            avatar: String::new(),
            mutual: Some(true),
            unread: 2,
            last_msg: "收到，我试试 60 帧输出，看看会不会糊".into(),
            last_msg_ts: now - 120,
            last_msg_mine: false,
            session_ts: now - 120,
            last_seqno: 12,
            is_follow: true,
        },
        Conversation {
            talker_id: 2,
            nickname: "-1℃".into(),
            avatar: String::new(),
            mutual: Some(true),
            unread: 0,
            last_msg: "这个参数在 12 秒那里会掉帧吗？".into(),
            last_msg_ts: now - 3_600,
            last_msg_mine: true,
            session_ts: now - 3_600,
            last_seqno: 8,
            is_follow: true,
        },
        Conversation {
            talker_id: 3,
            nickname: "空投猪".into(),
            avatar: String::new(),
            mutual: Some(false),
            unread: 0,
            last_msg: "消息已撤回".into(),
            last_msg_ts: now - 86_400,
            last_msg_mine: false,
            session_ts: now - 86_400,
            last_seqno: 3,
            is_follow: false,
        },
    ]
}

fn chat_messages() -> Vec<ChatMessage> {
    let now = now();
    vec![
        ChatMessage {
            seqno: 1,
            sender_uid: 0,
            timestamp: now - 3_600,
            kind: ChatKind::System,
            text: "你们已互相关注，开始聊天吧".into(),
            mine: false,
        },
        ChatMessage {
            seqno: 2,
            sender_uid: 1,
            timestamp: now - 3_500,
            kind: ChatKind::Text,
            text: "上次说的那个 4K 补帧参数，你用的哪套？".into(),
            mine: false,
        },
        ChatMessage {
            seqno: 3,
            sender_uid: 99,
            timestamp: now - 3_400,
            kind: ChatKind::Text,
            text: "RIFE 4.6，光流跑 1/2 分辨率，1080p 大概 12 帧。".into(),
            mine: true,
        },
        ChatMessage {
            seqno: 4,
            sender_uid: 1,
            timestamp: now - 120,
            kind: ChatKind::Text,
            text: "收到，我试试 60 帧输出，看看会不会糊。".into(),
            mine: false,
        },
        ChatMessage {
            seqno: 5,
            sender_uid: 99,
            timestamp: now - 60,
            kind: ChatKind::Revoked,
            text: "消息已撤回".into(),
            mine: true,
        },
    ]
}

fn render_reply() {
    capture(
        || {
            AppRoot::new().without_login_prompt().with_message_inbox(
                MessageChannel::Reply,
                reply_entries(),
                UnreadCounts {
                    reply: 3,
                    mention: 1,
                    like: 3,
                    dm: 2,
                },
            )
        },
        "/tmp/ykpili-messages-reply.ppm",
        900.0,
    );
}

fn render_mention() {
    capture(
        || {
            AppRoot::new().without_login_prompt().with_message_inbox(
                MessageChannel::Mention,
                mention_entries(),
                UnreadCounts {
                    reply: 0,
                    mention: 1,
                    like: 0,
                    dm: 0,
                },
            )
        },
        "/tmp/ykpili-messages-mention.ppm",
        760.0,
    );
}

fn render_like() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_message_inbox(
                    MessageChannel::Like,
                    like_entries(),
                    UnreadCounts {
                        reply: 0,
                        mention: 0,
                        like: 4,
                        dm: 0,
                    },
                )
                .with_message_expanded_like(10)
        },
        "/tmp/ykpili-messages-like.ppm",
        760.0,
    );
}

fn render_dm_mutual() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_message_conversations(
                    conversations(),
                    UnreadCounts {
                        reply: 0,
                        mention: 0,
                        like: 0,
                        dm: 2,
                    },
                )
                .with_message_only_mutual(true)
        },
        "/tmp/ykpili-messages-dm-mutual.ppm",
        760.0,
    );
}

fn render_dm_mutual_empty() {
    capture(
        || {
            let mut conversations = conversations();
            for conversation in &mut conversations {
                conversation.mutual = Some(false);
            }
            AppRoot::new()
                .without_login_prompt()
                .with_message_conversations(
                    conversations,
                    UnreadCounts {
                        reply: 0,
                        mention: 0,
                        like: 0,
                        dm: 0,
                    },
                )
                .with_message_only_mutual(true)
        },
        "/tmp/ykpili-messages-dm-mutual-empty.ppm",
        760.0,
    );
}

fn render_dm() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_message_conversations(
                    conversations(),
                    UnreadCounts {
                        reply: 0,
                        mention: 0,
                        like: 0,
                        dm: 2,
                    },
                )
        },
        "/tmp/ykpili-messages-dm.ppm",
        760.0,
    );
}

fn render_chat() {
    capture(
        || {
            let conversation = conversations()[0].clone();
            AppRoot::new()
                .without_login_prompt()
                .with_message_chat(conversation, chat_messages())
        },
        "/tmp/ykpili-messages-chat.ppm",
        820.0,
    );
}

fn render_chat_locked() {
    capture(
        || {
            let mut conversation = conversations()[2].clone();
            conversation.mutual = Some(false);
            AppRoot::new()
                .without_login_prompt()
                .with_message_chat(conversation, chat_messages())
        },
        "/tmp/ykpili-messages-chat-locked.ppm",
        820.0,
    );
}

fn fake_session() -> UserSession {
    UserSession {
        dede_user_id: 1,
        sess_data: "fake".into(),
        bili_jct: "fake".into(),
        nickname: "测试账号".into(),
        face_url: String::new(),
        is_logged_in: true,
        ..Default::default()
    }
}

fn render_dock_unread() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_session(Some(fake_session()))
                .with_message_inbox(
                    MessageChannel::Reply,
                    reply_entries(),
                    UnreadCounts {
                        reply: 3,
                        mention: 1,
                        like: 3,
                        dm: 2,
                    },
                )
        },
        "/tmp/ykpili-dock-unread.ppm",
        760.0,
    );
}

fn render_dock_tabs() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_message_inbox(
                    MessageChannel::Reply,
                    reply_entries(),
                    UnreadCounts {
                        reply: 3,
                        mention: 1,
                        like: 3,
                        dm: 2,
                    },
                )
                .with_dock_tabs(true)
        },
        "/tmp/ykpili-messages-dock-tabs.ppm",
        900.0,
    );
}

fn render_favorites() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_page(Route::Favorites)
        },
        "/tmp/ykpili-favorites.ppm",
        760.0,
    );
}

fn main() {
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[visual] skipped（设置 YK_VISUAL=1 生成消息中心截图）");
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        render_reply();
        render_mention();
        render_like();
        render_dm();
        render_dm_mutual();
        render_dm_mutual_empty();
        render_chat();
        render_chat_locked();
        render_dock_unread();
        render_dock_tabs();
        render_favorites();
        eprintln!("[visual] done");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    eprintln!("[visual] skipped：当前平台没有 GPUI headless renderer");
}
