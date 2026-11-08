//! 界面材质：原来的实色表面，或液态玻璃。
//!
//! GPUI 没有背景模糊，所以「玻璃」靠三件事拼出来：
//! 1. 两段同色系渐变（上亮下暗）当底；
//! 2. 白色细边 + 主题里现成的 EdgeGlow 内高光当倒角反光；
//! 3. 玻璃投影把面板从下层内容里「抬起来」。
//!
//! 这里只提供颜色 / 阴影，不生产元素：各处按自己的尺寸、圆角拼装，
//! 这样卡片、榜单项、Dock、搜索框能共用一套材质而不用复制数值。

use gpui::{linear_color_stop, linear_gradient, px, Background, BoxShadow, Hsla};
use serde::{Deserialize, Serialize};

use super::YkTheme;

/// 界面材质。设置页可以切换，写入 `settings.json`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceMaterial {
    /// 液态玻璃：渐变 + 白色细边 + 内高光 + 玻璃投影。
    #[default]
    Glass,
    /// 原来的材质：一层实色 / 半透明实底 + 常规描边。
    Solid,
}

impl SurfaceMaterial {
    pub const ALL: [Self; 2] = [Self::Glass, Self::Solid];

    pub fn label(self) -> &'static str {
        match self {
            Self::Glass => "液态玻璃",
            Self::Solid => "原有材质",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Glass => "渐变玻璃面 + 白色倒角反光，浮层感更强",
            Self::Solid => "原来的实色 / 半透明表面，对比更硬",
        }
    }

    pub fn is_glass(self) -> bool {
        matches!(self, Self::Glass)
    }
}

/// 一块玻璃面的三件套：渐变底、白色细边、投影（含内高光）。
pub struct GlassSurface {
    pub fill: Background,
    pub border: Hsla,
    pub shadows: Vec<BoxShadow>,
}

impl GlassSurface {
    /// 以 `base` 为色相生成一块玻璃面。
    ///
    /// `top_lighten` 是上缘提亮量；两个 alpha 控制「透多少」——
    /// 浮在封面上的面板要更实（否则背景白字会印进来），
    /// 铺在页面底色上的卡片可以更透。
    pub fn new(t: &YkTheme, base: Hsla, top_lighten: f32, alpha_top: f32, alpha_bottom: f32) -> Self {
        let top = Hsla {
            l: (base.l + top_lighten).min(1.0),
            a: alpha_top,
            ..base
        };
        let bottom = Hsla {
            l: (base.l - 0.01).max(0.0),
            a: alpha_bottom,
            ..base
        };
        let mut shadows = t.shadows.shadow_glass_elevated.clone();
        shadows.extend(t.shadows.edge_glow_1.clone());
        Self {
            fill: linear_gradient(
                180.,
                linear_color_stop(top, 0.0),
                linear_color_stop(bottom, 1.0),
            ),
            border: white_rim(),
            shadows,
        }
    }
}

/// 玻璃面板的白色细边。
pub fn white_rim() -> Hsla {
    hsla_white(0.16)
}

/// 半透明白（玻璃高光 / 磨砂片）。
pub fn hsla_white(alpha: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: alpha,
    }
}

/// 主题色玻璃胶囊（Dock 选中态）：上缘提亮、下缘压深，外加一圈主题色柔光。
pub fn accent_glass(t: &YkTheme) -> GlassSurface {
    let primary = t.palette.theme_primary;
    let top = Hsla {
        l: (primary.l + 0.11).min(1.0),
        a: 0.96,
        ..primary
    };
    let bottom = Hsla {
        l: (primary.l - 0.07).max(0.0),
        a: 0.92,
        ..primary
    };
    let mut shadows = t.shadows.edge_glow_1.clone();
    shadows.push(
        BoxShadow::new(px(0.), px(6.), primary.opacity(0.38))
            .blur_radius(px(16.))
            .spread_radius(px(-2.)),
    );
    GlassSurface {
        fill: linear_gradient(
            180.,
            linear_color_stop(top, 0.0),
            linear_color_stop(bottom, 1.0),
        ),
        border: hsla_white(0.22),
        shadows,
    }
}
