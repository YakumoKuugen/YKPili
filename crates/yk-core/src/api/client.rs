//! HTTP 客户端与完整取数流程。
//!
//! 这一层把「构造请求」（`api::bilibili`）与「发请求」接起来：
//! `ReqwestClient` 实现 `HttpClient`，`fetch_recommend` 负责
//! 「先取 WBI keys → 签名 → 拉数据 → 反序列化」这条完整链路。
//!
//! **两个必须带的头**：`User-Agent` 与 `Referer`。
//! B 站对默认 UA（`reqwest/x.y`）会直接 403，缺 Referer 有概率被风控拦。

use crate::api::bilibili::{self, HttpClient, Request};
use crate::api::models::{
    DynamicFeedResponse, HistoryResponse, PlayUrlData, RecommendResponse, RecommendVideoItem,
    ReplyResponse, UserInfoResponse, VideoInfoData, VideoInfoResponse, VideoPlayUrlResponse,
};
use crate::api::wbi;
use crate::dynamic::{CommentTarget, DynamicComment, DynamicItem, DynamicPage};
use crate::history::{HistoryCursor, HistoryEntry};
use crate::playback::{select_playback, PlaybackSource};

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

impl ReqwestClient {
    /// 提交表单（写操作统一走这里）。
    ///
    /// 只给删除历史这类必须 POST 的接口用；读接口仍走 `HttpClient::get`，
    /// 这样测试替身不需要实现写能力。
    pub fn post_form(
        &self,
        url: &str,
        form: &[(&str, String)],
        cookie: &str,
    ) -> Result<String, String> {
        let mut rb = self
            .inner
            .post(url)
            .header("User-Agent", UA)
            .header("Referer", "https://www.bilibili.com/")
            .header("Accept", "application/json, text/plain, */*");
        if !cookie.is_empty() {
            rb = rb.header("Cookie", cookie);
        }
        let resp = rb
            .form(form)
            .send()
            .map_err(|e| format!("写操作请求失败：{e}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .map_err(|e| format!("写操作读取响应失败：{e}"))?;
        if !status.is_success() {
            return Err(format!("写操作返回 HTTP {status}"));
        }
        Ok(body)
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
        let body = resp
            .text()
            .map_err(|e| format!("{} 读取响应失败：{e}", req.name))?;
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
    let parsed: UserInfoResponse =
        serde_json::from_str(&body).map_err(|e| format!("nav 响应解析失败：{e}"))?;
    let data = parsed.data.ok_or_else(|| "nav 响应缺少 data".to_string())?;
    let wbi = data
        .wbi_img
        .ok_or_else(|| "nav 响应缺少 wbi_img".to_string())?;
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
    fetch_recommend_page(client, cookie, ps, wts, 0)
}

/// 拉推荐流的第 `fresh_idx` 页；首页首次用 0，触底加载依次 1、2…
pub fn fetch_recommend_page<C>(
    client: &C,
    cookie: &str,
    ps: u32,
    wts: u64,
    fresh_idx: u32,
) -> Result<Vec<RecommendVideoItem>, String>
where
    C: HttpClient<Error = String>,
{
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    let req = bilibili::recommend_page(&img, &sub, cookie, wts, ps, fresh_idx);
    let body = client.get(&req)?;
    let parsed: RecommendResponse =
        serde_json::from_str(&body).map_err(|e| format!("推荐响应解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("推荐接口返回 code={}", parsed.code));
    }
    Ok(parsed.data.map(|d| d.item).unwrap_or_default())
}

/// 取视频详情（含 cid）。
pub fn fetch_video_info<C>(client: &C, bvid: &str, cookie: &str) -> Result<VideoInfoData, String>
where
    C: HttpClient<Error = String>,
{
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    fetch_video_info_with_keys(client, bvid, cookie, &img, &sub)
}

fn fetch_video_info_with_keys<C>(
    client: &C,
    bvid: &str,
    cookie: &str,
    img: &str,
    sub: &str,
) -> Result<VideoInfoData, String>
where
    C: HttpClient<Error = String>,
{
    let req = bilibili::video_info(bvid, img, sub, cookie, now_wts());
    let body = client.get(&req)?;
    let parsed: VideoInfoResponse =
        serde_json::from_str(&body).map_err(|e| format!("视频详情解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("视频详情返回 code={}", parsed.code));
    }
    parsed.data.ok_or_else(|| "视频详情缺少 data".to_string())
}

fn fetch_play_url_with_keys<C>(
    client: &C,
    bvid: &str,
    cid: i64,
    cookie: &str,
    img: &str,
    sub: &str,
) -> Result<PlayUrlData, String>
where
    C: HttpClient<Error = String>,
{
    let req = bilibili::play_url(bvid, cid, img, sub, cookie, now_wts());
    let body = client.get(&req)?;
    let parsed: VideoPlayUrlResponse =
        serde_json::from_str(&body).map_err(|e| format!("播放地址解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("播放地址返回 code={}", parsed.code));
    }
    parsed.data.ok_or_else(|| "播放地址缺少 data".to_string())
}

/// 点击卡片后的完整取流：详情取 cid → playurl → 选一条视频 + 可选音频。
///
/// 只取一次 WBI keys，两次接口复用；cookie 从登录会话传入（未登录也能拿 480P）。
pub fn fetch_playback<C>(client: &C, bvid: &str, cookie: &str) -> Result<PlaybackSource, String>
where
    C: HttpClient<Error = String>,
{
    if bvid.trim().is_empty() {
        return Err("卡片没有 bvid，无法播放".into());
    }
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    let info = fetch_video_info_with_keys(client, bvid, cookie, &img, &sub)?;
    let cid = if info.cid > 0 {
        info.cid
    } else {
        info.pages
            .iter()
            .find(|p| p.cid > 0)
            .map(|p| p.cid)
            .unwrap_or(0)
    };
    if cid == 0 {
        return Err("视频详情没有 cid，无法取播放地址".into());
    }
    let play = fetch_play_url_with_keys(client, bvid, cid, cookie, &img, &sub)?;
    let mut source = select_playback(&info, &play)?;
    // 详情里 cid 有时是 0（分 P），播放地址用的是 pages 里的 cid；
    // 心跳上报必须用真正播放的那一 P。
    source.cid = cid;
    if source.bvid.trim().is_empty() {
        source.bvid = bvid.to_string();
    }
    Ok(source)
}

/// 拉一页观看历史。
///
/// `cursor = None` 表示第一页；返回值里的第二个元素是下一页游标
/// （接口不再给游标 / 列表为空时为 `None`）。未登录直接返回错误，
/// 避免页面把「空历史」和「未登录」混成同一种状态。
pub fn fetch_history<C>(
    client: &C,
    cookie: &str,
    cursor: Option<HistoryCursor>,
    business: &str,
) -> Result<(Vec<HistoryEntry>, Option<HistoryCursor>), String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法读取观看历史".into());
    }
    let cursor = cursor.unwrap_or_default();
    let req = bilibili::history_cursor(cursor.max, cursor.view_at, business, cookie);
    let body = client.get(&req)?;
    let parsed: HistoryResponse =
        serde_json::from_str(&body).map_err(|e| format!("历史响应解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!(
            "历史接口返回 code={}（{}）",
            parsed.code,
            parsed.message.trim()
        ));
    }
    let data = parsed.data.ok_or_else(|| "历史响应缺少 data".to_string())?;
    let next = data
        .cursor
        .filter(|c| c.max != 0 || c.view_at != 0)
        .map(|c| HistoryCursor {
            max: c.max,
            view_at: c.view_at,
        });
    let items = data
        .list
        .into_iter()
        .map(HistoryEntry::from_raw)
        .collect::<Vec<_>>();
    Ok((items, next))
}

/// 拉一页关注动态。
///
/// 不带 WBI 签名（该接口只认 Cookie）。未登录直接返回错误，避免把
/// 「-101 账号未登录」包装成空列表，页面上分不清「没关注」和「没登录」。
pub fn fetch_following_dynamics<C>(
    client: &C,
    cookie: &str,
    offset: &str,
) -> Result<DynamicPage, String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法获取关注动态".into());
    }
    let req = bilibili::following_dynamics(offset, cookie);
    let body = client.get(&req)?;
    let parsed: DynamicFeedResponse =
        serde_json::from_str(&body).map_err(|e| format!("动态流响应解析失败：{e}"))?;
    if parsed.code != 0 {
        let message = if parsed.message.is_empty() {
            String::new()
        } else {
            format!("：{}", parsed.message)
        };
        return Err(format!("动态流返回 code={}{message}", parsed.code));
    }
    let data = parsed
        .data
        .ok_or_else(|| "动态流响应缺少 data".to_string())?;
    let mut items: Vec<DynamicItem> = data
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, raw)| DynamicItem::from_raw(raw, index))
        .collect();
    // 原始条目里可能夹着已删除 / 纯直播推荐等被过滤掉的项，序号会断档。
    // 重新编号，保证瀑布流的卡片 id 连续、稳定。
    for (index, item) in items.iter_mut().enumerate() {
        item.index = index;
    }
    Ok(DynamicPage {
        items,
        offset: data.offset,
        has_more: data.has_more,
    })
}

