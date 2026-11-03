//! 「正在关注」卡片的复制边界验收（离屏）。
//!
//! 验收点：
//! ① 卡片正文**不能**被拖选复制 —— 复制只放在打开详情后的右键菜单里；
//! ② 点正文 / 图片都打开详情，不会直接弹出图片查看器；
//! ③ 详情正文右键可复制；
//! ④ 查看器里点图片外的背景只关查看器，详情仍然保留。
//!
//! 本地验收：`cargo test -p yk-app --test following_card_copy`

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, AnyWindowHandle, App, Bounds, Entity, HeadlessAppContext, InputEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, WindowBounds,
    WindowOptions,
};
use yk_app::AppRoot;
use yk_infra::dynamic::{DynamicAuthor, DynamicImage, DynamicItem, DynamicKind};

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

fn image_item() -> DynamicItem {
    DynamicItem {
        index: 0,
        id: "dynamic-card-image-probe".into(),
        kind: DynamicKind::Images,
        author: DynamicAuthor {
            mid: 1,
            name: "测试 UP".into(),
            face: String::new(),
        },
        pub_ts: 1_793_000_000,
        pub_time: "1 小时前".into(),
        action: "发布了动态".into(),
        title: "图文卡片".into(),
        text: "点图片应该先进详情，而不是直接弹出图片查看器。".into(),
        images: vec![DynamicImage {
            url: "https://example.invalid/card-image.jpg".into(),
            width: 1000,
            height: 1000,
        }],
        video: None,
        link: None,
        stats: Default::default(),
        comment_target: None,
        origin: None,
        placeholder_color: "#445566".into(),
    }
}

fn open_app(cx: &mut HeadlessAppContext, item: DynamicItem) -> (AnyWindowHandle, Entity<AppRoot>) {
    let theme = yk_theme::theme::YkTheme::dark();
    cx.update(|cx| {
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
                yk_theme::gpui_theme::apply(cx, &theme);
                cx.new(|_| {
                    AppRoot::new()
                        .without_login_prompt()
                        .with_signed_in()
                        .with_following(vec![item])
                })
            },
        )
    })
    .unwrap()
}

fn dispatch_click(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    position: Point<Pixels>,
    button: MouseButton,
) {
    cx.update_window(handle, |_, window, cx| {
        let modifiers = window.modifiers();
        window.dispatch_event(
            MouseDownEvent {
                button,
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
                button,
                position,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
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

fn render(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
}

fn detail_body_center(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) -> Point<Pixels> {
    cx.update_window(handle, |_, window, _| {
        window.find("dynamic-detail-body").bounds().center()
    })
    .unwrap()
}

fn right_click_body_and_copy(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) -> String {
    let center = detail_body_center(cx, handle);
    dispatch_click(cx, handle, center, MouseButton::Right);
    // 菜单锚在右键位置，首项在菜单内边距之后。
    dispatch_click(
        cx,
        handle,
        center + point(px(64.), px(17.)),
        MouseButton::Left,
    );
    cx.update(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("右键菜单必须能把正文写进剪贴板")
}

fn main() {
    let mut cx = context();

    // ① 纯文字卡：不能拖选，点击开详情，详情正文可右键复制。
    let (handle, app) = open_app(&mut cx, item());
    render(&mut cx, handle);

    let from = point(px(52.), px(205.));
    let to = point(px(300.), px(205.));
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

    dispatch_click(&mut cx, handle, from, MouseButton::Left);
    assert!(
        cx.update(|cx| app.read(cx).test_dynamic_detail_open()),
        "点击卡片正文应当打开详情浮层"
    );
    // 等入场动画走完，详情页才落到可交互的终态。
    std::thread::sleep(Duration::from_millis(350));
    render(&mut cx, handle);
    let copied = right_click_body_and_copy(&mut cx, handle);
    assert!(
        copied.contains("卡片只做摘要浏览"),
        "详情正文右键复制了错误内容：{copied:?}"
    );

    // ② 图文卡：点图片先进详情；详情里点大图才开查看器；
    //    查看器背景点击只关查看器，详情保留。
    let (handle, app) = open_app(&mut cx, image_item());
    render(&mut cx, handle);
    // 图片区域在图文中段（卡片第一列，标题 / 正文下方）。
    dispatch_click(
        &mut cx,
        handle,
        point(px(200.), px(450.)),
        MouseButton::Left,
    );
    assert!(
        cx.update(|cx| app.read(cx).test_dynamic_detail_open()),
        "点击卡片的图片区域应当先打开详情"
    );
    assert!(
        !cx.update(|cx| app.read(cx).test_image_viewer_open(cx)),
        "点卡片图片不应该直接弹出图片查看器"
    );
    std::thread::sleep(Duration::from_millis(350));
    render(&mut cx, handle);

    // 详情左侧媒体列中心：点大图打开查看器。
    dispatch_click(
        &mut cx,
        handle,
        point(px(390.), px(400.)),
        MouseButton::Left,
    );
    assert!(
        cx.update(|cx| app.read(cx).test_image_viewer_open(cx)),
        "详情里点大图应当打开图片查看器"
    );
    assert!(
        cx.update_window(handle, |_, window, _| {
            window.try_find("image-viewer-copy").is_some()
        })
        .unwrap(),
        "图片查看器右下角应当有复制按钮"
    );

    // 查看器图片占屏幕 86%，左上角 (20, 20) 落在背景上。
    dispatch_click(&mut cx, handle, point(px(20.), px(20.)), MouseButton::Left);
    assert!(
        !cx.update(|cx| app.read(cx).test_image_viewer_open(cx)),
        "点查看器背景应当关闭查看器"
    );
    assert!(
        cx.update(|cx| app.read(cx).test_dynamic_detail_open()),
        "点查看器背景不应该把详情浮层一起关掉"
    );

    // 图文详情的右侧正文同样可复制。
    let copied = right_click_body_and_copy(&mut cx, handle);
    assert!(
        copied.contains("点图片应该先进详情"),
        "图文详情正文右键复制了错误内容：{copied:?}"
    );

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

    eprintln!("[following-card] 卡片不可拖选、图片先进详情、查看器不再误关详情、正文可复制");
}
