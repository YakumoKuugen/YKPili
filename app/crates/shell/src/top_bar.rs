//! 顶栏：窗口级 chrome（拖动区、窗口按钮、材质胶囊）。
//!
//! 搜索框与头像属于能力/业务内容，由组装点作为槽位传进来；这里只负责
//! 顶栏布局、窗口拖动、Windows 自绘标题栏按钮。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yk_theme::theme::*;

/// 顶栏（`TOP_BAR_HEIGHT`）。
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
        //
        // 分流依据是**顶栏自己记的时间 + 位移**，不是 `event.click_count`：
        // 拖窗（`begin_window_drag`）要 post 一条 `WM_NCLBUTTONDOWN` 给系统
        // 拖动循环，而 GPUI 的窗口过程把「NC 按下」也翻译成一次客户端
        // mouse_down 派发回来（gpui-pre-windows 的 `handle_nc_mouse_down_msg`）。
        // 那条回声带的是我们伪造的坐标，GPUI 的点击计数器于是把它记成「在窗口外
        // 按了一下」——双击序列当场作废，click_count 永远回到 1，双击缩放失效。
        // 详见 `TitlebarClickTracker`。
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            // 按下只记状态：拖动等鼠标移动（`on_mouse_move`），双击等抬起
            // （`on_mouse_up`）。两者都不能在按下时就动手，原因见下面对应注释。
            note_titlebar_press(window, event.position);
        })
        // 双击动作在**抬起**时执行，不能放在按下。
        //
        // 按下阶段系统给窗口 `SetCapture` 了，此时 post 出去的
        // `WM_SYSCOMMAND / SC_MAXIMIZE` 会被直接丢弃（实测：同一条消息在
        // `on_click` —— 也就是抬起阶段 —— 发就正常最大化，在 `on_mouse_down`
        // 发则窗口纹丝不动）。窗口按钮走的就是抬起路径，所以它一直好使。
        .on_mouse_up(MouseButton::Left, |_, window, _| {
            if with_click_tracker(window, |tracker| tracker.take_pending_double_click()) {
                perform_titlebar_double_click(window);
            }
        })
        // 按住后**真正移动**才进系统拖动循环。
        //
        // 不能按下即拖：那样每次单击都会开一个模态移动循环，双击的第二下就
        // 落在第一个循环里——`SC_MAXIMIZE` 刚发出去就被循环收尾时的「落地」
        // 位置覆盖掉，表现就是双击毫无反应（实测：单击 + 双击都能走到
        // `toggle_maximize`，窗口却纹丝不动，位置还被挪了几个像素）。
        .on_mouse_move(|event, window, _| {
            if event.pressed_button == Some(MouseButton::Left)
                && with_click_tracker(window, |tracker| {
                    tracker.drag_should_start(Instant::now(), event.position)
                })
            {
                begin_window_drag(window);
            }
        })
        .flex()
        .items_center()
        // 拖动区独占整条顶栏，窗口按钮靠右：`justify_between` 让「头像胶囊」
        // 贴左、「窗口按钮」贴右。少了它，“贴右边缘”的按钮会紧跟在头像后面
        // 跑到左边去（搜索槽位是绝对定位，不参与这条排布）。
        .justify_between()
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
/// 顶栏同时是拖动区，双击必须和拖动分开处理：第二次按下若继续拖窗，
/// 只会再进一次系统拖动循环，永远走不到双击动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitlebarMouseAction {
    /// 单击：待命，鼠标移动后开始拖动窗口。
    Drag,
    /// 双击：抬起时执行平台原生的标题栏双击动作。
    DoubleClick,
}

