//! 「正在关注 · 直播」分栏：正在直播的关注 UP 主封面网格。
//!
//! 与动态瀑布流并列（由 Dock 的二级分页切换），视觉规格见
//! `docs/关注页-动态与直播分栏.md` 第 4 节：
//!
//! - 几何直接复用首页的 `home::card_metrics` / `card::card_height`，
//!   不写第二套列宽算法；
//! - 封面复用 `cover_art` + `cover_visual(is_live: true, show_stats: false)`，
//!   「直播中」角标与压暗层都来自同一份实现；
//! - 只构建「可视行 ± 1 行」的真实卡片，其余用等高占位块 —— 与动态流
//!   同一个原因：每张卡注册一串监听器，滚两页后鼠标一动就卡。
//!
//! 这里只做视图：数据、分页与登录入口走 [`FollowingLiveHost`]。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, TestSupportExt as _, *};
use yk_components::status;
use yk_infra::live::LiveRoom;
use yk_theme::theme::*;

use crate::card::{animate_cover_zoom, card_height};
use crate::cover::{cover_art, cover_visual, CoverVisual};
use crate::dynamic_card::CardOrigin;
use crate::following::FollowingHost;
use crate::home::card_metrics;
use crate::image::ImageResidency;

/// 关注页的二级分栏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowingTab {
    #[default]
    Videos,
    Live,
}

impl FollowingTab {
    /// 胶囊顺序：动态在前、直播在后。
    pub const ALL: [FollowingTab; 2] = [FollowingTab::Videos, FollowingTab::Live];

