//! 收藏夹页（设计文档 12.5.2）。
//!
//! 结构：左侧固定收藏夹栏（默认夹 / 自建夹 / 合集），右侧当前夹子的视频网格。
//! 视频卡直接复用首页 `card()`：封面、UP 头像、播放量、点开播放都走同一条链路，
//! 不再为收藏夹单独画一套卡片。
//!
//! 数据：
//! - 夹子列表 `GET /x/v3/fav/folder/created/list-all`（复用 `interaction` 的解析）；
//! - 夹内视频 `GET /x/v3/fav/resource/list`（`favorites.rs`，按收藏时间倒序分页）。
//!
//! 未登录时只给登录引导，不发请求——收藏夹接口无 Cookie 一定返回 `-101`。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_infra::api::client::ReqwestClient;
use yk_infra::favorites::{self, FavVideo, FAV_PAGE_SIZE};
use yk_infra::interaction::fetch_fav_folders;
use yk_infra::interaction::FavFolder;
use yk_theme::theme::*;

use crate::card::{card, card_height, CardHost};
use crate::home::card_metrics;
use crate::image::ImageResidency;

/// 左侧收藏夹栏宽度。
const RAIL_W: f32 = 232.0;
/// 左栏一行的行高。
const RAIL_ROW_H: f32 = 40.0;
/// 右栏工具条高度（标题 + 排序胶囊）。
const TOOLBAR_H: f32 = 30.0;

/// 收藏夹内排序（本地排序，不发请求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FavSort {
    #[default]
    Recent,
    Views,
    Newest,
}

impl FavSort {
    pub const ALL: [Self; 3] = [Self::Recent, Self::Views, Self::Newest];

    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "最近收藏",
            Self::Views => "最多播放",
            Self::Newest => "最新发布",
        }
    }
}

/// 收藏夹页需要宿主提供的动作。
///
/// 数据加载仍由组装点实现（它持有 session / cookie / 异步上下文），
/// 视图只通过这组动作触发，不接触 `AppRoot`。
pub trait FavoritesHost: CardHost {
    fn favorites(&self) -> &FavoritesState;
    fn favorites_mut(&mut self) -> &mut FavoritesState;
    fn favorites_session(&self) -> Option<&yk_domain::models::UserSession>;
    fn favorites_cookie(&self) -> String;
    fn favorites_open_login(&mut self, cx: &mut Context<Self>);
    fn favorites_reset_hover(&mut self) {}

    fn favorites_load_more(&mut self, cx: &mut Context<Self>) {
        self.load_more_favorites(cx);
    }

    /// 右侧网格滚动到接近底部时自动补下一页。
    ///
    /// `ScrollHandle` 的 `offset` 向下为负、`max_offset` 为正；统一换成
    /// 「已滚动距离 / 最大可滚动距离」后再比较。`loading_more` 与
    /// `has_more` 两道闸门保证不会重复拉取。
    fn favorites_auto_load_on_scroll(&mut self, cx: &mut Context<Self>) {
        let (has_more, loading_more, loading, scroll) = {
            let state = self.favorites();
            (
                state.has_more,
                state.loading_more,
                state.loading,
                state.content_scroll.clone(),
            )
        };
        if !has_more || loading_more || loading {
            return;
        }
        let offset = (-scroll.offset().y.as_f32()).max(0.0);
        let max = scroll.max_offset().y.as_f32().abs();
        let viewport = scroll.bounds().size.height.as_f32().max(1.0);
        // 距底部半个视口内开始预取。监听在滚动写回 offset 之前触发，
        // 留半个视口能保证大幅滚动 / 惯性滚动也不会错过。
        if max > 0.0 && max - offset <= viewport * 0.5 {
            self.favorites_load_more(cx);
        }
    }
    fn favorites_select_folder(&mut self, folder_id: i64, cx: &mut Context<Self>) {
        self.select_favorites_folder(folder_id, cx);
    }
    fn favorites_set_sort(&mut self, sort: FavSort, cx: &mut Context<Self>) {
        self.set_favorites_sort(sort, cx);
    }
    fn favorites_reload(&mut self, cx: &mut Context<Self>) {
        self.load_favorites(cx);
    }
    fn favorites_retry_page(&mut self, cx: &mut Context<Self>) {
        self.load_favorites_page(false, cx);
    }


