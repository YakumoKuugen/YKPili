//! YKPili 的 GPUI 外壳（M2）。
//!
//! 对应旧 `MainView.axaml` 的四层结构，但换成 GPUI 的**绘制顺序**表达：
//!
//! ```text
//! Root
//!   ├─ ① 背景渐变        （最先画，铺满）
//!   ├─ ② 主题色光晕      （叠在渐变上）
//!   ├─ ③ 页面内容        （顶栏 → 分类 → 网格）
//!   ├─ ④ 播放浮窗        （独立 View，整场景压在页面上；M3 再接）
//!   ├─ ⑤ 模态遮罩/对话框  （M4 接）
//!   └─ ⑥ Toast           （deferred，永远最上；M4 接）
//! ```
//!
//! GPUI 没有 z-index，「谁在上」只由 child 的先后顺序决定（见设计文档附录 D · D.1 事实 2）。
//! 因此这个文件里 child 的排列顺序**就是**层级契约本身，改动顺序等于改动层级。

use gpui::component::{v_virtual_list, VirtualListScrollHandle};
use gpui::prelude::*;
use gpui::{InteractiveElement, ImageSource, StatefulInteractiveElement, *};
use std::rc::Rc;
use yk_core::api::models::{RecommendVideoItem, VideoOwner, VideoStat};
use yk_core::models::CoverItem;
use yk_core::Feed;

pub mod components;
pub mod http;
pub mod pages;
pub mod player;
pub mod video;
pub mod video_worker;
use yk_ui::theme::*;

/// 应用根视图。
pub struct AppRoot {
    theme: YkTheme,
    feed: Feed,
    active_category: usize,
    /// Some = 正在播放（整场景压在页面上）
    playing: Option<gpui::Entity<player::PlayerView>>,
    /// M4 组件验收页
    gallery: Option<gpui::Entity<components::ComponentGallery>>,
    /// 当前页面；None = 首页（卡片网格）
    page: Option<pages::PageKind>,
    page_state: pages::PageState,
    /// 当前展示的是占位数据还是真实接口数据（用于顶栏角标）
    using_mock: bool,
    /// 内容区滚动句柄：首页虚拟列表用它，页面容器复用它的 base handle
    content_scroll: VirtualListScrollHandle,
    /// 当前是否离线（启动时拉取失败 / 显式 YK_OFFLINE）
    offline: bool,
    /// 离线时展示的缓存时间（Unix 秒）；None = 没有可用缓存
    offline_updated_at: Option<u64>,
    /// 当前 hover 的卡片（CoverItem::index）
    hovered_card: Option<usize>,
    /// 刚离开的卡片：只为它播放 1→0 的收尾动画，避免所有旧卡一起重播
    leaving_card: Option<usize>,
    /// 离线条上的「重试」是否正在进行
    offline_retrying: bool,
    /// 最近一次重试失败的原因（只用于日志/提示）
    offline_error: Option<String>,
    /// hover 进入/离开时 +1：让 with_animation 的 id 变化、动画重新开始
    hover_generation: u64,
    /// 系统「减少动效」轮询是否已启动（macOS/Windows 没有订阅通知）
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    motion_watchdog_started: bool,
}

impl AppRoot {
    /// 用真实接口数据构造；拉取失败时退回占位数据（不阻塞启动）。
    pub fn with_real_data(items: &[RecommendVideoItem]) -> Self {
        let mut app = Self::new();
        app.feed.load(items);
        app.using_mock = false;
        app
    }

    /// 由外部（open_window 的 build 闭包）注入已创建好的播放器。
    /// 这样 PlayerView 只在窗口创建时构造一次，不会因为 render 重入而重复 load_file。
    pub fn with_player(mut self, player: gpui::Entity<player::PlayerView>) -> Self {
        self.playing = Some(player);
        self
    }

    /// 打开播放器。**只在没有实例时创建，已有就复用。**
    ///
    /// 这是附录 F L-1 的修复：`PlayerView` 里唯一的 `InputState` 跟着实例走，
    /// 一旦重建，输入框内容就换成新的空 state（表现为「打字打一半没了」）。
    /// 所以所有入口都走这里，绝不无条件 `cx.new`。
    /// `PlayerView::new` 里另有单实例断言兜底，防止以后又冒出新路径。
    fn ensure_player(&mut self, cx: &mut Context<Self>) {
        if self.playing.is_some() {
            return;
        }
        let theme = self.theme.clone();
        self.playing = Some(cx.new(|_| player::PlayerView::new(theme)));
    }

