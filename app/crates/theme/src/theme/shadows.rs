//! 阴影与玻璃边缘光（分亮/暗两套）。
//!
//! 对应 `YKPili/Styles/Tokens/Shadows.axaml`，由脚本转换，数值零改动。
//!
//! 「磨砂玻璃」观感有一半来自 EdgeGlow：用 inset 白色高光在内侧勾出玻璃倒角，
//! 不是真的背景模糊。GPUI 的 BoxShadow 支持 inset，因此可 1:1 复刻。
//!
//! 用法：Shadow1 卡片 / Shadow2 顶栏 / Shadow3 浮层 / Shadow4 对话框；
//! EdgeGlow1 玻璃面板、EdgeGlow2 玻璃对话框。

use gpui::{px, rgba, BoxShadow};

#[derive(Clone, Debug)]
pub struct YkShadows {
    pub shadow_1: Vec<BoxShadow>,
    pub shadow_2: Vec<BoxShadow>,
    pub shadow_3: Vec<BoxShadow>,
    pub shadow_4: Vec<BoxShadow>,
    pub edge_glow_1: Vec<BoxShadow>,
    pub edge_glow_2: Vec<BoxShadow>,
    pub shadow_card_hover: Vec<BoxShadow>,
    pub shadow_glass_card: Vec<BoxShadow>,
    pub shadow_glass_elevated: Vec<BoxShadow>,
    pub shadow_glass_modal: Vec<BoxShadow>,
}

