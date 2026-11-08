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

/// 视频标签（不需要 WBI 签名）。信息面板的标签行用它。
pub fn video_tags(bvid: &str, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/tag/archive/tags?bvid={bvid}"),
        cookie: cookie.to_string(),
        name: "video_tags",
    }
}

/// 播放地址（需要 WBI 签名）。
///
/// `qn` 指定画质 id（127 = 最高档）；`fnval=4048` 打开 DASH（含 4K/HDR/杜比等位域）。
pub fn play_url(
    bvid: &str,
    cid: i64,
    qn: i32,
    img_key: &str,
    sub_key: &str,
    cookie: &str,
    wts: u64,
) -> Request {
    let params = vec![
        ("bvid".to_string(), bvid.to_string()),
        ("cid".to_string(), cid.to_string()),
        ("qn".to_string(), qn.to_string()),
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

/// 不带 WBI 签名的旧 playurl 端点。
///
/// **风控兜底**：同一账号、同一视频，WBI 端点可能返回
/// `code=0 + data.v_voucher` 的空响应，而旧端点仍能正常返回 DASH。
/// 实测 rustls 客户端在 WBI 端点稳定拿到 voucher、旧端点稳定成功。
pub fn play_url_legacy(bvid: &str, cid: i64, qn: i32, cookie: &str) -> Request {
    Request {
        url: format!(
            "{API_BASE}/x/player/playurl?bvid={bvid}&cid={cid}&qn={qn}&fnval=4048&fnver=0"
        ),
        cookie: cookie.to_string(),
        name: "play_url_legacy",
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

/// 评论分页（`/x/v2/reply/main`，游标翻页）。
///
/// 旧接口的 `pn` 翻页在线上已经失效（第 2 页起 `replies` 恒空），
/// 新版把游标放在 `cursor.next`，原样回传给 `next`；`cursor.is_end`
/// 表示「真的到底了」。
///
/// `ps` 线上上限是 30（再大也会被截到 30）；调用方只取一页
/// （[`super::client::COMMENT_PAGE_SIZE`] = 20），滚动到底再取下页。
/// **不要**在上层拼页凑「一次 100 条」：连续 4 个请求会被频控打成 `-352`。
pub fn comment_main(oid: &str, kind: i64, cookie: &str, next: u64, ps: u32) -> Request {
    Request {
        url: format!(
            "{API_BASE}/x/v2/reply/main?oid={oid}&type={kind}&mode=3&next={next}&ps={ps}&web_location=1315875"
        ),
        cookie: cookie.to_string(),
        name: "comment_main",
    }
}

/// 视频弹幕 XML（`/x/v1/dm/list.so`）。
///
/// `oid` 是视频当前分 P 的 cid。响应固定带 `Content-Encoding: deflate`，
/// 解压与解析分别在 `client::decode_danmaku_body` / `danmaku::DanmakuSet::from_xml`。
pub fn danmaku_xml(cid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/v1/dm/list.so?oid={cid}&type=1"),
        cookie: cookie.to_string(),
        name: "danmaku_xml",
    }
}

/// 分段弹幕（`/x/v2/dm/web/seg.so`）。
///
/// 一段 6 分钟（360 秒）：第 `index` 段覆盖 `[(index-1)*360, index*360)`，
/// `index` 从 1 开始。响应是 protobuf，不需要 WBI 签名。
/// `pid` 是 aid（AV 号），`oid` 是当前分 P 的 cid。
pub fn danmaku_segment(cid: i64, aid: i64, index: u64, cookie: &str) -> Request {
    Request {
        url: format!(
            "{API_BASE}/x/v2/dm/web/seg.so?type=1&oid={cid}&pid={aid}&segment_index={index}"
        ),
        cookie: cookie.to_string(),
        name: "danmaku_seg",
    }
}

// ================= 消息中心 =================

/// 私信 / 会话接口的域名。互动接口仍在 `api.bilibili.com` 下。
pub const MESSAGE_BASE: &str = "https://api.vc.bilibili.com";

/// 未读数（`/x/msgfeed/unread`）。
pub fn msgfeed_unread(cookie: &str) -> Request {
    Request {
        url: format!(
            "{API_BASE}/x/msgfeed/unread?platform=web&build=0&mobi_app=web"
        ),
        cookie: cookie.to_string(),
        name: "msgfeed_unread",
    }
}

/// 互动列表（回复 / @ / 赞）。三个通道共用一套游标参数。
///
/// `cursor.id` / `cursor.time` 来自上一页响应的 `data.cursor`；
/// 第一页传 0 / 0，`is_end` 判到底。
pub fn msgfeed_inbox(
    channel: crate::messages::MessageChannel,
    cursor: (i64, i64),
    cookie: &str,
) -> Request {
    let path = match channel {
        crate::messages::MessageChannel::Reply => "reply",
        crate::messages::MessageChannel::Mention => "at",
        crate::messages::MessageChannel::Like => "like",
        // 私信不走 msgfeed；调用方不应该走到这里。
        crate::messages::MessageChannel::Dm => "reply",
    };
    Request {
        url: format!(
            "{API_BASE}/x/msgfeed/{path}?platform=web&mobi_app=web&build=0&ps=30&id={}&time={}",
            cursor.0, cursor.1
        ),
        cookie: cookie.to_string(),
        name: "msgfeed_inbox",
    }
}

/// 私信会话列表：`session_type=9` 是「互相关注」档，本客户端只请求这一档。
pub fn message_sessions(begin_ts: Option<i64>, cookie: &str) -> Request {
    let begin = begin_ts
        .filter(|ts| *ts > 0)
        .map(|ts| format!("&begin_ts={ts}"))
        .unwrap_or_default();
    Request {
        url: format!(
            "{MESSAGE_BASE}/session_svr/v1/session_svr/get_sessions?session_type=9&group_fold=1&unfold_fold=0&sort_rule=2&build=0&mobi_app=web{begin}"
        ),
        cookie: cookie.to_string(),
        name: "message_sessions",
    }
}

/// 会话消息。`begin_seqno` 为空表示最新一页；更早一页传上一页的 `min_seqno`。
pub fn session_messages(talker_id: i64, begin_seqno: Option<i64>, size: u32, cookie: &str) -> Request {
    let begin = begin_seqno
        .filter(|seqno| *seqno > 0)
        .map(|seqno| format!("&begin_seqno={seqno}"))
        .unwrap_or_default();
    Request {
        url: format!(
            "{MESSAGE_BASE}/svr_sync/v1/svr_sync/fetch_session_msgs?talker_id={talker_id}&session_type=1&size={size}{begin}&build=0&mobi_app=web"
        ),
        cookie: cookie.to_string(),
        name: "session_messages",
    }
}

/// 用户名片（取 UP 主签名）。不需要 WBI 签名。
pub fn user_card(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/web-interface/card?mid={mid}"),
        cookie: cookie.to_string(),
        name: "user_card",
    }
}

/// 用户名片：补私信会话的昵称与头像。
pub fn user_cards(uids: &[i64], cookie: &str) -> Request {
    let joined = uids
        .iter()
        .map(|mid| mid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    Request {
        url: format!(
            "{MESSAGE_BASE}/account/v1/user/cards?uids={joined}&build=0&mobi_app=web"
        ),
        cookie: cookie.to_string(),
        name: "user_cards",
    }
}

/// 互关好友名单（E-24 的退路：进页拉一次，5 分钟 TTL）。
pub fn mutual_friends(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/relation/friends?fid={mid}&page=1&ps=50&order=desc"),
        cookie: cookie.to_string(),
        name: "mutual_friends",
    }
}

/// 单条关系（打开会话时再校验一次）：`attribute` 6 = 已互粉，128 = 已拉黑。
pub fn relation_attribute(fid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/relation?fid={fid}"),
        cookie: cookie.to_string(),
        name: "relation_attribute",
    }
}

/// 关注数 / 粉丝数（视频页 UP 主信息用；公开接口，未登录也能读）。
pub fn relation_stat(vmid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/relation/stat?vmid={vmid}"),
        cookie: cookie.to_string(),
        name: "relation_stat",
    }
}

