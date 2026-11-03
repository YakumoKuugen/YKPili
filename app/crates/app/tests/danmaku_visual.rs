//! 弹幕层对齐的离屏渲染验收。
//!
//! 只在 macOS / Linux 上跑；GPUI 的 headless renderer 需要 Metal / WGPU。
//! 图片写到 `/tmp/ykpili-danmaku.ppm`，用 `sips -s format png` 转 PNG 后目检。
//!
//! 验收点：
//! ① 弹幕层的左下角必须和播放层左下角重合 —— 第一行弹幕贴在画面顶部内侧，
//!    整层不应整体上移一条顶栏；
//! ② 高密度下右边缘必须有正在进场的弹幕 —— 不能被「按条数截断」挡住，
//!    否则弹幕会走到画面中间才出现。
//!
//! 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test danmaku_visual`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::player::PlayerView;
use yk_domain::danmaku::DanmakuSet;

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

/// 高密度滚动弹幕：30 条/秒 × 6 秒。
///
/// 取样点 t=5s 时同屏可见 ~130 条 —— 旧的 `.take(80)` 会把最新的一批
/// 挡在绘制集合外，它们要等老弹幕离场才出现（那时已经在画面中间）。
/// 这张图同时验收「右边缘有正在进场的弹幕」。
fn dense_set() -> DanmakuSet {
    let mut raw = Vec::new();
    for tick in 0..180 {
        let time = tick as f64 / 30.0;
        let text = if tick % 8 == 0 {
            format!("轨道对齐验收 第 {tick} 条比较长的弹幕")
        } else {
            format!("轨道 {tick}")
        };
        raw.push((format!("{time},1,25,16777215,0,0,0,0"), text));
    }
    DanmakuSet::from_raw(raw)
}

fn capture(path: &str, width: f32, height: f32) {
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
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    yk_theme::gpui_theme::apply(cx, &theme);
                    // 直接拿播放器当根元素：不要页面内容与登录卡片挡住画面，
                    // 弹幕墙才能整屏目检。
                    cx.new(|cx| {
                        PlayerView::new(theme.clone(), cx).with_danmaku(dense_set(), 5.0)
                    })
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
    eprintln!("[danmaku] {path} {width}x{height}");
}

fn main() {
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[danmaku] skipped（设置 YK_VISUAL=1 生成弹幕层截图）");
        return;
    }
    capture("/tmp/ykpili-danmaku.ppm", 1280.0, 820.0);
}