    /// 标记为离线，并记录缓存时间（None = 没有可用缓存，只能展示占位内容）。
    pub fn with_offline(mut self, updated_at: Option<u64>) -> Self {
        self.offline = true;
        self.offline_updated_at = updated_at;
        self
    }

    /// 离线时点「重试」：后台重新拉一页推荐，成功则清掉离线条并更新缓存。
    fn retry_online(&mut self, cx: &mut Context<Self>) {
        if self.offline_retrying {
            return;
        }
        self.offline_retrying = true;
        self.offline_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|e| e.to_string())?;
                    yk_core::api::client::fetch_recommend(
                        &client,
                        "",
                        30,
                        yk_core::api::client::now_wts(),
                    )
                    .map_err(|e| e.to_string())
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                view.offline_retrying = false;
                match result {
                    Ok(items) => {
                        eprintln!("[offline] 重试成功：{} 条", items.len());
                        view.feed.load(&items);
                        view.using_mock = false;
                        view.offline = false;
                        view.offline_updated_at = None;
                        view.offline_error = None;
                        if let Err(e) = yk_core::feed_cache::FeedCache::new().save(&items) {
                            eprintln!("[cache] 重试后写入失败：{e}");
                        }
                        view.content_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    }
                    Err(e) => {
                        eprintln!("[offline] 重试失败：{e}");
                        view.offline_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn new() -> Self {
        eprintln!("[app] AppRoot::new 被调用");
        Self {
            theme: YkTheme::dark(),
            feed: {
                let mut f = Feed::new();
                f.load(&mock_recommend(24));
                f
            },
            active_category: 0,
            playing: None,
            gallery: None,
            page: None,
            page_state: pages::PageState::Ready,
            using_mock: true,
            content_scroll: VirtualListScrollHandle::new(),
            offline: false,
            offline_updated_at: None,
            hovered_card: None,
            leaving_card: None,
            offline_retrying: false,
            offline_error: None,
            hover_generation: 0,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            motion_watchdog_started: false,
        }
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 系统「减少动效」：macOS / Windows 没有可订阅的通知，
        // 启动时 gpui-base 已经读过一次；运行中改开关只能轮询重读。
        // Linux 由 gpui-base 跟随 portal 信号，不需要这里再轮。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if !self.motion_watchdog_started {
            self.motion_watchdog_started = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    let alive = this.update(cx, |_view, cx| {
                        let before = cx.reduce_motion();
                        gpui::base::apply_system_reduce_motion(cx);
                        let after = cx.reduce_motion();
                        if before != after {
                            eprintln!("[motion] reduce_motion {before} -> {after}");
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        // 验收辅助：设了 YK_VIDEO 且还没进播放器，就直接进
        // 播放器由 open_window 的 build 闭包注入（见 AppRoot::with_player），
        // 这里不再创建 —— 在 render 里 cx.new 会递归触发渲染，导致重复 load_file。

        let t = &self.theme;

        div()
            .relative()
            .size_full()
            // ① 背景渐变
            // GPUI 的 linear_gradient 只吃 2 个色标（[LinearColorStop; 2]），
            // 而设计稿是三段（#DCF1F9 / #EFF2F7 / #F2F2F8）。
            // 中间段与两端差异很小，这里取首尾两段近似；如需精确三段落差，
            // 要在上层再叠一条只覆盖中段的渐变。
            .bg(linear_gradient(
                135.0,
                linear_color_stop(t.bg_gradient_stops()[0].0, 0.0),
                linear_color_stop(t.bg_gradient_stops()[2].0, 1.0),
            ))
            // ② 主题色光晕（径向）
            .child(glow_layer(t))
            // ③ 页面内容
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(top_bar(t))
                    .child(category_bar(t, self.active_category))
                    .children(if self.offline {
                        Some(offline_banner(
                            t,
                            self.offline_updated_at,
                            self.feed.items().len(),
                            self.offline_retrying,
                            self.offline_error.is_some(),
                            cx,
                        ))
                    } else {
                        None
                    })
                    .child(content_area(
                        t,
                        self.feed.items(),
                        self.page,
                        self.page_state,
                        window,
                        &self.content_scroll,
                        cx,
                    )),
            )
            // Dock 是「页面场景内的浮层」，挂在内容之后
            .child(dock(t, self.active_category, cx))
            // ④ 播放浮窗：独立场景，整块压在页面之上。
            //    它必须挂在页面内容**之后**，且不与页面内容平铺成兄弟层。
            .children(self.gallery.clone())
            .children(self.playing.clone())
    }
}

// ================= ② 光晕 =================

fn glow_layer(t: &YkTheme) -> impl IntoElement {
    let glow = t.bg_accent_glow_stops();
    div().absolute().inset_0().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(760.))
            .h(px(520.))
            // GPUI 只有线性渐变，没有 `radial_gradient`。
            // 设计稿是左上角为中心的径向光晕（70% × 55% 椭圆）；
            // 这里用一条垂直渐变从主题色衰减到透明来近似 —— 视觉上最接近，
            // 真正的径向衰减需要自定义 Element 画（列为已知限制）。
            .bg(linear_gradient(
                160.0,
                linear_color_stop(glow[0].0, 0.0),
                linear_color_stop(glow[1].0, 1.0),
            )),
    )
}

// ================= 顶栏（64px） =================

fn top_bar(t: &YkTheme) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .w_full()
        .h(px(TOP_BAR_HEIGHT))
        .px(px(SPACE_6))
        .gap(px(SPACE_4))
        // 顶栏压在内容上：必须用 Strong 变体（浮层压在内容之上，
        // 而 GPUI 没有元素级 backdrop blur，只提高自身不透明度）
        .bg(s.surface_elevated_strong)
        .border_b_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_3))
                .child(logo_mark())
                .child(
                    div()
                        .text_size(px(FONT_SIZE_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("YKPili"),
                ),
        )
        .child(search_placeholder(t))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(icon_button(t, "搜"))
                .child(avatar(t)),
        )
}