    /// 首次进入收藏夹：夹子列表没拉过就拉；拉过但夹内还没加载就补一页。
    fn ensure_favorites(&mut self, cx: &mut Context<Self>) {
        if self.favorites_session().is_none() {
            return;
        }
        if self.favorites_mut().folders_loaded {
            if !self.favorites_mut().loaded && self.favorites_mut().selected.is_some() {
                self.load_favorites_page(false, cx);
            }
        } else {
            self.load_favorites_folders(cx);
        }
    }

    /// 进入 / 刷新收藏夹页：重新拉夹子列表，再拉当前夹子第一页。
    fn load_favorites(&mut self, cx: &mut Context<Self>) {
        if self.favorites_session().is_none() {
            return;
        }
        self.load_favorites_folders(cx);
    }

    fn load_favorites_folders(&mut self, cx: &mut Context<Self>) {
        if self.favorites_mut().folders_loading {
            return;
        }
        if self.favorites_session().is_none() {
            self.favorites_mut().reset();
            cx.notify();
            return;
        }
        let Some(mid) = self.favorites_session().as_ref().map(|s| s.dede_user_id) else {
            return;
        };
        self.favorites_mut().generation = self.favorites_mut().generation.wrapping_add(1);
        let generation = self.favorites_mut().generation;
        self.favorites_mut().folders_loading = true;
        self.favorites_mut().folders_error = None;
        // 旧的一页请求可能还在路上：代际已经变了，回包会被丢弃；这里立刻
        // 把加载标记复位，否则新的夹子页会因为 `loading == true` 永远不再发起。
        self.favorites_mut().loading = false;
        self.favorites_mut().loading_more = false;
        let cookie = self.favorites_cookie();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    // 收藏夹页只关心夹子本身，不需要「某个视频在不在夹里」。
                    fetch_fav_folders(&client, &cookie, mid, 0)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.favorites_mut().generation != generation {
                    return;
                }
                view.favorites_mut().folders_loading = false;
                match result {
                    Ok(folders) => {
                        eprintln!("[favorites] 收藏夹列表：{} 个", folders.len());
                        view.favorites_mut().folders_loaded = true;
                        view.favorites_mut().folders_error = None;
                        let previous = view.favorites_mut().selected;
                        view.favorites_mut().folders = folders;
                        let next = previous
                            .filter(|id| {
                                view.favorites_mut().folders.iter().any(|folder| folder.id == *id)
                            })
                            .or_else(|| view.favorites_mut().folders.first().map(|folder| folder.id));
                        view.favorites_mut().selected = next;
                        // 同一夹子刷新时保留旧网格（数据回来再替换，不闪骨架）；
                        // 换夹子才清空，避免上一夹子的内容留在屏幕上。
                        if previous != next {
                            view.favorites_mut().items.clear();
                            view.favorites_mut().page = 0;
                            view.favorites_mut().has_more = false;
                        }
                        view.favorites_mut().loaded = false;
                        view.favorites_mut().error = None;
                        view.favorites_mut().loading = false;
                        view.favorites_mut().loading_more = false;
                        if next.is_some() {
                            view.load_favorites_page(false, cx);
                        } else {
                            view.favorites_mut().loaded = true;
                        }
                    }
                    Err(error) => {
                        eprintln!("[favorites] 收藏夹列表加载失败：{error}");
                        view.favorites_mut().folders_loaded = true;
                        view.favorites_mut().folders_error = Some(error);
                        view.favorites_mut().loaded = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉当前收藏夹的一页。`append = true` 接在现有列表后面。
    fn load_favorites_page(&mut self, append: bool, cx: &mut Context<Self>) {
        if self.favorites_mut().loading || self.favorites_mut().loading_more {
            return;
        }
        if self.favorites_session().is_none() {
            return;
        }
        let Some(media_id) = self.favorites_mut().selected else {
            self.favorites_mut().items.clear();
            self.favorites_mut().loaded = true;
            self.favorites_mut().loading = false;
            self.favorites_mut().loading_more = false;
            self.favorites_mut().has_more = false;
            cx.notify();
            return;
        };
        let page = if append {
            self.favorites_mut().page.saturating_add(1)
        } else {
            1
        };
        self.favorites_mut().generation = self.favorites_mut().generation.wrapping_add(1);
        let generation = self.favorites_mut().generation;
        if append {
            self.favorites_mut().loading_more = true;
        } else {
            self.favorites_mut().loading = true;
            self.favorites_mut().error = None;
        }
        let cookie = self.favorites_cookie();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new()?;
                    favorites::fetch_fav_videos(&client, &cookie, media_id, page, FAV_PAGE_SIZE)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.favorites_mut().generation != generation
                    || view.favorites_mut().selected != Some(media_id)
                {
                    return;
                }
                view.favorites_mut().loading = false;
                view.favorites_mut().loading_more = false;
                match result {
                    Ok(page_data) => {
                        eprintln!(
                            "[favorites] 第 {page} 页：{} 条（has_more={}）",
                            page_data.items.len(),
                            page_data.has_more
                        );
                        if append {
                            for item in page_data.items {
                                if !view
                                    .favorites()
                                    .items
                                    .iter()
                                    .any(|old| !old.bvid.is_empty() && old.bvid == item.bvid)
                                {
                                    view.favorites_mut().items.push(item);
                                }
                            }
                        } else {
                            view.favorites_mut().items = page_data.items;
                        }
                        view.favorites_mut().page = page;
                        view.favorites_mut().has_more = page_data.has_more;
                        if page_data.total > 0 {
                            view.favorites_mut().total = page_data.total;
                        }
                        view.favorites_mut().loaded = true;
                        view.favorites_mut().error = None;
                    }
                    Err(error) => {
                        eprintln!("[favorites] 加载失败：{error}");
                        view.favorites_mut().error = Some(error);
                        view.favorites_mut().loaded = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_more_favorites(&mut self, cx: &mut Context<Self>) {
        if !self.favorites_mut().has_more || self.favorites_mut().loading_more {
            return;
        }
        self.load_favorites_page(true, cx);
    }

    /// 切换收藏夹：清空旧网格，避免上一夹子的内容留在屏幕上。
    fn select_favorites_folder(&mut self, folder_id: i64, cx: &mut Context<Self>) {
        if self.favorites_mut().selected == Some(folder_id) {
            return;
        }
        // 旧夹子的请求可能还在路上：先换代 + 复位加载标记，否则新夹子会因为
        // `loading == true` 被 `load_favorites_page` 直接挡掉，页面永远停在骨架。
        self.favorites_mut().generation = self.favorites_mut().generation.wrapping_add(1);
        self.favorites_mut().loading = false;
        self.favorites_mut().loading_more = false;
        self.favorites_mut().selected = Some(folder_id);
        self.favorites_mut().items.clear();
        self.favorites_mut().loaded = false;
        self.favorites_mut().error = None;
        self.favorites_mut().page = 0;
        self.favorites_mut().has_more = false;
        self.favorites_mut()
            .content_scroll
            .set_offset(point(px(0.), px(0.)));
        self.favorites_reset_hover();
        self.load_favorites_page(false, cx);
        cx.notify();
    }

    fn set_favorites_sort(&mut self, sort: FavSort, cx: &mut Context<Self>) {
        if self.favorites_mut().sort == sort {
            return;
        }
        self.favorites_mut().sort = sort;
        self.favorites_mut()
            .content_scroll
            .set_offset(point(px(0.), px(0.)));
        self.favorites_reset_hover();
        cx.notify();
    }

}

/// 收藏夹页状态。
#[derive(Debug, Clone)]
pub struct FavoritesState {
    pub folders: Vec<FavFolder>,
    pub folders_loading: bool,
    pub folders_loaded: bool,
    pub folders_error: Option<String>,
    /// 当前选中的收藏夹 id。
    pub selected: Option<i64>,
    pub items: Vec<FavVideo>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    /// 已加载到第几页（从 1 开始；0 = 还没加载）。
    pub page: u32,
    pub has_more: bool,
    pub loading_more: bool,
    /// 夹内总条数（接口 `info.media_count`）。
    pub total: u64,
    /// 请求代际：刷新 / 切夹后只接受最后一次结果。
    pub generation: u64,
    pub sort: FavSort,
    /// 左侧收藏夹目录的滚动位置（与右侧视频列表独立）。
    pub rail_scroll: ScrollHandle,
    /// 右侧视频网格的滚动位置。
    pub content_scroll: ScrollHandle,
}

impl Default for FavoritesState {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            folders_loading: false,
            folders_loaded: false,
            folders_error: None,
            selected: None,
            items: Vec::new(),
            loading: false,
            loaded: false,
            error: None,
            page: 0,
            has_more: false,
            loading_more: false,
            total: 0,
            generation: 0,
            sort: FavSort::Recent,
            rail_scroll: ScrollHandle::new(),
            content_scroll: ScrollHandle::new(),
        }
    }
}

impl FavoritesState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn selected_folder(&self) -> Option<&FavFolder> {
        let selected = self.selected?;
        self.folders.iter().find(|folder| folder.id == selected)
    }

    /// 当前夹子里按本地排序后的视频。
    ///
    /// `attr != 0` 的失效视频不进网格：它们没有 bvid，点开只会进错误态。
    pub fn visible_items(&self) -> Vec<&FavVideo> {
        let mut items = self
            .items
            .iter()
            .filter(|video| video.is_playable())
            .collect::<Vec<_>>();
        match self.sort {
            FavSort::Recent => items.sort_by(|a, b| b.fav_time.cmp(&a.fav_time)),
            FavSort::Views => items.sort_by(|a, b| b.view_count.cmp(&a.view_count)),
            FavSort::Newest => items.sort_by(|a, b| b.pubdate.cmp(&a.pubdate)),
        }
        items
    }
}

// ================= 数据加载 =================



// ================= 渲染 =================

pub fn page<V: FavoritesHost>(
    t: &YkTheme,
    state: &FavoritesState,
    signed_in: bool,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    material: SurfaceMaterial,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    if !signed_in {
        // 收藏页已经脱离外层 content-scroll，未登录引导要自己补回页面内边距。
        return div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .px(px(SPACE_6))
            .pt(px(SPACE_4))
            .pb(px(DOCK_RESERVED))
            .child(login_prompt(t, cx))
            .into_any_element();
    }

    // 收藏夹页没有接首页的可视范围计算，直接把这批卡片的封面 / 头像全部登记驻留；
    // 不登记的话下一帧 `prune_page_images` 会把刚取回来的图 drop 掉。
    {
        let mut residency = active_images.borrow_mut();
        for video in &state.items {
            residency.mark(&video.cover_thumb_url());
            residency.mark(&video.face_thumb_url());
        }
    }

    let viewport = window.viewport_size();
    let viewport_w = viewport.width.as_f32();
    let content_w = (viewport_w - SPACE_6 * 2.0 - RAIL_W - SPACE_5).max(COVER_CELL_WIDTH);
    // 与首页「个性推荐」同一个栅格算法：卡片按可用宽均分、整行铺满，
    // 不再用固定 208 宽把右侧留出一条空带。
    let (cols, card_w) = card_metrics(content_w);
    // 收藏夹页自己管高度：左右两栏各自 `overflow_y_scroll`，不接外层 content-scroll。
    // 两栏都直接拉到窗口下边缘；底部留白放进滚动内容里，最后一行仍能滚到 Dock 上方。
    let page_h = (viewport.height.as_f32() - TOP_BAR_HEIGHT).max(320.0);
    let body_h = (page_h - SPACE_4).max(240.0);

    div()
        .flex()
        .flex_col()
        .h(px(page_h))
        .w_full()
        .flex_shrink_0()
        .px(px(SPACE_6))
        .pt(px(SPACE_4))
        .child(
            div()
                .flex()
                .h(px(body_h))
                .w_full()
                .min_h(px(0.))
                .gap(px(SPACE_5))
                // 左栏：目录自己滚。整块卡片拉到底，滚的是里面的条目。
                .child(folder_rail(t, state, cx))
                // 右栏：工具条固定，只有视频网格滚。
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_4))
                        .child(toolbar(t, state, cx))
                        .child(
                            div()
                                .id("fav-content-scroll")
                                .flex_1()
                                .min_h(px(0.))
                                .w_full()
                                .overflow_y_scroll()
                                .track_scroll(&state.content_scroll)
                                // 滚到接近底部自动补下一页；没有更多时列表底部显示「已经到底了」。
                                .on_scroll_wheel(cx.listener(
                                    |this, _: &ScrollWheelEvent, _window, cx| {
                                        this.favorites_auto_load_on_scroll(cx);
                                    },
                                ))
                                // 底部留白跟着内容滚，视口本身一直铺到窗口下边缘。
                                .child(div().pb(px(DOCK_RESERVED)).child(content(
                                    t,
                                    state,
                                    cols,
                                    card_w,
                                    hovered_card,
                                    leaving_card,
                                    hover_generation,
                                    material,
                                    layout_bounds,
                                    active_images,
                                    cx,
                                ))),
                        ),
                ),
        )
        .into_any_element()
}

