//! 排行榜：`/x/web-interface/ranking/v2` 的模型与解析。
//!
//! 榜单是**非个性化**的：同一个 `rid` 对所有人下发同一张榜，整榜一次返回
//! （全站 / 一级分区各 100 条），不存在滚动加载与「换一批」。
//! 因此这里没有游标，只有一次拉取。
//!
//! - `rid = 0` → 全站榜；
//! - `rid = 一级分区 tid` → 分区榜（二级分区在接口层就没有榜）；
//! - `data.note` 是官方对榜单口径的说明，界面原样展示。

use crate::api::bilibili::{self, HttpClient, Request};
use crate::format;
use crate::models::CoverItem;
use crate::search::image_url;
use serde_json::Value;

/// 榜单接口固定会返回的口径说明；`note` 缺失时用它兜底。
pub const DEFAULT_NOTE: &str = "根据稿件内容质量、近期的数据综合展示，动态更新";

/// 一个榜单分区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankZone {
    pub rid: u32,
    pub label: &'static str,
}

/// 全站榜 + 一级分区榜。`rid` 直接取 B 站一级分区 tid。
pub const ZONES: [RankZone; 20] = [
    RankZone {
        rid: 0,
        label: "全站",
    },
    RankZone {
        rid: 1,
        label: "动画",
    },
    RankZone {
        rid: 13,
        label: "番剧",
    },
    RankZone {
        rid: 168,
        label: "国创",
    },
    RankZone {
        rid: 3,
        label: "音乐",
    },
    RankZone {
        rid: 129,
        label: "舞蹈",
    },
    RankZone {
        rid: 4,
        label: "游戏",
    },
    RankZone {
        rid: 36,
        label: "知识",
    },
    RankZone {
        rid: 188,
        label: "科技",
    },
    RankZone {
        rid: 234,
        label: "运动",
    },
    RankZone {
        rid: 223,
        label: "汽车",
    },
    RankZone {
        rid: 160,
        label: "生活",
    },
    RankZone {
        rid: 211,
        label: "美食",
    },
    RankZone {
        rid: 217,
        label: "动物圈",
    },
    RankZone {
        rid: 119,
        label: "鬼畜",
    },
    RankZone {
        rid: 155,
        label: "时尚",
    },
    RankZone {
        rid: 202,
        label: "资讯",
    },
    RankZone {
        rid: 5,
        label: "娱乐",
    },
    RankZone {
        rid: 181,
        label: "影视",
    },
    RankZone {
        rid: 177,
        label: "纪录片",
    },
];

impl RankZone {
    /// 全站榜（`rid = 0`）。
    pub fn is_global(self) -> bool {
        self.rid == 0
    }
}

/// 榜单里的一个视频。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RankItem {
    /// 名次，从 1 开始。
    pub rank: usize,
    pub aid: i64,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub cover: String,
    pub up_name: String,
    pub up_face: String,
    pub up_mid: i64,
    pub view_count: u64,
    pub danmaku_count: u64,
    pub duration_secs: u64,
    pub pubdate: u64,
    /// 历史最高名次（接口 `stat.his_rank`；0 = 未知）。
    pub his_rank: u64,
    /// 较上次上升的名次数（接口 `up_from_v2`，不保证每条都有）。
    pub up_from: u64,
    /// 一级分区名，例如「数码」。
    pub tname: String,
}

impl RankItem {
    /// 封面缩略图（480×270 webp）。
    pub fn cover_thumb_url(&self) -> String {
        thumb(&image_url(&self.cover), "@480w_270h_1c.webp")
    }

    /// UP 头像缩略图（64px）。
    pub fn face_thumb_url(&self) -> String {
        thumb(&image_url(&self.up_face), "@64w_64h_1c.webp")
    }

    /// 名次变化文案：只信接口的 `up_from_v2`，拿不到就返回 `None`，
    /// 不按本地差分伪造（本期没有落盘快照）。
    pub fn rise_label(&self) -> Option<String> {
        (self.up_from > 0).then(|| format!("↑ {}", self.up_from))
    }

    /// 历史最高名次文案。
    pub fn his_rank_label(&self) -> Option<String> {
        (self.his_rank > 0).then(|| format!("历史最高 #{}", self.his_rank))
    }

    /// 转成首页卡片模型（排行榜页若切回网格布局可直接复用）。
    pub fn to_cover_item(&self, index: usize) -> CoverItem {
        CoverItem {
            index,
            title: self.title.clone(),
            placeholder_color: crate::feed::PLACEHOLDER_PALETTE
                [index % crate::feed::PLACEHOLDER_PALETTE.len()]
            .to_string(),
            image_url: image_url(&self.cover),
            bvid: self.bvid.clone(),
            cid: self.cid,
            up_name: self.up_name.clone(),
            up_face: self.face_thumb_url(),
            up_mid: self.up_mid,
            view_count: self.view_count,
            danmaku_count: self.danmaku_count,
            duration_secs: self.duration_secs,
            pubdate: self.pubdate,
            is_live: false,
            progress_secs: 0,
        }
    }
}

/// 一张榜。
#[derive(Debug, Clone, Default)]
pub struct RankPage {
    pub items: Vec<RankItem>,
    /// 接口原文的榜单说明。
    pub note: String,
}

/// 构造榜单请求。`type=all` 是网页版默认口径（只含普通视频稿件）。
pub fn ranking_request(rid: u32, cookie: &str) -> Request {
    Request {
        url: format!(
            "{}/x/web-interface/ranking/v2?rid={rid}&type=all&web_location=333.934",
            bilibili::API_BASE
        ),
        cookie: cookie.to_string(),
        name: "ranking_v2",
    }
}

