//! 动态详情浮层：图文 / 文字动态的展开界面。
//!
//! 交互与首页封面卡的播放器一致：
//! - 从被点击的卡片矩形**连续展开**到浮层目标框（280ms，同一缓动）；
//! - 点浮层外部（遮罩）/ 右上角 × / Esc 关闭，260ms 缩回原卡片位置；
//! - 展开期间只画海报，到终点才挂完整内容，避免控件被塞进小盒子重排。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use std::time::Duration;
use yk_core::api::client::{fetch_dynamic_comments, ReqwestClient};
use yk_core::dynamic::{
    asset_url, fitted_width, DynamicComment, DynamicImage, DynamicItem, DynamicKind,
};
use yk_core::format;
use yk_ui::theme::*;

use crate::placeholder_gradient;

/// 与播放器保持同一节奏。
const DETAIL_ENTRANCE_MS: u64 = 280;
const DETAIL_EXIT_MS: u64 = 260;

/// 点击卡片时冻结的几何。`bounds` 是整张卡，`cover_bounds` 是视频缩略图。
#[derive(Clone, Copy, Debug)]
pub struct CardOrigin {
    pub bounds: Bounds<Pixels>,
    pub cover_bounds: Option<Bounds<Pixels>>,
    pub root_size: Size<Pixels>,
}

#[derive(Clone)]
enum CommentState {
    None,
    Loading,
    Ready(Vec<DynamicComment>),
    Error(String),
}

/// 详情浮层视图。
pub struct DynamicDetailView {
    theme: YkTheme,
    item: DynamicItem,
    origin: Option<CardOrigin>,
    cookie: String,
    comments: CommentState,
    comments_generation: u64,
    image_index: usize,
    left_scroll: ScrollHandle,
    right_scroll: ScrollHandle,
    entrance_generation: u64,
    closing: bool,
    closing_generation: u64,
    close_handler: Option<Box<dyn Fn(&mut App, bool)>>,
    focus_handle: FocusHandle,
    needs_focus: bool,
}

static ENTRANCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static EXIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl DynamicDetailView {
    pub fn new(
        theme: YkTheme,
        item: DynamicItem,
        origin: Option<CardOrigin>,
        cookie: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            theme,
            item,
            origin,
            cookie,
            comments: CommentState::None,
            comments_generation: 0,
            image_index: 0,
            left_scroll: ScrollHandle::new(),
            right_scroll: ScrollHandle::new(),
            entrance_generation: ENTRANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            closing: false,
            closing_generation: 0,
            close_handler: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            needs_focus: true,
        };
        view.spawn_comments(cx);
        view
    }

    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App, bool) + 'static) {
        self.close_handler = Some(Box::new(handler));
    }

    fn spawn_comments(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.item.comment_target.clone() else {
            self.comments = CommentState::Ready(Vec::new());
            return;
        };
        self.comments_generation = self.comments_generation.wrapping_add(1);
        let generation = self.comments_generation;
        let cookie = self.cookie.clone();
        self.comments = CommentState::Loading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new().map_err(|e| e.to_string())?;
                    fetch_dynamic_comments(&client, &cookie, &target)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.comments_generation != generation {
                    return;
                }
                view.comments = match result {
                    Ok(list) => CommentState::Ready(list),
                    Err(e) => {
                        eprintln!("[dynamic] 评论加载失败：{e}");
                        CommentState::Error(e)
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn retry_comments(&mut self, cx: &mut Context<Self>) {
        if matches!(self.comments, CommentState::Loading) {
            return;
        }
        self.spawn_comments(cx);
    }

    pub(crate) fn begin_close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if let Some(handler) = self.close_handler.as_ref() {
            handler(cx, false);
        }
        self.closing = true;
        self.closing_generation = EXIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.closing_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DETAIL_EXIT_MS + 30))
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.closing || view.closing_generation != generation {
                    return;
                }
                view.closing = false;
                if let Some(handler) = view.close_handler.as_ref() {
                    handler(cx, true);
                }
            });
        })
        .detach();
    }

    fn display_images(&self) -> &[DynamicImage] {
        if !self.item.images.is_empty() {
            return &self.item.images;
        }
        if let Some(origin) = self.item.origin.as_ref() {
            return &origin.images;
        }
        &[]
    }

    fn display_cover(&self) -> String {
        if let Some(video) = self.item.video.as_ref() {
            return video.cover.clone();
        }
        if let Some(link) = self.item.link.as_ref() {
            return link.cover.clone();
        }
        if let Some(origin) = self.item.origin.as_ref() {
            if let Some(video) = origin.video.as_ref() {
                return video.cover.clone();
            }
            if let Some(link) = origin.link.as_ref() {
                return link.cover.clone();
            }
        }
        String::new()
    }

    fn has_media(&self) -> bool {
        !self.display_images().is_empty() || !self.display_cover().is_empty()
    }

    fn content(
        &mut self,
        t: &YkTheme,
        target: Bounds<Pixels>,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let item = self.item.clone();
        let has_media = self.has_media();
        let images = self.display_images().to_vec();
        let media_w = target.size.width.as_f32() * 0.56;
        let cover = self.display_cover();
        let cover_url = if cover.is_empty() {
            String::new()
        } else {
            asset_url(&cover, fitted_width(media_w, scale_factor, 0))
        };
        let face_url = item.author_face_url(40.0, scale_factor);

        let header = div()
            .flex()
            .items_center()
            .gap(px(SPACE_3))
            .h(px(72.))
            .px(px(SPACE_6))
            .pr(px(64.))
            .w_full()
            .flex_shrink_0()
            .child(
                div()
                    .relative()
                    .w(px(40.))
                    .h(px(40.))
                    .flex_shrink_0()
                    .rounded_full()
                    .overflow_hidden()
                    .bg(linear_gradient(
                        135.,
                        linear_color_stop(t.palette.bili_blue, 0.),
                        linear_color_stop(t.palette.bili_pink, 1.),
                    ))
                    .when(!face_url.is_empty(), |el| {
                        el.child(
                            img(ImageSource::from(face_url.clone()))
                                .absolute()
                                .inset_0()
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(s.text_primary)
                            .truncate()
                            .child(item.author_name().to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_tertiary)
                            .truncate()
                            .child(item.meta_line()),
                    ),
            )
            .child(
                div()
                    .h(px(22.))
                    .px(px(SPACE_2))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(t.palette.theme_primary_20)
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(t.palette.theme_primary)
                    .child(item.kind.label()),
            );

        let body = if has_media {
            let media = self.media_column(t, &images, &cover_url, media_w, scale_factor, cx);
            let right = self.article_column(t, cx);
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .child(
                    div()
                        .w(relative(0.56))
                        .h_full()
                        .flex_shrink_0()
                        .child(media),
                )
                .child(div().w(px(1.)).h_full().bg(s.border_subtle))
                .child(div().flex_1().h_full().min_w(px(0.)).child(right))
                .into_any_element()
        } else {
            let left = self.text_column(t, false);
            let right = self.comments_column(t, true, cx);
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .child(div().w(relative(0.56)).h_full().flex_shrink_0().child(left))
                .child(div().w(px(1.)).h_full().bg(s.border_subtle))
                .child(div().flex_1().h_full().min_w(px(0.)).child(right))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn media_column(
        &mut self,
        t: &YkTheme,
        images: &[DynamicImage],
        cover_url: &str,
        media_w: f32,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let len = images.len();
        let current = if len > 0 {
            self.image_index.min(len - 1)
        } else {
            0
        };
        let mut column = div()
            .relative()
            .h_full()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x101216));

        if len > 0 {
            column = column.child(
                img(ImageSource::from(
                    images[current].fit_url(media_w, scale_factor),
                ))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Contain),
            );
            if len > 1 {
                let prev_len = len;
                column = column.child(media_arrow(
                    "‹",
                    true,
                    move |view: &mut Self, cx| {
                        let idx = view.image_index.min(prev_len - 1);
                        view.image_index = (idx + prev_len - 1) % prev_len;
                        cx.notify();
                    },
                    cx,
                ));
                column = column.child(media_arrow(
                    "›",
                    false,
                    move |view: &mut Self, cx| {
                        let idx = view.image_index.min(prev_len - 1);
                        view.image_index = (idx + 1) % prev_len;
                        cx.notify();
                    },
                    cx,
                ));
                column = column.child(
                    div()
                        .absolute()
                        .right(px(SPACE_4))
                        .bottom(px(SPACE_4))
                        .h(px(24.))
                        .px(px(SPACE_2))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(rgba(0x00000099))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child(format!("{} / {}", current + 1, len)),
                );
            }
        } else if !cover_url.is_empty() {
            column = column.child(
                img(ImageSource::from(cover_url.to_string()))
                    .absolute()
                    .inset_0()
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Contain),
            );
        }

        if self.item.kind == DynamicKind::Video {
            column = column.child(
                div()
                    .w(px(64.))
                    .h(px(64.))
                    .rounded_full()
                    .bg(rgba(0x00000099))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Play)
                            .size(px(28.))
                            .text_color(rgb(0xFFFFFF)),
                    ),
            );
        }

        // 底部缩略图条：多图时给当前位置一个明确锚点。
        if len > 1 {
            let mut strip = div()
                .absolute()
                .left(px(SPACE_4))
                .right(px(SPACE_4))
                .bottom(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(6.))
                .overflow_hidden();
            for (i, image) in images.iter().take(8).enumerate() {
                let active = i == current;
                let thumb = image.thumb_url(96);
                strip = strip.child(
                    div()
                        .w(px(44.))
                        .h(px(34.))
                        .flex_shrink_0()
                        .rounded(px(RADIUS_SM))
                        .overflow_hidden()
                        .border_1()
                        .border_color(if active {
                            t.palette.theme_primary
                        } else {
                            rgba(0xFFFFFF33).into()
                        })
                        .child(
                            img(ImageSource::from(thumb))
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
                        ),
                );
            }
            // 缩略图条固定在媒体底部，不与右侧正文重复。
            column = column.child(strip);
        }

        column.into_any_element()
    }

    /// 媒体右侧：正文 + 操作 + 评论。
    fn article_column(&mut self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        let body = self.article_body(t);
        div()
            .id("dynamic-detail-article")
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .track_scroll(&self.right_scroll)
            .overflow_y_scroll()
            .child(div().w_full().px(px(SPACE_6)).pt(px(SPACE_4)).child(body))
            .child(self.action_row(t))
            .child(div().mx(px(SPACE_6)).h(px(1.)).bg(s.border_subtle))
            .child(self.comments_column(t, false, cx))
            .into_any_element()
    }

    fn article_body(&self, t: &YkTheme) -> AnyElement {
        let s = &t.surfaces;
        let item = &self.item;
        let mut col = div().flex().flex_col().gap(px(SPACE_3)).w_full();
        if !item.title.trim().is_empty() {
            col = col.child(
                div()
                    .text_size(px(FONT_SIZE_HEADING))
                    .line_height(px(LINE_HEIGHT_HEADING))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child(item.title.clone()),
            );
        }
        if !item.text.trim().is_empty() {
            col = col.child(
                div()
                    .text_size(px(FONT_SIZE_BODY))
                    .line_height(px(LINE_HEIGHT_BODY))
                    .text_color(s.text_secondary)
                    .child(item.text.clone()),
            );
        }

        if let Some(origin) = item.origin.as_ref() {
            let quote_text = if let Some(video) = origin.video.as_ref() {
                format!("视频：{}", video.title)
            } else if !origin.text.trim().is_empty() {
                origin.text.clone()
            } else if !origin.title.trim().is_empty() {
                origin.title.clone()
            } else {
                "原动态".to_string()
            };
            col = col.child(
                div()
                    .p(px(SPACE_3))
                    .rounded(px(RADIUS_LG))
                    .bg(s.fill_1)
                    .border_1()
                    .border_color(s.border_subtle)
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_1))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.palette.theme_primary)
                            .child(format!("@{}", origin.author_name())),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .line_height(px(LINE_HEIGHT_CONTROL))
                            .text_color(s.text_secondary)
                            .child(quote_text),
                    ),
            );
        }

        if let Some(link) = item.link.as_ref() {
            if link.cover.is_empty() {
                col = col.child(
                    div()
                        .p(px(SPACE_3))
                        .rounded(px(RADIUS_LG))
                        .bg(s.fill_1)
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(t.palette.theme_primary)
                        .child("查看链接 ›"),
                );
            }
        }

        col.into_any_element()
    }

    fn action_row(&self, t: &YkTheme) -> AnyElement {
        let s = &t.surfaces;
        let stats = &self.item.stats;
        div()
            .flex()
            .items_center()
            .gap(px(SPACE_6))
            .h(px(52.))
            .px(px(SPACE_6))
            .child(detail_action(t, IconName::Share2, "转发", stats.forward))
            .child(detail_action(
                t,
                IconName::MessageCircle,
                "评论",
                stats.comment,
            ))
            .child(detail_action(t, IconName::ThumbsUp, "赞", stats.like))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_quaternary)
                    .child("举报"),
            )
            .into_any_element()
    }

    /// 纯文字模式的左栏：标题 + 正文，可滚动。
    fn text_column(&mut self, t: &YkTheme, _with_header: bool) -> AnyElement {
        let s = &t.surfaces;
        div()
            .id("dynamic-detail-text")
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .track_scroll(&self.left_scroll)
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .px(px(SPACE_6))
                    .py(px(SPACE_5))
                    .child(self.article_body(t)),
            )
            .child(self.action_row(t))
            .child(div().mx(px(SPACE_6)).h(px(1.)).bg(s.border_subtle))
            .into_any_element()
    }

    /// 评论栏。
    ///
    /// `standalone = true` 时它是「纯文字模式」的右栏，自带滚动；
    /// 否则嵌在正文右侧的滚动容器里，只负责评论列表本身。
    fn comments_column(
        &mut self,
        t: &YkTheme,
        standalone: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let count = self.item.stats.comment;
        let mut list = div().flex().flex_col().w_full();
        match &self.comments {
            CommentState::None => {}
            CommentState::Loading => {
                for _ in 0..3 {
                    list = list.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(SPACE_2))
                            .py(px(SPACE_3))
                            .child(
                                div()
                                    .w(px(28.))
                                    .h(px(28.))
                                    .rounded_full()
                                    .bg(s.skeleton_brush),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .h(px(10.))
                                            .w(relative(0.35))
                                            .rounded(px(RADIUS_SM))
                                            .bg(s.skeleton_brush),
                                    )
                                    .child(
                                        div()
                                            .h(px(12.))
                                            .w(relative(0.85))
                                            .rounded(px(RADIUS_SM))
                                            .bg(s.skeleton_brush),
                                    ),
                            ),
                    );
                }
            }
            CommentState::Error(error) => {
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_2))
                        .py(px(SPACE_4))
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_tertiary)
                                .child(format!("评论加载失败：{error}")),
                        )
                        .child(
                            div()
                                .id("dynamic-comments-retry")
                                .h(px(28.))
                                .px(px(SPACE_3))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .bg(s.fill_1)
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(s.text_secondary)
                                .cursor_pointer()
                                .child("重试")
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.retry_comments(cx);
                                })),
                        ),
                );
            }
            CommentState::Ready(comments) => {
                if comments.is_empty() {
                    list = list.child(
                        div()
                            .py(px(SPACE_6))
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_tertiary)
                            .child("还没有评论，来抢沙发吧"),
                    );
                } else {
                    for comment in comments {
                        list = list.child(comment_row(t, comment));
                    }
                }
            }
        }

        let header = div()
            .flex()
            .items_center()
            .gap(px(SPACE_3))
            .py(px(SPACE_3))
            .child(
                div()
                    .text_size(px(FONT_SIZE_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child(format!("评论 {}", format::count(count))),
            )
            .child(
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(t.palette.theme_primary)
                    .child("最热"),
            )
            .child(
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_quaternary)
                    .child("最新"),
            );

        if standalone {
            div()
                .id("dynamic-detail-comments")
                .flex()
                .flex_col()
                .h_full()
                .w_full()
                .min_h(px(0.))
                .min_w(px(0.))
                .track_scroll(&self.right_scroll)
                .overflow_y_scroll()
                .child(div().w_full().px(px(SPACE_6)).child(header))
                .child(div().w_full().px(px(SPACE_6)).child(list))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .px(px(SPACE_6))
                .child(header)
                .child(list)
                .into_any_element()
        }
    }
}

