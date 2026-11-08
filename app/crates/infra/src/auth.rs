//! 登录与鉴权（扫码 / 账号密码 / 手机验证码）。
//!
//! 所有 Cookie 都从响应头 `Set-Cookie`（或扫码成功 `data.url` 的 query）里提取，
//! 再调用 `nav` 补齐昵称/头像。UI 只负责状态与输入，不直接碰 HTTP。

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use num_bigint::BigUint;
use rand::RngCore;
use serde::Deserialize;

use crate::api::bilibili::{API_BASE, PASSPORT_BASE};
use crate::api::limits;
use crate::api::models::{LoginPollResponse, QrCodeData, UserInfoResponse};
use crate::models::UserSession;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建登录客户端失败：{e}"))
}

fn request(
    client: &reqwest::blocking::Client,
    method: reqwest::Method,
    url: &str,
    form: Option<&[(&str, String)]>,
    cookie: &str,
) -> Result<(String, Vec<String>), String> {
    let write = method == reqwest::Method::POST;
    let mut rb = client
        .request(method, url)
        .header("User-Agent", UA)
        .header("Referer", "https://www.bilibili.com/")
        .header("Accept", "application/json, text/plain, */*");
    if !cookie.is_empty() {
        rb = rb.header("Cookie", cookie);
    }
    if let Some(form) = form {
        rb = rb.form(form);
    }
    // 登录接口也走统一节流：扫码轮询、短信、密码登录各算各的间隔。
    limits::acquire_for(url, write);
    let resp = rb.send().map_err(|e| format!("请求登录接口失败：{e}"))?;
    let status = resp.status();
    let cookies = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let body = resp.text().map_err(|e| format!("读取登录响应失败：{e}"))?;
    if !status.is_success() {
        return Err(format!("登录接口返回 HTTP {status}"));
    }
    Ok((body, cookies))
}

/// 扫码登录二维码。
#[derive(Debug, Clone)]
pub struct QrLogin {
    pub url: String,
    pub key: String,
}

/// 扫码轮询结果。
#[derive(Debug, Clone)]
pub enum QrPoll {
    Waiting,
    Scanned,
    Expired,
    Success(Box<UserSession>),
}

#[derive(Debug, Deserialize)]
struct QrGenerateResponse {
    code: i64,
    data: Option<QrCodeData>,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct KeyData {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    key: String,
}

#[derive(Debug, Deserialize)]
struct KeyResponse {
    code: i64,
    data: Option<KeyData>,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize, Default)]
struct LoginResponse {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<LoginData>,
}

#[derive(Debug, Deserialize, Default)]
struct LoginData {
    #[serde(default)]
    cookie_info: Option<CookieInfo>,
}

#[derive(Debug, Deserialize, Default)]
struct CookieInfo {
    #[serde(default)]
    cookies: Vec<CookieItem>,
}

#[derive(Debug, Deserialize, Default)]
struct CookieItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
}

/// 生成扫码登录二维码。
pub fn generate_qr() -> Result<QrLogin, String> {
    let client = client()?;
    let (body, _) = request(
        &client,
        reqwest::Method::GET,
        &format!("{PASSPORT_BASE}/x/passport-login/web/qrcode/generate"),
        None,
        "",
    )?;
    let parsed: QrGenerateResponse =
        serde_json::from_str(&body).map_err(|e| format!("二维码响应解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!(
            "生成二维码失败：{}（{}）",
            parsed.message, parsed.code
        ));
    }
    let data = parsed
        .data
        .ok_or_else(|| "二维码响应缺少 data".to_string())?;
    if data.url.is_empty() || data.qrcode_key.is_empty() {
        return Err("二维码响应缺少 url / qrcode_key".into());
    }
    Ok(QrLogin {
        url: data.url,
        key: data.qrcode_key,
    })
}