    pub fn label(self) -> &'static str {
        match self {
            FollowingTab::Videos => "动态",
            FollowingTab::Live => "直播",
        }
    }

    pub fn index(self) -> usize {
        match self {
            FollowingTab::Videos => 0,
            FollowingTab::Live => 1,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// 直播分栏的全部状态（app 持有、视图只读）。
#[derive(Debug, Default)]
pub struct FollowingLiveState {
    pub items: Vec<LiveRoom>,
    pub loading: bool,
    pub loading_more: bool,
    pub refreshing: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub has_more: bool,
    /// 下一次「加载更多」要请求的页码（第一页成功后变为 2）。
    pub page: u32,
    /// 请求代际：登录 / 退出 / 刷新 / 翻页只接受最后一次结果。
    pub generation: u64,
    /// 最近一次成功加载的时刻：切回分栏时用它判断要不要静默刷新。
    pub last_ok_at: Option<std::time::Instant>,
}

impl FollowingLiveState {
    /// 距上次成功加载是否已超过静默刷新间隔。
    pub fn stale(&self, interval: std::time::Duration) -> bool {
        match self.last_ok_at {
            Some(at) => at.elapsed() >= interval,
            None => true,
        }
    }

    /// 清空并推进代际：退出登录 / 换账号时调用。
    pub fn reset(&mut self) {
        self.items.clear();
        self.loading = false;
        self.loading_more = false;
        self.refreshing = false;
        self.loaded = false;
        self.error = None;
        self.has_more = false;
        self.page = 0;
        self.last_ok_at = None;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// 视图用的一次性只读快照。
pub struct LiveFeedState<'a> {
    pub items: &'a [LiveRoom],
    pub loading: bool,
    pub loading_more: bool,
    pub refreshing: bool,
    pub loaded: bool,
    pub error: Option<&'a str>,
    pub has_more: bool,
    pub signed_in: bool,
    /// 当前 hover 的卡片下标（与首页共用一套 hover 状态机）。
    pub hovered: Option<usize>,
    pub leaving: Option<usize>,
    pub hover_generation: u64,
    pub material: SurfaceMaterial,
}

/// 直播分栏需要的宿主能力：在关注页能力之上，补直播列表的状态与动作。
pub trait FollowingLiveHost: FollowingHost {
    fn live_state(&self) -> LiveFeedState<'_>;

    /// 手动刷新当前分栏（保留旧列表）。
    fn live_refresh(&mut self, cx: &mut Context<Self>);

    /// 加载下一页。
    fn live_load_more(&mut self, cx: &mut Context<Self>);

    /// 未登录时的登录入口。
    fn live_sign_in(&mut self, cx: &mut Context<Self>);

    /// 点封面：打开直播间（入场几何已经冻结）。
    fn live_open_room(
        &mut self,
        room: LiveRoom,
        origin: Option<CardOrigin>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// 点主播昵称：打开 UP 主空间浮窗（与头像同一个入口）。
    fn live_open_up(
        &mut self,
        mid: i64,
        name: String,
        face: String,
        opener: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// 空态按钮：切回动态分栏。
    fn live_show_videos(&mut self, cx: &mut Context<Self>);
}

/// 渲染直播分栏（四态 + 封面网格 + 加载更多）。
pub fn page<V: FollowingLiveHost + 'static>(
    t: &YkTheme,
    state: LiveFeedState<'_>,
    visible_y: std::ops::Range<f32>,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let available = (window.viewport_size().width.as_f32() - SPACE_6 * 2.0)
        .max(COVER_CELL_WIDTH);
    let (cols, card_w) = card_metrics(available);
    let card_h = card_height(card_w);

    let body: AnyElement = if !state.signed_in {
        login_prompt(t, cx).into_any_element()
    } else if state.loading && !state.loaded {
        skeleton(t, cols, card_w, card_h).into_any_element()
    } else if state.items.is_empty() {
        match state.error {
            Some(error) => error_state(t, error, cx).into_any_element(),
            None if state.loaded => empty_state(t, cx).into_any_element(),
            None => skeleton(t, cols, card_w, card_h).into_any_element(),
        }
    } else {
        grid(
            t,
            &state,
            cols,
            card_w,
            card_h,
            available,
            visible_y,
            active_images,
            root_bounds,
            cx,
        )
    };

    // 底部：加载更多 / 翻页失败 / 刷新失败（有旧数据）三选一。
    let footer: AnyElement = if state.has_more || state.loading_more {
        div()
            .flex()
            .justify_center()
            .w_full()
            .child(
                div()
                    .id("following-live-load-more")
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
                        if !this.live_state().loading_more {
                            this.live_load_more(cx);
                        }
                        window.refresh();
                    })),
            )
            .into_any_element()
    } else if let (Some(error), false) = (state.error, state.items.is_empty()) {
        div()
            .flex()
            .justify_center()
            .w_full()
            .child(
                div()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_tertiary)
                    .child(format!("刷新失败，显示上次结果（{error}）")),
            )
            .into_any_element()
    } else {
        div().into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(body)
        .child(footer)
        .into_any_element()
}

fn login_prompt<V: FollowingLiveHost + 'static>(
    t: &YkTheme,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .id("following-live-login-card")
        .test_support()
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
                    Icon::new(IconName::Radio)
                        .size(px(24.))
                        .text_color(p.theme_primary),
                ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("登录后查看正在直播的关注"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("关注的 UP 主开播后会出现在这里"),
        )
        .child(
            div()
                .id("following-live-login")
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
                    this.live_sign_in(cx);
                })),
        )
}

fn error_state<V: FollowingLiveHost + 'static>(
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
                .child("直播列表加载失败"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(error.to_string()),
        )
        .child(
            div()
                .id("following-live-retry")
                .test_support()
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
                    this.live_refresh(cx);
                    window.refresh();
                })),
        )
}

/// 空态：刻意不写「暂未开播」—— P0 不轮询，不能暗示客户端会自己更新。
fn empty_state<V: FollowingLiveHost + 'static>(
    t: &YkTheme,
    cx: &mut Context<V>,
) -> impl IntoElement {
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
                .bg(p.bili_pink.opacity(0.16))
                .child(
                    Icon::new(IconName::Radio)
                        .size(px(24.))
                        .text_color(p.bili_pink),
                ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("关注的主播都没开播"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("开播后会出现在这里，先去动态看看"),
        )
        .child(
            div()
                .id("following-live-show-videos")
                .test_support()
                .mt(px(SPACE_2))
                .h(px(32.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded_full()
                .bg(s.fill_1)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
                .child("查看关注动态")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.live_show_videos(cx);
                    window.refresh();
                })),
        )
}

