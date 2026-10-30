//! B 站接口的访问节流。
//!
//! **为什么需要**：B 站的风控按「同一类接口的请求密度」判定，短时间连发
//! 就会回 `-352`（评论）/ `-412`（请求被拦截）。客户端单次请求再小，
//! 只要频率上去了照样封；评论区的 `-352` 就是「一批 100 条 = 4 个连发」撞的。
//!
//! **怎么限**：按接口**分组**给最小间隔（[`Bucket`]），所有真实请求在
//! [`super::client::ReqwestClient`] 里统一过 [`acquire`]：
//! - 每组第一次请求**不等待**，所以打开页面 / 起播不会因为限流变慢；
//! - 同组第二次起按间隔排队（先占未来的时间片再睡，避免多线程同时醒来一起发）；
//! - 另有一条很短的全局间隔（[`GLOBAL_GAP`]），只用来把同一瞬间的并发
//!   请求排开，不作为主要限流手段。
//!
//! **不在这一层做的**：
//! - 测试替身（`HttpClient` 的假实现）不走网络，不经过这里；
//! - 图床（`i0.hdslb.com` 等）走 GPUI 的图片客户端，不是接口，另算。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 同一瞬间允许挤在一起的请求之间的最小间隔。
///
/// 只用来「排队」：一次页面加载会自然并发 3–10 个不同接口的请求，
/// 错开 80ms 既不影响体验，也不会在同一毫秒打出十几个连接。
pub const GLOBAL_GAP: Duration = Duration::from_millis(80);

/// 接口分组。同一组的请求共用一条时间线。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// 评论区（`/x/v2/reply`、`/x/v2/reply/main`），带楼中楼，响应最大。
    Comment,
    /// 搜索联想词（`s.search.bilibili.com/main/suggest`）。
    Suggest,
    /// 搜索接口（`/x/web-interface/wbi/search/*`）。
    Search,
    /// 搜索热搜词。
    Hotword,
    /// WBI 密钥（`/x/web-interface/nav`）；有缓存，正常很少发。
    Nav,
    /// 首页 / 推荐流。
    Recommend,
    /// 视频详情（`/x/web-interface/*view`）。
    VideoInfo,
    /// 取流（`/x/player/*playurl`）。
    PlayUrl,
    /// 弹幕 XML。
    Danmaku,
    /// 动态流（`web-dynamic` / `dynamic_svr`）。
    Dynamic,
    /// 观看历史。
    History,
    /// 消息中心（`/x/msgfeed/*`）。
    MsgFeed,
    /// 私信会话与消息（`api.vc.bilibili.com`）。
    Session,
    /// 关注 / 粉丝关系。
    Relation,
    /// 排行榜。
    Ranking,
    /// 收藏夹。
    Fav,
    /// 用户名片 / 空间。
    UserCards,
    /// 登录相关（passport：二维码、短信、密码登录）。
    Passport,
    /// 所有写操作（点赞 / 投币 / 收藏 / 发动态 / 发私信 / 删历史 / 上报进度）。
    Write,
    /// 未归类的读接口兜底。
    Other,
}

impl Bucket {
    /// 同一组两次请求之间的最小间隔。
    ///
    /// 取值口径：越是「重」、越靠近风控的接口，间隔越大；
    /// 读多写少、本身就是用户手动触发的可以小一点。
    pub fn interval(self) -> Duration {
        match self {
            // 评论区是重灾区：一次响应就 ~270KB，连发必 -352。
            Bucket::Comment => Duration::from_millis(1200),
            // 联想词跟着输入走，250ms 防抖之后再来一道闸。
            Bucket::Suggest => Duration::from_millis(300),
            Bucket::Search => Duration::from_millis(500),
            Bucket::Hotword => Duration::from_millis(1000),
            // nav 被所有 WBI 接口复用，靠缓存兜底；真发的时候慢一点。
            Bucket::Nav => Duration::from_millis(1000),
            Bucket::Recommend => Duration::from_millis(800),
            Bucket::VideoInfo => Duration::from_millis(400),
            Bucket::PlayUrl => Duration::from_millis(600),
            Bucket::Danmaku => Duration::from_millis(400),
            Bucket::Dynamic => Duration::from_millis(800),
            Bucket::History => Duration::from_millis(500),
            // 消息中心：三个通道 + 未读数都是同一族接口，别挤在一起。
            Bucket::MsgFeed => Duration::from_millis(700),
            Bucket::Session => Duration::from_millis(700),
            Bucket::Relation => Duration::from_millis(400),
            Bucket::Ranking => Duration::from_millis(600),
            Bucket::Fav => Duration::from_millis(600),
            Bucket::UserCards => Duration::from_millis(400),
            Bucket::Passport => Duration::from_millis(1000),
            // 写操作最容易被风控：1.2 秒一次，手动点也够用。
            Bucket::Write => Duration::from_millis(1200),
            Bucket::Other => Duration::from_millis(300),
        }
    }

