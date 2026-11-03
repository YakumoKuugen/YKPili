//! 「正在关注」瀑布流：动态卡片 + 自适应多列布局。
//!
//! 设计要点：
//! - **瀑布流不是网格**：每张卡根据正文行数、图片数量 / 比例算出高度，
//!   再放进当前最矮的一列，列与列之间允许高度不同。
//! - 卡片分三类交互：视频卡直接展开播放器（沿用首页封面卡的入场动画）；
//!   纯文字 / 图文卡展开详情浮层；浮层外点击缩回原卡位置。
//! - 高度必须**先算再排**：GPUI 没有 masonry 布局，也不能在 prepaint 里反过来
//!   改兄弟元素位置，因此卡片内部所有可变高度都由 `card_height()` 统一推导，
//!   渲染时使用同一套行数 / 图片几何。
//!
//! 这里只做视图：数据的取用、分页与登录入口都走 [`FollowingHost`]，
//! 组装点负责状态与网络。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_components::status;
use yk_infra::dynamic::DynamicItem;
use yk_theme::theme::*;

use crate::dynamic_card::{card, card_height, DynamicCardHost};
use crate::image::ImageResidency;

/// 瀑布流卡片的目标最小宽度；实际列宽 = 可用宽按这个值均分。
const MIN_COLUMN_WIDTH: f32 = 320.0;
/// 列间距 / 卡片纵向间距。
const CARD_GAP: f32 = SPACE_4;

/// 一次「正在关注」页面的全部输入（只读快照）。
pub struct FeedState<'a> {
    pub items: &'a [DynamicItem],
    pub loading: bool,
    pub error: Option<&'a str>,
    /// 是否已经成功拉过一页；用来区分「空列表」与「还没开始」。
    pub loaded: bool,
    pub has_more: bool,
    pub loading_more: bool,
    pub signed_in: bool,
}

/// 关注流需要的宿主能力：状态快照 + 三个动作入口。
pub trait FollowingHost: DynamicCardHost {
    fn following_state(&self) -> FeedState<'_>;

    /// 手动刷新：保留旧列表，成功后再替换。
    fn following_refresh(&mut self, cx: &mut Context<Self>);

    /// 加载下一页。
    fn following_load_more(&mut self, cx: &mut Context<Self>);

    /// 未登录时的登录入口。
    fn following_sign_in(&mut self, cx: &mut Context<Self>);
}

/// 渲染关注动态页面（含标题、四态与瀑布流）。
pub fn page<V: FollowingHost + 'static>(
    t: &YkTheme,
    state: FeedState<'_>,
    visible_y: std::ops::Range<f32>,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let scale_factor = window.scale_factor();
    let available = (window.viewport_size().width.as_f32() - SPACE_6 * 2.0).max(MIN_COLUMN_WIDTH);

    let refresh = div()
        .id("following-refresh")
        .h(px(30.))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_1))
        .rounded_full()
        .bg(s.fill_1)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_secondary)
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
        .child(Icon::new(IconName::RefreshCw).size(px(14.)))
        .child("刷新")
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.following_refresh(cx);
            window.refresh();
        }));

    let header = div()
        .flex()
        .items_end()
        .justify_between()
        .w_full()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(SPACE_3))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_HEADING))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("正在关注"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(if !state.signed_in {
                            "登录后同步关注动态".to_string()
                        } else if state.loaded {
                            format!("{} 条动态 · 瀑布流", state.items.len())
                        } else {
                            "来自你关注的 UP 主".to_string()
                        }),
                ),
        )
        .children(state.signed_in.then_some(refresh));

    let body: AnyElement = if !state.signed_in {
        login_prompt(t, cx).into_any_element()
    } else if state.loading && !state.loaded {
        skeleton_columns(t, available).into_any_element()
    } else if let Some(error) = state.error {
        error_state(t, error, cx).into_any_element()
    } else if state.items.is_empty() {
        status::empty(
            t,
            "关注流还是空的",
            "关注一些 UP 主后，TA 的视频、图文和一句话动态会出现在这里",
        )
        .into_any_element()
    } else {
        waterfall(
            t,
            state.items,
            available,
            scale_factor,
            visible_y,
            active_images,
            root_bounds,
            cx,
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(header)
        .child(body)
        .when(state.has_more || state.loading_more, |el| {
            el.child(
                div().flex().justify_center().w_full().child(
                    div()
                        .id("following-load-more")
                        .h(px(34.))
                        .px(px(SPACE_5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(s.fill_1)
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(if state.loading_more {
                            s.text_tertiary
                        } else {
                            s.text_secondary
                        })
                        .cursor_pointer()
                        .when(!state.loading_more, |el| {
                            el.hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
                        })
                        .child(if state.loading_more {
                            "正在加载…"
                        } else {
                            "加载更多"
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if !this.following_state().loading_more {
                                this.following_load_more(cx);
                            }
                            window.refresh();
                        })),
                ),
            )
        })
        .into_any_element()
}

fn login_prompt<V: FollowingHost + 'static>(t: &YkTheme, cx: &mut Context<V>) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(360.))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(48.))
                .h(px(48.))
                .rounded_full()
                .bg(p.theme_primary_20)
                .child(
                    Icon::new(IconName::Users)
                        .size(px(24.))
                        .text_color(p.theme_primary),
                ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("登录后查看关注动态"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("你关注的 UP 主发布的视频、图文和一句话都会出现在这里"),
        )
        .child(
            div()
                .id("following-login")
                .mt(px(SPACE_2))
                .h(px(32.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .child("去登录")
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.following_sign_in(cx);
                })),
        )
}

fn error_state<V: FollowingHost + 'static>(
    t: &YkTheme,
    error: &str,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(320.))
        .w_full()
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("关注动态加载失败"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(error.to_string()),
        )
        .child(
            div()
                .id("following-retry")
                .mt(px(SPACE_2))
                .h(px(30.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(t.palette.theme_primary)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .child("重试")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.following_refresh(cx);
                    window.refresh();
                })),
        )
}

