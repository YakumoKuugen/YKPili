//! 深链 URL 解析与规范化。
//!
//! 支持三种入口，全部收敛成同一种 [`DeepLink`]：
//!
//! 1. 自有 Scheme：`ykpili://video/BV1xx411c7mD?p=2&t=90`
//! 2. B 站网页：`https://www.bilibili.com/video/BV1xx411c7mD/?p=2&t=90`
//! 3. 短链：`https://b23.tv/xxxx`（交给 App 侧再解析一次跳转）
//!
//! 解析器只做纯字符串工作，不发网络请求，因此可以放心在 UI 线程调用。

use url::Url;

/// 应用注册的自定义协议。Info.plist / 注册表 / 用户脚本必须和这里一致。
pub const SCHEME: &str = "ykpili";

/// Universal Link 使用的自有域名。
///
/// 这个域名必须由项目自己控制，并在
/// `https://<host>/.well-known/apple-app-site-association` 部署 AASA。
/// 换成自己的域名时，同时修改：
/// - `packaging/macos/YKPili.entitlements`
/// - `packaging/macos/apple-app-site-association`
/// - 这里的常量与测试
pub const UNIVERSAL_LINK_HOST: &str = "go.ykpili.app";

/// 解析后的深链。
#[derive(Debug, Clone, PartialEq)]
pub enum DeepLink {
    /// 可直接打开的视频。
    Video(VideoLink),
    /// B 站短链（`b23.tv` / `bili2233.cn`）：需要先跟一次 HTTP 跳转。
    ShortLink { url: String, raw: String },
    /// 站内搜索。
    Search { keyword: String, raw: String },
    /// 认识协议、但当前客户端没有对应页面。
    Unsupported { raw: String, reason: String },
}

/// 视频深链参数。
#[derive(Debug, Clone, PartialEq)]
pub struct VideoLink {
    /// `BV` 号（大小写保持原样）。
    pub bvid: Option<String>,
    /// 旧式 `av` 号。
    pub avid: Option<u64>,
    /// 分 P，从 1 开始；普通单 P 视频为 1。
    pub page: u32,
    /// 起播秒数（B 站链接里的 `t=` / `start=`）。
    pub start_secs: Option<f64>,
    /// 原始 URL（提示文案 / 日志用）。
    pub raw: String,
}

impl VideoLink {
    /// 生成客户端自己的深链。用户脚本、分享菜单、测试都用它。
    pub fn to_uri(&self) -> String {
        let id = match (&self.bvid, self.avid) {
            (Some(bvid), _) => bvid.clone(),
            (None, Some(avid)) => format!("av{avid}"),
            (None, None) => String::new(),
        };
        let mut uri = format!("{SCHEME}://video/{id}");
        let mut query = Vec::new();
        if self.page > 1 {
            query.push(format!("p={}", self.page));
        }
        if let Some(seconds) = self.start_secs.filter(|s| *s > 0.0) {
            query.push(format!("t={}", seconds.round() as u64));
        }
        if !query.is_empty() {
            uri.push('?');
            uri.push_str(&query.join("&"));
        }
        uri
    }

    /// 展示用标题：优先 BV，其次 av。
    pub fn display_id(&self) -> String {
        match (&self.bvid, self.avid) {
            (Some(bvid), _) => bvid.clone(),
            (None, Some(avid)) => format!("av{avid}"),
            (None, None) => "未知视频".to_string(),
        }
    }
}

impl DeepLink {
    /// 解析任意入口 URL。无法识别时返回 [`DeepLink::Unsupported`]，
    /// 不 panic，也不把非 B 站链接误当视频。
    pub fn parse(raw: &str) -> Self {
        Self::parse_inner(raw, 0)
    }