    /// 日志用的短名。
    pub fn label(self) -> &'static str {
        match self {
            Bucket::Comment => "comment",
            Bucket::Suggest => "suggest",
            Bucket::Search => "search",
            Bucket::Hotword => "hotword",
            Bucket::Nav => "nav",
            Bucket::Recommend => "recommend",
            Bucket::VideoInfo => "video_info",
            Bucket::PlayUrl => "playurl",
            Bucket::Danmaku => "danmaku",
            Bucket::Dynamic => "dynamic",
            Bucket::History => "history",
            Bucket::MsgFeed => "msgfeed",
            Bucket::Session => "session",
            Bucket::Relation => "relation",
            Bucket::Ranking => "ranking",
            Bucket::Fav => "fav",
            Bucket::UserCards => "user_cards",
            Bucket::Passport => "passport",
            Bucket::Write => "write",
            Bucket::Other => "other",
        }
    }
}

/// 把 URL 归类到 [`Bucket`]。写操作优先（POST 一律进 `Write`）。
///
/// 只认 host + path，不解析 query —— 参数再多也不影响归类，还省一次解析。
pub fn bucket_for(url: &str, write: bool) -> Bucket {
    if write {
        return Bucket::Write;
    }
    let path = url.split('?').next().unwrap_or(url);
    let host = path
        .split("//")
        .nth(1)
        .map(|rest| rest.split('/').next().unwrap_or(""))
        .unwrap_or("");

    // 搜索联想 / 热搜在独立域名上，先按 host 判。
    if host == "s.search.bilibili.com" {
        return if path.contains("/hotword") {
            Bucket::Hotword
        } else {
            Bucket::Suggest
        };
    }
    if host == "passport.bilibili.com" || path.contains("/passport-login/") {
        return Bucket::Passport;
    }
    if host == "api.vc.bilibili.com"
        || path.contains("/session_svr/")
        || path.contains("/svr_sync/")
        || path.contains("/web_im/")
    {
        return Bucket::Session;
    }

    // 顺序有讲究：/x/v2/reply 要在 /x/v2/ 之前判，search 要在 web-interface 之前判。
    if path.contains("/x/v2/reply") || path.contains("/x/v2/feed/reply") {
        Bucket::Comment
    } else if path.contains("/search/") {
        Bucket::Search
    } else if path.contains("/x/web-interface/nav") {
        Bucket::Nav
    } else if path.contains("/feed/rcmd") || path.contains("/feed/index") {
        Bucket::Recommend
    } else if path.contains("/x/player/") {
        Bucket::PlayUrl
    } else if path.contains("/x/v1/dm/") || path.contains("/x/v2/dm/") {
        Bucket::Danmaku
    } else if path.contains("/web-interface/wbi/view")
        || path.contains("/web-interface/view")
        || path.contains("/web-interface/archive/")
    {
        // `archive/has/like`、`archive/coins` 这类读接口也在这一族；
        // `archive/like` 是写操作，POST 已在函数开头分流到 `Write`。
        Bucket::VideoInfo
    } else if path.contains("/web-interface/history") || path.contains("/x/v2/history/") {
        Bucket::History
    } else if path.contains("/x/msgfeed/") {
        Bucket::MsgFeed
    } else if path.contains("web-dynamic")
        || path.contains("dynamic_svr")
        || path.contains("/x/dynamic/")
    {
        Bucket::Dynamic
    } else if path.contains("/x/relation") || path.contains("/x/space/") {
        Bucket::Relation
    } else if path.contains("/ranking") {
        Bucket::Ranking
    } else if path.contains("/x/v3/fav")
        || path.contains("/x/v2/fav")
        || path.contains("/fav/resource")
    {
        Bucket::Fav
    } else if path.contains("/web-interface/card")
        || path.contains("/x/web-interface/space")
        || path.contains("/x/member/")
    {
        Bucket::UserCards
    } else {
        Bucket::Other
    }
}