/// 一次按下之后多久内再按算双击（与系统默认 500ms 一致）。
const TITLEBAR_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);
/// 两次按下的位置差容差（逻辑像素，对应系统默认的 4px）。
const TITLEBAR_DOUBLE_CLICK_TOLERANCE: f32 = 4.0;
/// 按下后要离开按下点这么远才开始拖窗（逻辑像素）。
///
/// 与双击容差取同一个值：位移一旦超过它，这次按压就不该再算双击，
/// 两个判定口径对齐，免得「既拖了窗又放了缩」。
const TITLEBAR_DRAG_SLOP: f32 = TITLEBAR_DOUBLE_CLICK_TOLERANCE;
/// `WM_NCLBUTTONDOWN` 的「回声」必然在 post 后几毫秒内到达；
/// 超过这个窗口就不再当成回声，免得把用户真的下一次点击吞掉。
const TITLEBAR_DRAG_ECHO_WINDOW: Duration = Duration::from_millis(20);

/// 顶栏双击判定器。
///
/// **为什么不直接用 `MouseDownEvent::click_count`**：顶栏按下要拖窗，而拖动
/// 是把一条 `WM_NCLBUTTONDOWN`（HTCAPTION）post 给系统拖动循环。GPUI 的窗口
/// 过程对「NC 按下」的处理是「转成一次客户端 mouse_down 再派发」
/// （gpui-pre-windows `handle_nc_mouse_down_msg`），而那条消息的 lParam 不会是
/// 真实光标位置，于是 GPUI 的 `ClickState` 会把它记成一次**位置完全不同**的
/// 按下：`current_count` 归 1、`last_position` 被写坏。结果就是双击的第二次
/// 按下永远读到 `click_count == 1`，顶栏双击缩放彻底失效。
///
/// 所以这里自己按「时间 + 位移」判定双击，并显式丢掉那条回声：
/// 回声紧跟在我们 post 之后（几毫秒内），真实点击不可能这么快。
#[derive(Debug, Default)]
struct TitlebarClickTracker {
    /// 上一次**真实**按下的时间与位置；双击判成后清空，拖动开始后也清空。
    ///
    /// 它同时是「拖动锚点」：只有还在这一串按压里，移动才允许开拖。
    last_press: Option<(Instant, Point<Pixels>)>,
    /// 最近一次 post 拖动消息的时间，用来识别接踵而来的回声。
    drag_posted_at: Option<Instant>,
    /// 这一拍按下了「双击」，等抬起时再执行平台动作。
    pending_double_click: bool,
}

impl TitlebarClickTracker {
    /// 记下「刚 post 了拖动消息」，下一次派发回来的同源 mouse_down 会被丢掉。
    fn note_drag_posted(&mut self, now: Instant) {
        self.drag_posted_at = Some(now);
    }

    /// 取走「这次抬起要执行双击动作」的标记。
    ///
    /// 动作放在抬起执行：按下阶段窗口还持着 `SetCapture`，此时 post 的
    /// `SC_MAXIMIZE` 会被系统丢掉。
    fn take_pending_double_click(&mut self) -> bool {
        std::mem::take(&mut self.pending_double_click)
    }

    /// 这次移动要不要开始拖窗。
    ///
    /// 只认「同一串按压 + 离开按下点超过阈值」：判成双击后 `last_press` 已清空，
    /// 所以双击后的移动、拖动消息的回声都不会开拖；开拖时同样清空锚点，
    /// 一次按压最多开一次拖动循环。
    fn drag_should_start(&mut self, now: Instant, position: Point<Pixels>) -> bool {
        if self.pending_double_click {
            return false;
        }
        let Some((at, start)) = self.last_press else {
            return false;
        };
        // 按下太久还按着（比如按住不动发呆）：不再当成拖动。
        if now.saturating_duration_since(at) > TITLEBAR_DOUBLE_CLICK_WINDOW {
            self.last_press = None;
            return false;
        }
        let moved = (position.x.as_f32() - start.x.as_f32()).abs() > TITLEBAR_DRAG_SLOP
            || (position.y.as_f32() - start.y.as_f32()).abs() > TITLEBAR_DRAG_SLOP;
        if moved {
            // 已经开始拖了，这串按压不能再回头拼成双击，也不能再开第二次拖动。
            self.last_press = None;
            self.pending_double_click = false;
        }
        moved
    }

