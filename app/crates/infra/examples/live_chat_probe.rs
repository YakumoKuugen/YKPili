//! 直播弹幕探针：连一个直播间，打印连接事件与前几条消息。
//!
//! `cargo run -p yk-infra --example live_chat_probe [room_id]`

use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use yk_infra::live_chat::{connect_live_chat, LiveChatEvent};

fn main() {
    let session = yk_infra::session::load(&yk_infra::session::session_path());
    let cookie = session
        .as_ref()
        .map(|session| session.cookie_header())
        .unwrap_or_default();
    let uid = session
        .as_ref()
        .map(|session| session.dede_user_id)
        .unwrap_or(0);

    let room_id = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            let client = yk_infra::api::client::ReqwestClient::new().ok()?;
            yk_infra::live::fetch_following_lives(&client, &cookie, 1)
                .ok()?
                .items
                .first()
                .map(|room| room.room_id)
        });
    let Some(room_id) = room_id else {
        println!("没有可用直播间");
        return;
    };
    println!("连接直播间 {room_id}（uid={uid}）");
    let (_handle, rx) = connect_live_chat(room_id, cookie, uid);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut messages = 0;
    while std::time::Instant::now() < deadline && messages < 8 {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(LiveChatEvent::Connected) => println!("[event] 已连接"),
            Ok(LiveChatEvent::Reconnecting {
                attempt,
                delay_secs,
            }) => println!("[event] 重连 {attempt}（{delay_secs}s）"),
            Ok(LiveChatEvent::Failed(error)) => {
                println!("[event] 失败：{error}");
                return;
            }
            Ok(LiveChatEvent::Message(message)) => {
                messages += 1;
                println!(
                    "[{:?}] {}: {}",
                    message.kind, message.uname, message.text
                );
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    println!("收到 {messages} 条消息");
}