/// 轮询扫码状态；成功时直接返回带 Cookie 的会话。
pub fn poll_qr(key: &str) -> Result<QrPoll, String> {
    let client = client()?;
    let (body, cookies) = request(
        &client,
        reqwest::Method::GET,
        &format!("{PASSPORT_BASE}/x/passport-login/web/qrcode/poll?qrcode_key={key}"),
        None,
        "",
    )?;
    let parsed: LoginPollResponse =
        serde_json::from_str(&body).map_err(|e| format!("扫码轮询解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!(
            "扫码轮询失败：{}（{}）",
            parsed.message, parsed.code
        ));
    }
    let data = parsed.data.unwrap_or_default();
    match data.code {
        0 => {
            let cookie = cookies_to_header(&cookies)
                .or_else(|| cookie_from_url(&data.url))
                .ok_or_else(|| "扫码成功但响应里没有 Cookie".to_string())?;
            Ok(QrPoll::Success(Box::new(finish_session(&cookie)?)))
        }
        86_038 => Ok(QrPoll::Expired),
        86_090 => Ok(QrPoll::Scanned),
        86_101 => Ok(QrPoll::Waiting),
        other => Err(format!("未知扫码状态码：{other}")),
    }
}

/// 二维码图片：交给外部 QR 服务渲染，避免在客户端引入一套二维码编码器。
/// 纯白底由 UI 保证；二维码内容本身只包含 B 站登录 URL。
pub fn qr_image_url(url: &str) -> String {
    let encoded: String = url
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("https://api.qrserver.com/v1/create-qr-code/?size=240x240&margin=8&data={encoded}")
}

/// 账号密码登录。B 站 web 端要求 RSA 加密，并在风控时要求极验验证码；
/// 这里先完成加密与标准表单，验证码字段留空由服务端决定是否放行。
pub fn password_login(username: &str, password: &str) -> Result<UserSession, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("账号和密码不能为空".into());
    }
    let client = client()?;
    let (key_body, _) = request(
        &client,
        reqwest::Method::GET,
        &format!("{PASSPORT_BASE}/x/passport-login/web/key"),
        None,
        "",
    )?;
    let key: KeyResponse =
        serde_json::from_str(&key_body).map_err(|e| format!("登录公钥解析失败：{e}"))?;
    if key.code != 0 {
        return Err(format!("获取登录公钥失败：{}（{}）", key.message, key.code));
    }
    let data = key.data.ok_or_else(|| "登录公钥缺少 data".to_string())?;
    let encrypted = rsa_encrypt(&data.key, &format!("{}{}", data.hash, password))?;
    let (body, cookies) = request(
        &client,
        reqwest::Method::POST,
        &format!("{PASSPORT_BASE}/x/passport-login/web/login"),
        Some(&[
            ("username", username.trim().to_string()),
            ("password", encrypted),
            ("keep", "0".into()),
            ("source", "main_web".into()),
            ("token", String::new()),
            ("challenge", String::new()),
            ("validate", String::new()),
            ("seccode", String::new()),
        ]),
        "",
    )?;
    parse_login_result(&body, &cookies)
}

/// 发送手机验证码。
pub fn send_sms(phone: &str) -> Result<(), String> {
    let phone = phone.trim();
    if phone.is_empty() {
        return Err("手机号不能为空".into());
    }
    let client = client()?;
    let (body, _) = request(
        &client,
        reqwest::Method::POST,
        &format!("{PASSPORT_BASE}/x/passport-login/web/sms/send"),
        Some(&[
            ("cid", "86".into()),
            ("tel", phone.to_string()),
            ("source", "main_web".into()),
            ("token", String::new()),
            ("challenge", String::new()),
            ("validate", String::new()),
            ("seccode", String::new()),
        ]),
        "",
    )?;
    let parsed: LoginResponse =
        serde_json::from_str(&body).map_err(|e| format!("验证码响应解析失败：{e}"))?;
    if parsed.code == 0 {
        Ok(())
    } else {
        Err(format!(
            "验证码发送失败：{}（{}）",
            parsed.message, parsed.code
        ))
    }
}

/// 手机验证码登录。
pub fn sms_login(phone: &str, code: &str) -> Result<UserSession, String> {
    let phone = phone.trim();
    let code = code.trim();
    if phone.is_empty() || code.is_empty() {
        return Err("手机号和验证码不能为空".into());
    }
    let client = client()?;
    let (body, cookies) = request(
        &client,
        reqwest::Method::POST,
        &format!("{PASSPORT_BASE}/x/passport-login/web/login"),
        Some(&[
            ("tel", phone.to_string()),
            ("smsCode", code.to_string()),
            ("cid", "86".into()),
            ("source", "main_web".into()),
            ("keep", "0".into()),
            ("token", String::new()),
            ("challenge", String::new()),
            ("validate", String::new()),
            ("seccode", String::new()),
        ]),
        "",
    )?;
    parse_login_result(&body, &cookies)
}