/// 拉一张榜。
pub fn fetch_ranking<C>(client: &C, rid: u32) -> Result<RankPage, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&ranking_request(rid, ""))?;
    parse_ranking(&body)
}

/// 解析 `ranking/v2` 响应。
pub fn parse_ranking(body: &str) -> Result<RankPage, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("榜单响应解析失败：{e}"))?;
    let code = root.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = str_of(&root, "message");
        return Err(format!(
            "榜单接口返回 code={code}{}",
            if message.is_empty() {
                String::new()
            } else {
                format!("（{message}）")
            }
        ));
    }
    let data = root
        .get("data")
        .ok_or_else(|| "榜单响应缺少 data".to_string())?;
    let note = {
        let note = format::html_decode(&str_of(data, "note"));
        if note.trim().is_empty() {
            DEFAULT_NOTE.to_string()
        } else {
            note
        }
    };
    let items = data
        .get("list")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(index, item)| parse_item(item, index + 1))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(RankPage { items, note })
}

fn parse_item(item: &Value, rank: usize) -> RankItem {
    let owner = item.get("owner");
    let stat = item.get("stat");
    RankItem {
        rank,
        aid: num(item, "aid") as i64,
        bvid: str_of(item, "bvid"),
        cid: num(item, "cid") as i64,
        title: format::html_decode(&str_of(item, "title")),
        cover: str_of(item, "pic"),
        up_name: owner
            .map(|o| format::html_decode(&str_of(o, "name")))
            .unwrap_or_default(),
        up_face: owner.map(|o| str_of(o, "face")).unwrap_or_default(),
        up_mid: owner.map(|o| num(o, "mid") as i64).unwrap_or(0),
        view_count: stat.map(|s| num(s, "view")).unwrap_or(0),
        danmaku_count: stat.map(|s| num(s, "danmaku")).unwrap_or(0),
        duration_secs: num(item, "duration"),
        pubdate: num(item, "pubdate"),
        his_rank: stat.map(|s| num(s, "his_rank")).unwrap_or(0),
        up_from: num(item, "up_from_v2"),
        tname: str_of(item, "tname"),
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
            "note": "根据稿件内容质量、近期的数据综合展示，动态更新",
            "list": [
              {
                "aid": 117511731285323,
                "bvid": "BV1kMxM6FECM",
                "cid": 43107221723,
                "title": "旗舰手机芯片大横评 &amp; 能效实测",
                "pic": "http://i1.hdslb.com/bfs/archive/a.jpg",
                "duration": 3435,
                "pubdate": 1793087129,
                "tname": "数码",
                "up_from_v2": 8,
                "owner": {
                  "mid": 25876945,
                  "name": "极客湾Geekerwan",
                  "face": "http://i1.hdslb.com/bfs/face/b.jpg"
                },
                "stat": {"view": 2436575, "danmaku": 40763, "his_rank": 1}
              },
              {
                "aid": 2,
                "bvid": "BV2",
                "cid": 3,
                "title": "第二条",
                "pic": "https://i0.hdslb.com/bfs/archive/c.jpg",
                "duration": 60,
                "pubdate": 1793000000,
                "tname": "动画",
                "owner": {"mid": 9, "name": "UP", "face": ""},
                "stat": {"view": 12, "danmaku": 3, "his_rank": 0}
              }
            ]
          }
        }"#
    }

    #[test]
    fn parses_ranking_items() {
        let page = parse_ranking(sample_body()).expect("parse");
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.note, DEFAULT_NOTE);

        let first = &page.items[0];
        assert_eq!(first.rank, 1);
        assert_eq!(first.aid, 117511731285323);
        assert_eq!(first.bvid, "BV1kMxM6FECM");
        assert_eq!(first.cid, 43107221723);
        assert_eq!(first.title, "旗舰手机芯片大横评 & 能效实测");
        assert_eq!(first.up_name, "极客湾Geekerwan");
        assert_eq!(first.up_mid, 25876945);
        assert_eq!(first.view_count, 2436575);
        assert_eq!(first.danmaku_count, 40763);
        assert_eq!(first.his_rank, 1);
        assert_eq!(first.up_from, 8);
        assert_eq!(first.rise_label().as_deref(), Some("↑ 8"));
        assert_eq!(first.his_rank_label().as_deref(), Some("历史最高 #1"));
        assert_eq!(
            first.cover_thumb_url(),
            "https://i1.hdslb.com/bfs/archive/a.jpg@480w_270h_1c.webp"
        );
        assert_eq!(
            first.face_thumb_url(),
            "https://i1.hdslb.com/bfs/face/b.jpg@64w_64h_1c.webp"
        );

        assert_eq!(page.items[1].rank, 2, "名次按响应顺序补");
        assert!(page.items[1].rise_label().is_none());
        assert!(page.items[1].his_rank_label().is_none());
    }

    #[test]
    fn note_falls_back_when_missing() {
        let page = parse_ranking(r#"{"code":0,"data":{"list":[]}}"#).expect("parse");
        assert_eq!(page.note, DEFAULT_NOTE);
    }

    #[test]
    fn request_uses_rid_and_type_all() {
        let req = ranking_request(4, "SESSDATA=x");
        assert!(req.url.contains("/x/web-interface/ranking/v2"));
        assert!(req.url.contains("rid=4"));
        assert!(req.url.contains("type=all"));
        assert_eq!(req.cookie, "SESSDATA=x");
    }

    #[test]
    fn non_zero_code_is_an_error() {
        let err =
            parse_ranking(r#"{"code":-352,"message":"风控校验失败"}"#).expect_err("must fail");
        assert!(err.contains("-352"));
        assert!(err.contains("风控校验失败"));
    }
}