fn skeleton(t: &YkTheme, cols: usize, card_w: f32, card_h: f32) -> impl IntoElement {
    let brush = t.surfaces.skeleton_brush;
    let cover_w = (card_w - SPACE_2 * 2.0).max(1.0);
    let cover_h = cover_w * (COVER_ITEM_HEIGHT / COVER_ITEM_WIDTH);
    let ease = ease_standard();
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .with_animation(
            "following-live-skeleton",
            Animation::new(std::time::Duration::from_millis(status::SKELETON_PULSE_MS))
                .repeat(),
            move |el, delta| {
                // 三角波：0 → 0.5 → 1 对应 1 → 0.45 → 1。
                let phase = if delta < 0.5 {
                    delta * 2.0
                } else {
                    (1.0 - delta) * 2.0
                };
                el.opacity(1.0 - 0.55 * ease(phase))
            },
        )
        .children((0..3).map(|_| {
            div()
                .flex()
                .justify_center()
                .gap(px(SPACE_4))
                .w_full()
                .h(px(card_h))
                .children((0..cols).map(move |_| {
                    div()
                        .w(px(card_w))
                        .h(px(card_h))
                        .flex()
                        .flex_col()
                        .p(px(SPACE_2))
                        .child(
                            div()
                                .w(px(cover_w))
                                .h(px(cover_h))
                                .rounded(px(MEDIA_RADIUS))
                                .bg(brush),
                        )
                        .child(
                            div()
                                .mt(px(SPACE_1))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .h(px(11.))
                                        .w(relative(0.92))
                                        .rounded(px(RADIUS_SM))
                                        .bg(brush),
                                )
                                .child(
                                    div()
                                        .h(px(11.))
                                        .w(relative(0.64))
                                        .rounded(px(RADIUS_SM))
                                        .bg(brush),
                                ),
                        )
                        .child(
                            div()
                                .mt(px(2.))
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().w(px(18.)).h(px(18.)).rounded_full().bg(brush))
                                .child(
                                    div()
                                        .h(px(11.))
                                        .flex_1()
                                        .rounded(px(RADIUS_SM))
                                        .bg(brush),
                                ),
                        )
                }))
        }))
}

