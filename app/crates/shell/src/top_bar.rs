//! 顶栏：窗口级 chrome（拖动区、窗口按钮、材质胶囊）。
//!
//! 搜索框与头像属于能力/业务内容，由组装点作为槽位传进来；这里只负责
//! 顶栏布局、窗口拖动、Windows 自绘标题栏按钮。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_theme::theme::*;

/// 顶栏（64px）。
///
/// `leading` 通常是左侧头像胶囊的完整内容；`search` 是顶栏中部的搜索框。
pub fn top_bar<V: 'static>(
    t: &YkTheme,
    leading: AnyElement,
    search: AnyElement,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let bar = div()
        .id("topbar-drag")
        .relative()
        // 整条顶栏都能按住拖动窗口。控件组各自截住 mouse_down，
        // 所以点头像 / 输入框 / 窗口按钮不会误拖窗口。
        //
        // 双击必须在这里分流，不能挂 `on_double_click`：ClickEvent 会从
        // 搜索框冒泡到顶栏，双击输入框里的文字就会误缩放窗口；而搜索框在
        // capture 阶段就吞掉了 mouse_down，放在这里双击它不会被顶栏接住。
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            match titlebar_mouse_action(event.click_count) {
                TitlebarMouseAction::Drag => begin_window_drag(window),
                TitlebarMouseAction::DoubleClick => perform_titlebar_double_click(window),
                TitlebarMouseAction::Ignore => {}
            }
        })
        .flex()
        .items_center()
        .w_full()
        .h(px(TOP_BAR_HEIGHT))
        // 窗口再矮也不能把工具栏压扁：内容区自己滚动 / 留白。
        .flex_shrink_0()
        // macOS 透明标题栏的红黄绿压在顶栏左上：按钮占 20..72，头像胶囊从 92
        // 开始，留出呼吸位。Windows 没有红黄绿，头像直接贴左内边距。
        .pl(px(if cfg!(target_os = "macos") {
            SPACE_5 + 76.0
        } else {
            SPACE_5
        }))
        // Windows 的三个窗口按钮贴右边缘，方便按角落关闭；macOS 保留右边距。
        .pr(px(if cfg!(target_os = "windows") {
            0.0
        } else {
            SPACE_5
        }))
        .gap(px(SPACE_4))
        // 比卡片 / 浮层更暗、带一点透明：工具栏是「窗口的外壳」，不抢内容的视觉重量。
        .bg(s.surface_toolbar)
        .border_b_1()
        .border_color(s.border_subtle)
        .child(leading)
        // 搜索槽位按**窗口中线**绝对定位：收起态胶囊和展开浮层用同一个中心，
        // 展开 / 收回时不会因为左侧头像占位而横向跳一下。
        .child(
            div()
                .absolute()
                .left(px(
                    (window.viewport_size().width.as_f32() - TOP_BAR_SEARCH_COLLAPSED_WIDTH) / 2.0,
                ))
                .top(px(TOP_BAR_SEARCH_TOP))
                .child(search),
        );

    #[cfg(target_os = "windows")]
    let bar = bar.child(window_controls(t, window, cx));

    bar.into_any_element()
}

/// 左上角材质胶囊。内容由组装点提供（头像、设置入口等）。
pub fn toolbar_group(t: &YkTheme, child: AnyElement) -> AnyElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_0_5))
        .p(px(SPACE_0_5))
        .h(px(TOP_BAR_CONTROL_HEIGHT))
        .flex_shrink_0()
        .rounded_full()
        .bg(s.fill_2)
        .border_1()
        .border_color(s.border_strong)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(child)
        .into_any_element()
}

// ================= Windows 自绘标题栏 =================

/// 取当前窗口的 HWND（Win32）。
#[cfg(target_os = "windows")]
fn window_hwnd(window: &Window) -> Option<*mut core::ffi::c_void> {
    use raw_window_handle::RawWindowHandle;

    let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get() as *mut core::ffi::c_void),
        _ => None,
    }
}

/// 顶栏左键按下要做什么。
///
/// 顶栏同时是拖动区，双击必须和拖动分开处理：第二次按下若继续
/// `begin_window_drag`，只会再进一次系统拖动循环，永远走不到双击动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitlebarMouseAction {
    /// 单击：按住拖动窗口。
    Drag,
    /// 双击：执行平台原生的标题栏双击动作。
    DoubleClick,
    /// 三连击及以上：什么都不做，避免刚缩放又被还原。
    Ignore,
}

fn titlebar_mouse_action(click_count: usize) -> TitlebarMouseAction {
    match click_count {
        1 => TitlebarMouseAction::Drag,
        2 => TitlebarMouseAction::DoubleClick,
        _ => TitlebarMouseAction::Ignore,
    }
}