struct State {
    /// 每组「下一个可用时间片」。
    last: HashMap<Bucket, Instant>,
    /// 全局「下一个可用时间片」。
    global: Option<Instant>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            last: HashMap::new(),
            global: None,
        })
    })
}

/// 算一个时间片：不早于 `now`，不早于同组上次 + 间隔，也不早于全局上次 + 间隔。
///
/// 抽成纯函数是为了能单测「排队」规则，不用真的睡。
fn next_slot(
    last_group: Option<Instant>,
    last_global: Option<Instant>,
    interval: Duration,
    now: Instant,
) -> Instant {
    let mut slot = now;
    if let Some(last) = last_group {
        slot = slot.max(last + interval);
    }
    if let Some(last) = last_global {
        slot = slot.max(last + GLOBAL_GAP);
    }
    slot
}

/// 排队并等待到自己那一格。返回实际等待时长。
///
/// 分两步，顺序很重要：
/// 1. **组内排队**：在锁内把该组的时间线推到未来（占位），再睡到那一格。
///    多个线程同时调用会依次排开，而不是一起醒来同时发包。
/// 2. **全局排队**：睡醒之后才占全局的一格。全局时间线必须只记录
///    「真正开始发出去的请求」—— 如果第 1 步的远期占位也写进全局，
///    一次 1.2 秒后的评论请求会把之后所有别的接口都顶到 1.2 秒开外。
pub fn acquire(bucket: Bucket) -> Duration {
    let started = Instant::now();

    let slot = {
        let mut state = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = next_slot(
            state.last.get(&bucket).copied(),
            None,
            bucket.interval(),
            started,
        );
        state.last.insert(bucket, slot);
        slot
    };
    sleep_until(slot);

    let slot = {
        let mut state = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let slot = next_slot(None, state.global, GLOBAL_GAP, now);
        state.global = Some(slot);
        slot
    };
    sleep_until(slot);

    let waited = started.elapsed();
    if waited >= Duration::from_millis(300) {
        eprintln!(
            "[api] 节流 {}：等待 {}ms",
            bucket.label(),
            waited.as_millis()
        );
    }
    waited
}