fn folder_rail<V: FavoritesHost>(t: &YkTheme, state: &FavoritesState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut rail = div()
        .id("fav-folders-scroll")
        .w(px(RAIL_W))
        .h_full()
        .min_h(px(0.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(SPACE_1))
        .p(px(SPACE_2))
        // 目录拉到窗口下边缘；底部留出 Dock 高度，最后几个收藏夹能滚到 Dock 上方。
        .pb(px(DOCK_RESERVED))
        .overflow_y_scroll()
        .track_scroll(&state.rail_scroll)
        .rounded(px(CARD_RADIUS))
        .bg(s.surface_elevated)
        .border_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .px(px(SPACE_2))
                .py(px(SPACE_2))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_quaternary)
                .child("收藏夹")
                .child(format!("{}", state.folders.len())),
        );

    if state.folders_loading {
        for i in 0..8 {
            rail = rail.child(
                div()
                    .h(px(RAIL_ROW_H))
                    .w_full()
                    .rounded(px(INTERACTIVE_RADIUS))
                    .bg(s.skeleton_brush)
                    .opacity(0.35 + 0.05 * (i as f32 % 4.)),
            );
        }
        return rail.into_any_element();
    }

    if let Some(error) = state.folders_error.as_ref() {
        rail = rail
            .child(
                div()
                    .px(px(SPACE_2))
                    .py(px(SPACE_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(p.status_error)
                    .child("收藏夹加载失败"),
            )
            .child(
                div()
                    .px(px(SPACE_2))
                    .pb(px(SPACE_3))
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_tertiary)
                    .line_clamp(3)
                    .child(error.clone()),
            )
            .child(retry_row(t, cx));
        return rail.into_any_element();
    }

    if state.folders.is_empty() {
        rail = rail.child(
            div()
                .px(px(SPACE_2))
                .py(px(SPACE_3))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("还没有收藏夹"),
        );
        return rail.into_any_element();
    }

    for folder in &state.folders {
        let id = folder.id;
        let active = state.selected == Some(id);
        let fg = if active {
            p.theme_primary
        } else {
            s.text_secondary
        };
        let bg = if active {
            p.theme_primary_10
        } else {
            hsla(0., 0., 0., 0.)
        };
        rail = rail.child(
            div()
                .id(("fav-folder", id as usize))
                .h(px(RAIL_ROW_H))
                .w_full()
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(bg)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(fg)
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2))
                .child(
                    Icon::new(if folder.is_default() {
                        IconName::Bookmark
                    } else {
                        IconName::List
                    })
                    .size(px(14.))
                    .text_color(if active {
                        p.theme_primary
                    } else {
                        s.text_quaternary
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .child(folder.title.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(format!("{}", folder.media_count)),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.favorites_select_folder(id, cx);
                    window.refresh();
                })),
        );
    }
    rail.into_any_element()
}

