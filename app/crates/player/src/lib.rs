//! 播放浮窗（M3）。
//!
//! 绘制顺序 = 这个文件里 child 的排列顺序（GPUI 没有 z-index）：
//!
//! ```text
//! ⓪ 浮窗外侧点击层    topbar 之下、整页      点击关闭（顶栏保持可用）
//! ① 浮窗 600 高       顶栏下方 16px          圆角 + shadow_glass_modal
//!    ⓪ 入场          封面 bounds → 目标框    280ms 单段曲线 + 轻回弹
//!    ①a 视频 surface   红框：窗口−侧栏200    Contain 在红框里居中
//!    ①b 弹幕层 / 取流状态
//!    ①c 控制栏 88px     bottom；right 200   宽度 = 播放层（贴侧栏左缘）
//!    ①d 信息面板       展开 400 / 收起 200    贴窗口右边界、通高、常驻
//!    ①e 骑边界按钮     骑在面板左边界
//!    ①f Esc 提示       第一次 Esc 显示
//! ```
//!
//! **面板 / 播放层（2026-10-25 用户调整）**：
//! 右侧是侧栏**常驻占用**的区域（收起 200 / 展开 400，贴窗口右边界、top:0 → bottom:0 通高），
//! 它不属于播放层。播放层（红框）宽度 = 窗口宽 − 200，视频在该红框内居中。
//! 控制栏宽度 = 播放层宽度（窗口 − 200），右端贴侧栏左边缘。
//! 侧栏收起时，鼠标进入侧栏（200px）即展开；视频右缘不铺 hover 热区，
//! 避免在画面上移动鼠标时误触发展开。展开 / 收起仍可点骑边界按钮。
//! 播放器底边在 Dock 上方 20px（距窗口底部 92），宽度/高度随窗口缩放。
//!
//! **控制栏自动隐藏**：起播、窗口失焦、有焦点但鼠标 5s 不动时隐藏；
//! 鼠标移动 / 点击后重新浮出。隐藏时只剩底部 3px 进度线。

mod controller;
pub mod model;
mod view;

pub mod comment_section;
pub mod comment_view;
pub mod video;
pub mod video_worker;

use gpui::assets::IconName;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::kbd::Kbd;
use gpui::component::menu::ContextMenuExt as _;
use gpui::component::slider::{Slider, SliderState};
use gpui::component::Icon;
use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{FocusHandle, InteractiveElement, StatefulInteractiveElement, *};
use yk_domain::danmaku::{Danmaku, DanmakuMode, DanmakuSet, LaneAllocator};
use yk_infra::dynamic::{asset_url, fitted_width, CommentTarget, DynamicComment};
use yk_domain::format;
use controller::dispatch_history_report;
pub use controller::set_test_cursor;
use model::PlayerModel;
use view::{ActionOverlay, VolumePanelView};
use yk_domain::player::PlayerState;
use yk_components::status;
use yk_theme::theme::*;

use crate::comment_view::text_copy_menu;

use crate::video::TestFrame;

/// 当前帧的类型按平台分派：macOS 是 IOSurface-backed `CVPixelBuffer`，
/// Windows 是 mpv 直接出的 BGRA 缓冲区。两者都只经 `video_worker` 产出。
#[cfg(target_os = "macos")]
type CurrentFrame = objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>;
#[cfg(target_os = "windows")]
type CurrentFrame = crate::video_worker::VideoFrame;

/// 真实视频源（libmpv 解一帧）。拿到就用它，拿不到退回测试帧。
#[cfg(target_os = "macos")]
pub struct MpvFrameSource {
    player: yk_infra::media::MpvPlayer,
    buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    tick: u64,
}

#[cfg(target_os = "macos")]
impl MpvFrameSource {
    /// 用 libmpv 解一帧并转成 CVPixelBuffer。失败时返回 None，调用方退回测试帧。
    pub fn open(video: &str, w: u32, h: u32) -> Option<Self> {
        let mut mpv = yk_infra::media::MpvPlayer::new(w, h).ok()?;
        mpv.load_file(video).ok()?;
        // 给解码器一点时间产出首帧
        std::thread::sleep(std::time::Duration::from_millis(350));
        let rgba = mpv.render_frame().ok()?;
        let nv12 = yk_infra::media::convert::rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
        Some(Self {
            player: mpv,
            buffer: yk_infra::media::pixel_buffer::nv12_to_pixel_buffer(&nv12),
            tick: 0,
        })
    }

    /// 取一帧最新画面。播放循环每帧调一次。
    ///
    /// 每次都重新建 CVPixelBuffer —— 验证阶段够用，但它是一帧一次分配，
    /// 真机上应该改成「复用 buffer + 只更新两个平面」。
    /// 取一帧最新画面，返回当前播放位置（秒）。播放循环每帧调一次。
    pub fn pump(&mut self) -> f64 {
        let t0 = std::time::Instant::now();
        self.player.drain_events();
        let t = self.player.get_property_double("time-pos").unwrap_or(0.0);
        // 诊断：时间轴到底有没有在走
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 60 == 1 {
            let t = self.player.get_property_double("time-pos").unwrap_or(-1.0);
            let aid = self.player.get_property_double("aid").unwrap_or(-1.0);
            let vol = self.player.get_property_double("volume").unwrap_or(-1.0);
            eprintln!(
                "[mpv] time-pos={t:.3}s aid={aid} volume={vol} tick={}",
                self.tick
            );
        }
        if let Ok(rgba) = self.player.render_frame() {
            let nv12 = yk_infra::media::convert::rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
            self.buffer = yk_infra::media::pixel_buffer::nv12_to_pixel_buffer(&nv12);
        }
        let cost = t0.elapsed();
        if cost.as_millis() >= 8 {
            eprintln!(
                "[player] pump 耗时 {}ms（>8ms 会拖慢 UI）",
                cost.as_millis()
            );
        }
        t
    }