#[allow(clippy::too_many_arguments)]
fn grid<V: FollowingLiveHost + 'static>(
    t: &YkTheme,
    state: &LiveFeedState<'_>,
    cols: usize,
    card_w: f32,
    card_h: f32,
    available: f32,
    visible_y: std::ops::Range<f32>,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let items = state.items;
    let row_h = card_h + SPACE_4;
    let rows = items.len().div_ceil(cols);

    // 只构建可视行 ± 1 行；其余用等高占位，滚动条高度不变。
    let first = (visible_y.start.max(0.0) / row_h).floor() as usize;
    let last = (visible_y.end.max(0.0) / row_h).ceil() as usize;
    let from = first.saturating_sub(1);
    let to = (last + 2).min(rows).max(from.min(rows));

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .children((0..rows).map(|row| {
            if row < from || row >= to {
                return div().w_full().h(px(card_h)).into_any_element();
            }
            let start = row * cols;
            let end = (start + cols).min(items.len());
            div()
                .flex()
                .justify_center()
                .gap(px(SPACE_4))
                .w_full()
                .h(px(card_h))
                .children((start..end).map(|index| {
                    card(
                        t,
                        state,
                        index,
                        &items[index],
                        card_w,
                        available,
                        active_images.clone(),
                        root_bounds.clone(),
                        cx,
                    )
                    .into_any_element()
                }))
                .into_any_element()
        }))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn card<V: FollowingLiveHost + 'static>(
    t: &YkTheme,
    state: &LiveFeedState<'_>,
    index: usize,
    room: &LiveRoom,
    card_w: f32,
    available: f32,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let cover_w = (card_w - SPACE_2 * 2.0).max(1.0);
    let hovered = state.hovered == Some(index);
    let leaving = state.leaving == Some(index);
    let hover_generation = state.hover_generation;
    // 悬停表面跟首页卡片同一档：液态玻璃给玻璃投影，实色给常用卡片投影。
    let hover_shadow = if state.material.is_glass() {
        GlassSurface::new(t, s.fill_2, 0.04, 0.40, 0.52).shadows.clone()
    } else {
        t.shadows.shadow_glass_card.clone()
    };
    let room_for_click = room.clone();
    let title = room.title.clone();

    // 封面几何在 prepaint 里冻结：点击时播放器 / 详情浮层的入场动画从这里起飞。
    let cover_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let cover_bounds_for_prepaint = cover_bounds.clone();
    let root_for_click = root_bounds.clone();

    let cover_url = room.cover_thumb_url();
    let show_cover = !cover_url.is_empty();
    if show_cover {
        active_images.borrow_mut().mark(&cover_url);
    }
    if !room.face.is_empty() {
        active_images.borrow_mut().mark(&room.face_thumb_url());
    }
    let art = cover_art(
        &title,
        if show_cover { cover_url.as_str() } else { "" },
        None,
        s.fill_3,
    );
    let art = animate_cover_zoom(art, hovered, leaving, index, hover_generation);
    let mut cover = cover_visual(
        t,
        CoverVisual {
            art: art.into_any_element(),
            views: 0,
            danmaku: 0,
            duration_secs: 0,
            is_live: true,
            show_stats: false,
        },
        COVER_ITEM_WIDTH / COVER_ITEM_HEIGHT,
    )
    .id(("live-cover", index));

    // 「N 看过」压在左下角的白字；没有人数就不画，避免假数据。
    let watched = room.watched_label();
    if !watched.is_empty() {
        cover = cover.child(
            div()
                .absolute()
                .left(px(SPACE_2))
                .bottom(px(6.))
                .text_size(px(FONT_SIZE_DATA_OVERLAY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xFFFFFF))
                .child(format!("{watched} 看过")),
        );
    }
    // hover 时在封面中央浮出播放入口，明确「点封面 = 进入直播间」。
    if hovered || leaving {
        cover = cover.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(SPACE_1))
                .opacity(if hovered { 1.0 } else { 0.0 })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(36.))
                        .h(px(36.))
                        .rounded_full()
                        .bg(rgba(0x000000A6))
                        .child(
                            Icon::new(IconName::Play)
                                .size(px(16.))
                                .text_color(rgb(0xFFFFFF)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0xFFFFFF))
                        .child("进入直播间"),
                ),
        );
    }

    let face = room.face_thumb_url();
    // 昵称和头像一样可以点：比 18px 头像更大的热区，否则「这个人是谁」很难点中。
    let name_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let name_sink = name_bounds.clone();
    let (up_mid, up_name, up_face) = (room.uid, room.uname.clone(), room.face.clone());
    let byline = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(18.))
        .mt(px(2.))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_tertiary)
        .child(V::card_up_avatar(
            SharedString::from(format!("following-live-up-{}", room.uid)),
            room.uid,
            room.uname.clone(),
            room.face.clone(),
            18.0,
            yk_components::avatar::Avatar::new(18.0)
                .url(face.clone())
                .background(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .build(),
            cx,
        ))
        .child(
            div()
                .id(("following-live-up", index))
                .relative()
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .hover(|style| style.opacity(0.86))
                .child(
                    canvas(
                        move |bounds, _window, _cx| name_sink.set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    div()
                        .truncate()
                        .text_color(s.text_tertiary)
                        .child(room.uname.clone()),
                )
                .on_click(cx.listener(move |host, _: &ClickEvent, window, cx| {
                    // 昵称点击不能顺带触发整卡点击（否则一边开空间一边起播）。
                    cx.stop_propagation();
                    let opener = name_bounds.get().unwrap_or_else(|| Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(80.), px(18.)),
                    });
                    host.live_open_up(
                        up_mid,
                        up_name.clone(),
                        up_face.clone(),
                        opener,
                        window,
                        cx,
                    );
                })),
        )
        .when(!room.area_name.is_empty(), |el| {
            el.child(
                div()
                    .flex_shrink_0()
                    .text_color(s.text_quaternary)
                    .child("·"),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_quaternary)
                    .child(room.area_name.clone()),
            )
        });

    let content = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .p(px(SPACE_2))
        .child(cover)
        .child(
            div()
                .w(px(cover_w))
                .flex()
                .flex_col()
                .child(
                    div()
                        .mt(px(SPACE_1))
                        .h(px(LINE_HEIGHT_CONTROL * 2.))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(2)
                        .child(title.clone()),
                )
                .child(byline),
        )
        .on_children_prepainted(move |children, _window, _cx| {
            if let Some(bounds) = children.first().copied() {
                cover_bounds_for_prepaint.set(Some(bounds));
            }
        });

    div()
        .id(("live-card", index))
        .test_support()
        .relative()
        .w(px(card_w))
        .h(px(card_height(card_w)))
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
        .bg(rgba(0x00000000))
        .border_1()
        .border_color(rgba(0x00000000))
        .when(hovered, |el| {
            el.bg(s.fill_2)
                .border_color(s.border_strong)
                .shadow(hover_shadow.clone())
        })
        .cursor_pointer()
        // 保持与首页卡片一致的 hover 状态机：滚动期间只跟状态、不推进动画代际。
        .on_hover(cx.listener(move |host, hovered: &bool, _window, cx| {
            if host.card_is_playing() && !host.card_above_player() {
                return;
            }
            let animate = !host.card_scroll_settling();
            if host.card_update_hover(index, *hovered, animate) {
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |host, _: &ClickEvent, window, cx| {
            if (host.card_is_playing() && !host.card_above_player())
                || host.card_close_requested()
            {
                return;
            }
            let origin = CardOrigin {
                bounds: cover_bounds.get().unwrap_or_else(|| Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(cover_w), px(cover_w * COVER_ITEM_HEIGHT / COVER_ITEM_WIDTH)),
                }),
                cover_bounds: cover_bounds.get(),
                root_size: root_for_click
                    .get()
                    .map(|bounds| bounds.size)
                    .unwrap_or_else(|| size(px(available), px(available))),
            };
            host.live_open_room(room_for_click.clone(), Some(origin), window, cx);
            window.refresh();
        }))
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::{FollowingLiveState, FollowingTab};

    #[test]
    fn tab_indexes_are_stable() {
        assert_eq!(FollowingTab::Videos.index(), 0);
        assert_eq!(FollowingTab::Live.index(), 1);
        assert_eq!(FollowingTab::from_index(0), Some(FollowingTab::Videos));
        assert_eq!(FollowingTab::from_index(1), Some(FollowingTab::Live));
        assert_eq!(FollowingTab::from_index(2), None);
        assert_eq!(FollowingTab::Videos.label(), "动态");
        assert_eq!(FollowingTab::Live.label(), "直播");
    }

    #[test]
    fn reset_clears_list_and_advances_generation() {
        let mut state = FollowingLiveState::default();
        state.loaded = true;
        state.page = 3;
        state.has_more = true;
        state.items = vec![yk_infra::live::LiveRoom::default()];
        let generation = state.generation;
        state.reset();
        assert!(state.items.is_empty());
        assert!(!state.loaded);
        assert!(!state.has_more);
        assert_eq!(state.page, 0);
        assert!(state.generation > generation);
    }

    #[test]
    fn staleness_drives_the_sixty_second_refresh() {
        let mut state = FollowingLiveState::default();
        assert!(state.stale(std::time::Duration::from_secs(60)), "没加载过算过期");
        state.last_ok_at = Some(std::time::Instant::now());
        assert!(!state.stale(std::time::Duration::from_secs(60)));
        state.last_ok_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(61));
        assert!(state.stale(std::time::Duration::from_secs(60)));
    }
}
