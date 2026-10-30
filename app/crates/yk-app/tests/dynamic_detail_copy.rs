//! 动态详情浮层的复制能力验收（离屏）。
//!
//! 验收点：
//! ① 正文右键会弹出上下文菜单，菜单里的「复制文本」把该段正文写进剪贴板；
//! ② 左侧图片右下角有半透明复制按钮，点击后剪贴板里是图片条目；
//! ③ （可选）`YK_VISUAL=1` 时把右键菜单截图写到临时目录，便于目检。
//!
//! 本地验收：`cargo test -p yk-app --test dynamic_detail_copy`

use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    px, size, App, Bounds, ClipboardEntry, HeadlessAppContext, InputEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, WindowBounds,
    WindowOptions,
};
use yk_app::dynamic_view::DynamicDetailView;
use yk_core::dynamic::{DynamicAuthor, DynamicImage, DynamicItem, DynamicKind};

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn open_detail(
    cx: &mut HeadlessAppContext,
    item: DynamicItem,
) -> (gpui::AnyWindowHandle, gpui::Entity<DynamicDetailView>) {
    let theme = yk_ui::theme::YkTheme::dark();
    cx.update(|cx| {
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
                yk_ui::gpui_theme::apply(cx, &theme);
                let theme = theme.clone();
                cx.new(move |cx| DynamicDetailView::new(theme, item, None, String::new(), cx))
            },
        )
    })
    .unwrap()
}

fn capture(cx: &mut HeadlessAppContext, window: gpui::AnyWindowHandle, path: &str) {
    let Ok(image) = cx.capture_screenshot(window) else {
        eprintln!("[dynamic-copy] 当前平台没有离屏渲染器，跳过截图 {path}");
        return;
    };
    let (width, height) = image.dimensions();
    let raw = image.into_raw();
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in raw.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, ppm).unwrap();
    eprintln!("[dynamic-copy] {path} {width}x{height}");
}

/// 菜单项没有稳定的 ElementId，按窗口坐标点一下。
fn click_at(window: &mut Window, position: Point<Pixels>, button: MouseButton, cx: &mut App) {
    let modifiers = window.modifiers();
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
}

fn body_item(text: &str) -> DynamicItem {
    DynamicItem {
        kind: DynamicKind::Text,
        author: DynamicAuthor {
            mid: 1,
            name: "测试 UP".into(),
            face: String::new(),
        },
        text: text.into(),
        ..Default::default()
    }
}

/// 1×1 PNG（只要求文件头能被识别；剪贴板写入不需要解码）。
fn png_bytes() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    // IHDR chunk: 1×1 RGBA.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes.extend_from_slice(&[0x1F, 0x15, 0xD1, 0x9B]);
    // IEND.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
    bytes
}

fn main() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let cache_dir = std::env::temp_dir().join(format!(
        "ykpili-dynamic-copy-{}-{stamp}",
        std::process::id()
    ));
    // SAFETY: 测试进程在这里先设置环境变量，随后才第一次访问进程级图片缓存。
    std::env::set_var("YK_IMAGE_CACHE_DIR", &cache_dir);

    let body = "这是一段用于验证右键复制功能的动态正文。";
    let mut cx = context();
    let (handle, _view) = open_detail(&mut cx, body_item(body));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    // 详情浮层有 280ms 入场动画；动画用真实 Instant 计时，等一拍再渲染到终态，
    // 否则外壳仍缩在原卡片位置、图片右下角的按钮会被裁掉。
    std::thread::sleep(std::time::Duration::from_millis(350));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();

    // ① 正文右键 → 菜单 → 复制文本。
    let body_center = cx
        .update_window(handle, |_, window, _| {
            window.find("dynamic-detail-body").bounds().center()
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        click_at(window, body_center, MouseButton::Right, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    if std::env::var("YK_VISUAL").is_ok() {
        capture(&mut cx, handle, "/tmp/ykpili-dynamic-context-menu.ppm");
    }

    // 菜单锚在右键位置、默认 TopLeft；首项在菜单内边距之后。
    cx.update_window(handle, |_, window, cx| {
        click_at(
            window,
            body_center + gpui::point(px(64.), px(17.)),
            MouseButton::Left,
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let copied = cx
        .update(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some(body),
        "右键菜单的「复制文本」必须把正文写进剪贴板"
    );

    // ② 图片右下角的复制按钮 → 图片条目。
    let image = DynamicImage {
        url: "https://i0.hdslb.com/bfs/new_dyn/ykpili-copy-test.png".into(),
        width: 1448,
        height: 1086,
    };
    let item = DynamicItem {
        kind: DynamicKind::Images,
        author: DynamicAuthor {
            mid: 1,
            name: "测试 UP".into(),
            face: String::new(),
        },
        text: "图片动态".into(),
        images: vec![image.clone()],
        ..Default::default()
    };
    let (handle, _view) = open_detail(&mut cx, item);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(350));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();

    let scale_factor = cx
        .update_window(handle, |_, window, _| window.scale_factor())
        .unwrap();
    let media_w = 1120.0_f32 * 0.56;
    let fitted = image.fit_url(media_w, scale_factor);
    let cache = yk_core::ImageDiskCache::new(&cache_dir, 16 * 1024 * 1024);
    cache.put(&fitted, &png_bytes());

    cx.update_window(handle, |_, window, cx| {
        window.click("dynamic-image-copy", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    if std::env::var("YK_VISUAL").is_ok() {
        capture(&mut cx, handle, "/tmp/ykpili-dynamic-image-copy.ppm");
    }

    let item = cx.update(|cx| cx.read_from_clipboard());
    let item = item.expect("复制图片后剪贴板不应为空");
    assert!(
        matches!(item.entries().first(), Some(ClipboardEntry::Image(_))),
        "图片复制按钮必须写入图片剪贴板条目"
    );

    let _ = fs::remove_dir_all(&cache_dir);
}