    pub fn buffer(&self) -> objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer> {
        self.buffer.clone()
    }
}

/// 控制栏总高（唯一口径）。
const CTRL_H: f32 = PLAYER_CONTROL_BAR_HEIGHT; // 88
/// 弹幕输入条高。
const DANMAKU_INPUT_H: f32 = 38.0;
const INFO_PANEL_EXPANDED: f32 = PLAYER_INFO_PANEL_WIDTH; // 400
const INFO_PANEL_COLLAPSED: f32 = PLAYER_INFO_PANEL_COLLAPSED_WIDTH; // 200
/// 收藏夹面板一屏先摆几行：再长就先折叠，点「展开全部」再看。
/// 8 × 32 + 头尾 ≈ 330，与 §13.3「容器最大高 320」同一量级。
const FAV_PANEL_VISIBLE_ROWS: usize = 8;
/// 评论输入条收起态的圆形按钮直径。
const COMMENT_COMPOSER_FAB: f32 = 40.0;
/// 评论输入条高度：收起 / 展开同高，展开只是变宽。
const COMMENT_COMPOSER_HEIGHT: f32 = 40.0;
/// 输入条被「回到顶部」按钮顶起的距离：按钮 34 + 间距 8。
const COMMENT_COMPOSER_LIFT: f32 = 34.0 + SPACE_2;
/// 输入条展开 / 收起动画时长。
const COMMENT_COMPOSER_TOGGLE_MS: u64 = 180;
/// 「回到顶部」浮出 / 收起时，输入条让位 / 归位的时长。
const COMMENT_COMPOSER_LIFT_MS: u64 = 200;
/// 滚动结束后这段时间内不响应面板收起，避免滚轮末尾的 hover 抖动。
const PANEL_SCROLL_HOLD_MS: u64 = 400;
/// 互动 toast 从弹出到开始退场的时长（含入场动画，不含退场动画）。
const TOAST_VISIBLE_MS: u64 = 2600;
/// 一段弹幕覆盖的时长（B 站 `seg.so` 的分段粒度是 6 分钟，改不了）。
const DANMAKU_SEGMENT_SECS: f64 = 360.0;
/// 提前量：播到「已加载区间」末尾前 60 秒就去拉下一段，
/// 用户感知就是「看多少拉多少」，不会等画面到了才补。
const DANMAKU_PREFETCH_SECS: f64 = 60.0;
/// 互动 toast 缩放动画的起始比例：入场 88% → 100%，退场反过来。
const TOAST_SCALE_FROM: f32 = 0.88;

/// 播放器下边界距窗口底部的距离。
///
/// Dock 高 52（40 + 6×2）+ 下边距 20 = 顶部在 72；播放器底边再留 20px 呼吸：
/// 72 + 20 = 92。窗口缩放时播放器跟随拉伸，不再固定 600 高。
const PLAYER_BOTTOM_INSET: f32 = 92.0;
/// 从封面展开：一条连续曲线走完，避免两段缓动在交界处出现速度断点。
/// 280ms 向 macOS Quick Look 的快速预览节奏靠拢，尾部只留很短的过冲回弹。
const PLAYER_ENTRANCE_MS: u64 = 280;
/// 入场后段开始把「封面海报」交叉淡化成交互界面。
const ENTRANCE_CONTENT_FADE_START: f32 = 0.58;
/// 播放器外侧的页面遮罩最终不透明度（0 = 不遮，1 = 全黑）。
const PLAYER_SCRIM_OPACITY: f32 = 0.62;
/// 关闭缩回封面；Quick Look 收回通常比展开更短，这里 260ms。
const PLAYER_EXIT_MS: u64 = 260;

/// 点击卡片时冻结的封面几何与视觉信息。
///
/// 坐标已经换算成 AppRoot 页面坐标；`root_size` 用来在播放器窗口里
/// 重新算目标尺寸，窗口 resize 时不会用旧宽度。
#[derive(Clone)]
pub struct CoverSource {
    pub bounds: Bounds<Pixels>,
    pub root_size: Size<Pixels>,
    pub bvid: String,
    pub title: String,
    pub image_url: String,
    pub placeholder_color: String,
}

/// 一次播放的观看历史上报上下文。
///
/// 只在登录态下创建；`cookie` / `csrf` 在开始时冻结，避免中途退出登录
/// 把上报发到空会话上。
#[derive(Clone)]
struct HistoryReport {
    aid: i64,
    bvid: String,
    cid: i64,
    cookie: String,
    csrf: String,
    /// 本轮打开时刻（单调时钟，用于 5 秒冷却）。
    opened_at: std::time::Instant,
}

/// 信息面板「评论」Tab 的加载状态。
enum CommentState {
    /// 还没有播放源（取流中 / 失败），不显示评论态。
    Idle,
    Loading,
    /// 首批已经回来；具体条目在 `PlayerView::comments_list.items` 里。
    Ready,
    Error(String),
}