impl Render for DynamicDetailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.focus_handle.clone();
            cx.defer_in(window, move |_view, window, cx| {
                window.focus(&handle, cx);
            });
        }

        let t = self.theme.clone();
        let root_size = self
            .origin
            .map(|o| o.root_size)
            .unwrap_or_else(|| window.viewport_size());
        let target = target_bounds(root_size);
        let start = self
            .origin
            .map(|o| o.bounds)
            .unwrap_or_else(|| fallback_origin(target));
        let scale_factor = window.scale_factor();
        let content = self.content(&t, target, scale_factor, cx);
        let weak = cx.entity().downgrade();
        let shell = DetailShell {
            theme: t,
            item: self.item.clone(),
            start,
            target,
            progress: 1.0,
            closing: self.closing,
            content: Some(content),
            weak: weak.clone(),
            focus_handle: self.focus_handle.clone(),
        };

        if self.closing {
            shell
                .with_animation(
                    format!("dynamic-detail-exit-{}", self.closing_generation),
                    Animation::new(Duration::from_millis(DETAIL_EXIT_MS)),
                    |el, progress| DetailShell { progress, ..el },
                )
                .into_any_element()
        } else {
            shell
                .with_animation(
                    format!("dynamic-detail-enter-{}", self.entrance_generation),
                    Animation::new(Duration::from_millis(DETAIL_ENTRANCE_MS)),
                    |el, progress| DetailShell { progress, ..el },
                )
                .into_any_element()
        }
    }
}

