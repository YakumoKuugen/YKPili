//! 关注直播：列表模型、接口请求与响应解析。
//!
//! 数据源是 `api.live.bilibili.com/relation/v1/feed/feed_list`：一次请求
//! 返回「关注中且正在直播」的房间，不需要先拉关注列表再按 50 个一批查状态。
//!
//! 这里只做「响应 → 干净列表」：字段名按实测响应为准，缺字段一律降级兜底，
//! 解析失败不 panic；视图层的四态（未登录 / 加载 / 空 / 错误）由 app 维护。
//!
//! 设计见 `docs/关注页-动态与直播分栏.md` 5.1 / 5.3。

use std::collections::HashSet;

use serde_json::Value;

use crate::api::bilibili::{self, HttpClient};
use crate::api::limits::{self, Bucket};
use crate::format;

/// 一页拉多少个直播间。与动态流一页的量级接近，翻页时不会一次打太多。
pub const LIVE_PAGE_SIZE: u32 = 30;

/// 一个正在直播（或曾经直播）的关注房间。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveRoom {
    /// 直播间号；取流与外链的唯一键。
    pub room_id: i64,
    /// 主播 mid（打开 UP 主空间）。
    pub uid: i64,
    pub uname: String,
    pub face: String,
    pub title: String,
    /// 封面；可能为空，视图侧走渐变兜底。
    pub cover: String,
    /// 分区名，如「英雄联盟」。
    pub area_name: String,
    /// 累计看过（精确值）。
    pub watched: u64,
    /// 服务端缩写，如 `3.0万`。
    pub watched_text: String,
    /// 服务端完整文案，如 `3.0万人看过`。
    pub watched_full: String,
    /// 在线人气，仅用于排序。
    pub online: u64,
    /// 1 = 在播。
    pub live_status: i64,
    /// 数据源能直接给出的关注状态。
    ///
    /// 关注直播列表里的房间**天然都是已关注**（接口只返回关注中的在播房间），
    /// 播放器据此直接点亮关注按钮，不再额外请求 `x/relation`。
    pub followed: Option<bool>,
    /// 开播时间（Unix 秒）。
    pub started_at: Option<u64>,
}

impl LiveRoom {
    /// 封面缩略图（480×270 webp）：与排行榜 / 首页的缩略规则一致。
    pub fn cover_thumb_url(&self) -> String {
        thumb(&image_url(&self.cover), "@480w_270h_1c.webp")
    }

    /// 主播头像缩略图（64×64 webp）。
    pub fn face_thumb_url(&self) -> String {
        thumb(&image_url(&self.face), "@64w_64h_1c.webp")
    }

    /// 「看过」文案：服务端给了缩写就用，没给就用精确值兜底。
    pub fn watched_label(&self) -> String {
        let text = self.watched_text.trim();
        if !text.is_empty() {
            return text.to_string();
        }
        if self.watched > 0 {
            return format::count(self.watched);
        }
        String::new()
    }
}

/// 一页关注直播。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LivePage {
    pub items: Vec<LiveRoom>,
    pub has_more: bool,
    pub page: u32,
    /// 给界面看的补充说明（例如「已过滤推荐位」）；None = 不需要提示。
    pub note: Option<String>,
}

/// 直播域命中 `-352` 后统一冷却 30s：同域所有请求（列表 / 取流 / 房间信息 /
/// 弹幕 token）一起让路，而不是按 1s 间隔继续试探。
pub(crate) fn note_live_risk(error: &str) {
    if error.contains("-352") {
        limits::penalize(Bucket::Live, std::time::Duration::from_secs(10));
        eprintln!("[live] 命中 -352，直播接口冷却 10s");
    }
}

/// 拉一页关注直播。
///
/// 未登录直接报错，避免把「-101 账号未登录」包装成空列表 —— 页面上
/// 就分不清「关注的主播都没开播」和「登录失效」。
pub fn fetch_following_lives<C>(
    client: &C,
    cookie: &str,
    page: u32,
) -> Result<LivePage, String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法获取关注的直播".into());
    }
    let page = page.max(1);
    let req = bilibili::live_following(page, LIVE_PAGE_SIZE, cookie);
    let body = client.get(&req)?;
    let mut parsed = match parse_following_lives(&body) {
        Ok(parsed) => parsed,
        Err(error) => {
            note_live_risk(&error);
            return Err(error);
        }
    };
    parsed.page = page;
    Ok(parsed)
}

