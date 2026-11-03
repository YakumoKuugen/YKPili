//! 动态详情浮层：图文 / 文字动态的展开界面。
//!
//! 交互与首页封面卡的播放器一致：
//! - 从被点击的卡片矩形**连续展开**到浮层目标框（280ms，同一缓动）；
//! - 前 30% 用临时卡片与详情页交叉淡化，后 70% 详情页缩放到正常大小；
//! - 点浮层外部（遮罩）/ 右上角 × / Esc 关闭，260ms 缩回原卡片位置。

use gpui::assets::IconName;
use gpui::component::menu::ContextMenuExt as _;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::TestSupportExt as _;
use gpui::*;
use std::rc::Rc;
use std::time::Duration;
use yk_infra::dynamic::{
    asset_url, fitted_width, DynamicComment, DynamicImage, DynamicItem, DynamicKind,
};
use yk_domain::format;
use yk_components::status;
use yk_theme::theme::*;

use yk_player::comment_view::text_copy_menu;

use crate::dynamic_card::card_ghost;
use yk_components::cover::placeholder_gradient;

/// 与播放器保持同一节奏。
const DETAIL_ENTRANCE_MS: u64 = 280;
const DETAIL_EXIT_MS: u64 = 260;

/// 点击卡片时冻结的几何。`bounds` 是整张卡，`cover_bounds` 是视频缩略图。
pub use crate::dynamic_card::CardOrigin;

#[derive(Clone)]
enum CommentState {
    None,
    Loading,
    Ready,
    Error(String),
}