fn parse_login_result(body: &str, cookies: &[String]) -> Result<UserSession, String> {
    let parsed: LoginResponse =
        serde_json::from_str(body).map_err(|e| format!("登录响应解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("登录失败：{}（{}）", parsed.message, parsed.code));
    }
    let cookie = cookies_to_header(cookies).or_else(|| {
        parsed
            .data
            .as_ref()
            .and_then(|d| d.cookie_info.as_ref())
            .map(cookie_info_to_header)
            .filter(|h| !h.is_empty())
    });
    let cookie = cookie.ok_or_else(|| "登录成功但响应里没有 Cookie".to_string())?;
    finish_session(&cookie)
}

fn cookies_to_header(cookies: &[String]) -> Option<String> {
    let mut sess = None;
    let mut jct = None;
    let mut uid = None;
    for c in cookies {
        let first = c.split(';').next().unwrap_or(c);
        let Some((name, value)) = first.split_once('=') else {
            continue;
        };
        match name.trim() {
            "SESSDATA" => sess = Some(value.to_string()),
            "bili_jct" => jct = Some(value.to_string()),
            "DedeUserID" => uid = Some(value.to_string()),
            _ => {}
        }
    }
    let sess = sess?;
    let mut parts = vec![format!("SESSDATA={sess}")];
    if let Some(jct) = jct {
        if !jct.is_empty() {
            parts.push(format!("bili_jct={jct}"));
        }
    }
    if let Some(uid) = uid {
        if !uid.is_empty() {
            parts.push(format!("DedeUserID={uid}"));
        }
    }
    Some(parts.join("; "))
}

fn cookie_info_to_header(info: &CookieInfo) -> String {
    let mut sess = None;
    let mut jct = None;
    let mut uid = None;
    for c in &info.cookies {
        match c.name.as_str() {
            "SESSDATA" => sess = Some(c.value.clone()),
            "bili_jct" => jct = Some(c.value.clone()),
            "DedeUserID" => uid = Some(c.value.clone()),
            _ => {}
        }
    }
    let Some(sess) = sess else {
        return String::new();
    };
    let mut parts = vec![format!("SESSDATA={sess}")];
    if let Some(jct) = jct.filter(|v| !v.is_empty()) {
        parts.push(format!("bili_jct={jct}"));
    }
    if let Some(uid) = uid.filter(|v| !v.is_empty()) {
        parts.push(format!("DedeUserID={uid}"));
    }
    parts.join("; ")
}

fn cookie_from_url(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let mut sess = None;
    let mut jct = None;
    let mut uid = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        // 保留 URL 里的百分号编码：SESSDATA 本身常含 %2C，Cookie 头应原样带回去。
        let value = v.to_string();
        match k {
            "SESSDATA" => sess = Some(value),
            "bili_jct" => jct = Some(value),
            "DedeUserID" => uid = Some(value),
            _ => {}
        }
    }
    let sess = sess?;
    let mut parts = vec![format!("SESSDATA={sess}")];
    if let Some(jct) = jct {
        if !jct.is_empty() {
            parts.push(format!("bili_jct={jct}"));
        }
    }
    if let Some(uid) = uid {
        if !uid.is_empty() {
            parts.push(format!("DedeUserID={uid}"));
        }
    }
    Some(parts.join("; "))
}

