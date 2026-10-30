//! 收藏夹内容：接口模型 → 可直接喂给首页卡片的条目。
//!
//! 收藏夹列表（夹子本身）沿用 [`crate::interaction::fetch_fav_folders`]，
//! 这里只负责「某个夹子里的视频」：
//!
//! - `GET /x/v3/fav/resource/list`，按收藏时间倒序分页（`media_id` / `pn` / `ps`）；
//! - 封面与 UP 头像同样是 `http://` 原图，标题可能带 HTML 实体，统一在这里清洗；
//! - `attr != 0` 的条目是「已失效 / 私密」视频，保留但标记，界面点不开。
//!
//! 和 `history` / `search` 一样，本模块不依赖 gpui，方便单测。

use crate::api::bilibili::{self, HttpClient, Request};
use crate::format;
use crate::models::CoverItem;
use crate::search::image_url;
use serde_json::Value;

/// 收藏夹内容默认每页条数（与网页版一致）。
pub const FAV_PAGE_SIZE: u32 = 20;

/// 收藏夹里的一个视频。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FavVideo {
    /// av 号（收藏接口的 `id`）。
    pub aid: i64,
    pub bvid: String,
    pub title: String,
    pub cover: String,
    pub up_name: String,
    pub up_face: String,
    pub up_mid: i64,
    pub duration_secs: u64,
    pub view_count: u64,
    pub danmaku_count: u64,
    pub pubdate: u64,
    /// 收藏时间（Unix 秒）。
    pub fav_time: u64,
    /// 0 = 正常；非 0 = 已失效 / 不可见。
    pub attr: i64,
}

impl FavVideo {
    /// 能否交给播放器（失效视频没有 bvid）。
    pub fn is_playable(&self) -> bool {
        self.attr == 0 && !self.bvid.trim().is_empty()
    }

    /// 封面缩略图（480×270 webp，与历史页同一档）。
    pub fn cover_thumb_url(&self) -> String {
        thumb(&image_url(&self.cover), "@480w_270h_1c.webp")
    }

    /// UP 头像缩略图（64px）。
    pub fn face_thumb_url(&self) -> String {
        thumb(&image_url(&self.up_face), "@64w_64h_1c.webp")
    }

    /// 转成首页卡片模型：收藏夹页直接复用首页 `card()` 的封面 / 头像 / 点击播放。
    pub fn to_cover_item(&self, index: usize) -> CoverItem {
        CoverItem {
            index,
            title: self.title.clone(),
            placeholder_color: crate::feed::PLACEHOLDER_PALETTE
                [index % crate::feed::PLACEHOLDER_PALETTE.len()]
            .to_string(),
            image_url: image_url(&self.cover),
            bvid: self.bvid.clone(),
            cid: 0,
            up_name: self.up_name.clone(),
            up_face: self.face_thumb_url(),
            view_count: self.view_count,
            danmaku_count: self.danmaku_count,
            duration_secs: self.duration_secs,
            pubdate: self.pubdate,
            is_live: false,
            progress_secs: 0,
        }
    }
}

/// 一页收藏夹内容。
#[derive(Debug, Clone, Default)]
pub struct FavVideoPage {
    pub items: Vec<FavVideo>,
    /// 夹子标题（接口 `info.title`，用于校验当前选中的夹子）。
    pub title: String,
    /// 夹内总条数。
    pub total: u64,
    pub has_more: bool,
}

/// 构造「某个收藏夹的内容列表」请求。
///
/// `order=mtime` = 按收藏时间倒序；`type=0` = 全部类型；`platform=web` 是网页版
/// 的固定参数，缺了会被服务端当成 App 请求返回不同形状。
pub fn fav_videos_request(media_id: i64, page: u32, page_size: u32, cookie: &str) -> Request {
    Request {
        url: format!(
            "{}/x/v3/fav/resource/list?media_id={media_id}&pn={}&ps={}&order=mtime&type=0&tid=0&platform=web&web_location=333.1387",
            bilibili::API_BASE,
            page.max(1),
            page_size.max(1)
        ),
        cookie: cookie.to_string(),
        name: "fav_resource_list",
    }
}

/// 拉一页收藏夹内容。
pub fn fetch_fav_videos<C>(
    client: &C,
    cookie: &str,
    media_id: i64,
    page: u32,
    page_size: u32,
) -> Result<FavVideoPage, String>
where
    C: HttpClient<Error = String>,
{
    if cookie.trim().is_empty() {
        return Err("未登录，无法读取收藏夹".into());
    }
    if media_id <= 0 {
        return Err("收藏夹 id 无效".into());
    }
    let body = client.get(&fav_videos_request(media_id, page, page_size, cookie))?;
    parse_fav_videos(&body)
}

/// 解析 `/x/v3/fav/resource/list` 响应。
pub fn parse_fav_videos(body: &str) -> Result<FavVideoPage, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("收藏夹响应解析失败：{e}"))?;
    let code = root.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = str_of(&root, "message");
        return Err(format!(
            "收藏夹接口返回 code={code}{}",
            if message.is_empty() {
                String::new()
            } else {
                format!("（{message}）")
            }
        ));
    }
    let data = root
        .get("data")
        .ok_or_else(|| "收藏夹响应缺少 data".to_string())?;
    let info = data.get("info");
    let title = info
        .map(|i| format::html_decode(&str_of(i, "title")))
        .unwrap_or_default();
    let total = info.map(|i| num(i, "media_count")).unwrap_or(0);
    let items = data
        .get("medias")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_video).collect::<Vec<_>>())
        .unwrap_or_default();
    let has_more = data
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !items.is_empty();
    Ok(FavVideoPage {
        items,
        title,
        total,
        has_more,
    })
}

