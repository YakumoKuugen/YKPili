//! 关注直播 + 关注状态探针。
//!
//! `cargo run -p yk-infra --example live_follow_probe`

fn main() {
    let session = yk_infra::session::load(&yk_infra::session::session_path());
    let Some(session) = session else {
        println!("没有登录会话");
        return;
    };
    let cookie = session.cookie_header();
    let client = yk_infra::api::client::ReqwestClient::new().expect("HTTP 客户端");
    match yk_infra::live::fetch_following_lives(&client, &cookie, 1) {
        Ok(page) => {
            for room in page.items.iter().take(5) {
                let followed = yk_infra::api::client::fetch_followed(&client, &cookie, room.uid);
                println!(
                    "room={} uid={} {} -> followed={:?}",
                    room.room_id, room.uid, room.uname, followed
                );
            }
        }
        Err(error) => println!("列表失败：{error}"),
    }
}
