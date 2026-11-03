//! 视频互动：点赞 / 投币 / 收藏 / 转发到动态 / 发弹幕。
//!
//! 这一层把「互动」拆成两半：
//! - **读**（是否已点赞 / 已投币 / 已收藏、收藏夹列表）走 GET，
//!   用 [`crate::api::bilibili::HttpClient`] 抽象，测试里可以塞假客户端；
//! - **写**（点赞 / 投币 / 收藏 / 转发 / 发弹幕）走 POST + `bili_jct`，
//!   需要真实 HTTP 客户端，所以单独放在 [`api::client::ReqwestClient`] 上。
//!
//! 写操作都返回 `Result`：B 站的写接口成功时 `code == 0`，
//! 失败时 `code != 0` 且带一句人话（"硬币不足" / "请先登录"），
//! 这里把那句话原样透给界面。

use crate::api::bilibili::{HttpClient, Request};
use crate::api::client::{check_write_code, fetch_wbi_keys, ReqwestClient};
use crate::api::wbi;
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
///
/// 带上 `rid`（视频 aid）与 `type=2` 时，响应里每个夹子的 `fav_state`
/// 是「这个视频在不在这个夹里」（1 / 0）；不带的话永远是 0 ——
/// 面板上的勾选圈会全部空着，看起来像没收藏。
fn fav_folders_request(mid: i64, rid: i64, cookie: &str) -> Request {
    let mut url = format!(
        "{}/x/v3/fav/folder/created/list-all?up_mid={}",
        crate::api::bilibili::API_BASE,
        mid
    );
    if rid > 0 {
        url.push_str(&format!("&rid={rid}&type=2"));
    }
    Request {
        url,
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

/// 一次拿全部互动状态：`x/web-interface/archive/relation`。
///
/// 以前是三个接口串行（`has/like` → `archive/coins` → `favoured`），
/// 三个都在同一个限速桶里，等于每次开播先排三格队。
/// 这个接口把「赞 / 币 / 藏 / 关注」一起返回，一个请求搞定。
///
/// 返回 `(liked, coined, favoured, followed)`；`followed` 拿不到时为 `None`
/// （部分场景不返回 `attention`）。
pub fn fetch_archive_relation<C>(
    client: &C,
    cookie: &str,
    aid: i64,
    bvid: &str,
) -> Result<(bool, bool, bool, Option<bool>), String>
where
    C: HttpClient<Error = String>,
{
    let request = Request {
        url: format!(
            "{}/x/web-interface/archive/relation?{}",
            crate::api::bilibili::API_BASE,
            video_id_query(aid, bvid)
        ),
        cookie: cookie.to_string(),
        name: "archive_relation",
    };
    let body = client.get(&request)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("互动状态解析失败：{e}"))?;
    let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("读取互动状态失败：code={code} {message}"));
    }
    let data = parsed.get("data");
    // B 站这几个字段有时是 bool、有时是 0/1，统一按「真值」判断。
    let truthy = |key: &str| -> bool {
        data.and_then(|data| data.get(key))
            .map(|value| match value {
                serde_json::Value::Bool(flag) => *flag,
                serde_json::Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
                _ => false,
            })
            .unwrap_or(false)
    };
    let followed = data
        .and_then(|data| data.get("attention"))
        .map(|value| match value {
            serde_json::Value::Bool(flag) => *flag,
            serde_json::Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
            _ => false,
        });
    Ok((truthy("like"), truthy("coin"), truthy("favorite"), followed))
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
    let data = parsed.get("data");
    // 线上返回的是对象：`{"count":1,"favoured":true}`；
    // 老版本 / 某些兜底路径会直接给 bool，两种都接。
    let favoured = data
        .and_then(|data| data.get("favoured"))
        .and_then(|v| v.as_bool())
        .or_else(|| data.and_then(|v| v.as_bool()))
        .unwrap_or(false);
    Ok(favoured)
}

/// 拉取当前账号的收藏夹列表。`rid > 0` 时顺便带回「这个视频在不在该夹」。
pub fn fetch_fav_folders<C>(
    client: &C,
    cookie: &str,
    mid: i64,
    rid: i64,
) -> Result<Vec<FavFolder>, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&fav_folders_request(mid, rid, cookie))?;
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

