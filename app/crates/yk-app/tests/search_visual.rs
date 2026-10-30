//! 搜索 UX 的离屏渲染验收：结果页与联想面板各出一张图。
//!
//! 只在 macOS / Linux 上跑；GPUI 的 headless renderer 需要 Metal / WGPU。
//! 图片写到 `/tmp/ykpili-search-results.ppm` 与 `/tmp/ykpili-search-suggest.ppm`，
//! 用 `sips -s format png` 转 PNG 后目检。

use std::fs;
use std::sync::Arc;

use gpui::assets::AllAssets;
use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, HeadlessAppContext, WindowBounds, WindowOptions};
use yk_app::AppRoot;
use yk_core::search::{
    HighlightSpan, Hotword, SearchEpisode, SearchMedia, SearchResults, SearchUser, SearchVideo,
    SuggestItem,
};

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
    eprintln!("[visual] {path} {width}x{height}");
}

fn video(index: usize, title: &str) -> SearchVideo {
    SearchVideo {
        bvid: format!("BV1visual{index}"),
        aid: 100 + index as u64,
        title: title.into(),
        description: "一段用于验收的搜索视频结果。".into(),
        author: "测试 UP 主".into(),
        mid: 900 + index as i64,
        up_face: String::new(),
        cover_url: String::new(),
        view_count: 123_000 + index as u64 * 1000,
        danmaku_count: 400 + index as u64,
        favorite_count: 12_000,
        duration_secs: 632,
        pubdate: 1_790_000_000,
        typename: "动画".into(),
        is_live: false,
    }
}

fn results() -> SearchResults {
    let mut results = SearchResults::default();
    results.videos = (0..12)
        .map(|index| {
            video(
                index,
                &format!("搜索结果的视频标题第 {index} 条：两行标题在卡片里是否被正确截断"),
            )
        })
        .collect();
    results.films.push(SearchMedia {
        media_id: 1,
        season_id: 2,
        title: "鬼灭之刃：无限城篇 第一章 猗窝座再袭".into(),
        original_title: "劇場版「鬼滅の刃」無限城編".into(),
        cover_url: String::new(),
        areas: "日本".into(),
        styles: "动作/奇幻/动画".into(),
        index_show: "全 1 集".into(),
        description: "为了让变成鬼的妹妹祢豆子重新变回人类，灶门炭治郎加入了猎鬼组织「鬼杀队」，与同伴我妻善逸、嘴平伊之助一同对抗各类恶鬼。".into(),
        season_type_name: "电影".into(),
        score: Some(9.6),
        score_count: 2504,
        episodes: vec![SearchEpisode {
            id: 1,
            index_title: "PV1".into(),
            long_title: "预告".into(),
            cover_url: String::new(),
        }],
        is_film: true,
    });
    results.users.push(SearchUser {
        mid: 3,
        name: "哔哩哔哩电影".into(),
        sign: "电影官方账号，每周更新片单。".into(),
        avatar_url: String::new(),
        fans: 8_435_653,
        videos: 5_582,
        level: 6,
        verify: "bilibili 知名UP主".into(),
        previews: (0..3)
            .map(|index| video(20 + index, "UP 主最新投稿：三分钟看完本周新片"))
            .collect(),
    });
    results
}

fn suggest_item(term: &str) -> SuggestItem {
    let spans = if term.starts_with("test") {
        vec![
            HighlightSpan {
                text: "test".into(),
                highlighted: true,
            },
            HighlightSpan {
                text: term.trim_start_matches("test").to_string(),
                highlighted: false,
            },
        ]
    } else {
        vec![HighlightSpan {
            text: term.into(),
            highlighted: false,
        }]
    };
    SuggestItem {
        value: term.into(),
        spans,
    }
}

fn render_search_results() {
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_search_results("鬼灭", results(), 1000)
        },
        "/tmp/ykpili-search-results.ppm",
        1400.0,
    );
}

fn render_search_suggestions() {
    let items = vec![
        "testflight邀请码",
        "testflight",
        "test",
        "tes突然的陀螺",
        "TESTV官方频道",
        "托尔斯泰",
        "雅思 17test1",
        "testv官方频道耳机",
        "testv",
        "test是什么意思",
    ]
    .into_iter()
    .map(suggest_item)
    .collect();
    let hotwords = (0..10)
        .map(|index| Hotword {
            keyword: format!("热搜词条 {}", index + 1),
            pos: index as u32 + 1,
            word_type: if index % 3 == 0 { 4 } else { 5 },
        })
        .collect();
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_search_suggestions("test", items, hotwords)
        },
        "/tmp/ykpili-search-suggest.ppm",
        820.0,
    );
}

fn render_search_videos() {
    let mut results = SearchResults::default();
    results.videos = (0..10)
        .map(|index| {
            video(
                index,
                &format!("纯视频结果第 {index} 条：标题两行截断与卡片 hover 热区"),
            )
        })
        .collect();
    capture(
        || {
            AppRoot::new()
                .without_login_prompt()
                .with_search_results("西甲就姆巴佩不当言论道歉", results, 200)
        },
        "/tmp/ykpili-search-videos.ppm",
        900.0,
    );
}

fn main() {
    // 默认跳过：headless 渲染需要 GPU（macOS Metal / Linux WGPU），
    // CI 或没有图形后端的环境不该因为截图失败而红。
    // 本地验收：`YK_VISUAL=1 cargo test -p yk-app --test search_visual`。
    if std::env::var("YK_VISUAL").is_err() {
        eprintln!("[visual] skipped（设置 YK_VISUAL=1 生成搜索截图）");
        return;
    }
    // Headless 渲染要求 macOS 平台对象在主线程创建，因此走 `harness = false`。
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        render_search_results();
        render_search_suggestions();
        render_search_videos();
        eprintln!("[visual] done");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    eprintln!("[visual] skipped：当前平台没有 GPUI headless renderer");
}