    fn parse_inner(raw: &str, depth: usize) -> Self {
        const MAX_DEPTH: usize = 4;
        let raw = raw.trim().trim_matches('"').trim();
        if raw.is_empty() {
            return Self::Unsupported {
                raw: String::new(),
                reason: "空链接".to_string(),
            };
        }
        if depth > MAX_DEPTH {
            return Self::Unsupported {
                raw: raw.to_string(),
                reason: "深链嵌套层数过深".to_string(),
            };
        }

        // `ykpili:video/BV...`（不带 //）也是合法的自定义 Scheme 写法。
        let normalized = if raw
            .get(..SCHEME.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(SCHEME))
            && raw[SCHEME.len()..].starts_with(':')
            && !raw[SCHEME.len() + 1..].starts_with("//")
        {
            format!("{SCHEME}://{}", &raw[SCHEME.len() + 1..])
        } else {
            raw.to_string()
        };

        let Ok(url) = Url::parse(&normalized) else {
            return Self::Unsupported {
                raw: raw.to_string(),
                reason: "不是合法的 URL".to_string(),
            };
        };

        if url.scheme().eq_ignore_ascii_case(SCHEME) {
            return parse_scheme(&url, raw, depth);
        }

        if matches!(url.scheme(), "http" | "https") {
            return parse_web(&url, raw, depth);
        }

        Self::Unsupported {
            raw: raw.to_string(),
            reason: format!("不支持的协议：{}", url.scheme()),
        }
    }

    /// 是否是「能直接交给播放器」的链接。
    pub fn is_playable(&self) -> bool {
        matches!(self, Self::Video(_))
    }

    pub fn raw(&self) -> &str {
        match self {
            Self::Video(video) => &video.raw,
            Self::ShortLink { raw, .. }
            | Self::Search { raw, .. }
            | Self::Unsupported { raw, .. } => raw,
        }
    }
}

fn parse_scheme(url: &Url, raw: &str, depth: usize) -> DeepLink {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let path = url.path().trim_start_matches('/');

    match host.as_str() {
        "video" => parse_video_segment(path, url, raw),
        "search" => {
            let keyword = query_value(url, &["keyword", "q", "wd"]).unwrap_or_default();
            if keyword.trim().is_empty() {
                DeepLink::Unsupported {
                    raw: raw.to_string(),
                    reason: "搜索深链缺少 keyword".to_string(),
                }
            } else {
                DeepLink::Search {
                    keyword: keyword.trim().to_string(),
                    raw: raw.to_string(),
                }
            }
        }
        "open" => parse_open_target(url, raw, depth),
        "" => {
            // `ykpili:///video/BV...`：host 为空，路径里带页面类型。
            let mut segments = path.splitn(2, '/');
            match segments.next().unwrap_or("") {
                "video" => parse_video_segment(segments.next().unwrap_or(""), url, raw),
                "search" => {
                    let keyword = query_value(url, &["keyword", "q", "wd"]).unwrap_or_default();
                    if keyword.trim().is_empty() {
                        DeepLink::Unsupported {
                            raw: raw.to_string(),
                            reason: "搜索深链缺少 keyword".to_string(),
                        }
                    } else {
                        DeepLink::Search {
                            keyword: keyword.trim().to_string(),
                            raw: raw.to_string(),
                        }
                    }
                }
                other => DeepLink::Unsupported {
                    raw: raw.to_string(),
                    reason: format!("未知深链页面：{other}"),
                },
            }
        }
        other => DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: format!("未知深链页面：{other}"),
        },
    }
}

fn parse_open_target(url: &Url, raw: &str, depth: usize) -> DeepLink {
    // `ykpili://open?url=https%3A%2F%2F...`
    let target = query_value(url, &["url", "u", "target"]);
    match target {
        Some(target) if !target.trim().is_empty() => {
            let mut parsed = DeepLink::parse_inner(target.trim(), depth + 1);
            // `raw` 保留外层深链，方便日志定位是哪一跳进来的。
            match &mut parsed {
                DeepLink::Video(video) => video.raw = raw.to_string(),
                DeepLink::ShortLink { raw: inner, .. } => *inner = raw.to_string(),
                DeepLink::Search { raw: inner, .. } => *inner = raw.to_string(),
                DeepLink::Unsupported { raw: inner, .. } => *inner = raw.to_string(),
            }
            parsed
        }
        _ => DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: "open 深链缺少 url 参数".to_string(),
        },
    }
}