#[derive(gpui::IntoElement)]
struct DetailShell {
    theme: YkTheme,
    item: DynamicItem,
    start: Bounds<Pixels>,
    target: Bounds<Pixels>,
    /// 原始动画进度（未缓动）。
    progress: f32,
    closing: bool,
    content: Option<AnyElement>,
    weak: WeakEntity<DynamicDetailView>,
    focus_handle: FocusHandle,
}

impl RenderOnce for DetailShell {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let DetailShell {
            theme,
            item,
            start,
            target,
            progress,
            closing,
            content,
            weak,
            focus_handle,
        } = self;
        let raw = progress.clamp(0.0, 1.0);
        let eased = ease_entrance()(raw);
        let scale_factor = window.scale_factor();
        let media_w = target.size.width.as_f32() * 0.56;
        let p = if closing { 1.0 - eased } else { eased };
        let bounds = lerp_bounds(start, target, p);

        let mut scrim_color = theme.surfaces.overlay_scrim;
        scrim_color.a = (scrim_color.a * 0.82 * p).clamp(0.0, 0.82);
        let scrim = div()
            .id("dynamic-detail-scrim")
            .absolute()
            .inset_0()
            .bg(scrim_color)
            .occlude()
            .on_click({
                let weak = weak.clone();
                move |_: &ClickEvent, _window, cx| {
                    let _ = weak.update(cx, |view, cx| view.begin_close(cx));
                }
            });

