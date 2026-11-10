//! 从搜索结果起播时，结果浮窗必须让位的验收。
//!
//! 结果浮窗现在是最上层、还带一块整窗背板；不收起的话，点结果打开的
//! 播放器会被它整个挡住。这里点真实的视频卡片，断言：
//! ① 播放器被创建；
//! ② 结果浮窗开始退场，并在 260ms 后真正卸掉。
//!
//! 本地验收：`cargo test -p yk-app --test search_results_play`

use std::time::Duration;

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions};
use yk_app::AppRoot;
use yk_infra::search::{SearchResults, SearchVideo};

fn results() -> SearchResults {
    let mut results = SearchResults::default();
    results.videos = (0..5)
        .map(|index| SearchVideo {
            bvid: format!("BV1layer{index}"),
            aid: 100 + index as u64,
            title: format!("起播验收视频 {index}"),
            description: String::new(),
            author: "测试 UP 主".into(),
            mid: 900 + index as i64,
            up_face: String::new(),
            cover_url: String::new(),
            view_count: 1000,
            danmaku_count: 10,
            favorite_count: 20,
            duration_secs: 60,
            pubdate: 1_790_000_000,
            typename: "动画".into(),
            is_live: false,
        })
        .collect();
    results
}

#[gpui::test]
fn playing_from_results_closes_the_panel(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        gpui::init(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1280.), px(820.)),
                })),
                ..Default::default()
            },
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                let theme = yk_theme::theme::YkTheme::dark();
                yk_theme::gpui_theme::apply(cx, &theme);
                // 首页保持空的加载态：页面里没有 `cover` 卡片，点中的一定是结果里的那张。
                cx.new(|_| {
                    AppRoot::loading()
                        .without_login_prompt()
                        .with_search_results("鬼灭", results(), 5)
                })
            },
        )
        .unwrap()
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let app = window.root(&mut cx).expect("window root");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(app.read_with(&cx, |app, _| app.search_results_open_for_test()));
    assert!(!app.read_with(&cx, |app, _| app.player_open_for_test()));

    // 点结果里的第一张视频卡：走真实的整卡点击 → `CardHost::card_open`。
    cx.update(|window, cx| {
        window.click(("cover", 0usize), cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert!(
        app.read_with(&cx, |app, _| app.player_open_for_test()),
        "点结果卡片没有创建播放器"
    );
    assert!(
        app.read_with(&cx, |app, _| app.search_results_closing_for_test()),
        "起播后结果浮窗没有开始退场：它会一直盖在播放器上"
    );

    // 退场动画 260ms + 30ms 余量。
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        !app.read_with(&cx, |app, _| app.search_results_open_for_test()),
        "退场动画结束后结果浮窗还在树上"
    );
}