fn is_universal_host(host: &str) -> bool {
    host == UNIVERSAL_LINK_HOST || host == "ykpili.app" || host.ends_with(".ykpili.app")
}

fn parse_universal_link(url: &Url, raw: &str, depth: usize) -> DeepLink {
    let path = url.path().trim_start_matches('/');
    let mut segments = path.splitn(2, '/');
    match segments.next().unwrap_or("") {
        "video" => parse_video_segment(segments.next().unwrap_or(""), url, raw),
        "open" => parse_open_target(url, raw, depth),
        "search" => {
            let keyword = query_value(url, &["keyword", "q", "wd"]).unwrap_or_default();
            if keyword.trim().is_empty() {
                DeepLink::Unsupported {
                    raw: raw.to_string(),
                    reason: "Universal Link 搜索缺少 keyword".to_string(),
                }
            } else {
                DeepLink::Search {
                    keyword: keyword.trim().to_string(),
                    raw: raw.to_string(),
                }
            }
        }
        other => DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: format!("Universal Link 路径不支持：/{other}"),
        },
    }
}

fn parse_web(url: &Url, raw: &str, depth: usize) -> DeepLink {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);

    // Universal Link：https://go.ykpili.app/video/BV...
    // 和自定义 Scheme 共用同一套路径语义。
    if is_universal_host(host) {
        return parse_universal_link(url, raw, depth);
    }

    if matches!(host, "b23.tv" | "bili2233.cn") {
        return DeepLink::ShortLink {
            url: url.to_string(),
            raw: raw.to_string(),
        };
    }

    let is_bilibili =
        host == "bilibili.com" || host == "m.bilibili.com" || host.ends_with(".bilibili.com");

    if !is_bilibili {
        return DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: format!("不是 B 站链接：{host}"),
        };
    }

    // 1) 标准视频页：/video/BV... 或 /video/av...
    if let Some(segments) = url.path_segments() {
        let segments: Vec<&str> = segments.collect();
        if segments.len() >= 2 && segments[0].eq_ignore_ascii_case("video") {
            let id = segments[1];
            if is_bvid(id) || is_avid(id) {
                return parse_video_segment(id, url, raw);
            }
        }
    }

    // 2) 列表 / 收藏夹等页面用 `?bvid=BV...` 指向视频。
    if let Some(bvid) = query_value(url, &["bvid"]) {
        if is_bvid(&bvid) {
            return parse_video_segment(&bvid, url, raw);
        }
    }

    // 3) 番剧 / 直播：播放器当前没有对应能力，明确拒绝而不是误开。
    let path = url.path();
    if path.starts_with("/bangumi/") {
        return DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: "暂不支持番剧深链（播放器目前只处理 UGC 视频）".to_string(),
        };
    }
    if path.starts_with("/live/") {
        return DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: "暂不支持直播深链".to_string(),
        };
    }

    DeepLink::Unsupported {
        raw: raw.to_string(),
        reason: "B 站链接里没有找到视频号".to_string(),
    }
}

fn parse_video_segment(segment: &str, url: &Url, raw: &str) -> DeepLink {
    let segment = segment.trim().trim_end_matches('/');
    // 允许 `BV...` 后面再带一段（例如 /video/BV.../p2，不常见但别直接拒）。
    let first = segment.split('/').next().unwrap_or(segment);

    let bvid = if is_bvid(first) {
        // B 站接口区分大小写，统一成 `BV` 前缀。
        Some(format!("BV{}", &first[2..]))
    } else {
        None
    };
    let avid = if let Some(digits) = first
        .get(..2)
        .filter(|prefix| prefix.eq_ignore_ascii_case("av"))
        .and_then(|_| first.get(2..))
    {
        digits.parse::<u64>().ok()
    } else {
        None
    };

    if bvid.is_none() && avid.is_none() {
        return DeepLink::Unsupported {
            raw: raw.to_string(),
            reason: format!("无法识别的视频号：{first}"),
        };
    }

    let page = query_value(url, &["p", "page"])
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|page| (1..=9_999).contains(page))
        .unwrap_or(1);
    let start_secs = query_value(url, &["t", "start", "start_seconds", "time"])
        .and_then(|value| parse_time(&value));

    DeepLink::Video(VideoLink {
        bvid,
        avid,
        page,
        start_secs,
        raw: raw.to_string(),
    })
}

