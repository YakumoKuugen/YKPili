//! 设备指纹 cookie（`buvid3` / `buvid4`）。
//!
//! B 站的风控把这两个 cookie 当作「这是同一台浏览器」的依据。第三方客户端
//! 如果只带 `SESSDATA` / `bili_jct`，写接口（尤其是投币这种要花硬币的）
//! 很容易被打成风控：响应体里 `data.ga_data.decisions = ["verify_captcha_level3"]`
//! 且 `decision_ctx.buvid` 为空，界面只看到 `-401 非法访问`。
//!
//! 取法很轻：`GET /x/frontend/finger/spi` 返回 `data.b_3` / `data.b_4`，
//! 就是这两个 cookie 的值。首次取到后落盘，之后所有请求都带上，
//! 让风控看到稳定的设备标识。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::bilibili::{self, HttpClient};

/// 落盘文件名（与 `session.json` 同目录）。
const DEVICE_FILE: &str = "device.json";

/// 一份设备指纹。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceCookies {
    pub buvid3: String,
    pub buvid4: String,
}

impl DeviceCookies {
    /// 两个值都拿到了才算可用。
    pub fn is_complete(&self) -> bool {
        !self.buvid3.trim().is_empty() && !self.buvid4.trim().is_empty()
    }

    /// 把 `buvid3` / `buvid4` 拼到已有 Cookie 头后面。
    ///
    /// 原 Cookie 为空时只返回设备 cookie；设备指纹为空时原样返回。
    pub fn merge_into(&self, cookie: &str) -> String {
        if !self.is_complete() {
            return cookie.to_string();
        }
        let device = format!("buvid3={}; buvid4={}", self.buvid3, self.buvid4);
        if cookie.trim().is_empty() {
            device
        } else {
            format!("{}; {}", cookie.trim_end_matches(';'), device)
        }
    }
}

/// `device.json` 的路径：默认与 `session.json` 同目录。
///
/// `YK_DEVICE_PATH` 可覆盖（测试 / 便携启动用），避免测试写到真实用户目录。
pub fn path() -> PathBuf {
    if let Ok(path) = std::env::var("YK_DEVICE_PATH") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    crate::platform::config_file(DEVICE_FILE)
}

/// 读取落盘的设备指纹；缺失或损坏返回 `None`。
pub fn load() -> Option<DeviceCookies> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str::<DeviceCookies>(&text).ok())
        .filter(DeviceCookies::is_complete)
}

/// 写入设备指纹；失败不致命（下次再取一次）。
pub fn save(device: &DeviceCookies) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(device)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;
    std::fs::write(path, json)
}

/// 从 `x/frontend/finger/spi` 取一份新的设备指纹。
pub fn fetch<C>(client: &C, cookie: &str) -> Result<DeviceCookies, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&bilibili::finger_spi(cookie))?;
    parse(&body)
}

/// 解析 `finger/spi` 的响应。
pub fn parse(body: &str) -> Result<DeviceCookies, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| format!("设备指纹解析失败：{error}"))?;
    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(format!("设备指纹接口返回 code={code} {message}"));
    }
    let data = value.get("data").cloned().unwrap_or_default();
    let device = DeviceCookies {
        buvid3: data
            .get("b_3")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        buvid4: data
            .get("b_4")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    };
    if !device.is_complete() {
        return Err("设备指纹接口没返回 b_3 / b_4".into());
    }
    Ok(device)
}

/// 本地有就用本地，没有就取一次并落盘。
///
/// 取失败只返回 `None`：写操作照常发出（至少不比现在更差），
/// 下次再尝试，不会因为指纹拿不到把功能卡死。
pub fn ensure<C>(client: &C, cookie: &str) -> Option<DeviceCookies>
where
    C: HttpClient<Error = String>,
{
    if let Some(device) = load() {
        return Some(device);
    }
    let device = fetch(client, cookie).ok()?;
    if let Err(error) = save(&device) {
        eprintln!("[device] 设备指纹落盘失败：{error}");
    }
    Some(device)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::bilibili::Request;

    struct FakeClient(&'static str);

    impl HttpClient for FakeClient {
        type Error = String;
        fn get(&self, req: &Request) -> Result<String, String> {
            assert_eq!(req.name, "finger_spi");
            Ok(self.0.to_string())
        }
    }

    #[test]
    fn parses_spi_response() {
        let device = parse(
            r#"{"code":0,"data":{"b_3":"B3-abc","b_4":"B4-def"}}"#,
        )
        .unwrap();
        assert_eq!(device.buvid3, "B3-abc");
        assert_eq!(device.buvid4, "B4-def");
        assert!(device.is_complete());
    }

    #[test]
    fn rejects_incomplete_fingerprint() {
        assert!(parse(r#"{"code":0,"data":{"b_3":"only"}}"#).is_err());
        assert!(parse(r#"{"code":-101,"message":"账号未登录"}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn merge_keeps_session_cookie_and_appends_device() {
        let device = DeviceCookies {
            buvid3: "B3".into(),
            buvid4: "B4".into(),
        };
        assert_eq!(
            device.merge_into("SESSDATA=x; bili_jct=y"),
            "SESSDATA=x; bili_jct=y; buvid3=B3; buvid4=B4"
        );
        // 匿名请求也要带设备指纹：风控对读接口同样看这个。
        assert_eq!(device.merge_into(""), "buvid3=B3; buvid4=B4");
        // 不完整时原样返回，不拼半截。
        let empty = DeviceCookies::default();
        assert_eq!(empty.merge_into("SESSDATA=x"), "SESSDATA=x");
    }

    #[test]
    fn fetch_reads_spi() {
        let client = FakeClient(r#"{"code":0,"data":{"b_3":"B3","b_4":"B4"}}"#);
        let device = fetch(&client, "").unwrap();
        assert_eq!(device.buvid3, "B3");
    }
}
