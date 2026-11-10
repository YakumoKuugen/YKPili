//! 搜索结果浮窗的层级验收：它必须压在播放器之上。
//!
//! 之前的顺序是「结果浮窗 → 播放器」，起播后视频盖住结果；现在反过来，
//! 结果浮窗与搜索框浮层同属最上层。这里用一个真实存在的播放器验证：
//! 点面板外的空白处，必须由结果浮窗的整窗背板接住（开始退场），
//! 而不是落到下面播放器的浮层上。
//!
//! 本地验收：`cargo test -p yk-app --test search_results_layer`

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::{
    point, px, size, Bounds, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::player::PlayerView;
use yk_app::AppRoot;
use yk_infra::search::{SearchResults, SearchVideo};

fn results() -> SearchResults {
    let mut results = SearchResults::default();
    results.videos = (0..5)
        .map(|index| SearchVideo {
            bvid: format!("BV1layer{index}"),
            aid: 100 + index as u64,
            title: format!("层级验收视频 {index}"),
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

fn click_at(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    let modifiers = Default::default();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers,
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers,
        click_count: 1,
    });
}

#[gpui::test]
fn results_panel_sits_above_the_player(cx: &mut TestAppContext) {
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
                // 播放器先建好、结果浮窗后开：两者都真实存在于元素树里。
                let player = cx.new(|cx| PlayerView::new(theme.clone(), cx));
                cx.new(|_| {
                    AppRoot::new()
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
        app.read_with(&cx, |app, _| app.search_results_open_for_test()),
        "夹具里的结果浮窗没打开"
    );

    // 面板大概居中（宽 ≤1040 / 高 ≤720），左上角 (60,60) 落在面板外的背板上。
    click_at(&mut cx, point(px(60.), px(60.)));
    assert!(
        app.read_with(&cx, |app, _| app.search_results_closing_for_test()),
        "点结果浮窗的空白处没有由浮窗接住：说明播放器还压在它上面"
    );
}
