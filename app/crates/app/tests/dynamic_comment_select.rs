//! 动态详情评论区拖选文本的回归验收（与播放器评论区同一套 `comment_row`）。
//!
//! 本地验收：`cargo test -p yk-app --test dynamic_comment_select`

use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, App, Bounds, HeadlessAppContext, InputEvent as _, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, WindowBounds,
    WindowOptions,
};
use yk_app::dynamic_view::DynamicDetailView;
use yk_infra::dynamic::{DynamicAuthor, DynamicComment, DynamicItem, DynamicKind};

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn drag(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, cx: &mut App) {
    let modifiers = Modifiers::default();
    window.dispatch_event(
        MouseMoveEvent {
            position: from,
            modifiers,
            pressed_button: None,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
    let steps = 8;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        window.dispatch_event(
            MouseMoveEvent {
                position: point(
                    px(from.x.as_f32() + (to.x.as_f32() - from.x.as_f32()) * t),
                    px(from.y.as_f32() + (to.y.as_f32() - from.y.as_f32()) * t),
                ),
                modifiers,
                pressed_button: Some(MouseButton::Left),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    }
    window.dispatch_event(
        MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            modifiers,
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

fn main() {
    let mut cx = context();
    // 带图动态：评论在 `article_column` 的滚动容器里（和纯文字模式不同的一条路径）。
    let item = DynamicItem {
        kind: DynamicKind::Images,
        author: DynamicAuthor {
            mid: 1,
            name: "测试 UP".into(),
            face: String::new(),
        },
        text: "正文".into(),
        images: vec![yk_infra::dynamic::DynamicImage {
            url: "https://i0.hdslb.com/bfs/new_dyn/ykpili-select-test.png".into(),
            width: 1448,
            height: 1086,
        }],
        ..Default::default()
    };
    let theme = yk_theme::theme::YkTheme::dark();
    let (handle, view) = cx
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
                move |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_theme::gpui_theme::apply(cx, &theme);
                    let theme = theme.clone();
                    cx.new(move |cx| DynamicDetailView::new(theme, item, None, String::new(), cx))
                },
            )
        })
        .unwrap();

    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    // 入场动画 280ms：等它跑完再取几何，否则外壳还缩在原卡片位置。
    std::thread::sleep(std::time::Duration::from_millis(350));
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();

    let comment = DynamicComment {
        rpid: 4242,
        author: "评论用户".into(),
        text: "这是一条足够长的评论正文，用来验证左键拖动可以选中其中的文字。".into(),
        ..Default::default()
    };
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_test_comments(vec![comment.clone()]);
            cx.notify();
        });
    });
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();

    let bounds = cx
        .update_window(handle, |_, window, _| {
            window
                .find(gpui::ElementId::Name(
                    format!("dynamic-comment-text-{}", comment.rpid).into(),
                ))
                .bounds()
        })
        .unwrap();
    eprintln!("[test] 评论文本 bounds = {bounds:?}");

    // 单击（不拖动）不应该留下选区 —— 这是和播放器评论区对齐的行为。
    let click_at = point(
        px(bounds.origin.x.as_f32() + 6.0),
        px(bounds.origin.y.as_f32() + bounds.size.height.as_f32() / 2.0),
    );
    cx.update_window(handle, |_, window, cx| {
        let modifiers = Modifiers::default();
        window.dispatch_event(
            MouseMoveEvent {
                position: click_at,
                modifiers,
                pressed_button: None,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: click_at,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: click_at,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    let clicked = cx
        .update_window(handle, |_, window, cx| {
            gpui::base::TextSelection::selected_text(window, cx)
        })
        .unwrap();
    println!("单击后的选区：{clicked:?}");
    assert!(clicked.trim().is_empty(), "单击不应选中评论文字");

    let start = point(
        px(bounds.origin.x.as_f32() + 6.0),
        px(bounds.origin.y.as_f32() + bounds.size.height.as_f32() / 2.0),
    );
    let end = point(px(start.x.as_f32() + 120.0), start.y);
    cx.update_window(handle, |_, window, cx| {
        drag(window, start, end, cx);
    })
    .unwrap();

    let selected = cx
        .update_window(handle, |_, window, cx| {
            gpui::base::TextSelection::selected_text(window, cx)
        })
        .unwrap();
    println!("选中文本：{selected:?}");
    assert!(
        !selected.trim().is_empty(),
        "动态详情评论区左键拖动应当选中文字"
    );

    // 清空选区后，在两个不同位置各单击一次：不应把中间的文字选中。
    cx.update_window(handle, |_, window, cx| {
        gpui::base::TextSelection::clear(window, cx);
        let modifiers = Modifiers::default();
        for x in [
            bounds.origin.x.as_f32() + 8.0,
            bounds.origin.x.as_f32() + 200.0,
        ] {
            let position = point(px(x), px(start.y.as_f32()));
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    modifiers,
                    pressed_button: None,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        }
    })
    .unwrap();
    // 输入条：点 FAB 展开，点外面要能收起（与播放器同一个组件）。
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_test_comment_target(yk_infra::dynamic::CommentTarget {
                oid: "1".into(),
                kind: 11,
            });
            cx.notify();
        });
    });
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.update_window(handle, |_, window, cx| window.click("comment-fab", cx))
        .unwrap();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let expanded = cx
        .update_window(handle, |_, window, _| {
            window.try_find("comment-send").is_some()
        })
        .unwrap();
    assert!(expanded, "点圆按钮后应展开输入条");
    cx.update_window(handle, |_, window, cx| {
        window.click("dynamic-detail-body", cx)
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let collapsed = cx
        .update_window(handle, |_, window, _| {
            window.try_find("comment-send").is_none()
        })
        .unwrap();
    assert!(collapsed, "点输入条外面应收起输入条");

    let after_two_clicks = cx
        .update_window(handle, |_, window, cx| {
            gpui::base::TextSelection::selected_text(window, cx)
        })
        .unwrap();
    println!("两次单击后的选区：{after_two_clicks:?}");
    assert!(
        after_two_clicks.trim().is_empty(),
        "两次独立单击不应选中中间的文字"
    );
}
