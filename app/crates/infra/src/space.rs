//! UP 主空间：资料画像与投稿列表。
//!
//! 对应旧实现的「用户空间」页。三个约定：
//!
//! 1. **只有 `acc/info` 是必需的**。昵称 / 头像 / 签名来自它，失败直接返回
//!    `Err`，没有兜底的必要 —— 空画像没有意义。
//! 2. 其余接口（标签 / 总播放 / 粉丝数 / 置顶）**全部可选**：未登录、风控、
//!    接口下线都只降级成空 / 0 / `None`，不能让整个画像失败。
//! 3. B 站返回的昵称、签名、标签、标题都带 HTML 转义，统一过
//!    [`format::html_decode`]；封面与头像统一过 [`search::image_url`]
//!    （`//i0.hdslb.com/...` 要补 `https:`）。
//!
//! 投稿列表用 `x/space/wbi/arc/search`（需 WBI 签名），一条 `vlist` 与置顶
//! 稿件结构同构，因此共用 [`parse_video`]。

use crate::api::bilibili::{self, HttpClient};
use crate::api::client::{fetch_wbi_keys, now_wts};
use crate::format::html_decode;
use crate::search::{image_url, parse_duration};
use serde_json::Value;

/// `acc/info` 的那一屏：打开浮窗先看到的东西。
///
/// 单独拆出来是因为它**只有一个请求**。`x/space` 全族共用一个限速桶
/// （400ms/次），把粉丝数、标签、置顶、投稿全串起来要 3 秒以上；
/// 分阶段落地才能「先看到人，再看到数」。
#[derive(Debug, Clone, Default)]
pub struct SpaceBasic {
    pub mid: i64,
    pub name: String,
    pub face: String,
    pub sign: String,
    pub top_photo: String,
    pub level: u32,
    /// 认证头衔；无认证时为空串。
    pub official_title: String,
    /// 例如「年度大会员」；非大会员为空串。
    pub vip_label: String,
    /// 注册时间（Unix 秒）。
    pub joined_at: u64,
}

/// UP 主空间画像。
#[derive(Debug, Clone, Default)]
pub struct SpaceProfile {
    pub mid: i64,
    pub name: String,
    pub face: String,
    pub sign: String,
    pub top_photo: String,
    pub level: u32,
    /// 认证头衔；无认证时为空串。
    pub official_title: String,
    /// 例如「年度大会员」；非大会员为空串。
    pub vip_label: String,
    pub fans: u64,
    pub follows: u64,
    /// 获赞数（`x/space/upstat` 的 `data.likes`）。
    pub likes: u64,
    /// 总播放（`x/space/upstat` 的 `data.archive.view`）。
    pub plays: u64,
    /// 空间标签，最多 5 条。
    pub tags: Vec<String>,
    /// 注册时间（Unix 秒）。
    pub joined_at: u64,
    /// 置顶视频；没有置顶或接口失败都是 `None`。
    pub pinned: Option<SpaceVideo>,
}

/// 空间页里的一个视频（投稿列表条目 / 置顶稿件共用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpaceVideo {
    pub bvid: String,
    pub title: String,
    pub cover: String,
    pub duration_secs: u64,
    pub views: u64,
    pub danmaku: u64,
    /// 发布时间（Unix 秒）。
    pub created: u64,
}

/// 投稿列表一页。
#[derive(Debug, Clone, Default)]
pub struct SpaceVideoPage {
    pub items: Vec<SpaceVideo>,
    /// 该 UP 主的投稿总数（`data.page.count`），不是本页条数。
    pub total: u64,
}

/// `x/web-interface/card` 的公开名片：一个请求就能画完整块头部。
///
/// 字段缺失或类型对不上（数字写成字符串、布尔写成 0/1）时一律给
/// 0 / 空串 / false，不因为某个计数缺失就让整块头部失败。
#[derive(Debug, Clone, Default)]
pub struct SpaceCard {
    pub mid: i64,
    pub name: String,
    pub face: String,
    pub sign: String,
    pub level: u32,
    pub official_title: String,
    pub vip_label: String,
    pub fans: u64,
    pub follows: u64,       // 该 UP 关注了多少人
    pub archive_count: u64, // 投稿数（Tab 上的数字）
    pub likes: u64,         // 获赞
    pub followed: bool,     // 当前登录账号是否已关注
}

/// 空间动态流里的一条。
#[derive(Debug, Clone, Default)]
pub struct SpaceDynamic {
    pub id: i64,
    pub pub_ts: u64,   // Unix 秒
    pub text: String,  // 动态正文纯文本（已 html_decode / 去标签）
    pub cover: String, // 附带的封面（视频/图文/专栏），没有就空
    pub title: String, // 附带的标题（视频稿 / 专栏标题），没有就空
    pub kind: String,  // "video" / "article" / "image" / "text" / "forward" / "live" / "other"
    pub likes: u64,
    pub comments: u64,
    pub forwards: u64,
    /// 附带视频的 bvid（非视频动态为空串）；点封面起播要用它。
    pub bvid: String,
    /// 图文动态的图片列表（最多 9 张，非图文动态为空）。
    pub images: Vec<String>,
    /// 附带视频的播放量（非视频动态为 0）。
    pub views: u64,
    /// 附带视频的弹幕数（非视频动态为 0）。
    pub danmaku: u64,
    /// 附带视频的时长秒数（解析 `duration_text`，非视频动态为 0）。
    pub duration_secs: u64,
}

/// 空间专栏列表里的一篇。
#[derive(Debug, Clone, Default)]
pub struct SpaceArticle {
    pub id: i64,
    pub title: String,
    pub summary: String,
    pub cover: String,
    pub views: u64,
    pub comments: u64,
    /// 发布时间（Unix 秒）。
    pub published_at: u64,
}

/// 投稿列表排序。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceOrder {
    Newest,
    MostPlayed,
    MostFavorited,
}

impl SpaceOrder {
    /// 接口 `order` 参数的取值。
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceOrder::Newest => "pubdate",
            SpaceOrder::MostPlayed => "click",
            SpaceOrder::MostFavorited => "stow",
        }
    }
}

/// 取 UP 主空间画像。
///
/// `acc/info` 失败直接返回 `Err`；标签、总播放/获赞、粉丝数、置顶稿件
/// 任一失败都静默降级。
pub fn fetch_space_profile<C>(client: &C, cookie: &str, mid: i64) -> Result<SpaceProfile, String>
where
    C: HttpClient<Error = String>,
{
    let basic = fetch_space_basic(client, cookie, mid)?;
    let (fans, follows, likes, plays) = fetch_space_stats(client, cookie, basic.mid);
    let (tags, pinned) = fetch_space_extras(client, cookie, basic.mid);
    Ok(SpaceProfile {
        mid: basic.mid,
        name: basic.name,
        face: basic.face,
        sign: basic.sign,
        top_photo: basic.top_photo,
        level: basic.level,
        official_title: basic.official_title,
        vip_label: basic.vip_label,
        joined_at: basic.joined_at,
        fans,
        follows,
        likes,
        plays,
        tags,
        pinned,
    })
}