fn query_value(url: &Url, keys: &[&str]) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| {
            keys.iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
        })
        .map(|(_, value)| value.into_owned())
}

/// BVID：`BV` + 10 位 Base58 字符，总长 12。
pub fn is_bvid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 12 || !bytes[..2].eq_ignore_ascii_case(b"BV") {
        return false;
    }
    bytes[2..].iter().all(|byte| byte.is_ascii_alphanumeric())
}

fn is_avid(value: &str) -> bool {
    value
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("av"))
        && value.get(2..).is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// 解析 B 站的时间参数：`90`、`90.5`、`1m30s`、`1h2m3s`、`01:30`、`1:02:03`。
pub fn parse_time(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if value.contains(':') {
        let mut total = 0.0;
        for part in value.split(':') {
            let number = part.trim().parse::<f64>().ok()?;
            total = total * 60.0 + number;
        }
        return (total.is_finite() && total >= 0.0).then_some(total);
    }

    if value.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        let mut total = 0.0;
        let mut number = String::new();
        for ch in value.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                number.push(ch);
                continue;
            }
            let parsed = number.parse::<f64>().ok()?;
            number.clear();
            match ch.to_ascii_lowercase() {
                'h' => total += parsed * 3600.0,
                'm' => total += parsed * 60.0,
                's' => total += parsed,
                _ => return None,
            }
        }
        if !number.is_empty() {
            total += number.parse::<f64>().ok()?;
        }
        return (total.is_finite() && total >= 0.0).then_some(total);
    }

    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
}