fn logo_mark() -> impl IntoElement {
    div()
        .w(px(28.))
        .h(px(28.))
        .rounded(px(RADIUS_MD))
        .bg(palette().bili_blue)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(FONT_SIZE_CONTROL))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(0xFFFFFF))
        .child("B")
}

fn search_placeholder(t: &YkTheme) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_center()
        .w(px(420.))
        .h(px(CONTROL_HEIGHT))
        .px(px(SPACE_3))
        .rounded(px(INTERACTIVE_RADIUS))
        .bg(s.fill_1)
        .border_1()
        .border_color(s.border_subtle)
        .text_size(px(FONT_SIZE_CONTROL))
        .text_color(s.text_tertiary)
        .child("搜索视频、番剧、UP 主…")
}

fn avatar(t: &YkTheme) -> impl IntoElement {
    div()
        .w(px(34.))
        .h(px(34.))
        .rounded(px(34. / 2.))
        .bg(t.surfaces.fill_2)
        .border_1()
        .border_color(t.surfaces.border_popover)
}

fn icon_button(t: &YkTheme, glyph: &str) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .w(px(TOP_BAR_CONTROL_HEIGHT))
        .h(px(TOP_BAR_CONTROL_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(INTERACTIVE_RADIUS))
        .text_size(px(FONT_SIZE_CONTROL))
        .text_color(s.text_secondary)
        .child(glyph.to_string())
}

// ================= 分类条（吸顶，54px） =================

const CATEGORIES: [&str; 5] = ["首页", "热门", "分区", "排行", "直播"];
const CATEGORY_BAR_HEIGHT: f32 = 54.0;

fn category_bar(t: &YkTheme, active: usize) -> impl IntoElement {
    let s = &t.surfaces;
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(SPACE_5))
        .w_full()
        .h(px(CATEGORY_BAR_HEIGHT))
        .px(px(SPACE_6))
        .bg(s.bg_base)
        .border_b_1()
        .border_color(s.border_subtle);

    for (i, name) in CATEGORIES.iter().enumerate() {
        let is_active = i == active;
        row = row.child(
            div()
                .relative()
                .px(px(SPACE_1))
                .py(px(SPACE_1))
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(if is_active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(if is_active { s.text_primary } else { s.text_tertiary })
                .child(*name),
        );
    }
    row
}

// ================= 离线条 =================