/// 评论一次「加载更多」要多少条 —— **只发 1 个请求**。
///
/// 旧实现一批 100 条，等于在 1 秒内连打 4 个 `/x/v2/reply/main`
/// （服务端单页上限 30），正好撞上评论区频控 → `-352`。现在一次只要
/// 1 页 20 条（与网页端一致），其余等用户滚到底部再取。
const COMMENT_PAGE_SIZE: u32 = yk_infra::api::client::COMMENT_PAGE_SIZE;

/// 两次评论请求之间的最小间隔。
///
/// 「一次 20 条」解决单次过大，「1 秒 1 次」解决连续触发：窗口化列表在
/// 滚到底后会反复判定「该补一页」，间隔压不住的话仍然会被当成脚本。
const COMMENT_REQUEST_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1000);

/// 评论行里除正文之外的高度：上下 padding 16 + 作者行 16 + 两个 2px 间距
/// + 时间行 14。正文另算：行数 × [`COMMENT_TEXT_LINE_HEIGHT`]。
const COMMENT_ROW_CHROME: f32 = 50.0;
/// 虚拟滚动时在可见区上下各多画这么多像素。
///
/// 评论行的行高由我们自己算、并且渲染时强制同高，所以偏移是精确的；
/// 多画一段只是为了让滚动过程中不会出现「一帧空白」。
const COMMENT_OVERSCAN: f32 = 1200.0;

/// 窗口可见性状态机：失焦 ≠ 不可见。
///
/// - **失焦**（点了别的应用）：窗口还在屏幕上，视频必须照常按标准帧率播。
///   GPUI 默认会把失焦窗口的帧率压到 30fps，这里在 `main.rs` 用
///   `inactive_frame_interval: None` 关掉。
/// - **不可见**（最小化 / 被完全遮挡）：画面没人看，视频线程只报播放位置、
///   不出帧；页面侧的动画与补帧也一并停掉。
///
/// 抽成独立结构是为了两边（AppRoot / PlayerView）用同一套判定，
/// 并且能脱离 GPUI 单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibilityGate {
    visible: bool,
}

impl Default for VisibilityGate {
    fn default() -> Self {
        Self { visible: true }
    }
}

impl VisibilityGate {
    /// 记录新的可见性；返回「是否发生了变化」。
    pub fn set(&mut self, visible: bool) -> bool {
        if self.visible == visible {
            return false;
        }
        self.visible = visible;
        true
    }

    /// 当前是否可见。
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// 视频线程是否该停帧（不可见时停）。
    pub fn render_paused(&self) -> bool {
        !self.visible
    }
}

/// 弹幕数据的加载状态。
enum DanmakuState {
    /// 还没拿到 cid（取流中 / 失败）。
    Idle,
    Loading,
    Ready,
    Error(String),
}

/// 一条弹幕在屏幕上的排布方式。
#[derive(Debug, Clone, Copy)]
enum DanmakuLane {
    /// 滚动弹幕的轨道号。
    Scroll(usize),
    /// 顶部固定弹幕的槽位号（0 = 最上面）。
    Top(usize),
    /// 底部固定弹幕的槽位号（0 = 最下面）。
    Bottom(usize),
}

/// 一条已经排好屏幕位置的弹幕。
struct PlacedDanmaku {
    data: Danmaku,
    lane: DanmakuLane,
}

/// 屏幕最多同时排布的滚动轨道数。
///
/// 真实弹幕里同一秒可以来几十条；轨道无限增长会把字画到屏幕外，
/// 超出部分仍然进「弹幕」Tab，只是不上屏。
/// 滚动弹幕的轨道上限（真实条数按播放区高度动态算）。
const MAX_SCROLL_LANES: usize = 32;
/// 每条滚动弹幕占的行高。
const SCROLL_LANE_HEIGHT: f32 = 30.0;

/// 按播放区实际高度算能放几条轨道。
///
/// 播放浮窗高度 = 窗口高 − 顶栏下方 16 − 底部 92；弹幕从 y=8 开始排，
/// 每条占 30px。旧实现写死 14 条，在 820 高的窗口里只够铺到画面中部。
fn scroll_lane_capacity(window: &Window, fullscreen: bool) -> usize {
    let chrome = if fullscreen {
        0.0
    } else {
        TOP_BAR_HEIGHT + SPACE_4 + PLAYER_BOTTOM_INSET
    };
    let area_height = (window.viewport_size().height.as_f32() - chrome).max(120.0);
    (((area_height - 8.0) / SCROLL_LANE_HEIGHT).floor() as usize).clamp(8, MAX_SCROLL_LANES)
}
/// 顶部 / 底部固定弹幕各自的槽位数。
const FIXED_LANES: usize = 5;
/// 顶部 / 底部固定弹幕的停留时长（秒）。
const FIXED_DANMAKU_DURATION: f64 = 4.0;
/// 滚动弹幕最长可能飞多久（秒）——用于每帧向前取候选弹幕的窗口。
const SCROLL_LOOKBACK: f64 = 24.0;

/// 播放层宽度：窗口宽 − 左右各 `SPACE_4` 留白 − 当前右侧面板宽度。
///
/// 弹幕的 x 起点、可见时长都按这个宽度算；写死 1280 会在窗口缩放后
/// 让弹幕从面板底下冒出来，或在右侧留一条空白带。面板展开到 400 时
/// 弹幕区也跟着缩到「视频宽 − 400」（设计第 14 节）。
fn playback_layer_width(window: &Window, panel_width: f32) -> f32 {
    (window.viewport_size().width.as_f32() - 2.0 * SPACE_4 - panel_width).max(1.0)
}

