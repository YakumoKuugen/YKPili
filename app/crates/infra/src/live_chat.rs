//! 直播弹幕聊天：同步 WebSocket 连接、协议解析与重连。
//!
//! 与 rest 同样的风格：**阻塞** `tungstenite` + rustls，不引入 async runtime。
//! 连接在独立线程里跑，UI 侧只拿一个 `mpsc::Receiver<LiveChatEvent>`：
//!
//! ```text
//! connect_live_chat(room_id, cookie, uid)
//!   └─ 线程：getDanmuInfo → wss://{host}:{wss_port}/sub
//!        ├─ 认证包（op=7）→ op=8 {"code":0} → Connected
//!        ├─ 心跳 30s（op=2）
//!        └─ op=5 通知包（zlib）→ LiveChatMessage
//! ```
//!
//! 断线退避重连 3 次（2 / 5 / 10s），之后发 `Failed`；UI 只负责展示与手动重连。
//! 协议参考 `docs/关注页-动态与直播分栏.md` 6.8。

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tungstenite::client::IntoClientRequest;

use crate::api::bilibili::{self, HttpClient};

/// 一条直播聊天消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveChatMessage {
    pub uid: i64,
    pub uname: String,
    pub text: String,
    pub kind: LiveChatKind,
    /// 弹幕颜色（仅 DANMU_MSG 有）。
    pub color: Option<(u8, u8, u8)>,
    /// 舰队等级：0 普通 / 1 总督 / 2 提督 / 3 舰长。
    pub guard_level: i64,
    pub is_admin: bool,
    pub ts: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveChatKind {
    Danmaku,
    Gift,
    Guard,
    SuperChat,
    System,
}

/// 连接状态事件。
#[derive(Debug, Clone)]
pub enum LiveChatEvent {
    Connected,
    Reconnecting { attempt: u32, delay_secs: u64 },
    Failed(String),
    Message(LiveChatMessage),
}

/// 连接句柄：Drop 即通知线程退出。
pub struct LiveChatHandle {
    stop: Arc<AtomicBool>,
}

impl LiveChatHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for LiveChatHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 连接直播间聊天。返回句柄与事件接收端；网络 / 鉴权失败通过 `Failed` 事件返回。
pub fn connect_live_chat(room_id: i64, cookie: String, uid: i64) -> (LiveChatHandle, Receiver<LiveChatEvent>) {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    std::thread::Builder::new()
        .name(format!("yk-live-chat-{room_id}"))
        .spawn(move || run_chat(room_id, cookie, uid, stop_for_thread, tx))
        .ok();
    (LiveChatHandle { stop }, rx)
}

fn run_chat(
    room_id: i64,
    cookie: String,
    uid: i64,
    stop: Arc<AtomicBool>,
    tx: Sender<LiveChatEvent>,
) {
    // rustls 0.23 需要进程级 crypto provider；tungstenite 的默认构造不会装。
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut attempt = 0u32;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match run_once(room_id, &cookie, uid, &stop, &tx) {
            Ok(()) => return,
            Err(error) => {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                // -352 / 风控：再重试只会让限制更久，直接交给用户手动重连。
                if error.contains("-352") || error.contains("风控") || error.contains("限流") {
                    let _ = tx.send(LiveChatEvent::Failed(error));
                    return;
                }
                attempt += 1;
                if attempt > 3 {
                    let _ = tx.send(LiveChatEvent::Failed(error));
                    return;
                }
                let delay = match attempt {
                    1 => 2,
                    2 => 5,
                    _ => 10,
                };
                let _ = tx.send(LiveChatEvent::Reconnecting {
                    attempt,
                    delay_secs: delay,
                });
                eprintln!("[live-chat] 重连中（第 {attempt} 次，{delay}s 后）：{error}");
                wait_or_stop(&stop, delay);
            }
        }
    }
}

/// 调试开关读一次即可：收包热路径不重复查环境变量。
fn debug_enabled() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var_os("YK_LIVE_CHAT_DEBUG").is_some())
}

/// `getDanmuInfo` 的 token / 服务器缓存有效期。
///
/// 这个接口是 -352 的重灾区：每次重连都重新取一次，断线三次就是 4 个请求。
/// 实测 token 在几分钟内可复用，缓存 10 分钟足够覆盖一次直播会话。
const DANMU_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// 命中 -352 后这段时间内不再打接口，直接在本地失败，避免越重试越风控。
const DANMU_BLOCK_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct CachedDanmu {
    host: String,
    port: u16,
    token: String,
    at: Instant,
}

