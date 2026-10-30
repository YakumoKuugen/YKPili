//! 视频互动：点赞 / 投币 / 收藏 / 转发到动态。
//!
//! 这一层把「互动」拆成两半：
//! - **读**（是否已点赞 / 已投币 / 已收藏、收藏夹列表）走 GET，
//!   用 [`crate::api::bilibili::HttpClient`] 抽象，测试里可以塞假客户端；
//! - **写**（点赞 / 投币 / 收藏 / 转发）走 POST + `bili_jct`，
//!   需要真实 HTTP 客户端，所以单独放在 [`api::client::ReqwestClient`] 上。
//!
//! 写操作都返回 `Result`：B 站的写接口成功时 `code == 0`，
//! 失败时 `code != 0` 且带一句人话（"硬币不足" / "请先登录"），
//! 这里把那句话原样透给界面。

use crate::api::bilibili::{HttpClient, Request};
use crate::api::client::{check_write_code, ReqwestClient};
use serde::Deserialize;

// ================= 数据模型 =================

/// 一个收藏夹（自建收藏夹列表接口的条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavFolder {
    /// 收藏夹 id（收藏 / 取消收藏时用）。
    pub id: i64,
    /// 收藏夹名。
    pub title: String,
    /// 夹内条目数。
    pub media_count: u64,
    /// 夹属性：1 = 默认收藏夹，0 = 普通夹。
    pub attr: i64,
    /// 该视频在这个夹里的状态：1 = 已收藏，0 = 未收藏。
    pub fav_state: i64,
}

impl FavFolder {
    /// 这个视频是否已经在这个收藏夹里。
    pub fn contains_video(&self) -> bool {
        self.fav_state == 1
    }
}

impl FavFolder {
    /// 是否是默认收藏夹。
    pub fn is_default(&self) -> bool {
        self.attr == 1
    }
}

/// 视频的互动状态（是否已经做过某个动作）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractionState {
    /// 已点赞。
    pub liked: bool,
    /// 已投币。
    pub coined: bool,
    /// 已收藏（默认收藏夹）。
    pub favoured: bool,
}

// ================= 读操作 =================

/// 构造「是否已点赞」的请求。
///
/// **av 号与 bvid 一起带上**：B 站这组接口两个 id 都认，但历史上只认其中一个
/// 的版本存在（缺哪个都可能回 -400）。两个都带，服务端爱用哪个用哪个。
fn has_like_request(aid: i64, bvid: &str, cookie: &str) -> Request {
    Request {
        url: format!(
            "{}/x/web-interface/archive/has/like?{}",
            crate::api::bilibili::API_BASE,
            video_id_query(aid, bvid)
        ),
        cookie: cookie.to_string(),
        name: "has_like",
    }
}

/// 构造「是否已投币」的请求。
fn coins_request(aid: i64, bvid: &str, cookie: &str) -> Request {
    Request {
        url: format!(
            "{}/x/web-interface/archive/coins?{}",
            crate::api::bilibili::API_BASE,
            video_id_query(aid, bvid)
        ),
        cookie: cookie.to_string(),
        name: "coins",
    }
}

/// 视频 id 的 query 片段：`aid=xxx&bvid=xxx`。
///
/// 缺的 id 就整段省掉，不发空值（`aid=0` 会被当成无效稿件）。
fn video_id_query(aid: i64, bvid: &str) -> String {
    let mut parts = Vec::with_capacity(2);
    if aid > 0 {
        parts.push(format!("aid={aid}"));
    }
    let bvid = bvid.trim();
    if !bvid.is_empty() {
        parts.push(format!("bvid={bvid}"));
    }
    parts.join("&")
}

/// 构造「是否已收藏」的请求。
fn favoured_request(aid: i64, cookie: &str) -> Request {
    Request {
        url: format!(
            "{}/x/v2/fav/video/favoured?aid={}",
            crate::api::bilibili::API_BASE,
            aid
        ),
        cookie: cookie.to_string(),
        name: "favoured",
    }
}

/// 构造「收藏夹列表」的请求。
///
/// `up_mid` 是**当前登录用户**的 mid —— 只能列自己的收藏夹。
fn fav_folders_request(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!(
            "{}/x/v3/fav/folder/created/list-all?up_mid={}",
            crate::api::bilibili::API_BASE,
            mid
        ),
        cookie: cookie.to_string(),
        name: "fav_folders",
    }
}

