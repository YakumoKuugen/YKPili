//! 播放器评论区（分页 + 窗口化渲染）的离屏验收。
//!
//! 只在 macOS / Linux 上跑；图片写到 `/tmp/ykpili-comments-*.ppm`，
//! 用 `sips -s format png` 转 PNG 后目检。
//!
//! 验收点：
//! ① 300 条评论只创建可见的十几行元素（窗口化），而不是 300 行；
//! ② 滚到中段时行与行之间没有空洞、没有重叠；
//! ③ 「没有更多评论」只在 `is_end` 为真时出现。
//!
//! 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test comments_visual`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::player::PlayerView;
use yk_core::dynamic::DynamicComment;

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

/// 造 `count` 条评论：每 5 条夹一条超长评论（正文完整展开、行高跟着重算），
/// 第 2 条带显式换行，其余 1–2 行。
fn comments(count: usize) -> Vec<DynamicComment> {
    (0..count)
        .map(|i| {
            let text = if i % 5 == 0 {
                format!(
                    "第 {i} 条评论：这是一段明显更长的正文，用来验证多行折行的高度计算与\
                     行与行之间的衔接；末尾再补一长串不带标点的正文，验证评论会完整展开、\
                     行高跟着文本重算，而不是被截断成固定三行或补省略号：{}",
                    "字".repeat(60)
                )
            } else if i == 1 {
                // 多段正文：显式换行 + 空行，行高也要按完整行数算，不能压到下一行。
                "第一段：多段评论的第一段。\n第二段：第二段文字。\n\n第四段：空行之后还有一段，整条评论的高度必须按全部行数算。".to_string()
            } else if i % 3 == 0 {
                format!("第 {i} 条评论：两行左右的长度，用来验证不同行高之间的排布是否连续。")
            } else {
                format!("第 {i} 条评论")
            };
            DynamicComment {
                rpid: i as i64 + 1,
                author: format!("用户{i}"),
                face: String::new(),
                text,
                time: format!("{} 天前", i % 30 + 1),
                like: (i as u64) % 900,
                // 每 7 条带 2 张图，顺便验收缩略图条对行高的影响。
                pictures: if i % 7 == 3 {
                    (0..2)
                        .map(|k| yk_core::dynamic::DynamicImage {
                            url: format!("https://example.invalid/{i}-{k}.jpg"),
                            width: 800,
                            height: 600,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                is_top: i == 0,
            }
        })
        .collect()
}

fn capture(
    path: &str,
    items: Vec<DynamicComment>,
    total: u64,
    is_end: bool,
    scroll: f32,
    expand_composer: bool,
) -> usize {
    let mut cx = context();
    let theme = yk_ui::theme::YkTheme::dark();
    let (handle, player) = cx
        .update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1280.), px(820.)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_ui::gpui_theme::apply(cx, &theme);
                    cx.new(|cx| {
                        PlayerView::new(theme.clone(), cx)
                            .with_expanded(true)
                            .with_comments(items, total, is_end)
                    })
                },
            )
        })
        .unwrap();
    // 先渲染一帧让布局成立。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    // 可选：先点开评论输入条，等宽度动画跑完；再拨滚动位置，
    // 让「回到顶部」把它顶起来。顺序反了会点到底下的回到顶部按钮。
    if expand_composer {
        cx.update_window(handle, |_, window, cx| {
            window.click("comment-fab", cx);
        })
        .unwrap();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(12));
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
            })
            .unwrap();
        }
    }
    cx.update(|cx| {
        player.update(cx, |view, cx| {
            view.set_test_scroll(scroll);
            cx.notify();
        });
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    if scroll != 0.0 {
        // 位移 / 宽度动画都要走真实时间，等它们跑完再截图。
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(12));
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
            })
            .unwrap();
        }
    }
    let rows = cx.update(|cx| player.read(cx).rendered_comment_rows());
    let image = cx.capture_screenshot(handle).expect("headless screenshot");
    let (width, height) = image.dimensions();
    let raw = image.into_raw();
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in raw.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, ppm).unwrap();
    eprintln!("[comments] {path} {width}x{height} rows={rows}");
    rows
}

fn main() {
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[comments] skipped（设置 YK_VISUAL=1 生成评论区截图）");
        return;
    }
    let top = capture(
        "/tmp/ykpili-comments-top.ppm",
        comments(300),
        101_806,
        false,
        0.0,
        false,
    );
    assert!(top > 0, "首屏至少要画出几行评论");
    assert!(top < 80, "窗口化渲染不该把 300 条全建出来（实际 {top} 行）");

    let middle = capture(
        "/tmp/ykpili-comments-middle.ppm",
        comments(300),
        101_806,
        false,
        -1500.0,
        false,
    );
    assert!(middle > 0 && middle < 80, "滚动后仍然只画可见行（{middle} 行）");

    // 评论输入条：收起态被顶起 / 点开后的展开态 / 滚动后被「回到顶部」顶起的姿态。
    capture(
        "/tmp/ykpili-comments-fab-scrolled.ppm",
        comments(300),
        101_806,
        false,
        -1500.0,
        false,
    );
    capture(
        "/tmp/ykpili-comments-composer.ppm",
        comments(300),
        101_806,
        false,
        0.0,
        true,
    );
    capture(
        "/tmp/ykpili-comments-composer-scrolled.ppm",
        comments(300),
        101_806,
        false,
        -1500.0,
        true,
    );

    // 少量评论 + 服务端说到头：底部必须出现「没有更多评论」。
    capture(
        "/tmp/ykpili-comments-end.ppm",
        comments(4),
        4,
        true,
        0.0,
        false,
    );
}
