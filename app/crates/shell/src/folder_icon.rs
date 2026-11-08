//! 首页左下角的悬浮文件夹图标（液态玻璃版）。
//!
//! 造型参考 [Rare UI · Folder](https://www.rareui.com/components/foldercomponent)：
//! 背板 + 三张探出头的卡片 + 带页签的前盖板。原组件靠 CSS 的 `rotateX` 把
//! 盖板**朝观众掀开**，GPUI 没有 3D 变换，这里直接在 canvas 里做透视投影：
//!
//! ```text
//! 面板空间 (x, y, 0)  ──rotateX(θ)──▶  (x, y·cosθ, −y·sinθ)
//!                               ──透视──▶  screen = pivot + p · d/(d − z)
//! ```
//!
//! 于是 hover 时盖板的顶边会**往下沉、左右变宽**（近大远小），而不是简单
//! 地把高度压扁 —— 看起来才是「往前翻开」。
//!
//! 玻璃感来自：半透明蓝的渐变填充（能透出后面的卡片）+ 顶部高光斜掠 +
//! 1px 亮描边 + 只勾顶边的高亮线。GPUI 没有 backdrop blur，所以用「半透明
//! 叠层 + 内外描边」来做出液态玻璃的观感。
//!
//! 弹簧参数沿用原组件（stiffness 120 / damping 13 / mass 1），卡片与盖板
//! 共用同一条曲线，掀盖和抽卡片是一个动作。
//!
//! 这个图标**不接导航**：纯视觉元素，点击不跳转（`occlude` 只用来挡住
//! 背后卡片，避免点它的时候把下面的视频打开）。

use gpui::prelude::*;
use gpui::*;
use yk_theme::theme::*;

/// 图标占位尺寸。红框区域约 118×115，这里按原组件 321:270 的比例收一档。
pub const FOLDER_ICON_WIDTH: f32 = 108.0;
/// 高度：盖板底边与 Dock 同一条基线。
pub const FOLDER_ICON_HEIGHT: f32 = 112.0;
/// 视口窄于这个宽度就不再画图标：Dock 居中约 620 宽，窗口再窄两边会压在一起。
pub const FOLDER_ICON_MIN_VIEWPORT: f32 = 940.0;

/// 卡片尺寸（原组件 164×214，按 ~0.34 缩下来）。
const CARD_W: f32 = 54.0;
const CARD_H: f32 = 72.0;

/// 背板（文件夹内壁）。
const BACK_TOP: f32 = 20.0;

/// 前盖板：`BODY_TOP` 是静止时盖板顶边在图标里的 y，`FLAP_H` 是盖板高度。
const BODY_TOP: f32 = 32.0;
const FLAP_H: f32 = FOLDER_ICON_HEIGHT - BODY_TOP;
const FLAP_CX: f32 = FOLDER_ICON_WIDTH / 2.0;
/// 页签：贴左边，高出盖板顶边 14pt。
const TAB_W: f32 = 52.0;
const TAB_H: f32 = 14.0;
/// 圆角：盖板 13，页签左上 12，页签右上留 2（肩线不能是胶囊头）。
const FLAP_RADIUS: f32 = 13.0;
const TAB_RADIUS: f32 = 12.0;
const TAB_SHOULDER_RADIUS: f32 = 2.0;

/// 盖板的倾斜角：静止 -15°（几乎立着），hover -44°（朝观众倒下）。
const TILT_REST_DEG: f32 = 15.0;
const TILT_HOVER_DEG: f32 = 42.0;
/// 透视距离（pt）：越小越「广角」，顶边放大越明显。
const PERSPECTIVE: f32 = 680.0;

/// 三张卡片：静止 / hover 的位置，(left, top)。从右往左画，左边的压在最上层。
const CARD_REST: [(f32, f32); 3] = [(38., 26.), (27., 20.), (16., 24.)];
const CARD_HOVER: [(f32, f32); 3] = [(43., 16.), (27., 10.), (11., 14.)];

