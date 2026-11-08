//! 「正在关注」转发卡片里引用视频的验收（离屏）。
//!
//! 验收点：
//! ① 原动态是视频时，引用块画的是 16:9 视频封面，而不是一行「视频：标题」；
//! ② 点引用块直接进播放器，不会同时叠一层详情浮层；
//! ③ `YK_VISUAL=1` 时把截图写到 `target/visual/ykpili-following-quote-video.ppm` 目检。
//!
//! 本地验收：`cargo test -p yk-app --test following_quote_video`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, AnyWindowHandle, Bounds, HeadlessAppContext, InputEvent, MouseButton,
    MouseDownEvent, MouseUpEvent, Pixels, Point, WindowBounds, WindowOptions,
};
use yk_app::AppRoot;
use yk_infra::dynamic::{DynamicAuthor, DynamicItem, DynamicKind, DynamicStats, DynamicVideo};

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;
const CARD_INDEX: usize = 3;

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn video_origin() -> DynamicItem {
    DynamicItem {
        index: CARD_INDEX,
        id: "dynamic-origin-video".into(),
        kind: DynamicKind::Video,
        author: DynamicAuthor {
            mid: 2,
            name: "视频作者".into(),
            face: String::new(),
        },
        pub_ts: 1_793_000_000,
        pub_time: "2 小时前".into(),
        action: "投稿了视频".into(),
        title: "被引用的视频标题".into(),
        text: String::new(),
        images: Vec::new(),
        video: Some(DynamicVideo {
            bvid: "BV1xx411c7mD".into(),
            title: "被引用的视频：一条足够长的标题，用来看封面上的标题裁切".into(),
            cover: String::new(),
            desc: String::new(),
            duration_text: "12:34".into(),
            play_count: 12_345,
            danmaku_count: 678,
        }),
        link: None,
        stats: DynamicStats::default(),
        comment_target: None,
        origin: None,
        placeholder_color: "#334455".into(),
    }
}

fn forward_item() -> DynamicItem {
    DynamicItem {
        index: CARD_INDEX,
        id: "dynamic-forward-video".into(),
        kind: DynamicKind::Forward,
        author: DynamicAuthor {
            mid: 1,
            name: "转发的人".into(),
            face: String::new(),
        },
        pub_ts: 1_793_000_000,
        pub_time: "1 小时前".into(),
        action: "转发了动态".into(),
        title: String::new(),
        text: "看这个视频".into(),
        images: Vec::new(),
        video: None,
        link: None,
        stats: Default::default(),
        comment_target: None,
        origin: Some(Box::new(video_origin())),
        placeholder_color: "#445566".into(),
    }
}

fn open_app(
    cx: &mut HeadlessAppContext,
    item: DynamicItem,
) -> (AnyWindowHandle, gpui::Entity<AppRoot>) {
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

fn render(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
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

fn capture(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, path: &str) {
    let Ok(image) = cx.capture_screenshot(handle) else {
        eprintln!("[following-quote-video] 当前平台没有离屏渲染器，跳过截图 {path}");
        return;
    };
    let (width, height) = image.dimensions();
    let raw = image.into_raw();
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in raw.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, ppm).unwrap();
    eprintln!("[following-quote-video] {path} {width}x{height}");
}

fn main() {
    let mut cx = context();
    let (handle, app) = open_app(&mut cx, forward_item());
    render(&mut cx, handle);

    let quote_bounds = cx
        .update_window(handle, |_, window, _| {
            window
                .try_find(("dynamic-quote", CARD_INDEX))
                .expect("引用块应当有稳定 id，便于定位 / 验收")
                .bounds()
        })
        .unwrap();
    let quote_w = quote_bounds.size.width.as_f32();
    let quote_h = quote_bounds.size.height.as_f32();
    eprintln!("[following-quote-video] quote={quote_bounds:?}");

    // 纯文字引用块高约 82（两行摘要）；视频封面固定 16:9，必然明显更高。
    let cover_h = (quote_w - 20.0) * 9.0 / 16.0;
    assert!(
        quote_h > 160.0,
        "引用视频应当直接画封面，引用块高度 {quote_h} 太小，像是仍只有一行文字"
    );
    assert!(
        (quote_h - (38.0 + 2.0 + cover_h)).abs() < 2.0,
        "引用块高度 {quote_h} 与「作者行 + 16:9 封面」的估算（约 {}）不一致",
        38.0 + 2.0 + cover_h
    );

    if std::env::var("YK_VISUAL").is_ok() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/target/visual");
        let _ = fs::create_dir_all(dir);
        capture(
            &mut cx,
            handle,
            &format!("{dir}/ykpili-following-quote-video.ppm"),
        );
    }

    // 点引用块（封面）→ 直接进播放器，不能先叠一层详情。
    dispatch_click(
        &mut cx,
        handle,
        quote_bounds.center(),
        MouseButton::Left,
    );
    assert!(
        cx.update(|cx| app.read(cx).test_player_open()),
        "点引用视频的封面应当打开播放器"
    );
    assert!(
        !cx.update(|cx| app.read(cx).test_dynamic_detail_open()),
        "点引用视频的封面不应该同时打开详情浮层"
    );

    // ② 点卡片其它区域仍进详情；详情入场动画里的占位卡也要能画引用封面。
    let (handle, app) = open_app(&mut cx, forward_item());
    render(&mut cx, handle);
    dispatch_click(&mut cx, handle, point(px(120.), px(85.)), MouseButton::Left);
    assert!(
        cx.update(|cx| app.read(cx).test_dynamic_detail_open()),
        "点卡片正文区域应当打开详情浮层"
    );
    // 入场动画前 30% 由占位卡（ghost）撑着，多画几帧确认不会 panic。
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(60));
        render(&mut cx, handle);
    }

    eprintln!("[following-quote-video] 引用视频画封面、点封面进播放器且不叠详情");
}