fn parse_video(item: &Value) -> FavVideo {
    let upper = item.get("upper");
    let cnt = item.get("cnt_info");
    FavVideo {
        aid: num(item, "id") as i64,
        bvid: str_of(item, "bvid"),
        title: format::html_decode(&str_of(item, "title")),
        cover: str_of(item, "cover"),
        up_name: upper
            .map(|u| format::html_decode(&str_of(u, "name")))
            .unwrap_or_default(),
        up_face: upper.map(|u| str_of(u, "face")).unwrap_or_default(),
        up_mid: upper.map(|u| num(u, "mid") as i64).unwrap_or(0),
        duration_secs: num(item, "duration"),
        view_count: cnt.map(|c| num(c, "play")).unwrap_or(0),
        danmaku_count: cnt.map(|c| num(c, "danmaku")).unwrap_or(0),
        pubdate: num(item, "pubtime"),
        fav_time: num(item, "fav_time"),
        attr: num(item, "attr") as i64,
    }
}

// ================= 小工具 =================

fn thumb(url: &str, suffix: &str) -> String {
    if url.is_empty() || url.contains('@') || !url.contains("hdslb.com") {
        return url.to_string();
    }
    format!("{url}{suffix}")
}

fn str_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn num(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|v| {
            v.as_u64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
            })
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_body() -> &'static str {
        r#"{
          "code": 0,
          "message": "0",
          "data": {
            "info": {"id": 63915371, "title": "默认收藏夹", "media_count": 266},
            "medias": [
              {
                "id": 115514202395151,
                "bvid": "BV1mG1QBNEFn",
                "title": "当你试着 &amp; 把 deepseek 变成大昔涟",
                "cover": "http://i1.hdslb.com/bfs/archive/a.jpg",
                "duration": 353,
                "pubtime": 1762607157,
                "fav_time": 1793194318,
                "attr": 0,
                "upper": {
                  "mid": 2033371361,
                  "name": "ZC_盏稀",
                  "face": "//i2.hdslb.com/bfs/face/b.jpg"
                },
                "cnt_info": {"collect": 5226, "play": 185755, "danmaku": 288}
              },
              {
                "id": 1,
                "bvid": "",
                "title": "已失效视频",
                "cover": "",
                "duration": 0,
                "pubtime": 0,
                "fav_time": 1,
                "attr": 1,
                "upper": null,
                "cnt_info": {}
              }
            ],
            "has_more": true
          }
        }"#
    }

    #[test]
    fn parses_fav_videos_and_flags_invalid_ones() {
        let page = parse_fav_videos(sample_body()).expect("parse");
        assert_eq!(page.title, "默认收藏夹");
        assert_eq!(page.total, 266);
        assert!(page.has_more);
        assert_eq!(page.items.len(), 2);

        let video = &page.items[0];
        assert_eq!(video.aid, 115514202395151);
        assert_eq!(video.bvid, "BV1mG1QBNEFn");
        assert_eq!(video.title, "当你试着 & 把 deepseek 变成大昔涟");
        assert_eq!(video.up_name, "ZC_盏稀");
        assert_eq!(video.up_mid, 2033371361);
        assert_eq!(video.duration_secs, 353);
        assert_eq!(video.view_count, 185755);
        assert_eq!(video.danmaku_count, 288);
        assert_eq!(video.fav_time, 1793194318);
        assert!(video.is_playable());
        assert_eq!(
            video.cover_thumb_url(),
            "https://i1.hdslb.com/bfs/archive/a.jpg@480w_270h_1c.webp"
        );
        assert_eq!(
            video.face_thumb_url(),
            "https://i2.hdslb.com/bfs/face/b.jpg@64w_64h_1c.webp"
        );

        assert!(!page.items[1].is_playable(), "attr=1 是失效视频");
    }

    #[test]
    fn to_cover_item_keeps_avatar_and_stats() {
        let page = parse_fav_videos(sample_body()).expect("parse");
        let cover = page.items[0].to_cover_item(3);
        assert_eq!(cover.index, 3);
        assert_eq!(cover.bvid, "BV1mG1QBNEFn");
        assert_eq!(cover.up_name, "ZC_盏稀");
        assert_eq!(
            cover.up_face,
            "https://i2.hdslb.com/bfs/face/b.jpg@64w_64h_1c.webp"
        );
        assert_eq!(cover.view_count, 185755);
        assert_eq!(cover.danmaku_count, 288);
        assert_eq!(cover.duration_secs, 353);
        assert_eq!(cover.pubdate, 1762607157);
    }

    #[test]
    fn request_carries_paging_and_platform() {
        let req = fav_videos_request(63915371, 2, 20, "SESSDATA=x");
        assert!(req.url.contains("media_id=63915371"));
        assert!(req.url.contains("pn=2"));
        assert!(req.url.contains("ps=20"));
        assert!(req.url.contains("order=mtime"));
        assert!(req.url.contains("platform=web"));
        assert_eq!(req.cookie, "SESSDATA=x");
    }

    #[test]
    fn non_zero_code_is_an_error() {
        let err =
            parse_fav_videos(r#"{"code":-101,"message":"账号未登录"}"#).expect_err("must fail");
        assert!(err.contains("-101"));
        assert!(err.contains("账号未登录"));
    }
}