/// 只取 `acc/info`：一个请求，够画头部（昵称 / 头像 / 签名 / 头图 / 等级 / 认证 / 注册时间）。
pub fn fetch_space_basic<C>(client: &C, cookie: &str, mid: i64) -> Result<SpaceBasic, String>
where
    C: HttpClient<Error = String>,
{
    if mid <= 0 {
        return Err("缺少 UP 主 uid，无法读取空间资料".into());
    }
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    let req = bilibili::space_acc_info(mid, &img, &sub, cookie, now_wts());
    let body = client.get(&req)?;
    let root: Value =
        serde_json::from_str(&body).map_err(|e| format!("空间资料响应解析失败：{e}"))?;
    // wbi 端点少一个反爬参数就整体 -352（连 jointime / top_photo 一起没有），
    // 失败时回落到不带 wbi 的老端点——实测多数账号仍可用。
    let root = match ensure_ok(&root, "空间资料") {
        Ok(()) => root,
        Err(error) => {
            eprintln!("[space] {error}；回落 /x/space/acc/info");
            let legacy = client.get(&bilibili::space_acc_info_legacy(mid, cookie))?;
            let fallback: Value = serde_json::from_str(&legacy)
                .map_err(|e| format!("空间资料(旧接口)响应解析失败：{e}"))?;
            ensure_ok(&fallback, "空间资料(旧接口)")?;
            fallback
        }
    };
    let data = data_of(&root).ok_or_else(|| "空间资料响应缺少 data".to_string())?;
    let actual_mid = {
        let parsed = num(data, "mid");
        if parsed > 0 {
            parsed as i64
        } else {
            mid
        }
    };
    Ok(SpaceBasic {
        mid: actual_mid,
        name: decode(str_of(data, "name")),
        face: image_url(&str_of(data, "face")),
        sign: decode(str_of(data, "sign")),
        top_photo: image_url(&str_of(data, "top_photo")),
        level: num(data, "level").min(u32::MAX as u64) as u32,
        official_title: data
            .get("official")
            .map(|official| decode(str_of(official, "title")))
            .unwrap_or_default(),
        // 非大会员时 `vip.label` 是 null，取不到就是空串。
        vip_label: data
            .get("vip")
            .and_then(|vip| vip.get("label"))
            .map(|label| decode(str_of(label, "text")))
            .unwrap_or_default(),
        joined_at: num(data, "jointime"),
    })
}

/// 关注数 / 粉丝数 / 获赞 / 总播放；两个接口，失败一律降级成 0。
pub fn fetch_space_stats<C>(client: &C, cookie: &str, mid: i64) -> (u64, u64, u64, u64)
where
    C: HttpClient<Error = String>,
{
    let (fans, follows) = fetch_relation(client, cookie, mid);
    let (plays, likes) = fetch_upstat(client, cookie, mid);
    (fans, follows, likes, plays)
}

/// 总播放 / 获赞（`x/space/upstat`）。
///
/// **这是唯一能给「播放」的接口**：`card` 只带获赞，`acc/info` 只带注册时间/头图。
/// 未登录时服务端不返回 `archive`，这里给 (0, 0)，由界面决定不显示。
pub fn fetch_space_upstat<C>(client: &C, cookie: &str, mid: i64) -> (u64, u64)
where
    C: HttpClient<Error = String>,
{
    fetch_upstat(client, cookie, mid)
}

/// 标签与置顶；都失败时返回空。
pub fn fetch_space_extras<C>(
    client: &C,
    cookie: &str,
    mid: i64,
) -> (Vec<String>, Option<SpaceVideo>)
where
    C: HttpClient<Error = String>,
{
    (
        fetch_tags(client, cookie, mid),
        fetch_pinned(client, cookie, mid),
    )
}

/// 取 UP 主投稿列表的一页。
///
/// `pn` 从 1 开始，`ps` 是每页条数；`order` 决定排序。
pub fn fetch_space_videos<C>(
    client: &C,
    cookie: &str,
    mid: i64,
    pn: u32,
    ps: u32,
    order: SpaceOrder,
) -> Result<SpaceVideoPage, String>
where
    C: HttpClient<Error = String>,
{
    if mid <= 0 {
        return Err("缺少 UP 主 uid，无法读取投稿".into());
    }
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    let req = bilibili::space_arc_search(
        mid,
        pn.max(1),
        ps.max(1),
        order.as_str(),
        &img,
        &sub,
        cookie,
        now_wts(),
    );
    let body = client.get(&req)?;
    let root: Value =
        serde_json::from_str(&body).map_err(|e| format!("空间投稿响应解析失败：{e}"))?;
    ensure_ok(&root, "空间投稿")?;
    // 新号没有投稿时 `data` 可能整块是 null，这不算错误。
    let Some(data) = data_of(&root) else {
        return Ok(SpaceVideoPage::default());
    };
    let items = data
        .get("list")
        .and_then(|list| list.get("vlist"))
        .and_then(Value::as_array)
        .map(|list| list.iter().map(parse_video).collect())
        .unwrap_or_default();
    let total = data
        .get("page")
        .map(|page| num(page, "count"))
        .unwrap_or_default();
    Ok(SpaceVideoPage { items, total })
}

/// 取 UP 主的公开名片（`x/web-interface/card`）。
///
/// 一个请求就够画完整块头部，不需要 WBI 签名。字段缺失或类型对不上时
/// 按 0 / 空串 / false 处理；只有 `data` 整块缺失才算失败。
pub fn fetch_space_card<C>(client: &C, cookie: &str, mid: i64) -> Result<SpaceCard, String>
where
    C: HttpClient<Error = String>,
{
    if mid <= 0 {
        return Err("缺少 UP 主 uid，无法读取名片".into());
    }
    let body = client.get(&bilibili::space_card(mid, cookie))?;
    let root: Value =
        serde_json::from_str(&body).map_err(|e| format!("空间名片响应解析失败：{e}"))?;
    ensure_ok(&root, "空间名片")?;
    let data = data_of(&root).ok_or_else(|| "空间名片响应缺少 data".to_string())?;
    // 名片字段全在 `data.card` 下；整块缺失时退化成空对象，仍返回请求的 mid。
    let card = data
        .get("card")
        .filter(|card| !card.is_null())
        .unwrap_or(data);
    let actual_mid = {
        let parsed = num(card, "mid");
        if parsed > 0 {
            parsed as i64
        } else {
            mid
        }
    };
    // `level_info.current_level` 才是当前等级；老响应只给 `level`。
    let level = card
        .get("level_info")
        .map(|info| num(info, "current_level"))
        .unwrap_or(0);
    let level = if level > 0 { level } else { num(card, "level") };
    // `official_verify.type > 0` 才算认证；标题优先用 `desc`，没有再退回 `title`。
    let official_title = card
        .get("official_verify")
        .filter(|verify| !verify.is_null())
        .filter(|verify| num(verify, "type") > 0)
        .map(|verify| {
            let desc = decode(str_of(verify, "desc"));
            if desc.is_empty() {
                decode(str_of(verify, "title"))
            } else {
                desc
            }
        })
        .unwrap_or_default();
    Ok(SpaceCard {
        mid: actual_mid,
        name: decode(str_of(card, "name")),
        face: image_url(&str_of(card, "face")),
        sign: decode(str_of(card, "sign")),
        level: level.min(u32::MAX as u64) as u32,
        official_title,
        vip_label: card
            .get("vip")
            .and_then(|vip| vip.get("label"))
            .map(|label| decode(str_of(label, "text")))
            .unwrap_or_default(),
        // `data.follower` 是名片接口的粉丝数；缺了才退回 `card.fans`。
        fans: match data.get("follower") {
            Some(value) if !value.is_null() => num(data, "follower"),
            _ => num(card, "fans"),
        },
        follows: num(card, "attention"),
        archive_count: num(data, "archive_count"),
        likes: num(data, "like_num"),
        followed: flag(data, "following"),
    })
}

