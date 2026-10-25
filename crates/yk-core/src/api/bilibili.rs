//! B 站接口的请求构造。
//!
//! 对应旧 `BilibiliApi`。这一层**只产出「请求长什么样」**，不发网络请求：
//! - URL、参数、需不需要 Cookie、需不需要 WBI 签名；
//! - 发送与解析分别由 `HttpClient` trait 和 `api::models` 承担。
//!
//! 这样拆分的好处：签名与 URL 构造是**纯函数**，可以完整单测，
//! 不必依赖网络，也不会因为接口限流把测试跑红。

use super::wbi;

pub const API_BASE: &str = "https://api.bilibili.com";
pub const PASSPORT_BASE: &str = "https://passport.bilibili.com";

/// 一个待发送的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    /// 需要随请求带的 Cookie 头（没登录就是空）。
    pub cookie: String,
    /// 人类可读的接口名，用于日志与错误文案。
    pub name: &'static str,
}

/// 推荐流（需要 WBI 签名）。
///
/// `fresh_type=3` 是 APP 推荐流，`ps=30` 是每页数量 —— 与旧实现一致。
pub fn recommend(img_key: &str, sub_key: &str, cookie: &str, wts: u64, ps: u32) -> Request {
    recommend_page(img_key, sub_key, cookie, wts, ps, 0)
}

/// 推荐流分页：`fresh_idx` / `fresh_idx_1h` / `brush` 一起递增，
/// 否则服务端可能反复返回同一批内容。
pub fn recommend_page(
    img_key: &str,
    sub_key: &str,
    cookie: &str,
    wts: u64,
    ps: u32,
    fresh_idx: u32,
) -> Request {
    let params = vec![
        ("fresh_type".to_string(), "3".to_string()),
        ("version".to_string(), "1".to_string()),
        ("ps".to_string(), ps.to_string()),
        ("fresh_idx".to_string(), fresh_idx.to_string()),
        ("fresh_idx_1h".to_string(), fresh_idx.to_string()),
        ("brush".to_string(), fresh_idx.to_string()),
        ("homepage_ver".to_string(), "1".to_string()),
    ];
    Request {
        url: wbi::signed_url(
            &format!("{API_BASE}/x/web-interface/wbi/index/top/feed/rcmd"),
            &params,
            img_key,
            sub_key,
            wts,
        ),
        cookie: cookie.to_string(),
        name: "recommend",
    }
}

/// 视频详情（需要 WBI 签名）。
pub fn video_info(bvid: &str, img_key: &str, sub_key: &str, cookie: &str, wts: u64) -> Request {
    let params = vec![("bvid".to_string(), bvid.to_string())];
    Request {
        url: wbi::signed_url(
            &format!("{API_BASE}/x/web-interface/wbi/view"),
            &params,
            img_key,
            sub_key,
            wts,
        ),
        cookie: cookie.to_string(),
        name: "video_info",
    }
}

/// 播放地址（需要 WBI 签名）。
///
/// `qn=127` 请求最高画质；`fnval=4048` 打开 DASH（含 4K/HDR/杜比等位域）。
pub fn play_url(
    bvid: &str,
    cid: i64,
    img_key: &str,
    sub_key: &str,
    cookie: &str,
    wts: u64,
) -> Request {
    let params = vec![
        ("bvid".to_string(), bvid.to_string()),
        ("cid".to_string(), cid.to_string()),
        ("qn".to_string(), "127".to_string()),
        ("fnval".to_string(), "4048".to_string()),
        ("fnver".to_string(), "0".to_string()),
    ];
    Request {
        url: wbi::signed_url(
            &format!("{API_BASE}/x/player/wbi/playurl"),
            &params,
            img_key,
            sub_key,
            wts,
        ),
        cookie: cookie.to_string(),
        name: "play_url",
    }
}

/// 扫码登录：申请二维码（**不需要**签名与 Cookie）。
pub fn qrcode_generate() -> Request {
    Request {
        url: format!("{PASSPORT_BASE}/x/passport-login/web/qrcode/generate"),
        cookie: String::new(),
        name: "qrcode_generate",
    }
}

/// 扫码登录：轮询状态（需要 `qrcode_key`）。
pub fn qrcode_poll(qrcode_key: &str) -> Request {
    Request {
        url: format!("{PASSPORT_BASE}/x/passport-login/web/qrcode/poll?qrcode_key={qrcode_key}"),
        cookie: String::new(),
        name: "qrcode_poll",
    }
}

/// 当前登录用户信息（需要 Cookie；用于拿 `wbi_img` 里的 img_key / sub_key）。
pub fn user_info(cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/web-interface/nav"),
        cookie: cookie.to_string(),
        name: "user_info",
    }
}

/// 从 `wbi_img.img_url` / `sub_url` 里提取文件名（去掉路径与扩展名）。
///
/// 接口返回的是完整 URL，例如
/// `https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png`，
/// 签名需要的是最后那段 `7cd084941338484aae1ad9425b84077c`。
pub fn extract_wbi_key(url: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or(url);
    last.split('.').next().unwrap_or(last).to_string()
}

