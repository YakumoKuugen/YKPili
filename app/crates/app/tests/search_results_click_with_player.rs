//! 「播放器开着时，搜索结果里的封面还能点」的回归验收。
//!
//! 结果浮窗压在播放器之上，但卡片点击原本有一道「播放器打开就忽略」的闸门：
//! `card_is_playing()` 只看 `playing.is_some()`，于是播放器一开，结果里的封面
//! 全点不动。现在这道闸门只拦「播放器下面的卡片」，结果浮窗里的卡照常可点，
//! 点开就走 `card_open` 换源并让浮窗退场。
//!
//! 本地验收：`cargo test -p yk-app --test search_results_click_with_player`

use std::time::Duration;

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::player::PlayerView;
use yk_app::AppRoot;
use yk_infra::search::{SearchResults, SearchVideo};

fn results() -> SearchResults {
    let mut results = SearchResults::default();
    results.videos = (0..5)
        .map(|index| SearchVideo {
            bvid: format!("BV1click{index}"),
            aid: 100 + index as u64,
            title: format!("播放器开着时点我 {index}"),
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
fn covers_in_results_stay_clickable_while_player_is_open(cx: &mut TestAppContext) {
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
                let player = cx.new(|cx| PlayerView::new(theme.clone(), cx));
                // 播放器实例已经在（相当于用户已经打开了视频），再把结果浮窗展开。
                cx.new(|_| {
                    AppRoot::loading()
                        .without_login_prompt()
                        .with_player(player)
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
    assert!(
        app.read_with(&cx, |app, _| app.player_open_for_test()),
        "夹具里播放器没打开"
    );
    assert!(app.read_with(&cx, |app, _| app.search_results_open_for_test()));

    // 点结果里的第一张封面。之前这里被 `card_is_playing()` 拦掉，什么都不会发生。
    cx.update(|window, cx| {
        window.click(("cover", 0usize), cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert!(
        app.read_with(&cx, |app, _| app.search_results_closing_for_test()),
        "播放器开着时点结果封面没有反应：卡片点击被播放态闸门拦掉了"
    );

    cx.executor().advance_clock(Duration::from_millis(400));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        !app.read_with(&cx, |app, _| app.search_results_open_for_test()),
        "点完封面后结果浮窗应当退场，把画面让给播放器"
    );
}