pub struct PlayerView {
    theme: YkTheme,
    frame: TestFrame,
    /// 视频渲染线程（解码与转换都在它上面，UI 线程只收帧）
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    worker: Option<crate::video_worker::FrameWorker>,
    /// 最近一帧（由 worker 产出）
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    current: Option<CurrentFrame>,
    /// Windows：当前帧对应的 GPUI 贴图。换帧时先 `drop_image` 旧的，
    /// 否则每个帧号都会在图集里留一张纹理，几分钟就把显存吃满。
    #[cfg(target_os = "windows")]
    current_image: Option<std::sync::Arc<RenderImage>>,
    /// Windows：当前贴图对应的帧号。
    #[cfg(target_os = "windows")]
    current_image_serial: Option<u64>,
    /// Windows：换源时下一帧 render 把旧贴图从图集里摘掉。
    #[cfg(target_os = "windows")]
    needs_image_clear: bool,
    /// 播放循环是否已启动（worker 创建后同帧启动一次，不能等下一次 render）
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    loop_started: bool,
    /// 面板是否展开（hover 唤起）。
    expanded: bool,
    /// 截图 / 测试：即使窗口未激活也保持控制栏可见。
    ///
    /// 正常运行时窗口失焦会主动收起控制栏；headless 截图的窗口永远拿不到
    /// 系统焦点，没有这个开关就拍不到控制栏和它上面的画质 / 倍速菜单。
    force_controls_visible: bool,
    /// 最近一次切换全屏的时间。
    ///
    /// 系统全屏有几百毫秒动画，动画期间 `window.is_fullscreen()` 仍是旧值；
    /// 用这个时间戳做短暂宽限，避免刚进入全屏就被同步逻辑改回窗口态。
    fullscreen_changed_at: Option<std::time::Instant>,
    /// 当前 Tab：0 评论 / 1 弹幕 / 2 选集。
    tab: usize,
    /// 控制栏显隐、音量、错误统一收在视图模型里，视图与控制器共读这一份。
    model: PlayerModel,
    /// 音量弹出条是否展开。点控制栏音量图标切换，点外部 / 控制栏收起时关闭。
    volume_popover_open: bool,
    /// 音量弹出条（独立实体，拖滑杆只重绘这一块）。
    volume_panel: Entity<VolumePanelView>,
    /// 音量图标按钮的 bounds：点它属于「切换弹出条」，不能被外部点击关闭逻辑误判。
    volume_button_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 音量弹出条的 bounds：点它外部才收起。
    volume_popover_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 画质菜单是否展开。
    quality_popover_open: bool,
    /// 倍速菜单是否展开。
    speed_popover_open: bool,
    /// 正在按新清晰度换流；期间菜单置灰，避免连续点不同档位。
    quality_switching: bool,
    /// 换流代际：换视频 / 连续换档时只接受最后一次结果。
    quality_generation: u64,
    /// 画质 / 倍速胶囊的 bounds：点胶囊本身属于「切换菜单」，不能被外部点击关掉。
    quality_button_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    speed_button_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 当前展开的画质 / 倍速菜单 bounds（两个菜单同时只会开一个）。
    control_popover_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 弹幕：屏幕上已排好位置的滚动 / 顶部 / 底部弹幕。
    danmaku: Vec<PlacedDanmaku>,
    /// 当前视频的全部弹幕（按时间排序），信息面板「弹幕」Tab 用它。
    danmaku_all: Vec<Danmaku>,
    /// 已拉到的原始弹幕（按段累积），重排轨道时以它为准。
    danmaku_source: Vec<Danmaku>,
    danmaku_cid: i64,
    danmaku_aid: i64,
    /// 下一段要拉的序号（1 起；第 n 段覆盖 [(n-1)*360, n*360)）。
    danmaku_next_segment: u64,
    danmaku_segment_loading: bool,
    danmaku_segment_done: bool,
    /// 拉取失败后的退避时间，避免每帧重试。
    danmaku_retry_at: Option<std::time::Instant>,
    /// 弹幕加载状态（换源后重置）。
    danmaku_state: DanmakuState,
    /// 弹幕请求代次：换源后旧请求的结果直接丢弃。
    danmaku_generation: u64,
    lanes: LaneAllocator,
    /// 当前播放区实际能放下的滚动弹幕轨道数（按窗口高度算）。
    scroll_lanes: usize,
    /// 最近一次从 mpv 读到的播放位置（秒），弹幕位置靠它算
    play_time: f64,
    /// 弹幕输入框状态（首次 render 时创建，那时才有 window）
    danmaku_input: Option<Entity<InputState>>,
    /// 弹幕发送中：`/x/v2/dm/post` 还没回来之前不接受第二次提交。
    danmaku_sending: bool,
    /// 输入框按了 Enter（`InputEvent::PressEnter`），下一轮 render 里发送。
    /// 订阅回调拿不到 `Window`，而清空输入框需要，所以用 defer 转一手。
    submit_requested: bool,
    /// 是否已尝试启动渲染线程（换源时重置）
    tried_worker: bool,
    /// 当前卡片的播放源；None = 还没点卡片 / 正在请求。
    source: Option<yk_domain::playback::PlaybackSource>,
    /// 正在后台取播放地址。
    loading_source: bool,
    /// 当前视频的上报上下文；未登录或非投稿视频为 None。
    history_report: Option<HistoryReport>,
    /// 取流代次：连续点不同卡片时只接受最后一次结果。
    load_generation: u64,
    /// 输入框是否处于聚焦。聚焦时暂停渲染循环 ——
    /// 因为 pump() 在 UI 线程要做 10-25ms 的转换，会把输入事件和音频都拖卡。
    input_focused: bool,
    /// 浮窗根焦点：Esc 两次关闭。
    focus_handle: FocusHandle,
    /// 下一次 render 把焦点交给浮窗根（打开 / 换源时置位）。
    needs_focus: bool,
    /// 第一次 Esc 后显示「再次按 Esc 关闭」。
    escape_hint: bool,
    /// 提示代际：5 秒自动收回 / 关闭 / 换源时递增，旧定时器不能误关新提示。
    escape_hint_generation: u64,
    /// 点封面打开时的起始几何；None = 程序化打开（YK_BVID / YK_VIDEO）。
    entrance_source: Option<CoverSource>,
    /// 入场动画代际：只换 id，不在 render 里生成新时钟。
    entrance_generation: u64,
    /// 正在播放关闭缩回动画（此期间不再响应第二次关闭）。
    closing: bool,
    /// 关闭动画代际：旧定时器不能结束新一次关闭。
    closing_generation: u64,
    /// 播放结束是否已经处理过。
    ///
    /// 播放循环每 16ms 跑一次，而结束信号会一直为真；没有这道闸门，
    /// 关闭请求会被重复提交。换源时复位。
    finish_handled: bool,
    /// 关闭回调：`false` = 开始缩回，`true` = 缩回结束可移除实体。
    /// AppRoot 在开始时先恢复分类条/网格，让卡片回到封面快照的坐标。
    close_handler: Option<Box<dyn Fn(&mut App, bool)>>,
    /// 点信息面板的 UP 主头像：请 AppRoot 打开 UP 主浮窗（mid / 昵称 / 头像）。
    up_profile_handler:
        Option<Box<dyn Fn(&mut App, &mut Window, i64, String, String, Bounds<Pixels>)>>,
    /// 信息面板头像的屏幕 bounds（canvas 探针每帧写），给 UP 主浮窗做入场锚点。
    up_avatar_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// 本地续播记录（每个视频上次看到的位置）。
    resume: yk_infra::resume::ResumeStore,
    /// 打开时由「观看历史」带进来的起播位置（服务端进度）。
    /// 与本地记录一起交给 `yk_infra::resume::resolve_start_position` 取更靠后的。
    resume_seed: Option<f64>,
    /// 本次播放的实际起播位置（取流成功后算出，0 = 从头播）。
    start_position: f64,
    /// 当前视频的续播记录键（`bvid#cid`）。
    resume_key: Option<String>,
    /// 上次写续播记录时的播放位置，用来按间隔落盘。
    last_resume_save: f64,
    /// 信息面板标签（标签接口异步填充；失败时用分区名兜底）。
    tags: Vec<String>,
    /// 评论加载状态（打开视频后异步拉首批）。
    comments: CommentState,
    /// 评论区（列表 / 翻页 / 输入条 / 回到顶部），与动态详情共用同一份实现。
    comment_section: crate::comment_section::Section,
    /// 元数据请求代次：换源后旧请求的结果直接丢弃。
    meta_generation: u64,
    /// 左下角眼睛按钮：关掉后不再绘制弹幕层。
    danmaku_visible: bool,
    /// 关注按钮：服务端关系状态（已关注 / 未关注）。
    followed: bool,
    /// 是否已经从服务端读到过关注状态。没读到之前点按钮先查一次再写，
    /// 免得把「已关注」的 UP 主又关注一遍。
    follow_known: bool,
    /// 关注请求进行中：按钮置灰，避免连点。
    follow_busy: bool,
    /// 关注状态 / 粉丝数请求代次：换源后旧响应直接丢弃。
    follow_generation: u64,
    /// UP 主粉丝数（`x/relation/stat`；读到前不显示，不写死 0）。
    owner_fans: Option<u64>,
    /// 互动状态：是否已点赞 / 已投币 / 已收藏（打开视频时读一次）。
    interaction: yk_infra::InteractionState,
    /// 互动写操作进行中：按钮置灰，避免重复提交。
    interaction_busy: bool,
    /// 投币面板是否展开。
    coin_panel_open: bool,
    /// 投币面板里的「同时点赞」勾选。
    coin_also_like: bool,
    /// 收藏夹面板是否展开。
    fav_panel_open: bool,
    /// 分享面板是否展开。
    share_panel_open: bool,
    /// 自己的收藏夹列表（打开收藏面板时拉）。
    fav_folders: Vec<yk_infra::FavFolder>,
    /// 收藏夹列表是否在加载。
    fav_loading: bool,
    /// 收藏夹列表的失败原因：非空时面板显示「重试」，而不是空状态。
    fav_error: Option<String>,
    /// 收藏夹超过一屏时是否已展开（默认先折叠，别把标签 / 评论顶出视野）。
    fav_list_expanded: bool,
    /// 互动行（点赞 / 投币 / 收藏 / 分享）的实际几何（窗口坐标）。
    /// 三个互动弹层按它定位，不再占文档流。
    action_row_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 正在播退场动画的那张卡：bool 已经清零，但内容还要再画一帧两帧。
    overlay_closing_kind: Option<ActionOverlay>,
    /// 弹层动画代次：入场 / 退场各起一段，旧定时器不能把新弹层收掉。
    overlay_generation: u64,
    /// 互动提示（成功 / 失败），在窗口底部居中弹 toast，几秒后自动消失。
    interaction_note: Option<(String, bool)>,
    /// 互动提示代次：旧定时器不能把新提示收走。
    interaction_note_generation: u64,
    /// toast 正在播放退场缩放：提示内容保留到动画结束再摘掉。
    interaction_note_closing: bool,
    /// 互动状态请求代次：换源后旧响应直接丢弃。
    interaction_generation: u64,
    /// 信息面板的唯一滚动区：顶部信息与评论一起滚。
    /// 右下角「回到顶部」按钮直接读它的 offset，滚过阈值才出现。
    panel_scroll: ScrollHandle,
    /// 进度条拖动中：指针移出轨道 / 移出控制条后仍继续跟随。
    scrubbing: bool,
    /// 进度条轨道的实际 bounds，鼠标 x 换算 0..1 用。
    progress_track_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 上次向 mpv 发 seek 的时间：拖动中约 50ms 一次，松开时补最后一刀。
    last_scrub_seek: Option<std::time::Instant>,
    /// seek 发出后的短暂稳定期：旧帧的 time-pos 不能把刚拖到的位置顶回去。
    seek_settle_until: Option<std::time::Instant>,
    /// 面板 hover 延迟收起的代际：鼠标掠过边界不会闪收。
    panel_collapse_generation: u64,
    /// 面板宽度动画代际：每次展开 / 收起换一个 id，200ms EaseStandard 重新起跑。
    panel_generation: u64,
    /// 面板刚收起时的短暂抑制：鼠标还停在面板左缘时不立刻重新弹出。
    panel_hover_grace_until: Option<std::time::Instant>,
    /// 收起那一刻的光标位置：抑制期内光标**没动**才不弹回；
    /// 用户一动鼠标就立刻响应，不让面板显得"不听话"。
    panel_hover_grace_pos: Option<Point<Pixels>>,
    /// 排收起定时器那一刻的光标位置（收起时再取给 grace 用）。
    panel_collapse_cursor: Option<Point<Pixels>>,
    /// 最近一次在面板内滚动的时间。
    ///
    /// 滚动期间 hover 会因每帧重绘而偶发抖动；用它做一小段保持窗口，
    /// 避免「滚动 → hover 假 false → 收起 → 指针落回面板 → 再展开」的高频抖动。
    panel_last_scroll: Option<std::time::Instant>,
    /// 已经排了一个 220ms 收起定时器；避免 watchdog 每 100ms 重新排一次。
    panel_collapse_pending: bool,
    /// 鼠标位置 watchdog 是否已启动（首帧启动一次）。
    hover_watchdog_started: bool,
    /// 上次鼠标位置：过滤「位置没变」的重复 move（滚轮不应把控制栏叫出来）。
    last_pointer: Point<Pixels>,
    /// 最近一次鼠标 / 键盘活动时间，驱动 5 秒自动隐藏。
    last_activity: std::time::Instant,
    /// 窗口激活 / 失活观察者是否已注册（首帧注册一次）。
    activation_observed: bool,
    /// 观察者订阅句柄；随视图析构一起释放。
    activation_observer: Option<gpui::Subscription>,
    /// 窗口可见性观察者是否已注册（首帧注册一次）。
    visibility_observed: bool,
    /// 窗口可见性订阅句柄。
    visibility_observer: Option<gpui::Subscription>,
    /// 窗口可见性状态机（失焦与不可见要分开，见 [`VisibilityGate`]）。
    visibility: VisibilityGate,
    /// 评论图片查看器（独立实体，挂在播放器最上层）。
    /// 图片预览统一走 AppRoot 的 `open_image_viewer`：播放器不再持有自己的实例，
    /// 否则同一张图在播放器里和在页面里是两套状态（缩放/切图不共享）。
    image_handler: Option<std::rc::Rc<dyn Fn(&mut App, &mut Window, Vec<String>, usize)>>,
}

