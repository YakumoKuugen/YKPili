//! 文本选中的行为验收：拖动必须真的选中、复制拿到的是选中区间。
//!
//! 走的是 `gpui-base` 的窗口级选区：`SelectableText` 注册命中区，
//! `base::Root` 渲染 `TextSelectionLayer` 并在 Cmd/Ctrl+C 时写剪贴板。
//! 这两层任何一层没接上，这里都会失败 —— 这正是要守的东西。

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::{
    div, point, px, size, Bounds, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext,
    Window, WindowBounds, WindowOptions,
};

struct TextProbe;

impl Render for TextProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().p(px(40.)).child(gpui::base::SelectableText::new(
            "selection-probe",
            "选择这段文字 hello world 12345",
        ))
    }
}

#[gpui::test]
fn drag_selects_text_and_copy_matches(cx: &mut TestAppContext) {
    let (window, _view) = cx.update(|cx| {
        gpui::init(cx);
        // 必须走 gpui-kit 的 `open_window`：它会把内容包进 `base::Root`，
        // 选区层（`TextSelectionLayer`）和复制动作都在那一层上。
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(720.), px(240.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                cx.new(|_| TextProbe)
            },
        )
        .unwrap()
    });

    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    // 文本从 (40, 40) 开始、单行；从左往右拖一段。
    vcx.simulate_mouse_down(
        point(px(42.), px(46.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.simulate_mouse_move(point(px(260.), px(46.)), MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(
        point(px(260.), px(46.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    assert!(
        !selected.trim().is_empty(),
        "拖动之后没有任何选中内容（选区层或命中区没接上）"
    );
    assert!(
        "选择这段文字 hello world 12345".contains(selected.trim()),
        "选中的内容不属于这段文字：{selected:?}"
    );
    // 起点在最左侧：选中区间应当从第一个字开始。
    assert!(
        selected.starts_with('选'),
        "选区应当从拖动起点开始：{selected:?}"
    );
}

/// 点文字时，选区层会在 **mouse-down** 上 `stop_propagation`，
/// 所以普通 `on_click` 收不到；但 **mouse-up** 仍然会冒泡到祖先。
/// 关注页卡片就是靠这一点做「点文字 → 打开详情」的兜底，这里守住它。
#[gpui::test]
fn click_still_reaches_ancestor_mouse_up(cx: &mut TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    struct Probe {
        ups: Rc<Cell<usize>>,
    }
    impl Render for Probe {
        fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let ups = self.ups.clone();
            div()
                .size_full()
                .p(px(40.))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |_this, _: &gpui::MouseUpEvent, _window, _cx| {
                        ups.set(ups.get() + 1);
                    }),
                )
                .child(gpui::base::SelectableText::new(
                    "probe-up",
                    "点击这段文字",
                ))
        }
    }

    let ups = Rc::new(Cell::new(0));
    let (window, _view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(720.), px(240.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                cx.new(|_| Probe { ups: ups.clone() })
            },
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    vcx.simulate_click(point(px(60.), px(46.)), Modifiers::default());
    assert_eq!(
        ups.get(),
        1,
        "点在可选中的文字上时，祖先的 mouse-up 应当仍然触发（卡片点击兜底依赖它）"
    );
}

#[gpui::test]
fn single_click_does_not_leave_a_selection(cx: &mut TestAppContext) {
    let (window, _view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(720.), px(240.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                cx.new(|_| TextProbe)
            },
        )
        .unwrap()
    });

    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    vcx.simulate_click(point(px(60.), px(46.)), Modifiers::default());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    assert!(
        selected.trim().is_empty(),
        "单击不应留下选中内容：{selected:?}"
    );
}