/// 是否已点赞。
pub fn fetch_liked<C>(client: &C, cookie: &str, aid: i64, bvid: &str) -> Result<bool, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&has_like_request(aid, bvid, cookie))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("点赞状态解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("读取点赞状态失败：code={code} {message}"));
    }
    Ok(parsed.get("data").and_then(|v| v.as_i64()).unwrap_or(0) == 1)
}

/// 是否已投币。
pub fn fetch_coined<C>(client: &C, cookie: &str, aid: i64, bvid: &str) -> Result<bool, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&coins_request(aid, bvid, cookie))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("投币状态解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("读取投币状态失败：code={code} {message}"));
    }
    let multiply = parsed
        .get("data")
        .and_then(|v| v.get("multiply"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    Ok(multiply > 0)
}

/// 是否已收藏（默认收藏夹）。
pub fn fetch_favoured<C>(client: &C, cookie: &str, aid: i64) -> Result<bool, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&favoured_request(aid, cookie))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("收藏状态解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("读取收藏状态失败：code={code} {message}"));
    }
    Ok(parsed
        .get("data")
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// 拉取当前账号的收藏夹列表。
pub fn fetch_fav_folders<C>(client: &C, cookie: &str, mid: i64) -> Result<Vec<FavFolder>, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&fav_folders_request(mid, cookie))?;
    let parsed: FavFoldersResponse =
        serde_json::from_str(&body).map_err(|e| format!("收藏夹列表解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!(
            "读取收藏夹失败：code={} {}",
            parsed.code, parsed.message
        ));
    }
    let list = parsed.data.map(|data| data.list).unwrap_or_default();
    Ok(list
        .into_iter()
        .map(|item| FavFolder {
            id: item.id,
            title: item.title,
            media_count: item.media_count,
            attr: item.attr,
            fav_state: item.fav_state,
        })
        .collect())
}

