//! 排行榜页（设计文档 12.3）。
//!
//! 与首页推荐刻意做得不一样：名次徽标 + 榜单口径说明 + UP 主署名行。
//! 数据来自 `/x/web-interface/ranking/v2`，整榜一次拉取、没有分页；
//! 切分区只是换 `rid` 重新拉同一张榜。
//!
//! 离线 / 错误态保留已经拉到的旧榜（不闪空），只有首次加载才整页骨架。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::base::ElementExt as _;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_core::api::client::ReqwestClient;
use yk_core::ranking::{self, RankItem, RankPage, DEFAULT_NOTE, ZONES};
use yk_ui::status;
use yk_ui::theme::*;

use crate::{animate_cover_zoom, player, AppRoot, ImageResidency};

/// 一行榜单项固定高（与设计稿 12.3 的 8 行骨架同几何）。
const ROW_HEIGHT: f32 = 104.0;
/// 榜单封面 16:9。
const ROW_COVER_WIDTH: f32 = 160.0;
const ROW_COVER_HEIGHT: f32 = 90.0;

/// 排行榜页状态。
#[derive(Debug, Clone)]
pub struct RankState {
    /// 当前选中的分区在 [`ZONES`] 里的下标。
    pub zone: usize,
    pub items: Vec<RankItem>,
    pub note: String,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    /// 请求代际：快速切分区时只接受最后一次结果。
    pub generation: u64,
}

impl Default for RankState {
    fn default() -> Self {
        Self {
            zone: 0,
            items: Vec::new(),
            note: DEFAULT_NOTE.to_string(),
            loading: false,
            loaded: false,
            error: None,
            generation: 0,
        }
    }
}

impl RankState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn zone_rid(&self) -> u32 {
        ZONES.get(self.zone).map(|z| z.rid).unwrap_or(0)
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ================= 数据加载 =================

impl AppRoot {
    /// 首次进入排行榜：已加载 / 正在加载都不重复请求。
    pub(crate) fn ensure_rank(&mut self, cx: &mut Context<Self>) {
        if self.rank.loaded || self.rank.loading {
            return;
        }
        self.load_rank(cx);
    }

