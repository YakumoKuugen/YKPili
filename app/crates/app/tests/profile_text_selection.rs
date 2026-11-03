//! UP 主浮窗「动态正文可选」的复现用例。
//!
//! 现象：同一份 `comment_view::selectable_text`，评论区能拖选，浮窗里不行。
//! 这里把两种容器各搭一份，用同样的拖动手势比较选区结果。

use gpui::component::ThemeMode;
use gpui::test::TestWindowExt;
use gpui::prelude::*;
use gpui::{
    div, point, px, size, App, Bounds, Context, Modifiers, MouseButton, Render, TestAppContext,
    VisualTestContext, Window, WindowBounds, WindowOptions,
};

const TEXT: &str = "见字如画，文字用细节交织你的画面，这是一段足够长的动态正文用来拖选";

fn style() -> gpui::TextStyleRefinement {
    gpui::TextStyleRefinement {
        font_size: Some(px(13.).into()),
        line_height: Some(px(21.).into()),
        ..Default::default()
    }
}

fn notice() -> std::rc::Rc<dyn Fn(&mut App)> {
    std::rc::Rc::new(|_: &mut App| {})
}

struct Probe {
    /// true = 复刻浮窗的容器链；false = 评论行那种平坦结构。
    nested: bool,
}

impl Render for Probe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.nested {
            div().size_full().p(px(20.)).child(
                // shell：圆角 + 裁切（UpProfileShell 的两条关键属性）
                div()
                    .w(px(700.))
                    .h(px(400.))
                    .rounded(px(12.))
                    .overflow_hidden()
                    // body：滚动视口
                    .child(
                        div().id("nested-body").size_full().overflow_y_scroll().p(px(20.)).child(
                            div().p(px(12.)).child(yk_app::comment_view::selectable_text(
                                "nested-text",
                                "nested-select",
                                620.0,
                                true,
                                TEXT.to_string(),
                                style(),
                                TEXT.to_string(),
                                notice(),
                            )),
                        ),
                    ),
            )
        } else {
            div().size_full().p(px(20.)).child(
                div().w(px(700.)).child(yk_app::comment_view::selectable_text(
                    "flat-text",
                    "flat-select",
                    700.0,
                    true,
                    TEXT.to_string(),
                    style(),
                    TEXT.to_string(),
                    notice(),
                )),
            )
        }
    }
}