/// 详情浮层视图。
pub struct DynamicDetailView {
    theme: YkTheme,
    item: DynamicItem,
    origin: Option<CardOrigin>,
    cookie: String,
    comments: CommentState,
    /// 评论区：与播放器信息面板共用同一份实现（列表 / 翻页 / 输入条 / 回到顶部）。
    comments_section: yk_player::comment_section::Section,
    image_index: usize,
    left_scroll: ScrollHandle,
    right_scroll: ScrollHandle,
    entrance_generation: u64,
    closing: bool,
    closing_generation: u64,
    close_handler: Option<Box<dyn Fn(&mut App, bool)>>,
    /// 「点大图放大」回调：把整组图片与当前序号交给宿主（AppRoot 的查看器）。
    image_open_handler: Option<Box<dyn Fn(&mut App, &mut Window, Vec<String>, usize)>>,
    focus_handle: FocusHandle,
    needs_focus: bool,
    /// 图片复制请求进行中（按钮显示「复制中」）。
    copying_image: bool,
    /// 底部轻提示（复制成功 / 失败），`notice_generation` 防止旧定时器误清。
    notice: Option<String>,
    notice_generation: u64,
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
            comments_section: yk_player::comment_section::Section::default(),
            image_index: 0,
            left_scroll: ScrollHandle::new(),
            right_scroll: ScrollHandle::new(),
            entrance_generation: ENTRANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            closing: false,
            closing_generation: 0,
            close_handler: None,
            image_open_handler: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            needs_focus: true,
            copying_image: false,
            notice: None,
            notice_generation: 0,
        };
        view.spawn_comments(cx);
        view
    }

    /// 设置「点大图放大」回调。
    pub fn set_image_open_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, Vec<String>, usize) + 'static,
    ) {
        self.image_open_handler = Some(Box::new(handler));
    }

    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App, bool) + 'static) {
        self.close_handler = Some(Box::new(handler));
    }

    /// 测试入口：直接灌入评论，跳过网络请求。
    #[doc(hidden)]
    pub fn set_test_comments(&mut self, comments: Vec<DynamicComment>) {
        self.comments_section.list.apply_first(
            yk_infra::api::client::CommentPage {
                items: comments,
                next: 0,
                is_end: true,
                total: 0,
            },
            true,
        );
        self.comments = CommentState::Ready;
    }

    /// 测试入口：挂一个评论目标（不发请求），用来验证输入条等交互。
    #[doc(hidden)]
    pub fn set_test_comment_target(&mut self, target: yk_infra::dynamic::CommentTarget) {
        self.comments_section
            .set_target(Some(target.clone()), String::new());
        self.item.comment_target = Some(target);
        self.comments = CommentState::Ready;
    }

    /// 拉评论第一页。翻页 / 去重 / 输入条全在共享的 `comment_section` 里。
    fn spawn_comments(&mut self, cx: &mut Context<Self>) {
        self.comments_section
            .set_target(self.item.comment_target.clone(), self.cookie.clone());
        self.comments = CommentState::Loading;
        self.comments_section.load_first(cx);
        self.comments = CommentState::Ready;
    }

    fn retry_comments(&mut self, cx: &mut Context<Self>) {
        self.spawn_comments(cx);
    }

    /// 底部轻提示：1.8s 后自动消失，重复触发会重置计时。
    fn show_notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1800))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.notice_generation == generation {
                    view.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 复制当前左侧大图：先读原始字节（磁盘缓存优先），再写成图片剪贴板。
    fn copy_image(&mut self, url: String, cx: &mut Context<Self>) {
        if self.copying_image || url.trim().is_empty() {
            return;
        }
        self.copying_image = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { yk_infra::http::read_image_bytes(&url) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.copying_image = false;
                match result {
                    Ok((bytes, format)) => {
                        yk_infra::http::copy_image_to_clipboard(cx, bytes, format);
                        view.show_notice("图片已复制", cx);
                    }
                    Err(error) => {
                        eprintln!("[dynamic] 复制图片失败：{error}");
                        view.show_notice("图片复制失败", cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 从外部触发一次带退场动画的关闭（宿主 / 验收钩子用）。
    pub fn begin_close(&mut self, cx: &mut Context<Self>) {
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
        window: &mut Window,
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

        // 评论列宽度 = 详情目标宽的 44% − 左右 padding − 头像 − 间距；
        // 给 `SelectableText` 一个确定宽度，正文才能正常折行。
        let comments_width = target.size.width.as_f32() * 0.44 - 1.0;
        let comment_text_width =
            (comments_width - SPACE_6 * 2.0 - 28.0 - SPACE_2).max(120.0);
        let body_text_width = (comments_width - SPACE_6 * 2.0).max(160.0);
        let left_text_width = (target.size.width.as_f32() * 0.56 - SPACE_6 * 2.0).max(160.0);
        let body = if has_media {
            // 封面也按原图复制：显示用的 `cover_url` 带 `@xxxw.webp` 缩放后缀。
            let cover_copy_url = if cover.is_empty() {
                String::new()
            } else {
                yk_infra::dynamic::original_url(&cover)
            };
            let media = self.media_column(
                t,
                &images,
                &cover_url,
                &cover_copy_url,
                media_w,
                scale_factor,
                cx,
            );
            let right = self.article_column(
                t,
                body_text_width,
                comment_text_width,
                comments_width,
                window,
                cx,
            );
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
            let left = self.text_column(t, left_text_width, cx);
            // 右栏 = [评论滚动区][输入条]：输入条常驻底部，不跟列表一起滚。
            let can_comment = self.item.comment_target.is_some();
            let composer_width = comments_width;
            self.comments_section.sync_scroll(&self.right_scroll);
            let right = div()
                .relative()
                .flex()
                .flex_col()
                .h_full()
                .min_h(px(0.))
                .child(self.comments_column(t, true, comment_text_width, window, cx))
                // 输入条 / 回到顶部与播放器评论区是同一份组件。
                .when(can_comment, |el| {
                    el.child(
                        self.comments_section
                            .composer(t, window, cx, composer_width),
                    )
                })
                .child(
                    self.comments_section
                        .back_to_top(t, &self.right_scroll, cx),
                );
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
            // 拖选文字时选区只在鼠标事件里更新；没有这一层 notify，
            // 高亮要等到松手才出现（播放器面板本来就有 on_mouse_move）。
            .on_mouse_move(cx.listener(|_: &mut Self, _: &MouseMoveEvent, _, cx| {
                cx.notify();
            }))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn media_column(
        &mut self,
        t: &YkTheme,
        images: &[DynamicImage],
        cover_url: &str,
        cover_copy_url: &str,
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
            // 点大图 → 打开查看器（滚轮缩放 / 拖动 / 左右切换）。
            let urls: Vec<String> = images.iter().map(|image| image.detail_url()).collect();
            column = column.child(
                div()
                    .id("dynamic-detail-image")
                    .absolute()
                    .inset_0()
                    .cursor_pointer()
                    .child(
                        img(ImageSource::from(
                            images[current].fit_url(media_w, scale_factor),
                        ))
                        .absolute()
                        .inset_0()
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Contain),
                    )
                    .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                        if let Some(handler) = view.image_open_handler.as_ref() {
                            handler(cx, window, urls.clone(), current);
                        }
                        cx.stop_propagation();
                    })),
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
            if self.item.kind == DynamicKind::Video {
                // 视频封面复用公共卡面（渐变兜底 + 真实封面），
                // 不再各画各的；这里是详情页整块媒体区、不限 16:9，
                // 所以只借 `cover_art` 的画，播放按钮仍由下面单独叠。
                column = column.child(crate::cover::cover_art(
                    &self.item.title,
                    cover_url,
                    Some(&self.item.placeholder_color),
                    t.surfaces.fill_3,
                ));
            } else {
                column = column.child(
                    img(ImageSource::from(cover_url.to_string()))
                        .absolute()
                        .inset_0()
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Contain),
                );
            }
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

        // 右下角「复制图片」：半透明材质浮在大图上，多图时抬到缩略图条上方。
        //
        // 复制**原图**而不是屏幕上这张：显示用的地址是按显示宽度取的
        // `@480w.webp` 缩略图，贴到聊天框里一放大就糊。原图过宽时
        // `copy_url` 会收一档到 2560，避免解码 + 写剪贴板占太多内存。
        let copy_url = if len > 0 {
            images[current].copy_url()
        } else {
            cover_copy_url.to_string()
        };
        if !copy_url.is_empty() {
            column = column.child(self.image_copy_button(t, copy_url, len > 1, cx));
        }

        column.into_any_element()
    }

    /// 图片右下角的半透明复制按钮。
    fn image_copy_button(
        &self,
        t: &YkTheme,
        url: String,
        above_gallery: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copying = self.copying_image;
        let bottom = if above_gallery {
            SPACE_4 + 34.0 + SPACE_2
        } else {
            SPACE_4
        };
        div()
            .id("dynamic-image-copy")
            .debug_selector(|| "dynamic-image-copy".into())
            .absolute()
            .right(px(SPACE_4))
            .bottom(px(bottom))
            .h(px(32.))
            .px(px(SPACE_3))
            .flex()
            .items_center()
            .gap(px(SPACE_1 + 2.0))
            .rounded_full()
            .bg(rgba(0x00000099))
            .border_1()
            .border_color(rgba(0xFFFFFF33))
            .shadow(t.shadows.shadow_2.clone())
            .text_size(px(FONT_SIZE_CAPTION))
            .text_color(rgb(0xFFFFFF))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0x000000CC)))
            .when(copying, |el| el.opacity(0.65))
            .child(Icon::new(IconName::Copy).size(px(14.)))
            .child(if copying { "复制中…" } else { "复制" })
            .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                view.copy_image(url.clone(), cx);
            }))
            .test_support()
            .into_any_element()
    }

    /// 媒体右侧：正文 + 操作 + 评论。
    fn article_column(
        &mut self,
        t: &YkTheme,
        body_text_width: f32,
        comment_text_width: f32,
        column_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let weak = cx.entity().downgrade();
        let body = self.article_body(t, &weak, body_text_width);
        let scroll = div()
            .id("dynamic-detail-article")
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .track_scroll(&self.right_scroll)
            .overflow_y_scroll()
            // 滚到底部附近自动补下一页评论。
            .on_scroll_wheel(cx.listener(
                |view, _: &ScrollWheelEvent, _window, cx| {
                    view.comments_section
                        .maybe_load_more(&view.right_scroll, cx);
                    cx.notify();
                },
            ))
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(SPACE_6))
                    .pt(px(SPACE_4))
                    .child(body),
            )
            .child(self.action_row(t))
            .child(div().mx(px(SPACE_6)).h(px(1.)).bg(s.border_subtle))
            .child(self.comments_column(t, false, comment_text_width, window, cx));
        let can_comment = self.item.comment_target.is_some();
        let composer_width = column_width;
        self.comments_section.sync_scroll(&self.right_scroll);
        div()
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .min_h(px(0.))
            .child(scroll)
            .when(can_comment, |el| {
                el.child(
                    self.comments_section
                        .composer(t, window, cx, composer_width),
                )
            })
            .child(
                self.comments_section
                    .back_to_top(t, &self.right_scroll, cx),
            )
            .into_any_element()
    }

    /// 标题 + 正文 + 转发引用。
    ///
    /// 每段文字都挂同一个右键菜单（只有「复制文本」一项），复制内容跟随
    /// 被点击的那一段；评论正文走共用的 `comment_view::comment_row`。
    fn article_body(&self, t: &YkTheme, weak: &WeakEntity<Self>, text_width: f32) -> AnyElement {
        let s = &t.surfaces;
        let item = &self.item;
        let copy_notice = {
            let weak = weak.clone();
            Rc::new(move |cx: &mut App| {
                let _ = weak.update(cx, |view, cx| view.show_notice("已复制", cx));
            })
        };
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .w_full()
            .flex_shrink_0();
        if !item.title.trim().is_empty() {
            let title = item.title.clone();
            col = col.child(
                div()
                    .id("dynamic-detail-title")
                    .debug_selector(|| "dynamic-detail-title".into())
                    .w(px(text_width))
                    .child(
                        gpui::base::SelectableText::new("dynamic-detail-title-text", title.clone())
                            .text_style(gpui::TextStyleRefinement {
                                font_size: Some(px(FONT_SIZE_HEADING).into()),
                                line_height: Some(px(LINE_HEIGHT_HEADING).into()),
                                font_weight: Some(FontWeight::SEMIBOLD),
                                color: Some(s.text_primary),
                                ..Default::default()
                            }),
                    )
                    .context_menu(text_copy_menu(title, copy_notice.clone())),
            );
        }
        if !item.text.trim().is_empty() {
            let text = item.text.clone();
            col = col.child(
                div().id("dynamic-detail-body").test_support().child(
                    div()
                        .id("dynamic-detail-body-text")
                        .w(px(text_width))
                        .child(
                            gpui::base::SelectableText::new(
                                "dynamic-detail-body-text-select",
                                text.clone(),
                            )
                            .text_style(gpui::TextStyleRefinement {
                                font_size: Some(px(FONT_SIZE_BODY).into()),
                                line_height: Some(px(LINE_HEIGHT_BODY).into()),
                                color: Some(s.text_secondary),
                                ..Default::default()
                            }),
                        )
                        .context_menu(text_copy_menu(text, copy_notice.clone())),
                ),
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
                    .flex_shrink_0()
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
                            .id("dynamic-origin-text")
                            .w(px((text_width - SPACE_3 * 2.0).max(120.0)))
                            .child(
                                gpui::base::SelectableText::new(
                                    "dynamic-origin-text-select",
                                    quote_text.clone(),
                                )
                                .text_style(
                                    gpui::TextStyleRefinement {
                                        font_size: Some(px(FONT_SIZE_CONTROL).into()),
                                        line_height: Some(px(LINE_HEIGHT_CONTROL).into()),
                                        color: Some(s.text_secondary),
                                        ..Default::default()
                                    },
                                ),
                            )
                            .context_menu(text_copy_menu(quote_text, copy_notice.clone())),
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
            .flex_shrink_0()
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
    fn text_column(&mut self, t: &YkTheme, text_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        let weak = cx.entity().downgrade();
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
                    .flex_shrink_0()
                    .px(px(SPACE_6))
                    .py(px(SPACE_5))
                    .child(self.article_body(t, &weak, text_width)),
            )
            .child(self.action_row(t))
            .child(div().mx(px(SPACE_6)).h(px(1.)).bg(s.border_subtle))
            .into_any_element()
    }

    /// 评论栏：列表与播放器信息面板共用 `comment_section`。
    ///
    /// `standalone = true` 时它是「纯文字模式」的右栏，自带滚动；
    /// 否则嵌在正文右侧的滚动容器里，只负责评论列表本身。
    fn comments_column(
        &mut self,
        t: &YkTheme,
        standalone: bool,
        comment_text_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        // 服务端返回的总数比卡片上的计数更新（发完评论后会 +1）。
        let count = if self.comments_section.list.total > 0 {
            self.comments_section.list.total
        } else {
            self.item.stats.comment
        };
        let weak = cx.entity().downgrade();
        let mut list = div().flex().flex_col().w_full().flex_shrink_0();
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
            CommentState::Ready => {
                if self.comments_section.list.items.is_empty() {
                    list = list.child(
                        div()
                            .py(px(SPACE_6))
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_tertiary)
                            .child("还没有评论，来抢沙发吧"),
                    );
                } else {
                    let copy_notice = {
                        let weak = weak.clone();
                        Rc::new(move |cx: &mut App| {
                            let _ = weak.update(cx, |view, cx| {
                                view.show_notice("已复制", cx);
                            });
                        })
                    };
                    let on_image = {
                        let weak = weak.clone();
                        Rc::new(
                            move |urls: Vec<String>,
                                  index: usize,
                                  window: &mut Window,
                                  cx: &mut App| {
                                let _ = weak.update(cx, |view, cx| {
                                    if let Some(handler) = view.image_open_handler.as_ref() {
                                        handler(cx, window, urls, index);
                                    }
                                });
                            },
                        ) as Rc<dyn Fn(Vec<String>, usize, &mut Window, &mut App)>
                    };
                    list = list.child(self.comments_section.body(
                        t,
                        window,
                        &self.right_scroll,
                        comment_text_width,
                        "dynamic-comment",
                        copy_notice,
                        Some(on_image),
                        cx,
                    ));
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
                // 外层是「[滚动区][输入条]」的 flex 列，这里吃剩余高度。
                .flex_1()
                .w_full()
                .min_h(px(0.))
                .min_w(px(0.))
                .track_scroll(&self.right_scroll)
                .overflow_y_scroll()
                .on_scroll_wheel(cx.listener(
                    |view, _: &ScrollWheelEvent, _window, cx| {
                        view.comments_section
                            .maybe_load_more(&view.right_scroll, cx);
                        cx.notify();
                    },
                ))
                .child(div().w_full().px(px(SPACE_6)).child(header))
                .child(div().w_full().px(px(SPACE_6)).child(list))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .px(px(SPACE_6))
                .child(header)
                .child(list)
                .into_any_element()
        }
    }

}

impl yk_player::comment_section::CommentHost for DynamicDetailView {
    fn comments(&mut self) -> &mut yk_player::comment_section::Section {
        &mut self.comments_section
    }

    fn comment_notice(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.show_notice(text, cx);
        let _ = error;
    }

    fn comment_focus_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
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
        let content = self.content(&t, target, scale_factor, window, cx);
        let weak = cx.entity().downgrade();
        let shell = DetailShell {
            theme: t,
            item: self.item.clone(),
            start,
            target,
            progress: 1.0,
            closing: self.closing,
            content: Some(content),
            notice: self.notice.clone(),
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
    /// 复制成功 / 失败提示。挂在整窗层而不是详情页内层，才能落到窗口底部。
    notice: Option<String>,
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
            notice,
            weak,
            focus_handle,
        } = self;
        let raw = progress.clamp(0.0, 1.0);
        let scale_factor = window.scale_factor();
        // 打开 / 关闭共用一套「30% 交叉淡化 + 70% 缩放」：
        // - 打开：临时卡片(原卡尺寸) 1→0，详情页 0→1，同时外壳放大；
        // - 关闭：临时卡片(详情尺寸) 0→1，详情页 1→0，同时外壳缩小。
        let (pos, swap) = if closing {
            let pos = detail_entrance_position(1.0 - raw).0;
            (pos, detail_swap(raw))
        } else {
            detail_entrance_position(raw)
        };
        let p = pos.clamp(0.0, 1.0);
        let bounds = lerp_bounds(start, target, p);
        let content_opacity = if closing { 1.0 - swap } else { swap };
        let ghost_opacity = if closing { swap } else { 1.0 - swap };
        let scrim_t = if closing { 1.0 - raw } else { raw };

        let mut scrim_color = theme.surfaces.overlay_scrim;
        scrim_color.a = (scrim_color.a * 0.82 * scrim_t).clamp(0.0, 0.82);
        let scrim = div()
            .id("dynamic-detail-scrim")
            .absolute()
            .inset_0()
            .bg(scrim_color)
            .occlude()
            .on_click({
                let weak = weak.clone();
                move |_: &ClickEvent, _window, cx| {
                    // 关闭这一拍不要再传给下层 feed 卡片，否则浮层刚开始
                    // 缩回就又打开了下一张动态。
                    cx.stop_propagation();
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
            .on_key_down(detail_escape_handler(weak.clone()));
        // 不再在外壳上 `on_click(stop_propagation)`：那会把 MouseUp 从
        // 冒泡链上截断，`TextSelection` 的窗口级收尾监听收不到
        // （`is_selecting` 一直是 true），于是「点一下 A、再点一下 B」
        // 会把中间的文字选中。壳子已经 `.occlude()`，点击不会漏到下层。

        // 详情页按最终尺寸排版，由正在缩放的外壳裁剪；临时卡片填满外壳，
        // 因此关闭时它从「详情页同尺寸」开始，随外壳一起缩回原卡片位置。
        let content = content.expect("详情浮层必须有完整内容");
        let mut layer = shell.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(target.size.width.as_f32()))
                .h(px(target.size.height.as_f32()))
                .opacity(content_opacity)
                .child(content),
        );

        if ghost_opacity > 0.0 {
            layer = layer.child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(ghost_opacity)
                    // 临时卡片与 feed 卡片同材质、同结构，可被其它
                    // 展开 / 收起动画复用；透明度归零时直接不挂。
                    .child(card_ghost(
                        &theme,
                        &item,
                        bounds.size.width.as_f32().max(1.0),
                        bounds.size.height.as_f32().max(1.0),
                        scale_factor,
                    )),
            );
        }

        shell = layer;

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

        div()
            .absolute()
            .inset_0()
            .child(scrim)
            .child(shell)
            // Toast 在整窗层：详情卡片本身会被 shell 裁切，放里面只能贴在卡片
            // 底部；用户预期是贴近窗口底部、压在 Dock 上方的位置。
            .children(notice.map(|text| notice_chip(&theme, &text)))
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

/// 详情浮层的空间进度与「临时卡片 / 详情页」互换进度。
///
/// 前 30%：临时卡片与详情页透明度互换；后 70%：外壳缩放到目标 / 原卡片。
/// 空间进度走完整段缓动，不在前 30% 抢跑，否则详情页会在几帧内
/// 就铺满屏幕，看起来反而是「突然出现」。
fn detail_entrance_position(raw: f32) -> (f32, f32) {
    let pos = ease_entrance()(raw.clamp(0.0, 1.0));
    (pos, detail_swap(raw))
}

/// 前 30% 的透明度互换进度（平滑 0 → 1）。
fn detail_swap(raw: f32) -> f32 {
    const PHASE: f32 = 0.30;
    if raw < PHASE {
        let k = (raw / PHASE).clamp(0.0, 1.0);
        k * k * (3.0 - 2.0 * k)
    } else {
        1.0
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
#[allow(dead_code)] // 无卡片来源时的降级海报，保留给后续入口
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

/// 详情页底部的轻提示（复制成功 / 失败）。
fn notice_chip(t: &YkTheme, text: &str) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(SPACE_4))
        .flex()
        .justify_center()
        .child(status::toast_pill(t, text))
        .into_any_element()
}

/// 当前登录会话；未登录 / 旧文件损坏返回 None。
///
/// 详情页只在发评论时需要会话（cookie / CSRF / 昵称头像），
/// 构造时拿到的 `cookie` 只用于读评论。
fn current_session() -> Option<yk_domain::models::UserSession> {
    let path = yk_infra::session::session_path();
    yk_infra::session::load(&path).filter(|session| session.is_valid())
}