/// 取 UP 主空间动态流的一页。
///
/// `offset` 传空串表示第一页；返回 `(列表, 下一页 offset, 是否还有)`。
/// 该接口不需要 WBI 签名，`data` 整块缺失（空空间）按空页处理。
pub fn fetch_space_dynamics<C>(
    client: &C,
    cookie: &str,
    mid: i64,
    offset: &str,
) -> Result<(Vec<SpaceDynamic>, String, bool), String>
where
    C: HttpClient<Error = String>,
{
    if mid <= 0 {
        return Err("缺少 UP 主 uid，无法读取动态".into());
    }
    let body = client.get(&bilibili::space_dynamics(mid, offset, cookie))?;
    let root: Value =
        serde_json::from_str(&body).map_err(|e| format!("空间动态响应解析失败：{e}"))?;
    ensure_ok(&root, "空间动态")?;
    let Some(data) = data_of(&root) else {
        return Ok((Vec::new(), String::new(), false));
    };
    let items = data
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_dynamic).collect())
        .unwrap_or_default();
    Ok((items, str_of(data, "offset"), flag(data, "has_more")))
}

/// 取 UP 主专栏列表的一页（`pn` 从 1 开始）。
///
/// 该接口需要 WBI 签名；返回 `(列表, 总数)`，总数缺失时用本页条数。
pub fn fetch_space_articles<C>(
    client: &C,
    cookie: &str,
    mid: i64,
    pn: u32,
    ps: u32,
) -> Result<(Vec<SpaceArticle>, u64), String>
where
    C: HttpClient<Error = String>,
{
    if mid <= 0 {
        return Err("缺少 UP 主 uid，无法读取专栏".into());
    }
    let (img, sub) = fetch_wbi_keys(client, cookie)?;
    let req = bilibili::space_articles(mid, pn.max(1), ps.max(1), &img, &sub, cookie, now_wts());
    let body = client.get(&req)?;
    let root: Value =
        serde_json::from_str(&body).map_err(|e| format!("空间专栏响应解析失败：{e}"))?;
    ensure_ok(&root, "空间专栏")?;
    let Some(data) = data_of(&root) else {
        return Ok((Vec::new(), 0));
    };
    let items: Vec<SpaceArticle> = data
        .get("articles")
        .and_then(Value::as_array)
        .map(|articles| articles.iter().map(parse_article).collect())
        .unwrap_or_default();
    // `count` 缺失（新号 / 风控降级）时退回本页条数。
    let total = match data.get("count") {
        Some(value) if !value.is_null() => num(data, "count"),
        _ => items.len() as u64,
    };
    Ok((items, total))
}

// ================= 可选接口（失败一律降级） =================

/// 空间标签；失败返回空列表。
fn fetch_tags<C>(client: &C, cookie: &str, mid: i64) -> Vec<String>
where
    C: HttpClient<Error = String>,
{
    let Ok(body) = client.get(&bilibili::space_tags(mid, cookie)) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&body) else {
        return Vec::new();
    };
    if code_of(&root) != 0 {
        return Vec::new();
    }
    root.get("data")
        .and_then(|data| data.get("tags"))
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(|tag| decode(tag.trim().to_string()))
                .filter(|tag| !tag.is_empty())
                .take(5)
                .collect()
        })
        .unwrap_or_default()
}

/// 总播放 / 获赞；未登录时 `data` 整块缺失，按 0 处理。
fn fetch_upstat<C>(client: &C, cookie: &str, mid: i64) -> (u64, u64)
where
    C: HttpClient<Error = String>,
{
    let Ok(body) = client.get(&bilibili::space_upstat(mid, cookie)) else {
        return (0, 0);
    };
    let Ok(root) = serde_json::from_str::<Value>(&body) else {
        return (0, 0);
    };
    let code = code_of(&root);
    if code != 0 {
        // 别静默降级：-101 是没登录、-352 是风控，两者处理方式完全不同。
        eprintln!(
            "[space] upstat 失败 code={code} message={}，cookie {} 字节",
            str_of(&root, "message"),
            cookie.len()
        );
        return (0, 0);
    }
    let Some(data) = data_of(&root) else {
        eprintln!("[space] upstat 没有 data，cookie {} 字节", cookie.len());
        return (0, 0);
    };
    let plays = data
        .get("archive")
        .map(|archive| num(archive, "view"))
        .unwrap_or(0);
    if plays == 0 {
        let keys: Vec<&str> = data
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        eprintln!(
            "[space] upstat 播放为 0（cookie {} 字节，data 字段 {keys:?}）",
            cookie.len()
        );
    }
    (plays, num(data, "likes"))
}

/// 粉丝数 / 关注数。复用公开的 `/x/relation/stat`，但在这里自己解析。
fn fetch_relation<C>(client: &C, cookie: &str, mid: i64) -> (u64, u64)
where
    C: HttpClient<Error = String>,
{
    let Ok(body) = client.get(&bilibili::relation_stat(mid, cookie)) else {
        return (0, 0);
    };
    let Ok(root) = serde_json::from_str::<Value>(&body) else {
        return (0, 0);
    };
    if code_of(&root) != 0 {
        return (0, 0);
    }
    let Some(data) = data_of(&root) else {
        return (0, 0);
    };
    (num(data, "follower"), num(data, "following"))
}