/// 顶部离线条：不遮挡内容，只说明「现在是离线 + 缓存时间」，并给一个重试入口。
fn offline_banner(
    t: &YkTheme,
    updated_at: Option<u64>,
    count: usize,
    retrying: bool,
    retry_failed: bool,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let mut text = match updated_at {
        Some(ts) => format!(
            "离线 · 缓存时间：{} · {} 条",
            yk_core::feed_cache::age_text(ts),
            count
        ),
        None => "离线 · 暂无可用缓存，正在展示占位内容".to_string(),
    };
    if retrying {
        text = "正在重新连接…".to_string();
    } else if retry_failed {
        text.push_str(" · 重试失败");
    }

    div()
        .w_full()
        .h(px(36.))
        .px(px(SPACE_6))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .bg(s.surface_elevated_strong)
        .border_b_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(p.status_warning),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_secondary)
                .child(text),
        )
        .child(
            div()
                .id("offline-retry")
                .h(px(24.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded(px(RADIUS_SM))
                .bg(if retrying { s.fill_2 } else { p.theme_primary })
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(if retrying { s.text_tertiary } else { rgb(0xFFFFFF).into() })
                .cursor_pointer()
                .child(if retrying { "重试中…" } else { "重试" })
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.retry_online(cx);
                    window.refresh();
                })),
        )
}

// ================= 内容区 =================

fn content_area(
    t: &YkTheme,
    items: &[CoverItem],
    page: Option<pages::PageKind>,
    state: pages::PageState,
    window: &Window,
    scroll: &VirtualListScrollHandle,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    // Dock 是浮层（absolute），内容区必须为它让出左边距，
    // 否则第一列卡片会被压在 Dock 下面 —— 浮层「在上」不等于内容可以钻到它背后。
    // 让位宽度 = Dock 左边距 12 + 按钮宽 44 + 呼吸 16 = 72。
    const DOCK_GUTTER: f32 = 72.0;

    if let Some(kind) = page {
        // 非首页：交给 pages 模块；外壳（顶栏/分类/Dock）不动。
        // 页面容器不透明铺满，否则底层光晕会从内容区两侧透上来。
        return div()
            .id("content-scroll")
            .flex_1()
            // flex 子项的 min-height 默认是 auto，不压到 0 的话内容会撑开父级，
            // overflow_y_scroll 就不会真正滚动。
            .min_h(px(0.))
            .track_scroll(scroll.base_handle())
            .overflow_y_scroll()
            .bg(t.surfaces.bg_base)
            .child(
                div()
                    .pl(px(72.))
                    .pr(px(SPACE_4))
                    .pt(px(SPACE_4))
                    .pb(px(SPACE_8))
                    .w_full()
                    .child(pages::render(t, kind, state)),
            )
            .into_any_element();
    }

    // ---- 首页：按视口宽把卡片分组为「行」，交给 v_virtual_list ----
    // 列数 = 可用宽度能放下几个 208 + 16 的槽位；至少 1 列。
    // 窗口缩放时列数会变，item_sizes 每帧重算（行数很少，成本可忽略）。
    let available = (window.viewport_size().width.as_f32() - DOCK_GUTTER - SPACE_4)
        .max(COVER_CELL_WIDTH);
    let cols = (((available + SPACE_4) / (COVER_CELL_WIDTH + SPACE_4)).floor() as usize).max(1);
    let rows = items.len().div_ceil(cols);
    // 行高 = 卡片高 + 行间距；行内只画卡片，多出来的 16 就是行距。
    let row_h = COVER_CELL_HEIGHT + SPACE_4;
    let sizes = Rc::new(vec![size(px(available), px(row_h)); rows]);

    let entity = cx.entity();
    let theme = t.clone();
    v_virtual_list(
        entity,
        "home-grid",
        sizes,
        move |this, visible_rows, _window, cx| {
            visible_rows
                .map(|row| {
                    let start = row * cols;
                    let all = this.feed.items();
                    let end = (start + cols).min(all.len());
                    let mut row_el = div()
                        .flex()
                        .gap(px(SPACE_4))
                        .w_full()
                        .h(px(COVER_CELL_HEIGHT));
                    let hovered_idx = this.hovered_card;
                    let leaving_idx = this.leaving_card;
                    let hover_gen = this.hover_generation;
                    for item in &all[start..end] {
                        let hovered = hovered_idx == Some(item.index);
                        let leaving = leaving_idx == Some(item.index);
                        row_el = row_el.child(card(
                            &theme,
                            item,
                            hovered,
                            leaving,
                            hover_gen,
                            cx,
                        ));
                    }
                    row_el
                })
                .collect::<Vec<_>>()
        },
    )
    .track_scroll(scroll)
    .pl(px(DOCK_GUTTER))
    .pr(px(SPACE_4))
    .pt(px(SPACE_4))
    .pb(px(SPACE_8))
    .into_any_element()
}

