//! 数字与时长格式化。
//!
//! 规则来自设计文档**附录 C.1**，这里逐条实现，并有单测守着。
//! 摘要：
//!
//! | 原始值 n | 显示 |
//! |---|---|
//! | n < 10 000 | 整数原值（无千分位、无单位）：`862` / `1628` |
//! | 10 000 ≤ n < 1 亿 | `x.x万`，保留 1 位小数，去掉末尾 `.0`：`1万` / `1.5万` |
//! | n ≥ 1 亿 | `x.x亿`，同样规则：`1亿` / `12.3亿` |
//! | 进位保护 | 先按万换算四舍五入到 1 位；若结果 ≥ 10 000 万，改用亿 |
//!
//! 禁止：`1.2w` / `12.3M`；禁止「12.4 万播放」这种带「播放」二字的冗余形态
//! （单位由调用方的文案承担）。

const WAN: f64 = 10_000.0;
const YI: f64 = 100_000_000.0;

/// 把保留 1 位小数的数值渲染成字符串，去掉多余的 `.0`。
fn one_decimal(v: f64) -> String {
    // 先四舍五入到 1 位
    let r = (v * 10.0).round() / 10.0;
    if (r.fract()).abs() < f64::EPSILON {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

/// 播放量 / 弹幕数等计数的显示形态。
///
/// ```
/// # use yk_core::format::count;
/// assert_eq!(count(862), "862");
/// assert_eq!(count(9_999), "9999");
/// assert_eq!(count(10_000), "1万");
/// assert_eq!(count(15_190), "1.5万");
/// assert_eq!(count(99_999), "10万");
/// assert_eq!(count(100_000_000), "1亿");
/// assert_eq!(count(1_234_567_890), "12.3亿");
/// ```
pub fn count(n: u64) -> String {
    if n < 10_000 {
        return n.to_string();
    }
    if n < 100_000_000 {
        // 先按万换算，四舍五入到 1 位
        let wan = (n as f64 / WAN * 10.0).round() / 10.0;
        // 进位保护：99 999 → 10.0 万仍是万；但若四舍五入后 ≥ 10 000 万则改用亿
        if wan >= 10_000.0 {
            return format!("{}亿", one_decimal(n as f64 / YI));
        }
        return format!("{}万", one_decimal(wan));
    }
    format!("{}亿", one_decimal(n as f64 / YI))
}

/// 时长格式化：`mm:ss`，超过 1 小时用 `h:mm:ss`。
///
/// 对应设计文档第 14 节的进度气泡口径（`mm:ss` / 超 1 小时 `h:mm:ss`）。
///
/// ```
/// # use yk_core::format::duration;
/// assert_eq!(duration(0), "00:00");
/// assert_eq!(duration(65), "01:05");
/// assert_eq!(duration(3725), "1:02:05");
/// ```
pub fn duration(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 解码 B 站标题里的 HTML 实体。
///
/// 对应旧实现里的 `System.Net.WebUtility.HtmlDecode(v.Title)`。
/// 只处理 B 站标题实际会出现的实体（命名实体 + 十进制/十六进制数字实体），
/// 不做完整的 HTML5 实体表。
pub fn html_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != '&' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        // 找分号（限制长度，避免把普通 & 当实体）
        let end = bytes[i..].iter().take(12).position(|&c| c == ';');
        let Some(rel) = end else {
            out.push('&');
            i += 1;
            continue;
        };
        if rel < 2 {
            out.push('&');
            i += 1;
            continue;
        }
        let entity: String = bytes[i + 1..i + rel].iter().collect();
        let decoded = match entity.as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            _ => {
                if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(c) => {
                out.push(c);
                i += rel + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 直接抄自设计文档附录 C.1 的换算对照表。
    #[test]
    fn count_matches_appendix_c_table() {
        let cases = [
            (0u64, "0"),
            (862, "862"),
            (1_628, "1628"),
            (9_999, "9999"),
            (10_000, "1万"),
            (11_000, "1.1万"),
            (15_190, "1.5万"),
            (99_999, "10万"),
            (100_000, "10万"),
            (112_000, "11.2万"),
            (100_000_000, "1亿"),
            (120_000_000, "1.2亿"),
            (1_234_567_890, "12.3亿"),
        ];
        for (n, want) in cases {
            assert_eq!(count(n), want, "count({n}) 应为 {want}");
        }
    }

    /// 进位保护：接近 1 亿的数按万算会到 9999.9 万，四舍五入后 10000.0 万，
    /// 必须改用亿，不能显示「10000万」。
    #[test]
    fn carry_guard_never_prints_10000_wan() {
        let s = count(99_999_999);
        assert!(!s.contains("10000万"), "出现了 10000 万：{s}");
        assert!(s.ends_with('亿'), "进位后应改用亿：{s}");
    }

    #[test]
    fn counts_never_use_latin_units() {
        for n in [10_000u64, 1_500_000, 300_000_000] {
            let s = count(n);
            assert!(!s.contains('w') && !s.contains('M') && !s.contains('k'), "{s}");
        }
    }

    #[test]
    fn duration_formats() {
        assert_eq!(duration(0), "00:00");
        assert_eq!(duration(5), "00:05");
        assert_eq!(duration(65), "01:05");
        assert_eq!(duration(599), "09:59");
        assert_eq!(duration(3600), "1:00:00");
        assert_eq!(duration(3725), "1:02:05");
    }

    #[test]
    fn html_decode_covers_bili_titles() {
        assert_eq!(html_decode("A &amp; B"), "A & B");
        assert_eq!(html_decode("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_decode("&quot;x&quot;"), "\"x\"");
        assert_eq!(html_decode("&#39;"), "'");
        assert_eq!(html_decode("&#x4E00;"), "一");
        // 普通 & 不应被吞掉
        assert_eq!(html_decode("R&D"), "R&D");
        // 未知实体原样保留
        assert_eq!(html_decode("&unknown;"), "&unknown;");
    }
}