/// 图标用到的颜色，全部从主题令牌派生。文件夹用品牌蓝，卡片用正文色
/// （暗色下是白纸），描边取白色 —— 换主题不用改这里。
#[derive(Clone, Copy)]
struct FolderColors {
    back: Hsla,
    /// 盖板顶部的玻璃色。
    glass_top: Hsla,
    /// 盖板底部的玻璃色（更实、更暗，托住整个面）。
    glass_bottom: Hsla,
    /// 卡片透出来的柔光（相当于 backdrop blur 之后的纸影）。
    paper_glow: Hsla,
    /// 顶部斜掠的高光。
    sheen: Hsla,
    /// 外描边 / 顶边高光。
    rim: Hsla,
    rim_bright: Hsla,
    card: Hsla,
    card_border: Hsla,
    line: Hsla,
    line_soft: Hsla,
    card_shadow: Hsla,
}

fn colors(t: &YkTheme) -> FolderColors {
    let s = &t.surfaces;
    let blue = t.palette.bili_blue;
    FolderColors {
        back: Hsla {
            l: blue.l * 0.42,
            a: 0.96,
            ..blue
        },
        glass_top: Hsla {
            l: blue.l * 0.84,
            a: 0.64,
            ..blue
        },
        glass_bottom: Hsla {
            l: blue.l * 0.44,
            a: 0.86,
            ..blue
        },
        paper_glow: Hsla {
            a: 0.12,
            ..s.text_primary
        },
        sheen: Hsla {
            a: 0.22,
            ..s.text_primary
        },
        rim: Hsla {
            a: 0.45,
            ..s.text_primary
        },
        rim_bright: Hsla {
            a: 0.78,
            ..s.text_primary
        },
        // 卡片故意取正文色：暗色下是浅卡片，亮色下是深卡片。
        card: s.text_primary,
        card_border: Hsla {
            a: 0.20,
            ..s.bg_base
        },
        line: Hsla {
            a: 0.85,
            ..s.surface_content_solid
        },
        // 正文行比标题浅一档：隔着玻璃只留一点纸的影子，不抢盖板。
        line_soft: s.text_tertiary,
        card_shadow: Hsla {
            a: 0.35,
            ..s.overlay_scrim
        },
    }
}

fn lerp(from: f32, to: f32, phase: f32) -> f32 {
    from + (to - from) * phase
}

/// 弹簧：一个 hover 布尔值 → 0..1（可过冲到 1.05 左右）。
fn hover_spring(hovered: bool) -> SpringAnimation<f32> {
    SpringAnimation::new(SpringConfig::new(120.0, 13.0, 1.0))
        .to(if hovered { 1.0 } else { 0.0 })
}

/// 盖板上的一个点（面板空间，y 从盖板顶边 0 到 `FLAP_H`；页签为负）投影到
/// 图标内坐标。绕底边中心旋转，再做透视除法 —— 顶边越靠近观众就越大越靠下。
fn project(phase: f32, x: f32, y: f32) -> (f32, f32) {
    let theta = lerp(TILT_REST_DEG, TILT_HOVER_DEG, phase).to_radians();
    let (sin, cos) = theta.sin_cos();
    let dy = y - FLAP_H;
    let z = -dy * sin;
    let k = PERSPECTIVE / (PERSPECTIVE - z).max(1.0);
    (
        FLAP_CX + (x - FLAP_CX) * k,
        BODY_TOP + FLAP_H + dy * cos * k,
    )
}

/// 盖板轮廓（面板空间，顺时针）：页签 + 盖板一个整体，接缝处是凹角。
const FLAP_OUTLINE: [(f32, f32, f32); 6] = [
    (0.0, -TAB_H, TAB_RADIUS),                 // 页签左上
    (TAB_W, -TAB_H, TAB_SHOULDER_RADIUS),      // 页签右上（肩）
    (TAB_W, 0.0, 0.0),                         // 凹角：页签落回盖板顶边
    (FOLDER_ICON_WIDTH, 0.0, FLAP_RADIUS),     // 盖板右上
    (FOLDER_ICON_WIDTH, FLAP_H, FLAP_RADIUS),  // 盖板右下
    (0.0, FLAP_H, FLAP_RADIUS),                // 盖板左下
];