/// `coin/add` 的表单字段（拆出来方便单测，不需要联网）。
///
/// **`select_like` 必须传 `1` / `0`，不是 `"true"` / `"false"`** ——
/// 服务端按整数解析这个字段，传布尔字符串会直接回 `code=-400`（请求错误），
/// 硬币一枚都投不出去。这是「投币一直 400」的根因（2026-11-01 实测对照：
/// `select_like=true` → -400，`select_like=1` → 进入业务校验回 -404）。
///
/// **`ramval` 必带**：网页版点投币时前端会顺手带一个随机值（实测 25 / 30
/// 都可以，说明服务端只把它当「来自页面」的信号）。不带这个字段，即使
/// `aid` / `multiply` / `select_like` / `csrf` 全对，风控也会直接回
/// `-401 非法访问`（`decisions: ["verify_captcha_level2"]`）或 `-403 账号异常`。
/// 用真实浏览器请求逐项二分出来的结论：最小表单 + `ramval` → 放行（回业务错误
/// 34005），最小表单不带 → -403。
///
/// `multiply` 是硬币数（1 / 2，上限 2）；`csrf_token` 是顺手多带的同一值，
/// 服务端会忽略多余字段。
pub(crate) fn coin_form(
    aid: i64,
    multiply: u8,
    also_like: bool,
    csrf: &str,
    ramval: u32,
) -> Vec<(&'static str, String)> {
    vec![
        ("aid", aid.to_string()),
        ("multiply", multiply.clamp(1, 2).to_string()),
        // 1 = 同时点赞，0 = 不点赞。
        ("select_like", if also_like { "1" } else { "0" }.to_string()),
        ("csrf", csrf.to_string()),
        ("csrf_token", csrf.to_string()),
        // 网页版同样会带；缺了会被风控当成脚本。
        ("ramval", ramval.to_string()),
    ]
}

/// 每次投币带一个新随机值：网页版也是随机的，写死一个常量反而像脚本。
pub(crate) fn next_ramval() -> u32 {
    use rand::Rng as _;
    rand::thread_rng().gen_range(1..=100)
}

