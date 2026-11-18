//! 直播接口风控诊断：按固定间隔逐个打接口，打印每个接口的 code。
//!
//! `cargo run -p yk-infra --example live_risk_probe [room_id]`
//!
//! 用来确认 `-352` 到底是列表 / 取流 / 房间信息 / 弹幕 token 哪一个触发的。

use std::time::Duration;

use serde_json::Value;
use yk_infra::api::bilibili::{self, HttpClient, Request};
use yk_infra::api::client::ReqwestClient;

fn code_of(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            let code = value.get("code")?.as_i64()?;
            let message = value
                .get("message")
                .or_else(|| value.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(format!("code={code} {message}"))
        })
        .unwrap_or_else(|| format!("非 JSON：{}", body.chars().take(80).collect::<String>()))
}

fn probe(client: &ReqwestClient, label: &str, req: &Request) {
    let path = req.url.split('?').next().unwrap_or(&req.url);
    match client.get(req) {
        Ok(body) => println!("  {label:12} {path} -> {}", code_of(&body)),
        Err(error) => println!("  {label:12} {path} -> HTTP/网络错误：{error}"),
    }
}

fn main() {
    let session = yk_infra::session::load(&yk_infra::session::session_path());
    let cookie = session
        .as_ref()
        .map(|session| session.cookie_header())
        .unwrap_or_default();
    println!(
        "登录会话：{}（带 Cookie={}）",
        session.is_some(),
        !cookie.is_empty()
    );
    let client = ReqwestClient::new().expect("HTTP 客户端");
    let room_id = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(21686237);

    // 1. 关注直播列表
    probe(
        &client,
        "list",
        &bilibili::live_following(1, 30, &cookie),
    );
    std::thread::sleep(Duration::from_secs(2));

    // 2. 取流
    probe(
        &client,
        "playurl",
        &bilibili::live_play_url(room_id, 10000, &cookie),
    );
    std::thread::sleep(Duration::from_secs(2));

    // 3. 房间信息
    probe(
        &client,
        "room_info",
        &bilibili::live_room_info(room_id, &cookie),
    );
    std::thread::sleep(Duration::from_secs(2));

    // 4. 弹幕 token（WBI 签名；连打两次观察是否单独触发限制）
    let keys = yk_infra::api::client::fetch_wbi_keys(&client, &cookie);
    match keys {
        Ok((img, sub)) => {
            let wts = yk_infra::api::client::now_wts();
            let req = bilibili::live_danmu_info(room_id, &cookie, &img, &sub, wts);
            probe(&client, "danmu_wbi", &req);
            std::thread::sleep(Duration::from_secs(5));
            let req = bilibili::live_danmu_info(
                room_id,
                &cookie,
                &img,
                &sub,
                yk_infra::api::client::now_wts(),
            );
            probe(&client, "danmu_again", &req);
        }
        Err(error) => println!("  WBI keys 获取失败：{error}"),
    }
}