fn drag_select(cx: &mut TestAppContext, nested: bool) -> String {
    let (window, _view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(800.), px(500.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                cx.new(|_| Probe { nested })
            },
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    // 起点落在正文左上角内侧，向右拖 300px（单行）。
    let (x0, y0, x1) = if nested {
        (56.0, 60.0, 380.0)
    } else {
        (24.0, 28.0, 340.0)
    };
    vcx.simulate_mouse_down(point(px(x0), px(y0)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(point(px(x1), px(y0)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(point(px(x1), px(y0)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx))
}

#[gpui::test]
fn flat_container_selects(cx: &mut TestAppContext) {
    let selected = drag_select(cx, false);
    assert!(!selected.trim().is_empty(), "对照组没选中：{selected:?}");
}

#[gpui::test]
fn nested_containers_select(cx: &mut TestAppContext) {
    let selected = drag_select(cx, true);
    eprintln!("nested selected = {selected:?}");
    assert!(!selected.trim().is_empty(), "浮窗式容器没选中：{selected:?}");
}

// ================= 真实 UpProfileView 复现 =================

use gpui::Entity;
use yk_app::up_profile::UpProfileView;
use yk_infra::space::SpaceDynamic;

fn dynamics() -> Vec<SpaceDynamic> {
    vec![SpaceDynamic {
        id: 9001,
        pub_ts: 1_790_000_000,
        text: TEXT.to_string(),
        cover: String::new(),
        title: String::new(),
        kind: "text".into(),
        likes: 12,
        comments: 3,
        forwards: 1,
        bvid: String::new(),
        images: Vec::new(),
        views: 0,
        danmaku: 0,
        duration_secs: 0,
    }]
}

fn open_profile(cx: &mut TestAppContext) -> (VisualTestContext, Entity<UpProfileView>) {
    let (window, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1200.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                let viewport = window.viewport_size();
                cx.new(|cx| {
                    UpProfileView::new(
                        yk_theme::theme::YkTheme::dark(),
                        42,
                        "测试UP".into(),
                        String::new(),
                        Bounds {
                            origin: Default::default(),
                            size: size(px(200.), px(120.)),
                        },
                        viewport,
                        cx,
                    )
                    .with_dynamics(dynamics())
                })
            },
        )
        .unwrap()
    });
    // 复刻 `ensure_up_profile` 里的「打开即交焦点」。
    let focus = cx.update(|app| view.read(app).focus_handle());
    cx.executor().advance_clock(std::time::Duration::from_millis(500));
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.focus(&focus, cx));
    for _ in 0..3 {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    (vcx, view)
}

#[gpui::test]
fn profile_dynamic_text_drag_selects(cx: &mut TestAppContext) {
    let (mut vcx, view) = open_profile(cx);
    assert_eq!(view.read_with(&vcx, |v, _| v.test_dynamics_len()), 1);

    // 直接问测试框架正文元素在哪，不靠算坐标。
    let bounds = vcx
        .update(|window, _cx| window.try_find("up-profile-dynamic-text-0"))
        .expect("找不到动态正文元素（debug_selector 没生效？）")
        .bounds();
    eprintln!("正文 bounds = {bounds:?}");
    let y = bounds.origin.y.as_f32() + 10.0;
    let x0 = bounds.origin.x.as_f32() + 4.0;
    vcx.simulate_mouse_down(point(px(x0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("profile selected = {selected:?}");
    assert!(!selected.trim().is_empty(), "浮窗动态正文没选中：{selected:?}");
}

// ================= 播放器 + 浮窗共存 =================

use yk_app::player::PlayerView;

struct Host {
    player: Entity<PlayerView>,
    profile: Entity<UpProfileView>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.player.clone())
            .child(self.profile.clone())
    }
}

#[gpui::test]
fn profile_over_player_still_selects(cx: &mut TestAppContext) {
    let (window, host) = cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1200.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                let viewport = window.viewport_size();
                let player = cx.new(|cx| {
                    PlayerView::new(yk_theme::theme::YkTheme::dark(), cx).with_expanded(true)
                });
                let profile = cx.new(|cx| {
                    UpProfileView::new(
                        yk_theme::theme::YkTheme::dark(),
                        42,
                        "测试UP".into(),
                        String::new(),
                        Bounds { origin: Default::default(), size: size(px(200.), px(120.)) },
                        viewport,
                        cx,
                    )
                    .with_dynamics(dynamics())
                });
                cx.new(|_| Host { player, profile })
            },
        )
        .unwrap()
    });
    cx.executor().advance_clock(std::time::Duration::from_millis(500));
    let mut vcx = VisualTestContext::from_window(window, cx);
    for _ in 0..3 {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    let bounds = vcx
        .update(|window, _cx| window.try_find("up-profile-dynamic-text-0"))
        .expect("找不到动态正文元素")
        .bounds();
    eprintln!("叠加场景 正文 bounds = {bounds:?}");
    let y = bounds.origin.y.as_f32() + 10.0;
    let x0 = bounds.origin.x.as_f32() + 4.0;
    vcx.simulate_mouse_down(point(px(x0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("叠加场景 selected = {selected:?}");
    assert!(!selected.trim().is_empty(), "播放器之上的浮窗正文没选中：{selected:?}");
    let _ = host;
}

#[gpui::test]
fn profile_selects_after_frame_by_frame_animation(cx: &mut TestAppContext) {
    let (window, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1200.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                let viewport = window.viewport_size();
                cx.new(|cx| {
                    UpProfileView::new(
                        yk_theme::theme::YkTheme::dark(),
                        42,
                        "测试UP".into(),
                        String::new(),
                        Bounds { origin: Default::default(), size: size(px(200.), px(120.)) },
                        viewport,
                        cx,
                    )
                    .with_dynamics(dynamics())
                })
            },
        )
        .unwrap()
    });
    let focus = cx.update(|app| view.read(app).focus_handle());
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.focus(&focus, cx));
    // 逐帧跑入场动画（真实窗口就是这样），而不是一次性跳过。
    for _ in 0..40 {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        vcx.executor().advance_clock(std::time::Duration::from_millis(16));
    }
    let bounds = vcx
        .update(|window, _cx| window.try_find("up-profile-dynamic-text-0"))
        .expect("找不到动态正文元素")
        .bounds();
    eprintln!("逐帧动画后 正文 bounds = {bounds:?}");
    let y = bounds.origin.y.as_f32() + 10.0;
    let x0 = bounds.origin.x.as_f32() + 4.0;
    vcx.simulate_mouse_down(point(px(x0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("逐帧动画后 selected = {selected:?}");
    assert!(!selected.trim().is_empty(), "逐帧动画后无法选中：{selected:?}");
}

/// 拖动过程中界面重渲染（hover 动画、notify）会不会把选区打断。
#[gpui::test]
fn selection_survives_rerender_mid_drag(cx: &mut TestAppContext) {
    let (mut vcx, view) = open_profile(cx);
    let bounds = vcx
        .update(|window, _cx| window.try_find("up-profile-dynamic-text-0"))
        .expect("找不到动态正文元素")
        .bounds();
    let y = bounds.origin.y.as_f32() + 10.0;
    let x0 = bounds.origin.x.as_f32() + 4.0;

    vcx.simulate_mouse_down(point(px(x0), px(y)), MouseButton::Left, Modifiers::default());
    // 按下之后、移动之前，强制重渲染一次（模拟 hover / 加载回调触发的 notify）。
    vcx.update(|window, cx| {
        view.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear(cx);
    });
    vcx.simulate_mouse_move(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("重渲染之后 selected = {selected:?}");
    assert!(!selected.trim().is_empty(), "重渲染打断了选区：{selected:?}");
}

// ================= 真实的 AppRoot 组合 =================

use yk_app::AppRoot;

#[gpui::test]
fn selection_inside_real_app_root(cx: &mut TestAppContext) {
    let (window, root) = cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1200.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                let root = cx.new(|_| AppRoot::new().without_login_prompt());
                root.update(cx, |root, cx| {
                    root.open_up_profile_for_test(
                        42,
                        "测试UP".into(),
                        String::new(),
                        window,
                        cx,
                    );
                    root.inject_up_profile_dynamics_for_test(dynamics(), cx);
                });
                root
            },
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window, cx);
    for _ in 0..4 {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        vcx.executor().advance_clock(std::time::Duration::from_millis(16));
    }
    let bounds = vcx
        .update(|window, _cx| window.try_find("up-profile-dynamic-text-0"))
        .expect("AppRoot 里找不到动态正文")
        .bounds();
    eprintln!("AppRoot 场景 正文 bounds = {bounds:?}");
    let y = bounds.origin.y.as_f32() + 10.0;
    let x0 = bounds.origin.x.as_f32() + 4.0;
    vcx.simulate_mouse_down(point(px(x0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("AppRoot 场景 selected = {selected:?}");
    // 回归护栏：面板上那条 `on_click(stop_propagation)` 会吞掉 mouse-up，
    // 让选区的 `is_selecting` 永远停在 true，此后正文一律选不中。
    assert!(!selected.trim().is_empty(), "AppRoot 里正文没选中：{selected:?}");
    let _ = root;
}

/// 真实拖动是「按下 → 连续多帧移动 → 松开」，每帧都会重渲染。
#[gpui::test]
fn selection_survives_multi_frame_drag(cx: &mut TestAppContext) {
    let (mut vcx, _view) = open_profile(cx);
    let bounds = vcx
        .update(|window, _cx| window.try_find("up-profile-dynamic-text-0"))
        .expect("找不到动态正文元素")
        .bounds();
    let y = bounds.origin.y.as_f32() + 10.0;
    let x0 = bounds.origin.x.as_f32() + 4.0;
    vcx.simulate_mouse_down(point(px(x0), px(y)), MouseButton::Left, Modifiers::default());
    for step in 1..=20 {
        let x = x0 + 15.0 * step as f32;
        vcx.simulate_mouse_move(point(px(x), px(y)), MouseButton::Left, Modifiers::default());
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        vcx.executor().advance_clock(std::time::Duration::from_millis(16));
    }
    let selected_mid =
        vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("拖动中选区 = {selected_mid:?}");
    vcx.simulate_mouse_up(point(px(x0 + 300.0), px(y)), MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("松开后选区 = {selected:?}");
    assert!(!selected.trim().is_empty(), "多帧拖动没有选区：{selected:?}");
}