static DANMU_CACHE: OnceLock<Mutex<HashMap<i64, CachedDanmu>>> = OnceLock::new();
static DANMU_BLOCKED: OnceLock<Mutex<HashMap<i64, Instant>>> = OnceLock::new();

fn danmu_cache() -> &'static Mutex<HashMap<i64, CachedDanmu>> {
    DANMU_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn danmu_blocked() -> &'static Mutex<HashMap<i64, Instant>> {
    DANMU_BLOCKED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_danmu(room_id: i64) -> Option<CachedDanmu> {
    let mut cache = danmu_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cached = cache.get(&room_id).cloned()?;
    if cached.at.elapsed() >= DANMU_CACHE_TTL {
        cache.remove(&room_id);
        return None;
    }
    Some(cached)
}

fn store_danmu(room_id: i64, cached: CachedDanmu) {
    danmu_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(room_id, cached);
}

fn blocked_for(room_id: i64) -> Option<u64> {
    let mut blocked = danmu_blocked()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let until = blocked.get(&room_id).copied()?;
    let now = Instant::now();
    if until <= now {
        blocked.remove(&room_id);
        return None;
    }
    Some((until - now).as_secs().max(1))
}

fn block_room(room_id: i64) {
    danmu_blocked()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(room_id, Instant::now() + DANMU_BLOCK_TTL);
}

fn wait_or_stop(stop: &AtomicBool, seconds: u64) {    for _ in 0..seconds * 10 {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 一次完整连接（认证成功 → 读到断开）。返回 Ok(()) 表示被主动停止。
fn run_once(
    room_id: i64,
    cookie: &str,
    uid: i64,
    stop: &AtomicBool,
    tx: &Sender<LiveChatEvent>,
) -> Result<(), String> {
    let client = crate::api::client::ReqwestClient::new().map_err(|e| e.to_string())?;
    let (host, port, token) = fetch_danmu_server(&client, room_id, cookie)?;

    let url = format!("wss://{host}:{port}/sub");
    let mut request = url
        .clone()
        .into_client_request()
        .map_err(|e| format!("弹幕地址不合法：{e}"))?;
    {
        use tungstenite::http::header::HeaderValue;
        let headers = request.headers_mut();
        headers.insert("Origin", HeaderValue::from_static("https://live.bilibili.com"));
        headers.insert(
            "User-Agent",
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            ),
        );
    }
    let (mut socket, _) = tungstenite::connect(request)
        .map_err(|e| format!("弹幕连接失败（{host}:{port}）：{e}"))?;
    set_read_timeout(&mut socket, Duration::from_millis(800));

    // 认证包：protover=2（zlib），服务端会按这个压缩通知包。
    let auth = serde_json::json!({
        "uid": uid,
        "roomid": room_id,
        "protover": 2,
        "platform": "web",
        "type": 2,
        "key": token,
    });
    send_packet(&mut socket, 7, auth.to_string().as_bytes())
        .map_err(|e| format!("弹幕认证包发送失败：{e}"))?;

    let mut last_heartbeat = Instant::now();
    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = socket.close(None);
            return Ok(());
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(30) {
            last_heartbeat = Instant::now();
            send_packet(&mut socket, 2, b"")?;
        }
        match socket.read() {
            Ok(tungstenite::Message::Binary(data)) => {
                if let Some(error) = handle_packets(&data, tx, room_id) {
                    return Err(error);
                }
            }
            Ok(tungstenite::Message::Text(text)) => {
                handle_single_json(text.as_bytes(), tx);
            }
            Ok(tungstenite::Message::Ping(payload)) => {
                let _ = socket.send(tungstenite::Message::Pong(payload));
            }
            Ok(tungstenite::Message::Close(_)) => return Err("服务端关闭连接".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(format!("弹幕连接中断：{error}")),
        }
    }
}

fn set_read_timeout(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    timeout: Duration,
) {
    use tungstenite::stream::MaybeTlsStream;
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            let _ = stream.set_read_timeout(Some(timeout));
        }
        MaybeTlsStream::Rustls(stream) => {
            let _ = stream.get_mut().set_read_timeout(Some(timeout));
        }
        _ => {}
    }
}