/// 置顶稿件；`code=53016`（没有置顶）与任何失败都返回 `None`。
fn fetch_pinned<C>(client: &C, cookie: &str, mid: i64) -> Option<SpaceVideo>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&bilibili::space_top_arc(mid, cookie)).ok()?;
    let root = serde_json::from_str::<Value>(&body).ok()?;
    if code_of(&root) != 0 {
        return None;
    }
    let data = data_of(&root)?;
    let video = parse_video(data);
    // `data` 是空对象时同样当作没有置顶。
    if video.bvid.is_empty() {
        None
    } else {
        Some(video)
    }
}

// ================= 解析与取值 =================

/// 一条 `vlist` 条目 / 置顶稿件的公共形状。
fn parse_video(item: &Value) -> SpaceVideo {
    SpaceVideo {
        bvid: str_of(item, "bvid"),
        title: decode(str_of(item, "title")),
        cover: image_url(&str_of(item, "pic")),
        // `length` 是 "MM:SS" 或 "HH:MM:SS"，不是秒数。
        duration_secs: parse_duration(&str_of(item, "length")),
        views: num(item, "play"),
        danmaku: num(item, "video_review"),
        created: num(item, "created"),
    }
}

/// 一条空间动态（`data.items[]`）。
///
/// 正文、标题、封面都藏在 `modules` 里，且图文 (Opus) 与旧版结构并存：
/// 正文优先 `module_dynamic.desc.text`，没有再看 `major.opus.summary.text`。
fn parse_dynamic(item: &Value) -> SpaceDynamic {
    let modules = item.get("modules");
    let dynamic = modules.and_then(|modules| modules.get("module_dynamic"));
    let major = dynamic.and_then(|dynamic| dynamic.get("major"));
    let kind = dynamic_kind(item);
    let text = dynamic
        .and_then(|dynamic| dynamic.get("desc"))
        .map(|desc| plain_text(&str_of(desc, "text")))
        .filter(|text| !text.is_empty())
        .or_else(|| {
            major
                .and_then(|major| major.get("opus"))
                .and_then(|opus| opus.get("summary"))
                .map(|summary| plain_text(&str_of(summary, "text")))
                .filter(|text| !text.is_empty())
        })
        .unwrap_or_default();
    let bvid = if kind == "video" {
        major
            .and_then(|major| major.get("archive"))
            .map(|archive| str_of(archive, "bvid"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let (title, cover) = match kind {
        "video" => major
            .and_then(|major| major.get("archive"))
            .map(|archive| {
                (
                    decode(str_of(archive, "title")),
                    image_url(&str_of(archive, "cover")),
                )
            })
            .unwrap_or_default(),
        "article" => major
            .and_then(|major| major.get("article"))
            .map(|article| {
                let cover = article
                    .get("covers")
                    .and_then(Value::as_array)
                    .and_then(|covers| covers.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                (decode(str_of(article, "title")), image_url(cover))
            })
            .unwrap_or_default(),
        "image" => {
            let cover = major
                .and_then(|major| major.get("draw"))
                .and_then(|draw| draw.get("items"))
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("src"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            (String::new(), image_url(cover))
        }
        _ => (String::new(), String::new()),
    };
    // 视频动态的播放 / 弹幕 / 时长：缺字段或类型不对一律给 0。
    let (views, danmaku, duration_secs) = major
        .and_then(|major| major.get("archive"))
        .map(|archive| {
            let stat = archive.get("stat");
            (
                stat.map(|stat| num(stat, "play")).unwrap_or(0),
                stat.map(|stat| num(stat, "danmaku")).unwrap_or(0),
                archive
                    .get("duration_text")
                    .and_then(Value::as_str)
                    .map(parse_duration)
                    .unwrap_or(0),
            )
        })
        .unwrap_or_default();
    // 图片有两种来源：老图文的 `draw.items[].src`，新图文（OPUS）的
    // `opus.pics[].url`。两个都收——只认 draw 的话，新版图文会一条图都显示不出来。
    let mut images: Vec<String> = Vec::new();
    if let Some(items) = major
        .and_then(|major| major.get("draw"))
        .and_then(|draw| draw.get("items"))
        .and_then(Value::as_array)
    {
        images.extend(
            items
                .iter()
                .filter_map(|item| item.get("src").and_then(Value::as_str))
                .map(image_url),
        );
    }
    if let Some(pics) = major
        .and_then(|major| major.get("opus"))
        .and_then(|opus| opus.get("pics"))
        .and_then(Value::as_array)
    {
        images.extend(pics.iter().filter_map(|pic| {
            pic.get("url")
                .or_else(|| pic.get("src"))
                .and_then(Value::as_str)
                .map(image_url)
        }));
    }
    images.retain(|url| !url.is_empty());
    images.truncate(9);
    // 图文动态的封面就是第一张图；老逻辑只从 draw 取，OPUS 会漏。
    let mut cover = cover;
    if cover.is_empty() {
        cover = images.first().cloned().unwrap_or_default();
    }
    SpaceDynamic {
        id: str_of(item, "id_str")
            .parse::<i64>()
            .unwrap_or_else(|_| num(item, "id") as i64),
        pub_ts: modules
            .and_then(|modules| modules.get("module_author"))
            .map(|author| num(author, "pub_ts"))
            .unwrap_or(0),
        text,
        cover,
        title,
        kind: kind.to_string(),
        likes: dynamic_stat(modules, "like"),
        comments: dynamic_stat(modules, "comment"),
        forwards: dynamic_stat(modules, "forward"),
        bvid,
        images,
        views,
        danmaku,
        duration_secs,
    }
}

/// `modules.module_stat.<key>.count`。
fn dynamic_stat(modules: Option<&Value>, key: &str) -> u64 {
    modules
        .and_then(|modules| modules.get("module_stat"))
        .and_then(|stat| stat.get(key))
        .map(|count| num(count, "count"))
        .unwrap_or(0)
}

/// `type` 字段 → 卡片上的 kind。
fn dynamic_kind(item: &Value) -> &'static str {
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "DYNAMIC_TYPE_AV" => "video",
        "DYNAMIC_TYPE_ARTICLE" => "article",
        "DYNAMIC_TYPE_DRAW" => "image",
        // 新版图文：媒体在 `major.opus.pics[]`，不是 `major.draw.items[]`。
        "DYNAMIC_TYPE_OPUS" => "image",
        "DYNAMIC_TYPE_FORWARD" => "forward",
        "DYNAMIC_TYPE_LIVE_RCMD" => "live",
        "DYNAMIC_TYPE_WORD" => "text",
        _ => "other",
    }
}

/// 一篇专栏（`data.articles[]`）。
fn parse_article(item: &Value) -> SpaceArticle {
    let cover = item
        .get("image_urls")
        .and_then(Value::as_array)
        .and_then(|urls| urls.first())
        .and_then(Value::as_str)
        .unwrap_or_default();
    SpaceArticle {
        id: num(item, "id") as i64,
        title: decode(str_of(item, "title")),
        summary: decode(str_of(item, "desc")),
        cover: image_url(cover),
        views: num(item, "view"),
        comments: item
            .get("stats")
            .map(|stats| num(stats, "comment"))
            .unwrap_or(0),
        published_at: num(item, "publish_time"),
    }
}

/// 动态正文 → 纯文本：先丢 HTML 标签（表情图 / 链接 / 换行），再解实体。
///
/// 只把 `<` 后面紧跟字母、`/`、`!` 的片段当标签，正文里的「a < b」不会被误吞。
fn plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            match chars.peek() {
                Some(next) if next.is_ascii_alphanumeric() || *next == '/' || *next == '!' => {
                    for skipped in chars.by_ref() {
                        if skipped == '>' {
                            break;
                        }
                    }
                }
                _ => out.push(ch),
            }
        } else {
            out.push(ch);
        }
    }
    decode(out)
}