/// 关注动态流（`web-dynamic/feed/all`）。
///
/// 需要 SESSDATA；`features` 打开 Opus 大图与完整装饰卡，否则新版图文动态会
/// 退回成只有文字的旧版结构。`offset` 为空表示第一页。
pub fn following_dynamics(offset: &str, cookie: &str) -> Request {
    const FEATURES: &str =
        "itemOpusStyle,listOnlyfans,opusBigCover,onlyfansVote,decorationCard,commentsNewVersion";
    let offset = if offset.trim().is_empty() {
        String::new()
    } else {
        format!("&offset={}", offset.trim())
    };
    Request {
        url: format!(
            "{API_BASE}/x/polymer/web-dynamic/v1/feed/all?timezone_offset=-480&type=all&page=1{offset}&features={FEATURES}&platform=web"
        ),
        cookie: cookie.to_string(),
        name: "following_dynamics",
    }
}

/// 观看历史一页（`GET /x/web-interface/history/cursor`）。
///
/// 只认 Cookie，不需要 WBI 签名。`max` / `view_at` 是第一页响应里
/// `data.cursor` 回传的两个游标；首次请求都传 0。
pub fn history_cursor(
    max: i64,
    view_at: i64,
    business: &str,
    cookie: &str,
) -> Request {
    let business = business.trim();
    let business = if business.is_empty() {
        String::new()
    } else {
        format!("&business={business}")
    };
    Request {
        url: format!(
            "{API_BASE}/x/web-interface/history/cursor?max={max}&view_at={view_at}{business}"
        ),
        cookie: cookie.to_string(),
        name: "history_cursor",
    }
}

/// 动态评论第一页。
///
/// `kind` 来自动态条目的 `basic.comment_type`（图文 11、纯文字 17、视频 1）；
/// `oid` 是 `basic.comment_id_str`。
pub fn dynamic_comments(oid: &str, kind: i64, cookie: &str, pn: u32, ps: u32) -> Request {
    Request {
        url: format!("{API_BASE}/x/v2/reply?type={kind}&oid={oid}&sort=2&nohot=0&pn={pn}&ps={ps}"),
        cookie: cookie.to_string(),
        name: "dynamic_comments",
    }
}

/// 发送请求的能力抽象。
///
/// 目前没有接具体实现：离线环境里没有可用的 HTTP crate。
/// 接入时实现这个 trait 即可，`yk-app` 不需要关心用的是 reqwest 还是别的。
pub trait HttpClient {
    type Error;
    fn get(&self, req: &Request) -> Result<String, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG: &str = "7cd084941338484aae1ad9425b84077c";
    const SUB: &str = "4932caff0ff746eab6f01bf08b70ac45";
    const WTS: u64 = 1_702_204_169;

    #[test]
    fn recommend_is_signed_and_has_expected_params() {
        let r = recommend(IMG, SUB, "SESSDATA=x", WTS, 30);
        assert_eq!(r.name, "recommend");
        assert!(r
            .url
            .starts_with("https://api.bilibili.com/x/web-interface/wbi/index/top/feed/rcmd?"));
        assert!(r.url.contains("fresh_type=3"));
        assert!(r.url.contains("ps=30"));
        assert!(r.url.contains("wts=1702204169"));
        assert!(r.url.contains("w_rid="));
        assert_eq!(r.cookie, "SESSDATA=x");
    }

    #[test]
    fn recommend_pages_change_fresh_idx() {
        let p0 = recommend_page(IMG, SUB, "", WTS, 30, 0);
        let p2 = recommend_page(IMG, SUB, "", WTS, 30, 2);
        assert!(p0.url.contains("fresh_idx=0"));
        assert!(p2.url.contains("fresh_idx=2"));
        assert!(p2.url.contains("brush=2"));
        assert_ne!(p0.url, p2.url);
    }

    #[test]
    fn video_endpoints_carry_bvid() {
        let v = video_info("BV1xx411c7mD", IMG, SUB, "", WTS);
        assert!(v.url.contains("bvid=BV1xx411c7mD"));
        let p = play_url("BV1xx411c7mD", 12345, IMG, SUB, "", WTS);
        assert!(p.url.contains("cid=12345"));
        assert!(p.url.contains("fnval=4048"));
        assert!(p.url.contains("qn=127"));
    }

    #[test]
    fn qrcode_endpoints_need_no_signature() {
        let g = qrcode_generate();
        assert!(!g.url.contains("w_rid"));
        assert!(g.url.contains("/qrcode/generate"));
        let p = qrcode_poll("abc123");
        assert!(p.url.contains("qrcode_key=abc123"));
        assert!(!p.url.contains("w_rid"));
    }

    #[test]
    fn wbi_key_extraction() {
        assert_eq!(
            extract_wbi_key("https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"),
            "7cd084941338484aae1ad9425b84077c"
        );
        assert_eq!(extract_wbi_key("abc.png"), "abc");
        assert_eq!(extract_wbi_key(""), "");
    }

    /// 不同 wts 必须产出不同签名，否则等于没签名。
    #[test]
    fn signature_changes_with_timestamp() {
        let a = recommend(IMG, SUB, "", WTS, 30);
        let b = recommend(IMG, SUB, "", WTS + 1, 30);
        let rid = |s: &str| s.split("w_rid=").nth(1).unwrap().to_string();
        assert_ne!(rid(&a.url), rid(&b.url));
    }
}