    /// 一次 `mouse_down` 的判定结果。
    ///
    /// `None` = 这是拖动消息的回声，调用方应整颗忽略。
    fn register(&mut self, now: Instant, position: Point<Pixels>) -> Option<TitlebarMouseAction> {
        if let Some(posted_at) = self.drag_posted_at.take() {
            if now.saturating_duration_since(posted_at) < TITLEBAR_DRAG_ECHO_WINDOW {
                // 回声是系统复读回来的：既不算点击，也不能当拖动锚点。
                self.last_press = None;
                return None;
            }
        }

        let is_double = self.last_press.is_some_and(|(at, last)| {
            now.saturating_duration_since(at) < TITLEBAR_DOUBLE_CLICK_WINDOW
                && (last.x.as_f32() - position.x.as_f32()).abs() <= TITLEBAR_DOUBLE_CLICK_TOLERANCE
                && (last.y.as_f32() - position.y.as_f32()).abs() <= TITLEBAR_DOUBLE_CLICK_TOLERANCE
        });

        // 每次按下都重置上一拍的标记，避免残留。
        self.pending_double_click = false;
        self.last_press = Some((now, position));
        if is_double {
            // 双击后清空基准：紧接着的第三下从头算单击，不会「缩放 → 还原」
            // 来回切（第三下没有位移就不会真的拖窗）。
            self.last_press = None;
            self.pending_double_click = true;
            Some(TitlebarMouseAction::DoubleClick)
        } else {
            Some(TitlebarMouseAction::Drag)
        }
    }
}

thread_local! {
    /// 每个窗口一份双击判定状态：窗口很多也不串位置，窗口关掉也不会串到新窗口。
    static TITLEBAR_CLICKS: RefCell<HashMap<u64, TitlebarClickTracker>> =
        RefCell::new(HashMap::new());
}

fn with_click_tracker<R>(window: &Window, f: impl FnOnce(&mut TitlebarClickTracker) -> R) -> R {
    let id = window.window_handle().window_id().as_u64();
    TITLEBAR_CLICKS.with(|trackers| {
        let mut trackers = trackers.borrow_mut();
        f(trackers.entry(id).or_default())
    })
}

/// 记下一次顶栏左键按下（判定它属于单击还是双击都在追踪器里）。
///
/// 返回值只给单测用：拖动消息的回声返回 `None`，其余返回本次判定的动作。
fn titlebar_mouse_action(window: &Window, position: Point<Pixels>) -> Option<TitlebarMouseAction> {
    with_click_tracker(window, |tracker| tracker.register(Instant::now(), position))
}

/// 按下时的唯一入口：只更新追踪器状态，具体动作交给移动 / 抬起。
fn note_titlebar_press(window: &Window, position: Point<Pixels>) {
    titlebar_mouse_action(window, position);
}

