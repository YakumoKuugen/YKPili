//! HTTP 客户端与完整取数流程。
//!
//! 这一层把「构造请求」（`api::bilibili`）与「发请求」接起来：
//! `ReqwestClient` 实现 `HttpClient`，`fetch_recommend` 负责
//! 「先取 WBI keys → 签名 → 拉数据 → 反序列化」这条完整链路。
//!
//! **两个必须带的头**：`User-Agent` 与 `Referer`。
//! B 站对默认 UA（`reqwest/x.y`）会直接 403，缺 Referer 有概率被风控拦。
//!
//! **限流**：所有真实请求在 `ReqwestClient` 里统一过 [`limits::acquire_for`]，
//! 按接口分组限制请求间隔（评论 1.2s、写操作 1.2s、推荐 0.8s……），
//! 见 [`crate::api::limits`]。测试替身不走网络，因此不受影响、也不会被拖慢。

use crate::api::bilibili::{self, HttpClient, Request};
use crate::api::limits;
use crate::api::models::{
    DynamicFeedResponse, HistoryResponse, MsgfeedInboxResponse, MsgfeedUnreadResponse, PlayUrlData,
    RecommendResponse, RecommendVideoItem, RelationFriendsResponse, RelationResponse, ReplyMainResponse,
    ReplyResponse,
    SessionListResponse, SessionMsgsResponse, UserCardsResponse, UserInfoResponse, VideoInfoData,
    VideoInfoResponse, VideoPlayUrlResponse, VideoTagsResponse,
};
use crate::api::wbi;
use crate::danmaku::DanmakuSet;
use crate::dynamic::{CommentTarget, DynamicComment, DynamicItem, DynamicPage};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use crate::history::{HistoryCursor, HistoryEntry};
use crate::messages::{
    aggregate_likes, ChatMessage, Conversation, InboxCursor, MessageChannel, MessageEntry,
    MessageUser, UnreadCounts,
};
use crate::playback::{select_playback, PlaybackSource};

/// 伪装成桌面浏览器 —— 与旧实现的 UA 策略一致。
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 默认请求超时（秒）。与旧 `ApiManager` 一致：15s。
pub const REQUEST_TIMEOUT_SECS: u64 = 15;