/// 拉动态评论第一页。
///
/// 评论失败不应该影响详情浮层本身，因此调用方会把它当成「评论加载失败」而不是
/// 整个浮层错误；这里仍然返回 `Result`，把具体原因带出去。
pub fn fetch_dynamic_comments<C>(
    client: &C,
    cookie: &str,
    target: &CommentTarget,
) -> Result<Vec<DynamicComment>, String>
where
    C: HttpClient<Error = String>,
{
    if target.oid.is_empty() || target.kind <= 0 {
        return Ok(Vec::new());
    }
    let req = bilibili::dynamic_comments(&target.oid, target.kind, cookie, 1, 20);
    let body = client.get(&req)?;
    let parsed: ReplyResponse =
        serde_json::from_str(&body).map_err(|e| format!("评论响应解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("评论接口返回 code={}", parsed.code));
    }
    Ok(parsed
        .data
        .map(|data| data.replies.iter().map(DynamicComment::from_raw).collect())
        .unwrap_or_default())
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

/// 一次观看历史上报的数据。
///
/// 用 `/x/v2/history/report`：打开 / 关闭各调一次，写进度但不做周期心跳。
#[derive(Debug, Clone, Default)]
pub struct HistoryProgress {
    pub aid: i64,
    pub cid: i64,
    /// 观看进度（秒）；0 = 刚打开。
    pub progress: u64,
}

/// 上报一次观看历史进度。
///
/// 这是 B 站「上报观看进度」的专用接口，`progress` 单位秒；
/// 打开时传 0、关闭时传当前播放位置即可，不需要周期调用。
/// 需要 `bili_jct` 作为 CSRF token。
pub fn report_history(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    history: &HistoryProgress,
) -> Result<(), String> {
    if csrf.trim().is_empty() {
        return Err("登录会话缺少 bili_jct，无法上报观看历史".into());
    }
    if history.aid <= 0 || history.cid <= 0 {
        return Err("缺少 aid / cid，无法上报观看历史".into());
    }
    let form = vec![
        ("aid", history.aid.to_string()),
        ("cid", history.cid.to_string()),
        ("progress", history.progress.to_string()),
        ("csrf", csrf.to_string()),
    ];
    let body = client.post_form(
        &format!("{}/x/v2/history/report", bilibili::API_BASE),
        &form,
        cookie,
    )?;
    check_write_code(&body, "观看历史上报")?;
    Ok(())
}

/// 删除若干条观看历史。
///
/// B 站一次只收一个 `kid`，逐条提交；任何一条失败就立刻返回错误，
/// 调用方保留本地条目，避免出现「界面删了、服务端没删」的假成功。
/// 需要 `bili_jct`（`UserSession::bili_jct`）作为 CSRF token。
pub fn delete_history_entries(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    kids: &[i64],
) -> Result<(), String> {
    if csrf.trim().is_empty() {
        return Err("登录会话缺少 bili_jct，无法删除历史".into());
    }
    for kid in kids {
        let body = client.post_form(
            &format!("{}/x/v2/history/delete", bilibili::API_BASE),
            &[("kid", kid.to_string()), ("csrf", csrf.to_string())],
            cookie,
        )?;
        check_write_code(&body, "删除历史")?;
    }
    Ok(())
}

/// 清空当前账号的全部观看历史。
pub fn clear_history(client: &ReqwestClient, cookie: &str, csrf: &str) -> Result<(), String> {
    if csrf.trim().is_empty() {
        return Err("登录会话缺少 bili_jct，无法清空历史".into());
    }
    let body = client.post_form(
        &format!("{}/x/v2/history/clear", bilibili::API_BASE),
        &[("csrf", csrf.to_string())],
        cookie,
    )?;
    check_write_code(&body, "清空历史")
}

fn check_write_code(body: &str, action: &str) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("{action}响应解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("{action}失败：code={code} {message}"));
    }
    Ok(())
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
    fn playback_pipeline_selects_video_and_audio() {
        struct PlayClient;
        impl HttpClient for PlayClient {
            type Error = String;
            fn get(&self, req: &Request) -> Result<String, String> {
                match req.name {
                    "user_info" => Ok(r#"{"code":0,"data":{"wbi_img":{"img_url":"a.png","sub_url":"b.png"}}}"#.into()),
                    "video_info" => Ok(r#"{"code":0,"data":{"bvid":"BV1","cid":42,"title":"T","duration":100}}"#.into()),
                    "play_url" => Ok(r#"{"code":0,"data":{"timelength":107000,"dash":{"video":[{"id":80,"baseUrl":"v80","codecs":"avc1","width":1920,"height":1080},{"id":32,"baseUrl":"v32","codecs":"avc1","width":640,"height":360}],"audio":[{"id":30280,"baseUrl":"a80","codecs":"mp4a"}]}}}"#.into()),
                    other => Err(format!("未预期请求：{other}")),
                }
            }
        }
        let src = fetch_playback(&PlayClient, "BV1", "").unwrap();
        assert_eq!(src.video_url, "v80");
        assert_eq!(src.audio_url.as_deref(), Some("a80"));
        assert_eq!(src.duration_secs, 107);
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
