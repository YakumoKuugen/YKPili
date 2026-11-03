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
        .on_mouse_down(MouseButton::Left, |_, window, _| begin_window_drag(window))
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
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
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
        .p(px(6.))
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
