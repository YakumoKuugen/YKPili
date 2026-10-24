//! HTTP 客户端与完整取数流程。
//!
//! 这一层把「构造请求」（`api::bilibili`）与「发请求」接起来：
//! `ReqwestClient` 实现 `HttpClient`，`fetch_recommend` 负责
//! 「先取 WBI keys → 签名 → 拉数据 → 反序列化」这条完整链路。
//!
//! **两个必须带的头**：`User-Agent` 与 `Referer`。
//! B 站对默认 UA（`reqwest/x.y`）会直接 403，缺 Referer 有概率被风控拦。

use crate::api::bilibili::{self, HttpClient, Request};
use crate::api::models::{RecommendResponse, RecommendVideoItem, UserInfoResponse};
use crate::api::wbi;

/// 伪装成桌面浏览器 —— 与旧实现的 UA 策略一致。
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 默认请求超时（秒）。与旧 `ApiManager` 一致：15s。
pub const REQUEST_TIMEOUT_SECS: u64 = 15;

/// 基于 `reqwest::blocking` 的客户端。
pub struct ReqwestClient {
    inner: reqwest::blocking::Client,
}

impl ReqwestClient {
    pub fn new() -> Result<Self, String> {
        let inner = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
        Ok(Self { inner })
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new().expect("HTTP 客户端初始化失败")
    }
}

impl HttpClient for ReqwestClient {
    type Error = String;

    fn get(&self, req: &Request) -> Result<String, Self::Error> {
        let mut rb = self
            .inner
            .get(&req.url)
            .header("User-Agent", UA)
            .header("Referer", "https://www.bilibili.com/")
            .header("Accept", "application/json, text/plain, */*");
        if !req.cookie.is_empty() {
            rb = rb.header("Cookie", &req.cookie);
        }
        let resp = rb
            .send()
            .map_err(|e| format!("{} 请求失败：{e}", req.name))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| format!("{} 读取响应失败：{e}", req.name))?;
        if !status.is_success() {
            return Err(format!("{} 返回 HTTP {status}", req.name));
        }
        Ok(body)
    }
}

/// 当前秒级时间戳。
pub fn now_wts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 取 WBI keys（`img_key` / `sub_key`）。
///
/// **不需要登录**：`nav` 在未登录时也会返回 `wbi_img`，只是 `isLogin=false`。
/// 这一点很关键 —— 只有这样首页才能在没登录的情况下加载推荐。
pub fn fetch_wbi_keys<C>(client: &C, cookie: &str) -> Result<(String, String), String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&bilibili::user_info(cookie))?;
    let parsed: UserInfoResponse = serde_json::from_str(&body)
        .map_err(|e| format!("nav 响应解析失败：{e}"))?;
    let data = parsed.data.ok_or_else(|| "nav 响应缺少 data".to_string())?;
    let wbi = data.wbi_img.ok_or_else(|| "nav 响应缺少 wbi_img".to_string())?;
    let img = bilibili::extract_wbi_key(&wbi.img_url);
    let sub = bilibili::extract_wbi_key(&wbi.sub_url);
    if img.is_empty() || sub.is_empty() {
        return Err("wbi_img 里的 img_key/sub_url 为空".into());
    }
    Ok((img, sub))
}

/// 拉一页推荐流。返回条目列表（未做去重/上限，交给 `Feed`）。
pub fn fetch_recommend<C>(
    client: &C,
    cookie: &str,
    ps: u32,
    wts: u64,
) -> Result<Vec<RecommendVideoItem>, String>
where
    C: HttpClient<Error = String>,
{
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    let req = bilibili::recommend(&img, &sub, cookie, wts, ps);
    let body = client.get(&req)?;
    let parsed: RecommendResponse =
        serde_json::from_str(&body).map_err(|e| format!("推荐响应解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("推荐接口返回 code={}", parsed.code));
    }
    Ok(parsed.data.map(|d| d.item).unwrap_or_default())
}

/// 计算签名（给需要手工验证的场合用）。
pub fn sign_for_debug(
    base: &str,
    params: &[(String, String)],
    img: &str,
    sub: &str,
    wts: u64,
) -> String {
    wbi::signed_url(base, params, img, sub, wts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不联网的假客户端：按请求名返回预置 JSON。
    struct FakeClient {
        nav: String,
        recommend: String,
    }

    impl HttpClient for FakeClient {
        type Error = String;
        fn get(&self, req: &Request) -> Result<String, String> {
            match req.name {
                "user_info" => Ok(self.nav.clone()),
                "recommend" => Ok(self.recommend.clone()),
                other => Err(format!("未预期的请求：{other}")),
            }
        }
    }

    fn fake() -> FakeClient {
        FakeClient {
            nav: r#"{"code":0,"data":{"isLogin":false,"mid":0,"uname":"","face":"",
                "wbi_img":{"img_url":"https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png",
                           "sub_url":"https://i0.hdslb.com/bfs/wbi/4932caff0ff746eab6f01bf08b70ac45.png"}}}"#
                .to_string(),
            recommend: r#"{"code":0,"data":{"item":[
                {"id":1,"bvid":"BV1","title":"A &amp; B","pic":"p","duration":60,
                 "owner":{"name":"UP","face":"f"},"stat":{"view":1000,"danmaku":10}},
                {"id":2,"bvid":"BV2","title":"C","pic":"p","duration":120,
                 "owner":{"name":"UP2","face":"f"},"stat":{"view":2000,"danmaku":20}}
            ]}}"#
                .to_string(),
        }
    }

    #[test]
    fn wbi_keys_extracted_from_nav() {
        let (img, sub) = fetch_wbi_keys(&fake(), "").unwrap();
        assert_eq!(img, "7cd084941338484aae1ad9425b84077c");
        assert_eq!(sub, "4932caff0ff746eab6f01bf08b70ac45");
    }

    #[test]
    fn recommend_pipeline_parses_and_decodes() {
        let items = fetch_recommend(&fake(), "", 30, 1_702_204_169).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].bvid, "BV1");
        assert_eq!(items[0].owner.as_ref().unwrap().name, "UP");
        assert_eq!(items[0].stat.as_ref().unwrap().view, 1000);
        assert_eq!(items[0].duration, 60);
        // HTML 实体在 CoverItem::from_recommend 里解码，这里应保持原文
        assert_eq!(items[0].title, "A &amp; B");
    }

    #[test]
    fn non_zero_code_is_an_error() {
        struct ErrClient;
        impl HttpClient for ErrClient {
            type Error = String;
            fn get(&self, req: &Request) -> Result<String, String> {
                if req.name == "user_info" {
                    return Ok(
                        r#"{"code":0,"data":{"wbi_img":{"img_url":"a.png","sub_url":"b.png"}}}"#
                            .into(),
                    );
                }
                Ok(r#"{"code":-352,"data":null}"#.into())
            }
        }
        let err = fetch_recommend(&ErrClient, "", 30, 1).unwrap_err();
        assert!(err.contains("-352"), "错误应包含业务码：{err}");
    }

    #[test]
    fn missing_wbi_img_is_an_error() {
        struct NoWbi;
        impl HttpClient for NoWbi {
            type Error = String;
            fn get(&self, _: &Request) -> Result<String, String> {
                Ok(r#"{"code":0,"data":{"isLogin":false}}"#.into())
            }
        }
        assert!(fetch_wbi_keys(&NoWbi, "").is_err());
    }

    #[test]
    fn timeout_matches_legacy() {
        assert_eq!(REQUEST_TIMEOUT_SECS, 15);
    }
}
