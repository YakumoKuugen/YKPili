//! 封面类通用积木：占位色渐变。
//!
//! 真实封面加载前 / 失败后，卡片、UP 主投稿、播放器海报都画同一套
//! 「暗 → 中」双色渐变。实现只有这一份，避免各页面各写一套色值逻辑。

use gpui::*;

/// 自绘封面兜底渐变：主题令牌只有单色，设计稿的 135° 双色卡面
/// 用占位色派生「暗 → 中」两档自己画。
///
/// `hex` 支持 `#RRGGBB` / `#RRGGBBAA` / `#AARRGGBB`（接口口径）；
/// 透明或解析失败时退回主题表面色 `fallback`。
pub fn placeholder_gradient(hex: &str, fallback: Hsla) -> Background {
    let base = Hsla::from(parse_placeholder(hex));
    let base = if base.a < 0.05 { fallback } else { base };
    let start = Hsla {
        h: base.h,
        s: base.s,
        l: (base.l * 0.35).clamp(0.08, 0.32),
        a: 1.0,
    };
    let end = Hsla {
        h: base.h,
        s: base.s,
        l: (base.l * 0.30 + 0.24).clamp(0.16, 0.52),
        a: 1.0,
    };
    linear_gradient(
        135.,
        linear_color_stop(start, 0.),
        linear_color_stop(end, 1.),
    )
}

/// 把 `#RRGGBB` / `#RRGGBBAA` / `#AARRGGBB` 解析成颜色。
///
/// 占位色来自数据层，是字符串；这里按设计稿的口径：`#AARRGGBB`
/// （与 XAML 一致）。
fn parse_placeholder(hex: &str) -> Rgba {
    let h = hex.trim_start_matches('#');
    match h.len() {
        6 => {
            let v = u32::from_str_radix(h, 16).unwrap_or(0x555555);
            rgb(v)
        }
        8 => {
            let v = u32::from_str_radix(h, 16).unwrap_or(0xFF555555);
            rgba(v)
        }
        _ => rgb(0x555555),
    }
}
