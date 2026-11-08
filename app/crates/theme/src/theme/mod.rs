//! 设计令牌层。
//!
//! 与 Avalonia 时代的对应关系：
//!   Palette.axaml    -> palette.rs
//!   Surfaces.axaml   -> surfaces.rs
//!   Typography.axaml -> typography.rs
//!   Dimensions.axaml -> dimensions.rs
//!   Motion.axaml     -> motion.rs
//!   Shadows.axaml    -> shadows.rs
//!
//! 约定：
//! - 视图**不写**裸色值 / 裸尺寸，一律引用这里的令牌；
//! - 颜色是一等对象（`Hsla`），主题切换靠换一套 `YkSurfaces` / `YkShadows`，
//!   不需要重建控件；
//! - 字号用 `text_size(px(n))` 显式像素，不用 GPUI 的 rem 比例（见 typography 注释）。

pub mod dimensions;
pub mod material;
pub mod motion;
pub mod palette;
pub mod shadows;
pub mod surfaces;
pub mod typography;

pub use dimensions::*;
pub use material::*;
pub use motion::*;
pub use palette::{palette, YkPalette};
pub use shadows::{YkShadows, dark as shadows_dark, light as shadows_light};
pub use surfaces::{YkSurfaces, dark as surfaces_dark, light as surfaces_light};
pub use typography::*;

pub use gpui::Hsla;

/// 一套完整的主题令牌。
///
/// 对应旧世界的「主题字典」：切换主题 = 换一个 `YkTheme`，挂到 GPUI 的全局，
/// 视图读到新值后 `cx.notify()` 重绘即可 —— **不重建控件**。
///
/// 注意：`palette`（品牌色/状态色）不随主题变化，放进来只是为了取用时少一次查找。
#[derive(Clone, Debug)]
pub struct YkTheme {
    pub is_dark: bool,
    pub palette: YkPalette,
    pub surfaces: YkSurfaces,
    pub shadows: YkShadows,
}

impl YkTheme {
    pub fn light() -> Self {
        Self {
            is_dark: false,
            palette: palette(),
            surfaces: surfaces_light(),
            shadows: shadows_light(),
        }
    }

    pub fn dark() -> Self {
        Self {
            is_dark: true,
            palette: palette(),
            surfaces: surfaces_dark(),
            shadows: shadows_dark(),
        }
    }

    /// 页面背景渐变的三段色标（对应旧 `BgGradientBrush`）。
    ///
    /// GPUI 没有 `LinearGradientBrush` 资源，渐变在视图侧用
    /// `linear_gradient(angle, stops)` 现搭；这里只给出色标定义。
    pub fn bg_gradient_stops(&self) -> [(Hsla, f32); 3] {
        self.surfaces.bg_gradient
    }

    /// 渐变之上的主题色光晕（对应旧 `BgAccentGlowBrush`）。
    pub fn bg_accent_glow_stops(&self) -> [(Hsla, f32); 2] {
        self.surfaces.bg_accent_glow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Rgba;

    /// XAML 是 #AARRGGBB，gpui 的 rgba() 是 0xRRGGBBAA。
    /// 这个测试守着两者不能弄反。
    #[test]
    fn hex_channel_order_is_correct() {
        let p = palette();
        let c: Rgba = p.bili_blue.into();
        assert_eq!((c.r * 255.0).round() as u8, 0x00);
        assert_eq!((c.g * 255.0).round() as u8, 0xA1);
        assert_eq!((c.b * 255.0).round() as u8, 0xD6);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn alpha_survives_conversion() {
        let s = surfaces_light();
        let c: Rgba = s.surface_content.into(); // #ADFFFFFF
        assert_eq!((c.a * 255.0).round() as u8, 0xAD);
        assert_eq!((c.r * 255.0).round() as u8, 0xFF);
    }

    #[test]
    fn themes_differ_on_surfaces_not_palette() {
        let l = YkTheme::light();
        let d = YkTheme::dark();
        let lc: Rgba = l.surfaces.text_primary.into();
        let dc: Rgba = d.surfaces.text_primary.into();
        assert_ne!((lc.r * 255.0) as u8, (dc.r * 255.0) as u8);
        // 品牌色在两端一致
        assert_eq!(
            Rgba::from(l.palette.bili_blue),
            Rgba::from(d.palette.bili_blue)
        );
    }
}