    /// 重新拉当前分区的榜。保留旧内容，成功后再替换。
    pub(crate) fn load_rank(&mut self, cx: &mut Context<Self>) {
        if self.rank.loading {
            return;
        }
        self.rank.generation = self.rank.generation.wrapping_add(1);
        let generation = self.rank.generation;
        let rid = self.rank.zone_rid();
        self.rank.loading = true;
        self.rank.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    ranking::fetch_ranking(&client, rid)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.rank.generation != generation {
                    return;
                }
                view.rank.loading = false;
                match result {
                    Ok(page) => {
                        eprintln!(
                            "[rank] rid={rid} 拉取成功：{} 条（{}）",
                            page.items.len(),
                            page.note
                        );
                        view.apply_rank_page(page);
                        view.rank.loaded = true;
                        view.rank.error = None;
                    }
                    Err(error) => {
                        eprintln!("[rank] rid={rid} 加载失败：{error}");
                        view.rank.error = Some(error);
                        view.rank.loaded = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_rank_page(&mut self, page: RankPage) {
        self.rank.items = page.items;
        if !page.note.trim().is_empty() {
            self.rank.note = page.note;
        }
    }

    /// 切分区：清空旧榜再拉新的，避免用户把上一张榜误认为当前分区。
    pub(crate) fn select_rank_zone(&mut self, zone: usize, cx: &mut Context<Self>) {
        if zone >= ZONES.len() || self.rank.zone == zone {
            return;
        }
        self.rank.zone = zone;
        self.rank.items.clear();
        self.rank.loaded = false;
        self.rank.error = None;
        self.rank.loading = false;
        self.load_rank(cx);
    }

    /// 打开榜单里的视频：与首页 / 搜索共用同一个播放器实例。
    ///
    /// `entrance` 是点击时冻结的左侧封面几何；有它时播放器才从封面展开、
    /// 关闭时缩回同一张封面（与首页 / 历史页同一条入场动画）。
    pub(crate) fn open_rank_video(
        &mut self,
        bvid: String,
        title: String,
        entrance: Option<player::CoverSource>,
        cx: &mut Context<Self>,
    ) {
        if self.playing.is_some() || self.close_player_requested || bvid.trim().is_empty() {
            return;
        }
        eprintln!("[rank] 进入播放浮窗：{title}（{bvid}）");
        self.ensure_player(cx);
        if let Some(player) = self.playing.clone() {
            // 先给入场起点，再换播放源；播放器实例本身不重建。
            if let Some(source) = entrance {
                let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
            }
            let _ = player.update(cx, |view, cx| view.open_bvid(bvid, cx));
        }
        cx.notify();
    }
}

// ================= 渲染 =================

pub(crate) fn page(
    t: &YkTheme,
    state: &RankState,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    window: &Window,
    scroll: &gpui::component::VirtualListScrollHandle,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    // 榜单一次 100 行，全部把封面 / 头像塞进页面图片缓存会浪费几十 MB。
    // 这里按滚动偏移只登记「可视 + 两行缓冲」，与首页封面同一套驻留策略；
    // 滚动看门狗每 60ms 会 notify，滚到哪一批就加载哪一批。
    let pitch = ROW_HEIGHT + SPACE_2;
    let offset = scroll.base_handle().offset().y.as_f32();
    let viewport = (window.viewport_size().height.as_f32() - TOP_BAR_HEIGHT).max(ROW_HEIGHT);
    let top_inset = 148.0;
    let first = ((-offset - top_inset - 120.0).max(0.0) / pitch).floor() as usize;
    let last = (((-offset - top_inset + viewport) + 120.0).max(0.0) / pitch).ceil() as usize + 1;
    let first = first.min(state.items.len());
    let last = last.min(state.items.len());
    {
        let mut residency = active_images.borrow_mut();
        for item in &state.items[first..last] {
            residency.mark(&item.cover_thumb_url());
            residency.mark(&item.face_thumb_url());
        }
    }

    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(SPACE_3))
        .w_full()
        .child(header(t, state))
        .child(zone_row(t, state, cx));

    if state.error.is_some() && !state.items.is_empty() {
        col = col.child(error_banner(t, state, cx));
    }

    let body: AnyElement = if state.loading && state.items.is_empty() {
        skeleton_rows(t).into_any_element()
    } else if let Some(error) = state.error.as_ref().filter(|_| state.items.is_empty()) {
        error_state(t, error, cx)
    } else if state.items.is_empty() {
        status::empty(t, "该分区暂无排行", "换一个分区看看，或稍后重试").into_any_element()
    } else {
        let mut list = div().flex().flex_col().gap(px(SPACE_2)).w_full();
        for (index, item) in state.items.iter().enumerate() {
            let load_images = index >= first && index < last;
            list = list.child(row(
                t,
                item,
                index,
                hovered_card == Some(index),
                leaving_card == Some(index),
                hover_generation,
                layout_bounds.clone(),
                load_images,
                active_images.clone(),
                cx,
            ));
        }
        list.child(
            div()
                .w_full()
                .py(px(SPACE_4))
                .flex()
                .justify_center()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(t.surfaces.text_quaternary)
                .child(format!("已经到底了 · 共 {} 条", state.items.len())),
        )
        .into_any_element()
    };

    col.child(body).into_any_element()
}

fn header(t: &YkTheme, state: &RankState) -> AnyElement {
    let s = &t.surfaces;
    let zone = ZONES.get(state.zone).map(|z| z.label).unwrap_or("全站");
    let note = if state.note.trim().is_empty() {
        DEFAULT_NOTE
    } else {
        state.note.as_str()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_1))
        .w_full()
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_DISPLAY))
                        .line_height(px(LINE_HEIGHT_HEADING))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("视频排行榜"),
                )
                .child(
                    div()
                        .pb(px(1.))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(format!("{zone}榜")),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(
                    Icon::new(IconName::Trophy)
                        .size(px(14.))
                        .text_color(s.text_quaternary),
                )
                .child(note.to_string()),
        )
        .into_any_element()
}

fn zone_row(t: &YkTheme, state: &RankState, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(SPACE_2))
        .w_full();
    for (index, zone) in ZONES.iter().enumerate() {
        let active = index == state.zone;
        let fg = if active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let bg = if active { p.theme_primary } else { s.fill_2 };
        let hover = if active { p.theme_primary } else { s.fill_3 };
        row = row.child(
            div()
                .id(("rank-zone", index))
                .h(px(32.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded_full()
                .bg(bg)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(if zone.rid == 0 {
                    "全站榜".to_string()
                } else {
                    zone.label.to_string()
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.select_rank_zone(index, cx);
                    window.refresh();
                })),
        );
    }
    row.into_any_element()
}