        let mut shell = div()
            .id("dynamic-detail-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(MODAL_RADIUS))
            .overflow_hidden()
            .bg(theme.surfaces.surface_content_solid)
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude()
            .key_context("DynamicDetail")
            .track_focus(&focus_handle)
            .on_key_down(detail_escape_handler(weak.clone()))
            .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation());

        if p >= 1.0 {
            shell = shell.child(content.expect("详情展开结束时必须挂上完整内容"));
        } else {
            shell = shell.child(detail_poster(&theme, &item, media_w, scale_factor));
        }

        // 关闭按钮固定在浮层右上角，不随内容滚动。
        shell = shell.child(
            div()
                .id("dynamic-detail-close")
                .absolute()
                .top(px(SPACE_4))
                .right(px(SPACE_4))
                .w(px(30.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgba(0x00000040))
                .text_size(px(18.))
                .text_color(theme.surfaces.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0x00000066)))
                .child("×")
                .on_click({
                    let weak = weak.clone();
                    move |_: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        let _ = weak.update(cx, |view, cx| view.begin_close(cx));
                    }
                }),
        );

        div().absolute().inset_0().child(scrim).child(shell)
    }
}

fn detail_escape_handler(
    weak: WeakEntity<DynamicDetailView>,
) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    move |event, _window, cx| {
        if event.keystroke.key != "escape" {
            return;
        }
        let _ = weak.update(cx, |view, cx| view.begin_close(cx));
        cx.stop_propagation();
    }
}