fn toolbar<V: FavoritesHost>(t: &YkTheme, state: &FavoritesState, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let folder = state.selected_folder();
    let title = folder
        .map(|f| f.title.clone())
        .unwrap_or_else(|| "全部收藏".into());
    let count = folder.map(|f| f.media_count).unwrap_or(state.total);
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .h(px(TOOLBAR_H))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .max_w(px(260.))
                        .truncate()
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .child(format!("{count} 条")),
                ),
        )
        .child(div().flex_1())
        .children(FavSort::ALL.map(|sort| sort_chip(t, sort, state.sort == sort, cx)))
        .into_any_element()
}

fn sort_chip<V: FavoritesHost>(t: &YkTheme, sort: FavSort, active: bool, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .id(("fav-sort", sort as usize))
        .h(px(30.))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .rounded_full()
        .bg(if active { p.theme_primary_20 } else { s.fill_1 })
        .text_size(px(FONT_SIZE_CAPTION))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if active {
            p.theme_primary
        } else {
            s.text_secondary
        })
        .cursor_pointer()
        .hover(move |style| style.bg(if active { p.theme_primary_30 } else { s.fill_2 }))
        .child(sort.label())
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.favorites_set_sort(sort, cx);
        }))
        .into_any_element()
}

fn content<V: FavoritesHost>(
    t: &YkTheme,
    state: &FavoritesState,
    cols: usize,
    card_w: f32,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    material: SurfaceMaterial,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> AnyElement {
    if state.loading && state.items.is_empty() {
        return skeleton_grid(t, cols, card_w);
    }
    if let Some(error) = state.error.as_ref().filter(|_| state.items.is_empty()) {
        return error_block(t, error, cx);
    }
    let videos = state.visible_items();
    if videos.is_empty() {
        return div()
            .h(px(320.))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(SPACE_2))
            .text_color(t.surfaces.text_secondary)
            .child(
                div()
                    .text_size(px(28.))
                    .text_color(t.surfaces.text_quaternary)
                    .child("○"),
            )
            .child(
                div()
                    .text_size(px(FONT_SIZE_BODY))
                    .child("这个收藏夹还没有内容"),
            )
            .child(
                div()
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(t.surfaces.text_tertiary)
                    .child("在播放页点「收藏」就能把它加进来"),
            )
            .into_any_element();
    }

    let mut grid = div().flex().flex_wrap().gap(px(SPACE_4)).w_full();
    for (index, video) in videos.iter().enumerate() {
        let item = video.to_cover_item(index);
        grid = grid.child(card(
            t,
            card_w,
            &item,
            hovered_card == Some(index),
            leaving_card == Some(index),
            hover_generation,
            material,
            layout_bounds.clone(),
            true,
            active_images.clone(),
            cx,
        ));
    }

    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(SPACE_3))
        .w_full()
        .child(grid);
    if let Some(error) = state.error.as_ref().filter(|_| !state.items.is_empty()) {
        col = col.child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(t.palette.status_error)
                .child(format!("加载更多失败：{error}")),
        );
    }
    if state.has_more {
        col = col.child(
            div().w_full().flex().justify_center().child(
                div()
                    .id("fav-load-more")
                    .h(px(36.))
                    .px(px(SPACE_5))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(t.surfaces.fill_2)
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(t.surfaces.text_secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(t.surfaces.fill_3))
                    .child(if state.loading_more {
                        "加载中…"
                    } else {
                        "加载更多"
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.favorites_load_more(cx);
                        window.refresh();
                    })),
            ),
        );
    } else {
        col = col.child(
            div()
                .w_full()
                .py(px(SPACE_3))
                .flex()
                .justify_center()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(t.surfaces.text_quaternary)
                .child(format!(
                    "已经到底了 · 共 {} 条",
                    state.total.max(state.items.len() as u64)
                )),
        );
    }
    col.into_any_element()
}