/// 封面卡片：208 × 166（COVER_CELL_WIDTH / COVER_CELL_HEIGHT）。
///
/// 结构对应第 04 节：封面（16:9）+ 标题（2 行）+ 信息行。
fn card(
    t: &YkTheme,
    item: &CoverItem,
    hovered: bool,
    leaving: bool,
    hover_generation: u64,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let title = item.title.clone();
    let idx = item.index;
    let hover_shadow = t.shadows.shadow_card_hover.clone();
    let hover_border = s.border_popover;
    let hover_bg = s.surface_content_hover;

    let content = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .child(
            // 封面区：占位色 + 时长角标
            div()
                .relative()
                .w_full()
                .h(px(COVER_ITEM_HEIGHT))
                // 占位色垫底：封面未加载完（或加载失败）时显示纯色块，
                // 不会出现「白洞」。
                .bg(parse_placeholder(&item.placeholder_color))
                .child(
                    // 封面图。B 站图床对 Referer 敏感，若这里显示不出来，
                    // 说明需要带 Referer 预下载（见 yk-core 的 http 客户端）。
                    img(ImageSource::from(item.image_url.clone()))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(4.))
                        .px(px(4.))
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0x000000AB))
                        .text_size(px(FONT_SIZE_DATA_OVERLAY))
                        .text_color(rgb(0xFFFFFF))
                        .child(item.duration_text()),
                ),
        )
        .child(
            // 文字区
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .p(px(SPACE_3))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(2)
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(item.meta_line()),
                ),
        );

    // GPUI 没有元素级 transform：放大仍用「内层负 inset + 外层裁剪」近似。
    // 过渡靠 with_animation 插值 inset：进入 0→1，离开 1→0；
    // id 带 generation，每次进/出都是一段新的 150ms 动画。
    // 用 AnyElement 统一两个动画分支与静态分支的类型。
    let content: AnyElement = if hovered {
        content
            .with_animation(
                format!("card-hover-in-{idx}-{hover_generation}"),
                Animation::new(DURATION_FAST).with_easing(ease_standard()),
                |el, delta| el.inset(px(-3.0 * delta)),
            )
            .into_any_element()
    } else if leaving {
        content
            .with_animation(
                format!("card-hover-out-{idx}-{hover_generation}"),
                Animation::new(DURATION_FAST).with_easing(ease_standard()),
                |el, delta| el.inset(px(-3.0 * (1.0 - delta))),
            )
            .into_any_element()
    } else {
        content.into_any_element()
    };

    div()
        .id(("cover", idx))
        .relative()
        .w(px(COVER_CELL_WIDTH))
        .h(px(COVER_CELL_HEIGHT))
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        // 阴影/描边/背景仍即时切换；放大做 150ms 插值。
        .hover(move |style| {
            style
                .bg(hover_bg)
                .shadow(hover_shadow)
                .border_color(hover_border)
        })
        .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
            if *hovered {
                if this.hovered_card != Some(idx) {
                    // 从上一张卡移进来：让上一张播放收尾动画。
                    if let Some(prev) = this.hovered_card {
                        this.leaving_card = Some(prev);
                    }
                    // 重新进入正在收尾的自己：取消收尾，直接反向。
                    if this.leaving_card == Some(idx) {
                        this.leaving_card = None;
                    }
                    this.hovered_card = Some(idx);
                    this.hover_generation = this.hover_generation.wrapping_add(1);
                    cx.notify();
                }
            } else if this.hovered_card == Some(idx) {
                this.hovered_card = None;
                this.leaving_card = Some(idx);
                this.hover_generation = this.hover_generation.wrapping_add(1);
                cx.notify();
            }
        }))
        .child(content)
        // 点击整张卡片进入播放浮窗（M3 的最小入口）。
        // 已存在就复用：重建会换掉 PlayerView 里的 InputState，弹幕输入跟着丢（附录 F L-1）。
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            eprintln!("[M3] 进入播放浮窗: {title}");
            this.ensure_player(cx);
            window.refresh();
            cx.notify();
        }))
}

// ================= Dock（页面场景内浮层） =================

