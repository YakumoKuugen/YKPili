//! 播放浮窗信息面板的离屏渲染验收：互动区默认态 + 分享面板展开态。
//!
//! 只在 macOS / Linux 上跑；GPUI 的 headless renderer 需要 Metal / WGPU。
//! 图片写到 `target/visual/ykpili-player-*.ppm`（Windows 没有 `/tmp`），
//! macOS 用 `sips -s format png` 转 PNG 后目检。
//!
//! 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test player_visual`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::player::PlayerView;
use yk_app::AppRoot;

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn capture(
    path: &str,
    width: f32,
    height: f32,
    open_share: bool,
    open_coin: bool,
    open_fav: bool,
    expanded: bool,
    menu: u8,
) {
    let mut cx = context();
    let theme = yk_theme::theme::YkTheme::dark();
    let (handle, _) = cx
        .update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(width), px(height)),
                    })),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_theme::gpui_theme::apply(cx, &theme);
                    let player = cx.new(|cx| {
                        let mut player = PlayerView::new(theme.clone(), cx)
                            .with_open_panels(open_share, open_coin, open_fav)
                            .with_expanded(expanded);
                        match menu {
                            1 => player = player.with_quality_menu(),
                            2 => player = player.with_speed_menu(),
                            3 => player = player.with_fullscreen_layout(),
                            _ => {}
                        }
                        player
                    });
                    let app = AppRoot::new().without_login_prompt().with_player(player);
                    cx.new(|_| app)
                },
            )
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        // 控制栏在失焦窗口里会被主动隐藏；截图验收需要它可见，先把窗口激活。
        window.activate_window();
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
    // 输出目录跟着仓库走：Windows 上没有 `/tmp`，用 `target/visual` 两边都能读。
    let dir = std::path::Path::new("target/visual");
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(path);
    fs::write(&path, ppm).unwrap();
    eprintln!("[player] {} {width}x{height}", path.display());
}

fn main() {
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[player] skipped（设置 YK_VISUAL=1 生成播放器面板截图）");
        return;
    }
    capture(
        "ykpili-player-share.ppm",
        1280.0,
        820.0,
        true,
        false,
        false,
        false,
        0,
    );
    capture(
        "ykpili-player-coin.ppm",
        1280.0,
        820.0,
        false,
        true,
        false,
        false,
        0,
    );
    capture(
        "ykpili-player-fav.ppm",
        1280.0,
        820.0,
        false,
        false,
        true,
        // 展开态截图：368 宽的收藏夹卡片要按 400 侧栏排版才完整。
        true,
        0,
    );
    capture(
        "ykpili-player-expanded.ppm",
        1280.0,
        820.0,
        false,
        false,
        false,
        true,
        0,
    );
    capture(
        "ykpili-player-quality-menu.ppm",
        1280.0,
        820.0,
        false,
        false,
        false,
        false,
        1,
    );
    capture(
        "ykpili-player-speed-menu.ppm",
        1280.0,
        820.0,
        false,
        false,
        false,
        false,
        2,
    );
    capture(
        "ykpili-player-fullscreen.ppm",
        1280.0,
        820.0,
        false,
        false,
        false,
        false,
        3,
    );
}
