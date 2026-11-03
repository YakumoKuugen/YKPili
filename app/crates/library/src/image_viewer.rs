//! 图片查看器：点击图片放大查看，支持滚轮缩放、拖动平移、多图左右切换。
//!
//! 做成独立 `Entity<ImageViewer>`：它持有自己的焦点与交互状态，宿主只要
//! 在元素树末尾挂一句 `.children(self.image_viewer.clone())`，再用
//! [`ImageViewer::open`] 打开即可。这样 AppRoot / 播放器 / 详情页可以各挂一个，
//! 互不干扰，事件也不会落到宿主的 `Context` 上。
//!
//! 交互约定：
//! - 滚轮：以**指针位置**为中心缩放（1×–6×），缩到 1× 自动回中；
//! - 拖动：放大后按住左键平移；1× 时拖动不生效（避免误移）；
//! - ←/→ 或 A/D：上一张 / 下一张，切换后回到 1× 并居中；
//! - 右下角「复制」：把当前图片写入系统剪贴板；
//! - 右下角「还原」：只把缩放与位移还原成初始值，不关闭查看器；
//! - Esc / 右键 / 点击图片外的背景：关闭。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::TestSupportExt as _;
use gpui::prelude::*;
use gpui::{
    div, img, point, px, AnyElement, App, Context, FocusHandle, ImageSource, KeyDownEvent,
    MouseButton, ObjectFit, Pixels, Point, Render, ScrollWheelEvent, Window,
};

/// 缩放上下限。`ZOOM_FIT` = 适配窗口的初始尺寸（图片完整可见）。
///
/// 下限放到 0.25：以前夹在 1.0，等于**只能放大不能缩小**，
/// 长图 / 竖图想「再小一点看全」就做不到。
pub const ZOOM_MIN: f32 = 0.25;
pub const ZOOM_MAX: f32 = 6.0;
/// 适配窗口的那一档，也就是「图片完整显示」的默认倍数。
pub const ZOOM_FIT: f32 = 1.0;
/// 滚轮一格（约 40px）对应的缩放倍率变化。
const ZOOM_STEP: f32 = 0.24;
/// 复制结果在按钮上的停留时长。
const COPY_FEEDBACK_MS: u64 = 1800;

/// 纯状态：缩放 / 位移 / 当前第几张。
///
/// 与渲染解耦，方便单测边界（缩放夹取、锚点保持、切图复位）。
#[derive(Debug, Clone, Default)]
pub struct ViewerTransform {
    pub urls: Vec<String>,
    pub index: usize,
    pub zoom: f32,
    pub offset: Point<Pixels>,
}

impl ViewerTransform {
    /// 是否处于「已缩放 / 已拖动」状态（决定右下角还原按钮是否出现）。
    pub fn is_transformed(&self) -> bool {
        (self.zoom - ZOOM_FIT).abs() > 0.001
            || self.offset.x.as_f32().abs() > 0.5
            || self.offset.y.as_f32().abs() > 0.5
    }

    /// 回到初始大小与位置。
    pub fn reset(&mut self) {
        self.zoom = ZOOM_FIT;
        self.offset = Point::default();
    }

    /// 打开一组图片；`index` 越界时夹到合法范围。
    pub fn open(&mut self, urls: Vec<String>, index: usize) {
        if urls.is_empty() {
            return;
        }
        self.urls = urls;
        self.index = index.min(self.urls.len() - 1);
        self.zoom = ZOOM_FIT;
        self.offset = Point::default();
    }

    /// 以 `anchor`（相对屏幕中心）为中心缩放。
    ///
    /// 递推保证锚点下的内容不动：`offset' = anchor - (anchor - offset) * ratio`。
    pub fn zoom_at(&mut self, delta: f32, anchor: Point<Pixels>) {
        let before = self.zoom;
        let next = (self.zoom + delta).clamp(ZOOM_MIN, ZOOM_MAX);
        if (next - before).abs() < f32::EPSILON {
            return;
        }
        let ratio = next / before;
        let ax = anchor.x.as_f32();
        let ay = anchor.y.as_f32();
        self.offset = point(
            px(ax - (ax - self.offset.x.as_f32()) * ratio),
            px(ay - (ay - self.offset.y.as_f32()) * ratio),
        );
        self.zoom = next;
        if self.zoom <= ZOOM_FIT + 0.001 {
            // 不大于适配倍数时没有可平移空间：直接回中，避免留下半像素偏移。
            self.offset = Point::default();
        }
    }