/// 解析关注直播列表响应。
///
/// 公开给单测：字段名以真实响应为准，测试用小 fixture 固化下来，
/// 上游改结构时先在这里变红，而不是等视图空白才发现。
pub fn parse_following_lives(body: &str) -> Result<LivePage, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("直播列表响应解析失败：{e}"))?;
    let code = int_of(&value, "code");
    if code != 0 {
        let message = text_of(&value, "msg");
        let message = if message.is_empty() {
            text_of(&value, "message")
        } else {
            message
        };
        let suffix = if message.is_empty() {
            String::new()
        } else {
            format!("（{message}）")
        };
        return Err(format!("直播列表接口返回 code={code}{suffix}"));
    }

    let data = value
        .get("data")
        .filter(|data| data.is_object() || data.is_array())
        .ok_or_else(|| "直播列表响应缺少 data".to_string())?;
    // 正常形态是 `data.list`；个别风控响应会把 data 直接给成数组（空），
    // 这里两种都接住，避免「有 data 但没 list」时整页报错。
    let list = if let Some(array) = data.as_array() {
        array.clone()
    } else {
        data.get("list")
            .or_else(|| data.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    let mut items = Vec::with_capacity(list.len());
    let mut seen = HashSet::new();
    let mut filtered_ads = 0usize;
    for raw in &list {
        let Some(room) = LiveRoom::from_raw(raw) else {
            continue;
        };
        if room.room_id <= 0 || room.uid <= 0 {
            eprintln!(
                "[live] 丢弃缺少 room_id / uid 的直播条目：{}",
                room.title
            );
            continue;
        }
        if room.live_status != 1 {
            continue;
        }
        if is_ad(raw) {
            filtered_ads += 1;
            continue;
        }
        if !seen.insert(room.room_id) {
            continue;
        }
        items.push(room);
    }

    // 按看过人数降序（稳定排序）；全是 0 时保持接口原序。
    if items.iter().any(|room| room.watched > 0) {
        items.sort_by(|a, b| b.watched.cmp(&a.watched));
    }

    let has_more = match data.get("has_more") {
        Some(value) => value
            .as_bool()
            .or_else(|| value.as_u64().map(|number| number != 0))
            .unwrap_or(false),
        None => {
            // 实测列表接口回的是 `results / page / pagesize`，不是 has_more。
            let page = int_of(data, "page");
            let pagesize = int_of(data, "pagesize");
            let results = int_of(data, "results");
            if page > 0 && pagesize > 0 && results >= 0 {
                page * pagesize < results
            } else {
                list.len() as u32 >= LIVE_PAGE_SIZE
            }
        }
    };
    let note = (filtered_ads > 0)
        .then(|| format!("已过滤 {filtered_ads} 个推荐位直播间"));
    if filtered_ads > 0 {
        eprintln!("[live] 已过滤 {filtered_ads} 个推荐位直播间");
    }

    Ok(LivePage {
        items,
        has_more,
        page: 1,
        note,
    })
}

impl LiveRoom {
    /// 从一条原始 JSON 里取出房间；`room_id` 缺失时尝试从 `link` 解析。
    fn from_raw(raw: &Value) -> Option<Self> {
        let room_info = raw.get("room_info").filter(|v| v.is_object());
        let anchor = raw
            .get("anchor_info")
            .and_then(|info| info.get("base_info"))
            .filter(|v| v.is_object());
        let sources: Vec<&Value> = [Some(raw), room_info, anchor]
            .into_iter()
            .flatten()
            .collect();

        let mut room_id = num_in(&sources, "roomid").max(num_in(&sources, "room_id")) as i64;
        if room_id <= 0 {
            room_id = room_id_from_link(&str_in(&sources, "link"));
        }
        let uid = num_in(&sources, "uid").max(num_in(&sources, "mid")) as i64;
        if room_id <= 0 && uid <= 0 {
            return None;
        }

        let uname = {
            let name = str_in(&sources, "uname");
            if name.is_empty() {
                str_in(&sources, "name")
            } else {
                name
            }
        };
        let watched_show = [Some(raw), room_info, anchor]
            .into_iter()
            .flatten()
            .find_map(|source| source.get("watched_show").filter(|v| v.is_object()));
        let watched = watched_show
            .map(|show| num_of(show, "num"))
            .unwrap_or(0);
        let watched_text = watched_show
            .map(|show| text_of(show, "text_small"))
            .unwrap_or_default();
        let watched_full = watched_show
            .map(|show| text_of(show, "text_large"))
            .unwrap_or_default();

        let cover = {
            // 实测 `cover` 是主播头像，`pic` 才是直播间封面。
            let pic = str_in(&sources, "pic");
            if !pic.is_empty() {
                pic
            } else {
                let cover = str_in(&sources, "cover");
                if cover.is_empty() {
                    let keyframe = str_in(&sources, "keyframe");
                    if keyframe.is_empty() {
                        str_in(&sources, "user_cover")
                    } else {
                        keyframe
                    }
                } else {
                    cover
                }
            }
        };

        let started_at = opt_num_in(&sources, "live_start_time")
            .or_else(|| opt_num_in(&sources, "start_time"))
            .filter(|start| *start > 0);

        Some(Self {
            room_id,
            uid,
            uname: if uname.is_empty() {
                "未知主播".to_string()
            } else {
                uname
            },
            face: str_in(&sources, "face"),
            title: str_in(&sources, "title"),
            cover,
            area_name: {
                let area = str_in(&sources, "area_v2_name");
                if area.is_empty() {
                    str_in(&sources, "area_name")
                } else {
                    area
                }
            },
            watched,
            watched_text,
            watched_full,
            online: num_in(&sources, "online"),
            live_status: opt_num_in(&sources, "live_status")
                .or_else(|| opt_num_in(&sources, "status"))
                .unwrap_or(1) as i64,
            // 这个列表本身就是「关注中的直播」，无需再查关系。
            followed: Some(true),
            started_at,
        })
    }
}

fn is_ad(raw: &Value) -> bool {
    raw.get("is_ad")
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_u64().map(|number| number != 0))
                .or_else(|| {
                    value
                        .as_str()
                        .map(|text| text.eq_ignore_ascii_case("true") || text == "1")
                })
        })
        .unwrap_or(false)
}