/// 把「圆角多边形」写进 path，每个点都先投影。`stroke` 为 Some 时生成描边。
fn build_flap_path(origin: Point<Pixels>, phase: f32, stroke: Option<f32>) -> Path<Pixels> {
    const KAPPA: f32 = 0.552_284_8;
    let mut builder = match stroke {
        Some(width) => PathBuilder::stroke(px(width)),
        None => PathBuilder::fill(),
    };
    let at = |x: f32, y: f32| {
        let (sx, sy) = project(phase, x, y);
        point(origin.x + px(sx), origin.y + px(sy))
    };

    let n = FLAP_OUTLINE.len();
    let mut first = true;
    for i in 0..n {
        let (x, y, r) = FLAP_OUTLINE[i];
        let (prev_x, prev_y, _) = FLAP_OUTLINE[(i + n - 1) % n];
        let (next_x, next_y, _) = FLAP_OUTLINE[(i + 1) % n];

        let in_dir = unit(prev_x - x, prev_y - y);
        let out_dir = unit(next_x - x, next_y - y);

        if r <= 0.01 {
            let p = at(x, y);
            if first {
                builder.move_to(p);
                first = false;
            } else {
                builder.line_to(p);
            }
            continue;
        }

        let entry = at(x + in_dir.0 * r, y + in_dir.1 * r);
        let exit = at(x + out_dir.0 * r, y + out_dir.1 * r);
        if first {
            builder.move_to(entry);
            first = false;
        } else {
            builder.line_to(entry);
        }
        let ctrl_a = at(
            x + in_dir.0 * r * (1.0 - KAPPA),
            y + in_dir.1 * r * (1.0 - KAPPA),
        );
        let ctrl_b = at(
            x + out_dir.0 * r * (1.0 - KAPPA),
            y + out_dir.1 * r * (1.0 - KAPPA),
        );
        builder.cubic_bezier_to(exit, ctrl_a, ctrl_b);
    }
    builder.close();
    builder.build().unwrap_or_else(|_| Path::new(Point::default()))
}

/// 只勾「页签顶边 → 肩 → 盖板顶边」这一段：玻璃的受光边。
fn build_top_edge_path(origin: Point<Pixels>, phase: f32) -> Path<Pixels> {
    let at = |x: f32, y: f32| {
        let (sx, sy) = project(phase, x, y);
        point(origin.x + px(sx), origin.y + px(sy))
    };
    let mut builder = PathBuilder::stroke(px(1.2));
    builder.move_to(at(TAB_RADIUS, -TAB_H + 0.6));
    builder.line_to(at(TAB_W - 1.0, -TAB_H + 0.6));
    builder.line_to(at(TAB_W - 0.6, 0.6));
    builder.line_to(at(FOLDER_ICON_WIDTH - FLAP_RADIUS, 0.6));
    builder.build().unwrap_or_else(|_| Path::new(Point::default()))
}

fn unit(x: f32, y: f32) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    if len <= f32::EPSILON {
        (0.0, 0.0)
    } else {
        (x / len, y / len)
    }
}

fn transparent(color: Hsla) -> Hsla {
    Hsla { a: 0.0, ..color }
}

/// 在 canvas 里画整块盖板：玻璃填充 + 顶部高光 + 描边。
fn paint_flap(origin: Point<Pixels>, phase: f32, c: FolderColors, window: &mut Window) {
    // ① 纸影：顶部一团柔光，像卡片隔着磨砂玻璃透出来的样子。
    window.paint_path(
        build_flap_path(origin, phase, None),
        linear_gradient(
            180.,
            linear_color_stop(c.paper_glow, 0.0),
            linear_color_stop(transparent(c.paper_glow), 0.62),
        ),
    );

    // ② 玻璃主色：上亮下深，半透明让卡片的轮廓从盖板里透出来。
    window.paint_path(
        build_flap_path(origin, phase, None),
        linear_gradient(
            180.,
            linear_color_stop(c.glass_top, 0.0),
            linear_color_stop(c.glass_bottom, 1.0),
        ),
    );

    // ③ 顶部斜掠高光：液态玻璃那道「反光」。
    window.paint_path(
        build_flap_path(origin, phase, None),
        linear_gradient(
            208.,
            linear_color_stop(c.sheen, 0.0),
            linear_color_stop(transparent(c.sheen), 0.46),
        ),
    );

    // 外描边 + 受光顶边。
    window.paint_path(build_flap_path(origin, phase, Some(1.0)), c.rim);
    window.paint_path(build_top_edge_path(origin, phase), c.rim_bright);
}