/// 用 Cookie 调 nav，补齐昵称/头像/UID；同时把 Cookie 存进会话。
pub fn finish_session(cookie: &str) -> Result<UserSession, String> {
    let client = client()?;
    let (body, _) = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/x/web-interface/nav"),
        None,
        cookie,
    )?;
    let parsed: UserInfoResponse =
        serde_json::from_str(&body).map_err(|e| format!("nav 解析失败：{e}"))?;
    let data = parsed.data.unwrap_or_default();
    // 登录成功立刻刷新会员状态：画质菜单下一次打开就按大会员档位展示。
    crate::api::client::note_membership(data.vip.as_ref().is_some_and(|vip| vip.is_active()));
    let session = UserSession {
        is_logged_in: true,
        sess_data: cookie_value(cookie, "SESSDATA").unwrap_or_default(),
        bili_jct: cookie_value(cookie, "bili_jct").unwrap_or_default(),
        dede_user_id: cookie_value(cookie, "DedeUserID")
            .and_then(|v| v.parse().ok())
            .unwrap_or(data.mid),
        nickname: data.uname,
        face_url: data.face,
        ..Default::default()
    };
    if session.sess_data.is_empty() {
        return Err("Cookie 里没有 SESSDATA".into());
    }
    Ok(session)
}

