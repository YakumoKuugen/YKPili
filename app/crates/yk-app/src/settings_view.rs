//! 设置浮层：从个人面板的「设置」进入，负责缓存查看与清理。
//!
//! 目前管理两类缓存：
//! - 图片磁盘缓存：`~/Library/Caches/YKPili/images`，存压缩图，清掉后滚动会重新下载；
//! - 推荐流缓存：首页离线用的 `recommend.json`。
//!
//! 内存中的页面图片缓存不在这里清：它只保留可视 + 一行，滚动时会自动换出，
//! 点击清理后如果立刻清空反而会让当前屏幕的卡片重新请求网络。

use gpui::prelude::*;
use gpui::*;
use yk_core::feed_cache::{self, FeedCache};
use yk_ui::theme::*;

use crate::http::shared_image_cache;

const SETTINGS_W: f32 = 560.0;
const SETTINGS_H: f32 = 380.0;
const SETTINGS_OPEN_MS: u64 = 180;

pub struct SettingsView {
    theme: YkTheme,
    disk_stats: (usize, u64),
    disk_max: u64,
    feed_stats: (u64, Option<u64>),
    message: Option<(String, bool)>,
    close_handler: Option<Box<dyn Fn(&mut App)>>,
    focus_handle: FocusHandle,
    needs_focus: bool,
}

