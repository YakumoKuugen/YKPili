//! 语义表面色（分亮/暗两套），含页面背景渐变。
//!
//! 对应 `YKPili/Styles/Tokens/Surfaces.axaml`，由脚本转换，色值零改动。
//!
//! 渐变在 GPUI 里没有「资源字典」形态，视图侧用 `linear_gradient()` /
//! `radial_gradient()` 现搭；这里只提供色标，保证与 Avalonia 版一致。

use gpui::{rgba, Hsla};

/// 一套主题下的全部语义表面色与背景渐变。
#[derive(Clone, Debug)]
pub struct YkSurfaces {
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_quaternary: Hsla,
    pub text_on_accent: Hsla,
    pub bg_base: Hsla,
    pub bg_homepage: Hsla,
    pub surface_content: Hsla,
    pub surface_content_hover: Hsla,
    pub surface_content_alt: Hsla,
    pub surface_content_alt_hover: Hsla,
    pub surface_content_solid: Hsla,
    pub surface_content_solid_hover: Hsla,
    pub surface_elevated: Hsla,
    pub surface_elevated_hover: Hsla,
    pub surface_elevated_solid: Hsla,
    pub surface_elevated_solid_hover: Hsla,
    pub surface_elevated_strong: Hsla,
    /// 弹层 / 菜单底：比所在表面再亮一档。
    ///
    /// 暗色下 `surface_elevated_strong` 与侧栏底（#373A3E）同色，光靠描边和阴影
    /// 几乎看不出「这张卡浮在上面」；弹层单独一档，层级靠表面差就成立。
    pub surface_popover: Hsla,
    /// 顶栏 / 工具栏底：比卡片 / 浮层更暗、带一点透明，让工具栏成为「窗口外壳」
    /// 而不是另一块内容卡。取色参考 macOS 浏览器工具栏（#2E3034）。
    pub surface_toolbar: Hsla,
    pub fill_1: Hsla,
    pub fill_2: Hsla,
    pub fill_3: Hsla,
    pub fill_4: Hsla,
    pub fill_alt: Hsla,
    pub fill_disabled: Hsla,
    pub border_subtle: Hsla,
    pub border_popover: Hsla,
    pub border_strong: Hsla,
    pub skeleton_brush: Hsla,
    pub switch_bg: Hsla,
    pub ip_tag_bg: Hsla,
    pub ip_tag_text: Hsla,
    pub overlay_scrim: Hsla,
    /// 页面背景线性渐变：(色, 位置) × 3
    pub bg_gradient: [(Hsla, f32); 3],
    /// 渐变之上的主题色光晕（径向）：(色, 位置) × 2
    pub bg_accent_glow: [(Hsla, f32); 2],
}