/// 关注 / 取关的地址（写接口；表单在 `interaction::relation_modify_form` 里拼）。
pub fn relation_modify_url() -> String {
    format!("{API_BASE}/x/relation/modify")
}

/// 设备指纹（`buvid3` / `buvid4`）：写接口的风控会看这两个 cookie。
pub fn finger_spi(cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/frontend/finger/spi"),
        cookie: cookie.to_string(),
        name: "finger_spi",
    }
}

// ================= UP 主空间 =================

/// UP 主空间资料（`/x/space/wbi/acc/info`，**需要** WBI 签名）。
///
/// 这是空间页唯一必需的接口：昵称、头像、签名、等级、认证、会员档位都在这里。
/// 老接口 `/x/space/acc/info`：不带 wbi，也不带 dm_img_* 反爬参数。
///
/// `x/space/wbi/acc/info` 少一个反爬参数就整体回 `-352`，连
/// `jointime` / `top_photo` 都拿不到；这个老端点在多数账号上仍然可用，
/// 所以 wbi 失败时回落到它。
pub fn space_acc_info_legacy(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/space/acc/info?mid={mid}"),
        cookie: cookie.to_string(),
        name: "space_acc_info_legacy",
    }
}

pub fn space_acc_info(mid: i64, img_key: &str, sub_key: &str, cookie: &str, wts: u64) -> Request {
    // 这组 `dm_*` 是**反爬指纹**，不是业务参数：少任何一个，wbi 端点整体回 -352，
    // 连 `jointime` / `top_photo` 都拿不到。取值照抄 PiliPlus
    // （`lib/http/member.dart` 的 member view 请求）：前两个用固定串即可，
    // 后两个是 WebGL 渲染器/显卡的 base64 指纹。
    let params = vec![
        ("mid".to_string(), mid.to_string()),
        ("platform".to_string(), "web".to_string()),
        ("web_location".to_string(), "1550101".to_string()),
        ("dm_img_list".to_string(), "[]".to_string()),
        (
            "dm_img_str".to_string(),
            "V2ViR0wgMS4wIChPcGVuR0wgRVMgMi4wIENocm9taXVtKQ".to_string(),
        ),
        (
            "dm_cover_img_str".to_string(),
            "QU5HTEUgKE5WSURJQSwgTlZJRElBIEdlRm9yY2UgR1RYIDEwNjAgNkdCIERpcmVjdDNEMTEgdnNfNV8wIHBzXzVfMCwgRDNEMTEpR29vZ2xlIEluYy4gKE5WSURJQS".to_string(),
        ),
        (
            "dm_img_inter".to_string(),
            r#"{"ds":[],"wh":[0,0,0],"of":[0,0,0]}"#.to_string(),
        ),
    ];
    Request {
        url: wbi::signed_url(
            &format!("{API_BASE}/x/space/wbi/acc/info"),
            &params,
            img_key,
            sub_key,
            wts,
        ),
        cookie: cookie.to_string(),
        name: "space_acc_info",
    }
}