/// 投币。
///
/// **必须传 `aid`（av 号），不能只传 `bvid`** —— B 站的 `coin/add` 只认 av 号：
/// 只带 bvid 会拿到 `code=-400`（请求错误），投币永远失败。
/// 浏览器发的组合是：`aid` / `multiply` / `select_like`（0 / 1）/ `csrf` /
/// `ramval`（见 [`coin_form`]）。
///
/// `bvid` 只用来拼 `Referer`（视频页）：浏览器是在视频页点的投币，
/// 固定发首页 Referer 会被风控当成脚本。
///
/// `multiply` 是硬币数（1 / 2）；`also_like` 为真时同时点赞。
pub fn coin_video(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    aid: i64,
    bvid: &str,
    multiply: u8,
    also_like: bool,
) -> Result<(), String> {
    require_csrf(csrf, "投币")?;
    if aid <= 0 {
        return Err("缺少视频 aid，无法投币".into());
    }
    let referer = if bvid.trim().is_empty() {
        "https://www.bilibili.com/".to_string()
    } else {
        format!("https://www.bilibili.com/video/{}/", bvid.trim())
    };
    let body = client.post_form_with_referer(
        &format!(
            "{}/x/web-interface/coin/add",
            crate::api::bilibili::API_BASE
        ),
        &coin_form(aid, multiply, also_like, csrf, next_ramval()),
        cookie,
        &referer,
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

/// `x/relation/modify` 的表单字段（单独拆出来，方便单测不联网校验）。
///
/// `act` 是 B 站的关注动作码：**1 = 关注，2 = 取关**（不是布尔值）。
/// `re_src = 11` 表示「视频播放页」来源，与浏览器一致，少这个字段容易被风控。
pub(crate) fn relation_modify_form(
    fid: i64,
    follow: bool,
    csrf: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("fid", fid.to_string()),
        ("act", if follow { "1" } else { "2" }.to_string()),
        ("re_src", "11".to_string()),
        ("csrf", csrf.to_string()),
        ("csrf_token", csrf.to_string()),
    ]
}

/// 关注 / 取关一个 UP 主。
///
/// 成功时服务端回 `code = 0`；`-101` 是未登录，`22014`/`-400` 这类带
/// 服务端人话的失败原样透给界面，换成「关注失败：…」的 toast。
pub fn follow_user(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    fid: i64,
    follow: bool,
) -> Result<(), String> {
    let action = if follow { "关注" } else { "取消关注" };
    require_csrf(csrf, action)?;
    if fid <= 0 {
        return Err("缺少用户 uid，无法关注".into());
    }
    let body = client.post_form(
        &crate::api::bilibili::relation_modify_url(),
        &relation_modify_form(fid, follow, csrf),
        cookie,
    )?;
    check_write_code(&body, action)
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

// ================= 发弹幕 =================

/// 弹幕内容的字符上限。B 站按**字符**（不是字节）计，超出回 `36702`。
pub const MAX_DANMAKU_CHARS: usize = 100;

/// 网页播放器发弹幕时带的 `web_location`（WBI 签名要把它算进去）。
const DANMAKU_WEB_LOCATION: &str = "1315873";

/// 发弹幕的表单字段（拆出来方便单测，不需要联网）。
///
/// 与网页播放器的请求对齐：
/// - `type=1` 视频弹幕；`oid` 是 **cid**，不是 aid；
/// - `progress` 是**毫秒**（`Danmaku::time` 是秒，调用方负责换算）；
/// - `mode=1` 滚动；`pool=0` 普通池；`fontsize=25` / `color=16777215` 是默认白字；
/// - `rnd` 是微秒时间戳：带上它发送冷却从 90s 降到 5s（文档口径）；
/// - `bvid` 与 `oid` 一起带，服务端爱用哪个用哪个（只带 cid 会被当无效稿件）。
pub(crate) fn danmaku_form(
    cid: i64,
    bvid: &str,
    text: &str,
    progress_ms: u64,
    csrf: &str,
    rnd: u64,
) -> Vec<(&'static str, String)> {
    let mut form: Vec<(&'static str, String)> = vec![
        ("type", "1".to_string()),
        ("oid", cid.to_string()),
        ("msg", text.to_string()),
        ("progress", progress_ms.to_string()),
        ("color", "16777215".to_string()),
        ("fontsize", "25".to_string()),
        ("pool", "0".to_string()),
        ("mode", "1".to_string()),
        ("rnd", rnd.to_string()),
        ("csrf", csrf.to_string()),
    ];
    let bvid = bvid.trim();
    if !bvid.is_empty() {
        form.push(("bvid", bvid.to_string()));
    }
    form
}

/// 发送一条视频弹幕（`/x/v2/dm/post`）。
///
/// 这是真正的写接口：**没有 `bili_jct` 直接失败，绝不本地回显假装成功** ——
/// 界面上出现一条弹幕，服务端就必须真的存了一条。
///
/// `progress_ms` 是弹幕出现位置（毫秒）。超出视频时长会被服务端判
/// `36714 弹幕发送时间不合法`，夹到时长内是调用方的事。
///
/// **WBI 签名**：文档标 `w_rid` 必要。2026-11 实测未签名也能成功，但签名是
/// 浏览器的真实行为，风控收紧时才有退路，所以默认签；`nav` 取不到 key 时
/// 降级成未签名请求 —— 不能让「发弹幕」被一个辅助接口拖死。
pub fn send_danmaku(
    client: &ReqwestClient,
    cookie: &str,
    csrf: &str,
    bvid: &str,
    cid: i64,
    text: &str,
    progress_ms: u64,
) -> Result<(), String> {
    validate_danmaku(csrf, cid, text)?;
    let text = text.trim();

    let base = format!("{}/x/v2/dm/post", crate::api::bilibili::API_BASE);
    let params = vec![(
        "web_location".to_string(),
        DANMAKU_WEB_LOCATION.to_string(),
    )];
    let url = match fetch_wbi_keys(client, cookie) {
        Ok((img, sub)) => wbi::signed_url(&base, &params, &img, &sub, now_secs()),
        Err(error) => {
            eprintln!("[danmaku] WBI key 获取失败，本次发送降级为未签名：{error}");
            format!("{base}?web_location={DANMAKU_WEB_LOCATION}")
        }
    };

    // 浏览器是从视频页发的；固定成首页 Referer 会被风控当成脚本（与投币同理）。
    let referer = if bvid.trim().is_empty() {
        "https://www.bilibili.com/".to_string()
    } else {
        format!("https://www.bilibili.com/video/{}/", bvid.trim())
    };
    let body = client.post_form_with_referer(
        &url,
        &danmaku_form(cid, bvid, text, progress_ms, csrf, now_micros()),
        cookie,
        &referer,
    )?;
    check_write_code(&body, "发送弹幕")
}

/// 发弹幕前的本地校验（不发请求，单测直接测它）。
fn validate_danmaku(csrf: &str, cid: i64, text: &str) -> Result<(), String> {
    require_csrf(csrf, "发送弹幕")?;
    if cid <= 0 {
        return Err("缺少视频 cid，无法发送弹幕".into());
    }
    let text = text.trim();
    if text.is_empty() {
        return Err("弹幕内容不能为空".into());
    }
    // 服务端按字符数（不是字节数）算长度：100 个汉字合法，300 字节不构成超长。
    let chars = text.chars().count();
    if chars > MAX_DANMAKU_CHARS {
        return Err(format!("弹幕最长 {MAX_DANMAKU_CHARS} 字，当前 {chars} 字"));
    }
    Ok(())
}

/// 秒级 Unix 时间戳（WBI 的 `wts`）。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 微秒级时间戳（弹幕的 `rnd`）。
fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
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
        let folders = fetch_fav_folders(&client, "", 42, 0).unwrap();
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
        assert!(fetch_fav_folders(&client, "", 42, 0).unwrap().is_empty());
    }

    #[test]
    fn fav_folders_carry_video_id_when_given() {
        // 播放器展开收藏面板时要顺带问「这个视频在不在夹里」，
        // 不带 rid 时每个夹子的 fav_state 都是 0（勾选圈全空）。
        let client = FakeClient::new(r#"{"code":0,"data":{"count":0,"list":[]}}"#);
        let _ = fetch_fav_folders(&client, "", 42, 113_753_081_383_484);
        let url = client.last.borrow().clone().unwrap().url;
        assert!(url.contains("up_mid=42"), "{url}");
        assert!(url.contains("rid=113753081383484"), "{url}");
        assert!(url.contains("type=2"), "{url}");

        // 收藏夹页只要夹子本身，不要多这两个参数。
        let client = FakeClient::new(r#"{"code":0,"data":{"count":0,"list":[]}}"#);
        let _ = fetch_fav_folders(&client, "", 42, 0);
        let url = client.last.borrow().clone().unwrap().url;
        assert!(!url.contains("rid="), "{url}");
    }

    #[test]
    fn favoured_reads_object_and_legacy_bool() {
        // 线上是 `data: {count, favoured}`；老版本直接给 bool。两种都要接住。
        let object = FakeClient::new(r#"{"code":0,"data":{"count":1,"favoured":true}}"#);
        assert!(fetch_favoured(&object, "SESSDATA=x", 123).unwrap());
        let object_no = FakeClient::new(r#"{"code":0,"data":{"count":0,"favoured":false}}"#);
        assert!(!fetch_favoured(&object_no, "SESSDATA=x", 123).unwrap());
        let legacy = FakeClient::new(r#"{"code":0,"data":true}"#);
        assert!(fetch_favoured(&legacy, "SESSDATA=x", 123).unwrap());

        let error = FakeClient::new(r#"{"code":-101,"message":"账号未登录"}"#);
        assert!(fetch_favoured(&error, "", 123)
            .unwrap_err()
            .contains("未登录"));
    }

    #[test]
    fn missing_csrf_is_rejected_before_request() {
        // 这几个写操作都要 bili_jct；没有它就直接失败，不发请求。
        let client = ReqwestClient::new().unwrap();
        assert!(like_video(&client, "", "", "BV1abc", true)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(coin_video(&client, "", "", 123, "BV1abc", 1, false)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(fav_video(&client, "", "", 1, 2, true)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(share_to_dynamic(&client, "", "", "hi")
            .unwrap_err()
            .contains("bili_jct"));
        assert!(send_danmaku(&client, "", "", "BV1abc", 1, "hi", 0)
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
        assert!(coin_video(&client, "", "csrf", 0, "BV1abc", 1, false)
            .unwrap_err()
            .contains("aid"));
    }

    fn form_value<'a>(form: &'a [(&str, String)], key: &str) -> Option<&'a str> {
        form.iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn coin_form_sends_integer_select_like() {
        // 回归：`select_like` 传 "true" 会被服务端判 -400（2026-11-01 实测：
        // true → -400 请求错误；1 → 通过参数校验进入业务错误 -404）。
        let with_like = coin_form(117_489_652_602_947, 2, true, "csrf-token", 25);
        assert_eq!(form_value(&with_like, "select_like"), Some("1"));
        assert_eq!(form_value(&with_like, "multiply"), Some("2"));
        assert_eq!(form_value(&with_like, "aid"), Some("117489652602947"));
        assert_eq!(form_value(&with_like, "csrf"), Some("csrf-token"));

        let without_like = coin_form(1, 1, false, "csrf-token", 25);
        assert_eq!(form_value(&without_like, "select_like"), Some("0"));
    }

    #[test]
    fn coin_form_carries_ramval() {
        // 回归：不带 `ramval` 时，参数全对也会被风控拦成 -401 / -403
        // （2026-11-01 用浏览器真实请求二分确认：最小表单 + ramval → 放行）。
        let form = coin_form(1, 1, false, "csrf", 42);
        assert_eq!(form_value(&form, "ramval"), Some("42"));

        // 生成器给的是 1..=100 的随机值，每次都带。
        for _ in 0..20 {
            let ramval = next_ramval();
            assert!((1..=100).contains(&ramval), "ramval 越界：{ramval}");
        }
    }

    #[test]
    fn coin_form_clamps_multiply_to_one_or_two() {
        // 服务端只接受 1 / 2：多传会被判参数错误，少传会 0 枚。
        assert_eq!(
            form_value(&coin_form(1, 0, false, "x", 1), "multiply"),
            Some("1")
        );
        assert_eq!(
            form_value(&coin_form(1, 5, false, "x", 1), "multiply"),
            Some("2")
        );
    }

    #[test]
    fn relation_form_uses_act_codes() {
        // B 站的 act 是动作码：1 = 关注，2 = 取关；别把 bool 直接塞进去。
        let follow = relation_modify_form(2333, true, "csrf-token");
        assert_eq!(
            follow
                .iter()
                .find(|(key, _)| *key == "act")
                .map(|(_, value)| value.as_str()),
            Some("1")
        );
        let unfollow = relation_modify_form(2333, false, "csrf-token");
        assert_eq!(
            unfollow
                .iter()
                .find(|(key, _)| *key == "act")
                .map(|(_, value)| value.as_str()),
            Some("2")
        );
        // 表单要带 fid / csrf / re_src：少一个都可能被服务端判 -400。
        for (key, value) in &follow {
            match *key {
                "fid" => assert_eq!(value, "2333"),
                "csrf" | "csrf_token" => assert_eq!(value, "csrf-token"),
                "re_src" => assert_eq!(value, "11"),
                _ => {}
            }
        }
    }

    #[test]
    fn follow_user_rejects_missing_csrf_and_uid() {
        let client = ReqwestClient::new().unwrap();
        assert!(follow_user(&client, "", "", 2333, true)
            .unwrap_err()
            .contains("bili_jct"));
        assert!(follow_user(&client, "", "csrf", 0, true)
            .unwrap_err()
            .contains("uid"));
    }

    #[test]
    fn danmaku_form_matches_web_player() {
        // 对照网页播放器的真实请求：cid 进 `oid`、progress 是毫秒、默认白字 25 号。
        let form = danmaku_form(934_721_738, "BV14P4y1i7mb", "测试", 1_500, "csrf-token", 42);
        assert_eq!(form_value(&form, "type"), Some("1"));
        assert_eq!(form_value(&form, "oid"), Some("934721738"));
        assert_eq!(form_value(&form, "msg"), Some("测试"));
        assert_eq!(form_value(&form, "progress"), Some("1500"));
        assert_eq!(form_value(&form, "mode"), Some("1"));
        assert_eq!(form_value(&form, "pool"), Some("0"));
        assert_eq!(form_value(&form, "fontsize"), Some("25"));
        assert_eq!(form_value(&form, "color"), Some("16777215"));
        assert_eq!(form_value(&form, "rnd"), Some("42"));
        assert_eq!(form_value(&form, "csrf"), Some("csrf-token"));
        assert_eq!(form_value(&form, "bvid"), Some("BV14P4y1i7mb"));

        // 没有 bvid（例如只拿得到 cid 的分 P）时不发空值。
        let form = danmaku_form(1, "  ", "x", 0, "csrf", 1);
        assert_eq!(form_value(&form, "bvid"), None);
    }

    #[test]
    fn send_danmaku_rejects_bad_input_before_request() {
        // 没有 bili_jct：必须先失败，绝不本地假装成功。
        assert!(validate_danmaku("", 1, "hi").unwrap_err().contains("bili_jct"));
        // 没有 cid：不知道发到哪个视频。
        assert!(validate_danmaku("csrf", 0, "hi").unwrap_err().contains("cid"));
        // 空白内容不发。
        assert!(validate_danmaku("csrf", 1, "   ")
            .unwrap_err()
            .contains("不能为空"));
        // 超过 100 字符：服务端会回 36702，提前拦掉。
        let long = "弹".repeat(MAX_DANMAKU_CHARS + 1);
        assert!(validate_danmaku("csrf", 1, &long).unwrap_err().contains("100"));
    }

    #[test]
    fn danmaku_length_counts_chars_not_bytes() {
        // 100 个汉字 = 300 字节，但服务端按字符算，不能误判超长。
        let text = "弹".repeat(MAX_DANMAKU_CHARS);
        assert!(validate_danmaku("csrf", 1, &text).is_ok());
        // 少于 100 也合法（含两侧空白，先 trim）。
        assert!(validate_danmaku("csrf", 1, " 你好 ").is_ok());
    }
}
