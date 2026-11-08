//! 左下角悬浮文件夹图标的离屏渲染验收。
//!
//! 只在 macOS / Linux 上跑；图片写到 `/tmp/ykpili-folder-rest.ppm` 与
//! `/tmp/ykpili-folder-hover.ppm`，用 `sips -s format png` 转 PNG 后目检：
//! 静止态应该只看到三张卡片的头，hover 态卡片扇形上浮、盖板下沉。
//!
//! 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test folder_icon_visual`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::AppRoot;

fn capture(app: impl FnOnce() -> AppRoot, path: &str) {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    let (handle, _) = cx
        .update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1280.), px(819.)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                    cx.new(|_| app())
                },
            )
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    let image = cx.capture_screenshot(handle).expect("headless screenshot");
    let (width, height) = image.dimensions();
    let raw = image.into_raw();
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in raw.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, ppm).unwrap();
    eprintln!("[visual] {path} {width}x{height}");
}

fn main() {
    // 默认跳过：headless 渲染需要 GPU，CI 或没有图形后端的环境不该因为截图失败而红。
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[folder] skipped（设置 YK_VISUAL=1 生成文件夹图标截图）");
        return;
    }
    // Headless 渲染要求平台对象在主线程创建，因此走 `harness = false`。
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        capture(
            || AppRoot::new().without_login_prompt(),
            "/tmp/ykpili-folder-rest.ppm",
        );
        capture(
            || {
                AppRoot::new()
                    .without_login_prompt()
                    .with_folder_hovered(true)
            },
            "/tmp/ykpili-folder-hover.ppm",
        );
    }
}