/// 进程内 `PlayerView` 实例数。
///
/// 设计上**只允许 1 个**：输入框的 `InputState` 挂在实例上，重建就等于换一个空输入框
/// （附录 F L-1）。`AppRoot::ensure_player` 已经保证复用，这里是第二道防线 ——
/// 以后谁再写一条 `cx.new(|_| PlayerView::new(..))`，会在这里立刻暴露。
static LIVE_PLAYERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 入场动画 id 的进程内序号：关闭再打开也拿新 id，避免元素状态复用导致直接跳到终点。
static ENTRANCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// 关闭缩回动画 id 的进程内序号：同一窗口反复开关也不会复用旧动画状态。
static EXIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Drop for PlayerView {
    fn drop(&mut self) {
        // 兜底：正常关闭 / 换源已经在 begin_close / open_bvid 里补过上报；
        // 直接 drop（退出登录、窗口销毁）时这里再补最后一拍。
        if let Some(mut report) = self.history_report.take() {
            if report.opened_at.elapsed() >= std::time::Duration::from_secs(5) {
                let position = self.play_time.max(0.0) as u64;
                dispatch_history_report(&report, position);
            }
        }
        // 兜底：正常关闭 / 换源已经存过，这里再存一次不亏（换源时旧视频
        // 的位置可能还没落盘）。位置没变时是幂等的。
        self.save_resume_position();
        LIVE_PLAYERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl PlayerView {
    pub fn new(theme: YkTheme, cx: &mut Context<Self>) -> Self {
        let n = LIVE_PLAYERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        assert!(
            n <= 1,
            "PlayerView 只能有一个实例（当前 {n} 个）：重建会换掉 InputState，\
             输入内容随之丢失，见附录 F L-1 / 附录 D 事实 19"
        );
        eprintln!("[player] PlayerView::new（实例数 {n}）");
        let model = PlayerModel::new();
        // 音量滑杆：0–100 整数步进，初值取视图模型的音量（默认 100）。
        // 拖动不需要额外窗口句柄：SliderState 自己处理指针捕获。
        let volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value((model.state.volume * 100.0) as f32)
        });
        // 弹出条做成独立实体：拖动滑杆只重绘它自己，不带着整个播放器重排。
        let volume_popover_bounds = Rc::new(Cell::new(None));
        let weak_self = cx.entity().downgrade();
        let volume_panel = cx.new(|cx| {
            VolumePanelView::new(
                theme.clone(),
                volume_slider,
                weak_self,
                volume_popover_bounds.clone(),
                model.state.volume,
                model.state.muted,
                cx,
            )
        });
        Self {
            theme,
            frame: TestFrame::new(1280, 720),
            // worker 不在这里启动：构造只做纯赋值，启动线程/打开文件这类副作用
            // 放到首次 render（见 render 里的 tried_worker）。
            // 动机是「构造保持无副作用」这条约束，而不是 cx.new 会重试 ——
            // `Context::new` 收的是 FnOnce，只调用一次。
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            worker: None,
            tried_worker: false,
            source: None,
            loading_source: false,
            history_report: None,
            load_generation: 0,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            current: None,
            #[cfg(target_os = "windows")]
            current_image: None,
            #[cfg(target_os = "windows")]
            current_image_serial: None,
            #[cfg(target_os = "windows")]
            needs_image_clear: false,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            loop_started: false,
            // 沉浸优先：播放浮窗默认收起信息面板（鼠标进入右侧 200 面板时展开）。
            expanded: false,
            force_controls_visible: false,
            fullscreen_changed_at: None,
            tab: 0,
            danmaku: Vec::new(),
            danmaku_all: Vec::new(),
            danmaku_source: Vec::new(),
            danmaku_cid: 0,
            danmaku_aid: 0,
            danmaku_next_segment: 1,
            danmaku_segment_loading: false,
            danmaku_segment_done: false,
            danmaku_retry_at: None,
            danmaku_state: DanmakuState::Idle,
            danmaku_generation: 0,
            lanes: LaneAllocator::new(1280.0, 220.0),
            scroll_lanes: MAX_SCROLL_LANES,
            play_time: 0.0,
            danmaku_input: None,
            danmaku_sending: false,
            submit_requested: false,
            input_focused: false,
            focus_handle: cx.focus_handle().tab_stop(true),
            needs_focus: true,
            escape_hint: false,
            escape_hint_generation: 0,
            entrance_source: None,
            // 每个播放器实例一个唯一 id：即使没有封面入场（YK_BVID 直开），
            // 页面遮罩也能从 0 淡入，而不是复用上一实例已经跑完的动画。
            entrance_generation: ENTRANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            closing: false,
            closing_generation: 0,
            finish_handled: false,
            close_handler: None,
            up_profile_handler: None,
            up_avatar_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            resume: yk_infra::resume::ResumeStore::load_default(),
            resume_seed: None,
            start_position: 0.0,
            resume_key: None,
            last_resume_save: 0.0,
            tags: Vec::new(),
            comments: CommentState::Idle,
            comment_section: crate::comment_section::Section::default(),
            meta_generation: 0,
            danmaku_visible: true,
            followed: false,
            follow_known: false,
            follow_busy: false,
            follow_generation: 0,
            owner_fans: None,
            interaction: yk_infra::InteractionState::default(),
            interaction_busy: false,
            coin_panel_open: false,
            coin_also_like: false,
            fav_panel_open: false,
            share_panel_open: false,
            fav_folders: Vec::new(),
            fav_loading: false,
            fav_error: None,
            fav_list_expanded: false,
            action_row_bounds: Rc::new(Cell::new(None)),
            overlay_closing_kind: None,
            overlay_generation: 0,
            interaction_note: None,
            interaction_note_generation: 0,
            interaction_note_closing: false,
            interaction_generation: 0,
            panel_collapse_generation: 0,
            panel_scroll: ScrollHandle::new(),
            scrubbing: false,
            progress_track_bounds: Rc::new(Cell::new(None)),
            last_scrub_seek: None,
            seek_settle_until: None,
            panel_generation: 0,
            panel_hover_grace_until: None,
            panel_hover_grace_pos: None,
            panel_collapse_cursor: None,
            panel_last_scroll: None,
            panel_collapse_pending: false,
            hover_watchdog_started: false,
            last_pointer: point(px(0.), px(0.)),
            last_activity: std::time::Instant::now(),
            activation_observed: false,
            activation_observer: None,
            visibility_observed: false,
            visibility_observer: None,
            visibility: VisibilityGate::default(),
            image_handler: None,
            model,
            volume_popover_open: false,
            volume_panel,
            volume_button_bounds: Rc::new(Cell::new(None)),
            volume_popover_bounds,
            quality_popover_open: false,
            speed_popover_open: false,
            quality_switching: false,
            quality_generation: 0,
            quality_button_bounds: Rc::new(Cell::new(None)),
            speed_button_bounds: Rc::new(Cell::new(None)),
            control_popover_bounds: Rc::new(Cell::new(None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMENT_PAGE_SIZE, COMMENT_REQUEST_INTERVAL};
    use crate::controller::cursor_local;
    use crate::view::{percent_encode, qq_share_url, share_link_text, video_page_url};
    use crate::comment_view::COMMENT_TEXT_LINE_HEIGHT;
    use gpui::{point, px};

    /// 可见性状态机：失焦不算不可见，不可见才停帧。
    #[test]
    fn visibility_gate_separates_hidden_from_unfocused() {
        use super::VisibilityGate;

        let mut gate = VisibilityGate::default();
        assert!(gate.is_visible(), "初始可见");
        assert!(!gate.render_paused(), "可见时不该停帧");

        // 重复设置同一个状态不算变化（不该重复通知 worker）。
        assert!(!gate.set(true));
        assert!(gate.set(false), "可见 → 不可见应当算变化");
        assert!(gate.render_paused(), "不可见时视频线程要停帧");

        assert!(gate.set(true), "不可见 → 可见应当算变化");
        assert!(!gate.render_paused(), "恢复可见后要继续出帧");
    }

    #[test]
    fn cursor_local_matches_primary_screen() {
        // 主屏：CG 全局坐标与 GPUI 窗口原点本来就同源（屏幕原点 0,0）。
        let local = cursor_local(
            point(px(786.9), px(338.4)),
            point(px(0.), px(0.)),
            point(px(120.), px(80.)),
        );
        assert!((local.x.as_f32() - 666.9).abs() < 0.1);
        assert!((local.y.as_f32() - 258.4).abs() < 0.1);
    }

    #[test]
    fn cursor_local_handles_secondary_screen_origin() {
        // 左副屏：CGDisplayBounds 原点 (-1613,-1440)，GPUI 的窗口原点是
        // 「屏幕内」坐标 (108,342)。只减窗口原点会得到负数，侧栏判定失败；
        // 必须先减屏幕原点。
        let global = point(px(-231.8), px(-774.8));
        let window_only = point(px(-231.8 - 108.0), px(-774.8 - 342.0));
        assert!(window_only.x < px(0.), "旧算法会把光标算到窗口外");

        let local = cursor_local(global, point(px(-1613.), px(-1440.)), point(px(108.), px(342.)));
        assert!((local.x.as_f32() - 1273.2).abs() < 0.1);
        assert!((local.y.as_f32() - 323.2).abs() < 0.1);
    }

    #[test]
    fn share_link_text_puts_title_on_second_line() {
        let link = video_page_url("BV1mG1QBNEFN");
        assert_eq!(
            share_link_text(&link, "【攻略】若娜瓦"),
            "https://www.bilibili.com/video/BV1mG1QBNEFN
【攻略】若娜瓦"
        );
        // 标题两端空白要清掉；空标题不留空行。
        assert_eq!(share_link_text(&link, "  标题  "), format!("{link}
标题"));
        assert_eq!(share_link_text(&link, "   "), link);
    }

    #[test]
    fn video_page_url_uses_bvid() {
        assert_eq!(
            video_page_url("BV1mG1QBNEFN"),
            "https://www.bilibili.com/video/BV1mG1QBNEFN"
        );
        assert_eq!(
            video_page_url("  BV1x  "),
            "https://www.bilibili.com/video/BV1x"
        );
    }

    #[test]
    fn percent_encode_escapes_reserved_bytes() {
        assert_eq!(percent_encode("abc-_.~"), "abc-_.~");
        assert_eq!(percent_encode("a b&c=d"), "a%20b%26c%3Dd");
        // 中文按 UTF-8 字节逐个转义
        assert_eq!(percent_encode("动态"), "%E5%8A%A8%E6%80%81");
    }

    #[test]
    fn comment_batch_is_one_small_request() {
        // 一次「加载更多」= 1 页 20 条：不许再回到「拼 4 页凑 100 条」。
        assert!(COMMENT_PAGE_SIZE <= 20);
        assert!(
            COMMENT_REQUEST_INTERVAL >= std::time::Duration::from_millis(500),
            "请求间隔太短，评论区会再次被 -352 拦"
        );
    }

    #[test]
    fn qq_share_url_carries_link_and_title() {
        let url = qq_share_url(
            "https://www.bilibili.com/video/BV1x",
            "标题 空格",
            "简介&更多",
        );
        assert!(url.starts_with("https://connect.qq.com/widget/shareqq/index.html?url="));
        assert!(url.contains("url=https%3A%2F%2Fwww.bilibili.com%2Fvideo%2FBV1x"));
        assert!(url.contains("title=%E6%A0%87%E9%A2%98%20%E7%A9%BA%E6%A0%BC"));
        assert!(url.contains("desc=%E7%AE%80%E4%BB%8B%26%E6%9B%B4%E5%A4%9A"));
    }
}
