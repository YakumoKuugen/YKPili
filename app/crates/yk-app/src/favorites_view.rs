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
use yk_core::api::client::ReqwestClient;
use yk_core::favorites::{self, FavVideo, FAV_PAGE_SIZE};
use yk_core::interaction::{fetch_fav_folders, FavFolder};
use yk_ui::theme::*;

use crate::{AppRoot, ImageResidency};

/// 左侧收藏夹栏宽度。
const RAIL_W: f32 = 232.0;
/// 左栏一行的行高。
const RAIL_ROW_H: f32 = 40.0;

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

impl AppRoot {
    /// 首次进入收藏夹：夹子列表没拉过就拉；拉过但夹内还没加载就补一页。
    pub(crate) fn ensure_favorites(&mut self, cx: &mut Context<Self>) {
        if self.session.is_none() {
            return;
        }
        if self.favorites.folders_loaded {
            if !self.favorites.loaded && self.favorites.selected.is_some() {
                self.load_favorites_page(false, cx);
            }
        } else {
            self.load_favorites_folders(cx);
        }
    }

    /// 进入 / 刷新收藏夹页：重新拉夹子列表，再拉当前夹子第一页。
    pub(crate) fn load_favorites(&mut self, cx: &mut Context<Self>) {
        if self.session.is_none() {
            return;
        }
        self.load_favorites_folders(cx);
    }

    fn load_favorites_folders(&mut self, cx: &mut Context<Self>) {
        if self.favorites.folders_loading {
            return;
        }
        if self.session.is_none() {
            self.favorites.reset();
            cx.notify();
            return;
        }
        let Some(mid) = self.session.as_ref().map(|s| s.dede_user_id) else {
            return;
        };
        self.favorites.generation = self.favorites.generation.wrapping_add(1);
        let generation = self.favorites.generation;
        self.favorites.folders_loading = true;
        self.favorites.folders_error = None;
        // 旧的一页请求可能还在路上：代际已经变了，回包会被丢弃；这里立刻
        // 把加载标记复位，否则新的夹子页会因为 `loading == true` 永远不再发起。
        self.favorites.loading = false;
        self.favorites.loading_more = false;
        let cookie = self.cookie();
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
                if view.favorites.generation != generation {
                    return;
                }
                view.favorites.folders_loading = false;
                match result {
                    Ok(folders) => {
                        eprintln!("[favorites] 收藏夹列表：{} 个", folders.len());
                        view.favorites.folders_loaded = true;
                        view.favorites.folders_error = None;
                        let previous = view.favorites.selected;
                        view.favorites.folders = folders;
                        let next = previous
                            .filter(|id| {
                                view.favorites.folders.iter().any(|folder| folder.id == *id)
                            })
                            .or_else(|| view.favorites.folders.first().map(|folder| folder.id));
                        view.favorites.selected = next;
                        // 同一夹子刷新时保留旧网格（数据回来再替换，不闪骨架）；
                        // 换夹子才清空，避免上一夹子的内容留在屏幕上。
                        if previous != next {
                            view.favorites.items.clear();
                            view.favorites.page = 0;
                            view.favorites.has_more = false;
                        }
                        view.favorites.loaded = false;
                        view.favorites.error = None;
                        view.favorites.loading = false;
                        view.favorites.loading_more = false;
                        if next.is_some() {
                            view.load_favorites_page(false, cx);
                        } else {
                            view.favorites.loaded = true;
                        }
                    }
                    Err(error) => {
                        eprintln!("[favorites] 收藏夹列表加载失败：{error}");
                        view.favorites.folders_loaded = true;
                        view.favorites.folders_error = Some(error);
                        view.favorites.loaded = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉当前收藏夹的一页。`append = true` 接在现有列表后面。
    fn load_favorites_page(&mut self, append: bool, cx: &mut Context<Self>) {
        if self.favorites.loading || self.favorites.loading_more {
            return;
        }
        if self.session.is_none() {
            return;
        }
        let Some(media_id) = self.favorites.selected else {
            self.favorites.items.clear();
            self.favorites.loaded = true;
            self.favorites.loading = false;
            self.favorites.loading_more = false;
            self.favorites.has_more = false;
            cx.notify();
            return;
        };
        let page = if append {
            self.favorites.page.saturating_add(1)
        } else {
            1
        };
        self.favorites.generation = self.favorites.generation.wrapping_add(1);
        let generation = self.favorites.generation;
        if append {
            self.favorites.loading_more = true;
        } else {
            self.favorites.loading = true;
            self.favorites.error = None;
        }
        let cookie = self.cookie();
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
                if view.favorites.generation != generation
                    || view.favorites.selected != Some(media_id)
                {
                    return;
                }
                view.favorites.loading = false;
                view.favorites.loading_more = false;
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
                                    .favorites
                                    .items
                                    .iter()
                                    .any(|old| !old.bvid.is_empty() && old.bvid == item.bvid)
                                {
                                    view.favorites.items.push(item);
                                }
                            }
                        } else {
                            view.favorites.items = page_data.items;
                        }
                        view.favorites.page = page;
                        view.favorites.has_more = page_data.has_more;
                        if page_data.total > 0 {
                            view.favorites.total = page_data.total;
                        }
                        view.favorites.loaded = true;
                        view.favorites.error = None;
                    }
                    Err(error) => {
                        eprintln!("[favorites] 加载失败：{error}");
                        view.favorites.error = Some(error);
                        view.favorites.loaded = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn load_more_favorites(&mut self, cx: &mut Context<Self>) {
        if !self.favorites.has_more || self.favorites.loading_more {
            return;
        }
        self.load_favorites_page(true, cx);
    }

    /// 切换收藏夹：清空旧网格，避免上一夹子的内容留在屏幕上。
    pub(crate) fn select_favorites_folder(&mut self, folder_id: i64, cx: &mut Context<Self>) {
        if self.favorites.selected == Some(folder_id) {
            return;
        }
        // 旧夹子的请求可能还在路上：先换代 + 复位加载标记，否则新夹子会因为
        // `loading == true` 被 `load_favorites_page` 直接挡掉，页面永远停在骨架。
        self.favorites.generation = self.favorites.generation.wrapping_add(1);
        self.favorites.loading = false;
        self.favorites.loading_more = false;
        self.favorites.selected = Some(folder_id);
        self.favorites.items.clear();
        self.favorites.loaded = false;
        self.favorites.error = None;
        self.favorites.page = 0;
        self.favorites.has_more = false;
        self.reset_card_hover();
        self.load_favorites_page(false, cx);
        cx.notify();
    }

    pub(crate) fn set_favorites_sort(&mut self, sort: FavSort, cx: &mut Context<Self>) {
        if self.favorites.sort == sort {
            return;
        }
        self.favorites.sort = sort;
        self.reset_card_hover();
        cx.notify();
    }
}

// ================= 渲染 =================

pub(crate) fn page(
    t: &YkTheme,
    state: &FavoritesState,
    signed_in: bool,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    window: &Window,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    if !signed_in {
        return login_prompt(t, cx);
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

    let viewport_w = window.viewport_size().width.as_f32();
    let content_w = (viewport_w - SPACE_6 * 2.0 - RAIL_W - SPACE_5).max(COVER_CELL_WIDTH);
    let cols = (((content_w + SPACE_4) / (COVER_CELL_WIDTH + SPACE_4)).floor() as usize).max(1);

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(header(t, state))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(SPACE_5))
                .w_full()
                .child(folder_rail(t, state, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_4))
                        .child(toolbar(t, state, cx))
                        .child(content(
                            t,
                            state,
                            cols,
                            hovered_card,
                            leaving_card,
                            hover_generation,
                            layout_bounds,
                            active_images,
                            cx,
                        )),
                ),
        )
        .into_any_element()
}

