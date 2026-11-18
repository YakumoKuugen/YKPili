//! 直播接口探针：用本机已登录的会话真实打一次列表 + 取流。
//!
//! 只打印房间摘要与流地址前缀，不打印 Cookie。
//! `cargo run -p yk-infra --example live_probe`

use yk_infra::api::bilibili::HttpClient;
use yk_infra::api::client::ReqwestClient;
use yk_infra::live::{fetch_following_lives, fetch_live_playback};

fn main() {
    let path = yk_infra::session::session_path();
    let Some(session) = yk_infra::session::load(&path) else {
        eprintln!("没有登录会话：{}", path.display());
        return;
    };
    if !session.is_valid() {
        eprintln!("登录会话已失效：{}", path.display());
        return;
    }
    let cookie = session.cookie_header();
    let client = ReqwestClient::new().expect("HTTP 客户端");

    match fetch_following_lives(&client, &cookie, 1) {
        Ok(page) => {
            println!(
                "[list] {} 个正在直播（has_more={} note={:?}）",
                page.items.len(),
                page.has_more,
                page.note
            );
            for room in page.items.iter().take(5) {
                println!(
                    "  room={} uid={} {} / {} / 看过 {}",
                    room.room_id,
                    room.uid,
                    room.uname,
                    room.area_name,
                    room.watched_label()
                );
            }
            if let Some(first) = page.items.first() {
                match yk_infra::live::fetch_live_room_info(&client, first.room_id, &cookie) {
                    Ok(info) => println!(
                        "[info] room={} {} / {} · {} · {} 人气 · 看过 {}",
                        info.room_id,
                        info.uname,
                        info.area_name,
                        info.parent_area_name,
                        info.online,
                        info.watched_text
                    ),
                    Err(error) => println!("[info] 失败：{error}"),
                }
                if std::env::var("YK_LIVE_RAW").is_ok() {
                    let req = yk_infra::api::bilibili::live_following(1, 5, &cookie);
                    if let Ok(body) = client.get(&req) {
                        println!("[raw-list] {}", body.chars().take(4000).collect::<String>());
                    }
                    let req = yk_infra::api::bilibili::live_play_url(first.room_id, 10000, &cookie);
                    if let Ok(body) = client.get(&req) {
                        println!("[raw-play] {}", body.chars().take(6000).collect::<String>());
                    }
                }
                match fetch_live_playback(&client, first.room_id, 10000, &cookie) {
                    Ok(playback) => {
                        let prefix: String = playback.video_url.chars().take(80).collect();
                        println!(
                            "[play] room={} 协议={} qn={}（{}）画质 {} 项",
                            playback.room_id,
                            if playback.is_flv { "FLV" } else { "HLS" },
                            playback.quality_id,
                            playback.quality_label,
                            playback.qualities.len()
                        );
                        println!("       url={prefix}…");
                    }
                    Err(error) => println!("[play] 失败：{error}"),
                }
            }
        }
        Err(error) => println!("[list] 失败：{error}"),
    }
}