fn cookie_value(cookie: &str, name: &str) -> Option<String> {
    cookie.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn rsa_encrypt(public_key: &str, message: &str) -> Result<String, String> {
    let (n, e) = parse_rsa_public_key(public_key)?;
    let n = BigUint::from_bytes_be(&n);
    let e = BigUint::from_bytes_be(&e);
    let k = (n.bits() as usize + 7) / 8;
    if message.len() + 11 > k {
        return Err("密码过长，无法用 RSA 加密".into());
    }
    let ps_len = k - message.len() - 3;
    let mut ps = vec![0u8; ps_len];
    rand::thread_rng().fill_bytes(&mut ps);
    for b in &mut ps {
        if *b == 0 {
            *b = 1;
        }
    }
    let mut em = Vec::with_capacity(k);
    em.push(0);
    em.push(2);
    em.extend_from_slice(&ps);
    em.push(0);
    em.extend_from_slice(message.as_bytes());
    let m = BigUint::from_bytes_be(&em);
    let c = m.modpow(&e, &n).to_bytes_be();
    let mut out = vec![0u8; k.saturating_sub(c.len())];
    out.extend_from_slice(&c);
    Ok(B64.encode(out))
}

fn parse_rsa_public_key(key: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cleaned = key
        .replace("-----BEGIN PUBLIC KEY-----", "")
        .replace("-----END PUBLIC KEY-----", "")
        .replace("-----BEGIN RSA PUBLIC KEY-----", "")
        .replace("-----END RSA PUBLIC KEY-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let der = B64
        .decode(cleaned)
        .map_err(|e| format!("登录公钥 base64 解码失败：{e}"))?;
    parse_der_rsa(&der).ok_or_else(|| "登录公钥不是可识别的 RSA DER".to_string())
}

fn parse_der_rsa(der: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    // SubjectPublicKeyInfo: SEQ { SEQ { OID, NULL }, BIT STRING { SEQ { INTEGER n, INTEGER e } } }
    let (tag, body, _) = der_tlv(der, 0)?;
    if tag != 0x30 {
        return None;
    }
    let (_, _alg, next) = der_tlv(body, 0)?;
    let (tag, bits, _) = der_tlv(body, next)?;
    if tag != 0x03 || bits.len() < 2 {
        return None;
    }
    let (tag, rsa, _) = der_tlv(&bits[1..], 0)?;
    if tag != 0x30 {
        return None;
    }
    let (_, n, next) = der_tlv(rsa, 0)?;
    let (_, e, _) = der_tlv(rsa, next)?;
    Some((strip_leading_zero(n), strip_leading_zero(e)))
}

fn strip_leading_zero(bytes: &[u8]) -> Vec<u8> {
    bytes.strip_prefix(&[0]).unwrap_or(bytes).to_vec()
}

/// 极简 DER TLV：返回 (tag, value, next_offset)。
fn der_tlv(data: &[u8], mut offset: usize) -> Option<(u8, &[u8], usize)> {
    let tag = *data.get(offset)?;
    offset += 1;
    let first = *data.get(offset)?;
    offset += 1;
    let len = if first & 0x80 == 0 {
        first as usize
    } else {
        let count = (first & 0x7f) as usize;
        let mut len = 0usize;
        for _ in 0..count {
            len = (len << 8) | *data.get(offset)? as usize;
            offset += 1;
        }
        len
    };
    let end = offset.checked_add(len)?;
    Some((tag, data.get(offset..end)?, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_header_picks_only_login_cookies() {
        let h = cookies_to_header(&[
            "SESSDATA=abc%2C123; Path=/; HttpOnly".into(),
            "bili_jct=jct; Path=/".into(),
            "DedeUserID=42; Path=/".into(),
            "other=x".into(),
        ])
        .unwrap();
        assert_eq!(h, "SESSDATA=abc%2C123; bili_jct=jct; DedeUserID=42");
    }

    #[test]
    fn qr_url_encodes_query() {
        let u = qr_image_url("https://passport.bilibili.com/x?a=1&b=中");
        assert!(u.contains("%3A%2F%2F"));
        assert!(u.contains("%26"));
        assert!(u.contains("%E4%B8%AD"));
    }

    #[test]
    fn cookie_from_login_url() {
        let h = cookie_from_url(
            "https://www.bilibili.com/?SESSDATA=abc%2C1&bili_jct=j&gourl=x&DedeUserID=7",
        )
        .unwrap();
        assert_eq!(h, "SESSDATA=abc%2C1; bili_jct=j; DedeUserID=7");
    }
}

/// 账号面板所需的扩展资料。
#[derive(Debug, Clone, Default)]
pub struct AccountInfo {
    pub mid: i64,
    pub name: String,
    pub face: String,
    pub coins: f64,
    pub bcoin: f64,
    pub level: u8,
    pub following: u64,
    pub followers: u64,
    pub dynamics: Option<u64>,
    pub last_login_time: String,
    pub last_login_geo: String,
}

#[derive(Deserialize, Default)]
struct RelationStatResponse {
    #[serde(default)]
    data: Option<RelationStat>,
}
#[derive(Deserialize, Default)]
struct RelationStat {
    #[serde(default)]
    following: u64,
    #[serde(default)]
    follower: u64,
}

#[derive(Deserialize, Default)]
struct LoginLogResponse {
    #[serde(default)]
    data: Option<LoginLogData>,
}
#[derive(Deserialize, Default)]
struct LoginLogData {
    #[serde(default)]
    list: Vec<LoginLogItem>,
}
#[derive(Deserialize, Default)]
struct LoginLogItem {
    #[serde(default)]
    time_at: String,
    #[serde(default)]
    geo: String,
}

/// 拉取账号面板资料：nav + relation/stat + 最近登录记录。
pub fn fetch_account(cookie: &str) -> Result<AccountInfo, String> {
    let client = client()?;
    let (body, _) = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/x/web-interface/nav"),
        None,
        cookie,
    )?;
    let nav: UserInfoResponse =
        serde_json::from_str(&body).map_err(|e| format!("nav 解析失败：{e}"))?;
    let data = nav.data.unwrap_or_default();
    if !data.is_login {
        return Err("登录态已失效".into());
    }
    crate::api::client::note_membership(data.vip.as_ref().is_some_and(|vip| vip.is_active()));
    let mut info = AccountInfo {
        mid: data.mid,
        name: data.uname,
        face: data.face,
        coins: data.money,
        bcoin: data.wallet.as_ref().map(|w| w.bcoin_balance).unwrap_or(0.0),
        level: data
            .level_info
            .as_ref()
            .map(|l| l.current_level)
            .unwrap_or(0),
        ..Default::default()
    };

    let rel_url = format!("{API_BASE}/x/relation/stat?vmid={}", info.mid);
    if let Ok((body, _)) = request(&client, reqwest::Method::GET, &rel_url, None, cookie) {
        if let Ok(rel) = serde_json::from_str::<RelationStatResponse>(&body) {
            if let Some(d) = rel.data {
                info.following = d.following;
                info.followers = d.follower;
            }
        }
    }

    let log_url = format!("{API_BASE}/x/member/web/login/log");
    if let Ok((body, _)) = request(&client, reqwest::Method::GET, &log_url, None, cookie) {
        if let Ok(log) = serde_json::from_str::<LoginLogResponse>(&body) {
            if let Some(first) = log.data.and_then(|d| d.list.into_iter().next()) {
                info.last_login_time = first.time_at;
                info.last_login_geo = first.geo;
            }
        }
    }
    Ok(info)
}