fn decode(text: String) -> String {
    html_decode(&text)
}

fn code_of(root: &Value) -> i64 {
    root.get("code").and_then(Value::as_i64).unwrap_or(-1)
}

/// 必需接口的返回码检查。
fn ensure_ok(root: &Value, action: &str) -> Result<(), String> {
    let code = code_of(root);
    if code == 0 {
        return Ok(());
    }
    let message = str_of(root, "message");
    if message.is_empty() {
        Err(format!("{action}接口返回 code={code}"))
    } else {
        Err(format!("{action}接口返回 code={code}（{message}）"))
    }
}

fn data_of(root: &Value) -> Option<&Value> {
    root.get("data").filter(|data| !data.is_null())
}

fn str_of(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string().trim().to_string(),
    }
}

/// 数字字段：B 站有时把计数写成字符串，也偶尔给负数（视为 0）。
fn num(value: &Value, key: &str) -> u64 {
    match value.get(key) {
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| number.as_i64().map(|n| n.max(0) as u64))
            .unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

/// 布尔字段：线上给 `true`，部分降级路径给 `0/1` 或 `"1"`，三种都接。
fn flag(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().map(|n| n != 0).unwrap_or(false),
        Some(Value::String(text)) => matches!(text.trim(), "true" | "1"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::bilibili::Request;

    const NAV: &str = r#"{"code":0,"data":{"isLogin":false,"mid":0,"uname":"","face":"",
        "wbi_img":{"img_url":"https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png",
                   "sub_url":"https://i0.hdslb.com/bfs/wbi/4932caff0ff746eab6f01bf08b70ac45.png"}}}"#;

    const PROFILE: &str = r#"{
      "code": 0,
      "message": "0",
      "data": {
        "mid": 25876945,
        "name": "极客湾 &amp; 朋友",
        "face": "//i1.hdslb.com/bfs/face/a.jpg",
        "sign": "聊点硬的 &lt;数码&gt;",
        "top_photo": "http://i0.hdslb.com/bfs/top/b.jpg",
        "level": 6,
        "jointime": 1388419200,
        "official": {"type": 1, "title": "2023百大UP主"},
        "vip": {"type": 2, "status": 1, "label": {"text": "年度大会员"}}
      }
    }"#;

    const TAGS: &str = r#"{"code":0,"data":{"tags":["数码","评测 &amp; 实验","芯片","手机","音频","第六条"]}}"#;
    const UPSTAT: &str = r#"{"code":0,"data":{"archive":{"view":123456789},"likes":4567890}}"#;
    const RELATION: &str = r#"{"code":0,"data":{"mid":25876945,"follower":1200000,"following":42}}"#;
    const TOP: &str = r#"{"code":0,"data":{"bvid":"BV1top","title":"置顶 &amp; 视频",
        "pic":"//i0.hdslb.com/bfs/archive/t.jpg","length":"1:02:03",
        "play":900000,"video_review":1200,"created":1793000000,"reason":"置顶"}}"#;
    const VIDEOS: &str = r#"{"code":0,"data":{
        "list":{"vlist":[
          {"bvid":"BV1a","title":"旗舰 &amp; 横评","pic":"//i0.hdslb.com/bfs/archive/x.jpg",
           "length":"12:34","play":2436575,"video_review":40763,"created":1793087129},
          {"bvid":"BV1b","title":"第二条","pic":"https://i0.hdslb.com/bfs/archive/y.jpg",
           "length":"00:59","play":12,"video_review":3,"created":1793000000}
        ]},
        "page":{"pn":1,"ps":30,"count":187}
      }}"#;

    const CARD: &str = r#"{"code":0,"data":{
        "card":{
          "mid":25876945,"name":"极客湾 &amp; 朋友","face":"//i1.hdslb.com/bfs/face/a.jpg",
          "sign":"聊点硬的 &lt;数码&gt;","level_info":{"current_level":6},
          "official_verify":{"type":1,"desc":"2023百大UP主"},
          "vip":{"status":1,"label":{"text":"年度大会员"}},
          "fans":1200000,"attention":42
        },
        "follower":1200001,"following":true,
        "archive_count":187,"like_num":4567890
      }}"#;

    const DYNAMICS: &str = r#"{"code":0,"data":{
        "items":[
          {"id_str":"1001","type":"DYNAMIC_TYPE_AV","modules":{
            "module_author":{"pub_ts":1793000000},
            "module_dynamic":{
              "desc":{"text":"旗舰 &amp; 横评 <a href=\"//t.bilibili.com\">话题</a>"},
              "major":{"archive":{"title":"旗舰 &amp; 横评","cover":"//i0.hdslb.com/bfs/archive/x.jpg"}}},
            "module_stat":{"like":{"count":12},"comment":{"count":3},"forward":{"count":1}}}},
          {"id_str":"1002","type":"DYNAMIC_TYPE_ARTICLE","modules":{
            "module_author":{"pub_ts":1792900000},
            "module_dynamic":{"desc":null,
              "major":{"article":{"title":"专栏 &amp; 标题","covers":["http://i0.hdslb.com/bfs/article/c.jpg"]}}},
            "module_stat":{"like":{"count":"5"},"comment":{"count":2},"forward":{"count":0}}}},
          {"id_str":"1003","type":"DYNAMIC_TYPE_DRAW","modules":{
            "module_author":{"pub_ts":1792800000},
            "module_dynamic":{"desc":{"text":"看图 &gt;&gt;"},
              "major":{"draw":{"items":[{"src":"//i0.hdslb.com/bfs/new_dyn/1.jpg"}]}}},
            "module_stat":{"like":{"count":7},"comment":{"count":0},"forward":{"count":0}}}},
          {"id_str":"1004","type":"DYNAMIC_TYPE_FORWARD","modules":{
            "module_author":{"pub_ts":1792700000},
            "module_stat":{"like":{"count":1},"comment":{"count":0},"forward":{"count":0}}}}
        ],
        "offset":"next-page","has_more":true
      }}"#;

    const ARTICLES: &str = r#"{"code":0,"data":{
        "articles":[
          {"id":123,"title":"第一 &amp; 篇","desc":"摘要 &lt;一&gt;",
           "image_urls":["//i0.hdslb.com/bfs/article/a.jpg"],
           "view":1234,"stats":{"comment":56},"publish_time":1793000000},
          {"id":"124","title":"第二篇","desc":"摘要二","image_urls":[],
           "view":"7","stats":{},"publish_time":"1792900000"}
        ],
        "count":2
      }}"#;

    /// 不联网的假客户端：`Err` 模拟该接口请求失败。
    struct FakeClient {
        profile: Result<String, String>,
        videos: Result<String, String>,
        tags: Result<String, String>,
        upstat: Result<String, String>,
        relation: Result<String, String>,
        top: Result<String, String>,
        card: Result<String, String>,
        dynamics: Result<String, String>,
        articles: Result<String, String>,
    }

    impl FakeClient {
        fn ok() -> Self {
            Self {
                profile: Ok(PROFILE.to_string()),
                videos: Ok(VIDEOS.to_string()),
                tags: Ok(TAGS.to_string()),
                upstat: Ok(UPSTAT.to_string()),
                relation: Ok(RELATION.to_string()),
                top: Ok(TOP.to_string()),
                card: Ok(CARD.to_string()),
                dynamics: Ok(DYNAMICS.to_string()),
                articles: Ok(ARTICLES.to_string()),
            }
        }
    }

    impl HttpClient for FakeClient {
        type Error = String;
        fn get(&self, req: &Request) -> Result<String, String> {
            match req.name {
                "user_info" => Ok(NAV.to_string()),
                "space_acc_info" => self.profile.clone(),
                // 老接口回落：测试里直接复用同一份 mock，
                // 这样「wbi 失败 → 回落成功」与「两边都失败」都能覆盖。
                "space_acc_info_legacy" => self.profile.clone(),
                "space_arc_search" => self.videos.clone(),
                "space_tags" => self.tags.clone(),
                "space_upstat" => self.upstat.clone(),
                "relation_stat" => self.relation.clone(),
                "space_top_arc" => self.top.clone(),
                "space_card" => self.card.clone(),
                "space_dynamics" => self.dynamics.clone(),
                "space_articles" => self.articles.clone(),
                other => Err(format!("未预期的请求：{other}")),
            }
        }
    }

    #[test]
    fn profile_pipeline_parses_and_decodes() {
        let client = FakeClient::ok();
        let profile = fetch_space_profile(&client, "", 25876945).expect("profile");

        assert_eq!(profile.mid, 25876945);
        assert_eq!(profile.name, "极客湾 & 朋友");
        assert_eq!(profile.sign, "聊点硬的 <数码>");
        assert_eq!(profile.face, "https://i1.hdslb.com/bfs/face/a.jpg");
        assert_eq!(profile.top_photo, "https://i0.hdslb.com/bfs/top/b.jpg");
        assert_eq!(profile.level, 6);
        assert_eq!(profile.official_title, "2023百大UP主");
        assert_eq!(profile.vip_label, "年度大会员");
        assert_eq!(profile.fans, 1_200_000);
        assert_eq!(profile.follows, 42);
        assert_eq!(profile.plays, 123_456_789);
        assert_eq!(profile.likes, 4_567_890);
        assert_eq!(profile.joined_at, 1_388_419_200);
        assert_eq!(
            profile.tags,
            vec!["数码", "评测 & 实验", "芯片", "手机", "音频"],
            "最多 5 条，且要解 HTML 实体"
        );
        assert_eq!(profile.pinned.as_ref().map(|v| v.bvid.as_str()), Some("BV1top"));
        assert_eq!(profile.pinned.as_ref().unwrap().duration_secs, 3723);
    }

    /// 可选接口全部失败时，画像仍然成功，只是字段降级。
    #[test]
    fn optional_failures_degrade_quietly() {
        let client = FakeClient {
            tags: Err("tags 请求失败".into()),
            upstat: Err("upstat 请求失败".into()),
            relation: Err("relation 请求失败".into()),
            top: Err("top 请求失败".into()),
            ..FakeClient::ok()
        };
        let profile = fetch_space_profile(&client, "", 25876945).expect("profile");
        assert_eq!(profile.name, "极客湾 & 朋友");
        assert!(profile.tags.is_empty());
        assert_eq!(profile.plays, 0);
        assert_eq!(profile.likes, 0);
        assert_eq!(profile.fans, 0);
        assert_eq!(profile.follows, 0);
        assert!(profile.pinned.is_none());
    }

    /// 可选接口返回业务错误码（未登录 / 风控）同样只降级。
    #[test]
    fn optional_non_zero_codes_degrade() {
        let client = FakeClient {
            tags: Ok(r#"{"code":-101,"message":"账号未登录"}"#.to_string()),
            upstat: Ok(r#"{"code":0}"#.to_string()),
            relation: Ok(r#"{"code":0,"data":null}"#.to_string()),
            // 53016 = 没有置顶稿件，属正常状态。
            top: Ok(r#"{"code":53016,"message":"没有置顶视频"}"#.to_string()),
            ..FakeClient::ok()
        };
        let profile = fetch_space_profile(&client, "", 25876945).expect("profile");
        assert!(profile.tags.is_empty());
        assert_eq!(profile.plays, 0);
        assert_eq!(profile.fans, 0);
        assert!(profile.pinned.is_none());
    }

    /// `acc/info` 是必需接口：失败必须整体返回 Err。
    #[test]
    fn profile_requires_acc_info() {
        let client = FakeClient {
            profile: Ok(r#"{"code":-352,"message":"风控校验失败"}"#.to_string()),
            ..FakeClient::ok()
        };
        let err = fetch_space_profile(&client, "", 25876945).expect_err("must fail");
        assert!(err.contains("空间资料"), "{err}");
        assert!(err.contains("-352"), "{err}");
        assert!(err.contains("风控校验失败"), "{err}");

        let broken = FakeClient {
            profile: Ok("not json".to_string()),
            ..FakeClient::ok()
        };
        let err = fetch_space_profile(&broken, "", 25876945).expect_err("must fail");
        assert!(err.starts_with("空间资料响应解析失败："), "{err}");
    }

    #[test]
    fn videos_pipeline_parses_list_and_total() {
        let client = FakeClient::ok();
        let page = fetch_space_videos(&client, "", 25876945, 1, 30, SpaceOrder::Newest)
            .expect("videos");
        assert_eq!(page.total, 187);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].bvid, "BV1a");
        assert_eq!(page.items[0].title, "旗舰 & 横评");
        assert_eq!(page.items[0].cover, "https://i0.hdslb.com/bfs/archive/x.jpg");
        assert_eq!(page.items[0].duration_secs, 754);
        assert_eq!(page.items[0].views, 2_436_575);
        assert_eq!(page.items[0].danmaku, 40_763);
        assert_eq!(page.items[0].created, 1_793_087_129);
        assert_eq!(page.items[1].duration_secs, 59);
    }

    #[test]
    fn videos_failure_is_an_error() {
        let client = FakeClient {
            videos: Ok(r#"{"code":-352,"message":"风控校验失败"}"#.to_string()),
            ..FakeClient::ok()
        };
        let err = fetch_space_videos(&client, "", 25876945, 1, 30, SpaceOrder::MostPlayed)
            .expect_err("must fail");
        assert!(err.contains("空间投稿"), "{err}");
        assert!(err.contains("-352"), "{err}");

        let broken = FakeClient {
            videos: Ok("<html>".to_string()),
            ..FakeClient::ok()
        };
        let err = fetch_space_videos(&broken, "", 25876945, 1, 30, SpaceOrder::Newest)
            .expect_err("must fail");
        assert!(err.starts_with("空间投稿响应解析失败："), "{err}");
    }

    #[test]
    fn empty_video_data_is_an_empty_page() {
        let client = FakeClient {
            videos: Ok(r#"{"code":0,"data":null}"#.to_string()),
            ..FakeClient::ok()
        };
        let page = fetch_space_videos(&client, "", 25876945, 1, 30, SpaceOrder::Newest)
            .expect("videos");
        assert!(page.items.is_empty());
        assert_eq!(page.total, 0);
    }

    #[test]
    fn order_maps_to_bili_values() {
        assert_eq!(SpaceOrder::Newest.as_str(), "pubdate");
        assert_eq!(SpaceOrder::MostPlayed.as_str(), "click");
        assert_eq!(SpaceOrder::MostFavorited.as_str(), "stow");
    }

    #[test]
    fn space_urls_are_signed_only_where_needed() {
        const WTS: u64 = 1_702_204_169;
        const IMG: &str = "7cd084941338484aae1ad9425b84077c";
        const SUB: &str = "4932caff0ff746eab6f01bf08b70ac45";

        let info = bilibili::space_acc_info(25876945, IMG, SUB, "SESSDATA=x", WTS);
        assert!(info
            .url
            .starts_with("https://api.bilibili.com/x/space/wbi/acc/info?"));
        assert!(info.url.contains("mid=25876945"));
        assert!(info.url.contains("wts=1702204169"));
        assert!(info.url.contains("w_rid="));
        assert_eq!(info.cookie, "SESSDATA=x");

        let search = bilibili::space_arc_search(
            25876945,
            2,
            30,
            SpaceOrder::MostFavorited.as_str(),
            IMG,
            SUB,
            "",
            WTS,
        );
        assert!(search
            .url
            .starts_with("https://api.bilibili.com/x/space/wbi/arc/search?"));
        assert!(search.url.contains("pn=2"));
        assert!(search.url.contains("ps=30"));
        assert!(search.url.contains("order=stow"));
        assert!(search.url.contains("w_rid="));

        let tags = bilibili::space_tags(25876945, "");
        assert_eq!(tags.url, "https://api.bilibili.com/x/space/acc/tags?mid=25876945");
        assert!(!tags.url.contains("w_rid"));

        let upstat = bilibili::space_upstat(25876945, "");
        assert_eq!(
            upstat.url,
            "https://api.bilibili.com/x/space/upstat?mid=25876945"
        );

        let top = bilibili::space_top_arc(25876945, "");
        assert_eq!(
            top.url,
            "https://api.bilibili.com/x/space/top/arc?mid=25876945"
        );
        assert!(!top.url.contains("w_rid"));
    }

    #[test]
    fn invalid_mid_is_rejected_before_any_request() {
        let client = FakeClient::ok();
        assert!(fetch_space_profile(&client, "", 0).is_err());
        assert!(fetch_space_videos(&client, "", 0, 1, 30, SpaceOrder::Newest).is_err());
        assert!(fetch_space_card(&client, "", 0).is_err());
        assert!(fetch_space_dynamics(&client, "", 0, "").is_err());
        assert!(fetch_space_articles(&client, "", 0, 1, 10).is_err());
    }

    #[test]
    fn card_pipeline_parses_and_decodes() {
        let client = FakeClient::ok();
        let card = fetch_space_card(&client, "", 25876945).expect("card");

        assert_eq!(card.mid, 25876945);
        assert_eq!(card.name, "极客湾 & 朋友");
        assert_eq!(card.face, "https://i1.hdslb.com/bfs/face/a.jpg");
        assert_eq!(card.sign, "聊点硬的 <数码>");
        assert_eq!(card.level, 6);
        assert_eq!(card.official_title, "2023百大UP主");
        assert_eq!(card.vip_label, "年度大会员");
        assert_eq!(card.fans, 1_200_001, "data.follower 优先于 card.fans");
        assert_eq!(card.follows, 42);
        assert_eq!(card.archive_count, 187);
        assert_eq!(card.likes, 4_567_890);
        assert!(card.followed);
    }

    /// 名片字段大量缺失时仍然成功，缺的部分给 0 / 空 / false。
    #[test]
    fn card_missing_fields_degrade() {
        let client = FakeClient {
            card: Ok(r#"{"code":0,"data":{"following":1}}"#.to_string()),
            ..FakeClient::ok()
        };
        let card = fetch_space_card(&client, "", 25876945).expect("card");
        assert_eq!(card.mid, 25876945, "card 缺失时回退到请求的 mid");
        assert_eq!(card.name, "");
        assert_eq!(card.level, 0);
        assert_eq!(card.official_title, "");
        assert_eq!(card.fans, 0);
        assert!(card.followed, "following 为 1 也要算已关注");
    }

    #[test]
    fn dynamics_pipeline_parses_items_and_cursor() {
        let client = FakeClient::ok();
        let (items, offset, has_more) =
            fetch_space_dynamics(&client, "", 25876945, "").expect("dynamics");

        assert!(has_more);
        assert_eq!(offset, "next-page");
        assert_eq!(items.len(), 4);

        let video = &items[0];
        assert_eq!(video.id, 1001);
        assert_eq!(video.kind, "video");
        assert_eq!(video.pub_ts, 1_793_000_000);
        assert_eq!(video.text, "旗舰 & 横评 话题", "正文要去标签、解实体");
        assert_eq!(video.title, "旗舰 & 横评");
        assert_eq!(video.cover, "https://i0.hdslb.com/bfs/archive/x.jpg");
        assert_eq!((video.likes, video.comments, video.forwards), (12, 3, 1));

        let article = &items[1];
        assert_eq!(article.kind, "article");
        assert_eq!(article.title, "专栏 & 标题");
        assert_eq!(article.cover, "https://i0.hdslb.com/bfs/article/c.jpg");
        assert_eq!(article.likes, 5, "计数是字符串也要接");
        assert_eq!(article.text, "");

        let image = &items[2];
        assert_eq!(image.kind, "image");
        assert_eq!(image.text, "看图 >>");
        assert_eq!(image.title, "");
        assert_eq!(image.cover, "https://i0.hdslb.com/bfs/new_dyn/1.jpg");

        let forward = &items[3];
        assert_eq!(forward.kind, "forward");
        assert_eq!(forward.text, "");
        assert!(forward.cover.is_empty());
    }

    #[test]
    fn dynamics_carry_video_stats_and_duration() {
        let video: Value = serde_json::from_str(
            r#"{"id_str":"2001","type":"DYNAMIC_TYPE_AV","modules":{
                 "module_author":{"pub_ts":1793000000},
                 "module_dynamic":{"major":{"archive":{
                   "title":"旗舰 &amp; 横评","cover":"//i0.hdslb.com/bfs/archive/x.jpg",
                   "duration_text":"12:34",
                   "stat":{"play":2436575,"danmaku":"40763"}}}}}}"#,
        )
        .expect("video dynamic json");
        let video = parse_dynamic(&video);
        assert_eq!(
            (video.views, video.danmaku, video.duration_secs),
            (2_436_575, 40_763, 754)
        );

        let text: Value = serde_json::from_str(
            r#"{"id_str":"2002","type":"DYNAMIC_TYPE_WORD","modules":{
                 "module_author":{"pub_ts":1792900000},
                 "module_dynamic":{"desc":{"text":"随便说说"}}}}"#,
        )
        .expect("text dynamic json");
        let text = parse_dynamic(&text);
        assert_eq!((text.views, text.danmaku, text.duration_secs), (0, 0, 0));
    }

    #[test]
    fn opus_and_draw_dynamics_carry_images() {
        // 新版图文：媒体在 major.opus.pics[].url，且 type 是 OPUS。
        let opus: Value = serde_json::from_str(
            r#"{"id_str":"3001","type":"DYNAMIC_TYPE_OPUS","modules":{
                 "module_author":{"pub_ts":1793000000},
                 "module_dynamic":{"major":{"opus":{
                   "summary":{"text":"两张图"},
                   "pics":[{"url":"//i0.hdslb.com/bfs/new_dyn/a.jpg"},
                           {"url":"//i0.hdslb.com/bfs/new_dyn/b.jpg"}]}}}}}"#,
        )
        .expect("opus dynamic json");
        let opus = parse_dynamic(&opus);
        assert_eq!(opus.kind, "image");
        assert_eq!(opus.images.len(), 2);
        assert!(opus.images[0].starts_with("https:"));
        // 没有 draw 时，封面回落到第一张图。
        assert_eq!(opus.cover, opus.images[0]);

        // 老图文：draw.items[].src。
        let draw: Value = serde_json::from_str(
            r#"{"id_str":"3002","type":"DYNAMIC_TYPE_DRAW","modules":{
                 "module_author":{"pub_ts":1793000000},
                 "module_dynamic":{"major":{"draw":{"items":[{"src":"//i0.hdslb.com/x.jpg"}]}}}}}"#,
        )
        .expect("draw dynamic json");
        let draw = parse_dynamic(&draw);
        assert_eq!(draw.kind, "image");
        assert_eq!(draw.images.len(), 1);
    }

    #[test]
    fn empty_dynamics_is_an_empty_page() {
        let client = FakeClient {
            dynamics: Ok(r#"{"code":0,"data":null}"#.to_string()),
            ..FakeClient::ok()
        };
        let (items, offset, has_more) =
            fetch_space_dynamics(&client, "", 25876945, "").expect("dynamics");
        assert!(items.is_empty());
        assert_eq!(offset, "");
        assert!(!has_more);
    }

    #[test]
    fn articles_pipeline_parses_items_and_total() {
        let client = FakeClient::ok();
        let (items, total) = fetch_space_articles(&client, "", 25876945, 1, 10).expect("articles");

        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 123);
        assert_eq!(items[0].title, "第一 & 篇");
        assert_eq!(items[0].summary, "摘要 <一>");
        assert_eq!(items[0].cover, "https://i0.hdslb.com/bfs/article/a.jpg");
        assert_eq!(items[0].views, 1234);
        assert_eq!(items[0].comments, 56);
        assert_eq!(items[0].published_at, 1_793_000_000);
        assert_eq!(items[1].id, 124, "id 是字符串也要接");
        assert!(items[1].cover.is_empty());
    }

    /// 新号没有 `count` 时，总数退回本页条数。
    #[test]
    fn articles_total_falls_back_to_page_len() {
        let client = FakeClient {
            articles: Ok(
                r#"{"code":0,"data":{"articles":[{"id":1,"title":"唯一一篇"}]}}"#.to_string(),
            ),
            ..FakeClient::ok()
        };
        let (items, total) = fetch_space_articles(&client, "", 25876945, 1, 10).expect("articles");
        assert_eq!(items.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn article_failure_is_an_error() {
        let client = FakeClient {
            articles: Ok(r#"{"code":-352,"message":"风控校验失败"}"#.to_string()),
            ..FakeClient::ok()
        };
        let err = fetch_space_articles(&client, "", 25876945, 1, 10).expect_err("must fail");
        assert!(err.contains("空间专栏"), "{err}");
        assert!(err.contains("-352"), "{err}");
    }

    #[test]
    fn card_and_article_urls() {
        const WTS: u64 = 1_702_204_169;
        const IMG: &str = "7cd084941338484aae1ad9425b84077c";
        const SUB: &str = "4932caff0ff746eab6f01bf08b70ac45";

        let card = bilibili::space_card(25876945, "SESSDATA=x");
        assert_eq!(
            card.url,
            "https://api.bilibili.com/x/web-interface/card?mid=25876945&photo=false"
        );
        assert!(!card.url.contains("w_rid"));
        assert_eq!(card.cookie, "SESSDATA=x");

        let first = bilibili::space_dynamics(25876945, "", "");
        assert!(first
            .url
            .starts_with("https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space?"));
        assert!(first.url.contains("host_mid=25876945"));
        assert!(first.url.contains("offset=&timezone_offset=-480"));
        assert!(first.url.contains("features=itemOpusStyle"));
        assert!(!first.url.contains("w_rid"));
        let next = bilibili::space_dynamics(25876945, " cursor ", "");
        assert!(next.url.contains("offset=cursor&"));

        let articles = bilibili::space_articles(25876945, 2, 10, IMG, SUB, "", WTS);
        assert!(articles
            .url
            .starts_with("https://api.bilibili.com/x/space/wbi/article?"));
        assert!(articles.url.contains("mid=25876945"));
        assert!(articles.url.contains("pn=2"));
        assert!(articles.url.contains("ps=10"));
        assert!(articles.url.contains("order=pubdate"));
        assert!(articles.url.contains("w_rid="));
    }
}