/// light 主题。
pub fn light() -> YkSurfaces {
    YkSurfaces {
        text_primary: rgba(0x15181EFF).into(),
        text_secondary: rgba(0x323B49CC).into(),
        text_tertiary: rgba(0x3A455599).into(),
        text_quaternary: rgba(0x4A596D66).into(),
        text_on_accent: rgba(0xFFFFFFFF).into(),
        bg_base: rgba(0xFFFFFFFF).into(),
        bg_homepage: rgba(0xF2F2F8FF).into(),
        surface_content: rgba(0xFFFFFFAD).into(),
        surface_content_hover: rgba(0xD9D9D9AD).into(),
        surface_content_alt: rgba(0xEEEEF6AD).into(),
        surface_content_alt_hover: rgba(0xBCBCDCAD).into(),
        surface_content_solid: rgba(0xFFFFFFFF).into(),
        surface_content_solid_hover: rgba(0xD9D9D9FF).into(),
        surface_elevated: rgba(0xFFFFFFAD).into(),
        surface_elevated_hover: rgba(0xD9D9D9AD).into(),
        surface_elevated_solid: rgba(0xFFFFFFFF).into(),
        surface_elevated_solid_hover: rgba(0xD9D9D9FF).into(),
        surface_elevated_strong: rgba(0xFFFFFFEB).into(),
        // 浅色下弹层就是白卡：和侧栏同色时靠描边 + 阴影分层，别压成灰。
        surface_popover: rgba(0xFFFFFFFA).into(),
        surface_toolbar: rgba(0xF6F7F9F2).into(),
        fill_1: rgba(0x83839126).into(),
        fill_2: rgba(0x8383914D).into(),
        fill_3: rgba(0x83839173).into(),
        fill_4: rgba(0x83839199).into(),
        fill_alt: rgba(0xFFFFFF73).into(),
        fill_disabled: rgba(0x8383911A).into(),
        border_subtle: rgba(0x8383912E).into(),
        border_popover: rgba(0xFFFFFF66).into(),
        border_strong: rgba(0x8383914D).into(),
        skeleton_brush: rgba(0x8383914D).into(),
        switch_bg: rgba(0xE3E5E7FF).into(),
        ip_tag_bg: rgba(0xF1F2F3FF).into(),
        ip_tag_text: rgba(0x61666DFF).into(),
        overlay_scrim: rgba(0x0000008C).into(),
        bg_gradient: [
            (rgba(0xDCF1F9FF).into(), 0.0),
            (rgba(0xEFF2F7FF).into(), 0.55),
            (rgba(0xF2F2F8FF).into(), 1.0),
        ],
        bg_accent_glow: [
            (rgba(0x00A1D633).into(), 0.0),
            (rgba(0x00A1D600).into(), 1.0),
        ],
    }
}

/// dark 主题。
pub fn dark() -> YkSurfaces {
    YkSurfaces {
        text_primary: rgba(0xF1F2F4FF).into(),
        text_secondary: rgba(0xD4D7DEE6).into(),
        text_tertiary: rgba(0xB7BCC8BF).into(),
        text_quaternary: rgba(0x9AA2B18C).into(),
        text_on_accent: rgba(0xFFFFFFFF).into(),
        bg_base: rgba(0x222428FF).into(),
        bg_homepage: rgba(0x222428FF).into(),
        surface_content: rgba(0x303338AD).into(),
        surface_content_hover: rgba(0x3B3E42AD).into(),
        surface_content_alt: rgba(0x35383CAD).into(),
        surface_content_alt_hover: rgba(0x3F4247AD).into(),
        surface_content_solid: rgba(0x303338FF).into(),
        surface_content_solid_hover: rgba(0x3B3E42FF).into(),
        surface_elevated: rgba(0x373A3EAD).into(),
        surface_elevated_hover: rgba(0x414449AD).into(),
        surface_elevated_solid: rgba(0x373A3EFF).into(),
        surface_elevated_solid_hover: rgba(0x414449FF).into(),
        surface_elevated_strong: rgba(0x373A3EEB).into(),
        surface_popover: rgba(0x45494FF2).into(),
        surface_toolbar: rgba(0x2B2E33F2).into(),
        fill_1: rgba(0x8383911A).into(),
        fill_2: rgba(0x83839133).into(),
        fill_3: rgba(0x8383914D).into(),
        fill_4: rgba(0x83839166).into(),
        fill_alt: rgba(0x8383911A).into(),
        fill_disabled: rgba(0x83839114).into(),
        border_subtle: rgba(0x83839129).into(),
        border_popover: rgba(0xFFFFFF1F).into(),
        border_strong: rgba(0x8383913D).into(),
        skeleton_brush: rgba(0x83839133).into(),
        switch_bg: rgba(0x4A4D52FF).into(),
        ip_tag_bg: rgba(0x8383911A).into(),
        ip_tag_text: rgba(0xB7BCC8BF).into(),
        overlay_scrim: rgba(0x000000B3).into(),
        bg_gradient: [
            (rgba(0x1B2A33FF).into(), 0.0),
            (rgba(0x202329FF).into(), 0.55),
            (rgba(0x222428FF).into(), 1.0),
        ],
        bg_accent_glow: [
            (rgba(0x00A1D633).into(), 0.0),
            (rgba(0x00A1D600).into(), 1.0),
        ],
    }
}