/// 从 `https://live.bilibili.com/12345` / `...?room_id=12345` 里抠房间号。
fn room_id_from_link(link: &str) -> i64 {
    if let Some(rest) = link.split("room_id=").nth(1) {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(id) = digits.parse() {
            return id;
        }
    }
    let tail = link.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

// ================= 小工具 =================

fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|text| format::html_decode(text).trim().to_string())
        .unwrap_or_default()
}

fn str_in(sources: &[&Value], key: &str) -> String {
    sources
        .iter()
        .find_map(|source| {
            let text = text_of(source, key);
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_default()
}

/// 有符号整数（`code` 会是负数，不能用 u64 解析）。
fn int_of(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|value| {
            value.as_i64().or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.trim().parse::<i64>().ok())
                    .or_else(|| value.as_f64().map(|number| number as i64))
            })
        })
        .unwrap_or(0)
}

fn num_of(value: &Value, key: &str) -> u64 {    value
        .get(key)
        .and_then(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.trim().parse::<u64>().ok())
                    .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
            })
        })
        .unwrap_or(0)
}

fn num_in(sources: &[&Value], key: &str) -> u64 {
    sources
        .iter()
        .map(|source| num_of(source, key))
        .find(|value| *value > 0)
        .unwrap_or(0)
}

/// 取第一个「字段存在且能解析」的值（允许 0，用于 live_status 这类有意义的 0）。
fn opt_num_in(sources: &[&Value], key: &str) -> Option<u64> {
    sources
        .iter()
        .find(|source| source.get(key).is_some())
        .and_then(|source| source.get(key))
        .and_then(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.trim().parse::<u64>().ok())
                    .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
            })
        })
}

/// 图片地址规范化：补 `https:`、把 `http://` 升级成 https。
fn image_url(url: &str) -> String {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return format!("https://{rest}");
    }
    url.to_string()
}

