//! 品牌色与状态色（**不随主题变化**）。
//!
//! 对应 `YKPili/Styles/Tokens/Palette.axaml`，由脚本转换，色值零改动。
//!
//! 状态色的 10/20/30 是半透明档（叠加在表面上用），70/80 用于危险按钮的按压/悬停。

use gpui::{rgba, Hsla};

/// 全套品牌色与状态色。
#[derive(Clone, Debug)]
pub struct YkPalette {
    pub bili_blue: Hsla,
    pub bili_pink: Hsla,
    pub theme_primary: Hsla,
    pub theme_primary_10: Hsla,
    pub theme_primary_20: Hsla,
    pub theme_primary_30: Hsla,
    pub theme_primary_40: Hsla,
    pub theme_primary_50: Hsla,
    pub theme_primary_60: Hsla,
    pub theme_primary_70: Hsla,
    pub theme_primary_80: Hsla,
    pub theme_primary_90: Hsla,
    pub status_info: Hsla,
    pub status_info_10: Hsla,
    pub status_info_20: Hsla,
    pub status_info_30: Hsla,
    pub status_success: Hsla,
    pub status_success_10: Hsla,
    pub status_success_20: Hsla,
    pub status_success_30: Hsla,
    pub status_warning: Hsla,
    pub status_warning_10: Hsla,
    pub status_warning_20: Hsla,
    pub status_warning_30: Hsla,
    pub status_error: Hsla,
    pub status_error_10: Hsla,
    pub status_error_20: Hsla,
    pub status_error_30: Hsla,
    pub status_error_70: Hsla,
    pub status_error_80: Hsla,
    pub text_on_accent_disabled: Hsla,
}

pub fn palette() -> YkPalette {
    YkPalette {
        bili_blue: rgba(0x00A1D6FF).into(),
        bili_pink: rgba(0xFB749AFF).into(),
        theme_primary: rgba(0x00A1D6FF).into(),
        theme_primary_10: rgba(0x00A1D61A).into(),
        theme_primary_20: rgba(0x00A1D633).into(),
        theme_primary_30: rgba(0x00A1D64D).into(),
        theme_primary_40: rgba(0x00A1D666).into(),
        theme_primary_50: rgba(0x00A1D680).into(),
        theme_primary_60: rgba(0x00A1D699).into(),
        theme_primary_70: rgba(0x00A1D6B3).into(),
        theme_primary_80: rgba(0x00A1D6CC).into(),
        theme_primary_90: rgba(0x00A1D6E6).into(),
        status_info: rgba(0x3ABFF8FF).into(),
        status_info_10: rgba(0x3ABFF81A).into(),
        status_info_20: rgba(0x3ABFF833).into(),
        status_info_30: rgba(0x3ABFF84D).into(),
        status_success: rgba(0x29A383FF).into(),
        status_success_10: rgba(0x29A3831A).into(),
        status_success_20: rgba(0x29A38333).into(),
        status_success_30: rgba(0x29A3834D).into(),
        status_warning: rgba(0xF59E0BFF).into(),
        status_warning_10: rgba(0xF59E0B1A).into(),
        status_warning_20: rgba(0xF59E0B33).into(),
        status_warning_30: rgba(0xF59E0B4D).into(),
        status_error: rgba(0xE5484DFF).into(),
        status_error_10: rgba(0xE5484D1A).into(),
        status_error_20: rgba(0xE5484D33).into(),
        status_error_30: rgba(0xE5484D4D).into(),
        status_error_70: rgba(0xE5484DB3).into(),
        status_error_80: rgba(0xE5484DCC).into(),
        text_on_accent_disabled: rgba(0xFFFFFF80).into(),
    }
}