/// 双击标题栏：交给平台执行「连按标题栏」的默认动作。
///
/// macOS 走 `Window::titlebar_double_click()`：GPUI 会读取系统设置
/// `AppleActionOnDoubleClick`，按用户选的「缩放 / 最小化 / 填充 / 无操作」
/// 执行，和原生标题栏完全一致（`Fill` 在系统支持时走 `_zoomFill:`，
/// 尊重「平铺窗口留边距」）。这里不能直接调 `zoom_window`，否则会无视
/// 系统设置里的最小化 / 填充选项。
///
/// 之所以要自己转发：窗口开了 `app_owns_titlebar_drag`（整条 64px 顶栏
/// 可拖），AppKit 不再把内容区当系统标题栏，双击也就不会自动生效。
/// 见 main.rs 里 `app_owns_titlebar_drag` 的说明。
pub fn perform_titlebar_double_click(window: &Window) {
    eprintln!("[window] 标题栏双击");
    #[cfg(target_os = "macos")]
    window.titlebar_double_click();
    // Windows 约定：双击标题栏 = 最大化 / 还原。复用窗口按钮那条
    // PostMessage 路径，避免在事件回调里同步进系统循环造成 GPUI 重入。
    #[cfg(target_os = "windows")]
    toggle_maximize(window);
    #[cfg(target_os = "linux")]
    window.zoom_window();
}

/// 按住顶栏拖动窗口。
pub fn begin_window_drag(window: &Window) {
    #[cfg(target_os = "windows")]
    {
        if let Some(hwnd) = window_hwnd(window) {
            unsafe {
                ReleaseCapture();
                // 必须用 PostMessage：SendMessage 会在当前 mouse_down 处理函数里
                // 同步进入系统移动循环，GPUI 的窗口过程被重入，App 的 RefCell
                // 会直接 panic（already borrowed）。Post 出去等这一拍处理完再拖动。
                PostMessageW(hwnd, 0x00A1, 2, 0);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    window.start_window_move();
}

#[cfg(target_os = "windows")]
fn is_window_zoomed(window: &Window) -> bool {
    window_hwnd(window).is_some_and(|hwnd| unsafe { IsZoomed(hwnd) != 0 })
}

#[cfg(target_os = "windows")]
fn toggle_maximize(window: &Window) {
    if let Some(hwnd) = window_hwnd(window) {
        // 同样不能同步 ShowWindow：它会在 on_click 里立刻发 WM_SIZE，
        // GPUI 重入后拿不到新视口，画面会停在旧的 1280x820（右侧留白）。
        let command = if is_window_zoomed(window) {
            0xF120 // SC_RESTORE
        } else {
            0xF030 // SC_MAXIMIZE
        };
        unsafe {
            PostMessageW(hwnd, 0x0112, command, 0);
        }
    }
}

/// 标题栏右侧的最小化 / 最大化（还原）/ 关闭。
#[cfg(target_os = "windows")]
fn window_controls<V: 'static>(
    t: &YkTheme,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let close_hover: Hsla = rgb(0xC42B1C).into();
    let maximize_icon = if is_window_zoomed(window) {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };

    div()
        .flex()
        .items_center()
        .h_full()
        .flex_shrink_0()
        .child(caption_button(
            "win-minimize",
            IconName::WindowMinimize,
            s.fill_3,
            s.text_primary,
            cx,
            |window, _| window.minimize_window(),
        ))
        .child(caption_button(
            "win-maximize",
            maximize_icon,
            s.fill_3,
            s.text_primary,
            cx,
            |window, _| toggle_maximize(window),
        ))
        .child(caption_button(
            "win-close",
            IconName::WindowClose,
            close_hover,
            s.text_primary,
            cx,
            |_, cx| cx.quit(),
        ))
        .into_any_element()
}

#[cfg(target_os = "windows")]
fn caption_button<V: 'static>(
    id: &'static str,
    icon: IconName,
    hover_bg: Hsla,
    icon_color: Hsla,
    cx: &mut Context<V>,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .occlude()
        // 点按钮不能触发顶栏拖动。
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |_view, _: &ClickEvent, window, cx| {
            eprintln!("[window] 标题栏按钮：{id}");
            action(window, cx);
            // 最大化 / 还原后重绘，图标与悬停态立即切过去。
            cx.notify();
            cx.stop_propagation();
        }))
        .hover(move |style| style.bg(hover_bg))
        .child(Icon::new(icon).size(px(13.)).text_color(icon_color))
        .into_any_element()
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn ReleaseCapture() -> i32;
    fn PostMessageW(hwnd: *mut core::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn IsZoomed(hwnd: *mut core::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    // 只精确导入被测项：`use super::*` 会把父模块 `use gpui::*` 带来的
    // `test` 宏一起拉进来，`#[test]` 就会解析成 gpui 的测试宏并递归展开。
    use super::{titlebar_mouse_action, TitlebarMouseAction};

    /// 单击拖动、双击交给平台、三连击忽略 —— 三连击必须落在 Ignore，
    /// 否则会「缩放 → 还原」来回切。
    #[test]
    fn click_count_maps_to_titlebar_action() {
        assert_eq!(titlebar_mouse_action(1), TitlebarMouseAction::Drag);
        assert_eq!(titlebar_mouse_action(2), TitlebarMouseAction::DoubleClick);
        assert_eq!(titlebar_mouse_action(3), TitlebarMouseAction::Ignore);
        assert_eq!(titlebar_mouse_action(0), TitlebarMouseAction::Ignore);
    }
}