fn thumb(url: &str, suffix: &str) -> String {
    if url.is_empty() || url.contains('@') || !url.contains("hdslb.com") {
        return url.to_string();
    }
    format!("{url}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{
      "code": 0,
      "msg": "success",
      "message": "success",
      "data": {
        "has_more": true,
        "list": [
          {
            "roomid": 1001,
            "uid": 11,
            "uname": "生肉烹饪师",
            "face": "//i0.hdslb.com/bfs/face/a.jpg",
            "title": "今天也在认真打排位",
            "cover": "https://i0.hdslb.com/bfs/live/c1.jpg",
            "pic": "https://i0.hdslb.com/bfs/live/room_cover.jpg",
            "area_v2_name": "英雄联盟",
            "watched_show": {"num": 30000, "text_small": "3.0万", "text_large": "3.0万人看过"},
            "online": 999,
            "live_status": 1,
            "is_ad": false
          },
          {
            "roomid": 1002,
            "uid": 12,
            "uname": "小主播",
            "title": "深夜电台",
            "live_status": 1,
            "watched_show": {"num": 12},
            "online": 3
          },
          {
            "roomid": 1003,
            "uid": 13,
            "title": "已下播",
            "live_status": 0
          },
          {
            "roomid": 1004,
            "uid": 14,
            "title": "广告位",
            "live_status": 1,
            "is_ad": true
          },
          {
            "roomid": 1002,
            "uid": 12,
            "title": "重复的房间",
            "live_status": 1
          },
          {
            "uid": 15,
            "title": "从 link 里找房间号",
            "link": "https://live.bilibili.com/1005",
            "live_status": 1
          },
          {
            "roomid": 1006,
            "title": "缺少 uid，应被丢弃",
            "live_status": 1
          }
        ]
      }
    }"#;

    #[test]
    fn maps_fields_and_falls_back_for_missing_ones() {
        let page = parse_following_lives(BODY).unwrap();
        assert_eq!(page.items.len(), 3, "下播 / 广告 / 重复 / 无 uid 都要过滤");

        let first = &page.items[0];
        assert_eq!(first.room_id, 1001);
        assert_eq!(first.uid, 11);
        assert_eq!(first.uname, "生肉烹饪师");
        assert_eq!(first.area_name, "英雄联盟");
        assert_eq!(first.watched, 30000);
        assert_eq!(first.watched_text, "3.0万");
        assert_eq!(first.watched_full, "3.0万人看过");
        // 关注直播列表天然已关注，播放器据此免掉一次关系查询。
        assert_eq!(first.followed, Some(true));
        assert_eq!(
            first.cover_thumb_url(),
            "https://i0.hdslb.com/bfs/live/room_cover.jpg@480w_270h_1c.webp"
        );
        assert_eq!(
            first.face_thumb_url(),
            "https://i0.hdslb.com/bfs/face/a.jpg@64w_64h_1c.webp"
        );

        // 缺 watched_show 文案时用精确值兜底；缺封面不报错。
        let second = &page.items[1];
        assert_eq!(second.room_id, 1002);
        assert_eq!(second.watched, 12);
        assert_eq!(second.watched_text, "");
        assert_eq!(second.watched_label(), "12");
        assert_eq!(second.cover_thumb_url(), "");

        // room_id 从 link 末段解析出来。
        let third = &page.items[2];
        assert_eq!(third.room_id, 1005);
        assert_eq!(third.live_status, 1);
    }

    #[test]
    fn sorts_by_watched_desc_but_keeps_order_when_all_zero() {
        let page = parse_following_lives(BODY).unwrap();
        let watched: Vec<u64> = page.items.iter().map(|room| room.watched).collect();
        assert_eq!(watched, vec![30000, 12, 0]);

        let all_zero = r#"{"code":0,"data":{"list":[
            {"roomid":1,"uid":1,"title":"甲","live_status":1},
            {"roomid":2,"uid":2,"title":"乙","live_status":1},
            {"roomid":3,"uid":3,"title":"丙","live_status":1}
        ]}}"#;
        let page = parse_following_lives(all_zero).unwrap();
        let titles: Vec<&str> = page.items.iter().map(|room| room.title.as_str()).collect();
        assert_eq!(titles, vec!["甲", "乙", "丙"]);
    }

    #[test]
    fn reports_interface_error_with_code_and_message() {
        let error = parse_following_lives(
            r#"{"code":-101,"msg":"账号未登录","message":"账号未登录","data":[]}"#,
        )
        .unwrap_err();
        assert!(error.contains("-101"));
        assert!(error.contains("账号未登录"));

        let bad_json = parse_following_lives("not json").unwrap_err();
        assert!(bad_json.contains("解析失败"));
    }

    #[test]
    fn empty_or_missing_list_is_an_empty_page() {
        let page = parse_following_lives(r#"{"code":0,"data":{}}"#).unwrap();
        assert!(page.items.is_empty());
        assert!(!page.has_more);

        // 风控把 data 给成数组（空）：按空列表处理，不报「缺少 data」。
        let page = parse_following_lives(r#"{"code":0,"data":[]}"#).unwrap();
        assert!(page.items.is_empty());
        assert!(!page.has_more);
    }

    #[test]
    fn request_targets_live_feed_endpoint() {
        let req = bilibili::live_following(2, 30, "SESSDATA=x");
        assert_eq!(
            req.url,
            "https://api.live.bilibili.com/relation/v1/feed/feed_list?page=2&page_size=30"
        );
        assert_eq!(req.name, "live_following");
        assert_eq!(req.cookie, "SESSDATA=x");
    }

    #[test]
    fn unauthenticated_fetch_fails_without_a_request() {
        // 测试替身没有被调用：未登录时不应该发出任何请求。
        struct NoClient;
        impl HttpClient for NoClient {
            type Error = String;
            fn get(&self, _req: &bilibili::Request) -> Result<String, String> {
                panic!("未登录不应发请求");
            }
        }
        let error = fetch_following_lives(&NoClient, "  ", 1).unwrap_err();
        assert!(error.contains("未登录"));
    }
}

// ================= 直播取流 =================

/// 一档直播画质（`g_qn_desc` 的一项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveQuality {
    pub id: i32,
    pub label: String,
}

/// 一次直播取流的结果：直链 + 画质表 + 房间信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePlayback {
    pub room_id: i64,
    pub uid: i64,
    pub uname: String,
    pub face: String,
    pub title: String,
    pub area_name: String,
    /// 选中的 FLV / HLS 直链。
    pub video_url: String,
    pub quality_id: i32,
    pub quality_label: String,
    pub qualities: Vec<LiveQuality>,
    /// 选中的是 FLV（true）还是 HLS（false）。
    pub is_flv: bool,
    /// 视频像素宽高（码流的 media_info；0 = 未知）。
    pub video_width: u32,
    pub video_height: u32,
}