fn skeleton_columns(t: &YkTheme, available: f32) -> impl IntoElement {
    let cols = column_count(available);
    let col_w = (available - CARD_GAP * (cols.saturating_sub(1)) as f32) / cols as f32;
    let heights = [268.0, 196.0, 320.0, 232.0, 288.0, 180.0];
    let brush = t.surfaces.skeleton_brush;
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(CARD_GAP))
        .w_full()
        .with_animation(
            "following-skeleton",
            Animation::new(std::time::Duration::from_millis(
                status::SKELETON_PULSE_MS,
            ))
            .repeat(),
            move |el, delta| {
                let opacity = 0.35 + 0.30 * ((delta * std::f32::consts::PI).sin().abs());
                el.opacity(opacity).children((0..cols).map(|c| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(CARD_GAP))
                        .w(px(col_w))
                        .children((0..3).map(|r| {
                            let h = heights[(c * 3 + r) % heights.len()];
                            div().w_full().h(px(h)).rounded(px(RADIUS_XL)).bg(brush)
                        }))
                }))
            },
        )
}

// ================= 瀑布流布局 =================

fn column_count(available: f32) -> usize {
    (((available + CARD_GAP) / (MIN_COLUMN_WIDTH + CARD_GAP)).floor() as usize).clamp(1, 4)
}

/// 把卡片按「当前最矮列」分配；返回每列的条目下标和列高。
fn distribute(items: &[DynamicItem], col_w: f32, cols: usize) -> (Vec<Vec<usize>>, Vec<f32>) {
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); cols];
    let mut heights = vec![0.0_f32; cols];
    for (i, item) in items.iter().enumerate() {
        let target = heights
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        columns[target].push(i);
        heights[target] += card_height(item, col_w) + CARD_GAP;
    }
    (columns, heights)
}

fn waterfall<V: FollowingHost + 'static>(
    t: &YkTheme,
    items: &[DynamicItem],
    available: f32,
    scale_factor: f32,
    visible_y: std::ops::Range<f32>,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let cols = column_count(available);
    let col_w = (available - CARD_GAP * (cols.saturating_sub(1)) as f32) / cols as f32;
    let (columns, _) = distribute(items, col_w, cols);

    // 瀑布流没有严格行，按列计算：每列加载「可视卡片 + 上下各一张」。
    // 而不是用固定像素 margin，否则短卡多的时候会误留十几张。
    let mut load_flags = vec![false; items.len()];
    for column in &columns {
        let mut placements = Vec::with_capacity(column.len());
        let mut top = 0.0_f32;
        for &index in column {
            let height = card_height(&items[index], col_w);
            placements.push((index, top, top + height));
            top += height + CARD_GAP;
        }

        let first = placements
            .iter()
            .position(|(_, top, bottom)| *bottom >= visible_y.start && *top <= visible_y.end);
        let last = placements
            .iter()
            .rposition(|(_, top, bottom)| *bottom >= visible_y.start && *top <= visible_y.end);

        if let (Some(first), Some(last)) = (first, last) {
            let from = first.saturating_sub(1);
            let to = (last + 2).min(placements.len());
            for (index, _, _) in &placements[from..to] {
                load_flags[*index] = true;
            }
        } else if let Some((index, _, _)) = placements.iter().min_by(|a, b| {
            let da = if a.2 < visible_y.start {
                visible_y.start - a.2
            } else {
                a.1 - visible_y.end
            };
            let db = if b.2 < visible_y.start {
                visible_y.start - b.2
            } else {
                b.1 - visible_y.end
            };
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            // 视口刚好落在列的空隙里：只留离视口最近的一张，避免整列空白。
            load_flags[*index] = true;
        }
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(CARD_GAP))
        .w_full()
        .children(columns.into_iter().map(|indices| {
            div()
                .flex()
                .flex_col()
                .gap(px(CARD_GAP))
                .w(px(col_w))
                .children(indices.into_iter().map(|i| {
                    let item = &items[i];
                    card::<V>(
                        t,
                        item,
                        col_w,
                        card_height(item, col_w),
                        scale_factor,
                        load_flags[i],
                        active_images.clone(),
                        root_bounds.clone(),
                        cx,
                    )
                    .into_any_element()
                }))
        }))
        .into_any_element()
}
