//! 顶栏 UX 的离屏渲染验收：常态、搜索聚焦、窄窗三种状态。
//!
//! 只在 macOS / Linux 上跑；GPUI 的 headless renderer 需要 Metal / WGPU。
//! 图片写到 `/tmp/ykpili-topbar-*.ppm`，用 `sips -s format png` 转 PNG 后目检。
//!
//! 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test topbar_visual`

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::AppRoot;
use yk_core::history::HistoryEntry;
use yk_core::search::{Hotword, SuggestItem};

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn capture(app: impl FnOnce() -> AppRoot, path: &str, width: f32, height: f32) {
    let mut cx = context();
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
                    yk_ui::gpui_theme::apply(cx, &yk_ui::theme::YkTheme::dark());
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
    eprintln!("[topbar] {path} {width}x{height}");
}

fn suggestions() -> Vec<SuggestItem> {
    ["测试顶栏", "测试搜索聚焦", "testflight"]
        .into_iter()
        .map(|term| SuggestItem {
            value: term.into(),
            spans: vec![yk_core::search::HighlightSpan {
                text: term.into(),
                highlighted: false,
            }],
        })
        .collect()
}

fn hotwords() -> Vec<Hotword> {
    (0..8)
        .map(|index| Hotword {
            keyword: format!("热搜词条 {}", index + 1),
            pos: index as u32 + 1,
            word_type: 5,
        })
        .collect()
}

fn history_entry(index: usize) -> HistoryEntry {
    HistoryEntry {
        kid: 1000 + index as i64,
        title: format!("历史记录第 {index} 条：两行标题在卡片里如何截断"),
        cover: String::new(),
        bvid: format!("BV1hist{index}"),
        cid: index as i64,
        oid: index as i64,
        epid: 0,
        page: 1,
        videos: 1,
        author_name: "测试 UP 主".into(),
        author_face: String::new(),
        author_mid: 900 + index as i64,
        view_at: 1_790_000_000 - index as u64 * 3600,
        progress_secs: 300,
        duration_secs: 600,
        is_finish: false,
        business: "archive".into(),
        tag_name: "日常".into(),
        badge: String::new(),
    }
}

fn main() {
    // 默认跳过：headless 渲染需要 GPU，CI 或没有图形后端的环境不该因为截图失败而红。
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[topbar] skipped（设置 YK_VISUAL=1 生成顶栏截图）");
        return;
    }
    // Headless 渲染要求 macOS 平台对象在主线程创建，因此走 `harness = false`。
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        // 常态：品牌胶囊 + 居中搜索 + 右侧「设置 / 头像」胶囊。
        capture(
            || AppRoot::new().without_login_prompt(),
            "/tmp/ykpili-topbar-idle.ppm",
            1280.0,
            820.0,
        );
        // 聚焦：蓝色粗环 + 联想面板，确认环没有被顶栏裁切。
        capture(
            || {
                AppRoot::new()
                    .without_login_prompt()
                    .with_search_suggestions("测试", suggestions(), hotwords())
            },
            "/tmp/ykpili-topbar-focus.ppm",
            1280.0,
            820.0,
        );
        // 封面 hover：与顶栏胶囊同一套玻璃材质（fill_2 + border_strong + 内高光）。
        capture(
            || AppRoot::new().without_login_prompt().with_hovered_card(1),
            "/tmp/ykpili-topbar-hover.ppm",
            1280.0,
            820.0,
        );
        // 历史批量选中：选中卡与顶栏胶囊共用 fill_2 + border_strong + 内高光。
        capture(
            || {
                // 历史页没登录会显示登录空态；给一个假会话，只为让卡片渲染出来。
                let session = yk_core::models::UserSession {
                    dede_user_id: 1,
                    sess_data: "visual-test".into(),
                    nickname: "视觉测试".into(),
                    is_logged_in: true,
                    ..Default::default()
                };
                AppRoot::new()
                    .without_login_prompt()
                    .with_session(Some(session))
                    .with_history_entries((0..8).map(history_entry).collect())
            },
            "/tmp/ykpili-history-selected.ppm",
            1280.0,
            820.0,
        );
        // 窄窗：搜索框收缩，左右两组胶囊不重叠。
        capture(
            || AppRoot::new().without_login_prompt(),
            "/tmp/ykpili-topbar-narrow.ppm",
            900.0,
            820.0,
        );
        eprintln!("[topbar] done");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    eprintln!("[topbar] skipped：当前平台没有 GPUI headless renderer");
}