/// 拉一次直播取流。
pub fn fetch_live_playback<C>(
    client: &C,
    room_id: i64,
    qn: i32,
    cookie: &str,
) -> Result<LivePlayback, String>
where
    C: HttpClient<Error = String>,
{
    if room_id <= 0 {
        return Err("直播间号不合法".into());
    }
    let req = bilibili::live_play_url(room_id, qn, cookie);
    let body = client.get(&req)?;
    let parsed = parse_live_playback_with_qn(&body, room_id, qn);
    if let Err(error) = &parsed {
        note_live_risk(error);
    }
    parsed
}

/// 解析 `getRoomPlayInfo` 响应并选一条流（默认请求原画）。
pub fn parse_live_playback(body: &str, room_id: i64) -> Result<LivePlayback, String> {
    parse_live_playback_with_qn(body, room_id, 10000)
}

/// 同 [`parse_live_playback`]，可指定期望画质；实测响应里 `current_qn`
/// 在 codec 上，服务端可能降档，所以以它为准、请求值只做兜底。
pub fn parse_live_playback_with_qn(
    body: &str,
    room_id: i64,
    requested_qn: i32,
) -> Result<LivePlayback, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("直播取流响应解析失败：{e}"))?;
    let code = int_of(&value, "code");
    if code != 0 {
        let message = {
            let msg = text_of(&value, "msg");
            if msg.is_empty() {
                text_of(&value, "message")
            } else {
                msg
            }
        };
        let suffix = if message.is_empty() {
            String::new()
        } else {
            format!("（{message}）")
        };
        return Err(format!("直播取流返回 code={code}{suffix}"));
    }
    let data = value
        .get("data")
        .filter(|data| data.is_object())
        .ok_or_else(|| "直播取流响应缺少 data".to_string())?;
    let room_info = data.get("room_info").filter(|info| info.is_object());
    let anchor = data
        .get("anchor_info")
        .and_then(|info| info.get("base_info"));

    // 实测新接口把 live_status 放在 data 根上；旧形态在 room_info 里。
    let live_status = if data.get("live_status").is_some() {
        int_of(data, "live_status")
    } else {
        room_info
            .map(|info| int_of(info, "live_status"))
            .unwrap_or(1)
    };
    if live_status != 1 {
        return Err("直播已结束".into());
    }

    let playurl = data
        .get("playurl_info")
        .and_then(|info| info.get("playurl"))
        .filter(|playurl| playurl.is_object())
        .ok_or_else(|| "该直播间当前没有可用流（可能需要付费 / 特殊权限）".to_string())?;

    let selection = select_live_stream(playurl, requested_qn)?;
    let current_qn = if selection.current_qn > 0 {
        selection.current_qn
    } else {
        requested_qn
    };
    let accept_qn: Vec<i32> = if !selection.accept_qn.is_empty() {
        selection.accept_qn.clone()
    } else {
        playurl
            .get("accept_qn")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_i64)
                    .map(|value| value as i32)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut qualities: Vec<LiveQuality> = playurl
        .get("g_qn_desc")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|item| {
                    let id = int_of(item, "qn") as i32;
                    if id <= 0 {
                        return None;
                    }
                    // accept_qn 给了就取交集：服务端没返回的档位不列。
                    if !accept_qn.is_empty() && !accept_qn.contains(&id) {
                        return None;
                    }
                    let label = text_of(item, "desc");
                    Some(LiveQuality {
                        id,
                        label: if label.is_empty() {
                            format!("{id}")
                        } else {
                            label
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // 画质菜单从上到下按码率降序，和点播的画质菜单心智一致。
    qualities.sort_by(|a, b| b.id.cmp(&a.id));
    let quality_label = qualities
        .iter()
        .find(|quality| quality.id == current_qn)
        .map(|quality| quality.label.clone())
        .unwrap_or_else(|| {
            if current_qn > 0 {
                format!("{current_qn}")
            } else {
                "原画".to_string()
            }
        });

    let actual_room_id = {
        let id = int_of(data, "room_id");
        if id > 0 {
            id
        } else {
            room_id
        }
    };
    let uid = int_of(data, "uid")
        .max(room_info.map(|info| int_of(info, "uid")).unwrap_or(0))
        .max(anchor.map(|a| int_of(a, "uid")).unwrap_or(0));
    let title = {
        let title = text_of(data, "title");
        if title.is_empty() {
            room_info
                .map(|info| text_of(info, "title"))
                .unwrap_or_default()
        } else {
            title
        }
    };
    let area_name = data
        .get("area_info")
        .map(|info| text_of(info, "area_name"))
        .or_else(|| room_info.and_then(|info| info.get("area_info")).map(|info| text_of(info, "area_name")))
        .or_else(|| room_info.map(|info| text_of(info, "area_name")))
        .unwrap_or_default();

    Ok(LivePlayback {
        room_id: actual_room_id,
        uid,
        uname: anchor.map(|a| text_of(a, "uname")).unwrap_or_default(),
        face: anchor.map(|a| text_of(a, "face")).unwrap_or_default(),
        title,
        area_name,
        video_url: selection.url,
        quality_id: current_qn,
        quality_label,
        qualities,
        is_flv: selection.is_flv,
        video_width: selection.width,
        video_height: selection.height,
    })
}

/// 选流结果（内部）。
struct LiveSelection {
    url: String,
    is_flv: bool,
    current_qn: i32,
    accept_qn: Vec<i32>,
    width: u32,
    height: u32,
}

/// 选流：`http_stream`（FLV）优先，其次 `http_hls`；同一协议里优先 AVC。
fn select_live_stream(playurl: &Value, requested_qn: i32) -> Result<LiveSelection, String> {
    let streams = playurl
        .get("stream")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for prefer_flv in [true, false] {
        for stream in &streams {
            let protocol = text_of(stream, "protocol_name");
            let is_flv = protocol == "http_stream";
            if is_flv != prefer_flv {
                continue;
            }
            for format in stream
                .get("format")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for codec in format
                    .get("codec")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let codec_name = text_of(codec, "codec_name");
                    if !codec_name.is_empty() && codec_name != "avc" {
                        continue;
                    }
                    let base_url = {
                        let base = text_of(codec, "base_url");
                        if base.is_empty() {
                            // 个别老响应把 base_url 放在 url_info 里。
                            codec
                                .get("url_info")
                                .and_then(Value::as_array)
                                .and_then(|infos| infos.first())
                                .map(|info| text_of(info, "base_url"))
                                .unwrap_or_default()
                        } else {
                            base
                        }
                    };
                    if base_url.is_empty() {
                        continue;
                    }
                    let accept_qn: Vec<i32> = codec
                        .get("accept_qn")
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_i64)
                                .map(|value| value as i32)
                                .collect()
                        })
                        .unwrap_or_default();
                    let current_qn = {
                        let current = int_of(codec, "current_qn") as i32;
                        if current > 0 {
                            current
                        } else {
                            int_of(playurl, "current_qn") as i32
                        }
                    };
                    let media = codec.get("media_info");
                    let width = media
                        .map(|info| int_of(info, "width") as u32)
                        .unwrap_or(0);
                    let height = media
                        .map(|info| int_of(info, "height") as u32)
                        .unwrap_or(0);
                    for info in codec
                        .get("url_info")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let url = live_stream_url(info, &base_url);
                        if !url.is_empty() {
                            let current_qn = if current_qn > 0 {
                                current_qn
                            } else {
                                requested_qn
                            };
                            return Ok(LiveSelection {
                                url,
                                is_flv,
                                current_qn,
                                accept_qn,
                                width,
                                height,
                            });
                        }
                    }
                }
            }
        }
    }

    Err("直播流列表为空（CDN 未返回可用地址）".to_string())
}