fn sleep_until(slot: Instant) {
    let wait = slot.saturating_duration_since(Instant::now());
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

/// 按 URL 归类后排队。`write = true` 表示这是写操作（POST）。
pub fn acquire_for(url: &str, write: bool) -> Duration {
    acquire(bucket_for(url, write))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://api.bilibili.com";

    #[test]
    fn buckets_cover_the_real_endpoints() {
        let cases = [
            (
                format!("{BASE}/x/v2/reply/main?oid=1&type=1&next=0&ps=20"),
                Bucket::Comment,
            ),
            (
                format!("{BASE}/x/v2/reply?type=11&oid=2&pn=1"),
                Bucket::Comment,
            ),
            (
                "https://s.search.bilibili.com/main/suggest?term=bili".into(),
                Bucket::Suggest,
            ),
            (
                "https://s.search.bilibili.com/main/hotword?limit=10".into(),
                Bucket::Hotword,
            ),
            (
                format!("{BASE}/x/web-interface/wbi/search/type?keyword=x"),
                Bucket::Search,
            ),
            (format!("{BASE}/x/web-interface/nav"), Bucket::Nav),
            (
                format!("{BASE}/x/web-interface/wbi/index/top/feed/rcmd?ps=30"),
                Bucket::Recommend,
            ),
            (
                format!("{BASE}/x/web-interface/wbi/view?bvid=BV1"),
                Bucket::VideoInfo,
            ),
            (
                format!("{BASE}/x/web-interface/archive/has/like?aid=1"),
                Bucket::VideoInfo,
            ),
            (
                format!("{BASE}/x/player/wbi/playurl?bvid=BV1&cid=2"),
                Bucket::PlayUrl,
            ),
            (
                format!("{BASE}/x/v1/dm/list.so?oid=123&type=1"),
                Bucket::Danmaku,
            ),
            (
                format!("{BASE}/x/polymer/web-dynamic/v1/feed/all?page=1"),
                Bucket::Dynamic,
            ),
            (
                format!("{BASE}/x/web-interface/history/cursor?max=0&view_at=0"),
                Bucket::History,
            ),
            (format!("{BASE}/x/msgfeed/unread"), Bucket::MsgFeed),
            (
                "https://api.vc.bilibili.com/session_svr/v1/session_svr/get_sessions".into(),
                Bucket::Session,
            ),
            (
                format!("{BASE}/x/relation/friends?fid=1&page=1"),
                Bucket::Relation,
            ),
            (
                format!("{BASE}/x/web-interface/ranking/v2?rid=0"),
                Bucket::Ranking,
            ),
            (
                format!("{BASE}/x/v3/fav/resource/list?media_id=1&pn=1"),
                Bucket::Fav,
            ),
            (
                format!("{BASE}/x/web-interface/card?mid=1"),
                Bucket::UserCards,
            ),
            (
                "https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key=k"
                    .into(),
                Bucket::Passport,
            ),
            (format!("{BASE}/x/whatever"), Bucket::Other),
        ];
        for (url, expected) in cases {
            assert_eq!(bucket_for(&url, false), expected, "归类错误：{url}");
        }
    }

    #[test]
    fn writes_always_go_to_the_write_bucket() {
        // 点赞 / 投币 / 收藏 / 删历史都是同一个 URL 家族，但方法不同：
        // 写操作不能再按路径分流，否则点赞会被当成「视频详情」。
        for url in [
            format!("{BASE}/x/web-interface/archive/like"),
            format!("{BASE}/x/web-interface/coin/add"),
            format!("{BASE}/x/v3/fav/resource/deal"),
            format!("{BASE}/x/v2/history/report"),
        ] {
            assert_eq!(bucket_for(&url, true), Bucket::Write, "{url}");
        }
    }

    #[test]
    fn heavier_buckets_are_slower() {
        assert!(Bucket::Comment.interval() > Bucket::Search.interval());
        assert!(Bucket::Write.interval() > Bucket::VideoInfo.interval());
        assert!(Bucket::PlayUrl.interval() >= Bucket::Other.interval());
        // 每个桶的间隔都要比全局排队间隔大，否则全局间隔反而成了主限制。
        for bucket in [Bucket::Comment, Bucket::Suggest, Bucket::Other] {
            assert!(bucket.interval() >= GLOBAL_GAP);
        }
    }

    #[test]
    fn first_request_is_never_delayed() {
        let now = Instant::now();
        assert_eq!(next_slot(None, None, Bucket::Comment.interval(), now), now);
        // 全局也没有历史时同样立即返回。
        assert_eq!(next_slot(None, None, Bucket::Write.interval(), now), now);
    }

    #[test]
    fn same_bucket_waits_for_its_interval() {
        let now = Instant::now();
        let last = now;
        let slot = next_slot(Some(last), None, Bucket::Comment.interval(), now);
        assert_eq!(slot, last + Bucket::Comment.interval());
    }

    #[test]
    fn global_gap_spaces_out_different_buckets() {
        let now = Instant::now();
        let slot = next_slot(None, Some(now), Bucket::Other.interval(), now);
        assert_eq!(slot, now + GLOBAL_GAP);
    }

    #[test]
    fn stale_history_does_not_add_delay() {
        let now = Instant::now();
        let long_ago = now - Duration::from_secs(30);
        let slot = next_slot(
            Some(long_ago),
            Some(long_ago),
            Bucket::Comment.interval(),
            now,
        );
        assert_eq!(slot, now, "间隔早就过了，应立即放行");
    }

    #[test]
    fn queue_slots_do_not_overlap() {
        // 连续占 3 格：每格都必须比上一格晚「间隔」，模拟并发排队。
        let now = Instant::now();
        let mut last = None;
        let mut slots = Vec::new();
        for _ in 0..3 {
            let slot = next_slot(last, None, Bucket::MsgFeed.interval(), now);
            slots.push(slot);
            last = Some(slot);
        }
        assert_eq!(slots[1] - slots[0], Bucket::MsgFeed.interval());
        assert_eq!(slots[2] - slots[1], Bucket::MsgFeed.interval());
    }
}