/// 双击标题栏：交给平台执行「连按标题栏」的默认动作。
///
/// **必须在鼠标抬起时调用**：按下阶段系统给窗口 `SetCapture` 了，此时 post
/// 的 `SC_MAXIMIZE` 会被系统丢弃（见 `top_bar` 里 `on_mouse_up` 的说明）。
///
/// macOS 走 `Window::titlebar_double_click()`：GPUI 会读取系统设置
/// `AppleActionOnDoubleClick`，按用户选的「缩放 / 最小化 / 填充 / 无操作」
/// 执行，和原生标题栏完全一致（`Fill` 在系统支持时走 `_zoomFill:`，
/// 尊重「平铺窗口留边距」）。这里不能直接调 `zoom_window`，否则会无视
/// 系统设置里的最小化 / 填充选项。
///
/// 之所以要自己转发：窗口开了 `app_owns_titlebar_drag`（整条顶栏可拖），
/// AppKit 不再把内容区当系统标题栏，双击也就不会自动生效。
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
            // 先记时间：这条消息稍后被 GPUI 翻译成的「客户端 mouse_down」是回声，
            // 判定双击时要丢掉（见 `TitlebarClickTracker`）。
            with_click_tracker(window, |tracker| tracker.note_drag_posted(Instant::now()));
            unsafe {
                ReleaseCapture();
                // 必须用 PostMessage：SendMessage 会在当前 mouse_down 处理函数里
                // 同步进入系统移动循环，GPUI 的窗口过程被重入，App 的 RefCell
                // 会直接 panic（already borrowed）。Post 出去等这一拍处理完再拖动。
                //
                // lParam 要给真实光标位置（屏幕坐标）：系统拖动循环拿它当锚点，
                // 传 0 会把锚点定在屏幕原点，窗口按下去先跳一下，贴边还会被
                // Aero Snap 直接最大化。
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                let anchor = ((cursor.y as isize) << 16) | (cursor.x as isize & 0xFFFF);
                PostMessageW(hwnd, 0x00A1, 2, anchor);
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
#[repr(C)]
#[derive(Default)]
struct POINT {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn ReleaseCapture() -> i32;
    fn PostMessageW(hwnd: *mut core::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn IsZoomed(hwnd: *mut core::ffi::c_void) -> i32;
    fn GetCursorPos(point: *mut POINT) -> i32;
}

#[cfg(test)]
mod tests {
    // 只精确导入被测项：`use super::*` 会把父模块 `use gpui::*` 带来的
    // `test` 宏一起拉进来，`#[test]` 就会解析成 gpui 的测试宏并递归展开。
    use super::{
        TitlebarClickTracker, TitlebarMouseAction, TITLEBAR_DOUBLE_CLICK_WINDOW,
        TITLEBAR_DRAG_ECHO_WINDOW,
    };
    use gpui::{point, px};
    use std::time::{Duration, Instant};

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    /// 单击待命拖动、双击留给平台；停一下再双击仍要能连续生效
    /// （双击后不吞下一拍，否则「最大化 → 还原」的第二次双击会被吃掉）。
    #[test]
    fn click_sequence_maps_to_titlebar_action() {
        let t0 = Instant::now();
        let mut tracker = TitlebarClickTracker::default();
        let p = point(px(200.), px(16.));

        assert_eq!(
            tracker.register(at(t0, 0), p),
            Some(TitlebarMouseAction::Drag)
        );
        assert_eq!(
            tracker.register(at(t0, 120), p),
            Some(TitlebarMouseAction::DoubleClick)
        );
        // 双击后来的第三下：从头算单击，不会「缩放 → 还原」来回切。
        assert_eq!(
            tracker.register(at(t0, 200), p),
            Some(TitlebarMouseAction::Drag)
        );

        // 停顿之后再双击，必须能再触发一次（最大化 → 还原）。
        assert_eq!(
            tracker.register(at(t0, 1400), p),
            Some(TitlebarMouseAction::Drag)
        );
        assert_eq!(
            tracker.register(at(t0, 1500), p),
            Some(TitlebarMouseAction::DoubleClick)
        );
    }

    /// 两次按下间隔超过双击窗口 → 还是两次独立单击（拖动），不能误缩放。
    #[test]
    fn slow_second_click_is_still_a_drag() {
        let t0 = Instant::now();
        let mut tracker = TitlebarClickTracker::default();
        let p = point(px(200.), px(16.));
        let slow = u64::try_from(TITLEBAR_DOUBLE_CLICK_WINDOW.as_millis()).unwrap() + 50;

        assert_eq!(
            tracker.register(at(t0, 0), p),
            Some(TitlebarMouseAction::Drag)
        );
        assert_eq!(
            tracker.register(at(t0, slow), p),
            Some(TitlebarMouseAction::Drag)
        );
    }

    /// 两次按下位置差超过容差（比如先点头像附近再点中间）→ 不算双击。
    #[test]
    fn distant_second_click_is_still_a_drag() {
        let t0 = Instant::now();
        let mut tracker = TitlebarClickTracker::default();

        assert_eq!(
            tracker.register(at(t0, 0), point(px(40.), px(16.))),
            Some(TitlebarMouseAction::Drag)
        );
        assert_eq!(
            tracker.register(at(t0, 100), point(px(400.), px(16.))),
            Some(TitlebarMouseAction::Drag)
        );
    }

    /// 单击后要离开按下点超过阈值才允许开拖；开拖一次就清锚点，
    /// 双击 / 回声都不许开拖。
    #[test]
    fn only_a_moved_single_click_starts_a_drag() {
        let t0 = Instant::now();
        let p = point(px(200.), px(16.));
        let mut tracker = TitlebarClickTracker::default();

        tracker.register(at(t0, 0), p);
        assert!(
            !tracker.drag_should_start(at(t0, 10), point(px(202.), px(16.))),
            "抖动不算拖"
        );
        assert!(
            !tracker.drag_should_start(at(t0, 20), point(px(200.), px(16.))),
            "原地不算拖"
        );
        assert!(
            tracker.drag_should_start(at(t0, 40), point(px(200.), px(40.))),
            "离开阈值才开拖"
        );
        assert!(
            !tracker.drag_should_start(at(t0, 60), point(px(200.), px(80.))),
            "一次按压只开一次拖动"
        );
        assert!(!tracker.take_pending_double_click(), "拖动之后不该有双击");

        // 按住发呆太久不再当成拖动。
        let mut tracker = TitlebarClickTracker::default();
        tracker.register(at(t0, 0), p);
        let stale = u64::try_from(TITLEBAR_DOUBLE_CLICK_WINDOW.as_millis()).unwrap() + 100;
        assert!(!tracker.drag_should_start(at(t0, stale), point(px(200.), px(80.))));

        // 双击不能开拖，并且要留下「抬起时执行动作」的标记。
        let mut tracker = TitlebarClickTracker::default();
        tracker.register(at(t0, 0), p);
        assert!(!tracker.take_pending_double_click(), "单击没有双击动作");
        tracker.register(at(t0, 120), p);
        assert!(tracker.take_pending_double_click(), "第二下是双击");
        assert!(
            !tracker.drag_should_start(at(t0, 130), point(px(200.), px(80.))),
            "双击不拖窗"
        );

        // 拖动消息的回声同样不能当锚点。
        let mut tracker = TitlebarClickTracker::default();
        tracker.register(at(t0, 0), p);
        tracker.note_drag_posted(at(t0, 1));
        assert_eq!(tracker.register(at(t0, 2), p), None);
        assert!(
            !tracker.drag_should_start(at(t0, 30), point(px(200.), px(80.))),
            "回声不开拖"
        );
    }

    /// 拖动消息的回声必须被丢掉：它是我们 post 的 WM_NCLBUTTONDOWN 被 GPUI
    /// 翻译回来的那一次 mouse_down。不丢的话，一次单击就会被判成双击。
    #[test]
    fn drag_echo_is_swallowed() {
        let t0 = Instant::now();
        let mut tracker = TitlebarClickTracker::default();
        let p = point(px(200.), px(16.));

        assert_eq!(
            tracker.register(at(t0, 0), p),
            Some(TitlebarMouseAction::Drag)
        );
        tracker.note_drag_posted(at(t0, 1));
        assert_eq!(tracker.register(at(t0, 2), p), None, "同源回声要丢掉");

        // 回声之后按下要从单击重新算：回声只可能出现在「已经开拖」之后，
        // 拖动前的按下不该和拖动后的按下拼成一次双击。
        assert_eq!(
            tracker.register(at(t0, 140), p),
            Some(TitlebarMouseAction::Drag)
        );
    }

    /// 回声迟迟没来（比如落到了会 `stop_propagation` 的子控件上）时，
    /// 不能把用户下一次真实点击吞掉。
    #[test]
    fn stale_echo_marker_does_not_eat_real_click() {
        let t0 = Instant::now();
        let mut tracker = TitlebarClickTracker::default();
        let stale = u64::try_from(TITLEBAR_DRAG_ECHO_WINDOW.as_millis()).unwrap() + 80;

        assert_eq!(
            tracker.register(at(t0, 0), point(px(200.), px(16.))),
            Some(TitlebarMouseAction::Drag)
        );
        tracker.note_drag_posted(at(t0, 1));
        // 换个位置按，确保判定与「双击」无关，只看这颗按下有没有被吞。
        assert_eq!(
            tracker.register(at(t0, stale), point(px(600.), px(16.))),
            Some(TitlebarMouseAction::Drag),
            "超过回声窗口就是真实点击"
        );
    }
}
