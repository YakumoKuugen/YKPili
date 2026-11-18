//! 直播链路烟测：用本机会话取流 → 起 worker → 等首帧（静音）。
//!
//! `cargo run -p yk-player --example live_smoke`
//!
//! 只用于本机验收：需要已登录会话、libmpv 与网络。

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn run() {
    let path = yk_infra::session::session_path();
    let Some(session) = yk_infra::session::load(&path) else {
        println!("SKIP：没有登录会话");
        return;
    };
    let cookie = session.cookie_header();
    let client = yk_infra::api::client::ReqwestClient::new().expect("HTTP 客户端");
    let page = match yk_infra::live::fetch_following_lives(&client, &cookie, 1) {
        Ok(page) => page,
        Err(error) => {
            println!("FAIL：列表失败：{error}");
            return;
        }
    };
    let Some(room) = page.items.first() else {
        println!("SKIP：关注的主播都没开播");
        return;
    };
    let playback = match yk_infra::live::fetch_live_playback(&client, room.room_id, 10000, &cookie) {
        Ok(playback) => playback,
        Err(error) => {
            println!("FAIL：取流失败：{error}");
            return;
        }
    };
    println!(
        "取流：room={} 协议={} qn={} {}x{}",
        playback.room_id,
        if playback.is_flv { "FLV" } else { "HLS" },
        playback.quality_id,
        playback.video_width,
        playback.video_height
    );

    let worker = yk_player::video_worker::FrameWorker::spawn_with_audio(
        playback.video_url.clone(),
        None,
        1280,
        720,
        0.0,
        "https://live.bilibili.com/".to_string(),
    );
    let Some(worker) = worker else {
        println!("FAIL：渲染线程启动失败");
        return;
    };
    worker.set_muted(true);
    worker.set_volume(0.0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut frames = 0;
    let mut last = 0.0;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        match worker.latest() {
            Some(yk_player::video_worker::WorkerFrame::Frame(frame, t)) => {
                frames += 1;
                last = t;
                if frames <= 2 {
                    println!(
                        "首帧：{}x{} serial={} t={t:.2}s",
                        frame.width, frame.height, frame.serial
                    );
                }
            }
            Some(yk_player::video_worker::WorkerFrame::Position(t)) => last = t,
            None => {}
        }
        if frames >= 5 {
            break;
        }
    }
    if frames >= 5 {
        println!("OK：收到 {frames} 帧，最后位置 {last:.2}s");
    } else if frames > 0 {
        println!("WARN：只收到 {frames} 帧，位置 {last:.2}s");
    } else {
        println!("FAIL：10s 内没有帧（位置 {last:.2}s）");
    }
}

fn main() {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    run();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    println!("SKIP：该平台没有视频 worker");
}