    /// 上一张 / 下一张（首尾循环）；返回是否真的换了。
    pub fn step_image(&mut self, next: bool) -> bool {
        if self.urls.len() < 2 {
            return false;
        }
        self.index = if next {
            (self.index + 1) % self.urls.len()
        } else {
            (self.index + self.urls.len() - 1) % self.urls.len()
        };
        self.zoom = ZOOM_FIT;
        self.offset = Point::default();
        true
    }
}

/// 图片查看器视图（挂在宿主元素树末尾）。
pub struct ImageViewer {
    open: bool,
    transform: ViewerTransform,
    dragging: bool,
    drag_from: Option<Point<Pixels>>,
    focus_handle: FocusHandle,
    /// 当前图片正在写剪贴板。
    copying: bool,
    /// 复制结果文案（「已复制」/「复制失败」），短暂显示后清空。
    copy_feedback: Option<(String, bool)>,
    /// 防止旧的复制结果 / 清理定时器覆盖新状态。
    copy_generation: u64,
}

impl ImageViewer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            open: false,
            transform: ViewerTransform::default(),
            dragging: false,
            drag_from: None,
            focus_handle: cx.focus_handle(),
            copying: false,
            copy_feedback: None,
            copy_generation: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 打开一组图片并聚焦（键盘切换 / Esc 需要焦点）。
    pub fn open(
        &mut self,
        urls: Vec<String>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if urls.is_empty() {
            return;
        }
        self.transform.open(urls, index);
        self.open = true;
        self.dragging = false;
        self.drag_from = None;
        self.copying = false;
        self.copy_feedback = None;
        self.copy_generation = self.copy_generation.wrapping_add(1);
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// 打开但不抢焦点（headless 出图 / 没有窗口句柄的场合）。
    pub fn open_without_focus(&mut self, urls: Vec<String>, index: usize, cx: &mut Context<Self>) {
        if urls.is_empty() {
            return;
        }
        self.transform.open(urls, index);
        self.open = true;
        self.dragging = false;
        self.drag_from = None;
        self.copying = false;
        self.copy_feedback = None;
        self.copy_generation = self.copy_generation.wrapping_add(1);
        cx.notify();
    }

    /// 复制当前这张图：优先取原图地址，读完字节后写入系统图片剪贴板。
    fn copy_current_image(&mut self, cx: &mut Context<Self>) {
        if self.copying {
            return;
        }
        let Some(url) = self.transform.urls.get(self.transform.index).cloned() else {
            return;
        };
        let url = original_image_url(&url);
        if url.is_empty() {
            return;
        }
        self.copying = true;
        self.copy_feedback = None;
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let generation = self.copy_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { yk_infra::http::read_image_bytes(&url) })
                .await;
            let alive = this
                .update(cx, |view, cx| {
                    if view.copy_generation != generation {
                        return false;
                    }
                    view.copying = false;
                    match result {
                        Ok((bytes, format)) => {
                            yk_infra::http::copy_image_to_clipboard(cx, bytes, format);
                            view.copy_feedback = Some(("已复制".into(), false));
                        }
                        Err(error) => {
                            eprintln!("[image-viewer] 复制图片失败：{error}");
                            view.copy_feedback = Some(("复制失败".into(), true));
                        }
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !alive {
                return;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(COPY_FEEDBACK_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.copy_generation == generation {
                    view.copy_feedback = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.dragging = false;
        self.drag_from = None;
        self.copying = false;
        self.copy_feedback = None;
        self.copy_generation = self.copy_generation.wrapping_add(1);
        self.transform.reset();
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.transform.reset();
        self.dragging = false;
        self.drag_from = None;
        cx.notify();
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let Some(src) = self.transform.urls.get(self.transform.index).cloned() else {
            return div().into_any_element();
        };

        let theme = yk_theme::theme::YkTheme::dark();
        let surfaces = theme.surfaces.clone();
        let palette = theme.palette;
        let viewport = window.viewport_size();
        let (vw, vh) = (viewport.width.as_f32(), viewport.height.as_f32());
        // 初始展示区：屏幕的 86%，图片按 Contain 缩放。
        let zoom = self.transform.zoom;
        let copy_label = if self.copying {
            "复制中…".to_string()
        } else if let Some((text, _)) = self.copy_feedback.as_ref() {
            text.clone()
        } else {
            "复制".to_string()
        };
        let copy_error = self
            .copy_feedback
            .as_ref()
            .is_some_and(|(_, is_error)| *is_error);
        let w = (vw * 0.86 * zoom).max(1.0);
        let h = (vh * 0.86 * zoom).max(1.0);
        let offset = self.transform.offset;
        let transformed = self.transform.is_transformed();
        let count = self.transform.urls.len();
        let index = self.transform.index;

        let image_box = div()
            .id("image-viewer-image")
            .absolute()
            .left(px((vw - w) / 2.0 + offset.x.as_f32()))
            .top(px((vh - h) / 2.0 + offset.y.as_f32()))
            .w(px(w))
            .h(px(h))
            .child(
                img(ImageSource::from(src))
                    .w(px(w))
                    .h(px(h))
                    .object_fit(ObjectFit::Contain),
            )
            // 点在图上不关闭；放大后按住拖动平移。
            .on_click(|_, _, cx: &mut App| cx.stop_propagation())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _window, cx| {
                    if this.transform.is_transformed() {
                        this.dragging = true;
                        this.drag_from = Some(event.position);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _window, cx| {
                if !this.dragging {
                    return;
                }
                // **松手判定不靠 MouseUp 事件**：拖动时图片在指针下移动，
                // up 事件经常落到别的元素上（甚至整个窗口外），处理器收不到，
                // `dragging` 就一直为 true —— 表现就是「松开还跟着鼠标走」。
                // 每次移动都看「此刻左键还按着没」，没按就当场清状态。
                if event.pressed_button != Some(MouseButton::Left) {
                    this.dragging = false;
                    this.drag_from = None;
                    return;
                }
                if let Some(from) = this.drag_from {
                    let dx = event.position.x.as_f32() - from.x.as_f32();
                    let dy = event.position.y.as_f32() - from.y.as_f32();
                    this.transform.offset = point(
                        px(this.transform.offset.x.as_f32() + dx),
                        px(this.transform.offset.y.as_f32() + dy),
                    );
                    this.drag_from = Some(event.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _window, cx| {
                    if this.dragging {
                        this.dragging = false;
                        this.drag_from = None;
                        cx.notify();
                    }
                }),
            );

        let mut backdrop = div()
            .id("image-viewer")
            .absolute()
            .inset_0()
            // 深色底：0.9 不透明，压住下面的界面又不至于纯黑。
            .bg(gpui::rgba(0x000000E6))
            // 查看器是模态层：不能再把点击 / 命中漏给下面的详情遮罩，
            // 否则「点图片外面」会同时关掉查看器和详情两个浮层。
            .occlude()
            .track_focus(&self.focus_handle)
            // 点击背景只关查看器（图片自己 stop_propagation）。
            .on_click(cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                this.close(cx);
                cx.stop_propagation();
            }))
            // 滚轮缩放，锚点取指针相对屏幕中心的偏移。
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let delta_y = event.delta.pixel_delta(window.line_height()).y.as_f32();
                if delta_y.abs() < 0.01 {
                    return;
                }
                let viewport = window.viewport_size();
                let anchor = point(
                    px(event.position.x.as_f32() - viewport.width.as_f32() / 2.0),
                    px(event.position.y.as_f32() - viewport.height.as_f32() / 2.0),
                );
                this.transform.zoom_at(delta_y.signum() * ZOOM_STEP, anchor);
                cx.stop_propagation();
                cx.notify();
            }))
            // 松手统一在遮罩上收：指针滑出图片框后，图片自己的 on_mouse_up
            // 收不到事件，`dragging` 会一直是 true，表现为「松开还跟着鼠标走」。
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _window, cx| {
                    if this.dragging {
                        this.dragging = false;
                        this.drag_from = None;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &gpui::MouseDownEvent, _window, cx| {
                    this.close(cx);
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // 处理过的键要吞掉：否则方向键会滚到后面的详情页，
                // Esc 会顺手把详情页也关掉。
                match event.keystroke.key.as_str() {
                    "left" | "a" => {
                        this.transform.step_image(false);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "right" | "d" => {
                        this.transform.step_image(true);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "escape" => {
                        cx.stop_propagation();
                        this.close(cx);
                    }
                    // 0 或 Home：还原（和右下角按钮同一行为）。
                    "0" | "home" => {
                        this.transform.reset();
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .child(image_box);

        // 顶部：第几张 / 共几张。
        if count > 1 {
            backdrop = backdrop.child(
                div()
                    .absolute()
                    .top(px(18.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px(px(10.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(gpui::rgba(0x00000099))
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0xFFFFFF))
                            .child(format!("{} / {}", index + 1, count)),
                    ),
            );
        }

        // 右下角：还原按钮（只在缩放 / 拖动过之后出现）+ 缩放倍率。
        backdrop = backdrop.child(
            div()
                .absolute()
                .right(px(18.))
                .bottom(px(18.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id("image-viewer-copy")
                        .test_support()
                        .px(px(12.))
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(8.))
                        .bg(surfaces.surface_elevated_solid)
                        .text_size(px(12.))
                        .text_color(if copy_error {
                            palette.status_error
                        } else {
                            surfaces.text_secondary
                        })
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.88))
                        .when(self.copying, |el| el.opacity(0.65))
                        .child(Icon::new(IconName::Copy).size(px(14.)))
                        .child(copy_label)
                        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            this.copy_current_image(cx);
                        })),
                )
                .when(transformed, |el| {
                    el.child(
                        div()
                            .id("image-viewer-reset")
                            .px(px(12.))
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(8.))
                            .bg(palette.theme_primary)
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0xFFFFFF))
                            .cursor_pointer()
                            .hover(|style| style.opacity(0.88))
                            .child("还原")
                            .on_click(cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                                cx.stop_propagation();
                                this.reset(cx);
                            })),
                    )
                })
                .child(
                    div()
                        .px(px(10.))
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .bg(surfaces.surface_elevated_solid)
                        .text_size(px(12.))
                        .text_color(surfaces.text_secondary)
                        .child(format!("{:.0}%", zoom * 100.0)),
                ),
        );

        backdrop.into_any_element()
    }
}

/// 查看器里的地址通常是 `...jpg@1280w.webp` 的显示用缩略图；复制取原图。
fn original_image_url(url: &str) -> String {
    let url = url.trim();
    if let Some((base, suffix)) = url.split_once('@') {
        if base.contains("hdslb.com") && suffix.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            return base.to_string();
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::{original_image_url, ViewerTransform, ZOOM_FIT, ZOOM_MAX, ZOOM_MIN};
    use gpui::{point, px};

    #[test]
    fn copy_url_strips_bilibili_resize_suffix() {
        assert_eq!(
            original_image_url("https://i0.hdslb.com/bfs/a.jpg@1280w.webp"),
            "https://i0.hdslb.com/bfs/a.jpg"
        );
        assert_eq!(
            original_image_url("https://i0.hdslb.com/bfs/a.jpg"),
            "https://i0.hdslb.com/bfs/a.jpg"
        );
        assert_eq!(
            original_image_url("https://example.com/a@2x.png"),
            "https://example.com/a@2x.png"
        );
    }

    #[test]
    fn zoom_is_clamped_and_reset_clears_offset() {
        let mut state = ViewerTransform::default();
        state.open(vec!["a".into(), "b".into()], 0);

        for _ in 0..100 {
            state.zoom_at(0.5, point(px(0.), px(0.)));
        }
        assert!((state.zoom - ZOOM_MAX).abs() < 1e-4, "zoom={}", state.zoom);

        state.offset = point(px(30.), px(-20.));
        for _ in 0..100 {
            state.zoom_at(-0.5, point(px(0.), px(0.)));
        }
        // 允许缩到适配倍数以下，夹在下限；缩到 ≤ 适配倍数时位移清空。
        assert!((state.zoom - ZOOM_MIN).abs() < 1e-4, "zoom={}", state.zoom);
        assert_eq!(state.offset, point(px(0.), px(0.)), "缩到适配倍数以下应回中");

        // 再放大回适配档：位移仍然为 0。
        for _ in 0..20 {
            state.zoom_at(0.5, point(px(0.), px(0.)));
        }
        assert!(state.zoom > ZOOM_FIT);

        state.zoom_at(0.5, point(px(10.), px(10.)));
        state.reset();
        assert!(!state.is_transformed());
    }

    #[test]
    fn step_image_wraps_and_resets_transform() {
        let mut state = ViewerTransform::default();
        state.open(vec!["a".into(), "b".into(), "c".into()], 0);
        state.zoom = 2.5;
        state.offset = point(px(8.), px(8.));

        assert!(state.step_image(true));
        assert_eq!(state.index, 1);
        assert!(!state.is_transformed(), "切图后回到初始大小与位置");

        assert!(state.step_image(true));
        assert_eq!(state.index, 2);
        assert!(state.step_image(true));
        assert_eq!(state.index, 0, "末尾往后循环回第一张");
        assert!(state.step_image(false));
        assert_eq!(state.index, 2, "第一张往前循环到最后一张");

        let mut single = ViewerTransform::default();
        single.open(vec!["a".into()], 0);
        assert!(!single.step_image(true), "只有一张时切换无效");
        assert_eq!(single.index, 0);
    }

    #[test]
    fn open_clamps_index_and_zoom_at_keeps_anchor() {
        let mut state = ViewerTransform::default();
        state.open(vec!["a".into(), "b".into()], 9);
        assert_eq!(state.index, 1, "越界 index 夹到合法范围");

        state.zoom_at(0.5, point(px(100.), px(0.)));
        let ratio = state.zoom / ZOOM_FIT;
        let anchor_after = state.offset.x.as_f32() + 100.0 * ratio;
        assert!(
            (anchor_after - 100.0).abs() < 0.5,
            "锚点漂移：{anchor_after}"
        );
    }
}