fn row(
    t: &YkTheme,
    item: &RankItem,
    index: usize,
    hovered: bool,
    leaving: bool,
    hover_generation: u64,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    load_images: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let rank = item.rank;
    let top3 = rank <= 3;
    let rank_color = if top3 {
        p.theme_primary
    } else {
        s.text_tertiary
    };
    // 可视范围之外不挂 `img()`：图片地址仍留在模型里，滚回来再加载。
    let cover = if load_images {
        item.cover_thumb_url()
    } else {
        String::new()
    };
    if !cover.is_empty() {
        active_images.borrow_mut().mark(&cover);
    }
    let face = if load_images {
        item.face_thumb_url()
    } else {
        String::new()
    };
    if !face.is_empty() {
        active_images.borrow_mut().mark(&face);
    }

    let title = if item.title.trim().is_empty() {
        "标题缺失".to_string()
    } else {
        item.title.clone()
    };
    let up_name = if item.up_name.trim().is_empty() {
        "未知 UP 主".to_string()
    } else {
        item.up_name.clone()
    };
    let bvid = item.bvid.clone();

    let mut meta = div()
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_tertiary)
        .child(
            div()
                .relative()
                .w(px(20.))
                .h(px(20.))
                .rounded_full()
                .overflow_hidden()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .when(!face.is_empty(), |el| {
                    el.child(
                        img(ImageSource::from(face.clone()))
                            .w(px(20.))
                            .h(px(20.))
                            .object_fit(ObjectFit::Cover),
                    )
                }),
        )
        .child(
            div()
                .max_w(px(180.))
                .truncate()
                .text_color(s.text_secondary)
                .child(up_name),
        )
        // 播放 / 弹幕 / 时长已经在封面的数据层上，这里不再重复一遍；
        // 同一张卡上同一组数字出现两次，用户会以为是两个指标。
        ;
    if !item.tname.trim().is_empty() {
        meta = meta.child(meta_dot(t)).child(item.tname.clone());
    }

    let mut side = div()
        .flex_shrink_0()
        .w(px(96.))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(3.));
    if let Some(label) = item.rise_label() {
        side = side.child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.status_success)
                .child(label),
        );
    }
    if let Some(label) = item.his_rank_label() {
        side = side.child(
            div()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_quaternary)
                .child(label),
        );
    }

    // 封面视觉：渐变卡面 + 真实封面 + 播放/弹幕/时长全部走公共组件
    // （与首页卡片、UP 主浮窗同一份实现）；悬停 1.04 缓推只作用在画面层，
    // 名次、标题等不跟着放大。
    let source_placeholder = yk_core::feed::PLACEHOLDER_PALETTE
        [index % yk_core::feed::PLACEHOLDER_PALETTE.len()]
    .to_string();
    let art = animate_cover_zoom(
        crate::cover_art(&title, &cover, Some(&source_placeholder), s.fill_3),
        hovered,
        leaving,
        index,
        hover_generation,
    );
    let cover_visual = crate::cover_visual(
        t,
        crate::CoverVisual {
            art,
            views: item.view_count,
            danmaku: item.danmaku_count,
            duration_secs: item.duration_secs,
            is_live: false,
            show_stats: true,
        },
        ROW_COVER_WIDTH / ROW_COVER_HEIGHT,
    );

    // 封面实际矩形：prepaint 写入，点击时作为播放浮窗的展开 / 收回起点。
    // 记的是固定 160×90 外框（不含缓推），关闭时缩回的就是左侧这张封面本身。
    let cover_source: Rc<RefCell<Option<player::CoverSource>>> = Rc::new(RefCell::new(None));
    let cover_source_for_prepaint = cover_source.clone();
    let source_bvid = bvid.clone();
    let source_title = title.clone();
    let source_image_url = cover.clone();
    let cover_el = div()
        .relative()
        .flex_shrink_0()
        .w(px(ROW_COVER_WIDTH))
        .child(cover_visual)
        .on_prepaint(move |bounds, _window, _cx| {
            let Some(root) = root_bounds.get() else {
                return;
            };
            *cover_source_for_prepaint.borrow_mut() = Some(player::CoverSource {
                bounds: Bounds {
                    origin: point(
                        bounds.origin.x - root.origin.x,
                        bounds.origin.y - root.origin.y,
                    ),
                    size: bounds.size,
                },
                root_size: root.size,
                bvid: source_bvid.clone(),
                title: source_title.clone(),
                image_url: source_image_url.clone(),
                placeholder_color: source_placeholder.clone(),
            });
        });

    div()
        .id(("rank-row", rank))
        .flex()
        .items_center()
        .gap(px(SPACE_3))
        .h(px(ROW_HEIGHT))
        .px(px(SPACE_3))
        .rounded(px(CARD_RADIUS))
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(s.surface_content_hover)
                .border_color(s.border_strong)
        })
        // hover 状态驱动封面缓推：滚动期间不推进动画代际，
        // 避免卡片从指针下滑过时缩放反复重置。
        .on_hover(cx.listener(move |this, is_hovered: &bool, _window, cx| {
            if this.playing.is_some() {
                return;
            }
            let animate = !this.scroll_settling();
            if this.update_card_hover(index, *is_hovered, animate) {
                cx.notify();
            }
        }))
        .child(
            div()
                .w(px(34.))
                .flex_shrink_0()
                .text_size(px(if top3 { 22. } else { 17. }))
                .font_weight(if top3 {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                })
                .text_color(rank_color)
                .child(format!("{rank:02}")),
        )
        .child(cover_el)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_BODY))
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(2)
                        .child(title.clone()),
                )
                .child(meta),
        )
        .child(side)
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            // 在展开浮窗、页面重排之前冻结封面几何；窗口 resize 过时用最新根尺寸。
            let mut entrance = cover_source.borrow().clone().filter(|s| s.bvid == bvid);
            if let (Some(source), Some(root)) = (entrance.as_mut(), this.layout_bounds.get()) {
                source.root_size = root.size;
            }
            this.open_rank_video(bvid.clone(), title.clone(), entrance, cx);
            window.refresh();
        }))
        .into_any_element()
}