fn header(t: &YkTheme, state: &FavoritesState) -> AnyElement {
    let s = &t.surfaces;
    let mut title = div().flex().items_end().gap(px(SPACE_2)).child(
        div()
            .text_size(px(FONT_SIZE_DISPLAY))
            .line_height(px(LINE_HEIGHT_HEADING))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(s.text_primary)
            .child("我的收藏"),
    );
    if !state.folders.is_empty() {
        title = title.child(
            div()
                .pb(px(1.))
                .text_size(px(FONT_SIZE_HEADING))
                .font_weight(FontWeight::MEDIUM)
                .text_color(s.text_quaternary)
                .child(format!("({})", state.folders.len())),
        );
    }
    div()
        .flex()
        .items_end()
        .justify_between()
        .w_full()
        .child(title)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_1))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(
                    Icon::new(IconName::Cloud)
                        .size(px(14.))
                        .text_color(s.text_quaternary),
                )
                .child("已同步 B 站"),
        )
        .into_any_element()
}

fn folder_rail(t: &YkTheme, state: &FavoritesState, cx: &mut Context<AppRoot>) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut rail = div()
        .w(px(RAIL_W))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(SPACE_1))
        .p(px(SPACE_2))
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
                    this.select_favorites_folder(id, cx);
                    window.refresh();
                })),
        );
    }
    rail.into_any_element()
}

fn toolbar(t: &YkTheme, state: &FavoritesState, cx: &mut Context<AppRoot>) -> AnyElement {
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

fn sort_chip(t: &YkTheme, sort: FavSort, active: bool, cx: &mut Context<AppRoot>) -> AnyElement {
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
            this.set_favorites_sort(sort, cx);
        }))
        .into_any_element()
}

fn content(
    t: &YkTheme,
    state: &FavoritesState,
    cols: usize,
    hovered_card: Option<usize>,
    leaving_card: Option<usize>,
    hover_generation: u64,
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    if state.loading && state.items.is_empty() {
        return skeleton_grid(t, cols);
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
        grid = grid.child(crate::card(
            t,
            &item,
            hovered_card == Some(index),
            leaving_card == Some(index),
            hover_generation,
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
                        this.load_more_favorites(cx);
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

fn login_prompt(t: &YkTheme, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    this.ensure_login(cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn retry_row(t: &YkTheme, cx: &mut Context<AppRoot>) -> AnyElement {
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
            this.load_favorites(cx);
            window.refresh();
        }))
        .into_any_element()
}

fn error_block(t: &YkTheme, error: &str, cx: &mut Context<AppRoot>) -> AnyElement {
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
                    this.load_favorites_page(false, cx);
                    window.refresh();
                })),
        )
        .into_any_element()
}

fn skeleton_grid(t: &YkTheme, cols: usize) -> AnyElement {
    let brush = t.surfaces.skeleton_brush;
    let rows = 2;
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .with_animation(
            "fav-skeleton-pulse",
            Animation::new(std::time::Duration::from_millis(
                yk_ui::status::SKELETON_PULSE_MS,
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
                            .h(px(COVER_CELL_HEIGHT))
                            .children((0..cols).map(move |_| {
                                div()
                                    .w(px(COVER_CELL_WIDTH))
                                    .h(px(COVER_CELL_HEIGHT))
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
