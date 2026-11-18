//! 弹幕 token 请求 A/B：裸请求 / 带 web_location / WBI 签名。
//!
//! `cargo run -p yk-infra --example live_danmu_probe [room_id]`

use std::time::Duration;

use serde_json::Value;
use yk_infra::api::bilibili::{self, HttpClient, Request};
use yk_infra::api::client::{fetch_wbi_keys, now_wts, ReqwestClient};
use yk_infra::api::wbi;

fn code_of(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .map(|value| {
            let code = value.get("code").and_then(Value::as_i64).unwrap_or(-999);
            let message = value
                .get("message")
                .or_else(|| value.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("");
            format!("code={code} {message}")
        })
        .unwrap_or_else(|| "非 JSON".to_string())
}

fn main() {
    let session = yk_infra::session::load(&yk_infra::session::session_path());
    let cookie = session
        .as_ref()
        .map(|session| session.cookie_header())
        .unwrap_or_default();
    let client = ReqwestClient::new().expect("HTTP 客户端");
    let room_id = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1016);
    let base = format!(
        "{}/xlive/web-room/v1/index/getDanmuInfo",
        bilibili::LIVE_API_BASE
    );
    let mut urls: Vec<(&str, String)> = vec![
        ("plain", format!("{base}?id={room_id}&type=0")),
        (
            "web_location",
            format!("{base}?id={room_id}&type=0&web_location=444.8"),
        ),
    ];
    match fetch_wbi_keys(&client, &cookie) {
        Ok((img, sub)) => {
            let params = vec![
                ("id".to_string(), room_id.to_string()),
                ("type".to_string(), "0".to_string()),
                ("web_location".to_string(), "444.8".to_string()),
            ];
            urls.push((
                "wbi",
                wbi::signed_url(&base, &params, &img, &sub, now_wts()),
            ));
        }
        Err(error) => println!("WBI keys 获取失败：{error}"),
    }

    for (label, url) in urls {
        let req = Request {
            url,
            cookie: cookie.clone(),
            name: "danmu_probe",
        };
        match client.get(&req) {
            Ok(body) => println!("{label:12} -> {}", code_of(&body)),
            Err(error) => println!("{label:12} -> 网络错误：{error}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