fn live_stream_url(info: &Value, base_url: &str) -> String {
    let host = text_of(info, "host");
    let extra = text_of(info, "extra");
    let mut url = if base_url.starts_with("http://") || base_url.starts_with("https://") {
        base_url.to_string()
    } else {
        format!("{host}{base_url}")
    };
    if extra.is_empty() {
        return url;
    }
    if url.ends_with('?') {
        url.pop();
    }
    if extra.starts_with('?') {
        format!("{url}{extra}")
    } else if extra.starts_with('&') {
        format!("{url}{extra}")
    } else if url.contains('?') {
        format!("{url}&{extra}")
    } else {
        format!("{url}?{extra}")
    }
}

#[cfg(test)]
mod playback_tests {
    use super::*;

    const PLAY: &str = r#"{
      "code": 0,
      "msg": "success",
      "data": {
        "room_info": {
          "room_id": 12345,
          "uid": 88,
          "title": "今晚打排位",
          "live_status": 1,
          "area_name": "英雄联盟"
        },
        "anchor_info": {"base_info": {"uname": "生肉烹饪师", "face": "https://i0.hdslb.com/bfs/face/a.jpg"}},
        "playurl_info": {
          "playurl": {
            "current_qn": 10000,
            "accept_qn": [10000, 400, 250],
            "g_qn_desc": [
              {"qn": 10000, "desc": "原画"},
              {"qn": 400, "desc": "蓝光"},
              {"qn": 250, "desc": "超清"},
              {"qn": 80, "desc": "不存在于 accept_qn"}
            ],
            "stream": [
              {
                "protocol_name": "http_hls",
                "format": [{"format_name": "ts", "codec": [{"codec_name": "avc", "url_info": [{"host": "https://hls.example.com", "base_url": "/live/1.m3u8", "extra": "?token=hls"}]}]}]
              },
              {
                "protocol_name": "http_stream",
                "format": [{"format_name": "flv", "codec": [{"codec_name": "avc", "url_info": [{"host": "https://flv.example.com", "base_url": "/live/1.flv", "extra": "?token=flv"}]}]}]
              }
            ]
          }
        }
      }
    }"#;

    #[test]
    fn prefers_flv_and_maps_qualities() {
        let playback = parse_live_playback(PLAY, 12345).unwrap();
        assert_eq!(playback.video_url, "https://flv.example.com/live/1.flv?token=flv");
        assert!(playback.is_flv);
        assert_eq!(playback.quality_id, 10000);
        assert_eq!(playback.quality_label, "原画");
        assert_eq!(
            playback.qualities.iter().map(|q| q.id).collect::<Vec<_>>(),
            vec![10000, 400, 250]
        );
        assert_eq!(playback.uname, "生肉烹饪师");
        assert_eq!(playback.area_name, "英雄联盟");
        assert_eq!(playback.uid, 88);
    }

    #[test]
    fn falls_back_to_hls_when_flv_is_missing() {
        let fixture = r#"{"code":0,"data":{"room_info":{"room_id":1,"live_status":1,"title":"t"},"playurl_info":{"playurl":{"current_qn":400,"g_qn_desc":[{"qn":400,"desc":"蓝光"}],"stream":[{"protocol_name":"http_hls","format":[{"codec":[{"codec_name":"avc","url_info":[{"host":"https://h","base_url":"/a.m3u8","extra":"?x=1"}]}]}]}]}}}}"#;
        let playback = parse_live_playback(fixture, 1).unwrap();
        assert_eq!(playback.video_url, "https://h/a.m3u8?x=1");
        assert!(!playback.is_flv);
    }

    #[test]
    fn ended_room_and_missing_playurl_have_clear_errors() {
        let ended = r#"{"code":0,"data":{"room_info":{"room_id":1,"live_status":0}}}"#;
        assert!(parse_live_playback(ended, 1).unwrap_err().contains("直播已结束"));

        let paid = r#"{"code":0,"data":{"room_info":{"room_id":1,"live_status":1}}}"#;
        assert!(parse_live_playback(paid, 1).unwrap_err().contains("没有可用流"));

        let error = r#"{"code":-101,"msg":"账号未登录","data":{}}"#;
        assert!(parse_live_playback(error, 1).unwrap_err().contains("-101"));
    }

    #[test]
    fn extra_without_question_mark_is_appended() {
        let fixture = r#"{"code":0,"data":{"room_info":{"room_id":1,"live_status":1},"playurl_info":{"playurl":{"current_qn":250,"g_qn_desc":[{"qn":250,"desc":"超清"}],"stream":[{"protocol_name":"http_stream","format":[{"codec":[{"codec_name":"avc","url_info":[{"host":"https://h","base_url":"/a.flv","extra":"token=1"}]}]}]}]}}}}"#;
        let playback = parse_live_playback(fixture, 1).unwrap();
        assert_eq!(playback.video_url, "https://h/a.flv?token=1");
        assert_eq!(playback.quality_label, "超清");
    }

    #[test]
    fn request_targets_live_playurl_endpoint() {
        let req = bilibili::live_play_url(12345, 10000, "SESSDATA=x");
        assert!(req.url.starts_with(
            "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo?room_id=12345"
        ));
        assert!(req.url.contains("protocol=0,1"));
        assert!(req.url.contains("format=0,1,2"));
        assert!(req.url.contains("codec=0"));
        assert!(req.url.contains("qn=10000"));
        assert_eq!(req.name, "live_play_url");
    }
}

