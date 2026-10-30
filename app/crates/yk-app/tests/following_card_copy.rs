//! 「正在关注」卡片的复制边界验收（离屏）。
//!
//! 验收点：
//! ① 卡片正文**不能**被拖选复制 —— 复制只放在打开详情后的右键菜单里；
//! ② 点正文仍然会打开详情（卡片本身是进详情的入口）。
//!
//! 本地验收：`cargo test -p yk-app --test following_card_copy`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    px, size, App, Bounds, HeadlessAppContext, InputEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, WindowBounds, WindowOptions,
};
use yk_app::AppRoot;
use yk_core::dynamic::{DynamicAuthor, DynamicItem, DynamicKind};

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn item() -> DynamicItem {
    DynamicItem {
        index: 0,
        id: "dynamic-card-copy-probe".into(),
        kind: DynamicKind::Text,
        author: DynamicAuthor {
            mid: 1,
            name: "测试 UP".into(),
            face: String::new(),
        },
        pub_ts: 1_793_000_000,
        pub_time: "1 小时前".into(),
        action: "发布了动态".into(),
        title: "这是一条用于验证卡片不可复制的动态标题".into(),
        text: "卡片只做摘要浏览，文字不应该能被拖动选中；复制统一放到详情浮层里。".into(),
        images: Vec::new(),
        video: None,
        link: None,
        stats: Default::default(),
        comment_target: None,
        origin: None,
        placeholder_color: "#445566".into(),
    }
}

fn drag(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, cx: &mut App) {
    let modifiers = window.modifiers();
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
    for step in 1..=4 {
        let t = step as f32 / 4.0;
        let position = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        window.dispatch_event(
            MouseMoveEvent {
                position,
                modifiers,
                pressed_button: Some(MouseButton::Left),
            }
            .to_platform_input(),
            cx,
        );
    }
    window.render_frame(cx);
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
    let theme = yk_ui::theme::YkTheme::dark();
    let (handle, app) = cx
        .update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(WINDOW_W), px(WINDOW_H)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_ui::gpui_theme::apply(cx, &theme);
                    cx.new(|_| {
                        AppRoot::new()
                            .without_login_prompt()
                            .with_signed_in()
                            .with_following(vec![item()])
                    })
                },
            )
        })
        .unwrap();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();

    if std::env::var("YK_VISUAL").is_ok() {
        if let Ok(image) = cx.capture_screenshot(handle) {
            let (width, height) = image.dimensions();
            let raw = image.into_raw();
            let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
            for pixel in raw.chunks_exact(4) {
                ppm.extend_from_slice(&pixel[..3]);
            }
            fs::write("/tmp/ykpili-following-card.ppm", ppm).unwrap();
            eprintln!("[following-card] /tmp/ykpili-following-card.ppm {width}x{height}");
        }
    }

    // 首列第一张卡：内容区左 padding 24 + 卡片 padding 14，
    // 卡头 40 + 内边距 14 + 顶栏 / 页头，标题落在 y≈205。
    let from = Point::new(px(52.), px(205.));
    let to = Point::new(px(300.), px(205.));
    cx.update_window(handle, |_, window, cx| {
        drag(window, from, to, cx);
    })
    .unwrap();

    let selected = cx
        .update_window(handle, |_, window, cx| {
            gpui::base::TextSelection::selected_text(window, cx)
        })
        .unwrap();
    assert!(
        selected.trim().is_empty(),
        "关注页卡片不应该允许拖选复制，实际选中：{selected:?}"
    );

    // 正文现在是普通文本，点击应当照常落到卡片上打开详情。
    cx.update_window(handle, |_, window, cx| {
        let modifiers = window.modifiers();
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
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: from,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    let opened = cx.update(|cx| app.read(cx).test_dynamic_detail_open());
    assert!(opened, "点击卡片正文应当打开详情浮层");
    eprintln!("[following-card] 卡片不可拖选，点击仍可打开详情");
}