fn target_bounds(root: Size<Pixels>) -> Bounds<Pixels> {
    let root_w = root.width.as_f32().max(640.0);
    let root_h = root.height.as_f32().max(480.0);
    let w = (root_w - SPACE_12 * 2.0).clamp(520.0, 1120.0);
    let h = (root_h - SPACE_12 * 2.0).clamp(420.0, 760.0);
    Bounds {
        origin: point(px((root_w - w) / 2.0), px((root_h - h) / 2.0)),
        size: size(px(w), px(h)),
    }
}

fn fallback_origin(target: Bounds<Pixels>) -> Bounds<Pixels> {
    let w = 340.0_f32.min(target.size.width.as_f32());
    let h = 220.0_f32.min(target.size.height.as_f32());
    Bounds {
        origin: point(
            px(target.origin.x.as_f32() + (target.size.width.as_f32() - w) / 2.0),
            px(target.origin.y.as_f32() + (target.size.height.as_f32() - h) / 2.0),
        ),
        size: size(px(w), px(h)),
    }
}

fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |a: Pixels, b: Pixels| px(a.as_f32() + (b.as_f32() - a.as_f32()) * t);
    Bounds {
        origin: point(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        size: size(
            mix(a.size.width, b.size.width),
            mix(a.size.height, b.size.height),
        ),
    }
}

