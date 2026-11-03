//! 根布局的视觉底座：背景渐变 + 主题色光晕。
//!
//! 这两层铺满窗口、不参与任何业务，属于 shell 的根布局；页面内容与浮层
//! 由组装点按顺序叠在它上面。

use gpui::prelude::*;
use gpui::*;
use yk_theme::theme::*;

/// 铺满窗口的背景层。返回的元素是绝对定位的，直接挂在根节点第一个 child。
pub fn background(t: &YkTheme) -> AnyElement {
    let stops = t.bg_gradient_stops();
    div()
        .absolute()
        .inset_0()
        // GPUI 的 linear_gradient 只吃 2 个色标，设计稿是三段；
        // 中间段与两端差异很小，这里取首尾两段近似。
        .bg(linear_gradient(
            135.0,
            linear_color_stop(stops[0].0, 0.0),
            linear_color_stop(stops[2].0, 1.0),
        ))
        .child(glow(t))
        .into_any_element()
}

/// 左上角主题色光晕。GPUI 没有径向渐变，用一条垂直渐变近似。
fn glow(t: &YkTheme) -> Div {
    let glow = t.bg_accent_glow_stops();
    div().absolute().inset_0().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(760.))
            .h(px(520.))
            .bg(linear_gradient(
                160.0,
                linear_color_stop(glow[0].0, 0.0),
                linear_color_stop(glow[1].0, 1.0),
            )),
    )
}