#[derive(Debug, Clone, Deserialize)]
struct FavFoldersResponse {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<FavFoldersData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FavFoldersData {
    #[serde(default)]
    list: Vec<FavFolderRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FavFolderRaw {
    id: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    media_count: u64,
    #[serde(default)]
    attr: i64,
    /// 这个视频在该收藏夹里的状态（1 = 已收藏）。
    #[serde(default)]
    fav_state: i64,
}

// ================= 写操作 =================

/// 点赞 / 取消点赞。
///
/// `like = true` 点赞，`false` 取消（B 站的 `like` 字段是 1 / 2）。
pub fn like_video(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    bvid: &str,
    like: bool,
) -> Result<(), String> {
    require_csrf(csrf, "点赞")?;
    let body = client.post_form(
        &format!(
            "{}/x/web-interface/archive/like",
            crate::api::bilibili::API_BASE
        ),
        &[
            ("bvid", bvid.to_string()),
            ("like", if like { "1" } else { "2" }.to_string()),
            ("csrf", csrf.to_string()),
        ],
        cookie,
    )?;
    check_write_code(&body, if like { "点赞" } else { "取消点赞" })
}

/// 投币。
///
/// **必须传 `aid`（av 号），不能只传 `bvid`** —— B 站的 `coin/add` 只认 av 号：
/// 只带 bvid 会拿到 `code=-400`（请求错误），投币永远失败。
/// 浏览器发的就是这个组合：`aid` / `multiply` / `select_like` / `csrf` / `csrf_token`。
///
/// `multiply` 是硬币数（1 / 2）；`also_like` 为真时同时点赞。
pub fn coin_video(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    aid: i64,
    multiply: u8,
    also_like: bool,
) -> Result<(), String> {
    require_csrf(csrf, "投币")?;
    if aid <= 0 {
        return Err("缺少视频 aid，无法投币".into());
    }
    let body = client.post_form(
        &format!(
            "{}/x/web-interface/coin/add",
            crate::api::bilibili::API_BASE
        ),
        &[
            ("aid", aid.to_string()),
            ("multiply", multiply.clamp(1, 2).to_string()),
            (
                "select_like",
                if also_like { "true" } else { "false" }.to_string(),
            ),
            ("csrf", csrf.to_string()),
            ("csrf_token", csrf.to_string()),
        ],
        cookie,
    )?;
    check_write_code(&body, "投币")
}

/// 收藏 / 取消收藏到指定收藏夹。
///
/// `rid` 是视频的 aid；`add_media_ids` / `del_media_ids` 传的**是收藏夹 id**
/// （不是视频 id —— 传错会返回 11010「您访问的内容不存在」）。
///
/// `add = true` 收藏，`false` 从该收藏夹移除。
pub fn fav_video(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    aid: i64,
    folder_id: i64,
    add: bool,
) -> Result<(), String> {
    require_csrf(csrf, "收藏")?;
    if aid <= 0 || folder_id <= 0 {
        return Err("缺少 aid / 收藏夹 id，无法收藏".into());
    }
    let key = if add {
        "add_media_ids"
    } else {
        "del_media_ids"
    };
    let body = client.post_form(
        &format!("{}/x/v3/fav/resource/deal", crate::api::bilibili::API_BASE),
        &[
            ("rid", aid.to_string()),
            ("type", "2".to_string()),
            (key, folder_id.to_string()),
            ("csrf", csrf.to_string()),
        ],
        cookie,
    )?;
    check_write_code(&body, if add { "收藏" } else { "取消收藏" })
}

/// 把视频分享到自己的动态（发一条带视频链接的动态）。
///
/// 走 web 端的发布接口 `x/dynamic/feed/create/dyn`：CSRF 放 query，
/// body 是 `{"dyn_req": {...}}` 的 JSON。
///
/// 正文里带视频链接时 B 站会自己把它解析成视频卡片；解析不了也至少是一条
/// 带链接的动态，点链接能跳到视频。
///
/// 返回新动态的 id。
pub fn share_to_dynamic(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    content: &str,
) -> Result<String, String> {
    require_csrf(csrf, "分享到动态")?;
    if content.trim().is_empty() {
        return Err("分享内容为空".into());
    }
    let payload = serde_json::json!({
        "dyn_req": {
            "scene": 1,
            "meta": {
                "app_meta": {
                    "from": "create.dynamic.web",
                    "mobi_app": "web",
                }
            },
            "contents": [
                {
                    "biz_id": "",
                    "type": 1,
                    "raw_text": content,
                }
            ],
        }
    });
    let url = format!(
        "https://api.bilibili.com/x/dynamic/feed/create/dyn?csrf={}",
        csrf
    );
    let body = client.post_json(&url, &payload, cookie)?;
    check_write_code(&body, "分享到动态")?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("分享到动态响应解析失败：{e}"))?;
    let dynamic_id = parsed
        .get("data")
        .and_then(|v| v.get("dyn_id_str"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            parsed
                .get("data")
                .and_then(|v| v.get("dyn_id"))
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
        })
        .ok_or_else(|| "分享到动态：响应里没有动态 id".to_string())?;
    Ok(dynamic_id)
}

/// 删除一条动态（转发失败时的回滚 / 测试清理）。
pub fn remove_dynamic(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    dynamic_id: &str,
) -> Result<(), String> {
    require_csrf(csrf, "删除动态")?;
    let body = client.post_form(
        "https://api.vc.bilibili.com/dynamic_svr/v1/dynamic_svr/rm_dynamic",
        &[
            ("dynamic_id", dynamic_id.to_string()),
            ("csrf", csrf.to_string()),
        ],
        cookie,
    )?;
    check_write_code(&body, "删除动态")
}

fn require_csrf(csrf: &str, action: &str) -> Result<(), String> {
    if csrf.trim().is_empty() {
        return Err(format!("登录会话缺少 bili_jct，无法{action}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::bilibili::Request;

    /// 只回固定 JSON 的假客户端。
    struct FakeClient {
        body: String,
        last: std::cell::RefCell<Option<Request>>,
    }

    impl FakeClient {
        fn new(body: &str) -> Self {
            Self {
                body: body.to_string(),
                last: std::cell::RefCell::new(None),
            }
        }
    }

    impl HttpClient for FakeClient {
        type Error = String;

        fn get(&self, req: &Request) -> Result<String, Self::Error> {
            *self.last.borrow_mut() = Some(req.clone());
            Ok(self.body.clone())
        }
    }

    #[test]
    fn has_like_url_carries_both_ids() {
        let client = FakeClient::new(r#"{"code":0,"data":1}"#);
        assert!(fetch_liked(&client, "SESSDATA=x", 123, "BV1abc").unwrap());
        let req = client.last.borrow().clone().unwrap();
        // av 号和 bvid 一起带上：这两个接口两个 id 都认，带全了不会挑错。
        assert!(req
            .url
            .ends_with("/x/web-interface/archive/has/like?aid=123&bvid=BV1abc"));
        assert_eq!(req.cookie, "SESSDATA=x");
    }

    #[test]
    fn video_id_query_skips_missing_ids() {
        // 缺哪个就省掉哪个：不发 aid=0（会被当成无效稿件）。
        assert_eq!(video_id_query(0, "BV1abc"), "bvid=BV1abc");
        assert_eq!(video_id_query(123, ""), "aid=123");
        assert_eq!(video_id_query(123, "BV1abc"), "aid=123&bvid=BV1abc");
    }

    #[test]
    fn has_like_reads_flag() {
        let client = FakeClient::new(r#"{"code":0,"data":0}"#);
        assert!(!fetch_liked(&client, "", 123, "BV1abc").unwrap());
    }

    #[test]
    fn has_like_reports_code_error() {
        let client = FakeClient::new(r#"{"code":-101,"message":"账号未登录"}"#);
        let error = fetch_liked(&client, "", 123, "BV1abc").unwrap_err();
        assert!(error.contains("-101"), "{error}");
    }

    #[test]
    fn coins_multiply_means_coined() {
        let client = FakeClient::new(r#"{"code":0,"data":{"multiply":2}}"#);
        assert!(fetch_coined(&client, "", 123, "BV1abc").unwrap());
        let client = FakeClient::new(r#"{"code":0,"data":{"multiply":0}}"#);
        assert!(!fetch_coined(&client, "", 123, "BV1abc").unwrap());
    }

    #[test]
    fn favoured_reads_bool() {
        let client = FakeClient::new(r#"{"code":0,"data":true}"#);
        assert!(fetch_favoured(&client, "", 123).unwrap());
        let client = FakeClient::new(r#"{"code":0,"data":false}"#);
        assert!(!fetch_favoured(&client, "", 123).unwrap());
    }

    #[test]
    fn fav_folders_parse_list() {
        let client = FakeClient::new(
            r#"{"code":0,"data":{"count":2,"list":[
                {"id":11,"title":"默认收藏夹","media_count":3,"attr":1,"fav_state":1},
                {"id":22,"title":"硬件","media_count":1,"attr":0,"fav_state":0}]}}"#,
        );
        let folders = fetch_fav_folders(&client, "", 42).unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].id, 11);
        assert_eq!(folders[0].title, "默认收藏夹");
        assert!(folders[0].is_default());
        assert_eq!(folders[1].media_count, 1);
        assert!(!folders[1].is_default());
        assert!(folders[0].contains_video());
        assert!(!folders[1].contains_video());
    }

    #[test]
    fn fav_folders_empty_list_is_ok() {
        let client = FakeClient::new(r#"{"code":0,"data":{"count":0,"list":[]}}"#);
        assert!(fetch_fav_folders(&client, "", 42).unwrap().is_empty());
    }

    #[test]
    fn missing_csrf_is_rejected_before_request() {
        // 这几个写操作都要 bili_jct；没有它就直接失败，不发请求。
        let client = ReqwestClient::new().unwrap();
        assert!(like_video(&client, "", "", "BV1abc", true)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(coin_video(&client, "", "", 123, 1, false)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(fav_video(&client, "", "", 1, 2, true)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(share_to_dynamic(&client, "", "", "hi")
            .unwrap_err()
            .contains("bili_jct"));
    }

    #[test]
    fn fav_video_rejects_missing_ids() {
        let client = ReqwestClient::new().unwrap();
        assert!(fav_video(&client, "", "csrf", 0, 0, true)
            .unwrap_err()
            .contains("aid"));
    }

    #[test]
    fn coin_video_rejects_missing_aid() {
        // coin/add 只认 av 号：没有 aid 就直接失败，不发请求
        // （带上 bvid 也不行 —— 服务端会回 -400 请求错误）。
        let client = ReqwestClient::new().unwrap();
        assert!(coin_video(&client, "", "csrf", 0, 1, false)
            .unwrap_err()
            .contains("aid"));
    }
}