fn send_packet(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    op: u32,
    body: &[u8],
) -> Result<(), String> {
    let mut packet = Vec::with_capacity(16 + body.len());
    let len = (16 + body.len()) as u32;
    packet.extend_from_slice(&len.to_be_bytes());
    packet.extend_from_slice(&16u16.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&op.to_be_bytes());
    packet.extend_from_slice(&1u32.to_be_bytes());
    packet.extend_from_slice(body);
    socket
        .send(tungstenite::Message::Binary(packet.into()))
        .map_err(|e| e.to_string())
}

fn fetch_danmu_server(
    client: &crate::api::client::ReqwestClient,
    room_id: i64,
    cookie: &str,
) -> Result<(String, u16, String), String> {
    // 1. 先看负缓存：刚被 -352 过就别再打接口了。
    if let Some(secs) = blocked_for(room_id) {
        return Err(format!(
            "弹幕接口刚触发风控（-352），{secs} 秒后再试"
        ));
    }
    // 2. token / host 未过期就复用：断线重连不再请求 getDanmuInfo。
    if let Some(cached) = cached_danmu(room_id) {
        return Ok((cached.host, cached.port, cached.token));
    }

    // 3. 取 WBI keys 并签名：getDanmuInfo 不签名会直接回 -352。
    let (img_key, sub_key) = crate::api::client::fetch_wbi_keys(client, cookie)?;
    let req = bilibili::live_danmu_info(
        room_id,
        cookie,
        &img_key,
        &sub_key,
        crate::api::client::now_wts(),
    );
    let body = client.get(&req)?;
    let value: Value =
        serde_json::from_str(&body).map_err(|e| format!("弹幕服务器信息解析失败：{e}"))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == -352 {
        block_room(room_id);
        crate::api::limits::penalize(
            crate::api::limits::Bucket::Live,
            Duration::from_secs(10),
        );
        return Err("弹幕服务限流（-352），请稍后重新连接".to_string());
    }
    if code != 0 {
        let message = value
            .get("message")
            .or_else(|| value.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(format!("弹幕服务器信息返回 code={code}（{message}）"));
    }
    let data = value
        .get("data")
        .ok_or_else(|| "弹幕服务器信息缺少 data".to_string())?;
    let token = data
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let host = data
        .get("host_list")
        .and_then(Value::as_array)
        .and_then(|hosts| hosts.first())
        .and_then(|host| host.get("host"))
        .and_then(Value::as_str)
        .unwrap_or("broadcastlv.chat.bilibili.com")
        .to_string();
    let port = data
        .get("host_list")
        .and_then(Value::as_array)
        .and_then(|hosts| hosts.first())
        .and_then(|host| host.get("wss_port"))
        .and_then(Value::as_u64)
        .unwrap_or(443) as u16;
    if !token.is_empty() {
        store_danmu(
            room_id,
            CachedDanmu {
                host: host.clone(),
                port,
                token: token.clone(),
                at: Instant::now(),
            },
        );
    }
    Ok((host, port, token))
}

/// 拆一个 WebSocket 二进制帧：可能包含多个包，version=2 还要先 zlib 解压。
fn handle_packets(data: &[u8], tx: &Sender<LiveChatEvent>, room_id: i64) -> Option<String> {
    let mut offset = 0usize;
    while offset + 16 <= data.len() {
        let len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        if len < 16 || offset + len > data.len() {
            return None;
        }
        let header_len = u16::from_be_bytes([data[offset + 4], data[offset + 5]]) as usize;
        let version = u16::from_be_bytes([data[offset + 6], data[offset + 7]]);
        let op = u32::from_be_bytes([
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
        ]);
        let body_start = offset + header_len.max(16);
        let body_end = offset + len;
        if body_start <= body_end && body_start <= data.len() {
            let body = &data[body_start..body_end.min(data.len())];
            if debug_enabled() && op != 2 {
                eprintln!(
                    "[live-chat] 包 op={op} ver={version} body={} bytes",
                    body.len()
                );
            }
            match version {
                // 2 = zlib 压缩包：解压后是若干个完整包。
                2 => {
                    let mut decoded = Vec::new();
                    match flate2::read::ZlibDecoder::new(body).read_to_end(&mut decoded) {
                        Ok(_) => {
                            if let Some(error) = handle_packets(&decoded, tx, room_id) {
                                return Some(error);
                            }
                        }
                        Err(error) => {
                            if debug_enabled() {
                                eprintln!("[live-chat] zlib 解压失败：{error}");
                            }
                        }
                    }
                }
                // 3 = brotli；我们用 protover=2 请求，服务端正常不会回 3。
                3 => {
                    if debug_enabled() {
                        eprintln!("[live-chat] 收到 brotli(v3) 包，未接入解压");
                    }
                }
                _ => {
                    if op == 5 {
                        handle_single_json(body, tx);
                    } else if op == 8 {
                        if let Some(error) = handle_auth_reply(body, tx, room_id) {
                            return Some(error);
                        }
                    }
                }
            }
        }
        offset += len;
    }
    None
}

fn handle_auth_reply(body: &[u8], tx: &Sender<LiveChatEvent>, room_id: i64) -> Option<String> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == 0 {
        let _ = tx.send(LiveChatEvent::Connected);
        eprintln!("[live-chat] 已连接");
        None
    } else {
        // token 过期 / 失效：清缓存，下一次重连会重新 getDanmuInfo。
        danmu_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&room_id);
        Some(format!("弹幕认证失败：code={code}"))
    }
}