pub fn light() -> YkShadows {
    YkShadows {
        shadow_1: vec![
        BoxShadow::new(px(0.0), px(4.0), rgba(0x00000014).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-1.0)),
        BoxShadow::new(px(0.0), px(2.0), rgba(0x0000000A).into())
            .blur_radius(px(4.0))
            .spread_radius(px(-2.0)),
        ],
        shadow_2: vec![
        BoxShadow::new(px(0.0), px(10.0), rgba(0x0000001A).into())
            .blur_radius(px(15.0))
            .spread_radius(px(-3.0)),
        BoxShadow::new(px(0.0), px(4.0), rgba(0x0000000F).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-4.0)),
        ],
        shadow_3: vec![
        BoxShadow::new(px(0.0), px(20.0), rgba(0x0000001F).into())
            .blur_radius(px(25.0))
            .spread_radius(px(-5.0)),
        BoxShadow::new(px(0.0), px(8.0), rgba(0x00000014).into())
            .blur_radius(px(10.0))
            .spread_radius(px(-6.0)),
        ],
        shadow_4: vec![
        BoxShadow::new(px(0.0), px(34.0), rgba(0x00000024).into())
            .blur_radius(px(50.0))
            .spread_radius(px(-20.0)),
        BoxShadow::new(px(0.0), px(32.0), rgba(0x0000001A).into())
            .blur_radius(px(45.0))
            .spread_radius(px(-12.0)),
        BoxShadow::new(px(0.0), px(30.0), rgba(0x0000001F).into())
            .blur_radius(px(40.0))
            .spread_radius(px(-8.0)),
        ],
        edge_glow_1: vec![
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF57).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF24).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF99).into())
            .blur_radius(px(10.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        edge_glow_2: vec![
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF57).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF24).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF9E).into())
            .blur_radius(px(14.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        shadow_card_hover: vec![
        BoxShadow::new(px(0.0), px(10.0), rgba(0x0000001F).into())
            .blur_radius(px(15.0))
            .spread_radius(px(-3.0)),
        BoxShadow::new(px(0.0), px(4.0), rgba(0x00000014).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-4.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF57).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF24).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        ],
        shadow_glass_card: vec![
        BoxShadow::new(px(0.0), px(4.0), rgba(0x00000014).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-1.0)),
        BoxShadow::new(px(0.0), px(2.0), rgba(0x0000000A).into())
            .blur_radius(px(4.0))
            .spread_radius(px(-2.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF57).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF24).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF99).into())
            .blur_radius(px(10.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        shadow_glass_elevated: vec![
        BoxShadow::new(px(0.0), px(20.0), rgba(0x0000001F).into())
            .blur_radius(px(25.0))
            .spread_radius(px(-5.0)),
        BoxShadow::new(px(0.0), px(8.0), rgba(0x00000014).into())
            .blur_radius(px(10.0))
            .spread_radius(px(-6.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF57).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF24).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF99).into())
            .blur_radius(px(10.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        shadow_glass_modal: vec![
        BoxShadow::new(px(0.0), px(34.0), rgba(0x00000024).into())
            .blur_radius(px(50.0))
            .spread_radius(px(-20.0)),
        BoxShadow::new(px(0.0), px(32.0), rgba(0x0000001A).into())
            .blur_radius(px(45.0))
            .spread_radius(px(-12.0)),
        BoxShadow::new(px(0.0), px(30.0), rgba(0x0000001F).into())
            .blur_radius(px(40.0))
            .spread_radius(px(-8.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF57).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF24).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF9E).into())
            .blur_radius(px(14.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
    }
}

pub fn dark() -> YkShadows {
    YkShadows {
        shadow_1: vec![
        BoxShadow::new(px(0.0), px(4.0), rgba(0x00000014).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-1.0)),
        BoxShadow::new(px(0.0), px(2.0), rgba(0x0000000F).into())
            .blur_radius(px(4.0))
            .spread_radius(px(-2.0)),
        ],
        shadow_2: vec![
        BoxShadow::new(px(0.0), px(10.0), rgba(0x0000001A).into())
            .blur_radius(px(15.0))
            .spread_radius(px(-3.0)),
        BoxShadow::new(px(0.0), px(4.0), rgba(0x00000014).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-4.0)),
        ],
        shadow_3: vec![
        BoxShadow::new(px(0.0), px(20.0), rgba(0x0000001F).into())
            .blur_radius(px(25.0))
            .spread_radius(px(-5.0)),
        BoxShadow::new(px(0.0), px(8.0), rgba(0x0000001A).into())
            .blur_radius(px(10.0))
            .spread_radius(px(-6.0)),
        ],
        shadow_4: vec![
        BoxShadow::new(px(0.0), px(34.0), rgba(0x00000024).into())
            .blur_radius(px(50.0))
            .spread_radius(px(-20.0)),
        BoxShadow::new(px(0.0), px(32.0), rgba(0x0000001F).into())
            .blur_radius(px(45.0))
            .spread_radius(px(-12.0)),
        BoxShadow::new(px(0.0), px(30.0), rgba(0x00000024).into())
            .blur_radius(px(40.0))
            .spread_radius(px(-8.0)),
        ],
        edge_glow_1: vec![
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF0F).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF05).into())
            .blur_radius(px(10.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        edge_glow_2: vec![
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF0F).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(14.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        shadow_card_hover: vec![
        BoxShadow::new(px(0.0), px(10.0), rgba(0x0000001F).into())
            .blur_radius(px(15.0))
            .spread_radius(px(-3.0)),
        BoxShadow::new(px(0.0), px(4.0), rgba(0x0000001A).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-4.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF0F).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        ],
        shadow_glass_card: vec![
        BoxShadow::new(px(0.0), px(4.0), rgba(0x00000014).into())
            .blur_radius(px(6.0))
            .spread_radius(px(-1.0)),
        BoxShadow::new(px(0.0), px(2.0), rgba(0x0000000F).into())
            .blur_radius(px(4.0))
            .spread_radius(px(-2.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF0F).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF05).into())
            .blur_radius(px(10.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        shadow_glass_elevated: vec![
        BoxShadow::new(px(0.0), px(20.0), rgba(0x0000001F).into())
            .blur_radius(px(25.0))
            .spread_radius(px(-5.0)),
        BoxShadow::new(px(0.0), px(8.0), rgba(0x0000001A).into())
            .blur_radius(px(10.0))
            .spread_radius(px(-6.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF0F).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF05).into())
            .blur_radius(px(10.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
        shadow_glass_modal: vec![
        BoxShadow::new(px(0.0), px(34.0), rgba(0x00000024).into())
            .blur_radius(px(50.0))
            .spread_radius(px(-20.0)),
        BoxShadow::new(px(0.0), px(32.0), rgba(0x0000001F).into())
            .blur_radius(px(45.0))
            .spread_radius(px(-12.0)),
        BoxShadow::new(px(0.0), px(30.0), rgba(0x00000024).into())
            .blur_radius(px(40.0))
            .spread_radius(px(-8.0)),
        BoxShadow::new(px(1.0), px(1.0), rgba(0xFFFFFF0F).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(-1.0), px(-1.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(1.0))
            .spread_radius(px(-0.5))
            .inset(),
        BoxShadow::new(px(0.0), px(0.0), rgba(0xFFFFFF08).into())
            .blur_radius(px(14.0))
            .spread_radius(px(0.0))
            .inset(),
        ],
    }
}