fn dock(t: &YkTheme, active: usize, cx: &mut Context<AppRoot>) -> impl IntoElement {
    const DOCK_ITEMS: [&str; 6] = ["首", "热", "区", "排", "直", "我"];
    let s = &t.surfaces;
    let mut col = div()
        .absolute()
        .top(px(TOP_BAR_HEIGHT + CATEGORY_BAR_HEIGHT + 24.))
        .left(px(SPACE_3))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(SPACE_2))
        .p(px(SPACE_1))
        .rounded(px(PANEL_RADIUS))
        .bg(s.surface_elevated_strong)
        .border_1()
        .border_color(s.border_subtle)
        .shadow(shadows_shadow_2(t));

    const PAGE_OF_DOCK: [Option<pages::PageKind>; 6] = [
        None,
        Some(pages::PageKind::Hot),
        Some(pages::PageKind::Category),
        Some(pages::PageKind::Rank),
        Some(pages::PageKind::Live),
        Some(pages::PageKind::Mine),
    ];
    for (i, label) in DOCK_ITEMS.iter().enumerate() {
        let is_active = i == active;
        let target = PAGE_OF_DOCK[i];
        col = col.child(
            div()
                .id(("dock", i))
                .w(px(44.))
                .h(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(if is_active { s.fill_2 } else { hsla(0.0, 0.0, 0.0, 0.0) })
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(if is_active { s.text_primary } else { s.text_tertiary })
                .child(*label)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.page = target;
                    // 切页回到顶部：滚动位置属于「上一页」，不该带到新页面。
                    // 虚拟列表走 scroll_to_item；页面容器走 base handle。
                    this.content_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    this.content_scroll.base_handle().scroll_to_top_of_item(0);
                    window.refresh();
                    cx.notify();
                })),
        );
    }
    col
}

// ================= 工具 =================

fn shadows_shadow_2(t: &YkTheme) -> Vec<BoxShadow> {
    t.shadows.shadow_2.clone()
}

/// 把 `#RRGGBB` / `#RRGGBBAA` / `#AARRGGBB` 解析成背景色。
///
/// 占位色来自数据层（`CoverItem::placeholder_color`），是字符串。
/// 这里按设计稿的口径：`#AARRGGBB`（与 XAML 一致）。
fn parse_placeholder(hex: &str) -> Rgba {
    let h = hex.trim_start_matches('#');
    match h.len() {
        6 => {
            let v = u32::from_str_radix(h, 16).unwrap_or(0x555555);
            rgb(v)
        }
        8 => {
            let v = u32::from_str_radix(h, 16).unwrap_or(0xFF555555);
            rgba(v)
        }
        _ => rgb(0x555555),
    }
}

/// 占位数据。真实数据在 M1 的 `CoverItem::from_recommend` 里，
/// 外壳阶段先用与设计稿一致的样例值，避免网络依赖挡住视觉验证。
///
/// 注意：这里刻意产出 **接口原始模型**（`RecommendVideoItem`）而不是 `CoverItem`，
/// 让样例数据也走一遍真实的映射路径 —— 否则 `from_recommend` 丢字段的 bug
/// 在 mock 阶段就发现不了。
fn mock_recommend(n: usize) -> Vec<RecommendVideoItem> {
    const TITLES: [&str; 8] = [
        "生肉烹饪师和他的猫：一顿饭的成本到底有多少",
        "一碗甜子的深夜食堂 · 第 12 期",
        "班长captain 的极限运动合集",
        "用 Rust 从零写一个视频播放器",
        "当我把整个 B 站的热门都下载下来",
        "三分钟看懂 GPUI 的渲染管线",
        "这个夏天最值得去的十个地方",
        "为什么你的代码总是越写越乱",
    ];
    const UPS: [&str; 4] = ["生肉烹饪师", "一碗甜子", "班长captain", "技术宅小明"];
    const VIEWS: [u64; 6] = [124_000, 2_600, 3_000_000, 862, 112_000, 15_190];
    const DANMAKU: [u64; 6] = [144, 0, 1_200, 3, 124, 61];
    const DURATION: [u64; 6] = [754, 1_320, 245, 3_725, 96, 480];

    (0..n)
        .map(|i| RecommendVideoItem {
            id: i as i64,
            bvid: format!("BV1xx411c7m{i}"),
            title: TITLES[i % TITLES.len()].to_string(),
            pic: String::new(),
            desc: String::new(),
            duration: DURATION[i % 6],
            owner: Some(VideoOwner {
                name: UPS[i % UPS.len()].to_string(),
                face: String::new(),
            }),
            stat: Some(VideoStat {
                view: VIEWS[i % 6],
                danmaku: DANMAKU[i % 6],
            }),
        })
        .collect()
}