impl SettingsView {
    pub fn new(theme: YkTheme, cx: &mut Context<Self>) -> Self {
        let cache = shared_image_cache();
        let mut view = Self {
            theme,
            disk_stats: cache.stats(),
            disk_max: cache.max_bytes(),
            feed_stats: FeedCache::new().stats(),
            message: None,
            close_handler: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            needs_focus: true,
        };
        view.refresh_stats();
        view
    }

    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App) + 'static) {
        self.close_handler = Some(Box::new(handler));
    }

    fn refresh_stats(&mut self) {
        let cache = shared_image_cache();
        self.disk_stats = cache.stats();
        self.disk_max = cache.max_bytes();
        self.feed_stats = FeedCache::new().stats();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(handler) = self.close_handler.as_ref() {
            handler(cx);
        }
    }

    fn clear_images(&mut self, cx: &mut Context<Self>) {
        self.message = Some(match shared_image_cache().clear() {
            Ok(()) => ("图片磁盘缓存已清理".to_string(), false),
            Err(e) => (format!("图片缓存清理失败：{e}"), true),
        });
        self.refresh_stats();
        cx.notify();
    }

    fn clear_feed(&mut self, cx: &mut Context<Self>) {
        self.message = Some(match FeedCache::new().clear() {
            Ok(()) => ("推荐流缓存已清理".to_string(), false),
            Err(e) => (format!("推荐流缓存清理失败：{e}"), true),
        });
        self.refresh_stats();
        cx.notify();
    }

    fn clear_all(&mut self, cx: &mut Context<Self>) {
        let image = shared_image_cache().clear();
        let feed = FeedCache::new().clear();
        self.message = Some(match (image, feed) {
            (Ok(()), Ok(())) => ("图片与推荐流缓存已清理".to_string(), false),
            (Err(e), _) => (format!("图片缓存清理失败：{e}"), true),
            (_, Err(e)) => (format!("推荐流缓存清理失败：{e}"), true),
        });
        self.refresh_stats();
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.focus_handle.clone();
            cx.defer_in(window, move |_view, window, cx| {
                window.focus(&handle, cx);
            });
        }

        let t = self.theme.clone();
        let s = &t.surfaces;
        let p = &t.palette;
        let viewport = window.viewport_size();
        let width = SETTINGS_W.min(viewport.width.as_f32() - SPACE_8 * 2.0);
        let height = SETTINGS_H.min(viewport.height.as_f32() - SPACE_8 * 2.0);
        let left = (viewport.width.as_f32() - width) / 2.0;
        let top = (viewport.height.as_f32() - height) / 2.0;

        let disk_count = self.disk_stats.0;
        let disk_bytes = self.disk_stats.1;
        let disk_max = self.disk_max;
        let (feed_bytes, feed_updated) = self.feed_stats;

        let close = cx.listener(|view, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            view.close(cx);
        });
        let clear_images = cx.listener(|view, _: &ClickEvent, _window, cx| {
            view.clear_images(cx);
        });
        let clear_feed = cx.listener(|view, _: &ClickEvent, _window, cx| {
            view.clear_feed(cx);
        });
        let clear_all = cx.listener(|view, _: &ClickEvent, _window, cx| {
            view.clear_all(cx);
        });
        let escape = cx.listener(|view, event: &KeyDownEvent, _window, cx| {
            if event.keystroke.key == "escape" {
                view.close(cx);
                cx.stop_propagation();
            }
        });

        let panel = div()
            .id("settings-panel")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(height))
            .rounded(px(MODAL_RADIUS))
            .overflow_hidden()
            .bg(s.surface_content_solid)
            .border_1()
            .border_color(s.border_popover)
            .shadow(t.shadows.shadow_glass_modal.clone())
            .occlude()
            .key_context("Settings")
            .track_focus(&self.focus_handle)
            .on_key_down(escape)
            .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            // 顶部标题 + 关闭
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .px(px(SPACE_6))
                    .pt(px(SPACE_5))
                    .pb(px(SPACE_3))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(SPACE_1))
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_HEADING))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(s.text_primary)
                                    .child("设置"),
                            )
                            .child(
                                div()
                                    .text_size(px(FONT_SIZE_CAPTION))
                                    .text_color(s.text_tertiary)
                                    .child("缓存管理"),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-close")
                            .w(px(30.))
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(s.fill_1)
                            .text_size(px(18.))
                            .text_color(s.text_secondary)
                            .cursor_pointer()
                            .hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
                            .child("×")
                            .on_click(close),
                    ),
            )
            .child(div().mx(px(SPACE_6)).h(px(1.)).bg(s.border_subtle))
            // 图片缓存
            .child(cache_row(
                &t,
                "图片缓存",
                "封面 / 头像 / 动态图的压缩文件",
                format!("{disk_count} 个文件 · {} / {}", size_text(disk_bytes), size_text(disk_max)),
                Some(shared_image_cache().root().display().to_string()),
                "清理图片",
                clear_images,
            ))
            // 推荐流缓存
            .child(cache_row(
                &t,
                "推荐流缓存",
                "首页离线时使用的推荐数据",
                if feed_bytes == 0 {
                    "无缓存".to_string()
                } else {
                    match feed_updated {
                        Some(ts) => format!("{} · {}", size_text(feed_bytes), feed_cache::age_text(ts)),
                        None => size_text(feed_bytes),
                    }
                },
                None,
                "清理推荐流",
                clear_feed,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .px(px(SPACE_6))
                    .pb(px(SPACE_3))
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap(px(SPACE_2))
                    .when_some(self.message.clone(), |el, (message, is_error)| {
                        el.child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(if is_error {
                                    p.status_error
                                } else {
                                    p.status_success
                                })
                                .child(message),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_MICRO))
                            .line_height(px(16.))
                            .text_color(s.text_quaternary)
                            .child("清理图片缓存后，滚出屏幕的封面会重新下载；当前屏幕已显示的图片仍保留在内存中。"),
                    ),
            )
            .child(
                div()
                    .px(px(SPACE_6))
                    .pb(px(SPACE_5))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("settings-clear-all")
                            .h(px(34.))
                            .px(px(SPACE_4))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(p.status_error_20)
                            .border_1()
                            .border_color(p.status_error_30)
                            .text_size(px(FONT_SIZE_CAPTION))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.status_error)
                            .cursor_pointer()
                            .hover(|style| style.bg(p.status_error_30))
                            .child("清理全部缓存")
                            .on_click(clear_all),
                    ),
            );

        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("settings-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x000000B3))
                    .occlude()
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| view.close(cx))),
            )
            .child(panel.with_animation(
                "settings-open",
                Animation::new(std::time::Duration::from_millis(SETTINGS_OPEN_MS)),
                |el, progress| el.opacity(ease_standard()(progress)),
            ))
    }
}

fn size_text(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn cache_row(
    t: &YkTheme,
    title: &str,
    desc: &str,
    stats: String,
    path: Option<String>,
    action: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_4))
        .px(px(SPACE_6))
        .py(px(SPACE_4))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_2))
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CONTROL))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(s.text_primary)
                                .child(title.to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(s.text_quaternary)
                                .child(stats),
                        ),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(desc.to_string()),
                )
                .when_some(path, |el, path| {
                    el.child(
                        div()
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(s.text_quaternary)
                            .truncate()
                            .child(path),
                    )
                }),
        )
        .child(
            div()
                .id(action)
                .h(px(30.))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .rounded_full()
                .bg(s.fill_1)
                .border_1()
                .border_color(s.border_subtle)
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
                .child(action.to_string())
                .on_click(on_click),
        )
        .border_b_1()
        .border_color(s.border_subtle)
}