// ================= 直播弹幕发送 =================

/// 发送一条直播弹幕（`POST /msg/send`）。
///
/// 走直播站的 Referer；`rnd` 是浏览器每次请求都带的微秒时间戳
/// （风控把它当「页面里发出来的」信号）。返回服务端 `code != 0` 的原话。
pub fn send_live_danmaku(
    client: &crate::api::client::ReqwestClient,
    room_id: i64,
    text: &str,
    cookie: &str,
    csrf: &str,
) -> Result<(), String> {
    let text = text.trim();
    if room_id <= 0 {
        return Err("直播间号不合法".into());
    }
    if text.is_empty() {
        return Err("弹幕内容为空".into());
    }
    if csrf.trim().is_empty() {
        return Err("登录会话已失效，请重新登录".into());
    }
    if text.chars().count() > 20 {
        return Err("直播弹幕最多 20 个字".into());
    }
    let rnd = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let form = [
        ("roomid", room_id.to_string()),
        ("msg", text.to_string()),
        ("csrf", csrf.to_string()),
        // 文档口径：`rnd` 是当前 Unix **秒**时间戳（不是微秒）。
        // 以前发微秒虽然多数情况也能过，但风控会把它当异常时间戳。
        ("rnd", rnd),
        ("mode", "1".to_string()),
        ("color", "16777215".to_string()),
        ("fontsize", "25".to_string()),
        ("bubble", "0".to_string()),
        ("room_type", "0".to_string()),
        ("jumpfrom", "0".to_string()),
    ];
    let url = format!("{}/msg/send", bilibili::LIVE_API_BASE);
    let body = client.post_form_with_referer(
        &url,
        &form,
        cookie,
        "https://live.bilibili.com/",
    )?;
    let value: Value =
        serde_json::from_str(&body).map_err(|e| format!("直播弹幕发送响应解析失败：{e}"))?;
    let code = int_of(&value, "code");
    if code != 0 {
        if code == -352 {
            note_live_risk("-352");
        }
        let message = {
            let msg = text_of(&value, "msg");
            if msg.is_empty() {
                text_of(&value, "message")
            } else {
                msg
            }
        };
        let suffix = if message.is_empty() {
            String::new()
        } else {
            format!("（{message}）")
        };
        return Err(format!("直播弹幕发送失败：code={code}{suffix}"));
    }
    Ok(())
}

