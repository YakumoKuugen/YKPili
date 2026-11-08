//! 排行榜 / 收藏夹页的离屏渲染验收。
//!
//! 只在 macOS / Linux 上跑；图片写到 `/tmp/ykpili-rank.ppm` 与
//! `/tmp/ykpili-favorites.ppm`，用 `sips -s format png` 转 PNG 后目检。
//!
//! 这里不请求网络：封面 / 头像都留空，重点看名次徽标、榜单说明、收藏夹栏
//! 与卡片网格的几何是否成立。

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::AppRoot;
use yk_infra::favorites::FavVideo;
use yk_infra::interaction::FavFolder;
use yk_infra::ranking::{RankItem, RankPage};

fn temp_path(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn context() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        gpui::platform::current_platform(true).text_system(),
        Arc::new(AllAssets),
        gpui::platform::current_headless_renderer,
    );
    cx.update(gpui::init);
    cx
}

fn capture(app: impl FnOnce() -> AppRoot, path: &str, height: f32) {
    let mut cx = context();
    let (handle, _) = cx
        .update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1280.), px(height)),
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

fn rank_item(rank: usize, title: &str, up: &str, views: u64, his_rank: u64) -> RankItem {
    RankItem {
        rank,
        aid: rank as i64,
        bvid: format!("BV1rank{rank}"),
        cid: 0,
        title: title.into(),
        cover: String::new(),
        up_name: up.into(),
        up_face: String::new(),
        up_mid: rank as i64,
        view_count: views,
        danmaku_count: views / 50,
        duration_secs: 632,
        pubdate: 1_793_000_000,
        his_rank,
        up_from: if rank % 3 == 0 { 2 } else { 0 },
        tname: "动画".into(),
    }
}

fn rank_page() -> RankPage {
    RankPage {
        note: "根据稿件内容质量、近期的数据综合展示，动态更新".into(),
        items: vec![
            rank_item(1, "那些最终被证实成真的古代神话", "生肉烹饪师", 42_000, 1),
            rank_item(2, "5090 禁止出口？那我 B300 哪来的", "一碗甜子", 22_000, 5),
            rank_item(3, "卡琳超详细机制速课打法", "阿哲同学", 4_414, 3),
            rank_item(
                4,
                "6G 显存 + 16G 内存，能跑多大的模型",
                "班长captain",
                26_000,
                12,
            ),
            rank_item(
                5,
                "从零写一个视频播放器：解码与渲染",
                "技术宅小明",
                18_000,
                9,
            ),
            rank_item(6, "这个夏天最值得去的十个地方", "猫猫观察日记", 9_800, 20),
        ],
    }
}

fn folder(id: i64, title: &str, count: u64, attr: i64) -> FavFolder {
    FavFolder {
        id,
        title: title.into(),
        media_count: count,
        attr,
        fav_state: 0,
    }
}

fn fav_video(index: usize, title: &str) -> FavVideo {
    FavVideo {
        aid: index as i64,
        bvid: format!("BV1fav{index}"),
        title: title.into(),
        cover: String::new(),
        up_name: "收藏夹 UP 主".into(),
        up_face: String::new(),
        up_mid: 900 + index as i64,
        duration_secs: 632,
        view_count: 120_000 + index as u64 * 1000,
        danmaku_count: 300 + index as u64,
        pubdate: 1_790_000_000,
        fav_time: 1_793_000_000 - index as u64,
        attr: 0,
    }
}

fn render_rank() {
    let page = rank_page();
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_rank_page(0, page)
        },
        &temp_path("ykpili-rank.ppm"),
        1000.0,
    );
}

fn render_favorites() {
    let folders = vec![
        folder(1, "默认收藏夹", 266, 1),
        folder(2, "Rust 与 GPUI", 48, 0),
        folder(3, "纪录片", 12, 0),
        folder(4, "稍后再看", 3, 0),
    ];
    let items = (0..8)
        .map(|index| {
            fav_video(
                index,
                &format!("收藏的第 {index} 个视频：两行标题在卡片里是否被正确截断"),
            )
        })
        .collect();
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_signed_in()
                .with_favorites(folders, items, 266)
        },
        &temp_path("ykpili-favorites.ppm"),
        1000.0,
    );
}

/// 排行榜的分区胶囊收进了 Dock 二级分页：截图确认展开态与折行几何。
fn render_rank_zones() {
    let page = rank_page();
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_rank_page(0, page)
                .with_dock_tabs(true)
        },
        &temp_path("ykpili-rank-zones.ppm"),
        1000.0,
    );
}

fn render_quit_hint() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_quit_hint()
        },
        &temp_path("ykpili-quit-hint.ppm"),
        820.0,
    );
}

fn main() {
    // 默认跳过：headless 渲染需要 GPU（macOS Metal / Linux WGPU）。
    // 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test pages_visual`。
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[visual] skipped（设置 YK_VISUAL=1 生成排行榜 / 收藏夹 / 退出提示截图）");
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        render_rank();
        render_favorites();
        render_rank_zones();
        render_quit_hint();
        eprintln!("[visual] done");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    eprintln!("[visual] skipped：当前平台没有 GPUI headless renderer");
}