/// 写操作的来源站点。
///
/// B 站的写接口会拿 `Origin` 和 `Referer` 的域名做一致性校验，
/// 缺 `Origin` 会被当成可疑请求。两者都填同一个域名，和浏览器一致。
const ORIGIN: &str = "https://www.bilibili.com";

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
            .header("Origin", ORIGIN)
            .header("Accept", "application/json, text/plain, */*");
        if !cookie.is_empty() {
            rb = rb.header("Cookie", cookie);
        }
        // 写操作一律进 `Write` 桶：点赞 / 投币 / 收藏 / 发动态被连点最容易吃风控。
        limits::acquire_for(url, true);
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

    /// 提交 JSON 请求体（动态发布这类接口要 JSON，不是表单）。
    ///
    /// CSRF token 走 query（`?csrf=`），body 是 JSON —— B 站的
    /// `x/dynamic/feed/create/dyn` 就是这个约定。
    pub fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        cookie: &str,
    ) -> Result<String, String> {
        let mut rb = self
            .inner
            .post(url)
            .header("User-Agent", UA)
            .header("Referer", "https://www.bilibili.com/")
            .header("Origin", ORIGIN)
            .header("Accept", "application/json, text/plain, */*")
            .header("Content-Type", "application/json");
        if !cookie.is_empty() {
            rb = rb.header("Cookie", cookie);
        }
        limits::acquire_for(url, true);
        let resp = rb
            .json(body)
            .send()
            .map_err(|e| format!("JSON 写操作请求失败：{e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| format!("JSON 写操作读取响应失败：{e}"))?;
        if !status.is_success() {
            return Err(format!("JSON 写操作返回 HTTP {status}"));
        }
        Ok(text)
    }

    /// 读二进制响应（弹幕 XML 是 deflate 压缩体，不能先按 UTF-8 解码）。
    ///
    /// 不启用 reqwest 的 `deflate` feature：少一个依赖树分支，
    /// 解压交给 `fetch_danmaku` 里显式做，测试也更容易覆盖。
    pub fn get_bytes(&self, req: &Request) -> Result<Vec<u8>, String> {
        let mut rb = self
            .inner
            .get(&req.url)
            .header("User-Agent", UA)
            .header("Referer", "https://www.bilibili.com/")
            .header("Accept", "text/xml, application/octet-stream, */*");
        if !req.cookie.is_empty() {
            rb = rb.header("Cookie", &req.cookie);
        }
        limits::acquire_for(&req.url, false);
        let resp = rb
            .send()
            .map_err(|e| format!("{} 请求失败：{e}", req.name))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .map_err(|e| format!("{} 读取响应失败：{e}", req.name))?;
        if !status.is_success() {
            return Err(format!("{} 返回 HTTP {status}", req.name));
        }
        Ok(bytes.to_vec())
    }
}

impl HttpClient for ReqwestClient {
    type Error = String;

    fn cached_wbi_keys(&self) -> Option<(String, String)> {
        cached_wbi_keys()
    }

    fn store_wbi_keys(&self, img: &str, sub: &str) {
        store_wbi_keys(img, sub);
    }

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
        limits::acquire_for(&req.url, false);
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

/// WBI keys 的进程内缓存：(img_key, sub_key, 取到的时间)。
///
/// 放全局而不是 `ReqwestClient` 字段，是因为 App 每个后台任务都会
/// `ReqwestClient::new()` 一次；挂在实例上等于没缓存。
static WBI_KEYS: std::sync::OnceLock<
    std::sync::Mutex<Option<(String, String, std::time::Instant)>>,
> = std::sync::OnceLock::new();

/// WBI keys 缓存有效期。
///
/// B 站每天轮换一次；30 分钟足够激进地复用，又不会在轮换后一直用旧 key
/// 去签名（那正是 `-352` 的另一个来源）。
const WBI_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

fn cached_wbi_keys() -> Option<(String, String)> {
    let cache = WBI_KEYS.get_or_init(|| std::sync::Mutex::new(None));
    let guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .as_ref()
        .filter(|(_, _, at)| at.elapsed() < WBI_TTL)
        .map(|(img, sub, _)| (img.clone(), sub.clone()))
}

fn store_wbi_keys(img: &str, sub: &str) {
    let cache = WBI_KEYS.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some((img.to_string(), sub.to_string(), std::time::Instant::now()));
}

/// 取 WBI keys（`img_key` / `sub_key`）。
///
/// **不需要登录**：`nav` 在未登录时也会返回 `wbi_img`，只是 `isLogin=false`。
/// 这一点很关键 —— 只有这样首页才能在没登录的情况下加载推荐。
///
/// 真实客户端会走 [`WBI_KEYS`] 缓存：推荐 / 详情 / 取流 / 发私信都要签名，
/// 每个请求都重取一次 `nav` 等于把接口量翻倍，还多一次风控暴露面。
pub fn fetch_wbi_keys<C>(client: &C, cookie: &str) -> Result<(String, String), String>
where
    C: HttpClient<Error = String>,
{
    if let Some(keys) = client.cached_wbi_keys() {
        return Ok(keys);
    }
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
    client.store_wbi_keys(&img, &sub);
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

/// 拉视频标签。标签接口是纯读、不需要 WBI；失败时由调用方退回分区名。
pub fn fetch_video_tags<C>(client: &C, bvid: &str, cookie: &str) -> Result<Vec<String>, String>
where
    C: HttpClient<Error = String>,
{
    if bvid.trim().is_empty() {
        return Ok(Vec::new());
    }
    let body = client.get(&bilibili::video_tags(bvid, cookie))?;
    let parsed: VideoTagsResponse =
        serde_json::from_str(&body).map_err(|e| format!("标签解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("标签接口返回 code={}", parsed.code));
    }
    Ok(parsed
        .data
        .unwrap_or_default()
        .into_iter()
        .map(|tag| tag.tag_name.trim().to_string())
        .filter(|name| !name.is_empty())
        .take(3)
        .collect())
}

/// playurl 响应里是否真的有可播放流。
///
/// B 站风控会用 `code=0 + data.v_voucher` 返回空响应：
/// 不能把「没流」误判成视频不可播，调用方应走旧端点兜底。
fn has_playable_stream(play: &PlayUrlData) -> bool {
    play.dash
        .as_ref()
        .is_some_and(|dash| !dash.video.is_empty())
        || !play.durl.is_empty()
}

/// 解析 playurl 响应；WBI 与旧端点共用。
fn parse_play_url_response(body: &str) -> Result<PlayUrlData, String> {
    let parsed: VideoPlayUrlResponse =
        serde_json::from_str(body).map_err(|e| format!("播放地址解析失败：{e}"))?;
    if parsed.code != 0 {
        return Err(format!("播放地址返回 code={}", parsed.code));
    }
    parsed.data.ok_or_else(|| "播放地址缺少 data".to_string())
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
    parse_play_url_response(&body)
}

/// 旧 playurl 端点（不带 WBI 签名）；只在 WBI 端点被风控时调用。
fn fetch_legacy_play_url<C>(
    client: &C,
    bvid: &str,
    cid: i64,
    cookie: &str,
) -> Result<PlayUrlData, String>
where
    C: HttpClient<Error = String>,
{
    let req = bilibili::play_url_legacy(bvid, cid, cookie);
    let body = client.get(&req)?;
    parse_play_url_response(&body)
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
    let play = if has_playable_stream(&play) {
        play
    } else {
        // 典型场景：B 站风控对 WBI 端点返回 `code=0 + v_voucher`，
        // data 里没有 dash/durl。旧端点不带 WBI、风控策略不同，实测仍能出流。
        eprintln!(
            "[playurl] WBI 端点没有可播放流（v_voucher={}），改用旧端点兜底",
            play.v_voucher.is_some()
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
        match fetch_legacy_play_url(client, bvid, cid, cookie) {
            Ok(legacy) => legacy,
            Err(e) => {
                eprintln!("[playurl] 旧端点兜底失败：{e}");
                play
            }
        }
    };
    if !has_playable_stream(&play) && play.v_voucher.is_some() {
        return Err("播放地址被 B 站风控拦截（v_voucher），请稍后重试".into());
    }
    let mut source = select_playback(&info, &play)?;
    // 详情里 cid 有时是 0（分 P），播放地址用的是 pages 里的 cid；
    // 心跳上报必须用真正播放的那一 P。
    source.cid = cid;
    // cid 被纠正后，面板「选集」的当前 P 也要跟着重算。
    source.current_page = info
        .pages
        .iter()
        .find(|page| page.cid == cid)
        .map(|page| page.page)
        .unwrap_or(source.current_page);
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

/// 在观看历史里找某个视频的「已观看秒数」（续播用）。
///
/// 取**最近一次**观看的那条（同 bvid 可能有多条历史）。
/// 已看完 / 刚开始看的条目返回 `None` —— 前者从头播，后者不值得续。
pub fn find_history_progress(entries: &[HistoryEntry], bvid: &str) -> Option<u64> {
    let bvid = bvid.trim();
    if bvid.is_empty() {
        return None;
    }
    let entry = entries
        .iter()
        .filter(|e| e.bvid.trim() == bvid)
        .max_by_key(|e| e.view_at)?;
    if entry.is_finished() {
        return None;
    }
    let progress = entry.progress_secs;
    if progress < crate::resume::RESUME_MIN_SECS as u64 {
        return None;
    }
    Some(progress)
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

/// 每页评论要多少条 —— **每次只发 1 个请求**。
///
/// 线上 `ps` 上限是 30，但这里刻意要 20：网页端默认就是 20，单个响应已经
/// ~270KB（热门评论带楼中楼）。旧实现「一次要 100 条」在这里拼 4 个连续
/// 请求，正好撞上 B 站评论区的频控 → `-352`。现在首屏只发 1 个请求，
/// 用户滚到列表底部再取下一页。
pub const COMMENT_PAGE_SIZE: u32 = 20;

/// 一页评论（游标翻页）。
#[derive(Debug, Clone, Default)]
pub struct CommentPage {
    pub items: Vec<DynamicComment>,
    /// 下一页游标；原样回传给 [`fetch_comment_page`]。
    pub next: u64,
    /// 服务端说「真的没有更多了」。这是唯一权威的到底信号 ——
    /// 不能用「本页不满 ps」推断：置顶评论、被过滤的评论都会让页条数变少。
    pub is_end: bool,
    /// 评论总数（`cursor.all_count`）。
    pub total: u64,
}

/// 拉一页评论（`/x/v2/reply/main`），游标翻页。
///
/// `size` 会被夹到 1..=30（服务端硬上限）；调用方应传
/// [`COMMENT_PAGE_SIZE`]，**不要**为了「一次取更多」而在上层循环拼页。
pub fn fetch_comment_page<C>(
    client: &C,
    cookie: &str,
    target: &CommentTarget,
    next: u64,
    size: u32,
) -> Result<CommentPage, String>
where
    C: HttpClient<Error = String>,
{
    if target.oid.is_empty() || target.kind <= 0 {
        return Ok(CommentPage {
            is_end: true,
            ..Default::default()
        });
    }
    let req = bilibili::comment_main(&target.oid, target.kind, cookie, next, size.clamp(1, 30));
    let body = client.get(&req)?;
    let parsed: ReplyMainResponse =
        serde_json::from_str(&body).map_err(|e| format!("评论响应解析失败：{e}"))?;
    if parsed.code != 0 {
        // `-352` 是风控/频控的统一码：短时间连续打评论接口就会撞上。
        // 文案要说人话，否则界面上只剩一串数字，用户不知道等一会儿就好。
        if parsed.code == -352 {
            return Err("评论请求太频繁，已被 B 站风控拦截（-352），请稍后再试".to_string());
        }
        let message = if parsed.message.is_empty() {
            String::new()
        } else {
            format!("：{}", parsed.message)
        };
        return Err(format!("评论接口返回 code={}{message}", parsed.code));
    }
    let data = parsed
        .data
        .ok_or_else(|| "评论响应缺少 data".to_string())?;
    let cursor = data.cursor.clone().unwrap_or_default();
    let mut seen = HashSet::new();
    let items = data
        .replies
        .iter()
        .filter(|raw| seen.insert(raw.rpid))
        .map(DynamicComment::from_raw)
        .collect();
    Ok(CommentPage {
        items,
        next: cursor.next,
        is_end: cursor.is_end,
        total: cursor.all_count,
    })
}

/// 拉取一个视频（当前分 P）的全部弹幕。
///
/// 走 `/x/v1/dm/list.so`：返回 deflate 压缩的 XML，包含滚动 / 顶部 / 底部
/// 三种模式（比分段 protobuf 的 `seg.so` 更完整）。匿名可达，登录 Cookie
/// 可选；视频受限时服务端可能返回错误页，这里会带出解析失败的原因。
pub fn fetch_danmaku(client: &ReqwestClient, cid: i64, cookie: &str) -> Result<DanmakuSet, String> {
    if cid <= 0 {
        return Err("缺少 cid，无法加载弹幕".into());
    }
    let bytes = client.get_bytes(&bilibili::danmaku_xml(cid, cookie))?;
    let xml = decode_danmaku_body(&bytes)?;
    Ok(DanmakuSet::from_xml(&xml))
}

/// 弹幕响应解压。
///
/// B 站固定用 `Content-Encoding: deflate`，但 CDN / 代理偶尔会回明文；
/// 先按首字节判断，再依次试 raw deflate、zlib、gzip。都不行时把原始
/// 字节当文本返回，让上层错误信息能带出服务端到底说了什么。
fn decode_danmaku_body(bytes: &[u8]) -> Result<String, String> {
    if bytes.first() == Some(&b'<') {
        return String::from_utf8(bytes.to_vec()).map_err(|e| format!("弹幕 XML 编码失败：{e}"));
    }

    let mut errors: Vec<String> = Vec::new();
    match decompress_xml(flate2::read::DeflateDecoder::new(bytes)) {
        Ok(xml) => return Ok(xml),
        Err(error) => errors.push(format!("deflate: {error}")),
    }
    match decompress_xml(flate2::read::ZlibDecoder::new(bytes)) {
        Ok(xml) => return Ok(xml),
        Err(error) => errors.push(format!("zlib: {error}")),
    }
    match decompress_xml(flate2::read::GzDecoder::new(bytes)) {
        Ok(xml) => return Ok(xml),
        Err(error) => errors.push(format!("gzip: {error}")),
    }

    // 兜底：把响应原文（可能是 JSON 错误）带进错误信息。
    let plain = String::from_utf8_lossy(bytes);
    let preview: String = plain.chars().take(120).collect();
    Err(format!(
        "弹幕响应无法解压（{}）：{preview}",
        errors.join("；")
    ))
}

/// 解压一段压缩体，并确认结果确实是 XML。
fn decompress_xml(mut reader: impl Read) -> Result<String, String> {
    let mut out = Vec::new();
    reader
        .read_to_end(&mut out)
        .map_err(|error| error.to_string())?;
    if out.first() != Some(&b'<') {
        return Err("解压后不是 XML".into());
    }
    String::from_utf8(out).map_err(|error| error.to_string())
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

pub(crate) fn check_write_code(body: &str, action: &str) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("{action}响应解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("{action}失败：code={code} {message}"));
    }
    Ok(())
}

// ================= 消息中心 =================

/// 未读数：`reply` / `at` / `like` / `recv_like` 各取各的，私信由会话列表另算。
pub fn fetch_msgfeed_unread<C>(client: &C, cookie: &str) -> Result<UnreadCounts, String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法读取消息未读数".into());
    }
    let body = client.get(&bilibili::msgfeed_unread(cookie))?;
    let parsed: MsgfeedUnreadResponse =
        serde_json::from_str(&body).map_err(|e| format!("未读数解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "未读数")?;
    let data = parsed.data.ok_or_else(|| "未读数响应缺少 data".to_string())?;
    Ok(UnreadCounts::from_unread_data(&data))
}

/// 拉一页互动消息。`cursor = None` 为第一页；返回下一页游标（到底为 None）。
pub fn fetch_msgfeed_inbox<C>(
    client: &C,
    channel: MessageChannel,
    cursor: Option<InboxCursor>,
    cookie: &str,
) -> Result<(Vec<MessageEntry>, Option<InboxCursor>), String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法读取消息".into());
    }
    if channel == MessageChannel::Dm {
        return Err("私信通道不走 msgfeed 接口".into());
    }
    let cursor = cursor.unwrap_or_default();
    if cursor.is_end {
        return Ok((Vec::new(), None));
    }
    let body = client.get(&bilibili::msgfeed_inbox(
        channel,
        (cursor.id, cursor.time),
        cookie,
    ))?;
    let parsed: MsgfeedInboxResponse =
        serde_json::from_str(&body).map_err(|e| format!("消息列表解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "消息列表")?;
    let data = parsed
        .data
        .ok_or_else(|| "消息列表响应缺少 data".to_string())?;
    // 「收到的赞」是 `{ latest, total }`，不是平铺的 items。
    // `latest` 是上次查看后的新增，`total` 才是完整列表；两者拼起来并去重。
    let (mut raw_items, raw_cursor, last_view_at) = match data.total {
        Some(total) => {
            let last_view_at = data
                .latest
                .as_ref()
                .map(|latest| latest.last_view_at)
                .filter(|ts| *ts > 0)
                .unwrap_or(data.last_view_at);
            let mut raw_items = data
                .latest
                .map(|latest| latest.items)
                .unwrap_or_default();
            raw_items.extend(total.items);
            (raw_items, total.cursor, last_view_at)
        }
        None => (data.items, data.cursor, data.last_view_at),
    };
    if channel == MessageChannel::Like {
        let mut seen = std::collections::HashSet::new();
        raw_items.retain(|item| {
            let key = if item.id != 0 {
                item.id
            } else {
                item.item.item_id
            };
            seen.insert(key)
        });
    }
    // @ 没有 last_view_at、旧版赞也可能没有：默认全部已读，
    // 再由 app 按 unread 的「最新 N 条」降级成未读；不能默认成全部未读。
    let last_view_at = if last_view_at == 0 && channel != MessageChannel::Reply {
        now_wts() as i64
    } else {
        last_view_at
    };
    let raw_cursor = raw_cursor.unwrap_or_default();
    let is_end = raw_cursor.is_end || raw_items.is_empty();
    let next = (!is_end && (raw_cursor.id != 0 || raw_cursor.time != 0)).then_some(InboxCursor {
        id: raw_cursor.id,
        time: raw_cursor.time,
        is_end: false,
    });
    let items = raw_items
        .into_iter()
        .map(|raw| MessageEntry::from_raw(channel, raw, last_view_at))
        .collect::<Vec<_>>();
    // @ / 赞接口未被公开文档收录（E-25）：字段缺失时退回通用文案，
    // 但不要在客户端再猜一遍聚合，交给 aggregate_likes 的明确规则。
    let items = if channel == MessageChannel::Like {
        aggregate_likes(items)
    } else {
        items
    };
    Ok((items, next))
}

/// 拉互关好友白名单（最多 5 页 / 250 人），用于私信会话的「互关」判定。
pub fn fetch_mutual_ids<C>(client: &C, cookie: &str, mid: i64) -> Result<HashSet<i64>, String>
where
    C: HttpClient<Error = String>,
{
    if mid <= 0 {
        return Err("缺少当前账号 mid，无法判定互关".into());
    }
    let mut ids = HashSet::new();
    for page in 1..=5 {
        let mut req = bilibili::mutual_friends(mid, cookie);
        req.url = req.url.replace("page=1", &format!("page={page}"));
        let body = client.get(&req)?;
        let parsed: RelationFriendsResponse =
            serde_json::from_str(&body).map_err(|e| format!("互关列表解析失败：{e}"))?;
        check_message_code(parsed.code, &parsed.message, "互关列表")?;
        let list = parsed.data.map(|data| data.list).unwrap_or_default();
        if list.is_empty() {
            break;
        }
        let before = ids.len();
        for friend in list {
            if friend.mid > 0 {
                ids.insert(friend.mid);
            }
        }
        // 接口不分页（一次给完）时第二页会重复同一批，靠「没有新增」收手。
        if ids.len() == before {
            break;
        }
    }
    Ok(ids)
}

/// 单条关系校验：`attribute == 6` 是互关，`128` 是拉黑。
pub fn fetch_relation_attribute<C>(client: &C, cookie: &str, fid: i64) -> Result<i64, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&bilibili::relation_attribute(fid, cookie))?;
    let parsed: RelationResponse =
        serde_json::from_str(&body).map_err(|e| format!("关系解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "关系")?;
    Ok(parsed.data.map(|data| data.attribute).unwrap_or(0))
}

/// 私信会话列表 + 用户名片。昵称 / 头像不在会话响应里，必须二次补。
pub fn fetch_conversations<C>(
    client: &C,
    cookie: &str,
    my_mid: i64,
    begin_ts: Option<i64>,
) -> Result<(Vec<Conversation>, Option<i64>), String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法读取私信".into());
    }
    let body = client.get(&bilibili::message_sessions(begin_ts, cookie))?;
    let parsed: SessionListResponse =
        serde_json::from_str(&body).map_err(|e| format!("私信会话解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "私信会话")?;
    let data = parsed.data.unwrap_or_default();
    let raw_sessions = data.session_list;
    if raw_sessions.is_empty() {
        return Ok((Vec::new(), None));
    }
    let uids: Vec<i64> = raw_sessions
        .iter()
        .map(|session| session.talker_id)
        .filter(|uid| *uid > 0)
        .collect();
    let cards = fetch_user_cards(client, cookie, &uids).unwrap_or_default();
    let next = raw_sessions
        .iter()
        .map(|session| session.session_ts)
        .min()
        .filter(|ts| data.has_more != 0 && *ts > 0);
    let mut conversations: Vec<Conversation> = raw_sessions
        .into_iter()
        .filter(|session| {
            session.talker_id > 0
                && session.session_type != 2
                // 系统通知（登录 / 收益 / 预约）与「UP 主助手」不在这里。
                && session.system_msg_type == 0
                && session
                    .last_msg
                    .as_ref()
                    .map(|message| message.msg_type != 10)
                    .unwrap_or(true)
        })
        // session_type=9 本身就是「互相关注」档；白名单到了再逐条校正。
        .map(|session| Conversation::from_raw(session, my_mid, &cards, Some(true)))
        .collect();
    conversations.sort_by(|a, b| b.session_ts.cmp(&a.session_ts));
    Ok((conversations, next))
}

/// 用户名片：`data` 是以 mid 为 key 的对象，字段名按 `name` / `uname` 兼容。
pub fn fetch_user_cards<C>(
    client: &C,
    cookie: &str,
    uids: &[i64],
) -> Result<HashMap<i64, MessageUser>, String>
where
    C: HttpClient<Error = String>,
{
    let mut cards = HashMap::new();
    if uids.is_empty() {
        return Ok(cards);
    }
    let body = client.get(&bilibili::user_cards(uids, cookie))?;
    let parsed: UserCardsResponse =
        serde_json::from_str(&body).map_err(|e| format!("用户名片解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "用户名片")?;
    let Some(data) = parsed.data else {
        return Ok(cards);
    };
    // 真实响应是数组；旧版本 / 其他端点可能是以 mid 为 key 的对象。两种都接。
    let values: Vec<(Option<i64>, &serde_json::Value)> = match &data {
        serde_json::Value::Array(items) => items.iter().map(|item| (None, item)).collect(),
        serde_json::Value::Object(object) => object
            .iter()
            .map(|(key, value)| (key.parse::<i64>().ok(), value))
            .collect(),
        _ => Vec::new(),
    };
    for (fallback_mid, value) in values {
        let mid = value
            .get("mid")
            .and_then(|v| v.as_i64())
            .or(fallback_mid)
            .unwrap_or(0);
        if mid == 0 {
            continue;
        }
        let nickname = value
            .get("name")
            .or_else(|| value.get("uname"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let avatar = value
            .get("face")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        cards.insert(
            mid,
            MessageUser {
                mid,
                nickname,
                avatar,
            },
        );
    }
    Ok(cards)
}

/// 只算私信未读总数（给顶栏 / Dock 红点用，不拉名片也不落会话列表）。
///
/// `session_type=9` 是互关档；这与设计文档「Σ 互关会话 unread_count」同义。
pub fn fetch_dm_unread_total<C>(client: &C, cookie: &str) -> Result<u64, String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Ok(0);
    }
    let body = client.get(&bilibili::message_sessions(None, cookie))?;
    let parsed: SessionListResponse =
        serde_json::from_str(&body).map_err(|e| format!("私信未读解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "私信未读")?;
    Ok(parsed
        .data
        .map(|data| {
            data.session_list
                .iter()
                .filter(|session| {
                    session.session_type != 2
                        && session.system_msg_type == 0
                        && session
                            .last_msg
                            .as_ref()
                            .map(|message| message.msg_type != 10)
                            .unwrap_or(true)
                })
                .map(|session| session.unread_count.max(0) as u64)
                .sum()
        })
        .unwrap_or(0))
}

/// 拉一页聊天记录。返回 `(按时间升序的消息, 是否还有更早, 下一页 begin_seqno)`。
pub fn fetch_chat_messages<C>(
    client: &C,
    cookie: &str,
    talker_id: i64,
    my_mid: i64,
    begin_seqno: Option<i64>,
) -> Result<(Vec<ChatMessage>, bool, i64), String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法读取私信".into());
    }
    let body = client.get(&bilibili::session_messages(talker_id, begin_seqno, 20, cookie))?;
    let parsed: SessionMsgsResponse =
        serde_json::from_str(&body).map_err(|e| format!("聊天记录解析失败：{e}"))?;
    check_message_code(parsed.code, &parsed.message, "聊天记录")?;
    let data = parsed.data.ok_or_else(|| "聊天记录响应缺少 data".to_string())?;
    let has_more = data.has_more != 0;
    let next_begin = data.min_seqno;
    let mut messages: Vec<ChatMessage> = data
        .messages
        .into_iter()
        .map(|raw| ChatMessage::from_raw(raw, my_mid))
        .collect();
    // 接口给的是倒序；页面按时间正序排，相邻消息才好做「5 分钟」分组。
    messages.sort_by_key(|message| message.seqno);
    Ok((messages, has_more, next_begin))
}

/// 打开会话 / 点「标为已读」：`ack_seqno` 留空表示最新。
pub fn mark_conversation_read(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    talker_id: i64,
) -> Result<(), String> {
    if csrf.trim().is_empty() {
        return Err("登录会话缺少 bili_jct，无法标记已读".into());
    }
    let body = client.post_form(
        &format!(
            "{}/session_svr/v1/session_svr/update_ack",
            bilibili::MESSAGE_BASE
        ),
        &[
            ("talker_id", talker_id.to_string()),
            ("session_type", "1".to_string()),
            ("ack_seqno", String::new()),
            ("csrf", csrf.to_string()),
            ("csrf_token", csrf.to_string()),
        ],
        cookie,
    )?;
    check_message_code_value(&body, "标记已读")
}

/// 移除会话（需要模态确认；失败时调用方保留条目）。
pub fn remove_conversation(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    talker_id: i64,
) -> Result<(), String> {
    if csrf.trim().is_empty() {
        return Err("登录会话缺少 bili_jct，无法移除会话".into());
    }
    // 文档核实为 GET（`talker_id` + `session_type=1` + csrf）。
    let req = Request {
        url: format!(
            "{}/session_svr/v1/session_svr/remove_session?talker_id={talker_id}&session_type=1&csrf={csrf}&csrf_token={csrf}",
            bilibili::MESSAGE_BASE
        ),
        cookie: cookie.to_string(),
        name: "remove_session",
    };
    let body = client.get(&req)?;
    check_message_code_value(&body, "移除会话")
}

/// 发送私信（只做文本 `msg_type=1`）。
///
/// 成功后调用方**不要**乐观伪造服务端消息：用接口返回的时间 / seqno 再拉一次，
/// 或者先插入本地「发送中」气泡再以回包替换。这里只负责把请求发出去。
pub fn send_dm(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    my_mid: i64,
    talker_id: i64,
    text: &str,
    wbi_keys: Option<(String, String)>,
) -> Result<(), String> {
    crate::messages::validate_dm_text(text)?;
    if csrf.trim().is_empty() {
        return Err("登录会话缺少 bili_jct，无法发送私信".into());
    }
    if my_mid <= 0 || talker_id <= 0 {
        return Err("缺少发送者 / 接收者 mid，无法发送私信".into());
    }
    let (img, sub) = match wbi_keys {
        Some((img, sub)) if !img.is_empty() && !sub.is_empty() => (img, sub),
        _ => fetch_wbi_keys(client, cookie)?,
    };
    let now = now_wts();
    let signed = wbi::signed_url(
        &format!("{}/web_im/v1/web_im/send_msg", bilibili::MESSAGE_BASE),
        &[],
        &img,
        &sub,
        now,
    );
    // dev_id 只需稳定且像浏览器指纹；用 mid + 时间做 md5，避免额外依赖。
    let dev_id = {
        let digest = wbi::md5_hex(&format!("{my_mid}:{talker_id}:{now}"));
        format!(
            "{}-{}-{}-{}-{}",
            &digest[0..8],
            &digest[8..12],
            &digest[12..16],
            &digest[16..20],
            &digest[20..32]
        )
    };
    let content = serde_json::json!({ "content": text }).to_string();
    let form = vec![
        ("msg[sender_uid]", my_mid.to_string()),
        ("msg[receiver_id]", talker_id.to_string()),
        ("msg[receiver_type]", "1".to_string()),
        ("msg[msg_type]", "1".to_string()),
        ("msg[dev_id]", dev_id),
        ("msg[timestamp]", now.to_string()),
        ("msg[content]", content),
        ("csrf", csrf.to_string()),
        ("csrf_token", csrf.to_string()),
    ];
    let body = client.post_form(&signed, &form, cookie)?;
    check_message_code_value(&body, "发送私信")
}

fn check_message_code(code: i64, message: &str, action: &str) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    Err(format!("{action}失败：code={code} {}", message.trim()))
}

fn check_message_code_value(body: &str, action: &str) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("{action}响应解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(format!("{action}失败：code={code} {message}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn playurl_voucher_is_not_playable() {
        let voucher: PlayUrlData = serde_json::from_str(r#"{"v_voucher":"voucher_x"}"#).unwrap();
        assert!(voucher.v_voucher.is_some());
        assert!(!has_playable_stream(&voucher));

        let dash: PlayUrlData = serde_json::from_str(
            r#"{"dash":{"video":[{"id":32,"baseUrl":"https://x/v.m4s"}],"audio":[]}}"#,
        )
        .unwrap();
        assert!(has_playable_stream(&dash));

        let durl: PlayUrlData =
            serde_json::from_str(r#"{"durl":[{"url":"https://x/v.mp4"}]}"#).unwrap();
        assert!(has_playable_stream(&durl));
    }

    #[test]
    fn danmaku_body_decodes_raw_deflate_and_plain_text() {
        use std::io::Write;

        let xml = r#"<i><d p="1,1,25,16777215,0,0,a,1,10">hi</d></i>"#;
        // B 站实际返回的是无 zlib 头的 raw deflate。
        let mut encoder = flate2::write::DeflateEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        encoder.write_all(xml.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        assert_eq!(decode_danmaku_body(&compressed).unwrap(), xml);

        // CDN 回明文时也要能接住。
        assert_eq!(decode_danmaku_body(xml.as_bytes()).unwrap(), xml);
    }

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

    /// 评论分页假客户端：按 `next` 游标吐页，模拟线上 `ps` 被夹到 30。
    struct CommentClient {
        /// 该页实际返回的条数（服务端可能少给）。
        count: usize,
        /// 下一页游标；`is_end` 由它是否为 `None` 决定。
        next: Option<u64>,
        /// 故意在页内塞一条重复 `rpid`，验证按 rpid 去重。
        duplicate_first: bool,
        /// 服务端业务码；非 0 时直接回错误码。
        code: i64,
        calls: std::cell::RefCell<Vec<(u64, u32)>>,
    }

    impl HttpClient for CommentClient {
        type Error = String;
        fn get(&self, req: &Request) -> Result<String, String> {
            assert_eq!(req.name, "comment_main", "评论分页必须走 main 接口");
            let next: u64 = req
                .url
                .split("next=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .and_then(|v| v.parse().ok())
                .unwrap();
            let ps: u32 = req
                .url
                .split("ps=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .and_then(|v| v.parse().ok())
                .unwrap();
            self.calls.borrow_mut().push((next, ps));
            if self.code != 0 {
                return Ok(format!(r#"{{"code":{},"data":null}}"#, self.code));
            }
            let total = self.count.min(ps as usize);
            let mut rpids: Vec<i64> = (1..=total as i64).collect();
            if self.duplicate_first {
                rpids.push(1);
            }
            let replies: Vec<String> = rpids
                .iter()
                .map(|rpid| {
                    format!(
                        r#"{{"rpid":{rpid},"like":0,"ctime":1700000000,"member":{{"uname":"u{rpid}"}},"content":{{"message":"m{rpid}"}}}}"#
                    )
                })
                .collect();
            Ok(format!(
                r#"{{"code":0,"data":{{"replies":[{}],"cursor":{{"next":{},"is_end":{},"all_count":500}}}}}}"#,
                replies.join(","),
                self.next.unwrap_or(0),
                self.next.is_none()
            ))
        }
    }

    fn comment_target() -> CommentTarget {
        CommentTarget {
            oid: "123".into(),
            kind: 1,
        }
    }

    #[test]
    fn comment_page_is_one_request_of_twenty() {
        // 一次「加载更多」只发 1 个请求、只取 20 条 —— 这是躲开 -352 的前提。
        let client = CommentClient {
            count: 20,
            next: Some(2),
            duplicate_first: false,
            code: 0,
            calls: Default::default(),
        };
        let page = fetch_comment_page(&client, "", &comment_target(), 0, COMMENT_PAGE_SIZE).unwrap();
        assert_eq!(page.items.len(), 20);
        assert_eq!(page.next, 2, "游标原样回传，供下一次「滚到底」继续翻");
        assert!(!page.is_end, "服务端没说到底，就不能标成到底");
        assert_eq!(page.total, 500);
        assert_eq!(*client.calls.borrow(), vec![(0, 20)], "一批只能发一个请求");
    }

    #[test]
    fn comment_page_dedupes_repeated_rpid() {
        let client = CommentClient {
            count: 5,
            next: Some(3),
            duplicate_first: true,
            code: 0,
            calls: Default::default(),
        };
        let page = fetch_comment_page(&client, "", &comment_target(), 0, COMMENT_PAGE_SIZE).unwrap();
        assert_eq!(page.items.len(), 5, "页内重复 rpid 只保留一条");
    }

    #[test]
    fn comment_page_respects_server_end() {
        let client = CommentClient {
            count: 12,
            next: None,
            duplicate_first: false,
            code: 0,
            calls: Default::default(),
        };
        let page = fetch_comment_page(&client, "", &comment_target(), 4, COMMENT_PAGE_SIZE).unwrap();
        assert_eq!(page.items.len(), 12);
        assert!(page.is_end);
        assert_eq!(*client.calls.borrow(), vec![(4, 20)]);
    }

    #[test]
    fn comment_page_maps_risk_control_to_readable_error() {
        // -352 不能只把数字甩给界面：要告诉用户「等一会儿」。
        let client = CommentClient {
            count: 0,
            next: Some(1),
            duplicate_first: false,
            code: -352,
            calls: Default::default(),
        };
        let err = fetch_comment_page(&client, "", &comment_target(), 0, COMMENT_PAGE_SIZE).unwrap_err();
        assert!(err.contains("风控"), "错误文案应说明被风控：{err}");
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

    struct MsgClient;

    impl HttpClient for MsgClient {
        type Error = String;
        fn get(&self, req: &Request) -> Result<String, String> {
            match req.name {
                "msgfeed_unread" => Ok(r#"{"code":0,"data":{"reply":3,"at":1,"like":0,"recv_reply":4,"recv_like":3}}"#.into()),
                "msgfeed_inbox" => Ok(r#"{"code":0,"data":{"cursor":{"id":9,"time":100,"is_end":false},"last_view_at":90,"items":[
                    {"id":1,"user":{"mid":5,"nickname":"A","avatar":"http://x/a.jpg"},"item":{"type":"reply","business":"comment","title":"T","uri":"https://www.bilibili.com/video/BV1xx411c7mD","source_content":"hi"},"counts":{"reply":1},"reply_time":100,"last_view_at":90},
                    {"id":2,"user":{"mid":6,"nickname":"B","avatar":""},"item":{"type":"reply","business":"comment","title":"T","uri":"","source_content":"yo"},"counts":{"reply":1},"reply_time":95,"last_view_at":99}
                ]}}"#.into()),
                "message_sessions" => Ok(r#"{"code":0,"data":{"has_more":1,"session_list":[
                    {"talker_id":7,"session_type":1,"unread_count":2,"session_ts":1700000000123456,"is_follow":1,"last_msg":{"sender_uid":7,"msg_type":1,"content":"{\"content\":\"在吗\"}","msg_seqno":9,"timestamp":1700000000}}
                ]}}"#.into()),
                "user_cards" => Ok(r#"{"code":0,"data":{"7":{"mid":7,"name":"阿茶","face":"http://x/7.jpg"}}}"#.into()),
                "session_messages" => Ok(r#"{"code":0,"data":{"has_more":1,"min_seqno":3,"max_seqno":9,"messages":[
                    {"sender_uid":7,"msg_type":1,"content":"{\"content\":\"第二条\"}","msg_seqno":9,"timestamp":1700000100},
                    {"sender_uid":1,"msg_type":1,"content":"{\"content\":\"第一条\"}","msg_seqno":8,"timestamp":1700000000}
                ]}}"#.into()),
                other => Err(format!("未预期消息请求：{other}")),
            }
        }
    }

    #[test]
    fn message_unread_and_inbox_pipeline() {
        let counts = fetch_msgfeed_unread(&MsgClient, "SESSDATA=x").unwrap();
        assert_eq!((counts.reply, counts.mention, counts.like), (3, 1, 3));

        let (items, next) =
            fetch_msgfeed_inbox(&MsgClient, MessageChannel::Reply, None, "SESSDATA=x").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].user.nickname, "A");
        assert!(items[0].is_unread());
        assert!(!items[1].is_unread(), "reply_time <= last_view_at 是已读");
        assert_eq!(next, Some(InboxCursor { id: 9, time: 100, is_end: false }));
    }

    #[test]
    fn conversations_get_names_from_user_cards() {
        let (conversations, next) =
            fetch_conversations(&MsgClient, "SESSDATA=x", 1, None).unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].nickname, "阿茶");
        assert_eq!(conversations[0].preview(), "在吗");
        assert_eq!(conversations[0].unread, 2);
        assert_eq!(next, Some(1_700_000_000_123_456));
    }

    #[test]
    fn chat_messages_are_sorted_ascending() {
        let (messages, has_more, begin) =
            fetch_chat_messages(&MsgClient, "SESSDATA=x", 7, 1, None).unwrap();
        assert!(has_more);
        assert_eq!(begin, 3);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].seqno, 8);
        assert_eq!(messages[1].seqno, 9);
        assert_eq!(messages[0].text, "第一条");
        assert!(messages[0].mine);
        assert!(!messages[1].mine);
    }


    /// 真实响应的形状回归：`reply.counts` 是整数、`at_time`、赞是 latest/total、
    /// 用户名片是数组。这四点是线上抓包核对过的，别再被「同构假设」打回去。
    struct LiveMsgClient;

    impl HttpClient for LiveMsgClient {
        type Error = String;
        fn get(&self, req: &Request) -> Result<String, String> {
            if req.url.contains("/x/msgfeed/reply?") {
                Ok(r#"{"code":0,"data":{"cursor":{"is_end":false,"id":11,"time":22},"last_view_at":90,"items":[
                    {"id":1,"user":{"mid":5,"nickname":"A","avatar":"http://x/a.jpg"},"item":{"type":"reply","business":"评论","business_id":1,"subject_id":0,"item_id":99,"title":"T","uri":"https://www.bilibili.com/video/BV1xx411c7mD","source_content":"hi"},"counts":1,"is_multi":0,"reply_time":100}
                ]}}"#.into())
            } else if req.url.contains("/x/msgfeed/at?") {
                Ok(r#"{"code":0,"data":{"cursor":{"is_end":true,"id":1,"time":2},"items":[
                    {"id":2,"user":{"mid":6,"nickname":"B","avatar":""},"item":{"type":"reply","business":"评论","business_id":1,"title":"提到你","uri":"https://www.bilibili.com/video/BV1xx411c7mD","source_content":"@你 看这个"},"counts":{"reply":0},"at_time":50}
                ]}}"#.into())
            } else if req.url.contains("/x/msgfeed/like?") {
                Ok(r#"{"code":0,"data":{"latest":{"items":[],"last_view_at":90},"total":{"cursor":{"is_end":false,"id":7,"time":8},"items":[
                    {"id":3,"users":[{"mid":1,"nickname":"甲","avatar":""},{"mid":2,"nickname":"乙","avatar":""},{"mid":3,"nickname":"丙","avatar":""},{"mid":4,"nickname":"丁","avatar":""}],"item":{"item_id":77,"type":"reply","business":"评论","business_id":0,"title":"被赞的评论","uri":"https://www.bilibili.com/video/BV1xx411c7mD"},"counts":8,"like_time":70}
                ]}}}"#.into())
            } else if req.name == "message_sessions" {
                Ok(r#"{"code":0,"data":{"has_more":0,"session_list":[
                    {"talker_id":7,"session_type":1,"unread_count":1,"session_ts":1700000000123456,"is_follow":1,"last_msg":{"sender_uid":7,"msg_type":11,"content":"{\"title\":\"视频标题\"}","msg_seqno":9,"timestamp":1700000000}}
                ]}}"#.into())
            } else if req.name == "user_cards" {
                Ok(r#"{"code":0,"data":[{"mid":7,"name":"阿茶","face":"http://x/7.jpg"}]}"#.into())
            } else if req.name == "msgfeed_unread" {
                Ok(r#"{"code":0,"data":{"at":0,"chat":0,"like":0,"recv_like":0,"recv_reply":0,"reply":0}}"#.into())
            } else {
                Err(format!("未预期消息请求：{}", req.url))
            }
        }
    }

    #[test]
    fn live_message_shapes_parse() {
        let (reply, next) =
            fetch_msgfeed_inbox(&LiveMsgClient, MessageChannel::Reply, None, "SESSDATA=x")
                .unwrap();
        assert_eq!(reply.len(), 1);
        assert!(reply[0].is_unread());
        assert_eq!(next, Some(InboxCursor { id: 11, time: 22, is_end: false }));

        let (at, at_next) =
            fetch_msgfeed_inbox(&LiveMsgClient, MessageChannel::Mention, None, "SESSDATA=x")
                .unwrap();
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].time, 50, "at_time 要能映射成时间");
        assert!(at_next.is_none());

        let (like, like_next) =
            fetch_msgfeed_inbox(&LiveMsgClient, MessageChannel::Like, None, "SESSDATA=x")
                .unwrap();
        assert_eq!(like.len(), 1);
        assert_eq!(like[0].likers.len(), 4);
        assert_eq!(like[0].like_count, 8, "counts 是总人数，不是 users 长度");
        assert_eq!(like[0].like_actor_label(), "甲、乙 等 8 人");
        assert_eq!(like[0].target_title, "被赞的评论");
        assert_eq!(like_next, Some(InboxCursor { id: 7, time: 8, is_end: false }));

        let (conversations, _) =
            fetch_conversations(&LiveMsgClient, "SESSDATA=x", 1, None).unwrap();
        assert_eq!(conversations[0].nickname, "阿茶", "名片是数组也要能补昵称");
        assert_eq!(conversations[0].preview(), "[视频] 视频标题");
    }

}