/// 展开/收起期间的过渡海报：封面 + 作者 + 两行摘要。
///
/// 只用来避免完整布局被塞进卡片大小；到终点就换成真正的内容。
fn detail_poster(
    theme: &YkTheme,
    item: &DynamicItem,
    media_w: f32,
    scale_factor: f32,
) -> AnyElement {
    let s = &theme.surfaces;
    let p = &theme.palette;
    let image = item
        .images
        .first()
        .map(|i| i.fit_url(media_w, scale_factor))
        .or_else(|| {
            item.video
                .as_ref()
                .map(|v| asset_url(&v.cover, fitted_width(media_w, scale_factor, 0)))
        })
        .or_else(|| {
            item.link
                .as_ref()
                .map(|l| asset_url(&l.cover, fitted_width(media_w, scale_factor, 0)))
        })
        .or_else(|| {
            item.origin.as_ref().and_then(|o| {
                o.images
                    .first()
                    .map(|i| i.fit_url(media_w, scale_factor))
                    .or_else(|| {
                        o.video
                            .as_ref()
                            .map(|v| asset_url(&v.cover, fitted_width(media_w, scale_factor, 0)))
                    })
            })
        })
        .unwrap_or_default();
    let face_url = item.author_face_url(30.0, scale_factor);

    let mut poster = div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .bg(placeholder_gradient(&item.placeholder_color, s.fill_3));
    if !image.is_empty() {
        poster = poster.child(
            img(ImageSource::from(image))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
        poster = poster.child(div().absolute().inset_0().bg(linear_gradient(
            180.,
            linear_color_stop(rgba(0x00000000), 0.35),
            linear_color_stop(rgba(0x000000CC), 1.0),
        )));
    }

    poster
        .child(
            div()
                .absolute()
                .left(px(SPACE_4))
                .right(px(SPACE_4))
                .bottom(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .w(px(30.))
                        .h(px(30.))
                        .flex_shrink_0()
                        .rounded_full()
                        .overflow_hidden()
                        .bg(linear_gradient(
                            135.,
                            linear_color_stop(p.bili_blue, 0.),
                            linear_color_stop(p.bili_pink, 1.),
                        ))
                        .when(!face_url.is_empty(), |el| {
                            el.child(
                                img(ImageSource::from(face_url.clone()))
                                    .w(px(30.))
                                    .h(px(30.))
                                    .object_fit(ObjectFit::Cover),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .truncate()
                                .child(item.author_name().to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(rgba(0xFFFFFFB3))
                                .truncate()
                                .child(item.published_text()),
                        ),
                ),
        )
        .into_any_element()
}

fn media_arrow(
    glyph: &'static str,
    left: bool,
    action: impl Fn(&mut DynamicDetailView, &mut Context<DynamicDetailView>) + 'static,
    cx: &mut Context<DynamicDetailView>,
) -> impl IntoElement {
    let bg = rgba(0x00000080);
    div()
        .id(if left {
            "dynamic-media-prev"
        } else {
            "dynamic-media-next"
        })
        .absolute()
        .top(relative(0.5))
        .when(left, |el| el.left(px(SPACE_3)))
        .when(!left, |el| el.right(px(SPACE_3)))
        .mt(px(-18.))
        .w(px(36.))
        .h(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(bg)
        .text_size(px(22.))
        .text_color(rgb(0xFFFFFF))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0x000000CC)))
        .child(glyph)
        .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            action(view, cx);
        }))
        .into_any_element()
}

fn detail_action(t: &YkTheme, icon: IconName, label: &'static str, count: u64) -> impl IntoElement {
    let s = &t.surfaces;
    let color = s.text_tertiary;
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_1))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(color)
        .child(Icon::new(icon).size(px(16.)).text_color(color))
        .child(
            div()
                .text_color(s.text_secondary)
                .child(format!("{label} {}", format::count(count))),
        )
}

fn comment_row(t: &YkTheme, comment: &DynamicComment) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex()
        .items_start()
        .gap(px(SPACE_2))
        .py(px(SPACE_3))
        .border_b_1()
        .border_color(s.border_subtle)
        .w_full()
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .flex_shrink_0()
                .rounded_full()
                .overflow_hidden()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .when(!comment.face.is_empty(), |el| {
                    el.child(
                        img(ImageSource::from(comment.face.clone()))
                            .w(px(28.))
                            .h(px(28.))
                            .object_fit(ObjectFit::Cover),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(comment.author.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .text_color(s.text_secondary)
                        .child(comment.text.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(comment.time.clone()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_quaternary)
                .child(Icon::new(IconName::ThumbsUp).size(px(12.)))
                .child(format::count(comment.like)),
        )
}
