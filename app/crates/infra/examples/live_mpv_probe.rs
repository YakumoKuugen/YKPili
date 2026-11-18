//! 直播 FLV 的 mpv 属性探针：time-pos / duration / eof-reached / seekable。
//!
//! 用来确认「播到某个时间点误判结束」是不是 FLV 带了有限 duration。
//!
//! `YK_LIBMPV=... cargo run -p yk-infra --example live_mpv_probe [room_id]`

use std::time::Duration;

fn main() {
    let session = yk_infra::session::load(&yk_infra::session::session_path());
    let cookie = session
        .as_ref()
        .map(|session| session.cookie_header())
        .unwrap_or_default();
    let client = yk_infra::api::client::ReqwestClient::new().expect("HTTP 客户端");
    let room_id = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
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
    let playback = match yk_infra::live::fetch_live_playback(&client, room_id, 10000, &cookie) {
        Ok(playback) => playback,
        Err(error) => {
            println!("取流失败：{error}");
            return;
        }
    };
    println!("room={room_id} 协议={} qn={}", if playback.is_flv { "FLV" } else { "HLS" }, playback.quality_id);
    let mut player = match yk_infra::media::MpvPlayer::new_with_start_and_referer(
        1280,
        720,
        0.0,
        "https://live.bilibili.com/",
    ) {
        Ok(player) => player,
        Err(error) => {
            println!("mpv 初始化失败：{error}");
            return;
        }
    };
    if let Err(error) = player.load_file(&playback.video_url) {
        println!("loadfile 失败：{error}");
        return;
    }
    for index in 0..15 {
        std::thread::sleep(Duration::from_secs(2));
        player.drain_events();
        let t = player.get_property_double("time-pos").unwrap_or(-1.0);
        let duration = player.get_property_double("duration").unwrap_or(-1.0);
        let eof = player.get_property_flag("eof-reached").unwrap_or(false);
        println!(
            "#{} t={t:.2}s duration={duration:.2}s eof={eof} finished={}",
            index + 1,
            yk_domain::player::is_playback_finished(t, duration, eof)
        );
    }
}