/// 从命令行参数里找第一条深链。
///
/// Windows 协议启动把 URL 作为 `argv[1]` 传入；开发时也可能带
/// `--url=ykpili://...`。跳过可执行文件本身与普通开关。
pub fn first_link_in_args<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for (index, arg) in args.into_iter().enumerate() {
        let arg = arg.as_ref().trim();
        if arg.is_empty() {
            continue;
        }
        // 第一个参数通常是可执行文件本身；如果它是 URL，就照常解析。
        if index == 0 && !arg.contains("://") {
            continue;
        }

        let candidate = arg
            .strip_prefix("--url=")
            .or_else(|| arg.strip_prefix("--open="))
            .unwrap_or(arg)
            .trim_matches('"')
            .trim();

        if candidate
            .to_ascii_lowercase()
            .starts_with(&format!("{SCHEME}:"))
            || candidate
                .to_ascii_lowercase()
                .starts_with("https://www.bilibili.com/")
            || candidate
                .to_ascii_lowercase()
                .starts_with("https://m.bilibili.com/")
            || candidate
                .to_ascii_lowercase()
                .starts_with("https://b23.tv/")
            || candidate
                .to_ascii_lowercase()
                .starts_with("https://bili2233.cn/")
        {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_universal_link_on_own_domain() {
        let link = DeepLink::parse("https://go.ykpili.app/video/BV1xx411c7mD?p=2&t=1m30s");
        let DeepLink::Video(video) = link else {
            panic!("Universal Link 应解成视频：{link:?}");
        };
        assert_eq!(video.bvid.as_deref(), Some("BV1xx411c7mD"));
        assert_eq!(video.page, 2);
        assert_eq!(video.start_secs, Some(90.0));
    }

    #[test]
    fn bvid_prefix_is_normalized_to_uppercase() {
        let link = DeepLink::parse("https://www.bilibili.com/video/bv1xx411c7mD");
        let DeepLink::Video(video) = link else {
            panic!("应当识别小写 bv 前缀");
        };
        assert_eq!(video.bvid.as_deref(), Some("BV1xx411c7mD"));
    }

    #[test]
    fn nested_open_links_are_depth_limited() {
        let mut nested = "ykpili://video/BV1xx411c7mD".to_string();
        for _ in 0..8 {
            let encoded: String = url::form_urlencoded::byte_serialize(nested.as_bytes()).collect();
            nested = format!("ykpili://open?url={encoded}");
        }
        match DeepLink::parse(&nested) {
            DeepLink::Unsupported { reason, .. } => assert!(reason.contains("嵌套")),
            other => panic!("嵌套过深应当被拒绝：{other:?}"),
        }
    }

    #[test]
    fn parses_web_video_with_page_and_time() {
        let link = DeepLink::parse(
            "https://www.bilibili.com/video/BV1xx411c7mD/?p=2&t=1m30s&spm_id_from=333.999",
        );
        let DeepLink::Video(video) = link else {
            panic!("应当是视频深链：{link:?}");
        };
        assert_eq!(video.bvid.as_deref(), Some("BV1xx411c7mD"));
        assert_eq!(video.page, 2);
        assert_eq!(video.start_secs, Some(90.0));
    }

    #[test]
    fn parses_scheme_video() {
        let link = DeepLink::parse("ykpili://video/BV1xx411c7mD?p=3&t=01:02:03");
        let DeepLink::Video(video) = link else {
            panic!("应当是视频深链");
        };
        assert_eq!(video.page, 3);
        assert_eq!(video.start_secs, Some(3723.0));
        assert_eq!(video.to_uri(), "ykpili://video/BV1xx411c7mD?p=3&t=3723");
    }

    #[test]
    fn parses_av_and_scheme_without_slashes() {
        let link = DeepLink::parse("ykpili:video/av170001?t=42");
        let DeepLink::Video(video) = link else {
            panic!("应当是视频深链");
        };
        assert_eq!(video.avid, Some(170001));
        assert_eq!(video.start_secs, Some(42.0));
        assert_eq!(video.to_uri(), "ykpili://video/av170001?t=42");
    }

    #[test]
    fn short_link_is_kept_for_later_resolution() {
        let link = DeepLink::parse("https://b23.tv/AbCdEf");
        assert!(matches!(link, DeepLink::ShortLink { .. }));
    }

    #[test]
    fn open_wrapper_unwraps_video() {
        let inner = "https%3A%2F%2Fwww.bilibili.com%2Fvideo%2FBV1xx411c7mD%3Fp%3D4";
        let link = DeepLink::parse(&format!("ykpili://open?url={inner}"));
        let DeepLink::Video(video) = link else {
            panic!("应当解包为视频深链");
        };
        assert_eq!(video.page, 4);
    }

    #[test]
    fn rejects_other_hosts() {
        let link = DeepLink::parse("https://example.com/video/BV1xx411c7mD");
        assert!(matches!(link, DeepLink::Unsupported { .. }));
    }

    #[test]
    fn bvid_validation_is_strict() {
        assert!(is_bvid("BV1xx411c7mD"));
        assert!(!is_bvid("BV1xx"));
        assert!(!is_bvid("AV1xx411c7mD"));
    }

    #[test]
    fn parses_time_formats() {
        assert_eq!(parse_time("90"), Some(90.0));
        assert_eq!(parse_time("1m30s"), Some(90.0));
        assert_eq!(parse_time("1h2m3s"), Some(3723.0));
        assert_eq!(parse_time("01:30"), Some(90.0));
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("inf"), None);
    }

    #[test]
    fn absurd_page_numbers_fall_back_to_first() {
        let DeepLink::Video(video) =
            DeepLink::parse("https://www.bilibili.com/video/BV1xx411c7mD?p=99999")
        else {
            panic!("应当识别为视频");
        };
        assert_eq!(video.page, 1);
    }

    #[test]
    fn finds_link_in_cli_args() {
        let args = ["ykpili.exe", "--foo", "ykpili://video/BV1xx411c7mD?t=10"];
        assert_eq!(
            first_link_in_args(args).as_deref(),
            Some("ykpili://video/BV1xx411c7mD?t=10")
        );
    }

    #[test]
    fn unsupported_bangumi_has_reason() {
        let link = DeepLink::parse("https://www.bilibili.com/bangumi/play/ep123");
        match link {
            DeepLink::Unsupported { reason, .. } => assert!(reason.contains("番剧")),
            other => panic!("应当是 Unsupported：{other:?}"),
        }
    }
}