/// UP 主空间标签（`/x/space/acc/tags`，不需要签名）。
///
/// 服务端最多给 5 条、每条不足 10 字；内容可能带 HTML 转义。
pub fn space_tags(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/space/acc/tags?mid={mid}"),
        cookie: cookie.to_string(),
        name: "space_tags",
    }
}

/// UP 主总播放 / 获赞（`/x/space/upstat`，不需要签名）。
///
/// 未登录时整块 `data` 可能不返回 —— 调用方按 0 处理。
pub fn space_upstat(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/space/upstat?mid={mid}"),
        cookie: cookie.to_string(),
        name: "space_upstat",
    }
}

/// UP 主投稿列表（`/x/space/wbi/arc/search`，**需要** WBI 签名）。
///
/// `order` 只认 `pubdate` / `click` / `stow`（见 [`crate::space::SpaceOrder`]）。
pub fn space_arc_search(
    mid: i64,
    pn: u32,
    ps: u32,
    order: &str,
    img_key: &str,
    sub_key: &str,
    cookie: &str,
    wts: u64,
) -> Request {
    let params = vec![
        ("mid".to_string(), mid.to_string()),
        ("ps".to_string(), ps.to_string()),
        ("pn".to_string(), pn.to_string()),
        ("order".to_string(), order.to_string()),
    ];
    Request {
        url: wbi::signed_url(
            &format!("{API_BASE}/x/space/wbi/arc/search"),
            &params,
            img_key,
            sub_key,
            wts,
        ),
        cookie: cookie.to_string(),
        name: "space_arc_search",
    }
}

/// 置顶稿件（`/x/space/top/arc?mid=`，不需要签名）。
///
/// `code=53016` 表示该 UP 主没有置顶，属正常状态；调用方应把它当 `None`。
pub fn space_top_arc(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/space/top/arc?mid={mid}"),
        cookie: cookie.to_string(),
        name: "space_top_arc",
    }
}

/// UP 主公开名片（`/x/web-interface/card?mid=`，不需要签名）。
///
/// `photo=false` 省掉头图的尺寸协商；名片一个请求就带齐昵称、等级、
/// 认证、会员、粉丝数、投稿数与获赞。
pub fn space_card(mid: i64, cookie: &str) -> Request {
    Request {
        url: format!("{API_BASE}/x/web-interface/card?mid={mid}&photo=false"),
        cookie: cookie.to_string(),
        name: "space_card",
    }
}

/// UP 主空间动态流（`/x/polymer/web-dynamic/v1/feed/space`，不需要签名）。
///
/// `offset` 是上一页 `data.offset` 原样回传的游标，空串表示第一页；
/// `features=itemOpusStyle` 打开新版图文（Opus）结构。
pub fn space_dynamics(mid: i64, offset: &str, cookie: &str) -> Request {
    Request {
        url: format!(
            "{API_BASE}/x/polymer/web-dynamic/v1/feed/space?host_mid={mid}&offset={}&timezone_offset=-480&platform=web&features=itemOpusStyle",
            offset.trim()
        ),
        cookie: cookie.to_string(),
        name: "space_dynamics",
    }
}