fn login_prompt<V: FavoritesHost>(t: &YkTheme, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(420.))
        .w_full()
        .child(
            Icon::new(IconName::Bookmark)
                .size(px(30.))
                .text_color(s.text_quaternary),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("登录后查看收藏夹"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("收藏的视频会按收藏夹同步到这里"),
        )
        .child(
            div()
                .id("fav-login")
                .mt(px(SPACE_2))
                .h(px(34.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_on_accent)
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .child("登录 B 站")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.favorites_open_login(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn retry_row<V: FavoritesHost>(t: &YkTheme, cx: &mut Context<V>) -> AnyElement {
    let p = &t.palette;
    div()
        .id("fav-folders-retry")
        .mx(px(SPACE_2))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(p.theme_primary_20)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(p.theme_primary)
        .cursor_pointer()
        .hover(move |style| style.bg(p.theme_primary_30))
        .child("重试")
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.favorites_reload(cx);
            window.refresh();
        }))
        .into_any_element()
}

fn error_block<V: FavoritesHost>(t: &YkTheme, error: &str, cx: &mut Context<V>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .h(px(320.))
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .child(
            div()
                .text_size(px(28.))
                .text_color(p.status_error)
                .child("!"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("收藏夹加载失败"),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .line_clamp(3)
                .child(error.to_string()),
        )
        .child(
            div()
                .id("fav-content-retry")
                .mt(px(SPACE_2))
                .h(px(32.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary_20)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(p.theme_primary)
                .cursor_pointer()
                .hover(move |style| style.bg(p.theme_primary_30))
                .child("重试")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.favorites_retry_page(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn skeleton_grid(t: &YkTheme, cols: usize, card_w: f32) -> AnyElement {
    let brush = t.surfaces.skeleton_brush;
    let rows = 2;
    let card_h = card_height(card_w);
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .with_animation(
            "fav-skeleton-pulse",
            Animation::new(std::time::Duration::from_millis(
                yk_components::status::SKELETON_PULSE_MS,
            ))
            .repeat(),
            move |el, delta| {
                let phase = (delta * std::f32::consts::PI).sin().abs();
                el.opacity(0.35 + 0.35 * phase)
                    .children((0..rows).map(move |_| {
                        div()
                            .flex()
                            .gap(px(SPACE_4))
                            .w_full()
                            .h(px(card_h))
                            .children((0..cols).map(move |_| {
                                div()
                                    .w(px(card_w))
                                    .h(px(card_h))
                                    .rounded(px(CARD_RADIUS))
                                    .bg(brush)
                                    .into_any_element()
                            }))
                            .into_any_element()
                    }))
            },
        )
        .into_any_element()
}