/// 画图标。定位（absolute / 左下角）由调用方决定，这里只画本体。
pub fn folder_icon(t: &YkTheme, hovered: bool) -> Div {
    let c = colors(t);

    // 背板：比盖板深一档，卡片从它前面探出来。
    let back = div()
        .absolute()
        .left(px(3.))
        .top(px(BACK_TOP))
        .w(px(FOLDER_ICON_WIDTH - 6.))
        .h(px(FOLDER_ICON_HEIGHT - BACK_TOP))
        .rounded(px(FLAP_RADIUS))
        .bg(c.back)
        .border_1()
        .border_color(c.rim)
        .shadow(t.shadows.shadow_glass_elevated.clone());

    // 卡片从右到左叠（左边那张在最上层，和原组件的 DOM 顺序一致）。
    let cards = (0..3).rev().map(|i| {
        let (x0, y0) = CARD_REST[i];
        let (x1, y1) = CARD_HOVER[i];
        div()
            .absolute()
            .w(px(CARD_W))
            .h(px(CARD_H))
            .rounded(px(10.))
            .bg(Hsla {
                a: 0.97,
                ..c.card
            })
            .border_1()
            .border_color(c.card_border)
            .shadow(vec![
                BoxShadow::new(px(0.), px(3.), c.card_shadow)
                    .blur_radius(px(6.))
                    .spread_radius(px(-1.)),
            ])
            // 卡片内容全在顶部：静止时只露出标题条，hover 才看到正文行。
            .child(
                div()
                    .absolute()
                    .left(px(9.))
                    .top(px(8.))
                    .right(px(9.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    // 标题条：静止时就露在盖板外，是「卡片」的识别点。
                    .child(
                        div()
                            .w(px(30.))
                            .h(px(5.))
                            .rounded(px(2.5))
                            .bg(c.line),
                    )
                    // 正文行：放开一点，hover 掀开后才看到。
                    .child(div().h(px(6.)))
                    .child(
                        div()
                            .w_full()
                            .h(px(3.))
                            .rounded(px(1.5))
                            .bg(c.line_soft),
                    )
                    .child(
                        div()
                            .w(relative(0.82))
                            .h(px(3.))
                            .rounded(px(1.5))
                            .bg(c.line_soft),
                    ),
            )
            .with_spring(
                format!("home-folder-card-{i}"),
                hover_spring(hovered),
                move |el, phase: f32| {
                    el.left(px(lerp(x0, x1, phase)))
                        .top(px(lerp(y0, y1, phase)))
                },
            )
    });

    // 盖板：canvas 每帧按弹簧进度重新投影，画出真正的「往前翻开」。
    // `with_spring` 的 animator 每帧都会拿到新的 position，因此这里可以
    // 把 phase 捕获进 paint 闭包，不需要额外的实体或定时器。
    let flap = div()
        .absolute()
        .inset_0()
        .with_spring(
            "home-folder-flap",
            hover_spring(hovered),
            move |el, phase: f32| {
                el.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_flap(bounds.origin, phase, c, window);
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            },
        );

    div()
        .relative()
        .w(px(FOLDER_ICON_WIDTH))
        .h(px(FOLDER_ICON_HEIGHT))
        .child(back)
        .children(cards)
        .child(flap)
}

#[cfg(test)]
mod tests {
    use super::{
        project, BODY_TOP, CARD_HOVER, CARD_REST, CARD_W, FLAP_H, FOLDER_ICON_HEIGHT,
        FOLDER_ICON_WIDTH, TAB_H, TAB_W, TILT_HOVER_DEG, TILT_REST_DEG,
    };

    /// 静止时盖板顶边就在 `BODY_TOP` 附近（15° 透视只会让它略微上浮）。
    #[test]
    fn rest_flap_sits_at_body_top() {
        let (_, y) = project(0.0, 0.0, 0.0);
        assert!(
            (y - BODY_TOP).abs() < 2.0,
            "静止时盖板顶边应贴近 {BODY_TOP}，实际 {y}"
        );
        let (_, tab_y) = project(0.0, 0.0, -TAB_H);
        assert!(
            tab_y < y && tab_y > 10.0,
            "页签应高出盖板顶边，但不该探出图标：{tab_y}"
        );
        assert_eq!(FLAP_H, FOLDER_ICON_HEIGHT - BODY_TOP);
    }

    /// hover 是「往前翻开」：顶边往下沉，而且比底边更宽（近大远小）。
    #[test]
    fn hover_tilts_the_flap_towards_the_viewer() {
        assert!(TILT_HOVER_DEG > TILT_REST_DEG);
        let (_, rest_top) = project(0.0, 0.0, 0.0);
        let (_, hover_top) = project(1.0, 0.0, 0.0);
        assert!(
            hover_top - rest_top > 12.0,
            "hover 时盖板顶边应明显下沉：{rest_top} → {hover_top}"
        );

        let (left, _) = project(1.0, 0.0, 0.0);
        let (right, _) = project(1.0, FOLDER_ICON_WIDTH, 0.0);
        let widest = right - left;
        assert!(
            widest > FOLDER_ICON_WIDTH * 1.08,
            "透视下顶边应比盖板本体更宽：{widest}"
        );
        // 底边不动：它贴在图标底边上。
        let (_, bottom) = project(1.0, 0.0, FLAP_H);
        assert!((bottom - FOLDER_ICON_HEIGHT).abs() < 0.01);
    }

    /// 页签跟着同一块盖板一起转，不能单独留在原地。
    #[test]
    fn tab_tilts_with_the_flap() {
        let (_, rest_tab) = project(0.0, 0.0, -TAB_H);
        let (_, hover_tab) = project(1.0, 0.0, -TAB_H);
        assert!(hover_tab - rest_tab > 10.0);
        let (rest_left, _) = project(0.0, 0.0, -TAB_H);
        let (hover_left, _) = project(1.0, 0.0, -TAB_H);
        assert!(hover_left < rest_left, "透视下页签左缘应向外扩");
        assert!(TAB_W < FOLDER_ICON_WIDTH);
    }

    /// 卡片 hover 时只能往上探，不能把左右顶出画布。
    #[test]
    fn cards_rise_when_hovered() {
        for (i, ((x0, y0), (x1, y1))) in CARD_REST.iter().zip(CARD_HOVER).enumerate() {
            assert!(y1 < *y0, "第 {i} 张卡片 hover 应该上浮：{y1} < {y0}");
            assert!(
                x1 >= 0.0 && x1 + CARD_W <= FOLDER_ICON_WIDTH,
                "第 {i} 张卡片左右越界"
            );
            assert!(y1 >= -24.0, "第 {i} 张卡片上浮过多，会被窗口裁掉");
        }
    }

    /// hover 后盖板让出来的空间必须明显大于静止态。
    #[test]
    fn hover_exposes_more_of_the_cards() {
        let (_, rest_top) = project(0.0, 0.0, 0.0);
        let (_, hover_top) = project(1.0, 0.0, 0.0);
        for i in 0..CARD_REST.len() {
            let rest = rest_top - CARD_REST[i].1;
            let hover = hover_top - CARD_HOVER[i].1;
            assert!(
                hover >= rest + 20.0,
                "第 {i} 张卡片 hover 只多露了 {}pt",
                hover - rest
            );
        }
    }
}