/// UP 主专栏列表（`/x/space/wbi/article`，**需要** WBI 签名）。
///
/// `pn` 从 1 开始，`order=pubdate` 按发布时间倒序。
pub fn space_articles(
    mid: i64,
    pn: u32,
    ps: u32,
    img_key: &str,
    sub_key: &str,
    cookie: &str,
    wts: u64,
) -> Request {
    let params = vec![
        ("mid".to_string(), mid.to_string()),
        ("pn".to_string(), pn.to_string()),
        ("ps".to_string(), ps.to_string()),
        ("order".to_string(), "pubdate".to_string()),
    ];
    Request {
        url: wbi::signed_url(
            &format!("{API_BASE}/x/space/wbi/article"),
            &params,
            img_key,
            sub_key,
            wts,
        ),
        cookie: cookie.to_string(),
        name: "space_articles",
    }
}

/// 发送请求的能力抽象。
///
/// 目前没有接具体实现：离线环境里没有可用的 HTTP crate。
/// 接入时实现这个 trait 即可，`yk-app` 不需要关心用的是 reqwest 还是别的。
pub trait HttpClient {
    type Error;
    fn get(&self, req: &Request) -> Result<String, Self::Error>;

    /// 已缓存的 WBI keys（`img_key`, `sub_key`）。
    ///
    /// 只有真实客户端会实现：`/x/web-interface/nav` 被所有 WBI 接口共用，
    /// 每发一个签名请求就重取一次既浪费又容易触发风控。测试替身保持
    /// 默认实现（不缓存），这样单测仍然能验证「每次都会去取 nav」。
    fn cached_wbi_keys(&self) -> Option<(String, String)> {
        None
    }

    /// 把刚取到的 WBI keys 存进缓存（默认不缓存）。
    fn store_wbi_keys(&self, _img: &str, _sub: &str) {}
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
        let p = play_url("BV1xx411c7mD", 12345, 127, IMG, SUB, "", WTS);
        assert!(p.url.contains("cid=12345"));
        assert!(p.url.contains("fnval=4048"));
        assert!(p.url.contains("qn=127"));

        // 风控兜底：不带 w_rid 的旧端点，参数与主请求不同。
        let f = play_url_legacy("BV1xx411c7mD", 12345, 80, "");
        assert!(!f.url.contains("w_rid"));
        assert!(f.url.contains("/x/player/playurl?"));
        assert!(f.url.contains("fnval=4048"));
        assert!(f.url.contains("qn=80"));
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

    #[test]
    fn msgfeed_urls_carry_channel_and_cursor() {
        use crate::messages::MessageChannel;
        let reply = msgfeed_inbox(MessageChannel::Reply, (12, 34), "SESSDATA=x");
        assert!(reply.url.contains("/x/msgfeed/reply?"));
        assert!(reply.url.contains("id=12"));
        assert!(reply.url.contains("time=34"));
        let mention = msgfeed_inbox(MessageChannel::Mention, (0, 0), "");
        assert!(mention.url.contains("/x/msgfeed/at?"));
        let like = msgfeed_inbox(MessageChannel::Like, (0, 0), "");
        assert!(like.url.contains("/x/msgfeed/like?"));
        let unread = msgfeed_unread("SESSDATA=x");
        assert!(unread.url.contains("/x/msgfeed/unread"));
    }

    #[test]
    fn dm_urls_carry_optional_cursors() {
        let sessions = message_sessions(None, "SESSDATA=x");
        assert!(sessions.url.contains("session_type=9"));
        assert!(!sessions.url.contains("begin_ts="));
        let older = message_sessions(Some(1_700_000_000_123_456), "");
        assert!(older.url.contains("begin_ts=1700000000123456"));

        let latest = session_messages(77, None, 20, "");
        assert!(latest.url.contains("talker_id=77"));
        assert!(latest.url.contains("size=20"));
        assert!(!latest.url.contains("begin_seqno="));
        let older_msgs = session_messages(77, Some(123), 20, "");
        assert!(older_msgs.url.contains("begin_seqno=123"));

        let cards = user_cards(&[1, 2, 3], "");
        assert!(cards.url.contains("uids=1,2,3"));
    }

    #[test]
    fn danmaku_segment_url_uses_cid_aid_and_index() {
        let req = danmaku_segment(12345, 67890, 3, "SESSDATA=x");
        assert_eq!(
            req.url,
            "https://api.bilibili.com/x/v2/dm/web/seg.so?type=1&oid=12345&pid=67890&segment_index=3"
        );
        assert_eq!(req.name, "danmaku_seg");
        assert!(!req.url.contains("w_rid"), "分段弹幕不需要 WBI 签名");

        let first = danmaku_segment(1, 2, 1, "");
        assert!(first.url.contains("segment_index=1"));
    }

}
