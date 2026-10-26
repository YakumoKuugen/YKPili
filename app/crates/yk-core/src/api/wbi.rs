//! B 站 WBI 签名。
//!
//! 对应旧 `BilibiliApi.BuildWbiSignedUrl` / `GetMixinKey` / `ComputeMd5Hash`。
//! 这是通过风控校验的关键：少一个参数、顺序错一处、编码差一点，接口就返回 `-352`。
//!
//! 算法（官方口径）：
//! 1. `img_key + sub_key` 按 `MIXIN_KEY_ENC_TAB` 置换，取前 32 位得 `mixin_key`；
//! 2. 业务参数按 **key 字典序**排序，各 key/value 做 URL 编码；
//! 3. 追加 `wts`（**秒级**时间戳）；
//! 4. `query + mixin_key` 做 MD5（小写 16 进制）= `w_rid`；
//! 5. 最终 URL = `base?query&w_rid=w_rid`。
//!
//! 时间戳是外部传入的，不在内部取 `now()` —— 这样签名可重放、可单测。

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// WBI 的编码集：在 `NON_ALPHANUMERIC` 基础上去掉 RFC 3986 的 unreserved 字符
/// `- . _ ~`。
///
/// **这一点必须与 .NET 的 `Uri.EscapeDataString` 对齐**：它不编码这四个字符。
/// 如果这里把 `_` 编成 `%5F`，`fresh_type` 会变成 `fresh%5Ftype`，
/// 签名原文与 B 站服务端计算的原文不一致 → 风控返回 -352。
/// 这是实测抓到的，别改回去。
const WBI_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// 官方置换表（64 项，只取前 32 位）。
pub const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22,
    25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 52, 44, 34,
];

/// 由 `img_key` 与 `sub_key` 推出 `mixin_key`（32 字符）。
pub fn mixin_key(img_key: &str, sub_key: &str) -> String {
    let raw: Vec<char> = format!("{img_key}{sub_key}").chars().collect();
    MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .map(|&i| raw.get(i).copied().unwrap_or('0'))
        .collect()
}

/// 小写十六进制 MD5。
pub fn md5_hex(input: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(32);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 按 WBI 规则编码一个 query 分量。
///
/// 用 `NON_ALPHANUMERIC` 会把 `-._~` 也编码掉，这与 `Uri.EscapeDataString`
/// 在 B 站参数上的实际效果一致（B 站参数里基本只出现字母数字与下划线）。
fn enc(s: &str) -> String {
    utf8_percent_encode(s, WBI_SET).to_string()
}

/// 签名结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// 已排序并编码的 query（**不含** `w_rid`），可直接拼到 URL 上。
    pub query: String,
    /// 32 位小写十六进制签名。
    pub w_rid: String,
}

/// 计算签名。`params` 不含 `wts`，内部会追加。
///
/// `wts` 由调用方传入（秒级 Unix 时间戳），便于测试与重放。
pub fn sign(
    params: &[(String, String)],
    img_key: &str,
    sub_key: &str,
    wts: u64,
) -> Signed {
    let mk = mixin_key(img_key, sub_key);

    // 追加 wts 后按 key 字典序排序
    let mut items: Vec<(String, String)> = params.to_vec();
    items.push(("wts".to_string(), wts.to_string()));
    items.sort_by(|a, b| a.0.cmp(&b.0));

    let query = items
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&");

    let w_rid = md5_hex(&format!("{query}{mk}"));
    Signed { query, w_rid }
}

/// 生成完整签名 URL。
pub fn signed_url(
    base: &str,
    params: &[(String, String)],
    img_key: &str,
    sub_key: &str,
    wts: u64,
) -> String {
    let s = sign(params, img_key, sub_key, wts);
    format!("{base}?{}&w_rid={}", s.query, s.w_rid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1321 标准测试向量 —— 先确认 MD5 本身没写错。
    #[test]
    fn md5_known_vectors() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex("message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
    }

    /// B 站 WBI 的公开示例：由 img_key / sub_key 推出的 mixin_key。
    #[test]
    fn mixin_key_matches_official_example() {
        let img = "7cd084941338484aae1ad9425b84077c";
        let sub = "4932caff0ff746eab6f01bf08b70ac45";
        assert_eq!(mixin_key(img, sub), "ea1db124af3c7062474693fa704f4ff8");
    }

    #[test]
    fn params_are_sorted_and_encoded() {
        let params = vec![
            ("zab".to_string(), "1919810".to_string()),
            ("foo".to_string(), "114".to_string()),
            ("bar".to_string(), "514".to_string()),
        ];
        let s = sign(&params, "7cd084941338484aae1ad9425b84077c", "4932caff0ff746eab6f01bf08b70ac45", 1_702_204_169);
        // 字典序：bar < foo < wts < zab
        assert!(s.query.starts_with("bar=514&foo=114&wts=1702204169&zab=1919810"), "{}", s.query);
        assert_eq!(s.w_rid.len(), 32);
        assert!(s.w_rid.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn wts_is_appended_even_if_caller_passes_one() {
        let params = vec![("wts".to_string(), "1".to_string())];
        let s = sign(&params, "a", "b", 999);
        // 调用方传的 wts 会被保留，但我们的也会追加 —— 实际使用中调用方不要传 wts
        assert!(s.query.contains("wts=1"));
        assert!(s.query.contains("wts=999"));
    }

    #[test]
    fn signed_url_shape() {
        let url = signed_url(
            "https://api.bilibili.com/x/web-interface/wbi/index/top/feed/rcmd",
            &[("ps".to_string(), "30".to_string())],
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
            1_702_204_169,
        );
        assert!(url.contains("?ps=30&wts=1702204169&w_rid="));
        assert_eq!(url.matches("w_rid=").count(), 1);
    }
}