// ================= 直播间信息 =================

/// 面板用的直播间信息（`getInfoByRoom`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveRoomInfo {
    pub room_id: i64,
    pub uid: i64,
    pub title: String,
    pub uname: String,
    pub face: String,
    pub cover: String,
    pub area_name: String,
    pub parent_area_name: String,
    pub live_status: i64,
    pub online: u64,
    pub watched_text: String,
}

/// 拉一次直播间信息。
pub fn fetch_live_room_info<C>(
    client: &C,
    room_id: i64,
    cookie: &str,
) -> Result<LiveRoomInfo, String>
where
    C: HttpClient<Error = String>,
{
    if room_id <= 0 {
        return Err("直播间号不合法".into());
    }
    let req = bilibili::live_room_info(room_id, cookie);
    let body = client.get(&req)?;
    let parsed = parse_live_room_info(&body, room_id);
    if let Err(error) = &parsed {
        note_live_risk(error);
    }
    parsed
}

pub fn parse_live_room_info(body: &str, room_id: i64) -> Result<LiveRoomInfo, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("直播间信息解析失败：{e}"))?;
    let code = int_of(&value, "code");
    if code != 0 {
        let message = {
            let msg = text_of(&value, "msg");
            if msg.is_empty() {
                text_of(&value, "message")
            } else {
                msg
            }
        };
        let suffix = if message.is_empty() {
            String::new()
        } else {
            format!("（{message}）")
        };
        return Err(format!("直播间信息返回 code={code}{suffix}"));
    }
    let data = value
        .get("data")
        .filter(|data| data.is_object())
        .ok_or_else(|| "直播间信息缺少 data".to_string())?;
    let room = data
        .get("room_info")
        .filter(|room| room.is_object())
        .ok_or_else(|| "直播间信息缺少 room_info".to_string())?;
    let base = data
        .get("anchor_info")
        .and_then(|info| info.get("base_info"));
    let actual_id = {
        let id = int_of(room, "room_id");
        if id > 0 {
            id
        } else {
            room_id
        }
    };
    Ok(LiveRoomInfo {
        room_id: actual_id,
        uid: int_of(room, "uid").max(base.map(|b| int_of(b, "uid")).unwrap_or(0)),
        title: text_of(room, "title"),
        uname: base
            .map(|b| text_of(b, "uname"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "未知主播".to_string()),
        face: base.map(|b| text_of(b, "face")).unwrap_or_default(),
        cover: text_of(room, "cover"),
        area_name: text_of(room, "area_name"),
        parent_area_name: text_of(room, "parent_area_name"),
        live_status: {
            let status = int_of(room, "live_status");
            if status == 0 {
                1
            } else {
                status
            }
        },
        online: int_of(room, "online").max(0) as u64,
        watched_text: data
            .get("watched_show")
            .map(|show| text_of(show, "text_small"))
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod info_tests {
    use super::*;

    #[test]
    fn maps_room_and_anchor_info() {
        let fixture = r#"{"code":0,"data":{
          "room_info":{"room_id":12345,"uid":88,"title":"今晚打排位","cover":"https://i0.hdslb.com/c.jpg","live_status":1,"area_name":"英雄联盟","parent_area_name":"游戏","online":1234},
          "anchor_info":{"base_info":{"uname":"生肉烹饪师","face":"https://i0.hdslb.com/f.jpg"}},
          "watched_show":{"text_small":"3.0万"}
        }}"#;
        let info = parse_live_room_info(fixture, 1).unwrap();
        assert_eq!(info.room_id, 12345);
        assert_eq!(info.uid, 88);
        assert_eq!(info.title, "今晚打排位");
        assert_eq!(info.uname, "生肉烹饪师");
        assert_eq!(info.area_name, "英雄联盟");
        assert_eq!(info.parent_area_name, "游戏");
        assert_eq!(info.watched_text, "3.0万");
    }

    #[test]
    fn reports_interface_errors() {
        let error = parse_live_room_info(r#"{"code":-404,"message":"啥都木有"}"#, 1)
            .unwrap_err();
        assert!(error.contains("-404"));
        assert!(error.contains("啥都木有"));
    }
}