fn handle_single_json(body: &[u8], tx: &Sender<LiveChatEvent>) {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    // 有些包是 `{"cmd":"...","data":{...}}`，认证回包没有 cmd —— 忽略。
    let Some(cmd) = value.get("cmd").and_then(Value::as_str) else {
        return;
    };
    if debug_enabled() {
        eprintln!("[live-chat] cmd={cmd}");
    }
    let Some(message) = parse_command(cmd, &value) else {
        return;
    };
    let _ = tx.send(LiveChatEvent::Message(message));
}

fn parse_command(cmd: &str, value: &Value) -> Option<LiveChatMessage> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if cmd.starts_with("DANMU_MSG") {
        let info = value.get("info")?.as_array()?;
        let text = info.get(1)?.as_str()?.to_string();
        let user = info.get(2)?.as_array()?;
        let uid = user.first().and_then(Value::as_i64).unwrap_or(0);
        let uname = user.get(1).and_then(Value::as_str).unwrap_or_default().to_string();
        let is_admin = user
            .get(2)
            .and_then(Value::as_i64)
            .map(|flag| flag != 0)
            .unwrap_or(false);
        let color = info
            .first()
            .and_then(|meta| meta.get(3))
            .and_then(Value::as_u64)
            .map(decimal_color);
        let guard_level = info
            .get(7)
            .and_then(Value::as_i64)
            .unwrap_or(0);
        return Some(LiveChatMessage {
            uid,
            uname,
            text,
            kind: LiveChatKind::Danmaku,
            color,
            guard_level,
            is_admin,
            ts: now,
        });
    }
    if cmd == "SEND_GIFT" {
        let data = value.get("data")?;
        let uname = data.get("uname").and_then(Value::as_str).unwrap_or_default();
        let gift = data.get("giftName").and_then(Value::as_str).unwrap_or_default();
        let num = data.get("num").and_then(Value::as_u64).unwrap_or(1);
        return Some(LiveChatMessage {
            uid: data.get("uid").and_then(Value::as_i64).unwrap_or(0),
            uname: uname.to_string(),
            text: format!("赠送 {gift} ×{num}"),
            kind: LiveChatKind::Gift,
            color: None,
            guard_level: 0,
            is_admin: false,
            ts: now,
        });
    }
    if cmd == "GUARD_BUY" {
        let data = value.get("data")?;
        let uname = data.get("username").and_then(Value::as_str).unwrap_or_default();
        let gift = data.get("gift_name").and_then(Value::as_str).unwrap_or_default();
        let num = data.get("num").and_then(Value::as_u64).unwrap_or(1);
        return Some(LiveChatMessage {
            uid: data.get("uid").and_then(Value::as_i64).unwrap_or(0),
            uname: uname.to_string(),
            text: format!("开通了 {gift} ×{num}"),
            kind: LiveChatKind::Guard,
            color: None,
            guard_level: 3,
            is_admin: false,
            ts: now,
        });
    }
    if cmd == "SUPER_CHAT_MESSAGE" {
        let data = value.get("data")?;
        let uname = data
            .get("user_info")
            .and_then(|info| info.get("uname"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let text = data.get("message").and_then(Value::as_str).unwrap_or_default();
        let price = data.get("price").and_then(Value::as_u64).unwrap_or(0);
        return Some(LiveChatMessage {
            uid: data.get("uid").and_then(Value::as_i64).unwrap_or(0),
            uname: uname.to_string(),
            text: if price > 0 {
                format!("【¥{price}】{text}")
            } else {
                text.to_string()
            },
            kind: LiveChatKind::SuperChat,
            color: None,
            guard_level: 0,
            is_admin: false,
            ts: now,
        });
    }
    None
}

fn decimal_color(decimal: u64) -> (u8, u8, u8) {
    (
        ((decimal >> 16) & 0xFF) as u8,
        ((decimal >> 8) & 0xFF) as u8,
        (decimal & 0xFF) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_danmaku_message() {
        let value: Value = serde_json::from_str(
            r#"{"cmd":"DANMU_MSG","info":[[0,1,25,16777215,0], "你好", [123,"小明",0,0,0,0,0,""], [], [12], ["x"], [], 3]}"#,
        )
        .unwrap();
        let message = parse_command("DANMU_MSG", &value).unwrap();
        assert_eq!(message.text, "你好");
        assert_eq!(message.uid, 123);
        assert_eq!(message.uname, "小明");
        assert_eq!(message.color, Some((255, 255, 255)));
        assert_eq!(message.guard_level, 3);
        assert_eq!(message.kind, LiveChatKind::Danmaku);
    }

    #[test]
    fn parses_gift_guard_and_super_chat() {
        let gift: Value = serde_json::from_str(
            r#"{"cmd":"SEND_GIFT","data":{"uid":1,"uname":"A","giftName":"辣条","num":3}}"#,
        )
        .unwrap();
        assert_eq!(parse_command("SEND_GIFT", &gift).unwrap().text, "赠送 辣条 ×3");

        let guard: Value = serde_json::from_str(
            r#"{"cmd":"GUARD_BUY","data":{"uid":2,"username":"B","gift_name":"舰长","num":1}}"#,
        )
        .unwrap();
        assert_eq!(parse_command("GUARD_BUY", &guard).unwrap().kind, LiveChatKind::Guard);

        let sc: Value = serde_json::from_str(
            r#"{"cmd":"SUPER_CHAT_MESSAGE","data":{"uid":3,"message":"加油","price":30,"user_info":{"uname":"C"}}}"#,
        )
        .unwrap();
        let sc = parse_command("SUPER_CHAT_MESSAGE", &sc).unwrap();
        assert_eq!(sc.text, "【¥30】加油");
        assert_eq!(sc.kind, LiveChatKind::SuperChat);
    }

    #[test]
    fn splits_multiple_packets_and_ignores_bad_lengths() {
        // 构造两个 op=5 明文包。
        let mut packet = Vec::new();
        for body in [br#"{"cmd":"DANMU_MSG","info":[[0,1,25,0,0],"a",[1,"u",0],[],[],[],[],0]}"#.as_slice()] {
            let len = (16 + body.len()) as u32;
            packet.extend_from_slice(&len.to_be_bytes());
            packet.extend_from_slice(&16u16.to_be_bytes());
            packet.extend_from_slice(&1u16.to_be_bytes());
            packet.extend_from_slice(&5u32.to_be_bytes());
            packet.extend_from_slice(&1u32.to_be_bytes());
            packet.extend_from_slice(body);
        }
        let (tx, rx) = mpsc::channel();
        handle_packets(&packet, &tx, 0);
        assert!(matches!(rx.recv().unwrap(), LiveChatEvent::Message(_)));
        // 长度越界的包直接丢弃，不 panic。
        let (tx, rx) = mpsc::channel();
        handle_packets(
            &[0, 0, 0x03, 0xE7, 0, 16, 0, 1, 0, 0, 0, 5, 0, 0, 0, 1],
            &tx,
            0,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn decimal_colors_are_rgb() {
        assert_eq!(decimal_color(16777215), (255, 255, 255));
        assert_eq!(decimal_color(0xFF0000), (255, 0, 0));
    }

    #[test]
    fn danmu_token_is_cached_and_352_is_negatively_cached() {
        store_danmu(
            42,
            CachedDanmu {
                host: "h".into(),
                port: 443,
                token: "t".into(),
                at: Instant::now(),
            },
        );
        let cached = cached_danmu(42).expect("应命中缓存");
        assert_eq!(cached.host, "h");
        assert_eq!(cached.token, "t");

        // 负缓存只影响命中的房间，且会被 blocked_for 读到。
        block_room(43);
        assert!(blocked_for(43).is_some());
        assert!(blocked_for(42).is_none());
    }
}