fn meta_dot(t: &YkTheme) -> AnyElement {
    div()
        .text_color(t.surfaces.text_quaternary)
        .child("·")
        .into_any_element()
}

fn error_banner(t: &YkTheme, state: &RankState, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let error = state.error.clone().unwrap_or_default();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(SPACE_3))
        .px(px(SPACE_3))
        .h(px(40.))
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(t.palette.status_error_10)
        .border_1()
        .border_color(t.palette.status_error_30)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_secondary)
                .child(format!("刷新失败，显示的是上次榜单：{error}")),
        )
        .child(retry_button(t, cx))
        .into_any_element()
}

fn error_state(t: &YkTheme, error: &str, cx: &mut Context<AppRoot>) -> AnyElement {
    let body = status::error(t, "榜单加载失败", error).into_any_element();
    div()
        .relative()
        .w_full()
        .child(body)
        .child(
            div()
                .absolute()
                .bottom(px(72.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(retry_button(t, cx)),
        )
        .into_any_element()
}

fn retry_button(t: &YkTheme, cx: &mut Context<AppRoot>) -> AnyElement {
    let p = &t.palette;
    div()
        .id("rank-retry")
        .px(px(SPACE_4))
        .h(px(32.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(p.theme_primary_20)
        .text_size(px(FONT_SIZE_CONTROL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.theme_primary)
        .cursor_pointer()
        .hover(move |style| style.bg(p.theme_primary_30))
        .child("重试")
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.load_rank(cx);
            window.refresh();
        }))
        .into_any_element()
}

fn skeleton_rows(t: &YkTheme) -> impl IntoElement {
    let brush = t.surfaces.skeleton_brush;
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_2))
        .w_full()
        .with_animation(
            "rank-skeleton-pulse",
            Animation::new(std::time::Duration::from_millis(status::SKELETON_PULSE_MS)).repeat(),
            move |el, delta| {
                el.children((0..8).map(move |i| {
                    let phase = (delta + i as f32 * 0.12).fract();
                    let opacity = 0.35 + 0.30 * ((phase * std::f32::consts::PI).sin().abs());
                    div()
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .rounded(px(CARD_RADIUS))
                        .bg(brush)
                        .opacity(opacity)
                        .into_any_element()
                }))
            },
        )
}
