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

use gpui::assets::IconName;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::kbd::Kbd;
use gpui::component::menu::ContextMenuExt as _;
use gpui::component::Icon;
use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{FocusHandle, InteractiveElement, StatefulInteractiveElement, *};
use yk_core::danmaku::{Danmaku, DanmakuMode, DanmakuSet, LaneAllocator};
use yk_core::dynamic::{asset_url, fitted_width, CommentTarget, DynamicComment};
use yk_core::format;
use yk_core::player::PlayerState;
use yk_ui::status;
use yk_ui::theme::*;

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
    player: yk_media::MpvPlayer,
    buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    tick: u64,
}

#[cfg(target_os = "macos")]
impl MpvFrameSource {
    /// 用 libmpv 解一帧并转成 CVPixelBuffer。失败时返回 None，调用方退回测试帧。
    pub fn open(video: &str, w: u32, h: u32) -> Option<Self> {
        let mut mpv = yk_media::MpvPlayer::new(w, h).ok()?;
        mpv.load_file(video).ok()?;
        // 给解码器一点时间产出首帧
        std::thread::sleep(std::time::Duration::from_millis(350));
        let rgba = mpv.render_frame().ok()?;
        let nv12 = yk_media::convert::rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
        Some(Self {
            player: mpv,
            buffer: yk_media::pixel_buffer::nv12_to_pixel_buffer(&nv12),
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
            let nv12 = yk_media::convert::rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
            self.buffer = yk_media::pixel_buffer::nv12_to_pixel_buffer(&nv12);
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
const COMMENT_PAGE_SIZE: u32 = yk_core::api::client::COMMENT_PAGE_SIZE;

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
fn scroll_lane_capacity(window: &Window) -> usize {
    let area_height =
        (window.viewport_size().height.as_f32() - (TOP_BAR_HEIGHT + SPACE_4) - PLAYER_BOTTOM_INSET)
            .max(120.0);
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
    /// 当前 Tab：0 评论 / 1 弹幕 / 2 选集。
    tab: usize,
    /// 播放状态（音量/倍速/进度/全屏）全部来自 yk-core 的 PlayerState。
    state: PlayerState,
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
    source: Option<yk_core::PlaybackSource>,
    /// 正在后台取播放地址。
    loading_source: bool,
    /// 取流失败原因（卡片没有 bvid / 需要登录 / 视频受限）。
    source_error: Option<String>,
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
    resume: yk_core::resume::ResumeStore,
    /// 打开时由「观看历史」带进来的起播位置（服务端进度）。
    /// 与本地记录一起交给 `yk_core::resume::resolve_start_position` 取更靠后的。
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
    interaction: yk_core::InteractionState,
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
    fav_folders: Vec<yk_core::FavFolder>,
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
    /// 控制栏（连同底部黑框 / 左下角标）是否可见。
    controls_visible: bool,
    /// 最近一次鼠标 / 键盘活动时间，驱动 5 秒自动隐藏。
    last_activity: std::time::Instant,
    /// 5 秒空闲检测任务的代际：换源 / 隐藏时作废旧任务。
    controls_timer_generation: u64,
    /// 当前是否有一个空闲检测任务在跑，避免鼠标移动时重复 spawn。
    controls_timer_running: bool,
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
            source_error: None,
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
            resume: yk_core::resume::ResumeStore::load_default(),
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
            interaction: yk_core::InteractionState::default(),
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
            controls_visible: true,
            last_activity: std::time::Instant::now(),
            controls_timer_generation: 0,
            controls_timer_running: false,
            activation_observed: false,
            activation_observer: None,
            visibility_observed: false,
            visibility_observer: None,
            visibility: VisibilityGate::default(),
            image_handler: None,
            state: {
                let mut st = PlayerState::new();
                st.duration_secs = 754.0;
                st.position_secs = 256.0;
                st.playing = true;
                st
            },
        }
    }

    /// 卡片点击时注入起始封面；必须在 `open_bvid` 之前调用。
    pub fn set_entrance(&mut self, source: CoverSource, cx: &mut Context<Self>) {
        eprintln!(
            "[player] 封面展开：{}（{:.0}x{:.0} → 浮窗）",
            source.title,
            source.bounds.size.width.as_f32(),
            source.bounds.size.height.as_f32()
        );
        self.entrance_source = Some(source);
        self.entrance_generation = ENTRANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        cx.notify();
    }

    /// 卡片点击入口：按 bvid 取详情 + playurl，回填后由下一次 render 启动 mpv。
    ///
    /// 请求在后台 executor 跑；用 `load_generation` 丢弃过期结果，
    /// 连续点两张卡时不会出现旧视频覆盖新视频。
    pub fn open_bvid(&mut self, bvid: String, cx: &mut Context<Self>) {
        self.open_bvid_at(bvid, None, cx);
    }

    /// 同 [`Self::open_bvid`]，但带一个「已知进度」（秒）。
    ///
    /// `seed` 来自「观看历史」页：那条记录本身就写着上次看到哪，
    /// 直接拿来当起播位置，不用等本地记录或再发一次请求。
    pub fn open_bvid_at(&mut self, bvid: String, seed: Option<f64>, cx: &mut Context<Self>) {
        // 换源前先把上一条的关闭上报补掉，否则连续点两个视频时
        // 第一个只留下打开时的 0s 那一拍。
        self.flush_history_report();
        self.save_resume_position();
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.loading_source = true;
        self.source_error = None;
        self.source = None;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.worker = None;
            self.current = None;
        }
        #[cfg(target_os = "windows")]
        {
            self.needs_image_clear = true;
        }
        self.tried_worker = false;
        self.play_time = 0.0;
        self.state.position_secs = 0.0;
        self.state.playing = true;
        // 起播即藏控制栏：画面优先，鼠标一动再浮出来。
        self.controls_visible = false;
        self.controls_timer_generation = self.controls_timer_generation.wrapping_add(1);
        self.controls_timer_running = false;
        self.panel_hover_grace_until = None;
        self.panel_last_scroll = None;
        self.needs_focus = true;
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);
        self.finish_handled = false;
        // 续播上下文重置：起播位置要等取流成功（拿到时长）才能定。
        self.resume_seed = seed;
        self.start_position = 0.0;
        self.resume_key = None;
        self.last_resume_save = 0.0;
        // 信息面板数据跟着换源重置：旧视频的标签/评论不能落到新视频上。
        // 滚动位置也回到顶部，否则新视频一打开就停在旧视频的评论高度。
        self.tags.clear();
        self.comments = CommentState::Idle;
        self.comment_section.reset();
        // 弹幕跟着换源清空：旧视频的弹幕不能落在新视频的画面上。
        self.danmaku.clear();
        self.danmaku_all.clear();
        self.danmaku_sending = false;
        self.danmaku_state = DanmakuState::Idle;
        self.danmaku_generation = self.danmaku_generation.wrapping_add(1);
        self.lanes = LaneAllocator::new(1280.0, 220.0);
        self.panel_scroll.set_offset(point(px(0.), px(0.)));
        self.scrubbing = false;
        self.last_scrub_seek = None;
        self.seek_settle_until = None;
        self.meta_generation = self.meta_generation.wrapping_add(1);
        self.followed = false;
        self.follow_known = false;
        self.follow_busy = false;
        self.follow_generation = self.follow_generation.wrapping_add(1);
        self.owner_fans = None;
        // 互动状态跟着换源重置：上一个视频的点赞 / 投币 / 收藏不能带过来。
        self.interaction = yk_core::InteractionState::default();
        self.interaction_busy = false;
        self.coin_panel_open = false;
        self.coin_also_like = false;
        self.fav_panel_open = false;
        self.share_panel_open = false;
        // 换源时把可能还在播的退场动画也断掉，别让上一支视频的卡飘在新视频上。
        self.overlay_closing_kind = None;
        self.overlay_generation = self.overlay_generation.wrapping_add(1);
        self.fav_folders.clear();
        self.fav_loading = false;
        self.fav_error = None;
        self.fav_list_expanded = false;
        self.interaction_note = None;
        self.interaction_note_closing = false;
        self.interaction_note_generation = self.interaction_note_generation.wrapping_add(1);
        // 本地没有记录、也没有历史页带来的进度时，才去服务端历史里找一次
        // （别的设备 / 别的客户端看过，本地不知道）。
        let need_remote_history =
            seed.is_none() && self.resume.resume_position(bvid.trim()).is_none();
        eprintln!("[player] 请求播放：{bvid}");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let cookie = current_session_cookie();
                    let client = match yk_core::api::client::ReqwestClient::new() {
                        Ok(client) => client,
                        Err(error) => return (Err(error), None),
                    };
                    // 服务端历史查询与取流并行：都是阻塞请求，放两条线程上跑。
                    // 查询走 channel 而不是 join —— 就算它卡住，超时后也直接
                    // 从头播，不能让历史查询拖住起播。
                    let (history_tx, history_rx) = std::sync::mpsc::channel::<Option<u64>>();
                    if need_remote_history && !cookie.trim().is_empty() {
                        let cookie = cookie.clone();
                        let bvid = bvid.clone();
                        std::thread::spawn(move || {
                            let progress = (|| {
                                let client = match yk_core::api::client::ReqwestClient::new() {
                                    Ok(client) => client,
                                    Err(error) => {
                                        eprintln!("[resume] 历史查询客户端创建失败：{error}");
                                        return None;
                                    }
                                };
                                match yk_core::api::client::fetch_history(
                                    &client, &cookie, None, "",
                                ) {
                                    Ok((entries, _)) => {
                                        let progress = yk_core::api::client::find_history_progress(
                                            &entries, &bvid,
                                        );
                                        if let Some(progress) = progress {
                                            eprintln!(
                                                "[resume] 服务端历史命中：{bvid} @ {progress}s"
                                            );
                                        }
                                        progress
                                    }
                                    Err(error) => {
                                        eprintln!("[resume] 服务端历史查询失败：{error}");
                                        None
                                    }
                                }
                            })();
                            let _ = history_tx.send(progress);
                        });
                    }
                    let source = yk_core::api::client::fetch_playback(&client, &bvid, &cookie);
                    // 历史查询**不能拖住起播**：它只决定「从第几秒开始」，
                    // 拿不到就从 0 / 本地续播记录开始。以前这里等 2 秒，
                    // 相当于每次开播都先站 2 秒——这是我们比别家慢的主因。
                    let remote_progress = history_rx
                        .recv_timeout(std::time::Duration::from_millis(250))
                        .ok()
                        .flatten();
                    (source, remote_progress)
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                view.loading_source = false;
                let (source, remote_progress) = result;
                match source {
                    Ok(src) => {
                        eprintln!(
                            "[player] 取流成功：{}（{}s，音轨 {}）",
                            src.title,
                            src.duration_secs,
                            if src.audio_url.is_some() {
                                "有"
                            } else {
                                "无"
                            }
                        );
                        view.state.duration_secs = src.duration_secs as f64;
                        view.state.playing = true;
                        // 续播：本地记录 + 历史页带来的进度 + 服务端历史，取更靠后的。
                        let start =
                            view.resolve_resume_position(&src, remote_progress.map(|p| p as f64));
                        view.start_position = start;
                        view.state.position_secs = start;
                        let (aid, bvid, cid) = (src.aid, src.bvid.clone(), src.cid);
                        let owner_mid = src.owner_mid;
                        view.source = Some(src);
                        view.begin_history_report(aid, bvid.clone(), cid, start);
                        // 标签 / 评论 / 弹幕不阻塞起播：源挂上之后再拉，回来一轮重绘。
                        view.spawn_video_meta(aid, bvid.clone(), cx);
                        view.spawn_danmaku(cid, aid, cx);
                        // 互动状态（已点赞 / 已投币 / 已收藏）也放后台：按钮高亮晚一拍无妨。
                        view.spawn_interaction_state(aid, bvid, cx);
                        // 关注状态与粉丝数：同样是「先出画面，按钮稍后点亮」。
                        view.spawn_follow_state(owner_mid, cx);
                    }
                    Err(e) => {
                        eprintln!("[player] 取流失败：{e}");
                        view.source_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉信息面板的标签；评论交给共享的 `comment_section`。
    ///
    /// 起播不依赖这两个请求：取流成功、面板已有标题/UP 主之后再后台补。
    /// 失败只影响面板里的对应区块，不把整个播放器打成错误态。
    fn spawn_video_meta(&mut self, aid: i64, bvid: String, cx: &mut Context<Self>) {
        if aid <= 0 && bvid.trim().is_empty() {
            self.comments = CommentState::Ready;
            self.comment_section.list.is_end = true;
            return;
        }
        self.meta_generation = self.meta_generation.wrapping_add(1);
        let generation = self.meta_generation;
        self.comments = CommentState::Loading;
        // 评论目标：有 aid 才有评论区（动态 / 直播回放没有）。
        self.comment_section
            .set_target(
                (aid > 0).then(|| CommentTarget {
                    oid: aid.to_string(),
                    kind: 1,
                }),
                current_session_cookie(),
            );
        self.comment_section.load_first(cx);
        self.comments = CommentState::Ready;
        let cookie = current_session_cookie();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if bvid.trim().is_empty() {
                        return Ok::<_, String>(Vec::new());
                    }
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    Ok(match yk_core::api::client::fetch_video_tags(&client, &bvid, &cookie) {
                        Ok(tags) => tags,
                        Err(error) => {
                            eprintln!("[player] 标签加载失败：{error}");
                            Vec::new()
                        }
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.meta_generation != generation {
                    return;
                }
                if let Ok(tags) = result {
                    if !tags.is_empty() {
                        view.tags = tags;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开视频后拉一次互动状态：是否已点赞 / 已投币 / 已收藏。
    ///
    /// 未登录时直接跳过：这些接口都要登录态，返回的一定是「未互动」。
    /// 失败只影响按钮的高亮，不影响播放。
    fn spawn_interaction_state(&mut self, aid: i64, bvid: String, cx: &mut Context<Self>) {
        if bvid.trim().is_empty() {
            return;
        }
        let cookie = current_session_cookie();
        if cookie.trim().is_empty() {
            return;
        }
        self.interaction_generation = self.interaction_generation.wrapping_add(1);
        let generation = self.interaction_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    // 一个请求拿「赞 / 币 / 藏 / 关注」：
                    // 以前这三个状态是三个接口串行，等于每次开播多排两格队。
                    let (liked, coined, favoured, followed) =
                        yk_core::interaction::fetch_archive_relation(
                            &client, &cookie, aid, &bvid,
                        )
                        .unwrap_or((false, false, false, None));
                    Ok::<_, String>((liked, coined, favoured, followed))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.interaction_generation != generation {
                    return;
                }
                if let Ok((liked, coined, favoured, followed)) = result {
                    view.interaction.liked = liked;
                    view.interaction.coined = coined;
                    view.interaction.favoured = favoured;
                    // 关注态就靠这一次请求定：`attention` 回来了就是权威值。
                    if let Some(followed) = followed {
                        view.followed = followed;
                        view.follow_known = true;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 打开视频后读一次 UP 主关系：是否已关注 + 粉丝数。
    ///
    /// 两个请求都放后台、失败静默：
    /// - 关系状态要登录态，未登录直接跳过（按钮保持「关注」，点了会提示登录）；
    /// - 粉丝数是公开接口，未登录也能显示。
    fn spawn_follow_state(&mut self, owner_mid: i64, cx: &mut Context<Self>) {
        if owner_mid <= 0 {
            // 没有 uid 就无从查起：保持未知，点击时给出人话提示。
            return;
        }
        let cookie = current_session_cookie();
        self.follow_generation = self.follow_generation.wrapping_add(1);
        let generation = self.follow_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    // 关注状态不再单独发 `x/relation?fid`：同一次开播的
                    // `archive/relation` 已经把 `attention` 带回来了。
                    // 这里只补粉丝数（那是另一个接口的字段）。
                    let followed: Option<bool> = None;
                    let fans = match yk_core::api::client::fetch_relation_stat(
                        &client, &cookie, owner_mid,
                    ) {
                        Ok((fans, _)) => {
                            eprintln!("[follow] 粉丝数：mid={owner_mid} fans={fans}");
                            Some(fans)
                        }
                        Err(error) => {
                            eprintln!("[follow] 粉丝数读取失败：{error}");
                            None
                        }
                    };
                    Ok::<_, String>((followed, fans))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.follow_generation != generation {
                    return;
                }
                if let Ok((followed, fans)) = result {
                    if let Some(followed) = followed {
                        view.followed = followed;
                        view.follow_known = true;
                    }
                    if fans.is_some() {
                        view.owner_fans = fans;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 关注 / 取关当前 UP 主。
    ///
    /// 两种起点：
    /// - 已经读到过关系状态：按钮先乐观翻转，失败再翻回来；
    /// - 还没读到（比如刚打开视频就点）：先查一次关系，再写反状态，
    ///   期间按钮显示「处理中」，不猜当前状态。
    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        if self.follow_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        if source.owner_mid <= 0 {
            self.show_interaction_note("没拿到 UP 主 uid，无法关注".into(), true, cx);
            return;
        }
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再关注".into(), true, cx);
            return;
        };
        let mid = source.owner_mid;
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let known = self.follow_known;
        let mut next = !self.followed;
        self.follow_busy = true;
        if known {
            // 乐观更新：失败回滚（见下方 Err 分支）。
            self.followed = next;
            if let Some(fans) = self.owner_fans.as_mut() {
                *fans = if next {
                    fans.saturating_add(1)
                } else {
                    fans.saturating_sub(1)
                };
            }
        }
        self.follow_generation = self.follow_generation.wrapping_add(1);
        let generation = self.follow_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    if !known {
                        // 未知起点：先查真实状态，再决定写「关注」还是「取关」。
                        let followed =
                            yk_core::api::client::fetch_followed(&client, &cookie, mid)?;
                        next = !followed;
                    }
                    yk_core::interaction::follow_user(&client, &cookie, &csrf, mid, next)?;
                    Ok::<_, String>((known, next))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.follow_generation != generation {
                    return;
                }
                view.follow_busy = false;
                match result {
                    Ok((known, next)) => {
                        if !known {
                            // 未知起点只在成功后才点亮；粉丝数跟着动一下。
                            view.followed = next;
                            view.follow_known = true;
                            if let Some(fans) = view.owner_fans.as_mut() {
                                *fans = if next {
                                    fans.saturating_add(1)
                                } else {
                                    fans.saturating_sub(1)
                                };
                            }
                        }
                        view.show_interaction_note(
                            if next { "已关注" } else { "已取消关注" }.into(),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[interaction] 关注失败：{error}");
                        if known {
                            // 乐观更新回滚：按钮与粉丝数都退回去。
                            view.followed = !next;
                            if let Some(fans) = view.owner_fans.as_mut() {
                                *fans = if next {
                                    fans.saturating_sub(1)
                                } else {
                                    fans.saturating_add(1)
                                };
                            }
                        }
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 在窗口底部居中弹一条轻提示（toast）。
    ///
    /// 生命周期分三段：入场缩放 → 停留 `TOAST_VISIBLE_MS` → 退场缩放，
    /// 退场动画走完才把内容从 `interaction_note` 里摘掉。
    fn show_interaction_note(&mut self, text: String, is_error: bool, cx: &mut Context<Self>) {
        self.interaction_note = Some((text, is_error));
        self.interaction_note_closing = false;
        self.interaction_note_generation = self.interaction_note_generation.wrapping_add(1);
        let generation = self.interaction_note_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(TOAST_VISIBLE_MS))
                .await;
            let still_current = this
                .update(cx, |view, cx| {
                    if view.interaction_note_generation != generation {
                        return false;
                    }
                    // 先切到退场动画；内容还要再留 DURATION_FAST 让缩放收完。
                    view.interaction_note_closing = true;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !still_current {
                return;
            }
            // 动画的起点要等下一帧 request_layout 才建立，这里多留一两帧，
            // 避免缩放到一半就被摘掉（最后 10% 的透明度/尺寸跳变肉眼可见）。
            cx.background_executor()
                .timer(DURATION_FAST + std::time::Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.interaction_note_generation != generation {
                    return;
                }
                view.interaction_note = None;
                view.interaction_note_closing = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// 点赞 / 取消点赞。
    ///
    /// 先本地翻转（乐观更新），服务端失败再回滚并提示。
    fn toggle_like(&mut self, cx: &mut Context<Self>) {
        if self.interaction_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再点赞".into(), true, cx);
            return;
        };
        if source.bvid.trim().is_empty() {
            return;
        }
        let next = !self.interaction.liked;
        self.interaction.liked = next;
        self.interaction_busy = true;
        if let Some(source) = self.source.as_mut() {
            source.like_count = if next {
                source.like_count.saturating_add(1)
            } else {
                source.like_count.saturating_sub(1)
            };
        }
        let bvid = source.bvid.clone();
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::like_video(&client, &cookie, &csrf, &bvid, next)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(()) => {
                        view.show_interaction_note(
                            if next { "已点赞" } else { "已取消点赞" }.into(),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[interaction] 点赞失败：{error}");
                        // 乐观更新回滚：按钮状态和计数都退回去。
                        view.interaction.liked = !next;
                        if let Some(source) = view.source.as_mut() {
                            source.like_count = if next {
                                source.like_count.saturating_sub(1)
                            } else {
                                source.like_count.saturating_add(1)
                            };
                        }
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 展开 / 收起投币面板。
    fn toggle_coin_panel(&mut self, cx: &mut Context<Self>) {
        // 未登录时投不了币：直接提示，不展开一个点不动的面板。
        if !self.coin_panel_open && current_session().is_none() {
            self.show_interaction_note("请先登录后再投币".into(), true, cx);
            return;
        }
        if self.coin_panel_open {
            self.close_action_overlays(cx);
            return;
        }
        self.fav_panel_open = false;
        self.share_panel_open = false;
        self.coin_panel_open = true;
        self.note_overlay_opened(cx);
    }

    /// 投币（1 枚 / 2 枚，可同时点赞）。
    fn do_coin(&mut self, multiply: u8, also_like: bool, cx: &mut Context<Self>) {
        if self.interaction_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再投币".into(), true, cx);
            return;
        };
        if source.bvid.trim().is_empty() {
            return;
        }
        // 投币接口只认 av 号（bvid 会被服务端判成 -400 请求错误）。
        let aid = source.aid;
        // bvid 只用来拼视频页 Referer（风控会核对请求来源页）。
        let bvid = source.bvid.clone();
        if aid <= 0 {
            self.show_interaction_note("这个视频没有可投币的信息".into(), true, cx);
            return;
        }
        let coins = multiply.clamp(1, 2);
        self.interaction_busy = true;
        self.close_action_overlays(cx);
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::coin_video(
                        &client, &cookie, &csrf, aid, &bvid, coins, also_like,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(()) => {
                        view.interaction.coined = true;
                        if also_like {
                            view.interaction.liked = true;
                        }
                        if let Some(source) = view.source.as_mut() {
                            source.coin_count = source.coin_count.saturating_add(coins as u64);
                            if also_like {
                                source.like_count = source.like_count.saturating_add(1);
                            }
                        }
                        view.show_interaction_note(format!("已投 {coins} 枚硬币"), false, cx);
                    }
                    Err(error) => {
                        eprintln!("[interaction] 投币失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 展开 / 收起收藏夹面板；展开时顺手把收藏夹列表拉回来。
    fn toggle_fav_panel(&mut self, cx: &mut Context<Self>) {
        // 未登录时列不出自己的收藏夹：直接提示。
        if !self.fav_panel_open && current_session().is_none() {
            self.show_interaction_note("请先登录后再收藏".into(), true, cx);
            return;
        }
        if self.fav_panel_open {
            self.close_action_overlays(cx);
            return;
        }
        self.coin_panel_open = false;
        self.share_panel_open = false;
        self.fav_panel_open = true;
        self.note_overlay_opened(cx);
        if self.fav_folders.is_empty() {
            self.load_fav_folders(cx);
        }
    }

    /// 拉自己的收藏夹列表。
    fn load_fav_folders(&mut self, cx: &mut Context<Self>) {
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再收藏".into(), true, cx);
            return;
        };
        self.fav_loading = true;
        self.fav_error = None;
        let cookie = session.cookie_header();
        let mid = session.dede_user_id;
        // 带上当前视频的 aid：夹子的 `fav_state` 才是「这个视频在不在里面」，
        // 否则面板上的勾选圈永远空着（看起来像没收藏）。
        let rid = self.source.as_ref().map(|source| source.aid).unwrap_or(0);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::fetch_fav_folders(&client, &cookie, mid, rid)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.fav_loading = false;
                match result {
                    Ok(folders) => {
                        view.fav_folders = folders;
                        view.fav_error = None;
                    }
                    Err(error) => {
                        eprintln!("[interaction] 收藏夹加载失败：{error}");
                        // 面板内联显示失败态 + 重试：只弹 toast 的话，
                        // 面板里只剩一片空白，用户不知道是「没有收藏夹」还是「没加载出来」。
                        view.fav_error = Some(error.clone());
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 收藏到指定收藏夹 / 从收藏夹移除。
    fn do_fav(&mut self, folder_id: i64, add: bool, cx: &mut Context<Self>) {
        // 退场动画里那张卡只是残影：别再让它接业务操作。
        if self.interaction_busy || !self.fav_panel_open {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再收藏".into(), true, cx);
            return;
        };
        if source.aid <= 0 {
            return;
        }
        self.interaction_busy = true;
        let aid = source.aid;
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::fav_video(&client, &cookie, &csrf, aid, folder_id, add)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(()) => {
                        // 收藏夹是多选：勾上 / 取消只改这一夹的状态。
                        // 先回写列表，否则面板上的勾选框永远停在旧状态
                        // （看起来像点了没反应）。
                        if let Some(folder) = view
                            .fav_folders
                            .iter_mut()
                            .find(|folder| folder.id == folder_id)
                        {
                            folder.fav_state = if add { 1 } else { 0 };
                        }
                        // 顶部的收藏数只在「一个夹都没有 → 至少收进一个」这一下 ±1；
                        // 收进第二个夹不该再加一次数。
                        let was_favoured = view.interaction.favoured;
                        let now_favoured =
                            view.fav_folders.iter().any(|folder| folder.contains_video());
                        view.interaction.favoured = now_favoured;
                        if was_favoured != now_favoured {
                            if let Some(source) = view.source.as_mut() {
                                source.favorite_count = if now_favoured {
                                    source.favorite_count.saturating_add(1)
                                } else {
                                    source.favorite_count.saturating_sub(1)
                                };
                            }
                        }
                        view.show_interaction_note(
                            if add { "已收藏" } else { "已取消收藏" }.into(),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[interaction] 收藏失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 收起互动区的所有弹层；返回「刚才确实有弹层开着」。
    ///
    /// Esc 和「面板收起」都走这里：弹层是浮在内容上的一次性操作，
    /// 关掉它不应该带动其它状态（不退出播放器、不重置互动）。
    fn close_action_overlays(&mut self, cx: &mut Context<Self>) -> bool {
        let kind = if self.fav_panel_open {
            ActionOverlay::Fav
        } else if self.coin_panel_open {
            ActionOverlay::Coin
        } else if self.share_panel_open {
            ActionOverlay::Share
        } else {
            return false;
        };
        self.coin_panel_open = false;
        self.fav_panel_open = false;
        self.share_panel_open = false;
        // 留一两帧给退场动画：内容按 kind 重新画，动画走完再真正摘掉。
        self.overlay_closing_kind = Some(kind);
        self.overlay_generation = self.overlay_generation.wrapping_add(1);
        let generation = self.overlay_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // 动画起点要等下一帧 request_layout 才建立，多留两帧，
            // 免得淡到一半就被摘掉（最后 10% 的跳变肉眼可见）。
            cx.background_executor()
                .timer(DURATION_FAST + std::time::Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.overlay_generation != generation || view.overlay_closing_kind.is_none() {
                    return;
                }
                view.overlay_closing_kind = None;
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// 弹层刚打开：清掉可能还在跑的退场动画，并起一段新的入场动画。
    fn note_overlay_opened(&mut self, cx: &mut Context<Self>) {
        self.overlay_closing_kind = None;
        self.overlay_generation = self.overlay_generation.wrapping_add(1);
        // 打开弹层时可能已经排了一个 220ms 的收起：取消掉，
        // 否则刚打开就被收走（面板一收弹层也跟着关）。
        if self.panel_collapse_pending {
            self.panel_collapse_pending = false;
            self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
        }
        cx.notify();
    }

    /// 展开 / 收起分享面板。
    fn toggle_share_panel(&mut self, cx: &mut Context<Self>) {
        if self.share_panel_open {
            self.close_action_overlays(cx);
            return;
        }
        self.coin_panel_open = false;
        self.fav_panel_open = false;
        self.share_panel_open = true;
        self.note_overlay_opened(cx);
    }

    /// 分享到哔哩哔哩动态：转发视频，配文用标题。
    fn share_to_dynamic(&mut self, cx: &mut Context<Self>) {
        if self.interaction_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再分享".into(), true, cx);
            return;
        };
        if source.aid <= 0 {
            self.show_interaction_note("这个视频没有可转发的信息".into(), true, cx);
            return;
        }
        self.interaction_busy = true;
        self.close_action_overlays(cx);
        // 正文 = 标题 + 视频链接：B 站会把链接解析成视频卡片，
        // 解析不了也至少是一条能点进视频的动态。
        let content = format!("{} {}", source.title, video_page_url(&source.bvid));
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::share_to_dynamic(&client, &cookie, &csrf, &content)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(dynamic_id) => {
                        eprintln!("[interaction] 已转发到动态：{dynamic_id}");
                        if let Some(source) = view.source.as_mut() {
                            source.share_count = source.share_count.saturating_add(1);
                        }
                        view.show_interaction_note("已分享到哔哩哔哩动态".into(), false, cx);
                    }
                    Err(error) => {
                        eprintln!("[interaction] 分享到动态失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 分享到 QQ：打开腾讯的 QQ 分享卡片页（默认浏览器）。
    ///
    /// QQ 桌面版注册的 `tencent://` / `mqqapi://` 只能跳到指定聊天对象，
    /// 没有「选好友分享」的入口；网页端的分享卡片才是腾讯官方给的那条路。
    fn share_to_qq(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if source.bvid.trim().is_empty() {
            self.show_interaction_note("这个视频没有可分享的链接".into(), true, cx);
            return;
        }
        self.close_action_overlays(cx);
        let link = video_page_url(&source.bvid);
        let title = source.title.clone();
        let desc = source.desc.clone();
        let qq_url = qq_share_url(&link, &title, &desc);
        if open_in_browser(&qq_url) {
            if let Some(source) = self.source.as_mut() {
                source.share_count = source.share_count.saturating_add(1);
            }
            self.show_interaction_note("已打开 QQ 分享卡片".into(), false, cx);
        } else {
            self.show_interaction_note("打开 QQ 分享卡片失败".into(), true, cx);
        }
        cx.notify();
    }

    /// 复制视频链接：`链接 + 换行 + 标题`。
    ///
    /// 只贴一条裸链接，粘到聊天框里对方看不出是什么视频；带上标题一行，
    /// 既认得出内容，链接本身也还能点开（和 B 站网页版复制的形态一致）。
    fn copy_video_link(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.close_action_overlays(cx);
        let link = video_page_url(&source.bvid);
        let text = share_link_text(&link, &source.title);
        let with_title = text.len() > link.len();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.show_interaction_note(
            if with_title { "已复制链接和标题" } else { "链接已复制" }.into(),
            false,
            cx,
        );
    }

    /// 窗口可见性变化：不可见时让视频线程停帧，可见时恢复。
    ///
    /// **只管「看不见」，不管「失焦」**：失焦（点了别的应用）时窗口还在屏幕上，
    /// 视频必须照常按显示器的标准帧率播 —— 这正是 `inactive_frame_interval: None`
    /// 想保住的行为。最小化 / 被完全遮挡时画面没人看，解码 + NV12 转换
    /// 纯属烧电，这时才停。
    ///
    /// mpv 本身不暂停：音频继续播，进度继续走，恢复可见后接着出帧。
    fn set_window_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !self.visibility.set(visible) {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.set_render_paused(self.visibility.render_paused());
        }
        // 不可见时不必再排下一帧；可见时补一帧把画面拉回来。
        if visible {
            cx.notify();
        }
    }

    /// 拉当前分 P 的真实弹幕（`/x/v1/dm/list.so`，按 cid 区分）。
    ///
    /// 与标签 / 评论分开请求：弹幕和评论互不阻塞，谁先回来谁先渲染。
    /// 换源时 `danmaku_generation` 已经 +1，旧响应回来直接丢弃。
    /// 起播时先把弹幕状态复位，然后拉**第一段**（6 分钟）。
    ///
    /// 整包拉 `x/v1/dm/list.so` 是几 MB 的 deflate XML，等于每次开播都为
    /// 「可能只看了三十秒」付全片成本；改成按段取，看多少拉多少。
    fn spawn_danmaku(&mut self, cid: i64, aid: i64, cx: &mut Context<Self>) {
        self.danmaku_generation = self.danmaku_generation.wrapping_add(1);
        self.danmaku_source.clear();
        self.danmaku_all.clear();
        self.danmaku.clear();
        self.danmaku_cid = cid;
        self.danmaku_aid = aid;
        self.danmaku_next_segment = 1;
        self.danmaku_segment_loading = false;
        self.danmaku_retry_at = None;
        if cid <= 0 {
            self.danmaku_segment_done = true;
            self.danmaku_state = DanmakuState::Ready;
            cx.notify();
            return;
        }
        self.danmaku_segment_done = false;
        self.danmaku_state = DanmakuState::Loading;
        cx.notify();
        self.load_danmaku_segment(cx);
    }

    /// 拉下一段弹幕并追加。空段 = 到头了（第一段就空 = 这个视频没有弹幕）。
    fn load_danmaku_segment(&mut self, cx: &mut Context<Self>) {
        if self.danmaku_segment_loading || self.danmaku_segment_done || self.danmaku_cid <= 0 {
            return;
        }
        if let Some(at) = self.danmaku_retry_at {
            if at > std::time::Instant::now() {
                return;
            }
        }
        let index = self.danmaku_next_segment;
        let (cid, aid) = (self.danmaku_cid, self.danmaku_aid);
        let generation = self.danmaku_generation;
        let cookie = current_session_cookie();
        self.danmaku_segment_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::api::client::fetch_danmaku_segment(
                        &client, &cookie, cid, aid, index,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.danmaku_generation != generation {
                    return;
                }
                view.danmaku_segment_loading = false;
                match result {
                    Ok(items) => {
                        view.danmaku_retry_at = None;
                        if items.is_empty() {
                            // 没有这一段 = 播到结尾了。
                            view.danmaku_segment_done = true;
                            view.danmaku_state = DanmakuState::Ready;
                        } else {
                            view.danmaku_source.extend(items);
                            view.danmaku_source.sort_by(|a, b| {
                                a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            view.danmaku_next_segment = index + 1;
                            view.danmaku_state = DanmakuState::Ready;
                            let all = view.danmaku_source.clone();
                            view.layout_danmaku(all);
                        }
                    }
                    Err(error) => {
                        // 失败不封顶：5 秒后再试，别在计数帧里死循环。
                        eprintln!("[danmaku] 第 {index} 段加载失败：{error}");
                        view.danmaku_retry_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        if index == 1 {
                            view.danmaku_state = DanmakuState::Ready;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 播到「已加载区间」末尾前 60 秒就预取下一段。
    fn ensure_danmaku_ahead(&mut self, cx: &mut Context<Self>) {
        if self.danmaku_segment_done || self.danmaku_cid <= 0 {
            return;
        }
        let loaded_until = self.danmaku_next_segment.saturating_sub(1) as f64
            * DANMAKU_SEGMENT_SECS;
        if self.play_time + DANMAKU_PREFETCH_SECS >= loaded_until {
            self.load_danmaku_segment(cx);
        }
    }

    fn install_danmaku(&mut self, set: DanmakuSet) {
        self.layout_danmaku(set.items().to_vec());
    }

    /// 按当前轨道数把弹幕重新分配到轨道上。
    ///
    /// 窗口高度变化时轨道数会变，重排一次就能把弹幕铺满新的播放区；
    /// 否则 14 条轨道在 700px 高的画面里只会占用上半屏。
    fn layout_danmaku(&mut self, items: Vec<Danmaku>) {
        let max_lanes = self.scroll_lanes.clamp(1, MAX_SCROLL_LANES);
        let mut lanes = LaneAllocator::new(1280.0, 220.0);
        let mut top_free = [f64::NEG_INFINITY; FIXED_LANES];
        let mut bottom_free = [f64::NEG_INFINITY; FIXED_LANES];
        let mut placed = Vec::with_capacity(items.len());
        let (mut scroll, mut top, mut bottom) = (0usize, 0usize, 0usize);

        for d in &items {
            let lane = match d.mode {
                DanmakuMode::Scroll => {
                    // 轮转分配：短弹幕（满屏「？」）不会全挤在第 0 条轨道。
                    let Some(lane) = lanes.assign_spread(d, max_lanes) else {
                        continue;
                    };
                    scroll += 1;
                    DanmakuLane::Scroll(lane)
                }
                DanmakuMode::Top => {
                    let Some(slot) = take_fixed_slot(&mut top_free, d.time) else {
                        continue;
                    };
                    top += 1;
                    DanmakuLane::Top(slot)
                }
                DanmakuMode::Bottom => {
                    let Some(slot) = take_fixed_slot(&mut bottom_free, d.time) else {
                        continue;
                    };
                    bottom += 1;
                    DanmakuLane::Bottom(slot)
                }
            };
            placed.push(PlacedDanmaku {
                data: d.clone(),
                lane,
            });
        }

        eprintln!(
            "[danmaku] 上屏 {} 条（滚动 {scroll} / 顶部 {top} / 底部 {bottom}，轨道 {max_lanes}），列表 {} 条",
            placed.len(),
            items.len()
        );
        self.lanes = lanes;
        self.danmaku = placed;
        self.danmaku_all = items;
    }

    /// 窗口高度变化后重排已有弹幕。
    fn reflow_danmaku(&mut self) {
        if self.danmaku_all.is_empty() {
            return;
        }
        let items = self.danmaku_all.clone();
        self.layout_danmaku(items);
    }

    /// 给用户刚发的一条滚动弹幕找一个当前空着的轨道。
    ///
    /// 预分配的 `LaneAllocator` 状态停在「整段视频排完」那一刻，
    /// 不能直接拿来给当前时刻的新弹幕分配；这里按「此刻每条轨道上
    /// 是否还有可见弹幕」现查，保证发出去的弹幕不会立刻叠字。
    fn pick_free_scroll_lane(&self, now: f64, area_width: f32) -> usize {
        for lane in 0..self.scroll_lanes.clamp(1, MAX_SCROLL_LANES) {
            let occupied = self.danmaku.iter().any(|placed| {
                matches!(placed.lane, DanmakuLane::Scroll(l) if l == lane)
                    && placed.data.time <= now + 0.2
                    && self.lanes.visible_at_in(&placed.data, now, area_width)
            });
            if !occupied {
                return lane;
            }
        }
        0
    }

    /// 面板展开 / 收起。
    ///
    /// 展开立刻生效；收起延迟 220ms，并先确认指针真的不在面板区域。
    /// 收起判断不信任 `on_hover(false)`：播放中每帧重绘会让它在指针没动时闪 false。
    fn set_panel_hover(&mut self, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        if hovered {
            self.panel_collapse_pending = false;
            self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
            self.panel_hover_grace_until = None;
            self.panel_hover_grace_pos = None;
            if !self.expanded {
                self.expanded = true;
                self.panel_generation = self.panel_generation.wrapping_add(1);
                cx.notify();
            }
            return;
        }
        // 已经收起就没什么可收的，避免鼠标在视频区移动时反复起定时器。
        if !self.expanded {
            return;
        }
        // 正在写评论：输入框还活着，不能把整个侧栏收走。
        if self.comment_section.focused {
            return;
        }
        // 刚在面板里滚动过：滚动期间 hover 会抖，先不收起。
        if self.panel_scrolled_recently() {
            return;
        }
        // 播放中每帧重绘都会重算 hover；指针实际还在面板里时，
        // 这次 false 是布局闪动，不是用户移出，直接忽略。
        // 用**当前**光标位置判断：`window.mouse_position()` 停在最后一次
        // 事件上，鼠标移出窗口后它还是旧位置，会把「真的移出去了」误判成
        // 「还在面板里」—— 面板就再也不会收起了。
        if self.pointer_in_panel_zone(cursor_position(window, cx), window) {
            return;
        }
        self.arm_panel_collapse(window, cx);
    }

    /// 互动弹层（投币 / 收藏夹 / 分享）现在是否开着。
    fn action_overlay_open(&self) -> bool {
        self.coin_panel_open || self.fav_panel_open || self.share_panel_open
    }

    /// 排一个 220ms 的收起定时器。调用方必须已经确认指针在 hover 区外。
    ///
    /// 不在这里再查 `window.mouse_position()`：watchdog 用的是系统级光标，
    /// 而 `Window::mouse_position()` 在快速移出窗口时可能仍停留在旧位置。
    fn arm_panel_collapse(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.expanded {
            return;
        }
        // 弹层开着就把面板钉住：弹层是从互动行挂出来的浮层，
        // 指针为了看它 / 点它跑出面板边界，不该连带把面板和弹层一起收掉。
        // 弹层关掉后（外点 / Esc / 再点触发按钮）看门狗会重新判断，该收还是会收。
        if self.action_overlay_open() {
            return;
        }
        // 评论区输入框聚焦时，指针移到视频上也不能把侧栏收走。
        if self.comment_section.focused {
            return;
        }
        // 记下这一刻的光标位置：收起后用它判断「光标有没有动过」。
        self.panel_collapse_cursor = Some(cursor_position(window, cx));
        // watchdog 每 100ms 也会调到这里：已经有定时器在跑就不要再排一个，
        // 否则一直从现在起算 220ms，永远到不了点。
        if self.panel_collapse_pending {
            return;
        }
        self.panel_collapse_pending = true;
        self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
        let generation = self.panel_collapse_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.panel_collapse_generation != generation {
                    // 已经被 set_panel_hover(true) 取消，或已有更新的
                    // 定时器接管；不要在这里清 pending，避免和新定时器打架。
                    return;
                }
                view.panel_collapse_pending = false;
                view.collapse_panel_now(cx);
            });
        })
        .detach();
    }

    /// 面板最近是否在滚动（含滚轮惯性尾部）。
    fn panel_scrolled_recently(&self) -> bool {
        self.panel_last_scroll
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(PANEL_SCROLL_HOLD_MS))
    }

    /// 立即收起面板（鼠标离开窗口 / 收起定时器到点）。
    fn collapse_panel_now(&mut self, cx: &mut Context<Self>) {
        if !self.expanded {
            return;
        }
        self.expanded = false;
        self.panel_collapse_pending = false;
        self.panel_generation = self.panel_generation.wrapping_add(1);
        // 弹层现在已经浮在面板之外：面板一收，它们就会悬在视频画面上。
        // 指针离开面板本来就该关掉弹层，三个一起收。
        self.close_action_overlays(cx);
        // 鼠标可能还停在面板左缘：400ms 内不响应，避免反复弹出。
        self.panel_hover_grace_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        self.panel_hover_grace_pos = self.panel_collapse_cursor.take();
        cx.notify();
    }

    /// 按下进度条：记录拖动开始，并立刻跳到按下位置。
    fn begin_scrub(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if self.state.duration_secs <= 0.0 {
            return;
        }
        self.note_activity(window, cx);
        self.scrubbing = true;
        self.seek_scrub_to(position, true, cx);
    }

    /// 拖动中：指针离开轨道也继续按轨道宽度换算。
    fn update_scrub(&mut self, position: Point<Pixels>, _window: &Window, cx: &mut Context<Self>) {
        if !self.scrubbing {
            return;
        }
        self.seek_scrub_to(position, false, cx);
    }

    /// 松开 / 指针离开窗口：结束拖动，并把最终位置补发给 mpv。
    fn end_scrub(&mut self, cx: &mut Context<Self>) {
        if !self.scrubbing {
            return;
        }
        self.scrubbing = false;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.seek(self.state.position_secs);
        }
        self.last_scrub_seek = None;
        self.seek_settle_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        cx.notify();
    }

    fn seek_scrub_to(&mut self, position: Point<Pixels>, force: bool, cx: &mut Context<Self>) {
        let Some(bounds) = self.progress_track_bounds.get() else {
            return;
        };
        let width = bounds.size.width.as_f32();
        if width <= 0.0 {
            return;
        }
        let fraction =
            ((position.x.as_f32() - bounds.origin.x.as_f32()) / width).clamp(0.0, 1.0) as f64;
        self.state.seek_to_fraction(fraction);
        self.play_time = self.state.position_secs;
        // 拖动中约 50ms 才向 mpv 发一次 seek；松开时 force 补最后一刀。
        let due = force
            || self
                .last_scrub_seek
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(50));
        if due {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some(worker) = self.worker.as_ref() {
                worker.seek(self.state.position_secs);
            }
            self.last_scrub_seek = Some(std::time::Instant::now());
            self.seek_settle_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        }
        cx.notify();
    }

    /// 用真实指针坐标决定面板展开 / 收起。
    ///
    /// `on_hover` 在播放中会因每帧重绘而抖动；鼠标移动是可靠的，
    /// 所以展开状态以这条路径为准，hover 回调只做兜底。
    fn update_panel_hover_from_pointer(
        &mut self,
        _event_position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // 判定一律用**当前光标位置**，不用事件坐标：事件坐标是那一刻的
        // 位置，而面板展开 / 收起的真正依据是「光标现在在哪」。
        // 两套坐标混用时，面板会在「光标已移开」和「事件还在面板里」
        // 之间反复横跳 —— 表现就是侧栏反复弹出 / 收起。
        let position = cursor_position(window, cx);
        if self.pointer_in_panel_zone(position, window) {
            // 刚收起 / 刚点掉面板时给一小段冷却：光标**没动**就不弹回，
            // 动了就立刻响应（否则用户一动鼠标面板就弹回来，很烦）。
            if let Some(until) = self.panel_hover_grace_until {
                if until > std::time::Instant::now() {
                    let still = self.panel_hover_grace_pos.is_some_and(|at| {
                        (at.x - position.x).abs() < px(2.) && (at.y - position.y).abs() < px(2.)
                    });
                    if still {
                        return;
                    }
                }
            }
            self.set_panel_hover(true, window, cx);
        } else {
            // 真实鼠标已经移出：这次不受「滚动后 400ms 保持」影响，
            // 否则刚滚完就移到右侧边距会迟迟不收起。
            self.panel_last_scroll = None;
            self.set_panel_hover(false, window, cx);
        }
    }

    /// 用一个透明 canvas 注册全局鼠标移动监听。
    ///
    /// `on_mouse_move` 只有在元素的 hover 命中为真时才会回调；播放中每帧重绘
    /// 会让面板 / 播放器容器的 hover 偶发为 false，漏掉「鼠标真的移出面板」。
    /// 这里绕过 hover 命中，直接拿鼠标坐标驱动面板收起。
    fn pointer_tracker(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let weak_move = weak.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = weak_move.update(cx, |view, cx| {
                        if view.pointer_in_player(event.position, window) {
                            view.note_activity(window, cx);
                        }
                        view.update_scrub(event.position, window, cx);
                        view.update_panel_hover_from_pointer(event.position, window, cx);
                    });
                });
                let weak_down = weak.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let _ = weak_down.update(cx, |view, cx| {
                        // 展开的面板盖在控制条右侧之上，点面板不能误触进度条。
                        let panel_covers =
                            view.expanded && view.pointer_in_panel_zone(event.position, window);
                        let on_track = view.controls_visible
                            && !panel_covers
                            && view
                                .progress_track_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                        if on_track {
                            view.begin_scrub(event.position, window, cx);
                            cx.stop_propagation();
                        }
                    });
                });
                let weak_up = weak.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = weak_up.update(cx, |view, cx| view.end_scrub(cx));
                });
                // 指针直接离开窗口时没有 MouseMove / MouseUp；这里补一次收尾。
                window.on_mouse_event(move |event: &MouseExitEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let position = event.position;
                    let _ = weak.update(cx, |view, cx| {
                        view.end_scrub(cx);
                        // 播放中每帧重绘会改变布局，GPUI 会因此偶发派发
                        // 播放器区域的 MouseExit（指针其实还在面板里）。
                        // 面板收起不在这里做：统一交给真实鼠标坐标 + 220ms 延迟，
                        // 否则视频每帧都可能把侧栏收掉再弹回来。
                        if !view.pointer_in_window(position, window) {
                            view.hide_controls(cx);
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// 鼠标移动：位置真的变了才算活动。滚轮/重绘触发的重复 move 不应反复叫出控制栏。
    fn note_pointer_move(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let dx = (position.x.as_f32() - self.last_pointer.x.as_f32()).abs();
        let dy = (position.y.as_f32() - self.last_pointer.y.as_f32()).abs();
        self.last_pointer = position;
        // 滚轮/重绘可能带出位置几乎不变的 move；2px 以内不算「移动」。
        if dx < 2.0 && dy < 2.0 {
            return;
        }
        self.note_activity(window, cx);
    }

    /// 鼠标 / 键盘活动：控制栏浮出，并重新开始 5 秒空闲计时。
    ///
    /// 只在窗口有焦点时响应：窗口在后台时鼠标掠过不应该把控制栏叫出来。
    fn note_activity(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !window.is_window_active() {
            return;
        }
        self.last_activity = std::time::Instant::now();
        if !self.controls_visible {
            self.controls_visible = true;
            cx.notify();
        }
        if !self.controls_timer_running {
            self.arm_controls_timer(cx);
        }
    }

    /// 起一个 250ms 轮询的 5 秒空闲计时；可见后一直挂着，隐藏时退出。
    fn arm_controls_timer(&mut self, cx: &mut Context<Self>) {
        self.controls_timer_running = true;
        self.controls_timer_generation = self.controls_timer_generation.wrapping_add(1);
        let generation = self.controls_timer_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let keep = this.update(cx, |view, cx| {
                    if view.controls_timer_generation != generation {
                        return false;
                    }
                    if !view.controls_visible {
                        return false;
                    }
                    // 正在输入弹幕 / 拖动进度条时不自动隐藏。
                    if view.input_focused || view.scrubbing {
                        view.last_activity = std::time::Instant::now();
                        return true;
                    }
                    if view.last_activity.elapsed() >= std::time::Duration::from_secs(5) {
                        view.controls_visible = false;
                        view.controls_timer_running = false;
                        view.controls_timer_generation =
                            view.controls_timer_generation.wrapping_add(1);
                        cx.notify();
                        return false;
                    }
                    true
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// 立即藏起控制栏（播放开始 / 窗口失焦 / 鼠标离开时用）。
    fn hide_controls(&mut self, cx: &mut Context<Self>) {
        // 拖动中收控制条 / 窗口失焦：先把拖动收尾，避免 scrub 状态悬空。
        if self.scrubbing {
            self.end_scrub(cx);
        }
        // 正在输入弹幕时不能把输入框从树里拿掉，等失焦后再走自动隐藏。
        if self.input_focused {
            return;
        }
        self.controls_timer_generation = self.controls_timer_generation.wrapping_add(1);
        self.controls_timer_running = false;
        if self.controls_visible {
            self.controls_visible = false;
            cx.notify();
        }
    }

    /// 算出这次的起播位置（秒）；0 表示从头播。
    ///
    /// 三个来源取更靠后的：
    /// - 本地续播记录（`bvid#cid`，退而求其次只用 `bvid`）；
    /// - 打开时带进来的历史进度（`resume_seed`）；
    /// - 服务端观看历史里的进度（`remote`，本地没记录时才会去查）。
    ///
    /// 规则见 [`yk_core::resume::resolve_start_position`]：刚开始看 / 已看完
    /// 都从头播。
    fn resolve_resume_position(
        &mut self,
        src: &yk_core::PlaybackSource,
        remote: Option<f64>,
    ) -> f64 {
        let duration = src.duration_secs as f64;
        let key = yk_core::resume::key(&src.bvid, src.cid);
        let local = self
            .resume
            .resume_position(&key)
            .or_else(|| self.resume.resume_position(&src.bvid));
        let seed = self.resume_seed;
        let start = yk_core::resume::resolve_start_position(&[local, seed, remote], duration)
            .unwrap_or(0.0);
        self.resume_key = Some(key);
        if start > 0.0 {
            eprintln!(
                "[resume] 从 {:.1}s 起播：{}（本地 {:?} / 历史 {:?} / 服务端 {:?}，时长 {}s）",
                start, src.bvid, local, seed, remote, src.duration_secs
            );
        } else {
            eprintln!("[resume] 无可续播记录，从头播：{}", src.bvid);
        }
        start
    }

    /// 把当前播放位置写进本地续播记录。
    ///
    /// 只在明确的节点调用（关闭 / 换源 / 播放中定期落盘），
    /// 不在每帧写盘。
    fn save_resume_position(&mut self) {
        let Some(key) = self.resume_key.clone() else {
            return;
        };
        let position = self.play_time.max(0.0);
        let duration = self.state.duration_secs;
        self.resume.record(&key, position, duration);
        // 只按 bvid 存一份作为兜底（cid 未知时仍能续上）。
        let bvid = key.split('#').next().unwrap_or_default().to_string();
        if !bvid.is_empty() && bvid != key {
            self.resume.record(&bvid, position, duration);
        }
        if let Err(e) = self.resume.save() {
            eprintln!("[resume] 写盘失败：{e}");
        }
    }

    /// 打开视频时上报一次历史（进度 = 起播位置，状态「已观看」）。
    ///
    /// 不做周期心跳：只在打开 / 关闭两个时机各报一次。关闭时是否再报，
    /// 由 [`Self::flush_history_report`] 判断「打开是否已满 5 秒」。
    ///
    /// `start_position` 是这次实际的起播位置（续播时不是 0）：
    /// 服务端进度直接写成它，避免「续播中」的历史条目被重置成 0%。
    fn begin_history_report(&mut self, aid: i64, bvid: String, cid: i64, start_position: f64) {
        let Some(session) = current_session() else {
            self.history_report = None;
            return;
        };
        if aid <= 0 || cid <= 0 || bvid.trim().is_empty() {
            self.history_report = None;
            eprintln!("[history-report] 缺少 aid/cid/bvid，跳过：aid={aid} cid={cid} bvid={bvid}");
            return;
        }
        let report = HistoryReport {
            aid,
            bvid: bvid.clone(),
            cid,
            cookie: session.cookie_header(),
            csrf: session.bili_jct.clone(),
            opened_at: std::time::Instant::now(),
        };
        eprintln!(
            "[history-report] 打开上报：{bvid}（aid={aid} cid={cid} progress={:.0}s）",
            start_position
        );
        dispatch_history_report(&report, start_position.max(0.0) as u64);
        self.history_report = Some(report);
    }

    /// 关闭 / 换源时补报一次；打开不足 5 秒则只保留打开时的那一次。
    fn flush_history_report(&mut self) {
        let Some(mut report) = self.history_report.take() else {
            return;
        };
        if report.opened_at.elapsed() < std::time::Duration::from_secs(5) {
            eprintln!(
                "[history-report] 打开不足 5s，跳过关闭上报：{}（{:.1}s）",
                report.bvid,
                report.opened_at.elapsed().as_secs_f32()
            );
            return;
        }
        let position = self.play_time.max(0.0) as u64;
        eprintln!("[history-report] 关闭上报：{} @ {position}s", report.bvid);
        dispatch_history_report(&report, position);
    }

    /// AppRoot 注入：关闭时把自己的 `playing` 置空。
    /// 测试入口：读侧栏（信息面板）当前是否展开。
    pub fn panel_expanded(&self) -> bool {
        self.expanded
    }

    /// 测试入口：互动行的实际几何（窗口坐标）。
    /// 三个互动弹层按它定位，`None` 表示这一帧还没 prepaint。
    #[doc(hidden)]
    pub fn test_action_row_bounds(&self) -> Option<Bounds<Pixels>> {
        self.action_row_bounds.get()
    }

    /// 测试入口：直接打开收藏夹弹层（跳过登录检查，也不拉接口）。
    #[doc(hidden)]
    pub fn test_open_fav_overlay(&mut self, cx: &mut Context<Self>) {
        self.coin_panel_open = false;
        self.share_panel_open = false;
        self.fav_panel_open = true;
        self.note_overlay_opened(cx);
    }

    /// 测试入口：读三个互动面板的展开状态（投币 / 收藏 / 分享）。
    pub fn action_panels_open(&self) -> (bool, bool, bool) {
        (
            self.coin_panel_open,
            self.fav_panel_open,
            self.share_panel_open,
        )
    }

    /// 测试 / 截图入口：直接灌一批评论（不走网络），评论 Tab 走真实渲染路径。
    ///
    /// `is_end = false` 时列表底部显示「正在加载更多评论…」之外的正常态，
    /// 可以配合 [`Self::set_test_scroll`] 目检窗口化渲染。
    #[doc(hidden)]
    pub fn with_comments(
        mut self,
        items: Vec<DynamicComment>,
        total: u64,
        is_end: bool,
    ) -> Self {
        self.comment_section.list.items = items;
        self.comment_section.list.total = total;
        self.comment_section.list.is_end = is_end;
        self.comments = CommentState::Ready;
        self.tab = 0;
        self
    }

    /// 测试 / 截图入口：把信息面板滚到指定位置（负数向下滚）。
    #[doc(hidden)]
    pub fn set_test_scroll(&mut self, y: f32) {
        self.panel_scroll.set_offset(point(px(0.), px(y)));
    }

    /// 测试入口：信息面板当前的滚动偏移（负数向下滚）。
    #[doc(hidden)]
    pub fn test_scroll_offset(&self) -> f32 {
        self.panel_scroll.offset().y.as_f32()
    }

    /// 测试入口：最近一帧实际创建了几个评论行（窗口化渲染验收）。
    #[doc(hidden)]
    pub fn rendered_comment_rows(&self) -> usize {
        self.comment_section.rendered_rows
    }

    /// 测试入口：评论输入条是否展开 / 输入框是否拿到焦点。
    #[doc(hidden)]
    pub fn test_composer_state(&self) -> (bool, bool) {
        (
            self.comment_section.expanded,
            self.comment_section.focused,
        )
    }

    /// 测试 / 截图入口：把侧栏切到展开态（400 宽）。
    pub fn with_expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// 测试 / 截图入口：直接灌一批弹幕（不走网络请求），并把播放位置拨到
    /// `at` 秒。弹幕排布与绘制和真实数据走完全同一条路径，用来目检
    /// 「弹幕层是不是跟播放层对齐」。
    #[doc(hidden)]
    pub fn with_danmaku(mut self, set: DanmakuSet, at: f64) -> Self {
        self.state.position_secs = at;
        self.play_time = at;
        self.install_danmaku(set);
        self
    }

    /// 测试 / 截图入口：把互动区的展开面板预先打开。
    ///
    /// 分享面板是这次新增的面板，截图验收时用它检查排版；
    /// 传 `("", false, false)` 就全部收起。
    pub fn with_open_panels(mut self, share: bool, coin: bool, fav: bool) -> Self {
        self.share_panel_open = share;
        self.coin_panel_open = coin;
        self.fav_panel_open = fav;
        if fav {
            // 截图用的夹具按线上真实形态铺：十几夹、几十条、两条已收藏，
            // 这样折叠 / 展开、勾选态、长标题截断都能一次看到。
            let fixtures: [(&str, u64, i64); 14] = [
                ("默认收藏夹", 257, 1),
                ("硬件", 2, 1),
                ("硬件科普", 1, 0),
                ("身体护理", 1, 0),
                ("音乐", 14, 0),
                ("NAS", 3, 0),
                ("锻炼", 3, 0),
                ("Swift", 1, 0),
                ("tauri", 1, 0),
                ("AI", 12, 0),
                ("原神", 15, 0),
                ("弹射", 1, 0),
                ("环", 1, 0),
                ("书", 4, 0),
            ];
            self.fav_folders = fixtures
                .iter()
                .enumerate()
                .map(|(index, (title, count, state))| yk_core::FavFolder {
                    id: index as i64 + 1,
                    title: (*title).into(),
                    media_count: *count,
                    attr: if index == 0 { 1 } else { 0 },
                    fav_state: *state,
                })
                .collect();
        }
        self
    }

    pub fn set_image_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, Vec<String>, usize) + 'static,
    ) {
        self.image_handler = Some(std::rc::Rc::new(handler));
    }

    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App, bool) + 'static) {
        self.close_handler = Some(Box::new(handler));
    }

    /// 注入「点头像打开 UP 主浮窗」的回调。播放器不认识 AppRoot，
    /// 只把 mid / 昵称 / 头像交出去，开哪一层由根节点决定。
    pub fn set_up_profile_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, i64, String, String, Bounds<Pixels>) + 'static,
    ) {
        self.up_profile_handler = Some(Box::new(handler));
    }

    fn escape_hint_overlay(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.escape_hint {
            return None;
        }
        Some(
            div()
                .id("player-esc-hint")
                .absolute()
                // 顶栏右侧：在浮窗/视频之外，不遮画面。
                .top(px((TOP_BAR_HEIGHT - 32.) / 2.))
                .right(px(SPACE_4))
                .occlude()
                .h(px(32.))
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(t.surfaces.surface_elevated_strong)
                .border_1()
                .border_color(t.surfaces.border_popover)
                .shadow(t.shadows.shadow_2.clone())
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_SM))
                        .bg(t.palette.status_error)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child("×"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(t.surfaces.text_secondary)
                        .child("再次按 Esc 关闭"),
                )
                .child(
                    div()
                        .px(px(4.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .border_1()
                        .border_color(t.surfaces.border_strong)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(t.surfaces.text_tertiary)
                        .child("Esc"),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.request_close(window, cx);
                }))
                .into_any_element(),
        )
    }

    /// 第一次 Esc：显示提示，5 秒后自动收回；期间再按 Esc 才关闭。
    fn arm_escape_hint(&mut self, cx: &mut Context<Self>) {
        self.escape_hint = true;
        let generation = self.escape_hint_generation.wrapping_add(1);
        self.escape_hint_generation = generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.escape_hint_generation == generation && view.escape_hint {
                    view.escape_hint = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn request_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.begin_close(cx);
    }

    /// 关闭状态机本体；不依赖 Window，方便关闭请求从 Esc/遮罩/按钮统一进入。
    fn begin_close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        // 关闭前补最后一拍；打开不足 5s 的会在 flush 里被冷却拦掉。
        self.flush_history_report();
        // 关闭时把当前位置存进本地续播记录，下次打开从这里继续。
        self.save_resume_position();
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);

        // 先恢复页面布局：关闭起点是“打开前”的封面坐标，分类条必须回来，
        // 否则网格还压在上移后的位置，缩回终点会对不上卡片。
        if let Some(handler) = self.close_handler.as_ref() {
            handler(cx, false);
        }

        // 没有封面快照（YK_BVID/YK_VIDEO 程序化打开）就直接收掉。
        if self.entrance_source.is_none() {
            if let Some(handler) = self.close_handler.as_ref() {
                handler(cx, true);
            }
            return;
        }

        self.closing = true;
        self.closing_generation = EXIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.closing_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(PLAYER_EXIT_MS + 30))
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.closing || view.closing_generation != generation {
                    return;
                }
                view.closing = false;
                if let Some(handler) = view.close_handler.as_ref() {
                    handler(cx, true);
                }
            });
        })
        .detach();
    }

    /// 播放到结尾：按设置决定是否自动收起播放器。
    ///
    /// 关闭走的是和 Esc / 点浮窗外侧同一条 [`Self::begin_close`] 路径，
    /// 所以退出动画、历史上报、续播落盘都不会漏。
    /// 设置项关闭时只停在最后一帧并叫出控制栏，交给用户自己关。
    fn on_playback_finished(&mut self, cx: &mut Context<Self>) {
        if self.finish_handled {
            return;
        }
        self.finish_handled = true;
        self.state.playing = false;
        // 位置对齐到结尾：续播记录据此判定「已看完」并清掉，下次从头播。
        if self.state.duration_secs > 0.0 {
            self.play_time = self.state.duration_secs;
            self.state.position_secs = self.play_time;
        }
        self.save_resume_position();
        if !yk_core::settings::current().close_player_on_finish {
            eprintln!("[player] 播放结束：自动关闭已在设置里关闭，保留浮窗");
            self.controls_visible = true;
            self.last_activity = std::time::Instant::now();
            cx.notify();
            return;
        }
        eprintln!("[player] 播放结束：自动关闭播放器");
        self.begin_close(cx);
    }

    fn panel_width(&self) -> f32 {
        if self.expanded {
            INFO_PANEL_EXPANDED
        } else {
            INFO_PANEL_COLLAPSED
        }
    }

    /// 视频按 Contain 放进播放层后的实际宽度与左偏移。
    ///
    /// 播放层宽度 = 窗口 − 两侧 16 − 收起态侧栏 200；高度 = 窗口 − 顶栏下方 16
    /// − 底部 92。控制条跟着这段**实际画面**走，而不是铺满整个播放层。
    fn video_bar_geometry(&self, window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();
        let available_w = playback_layer_width(window, INFO_PANEL_COLLAPSED);
        let available_h =
            (viewport.height.as_f32() - (TOP_BAR_HEIGHT + SPACE_4) - PLAYER_BOTTOM_INSET).max(1.0);
        let (video_w, video_h) = self.video_pixel_size();
        if video_w <= 0.0 || video_h <= 0.0 {
            return (0.0, available_w);
        }
        let scale = (available_w / video_w).min(available_h / video_h);
        let width = (video_w * scale).min(available_w);
        ((available_w - width) / 2.0, width)
    }

    /// Windows：把最新一帧 BGRA 包成 GPUI 的 `RenderImage`。
    ///
    /// 两个必须做的收尾：
    /// 1. 每帧新建 `RenderImage` 会拿到新 `ImageId`，GPUI 的 DirectX 图集会为它
    ///    分配一张纹理；不显式 `drop_image` 旧图，显存会随播放时间线性上涨。
    /// 2. 同一帧号不重复建图：render 可能被 hover / 动画触发多次，
    ///    但视频帧（16ms 一条）并没有变。
    #[cfg(target_os = "windows")]
    fn sync_video_image(&mut self, window: &mut Window) {
        if self.needs_image_clear {
            self.needs_image_clear = false;
            if let Some(old) = self.current_image.take() {
                let _ = window.drop_image(old);
            }
            self.current_image_serial = None;
        }

        let Some(serial) = self.current.as_ref().map(|frame| frame.serial) else {
            return;
        };
        if self.current_image_serial == Some(serial) {
            return;
        }

        let frame = self.current.take().expect("上面刚确认过 current 存在");
        let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra) else {
            eprintln!(
                "[player] 视频帧尺寸不匹配：{}x{} / {} 字节",
                frame.width,
                frame.height,
                frame.width as usize * frame.height as usize * 4
            );
            return;
        };
        // 字节序已经是 BGRA（mpv 的 `bgra` 格式），而 `image::Frame` 只是
        // 一个「RgbaImage 容器」；GPUI 的 `paint_image` 原样上传，
        // 所以这里不需要再做通道交换。
        let image = std::sync::Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        if let Some(old) = self.current_image.replace(image) {
            let _ = window.drop_image(old);
        }
        self.current_image_serial = Some(serial);
    }

    /// 当前视频的像素尺寸：优先用已解码帧，其次用 playurl 返回的分辨率。
    fn video_pixel_size(&self) -> (f32, f32) {
        #[cfg(target_os = "macos")]
        if let Some(buffer) = self.current.as_ref() {
            let width = objc2_core_video::CVPixelBufferGetWidth(buffer) as f32;
            let height = objc2_core_video::CVPixelBufferGetHeight(buffer) as f32;
            if width > 0.0 && height > 0.0 {
                return (width, height);
            }
        }
        if let Some(source) = self.source.as_ref() {
            if source.video_width > 0 && source.video_height > 0 {
                return (source.video_width as f32, source.video_height as f32);
            }
        }
        (0.0, 0.0)
    }

    /// 指针是否还在右侧面板的交互区（面板 + 展开态骑边界按钮的 16px 外沿）。
    ///
    /// 收起态只认 200px 面板本身：视频右缘不再铺 88px 唤出热区，
    /// 鼠标在画面上移动不会把侧栏弹出来。
    /// GPUI 的 `on_hover(false)` 会在布局变化时重算；播放中每帧都有重绘，
    /// 面板内滚动 / 控制条显隐都可能让它在指针没动时闪一次 false。
    /// 收起前用真实鼠标坐标兜底，避免面板被这种假移出事件收掉。
    fn pointer_in_panel_zone(&self, position: Point<Pixels>, window: &Window) -> bool {
        let viewport = window.viewport_size();
        // 播放器窗口贴窗口右缘内缩 SPACE_4；面板再贴播放器右缘。
        // 右边界以内缩后的面板右缘为准：移出到窗口最右 16px 边距
        // 也算离开 hover 区，延迟 220ms 后收起面板。
        let right = viewport.width - px(SPACE_4);
        // 展开态给骑边界按钮留 16px；收起态就是面板本身。
        let width = if self.expanded {
            INFO_PANEL_EXPANDED + 16.0
        } else {
            INFO_PANEL_COLLAPSED
        };
        let top = px(TOP_BAR_HEIGHT + SPACE_4);
        let bottom = viewport.height - px(PLAYER_BOTTOM_INSET);
        position.x >= right - px(width)
            && position.x <= right + px(1.)
            && position.y >= top - px(1.)
            && position.y <= bottom + px(1.)
    }

    /// 指针是否还在窗口内（含四周 16px 边距）。
    fn pointer_in_window(&self, position: Point<Pixels>, window: &Window) -> bool {
        let viewport = window.viewport_size();
        position.x >= px(0.)
            && position.x <= viewport.width
            && position.y >= px(0.)
            && position.y <= viewport.height
    }

    /// 指针是否还在播放器窗口内（不是顶栏 / Dock / 页面）。
    fn pointer_in_player(&self, position: Point<Pixels>, window: &Window) -> bool {
        let viewport = window.viewport_size();
        let top = px(TOP_BAR_HEIGHT + SPACE_4);
        let bottom = viewport.height - px(PLAYER_BOTTOM_INSET);
        position.x >= px(SPACE_4) - px(1.)
            && position.x <= viewport.width - px(SPACE_4) + px(1.)
            && position.y >= top - px(1.)
            && position.y <= bottom + px(1.)
    }
}

/// 入场/退场共用一条平滑曲线：EaseStandard 起手，尾部约 0.9% 过冲后
/// 自然回到 1。单段曲线没有速度断点，缩放过程不会在 300ms 附近「顿一下」。
fn entrance_progress(raw: f32) -> f32 {
    ease_entrance()(raw.clamp(0.0, 1.0))
}

fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |a: Pixels, b: Pixels| px(a.as_f32() + (b.as_f32() - a.as_f32()) * t);
    Bounds {
        origin: point(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        size: size(
            mix(a.size.width, b.size.width),
            mix(a.size.height, b.size.height),
        ),
    }
}

/// 入场期的 Esc 也从同一个焦点句柄进；用弱实体回到 PlayerView 的状态机。
fn player_escape_handler(
    weak: WeakEntity<PlayerView>,
) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    move |event, window, cx| {
        if event.keystroke.key != "escape" {
            return;
        }
        let _ = weak.update(cx, |view, cx| {
            if view.escape_hint {
                view.request_close(window, cx);
            } else {
                view.arm_escape_hint(cx);
            }
        });
        cx.stop_propagation();
    }
}

/// 回车键位提示（分享面板「复制链接」行尾）。
///
/// 用组件库的 `Kbd`：macOS 上渲染成 `⏎`，别的平台渲染成 `Enter` ——
/// 平台自带的键位符号，比自己拼一个文本胶囊更贴手。
fn enter_key_hint() -> AnyElement {
    match Keystroke::parse("enter") {
        Ok(stroke) => Kbd::new(stroke).into_any_element(),
        // 解析不出来就不画提示：宁可没有，也不能画一个假的键位。
        Err(_) => div().into_any_element(),
    }
}

/// 入场/退场共用的封面海报：渐变兜底 + 标题垫底 + 真实缩略图。
fn cover_poster(theme: &YkTheme, source: &CoverSource) -> AnyElement {
    let mut poster = div().absolute().inset_0().bg(crate::placeholder_gradient(
        &source.placeholder_color,
        theme.surfaces.fill_3,
    ));
    if !source.title.is_empty() {
        poster = poster.child(
            div()
                .absolute()
                .inset_0()
                .p(px(SPACE_3))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(16.))
                .line_height(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .text_center()
                .line_clamp(3)
                .child(source.title.clone()),
        );
    }
    if !source.image_url.is_empty() {
        poster = poster.child(
            img(ImageSource::from(source.image_url.clone()))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    poster.into_any_element()
}

/// 从封面几何展开到播放器目标尺寸。
///
/// 过渡期只挂封面海报，到 `progress == 1` 才挂完整播放器内容：
/// 控制栏/信息面板不会被塞进 192×108 的盒子，也不会让 deferred 的
/// 面板唤回按钮从窗口边缘跑出去。
#[derive(gpui::IntoElement)]
struct PlayerEntrance {
    theme: YkTheme,
    source: CoverSource,
    content: Option<AnyElement>,
    focus_handle: FocusHandle,
    weak: WeakEntity<PlayerView>,
    progress: f32,
}

impl RenderOnce for PlayerEntrance {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PlayerEntrance {
            theme,
            source,
            content,
            focus_handle,
            weak,
            progress,
        } = self;
        let p = entrance_progress(progress);
        let root_w = source.root_size.width.as_f32().max(2.0 * SPACE_4 + 1.0);
        let root_h = source
            .root_size
            .height
            .as_f32()
            .max(TOP_BAR_HEIGHT + SPACE_4 + PLAYER_BOTTOM_INSET + 1.0);
        let top = TOP_BAR_HEIGHT + SPACE_4;
        let destination = Bounds {
            origin: point(px(SPACE_4), px(top)),
            size: size(
                px(root_w - 2.0 * SPACE_4),
                px((root_h - top - PLAYER_BOTTOM_INSET).max(1.0)),
            ),
        };
        let bounds = lerp_bounds(source.bounds, destination, p);

        let shell = div()
            .id("player-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(0x0B0F14))
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude()
            .key_context("Player")
            .track_focus(&focus_handle)
            .on_key_down(player_escape_handler(weak));

        if progress >= 1.0 {
            return shell.child(content.expect("入场结束时必须挂上播放器内容"));
        }

        // 后段交叉淡化：海报淡出、交互界面淡入；前半段仍只挂海报，
        // 避免控制栏/侧栏被塞进 192×108 的盒子逐帧重排。
        let fade = ((p - ENTRANCE_CONTENT_FADE_START) / (1.0 - ENTRANCE_CONTENT_FADE_START))
            .clamp(0.0, 1.0);
        let fade = fade * fade * (3.0 - 2.0 * fade);

        let mut shell = shell.child(
            div()
                .absolute()
                .inset_0()
                .opacity(1.0 - fade)
                .child(cover_poster(&theme, &source)),
        );

        if fade > 0.0 {
            let content = content.expect("开始淡入后必须有播放器内容");
            shell = shell.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    // 交互界面直接按最终尺寸排版，外形交给 shell 的
                    // overflow_hidden 裁剪：盒子变大时控件不跟着重排。
                    .w(px(destination.size.width.as_f32()))
                    .h(px(destination.size.height.as_f32()))
                    .opacity(fade)
                    .child(content),
            );
        }

        shell
    }
}

/// 关闭缩回：从当前播放器目标框缩回点击时的封面几何。
///
/// 过渡期只保留当前视频帧（没有帧时用封面海报），不挂控制栏/信息面板，
/// 避免窗口缩小时控件被重排得乱跳。
#[derive(gpui::IntoElement)]
struct PlayerExit {
    theme: YkTheme,
    source: CoverSource,
    visual: Option<AnyElement>,
    progress: f32,
}

impl RenderOnce for PlayerExit {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PlayerExit {
            theme,
            source,
            visual,
            progress,
        } = self;
        let p = entrance_progress(progress);
        let q = 1.0 - p;
        let root_w = source.root_size.width.as_f32().max(2.0 * SPACE_4 + 1.0);
        let root_h = source
            .root_size
            .height
            .as_f32()
            .max(TOP_BAR_HEIGHT + SPACE_4 + PLAYER_BOTTOM_INSET + 1.0);
        let top = TOP_BAR_HEIGHT + SPACE_4;
        let destination = Bounds {
            origin: point(px(SPACE_4), px(top)),
            size: size(
                px(root_w - 2.0 * SPACE_4),
                px((root_h - top - PLAYER_BOTTOM_INSET).max(1.0)),
            ),
        };
        let bounds = lerp_bounds(source.bounds, destination, q);
        let shell = div()
            .id("player-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(0x0B0F14))
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude();
        shell.children(visual)
    }
}

/// 当前登录会话；未登录/旧文件损坏返回 None。
fn current_session() -> Option<yk_core::models::UserSession> {
    let path = yk_core::session::session_path();
    yk_core::session::load(&path).filter(|s| s.is_valid())
}

/// 登录会话的 Cookie 头；未登录/旧文件损坏返回空串（仍可取 480P）。
fn current_session_cookie() -> String {
    current_session()
        .map(|s| s.cookie_header())
        .unwrap_or_default()
}

/// 系统级鼠标位置（全局显示坐标，原点在主屏左上角）。
///
/// `Window::mouse_position()` 只是「最后一次鼠标事件」的坐标；指针快速移出
/// 窗口时可能收不到新事件，读到的还是旧位置。CoreGraphics 这里直接问系统
/// 当前光标在哪，即使窗口不是 key/指针已在窗口外也能拿到。
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[cfg(target_os = "macos")]
fn system_mouse_position() -> Option<Point<Pixels>> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const std::ffi::c_void);
    }
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let location = CGEventGetLocation(event);
        CFRelease(event as *const std::ffi::c_void);
        Some(point(px(location.x as f32), px(location.y as f32)))
    }
}

/// 窗口所在显示器在 CoreGraphics 全局坐标里的左上角。
///
/// **这是多屏修复的关键**：GPUI 的 `Window::bounds()` 在 macOS 上是
/// 「相对当前屏幕」的坐标（窗口左上角减去屏幕左上角），而
/// `CGEventGetLocation` 给的是**跨屏全局坐标**（主屏左上角为 0）。
/// 窗口在主屏时两者恰好重合；拖到副屏后直接相减会把光标算到窗口外，
/// 侧栏就永远判定不到，表现为「换屏后侧栏无法展开」。
#[cfg(target_os = "macos")]
fn window_display_origin(window: &Window, cx: &App) -> Option<Point<Pixels>> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGDisplayBounds(display: u32) -> CGRect;
    }
    let display = window.display(cx)?;
    let raw: u64 = display.id().into();
    let bounds = unsafe { CGDisplayBounds(raw as u32) };
    Some(point(
        px(bounds.origin.x as f32),
        px(bounds.origin.y as f32),
    ))
}

#[cfg(not(target_os = "macos"))]
fn window_display_origin(_window: &Window, _cx: &App) -> Option<Point<Pixels>> {
    None
}

/// 非 macOS 没有 CoreGraphics 的全局光标查询；退回最后一次事件位置。
#[cfg(not(target_os = "macos"))]
fn system_mouse_position() -> Option<Point<Pixels>> {
    None
}

/// 测试钩子：把光标位置固定下来（`None` = 用真实光标）。
///
/// headless 测试里系统光标在别的窗口上，`cursor_position` 读到的
/// 位置和测试模拟的鼠标完全对不上，判定会跑偏。
static TEST_CURSOR: std::sync::Mutex<Option<(f32, f32)>> = std::sync::Mutex::new(None);

/// 测试入口：固定光标位置；传 `None` 恢复真实光标。
pub fn set_test_cursor(position: Option<Point<Pixels>>) {
    let mut guard = TEST_CURSOR.lock().unwrap();
    *guard = position.map(|p| (p.x.as_f32(), p.y.as_f32()));
}

/// 光标**当前**的位置，换算成窗口局部坐标。
///
/// 侧栏的展开 / 收起必须用这个，不能用 `Window::mouse_position()`：
/// 后者只是「最后一次鼠标事件」的位置 —— 鼠标移出窗口后没有新事件，
/// 它会一直停在旧位置。于是会出现「指针其实在窗口外，但判定说还在
/// 面板里」：面板被反复弹出 / 收起（鼠标在面板左半区跑动时尤其明显）。
///
/// 三套坐标要对齐：
/// - `CGEventGetLocation`：跨屏全局坐标（主屏左上角为 0）；
/// - `window.display(cx)` + `CGDisplayBounds`：窗口所在屏幕的全局左上角；
/// - `window.bounds().origin`：macOS 上**相对当前屏幕**的左上角。
///
/// 先减屏幕原点得到「屏幕内坐标」，再减窗口原点才是窗口局部坐标。
/// 只减窗口原点只在主屏成立；窗口拖到副屏后侧栏会彻底判定不到。
/// 三套全局坐标 → 窗口局部坐标。
///
/// `global` 是 CoreGraphics 的跨屏坐标；`display_origin` 是窗口所在屏幕的
/// 左上角；`window_origin` 是 GPUI 给的「屏幕内」窗口左上角。
fn cursor_local(
    global: Point<Pixels>,
    display_origin: Point<Pixels>,
    window_origin: Point<Pixels>,
) -> Point<Pixels> {
    point(
        global.x - display_origin.x - window_origin.x,
        global.y - display_origin.y - window_origin.y,
    )
}

fn cursor_position(window: &Window, cx: &App) -> Point<Pixels> {
    if let Some((x, y)) = *TEST_CURSOR.lock().unwrap() {
        return point(px(x), px(y));
    }
    system_mouse_position()
        .map(|global| {
            let display = window_display_origin(window, cx)
                .unwrap_or_else(|| point(px(0.), px(0.)));
            cursor_local(global, display, window.bounds().origin)
        })
        .unwrap_or_else(|| window.mouse_position())
}

/// 把一次历史上报快照丢到后台线程发送。
///
/// 不挂在 `cx.spawn` 上：关闭动画结束时 `PlayerView` 会被回收，
/// 挂在实体上的任务可能一起被取消；最后一次上报必须独立于实体生命周期。
fn dispatch_history_report(report: &HistoryReport, progress: u64) {
    let payload = yk_core::api::client::HistoryProgress {
        aid: report.aid,
        cid: report.cid,
        progress,
    };
    let cookie = report.cookie.clone();
    let csrf = report.csrf.clone();
    eprintln!(
        "[history-report] 上报 {}：progress={}s",
        report.bvid, payload.progress
    );
    std::thread::spawn(move || {
        let client = match yk_core::api::client::ReqwestClient::new() {
            Ok(client) => client,
            Err(error) => {
                eprintln!("[history-report] 创建客户端失败：{error}");
                return;
            }
        };
        if let Err(error) = yk_core::api::client::report_history(&client, &cookie, &csrf, &payload)
        {
            eprintln!("[history-report] 上报失败：{error}");
        }
    });
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lane_capacity = scroll_lane_capacity(window);
        if lane_capacity != self.scroll_lanes {
            self.scroll_lanes = lane_capacity;
            self.reflow_danmaku();
        }
        if !self.hover_watchdog_started {
            self.hover_watchdog_started = true;
            // 鼠标快速移出窗口时，GPUI 可能来不及派发 MouseMove / MouseExit。
            // 直接周期读取系统鼠标位置（CoreGraphics 的 CGEventGetLocation，
            // 即使指针已经在窗口外也会给出坐标），再走同一个 220ms 延迟收起。
            cx.spawn_in(window, async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let alive = this.update_in(cx, |view, window, cx| {
                    if !view.expanded {
                        return;
                    }
                    let position = cursor_position(window, cx);
                    if view.pointer_in_panel_zone(position, window) {
                        // 已经回到 hover 区：取消可能排着的收起。
                        if view.panel_collapse_pending {
                            view.panel_collapse_pending = false;
                            view.panel_collapse_generation =
                                view.panel_collapse_generation.wrapping_add(1);
                        }
                        return;
                    }
                    // 系统光标已经在 hover 区外：这是真实移出，
                    // 不受「滚动后 400ms 保持」影响，只保留 220ms 延迟。
                    // 直接 arm，避免再用可能过期的 window.mouse_position() 判断一次。
                    view.panel_last_scroll = None;
                    view.arm_panel_collapse(window, cx);
                });
                if alive.is_err() {
                    break;
                }
            })
            .detach();
        }
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.focus_handle.clone();
            // 等这一帧的元素树挂上 track_focus 后再交焦点，避免首帧焦点丢失。
            cx.defer_in(window, move |_view, window, cx| {
                window.focus(&handle, cx);
            });
        }
        if self.submit_requested {
            self.submit_requested = false;
            cx.defer_in(window, |view, window, cx| view.send_danmaku(window, cx));
        }
        // 渲染线程：有播放源才启动；换源时 open_bvid 会把 tried_worker 复位。
        // 没有播放源时只有 YK_VIDEO 验收入口会退回本地测试文件，
        // 正常点卡片期间（loading_source）绝不先播测试视频。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if !self.tried_worker {
            let source = self
                .source
                .as_ref()
                .map(|s| (s.video_url.clone(), s.audio_url.clone()));
            let fallback =
                if source.is_none() && !self.loading_source && self.source_error.is_none() {
                    std::env::var("YK_VIDEO").ok()
                } else {
                    None
                };
            if let Some((video, audio)) = source.or_else(|| fallback.map(|v| (v, None))) {
                self.tried_worker = true;
                // 起播位置（续播）：mpv 打开文件时直接定位过去。
                self.worker = crate::video_worker::FrameWorker::spawn_with_audio(
                    video,
                    audio,
                    1280,
                    720,
                    self.start_position,
                );
                // 取流期间用户可能已经按过暂停：新 worker 启动后补发一次状态。
                if let Some(w) = self.worker.as_ref() {
                    w.set_paused(!self.state.playing);
                    // 窗口当前不可见（最小化 / 被完全遮挡）：新 worker 直接停帧，
                    // 别先解码几帧再停。
                    w.set_render_paused(self.visibility.render_paused());
                }
                // 渲染循环已在同一个 render 里、worker 创建之后立即启动；
                // 这里再 notify 一次，让首帧尽快重绘，而不是等下一次用户事件。
                if self.worker.is_some() {
                    cx.notify();
                }
            }
        }

        // 播放循环：每 16ms 取一帧最新画面并重绘（≈60fps）。
        // **必须放在 worker 创建之后**：以前放在 render 开头，靠 cx.notify()
        // 等下一轮 render 才启动；窗口未激活/被遮挡时下一轮可能永远不来，
        // 表现就是音频在播、画面不出，点一下播放器触发 render 后才出首帧。
        // libmpv 已在自动播放，这里只负责「把当前帧搬到 UI 上」。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if !self.loop_started && self.worker.is_some() {
            self.loop_started = true;
            cx.spawn(async move |this, cx| {
                let mut received = 0u64;
                let mut idle_ticks = 0u32;
                loop {
                    // 轮询间隔：窗口可见时 16ms（≈60fps）；不可见时 100ms。
                    //
                    // 窗口不可见时视频线程本来就只报位置（100ms 一条），
                    // UI 线程再按 16ms 醒就是纯空转 —— 每秒 60 次定时器唤醒，
                    // 在电池供电的笔记本上是实打实的耗电。
                    let tick = this
                        .update(cx, |view, _| {
                            if view.visibility.is_visible() {
                                16
                            } else {
                                100
                            }
                        })
                        .unwrap_or(16);
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(tick))
                        .await;
                    // UI 线程只做「取一帧 + 通知重绘」，不碰任何解码/转换。
                    // 这就是输入与播放互不耦合的关键：即使正在打字，
                    // 渲染线程照常出帧，UI 也只是轻量地更新一下。
                    let alive = this.update(cx, |view, cx| {
                        let mut got = false;
                        let mut finished = false;
                        if let Some(w) = view.worker.as_ref() {
                            // 结束信号独立于「有没有新帧」读取：
                            // 播到结尾后渲染线程可能不再产出帧。
                            finished = w.finished();
                            match w.latest() {
                                Some(crate::video_worker::WorkerFrame::Frame(buf, t)) => {
                                    view.current = Some(buf);
                                    // 拖动 / seek 稳定期内不要让旧帧的 time-pos 把滑块顶回去。
                                    let settling = view
                                        .seek_settle_until
                                        .is_some_and(|until| until > std::time::Instant::now());
                                    if !view.scrubbing && !settling {
                                        view.play_time = t;
                                        view.state.position_secs = t;
                                        view.ensure_danmaku_ahead(cx);
                                        // 播放中定期把进度落盘：进程被杀 / 崩溃时
                                        // 也不至于丢掉续播位置。
                                        if t - view.last_resume_save >= 30.0 {
                                            view.last_resume_save = t;
                                            view.save_resume_position();
                                        }
                                    }
                                    got = true;
                                }
                                // 窗口不可见时视频线程只报位置：进度与续播位置照常推进，
                                // 只是没有新画面。这里**不** notify：没人看画面，
                                // 每次位置更新都重绘整窗只是白烧 CPU。
                                Some(crate::video_worker::WorkerFrame::Position(t)) => {
                                    view.play_time = t;
                                    view.state.position_secs = t;
                                    view.ensure_danmaku_ahead(cx);
                                    if t - view.last_resume_save >= 30.0 {
                                        view.last_resume_save = t;
                                        view.save_resume_position();
                                    }
                                }
                                None => {}
                            }
                        }
                        // 播完自动收起：开关关掉时只停在最后一帧，等用户自己关。
                        if finished && !view.finish_handled && !view.closing && !view.scrubbing {
                            view.on_playback_finished(cx);
                        }
                        if got {
                            received += 1;
                            idle_ticks = 0;
                            if received == 1 {
                                eprintln!("[player] 收到首帧");
                            }
                            cx.notify();
                        } else {
                            idle_ticks += 1;
                            if idle_ticks == 125 {
                                idle_ticks = 0;
                                eprintln!("[player] 连续 2s 没有新帧");
                            }
                        }
                        got
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        // 弹幕输入框：首次 render 才有 window，所以在这里创建。
        // 用 gpui-component 的 Input —— 它内部走 InputHandler，
        // 支持 IME 组合（marked text），中文输入必需。
        if self.danmaku_input.is_none() {
            let state =
                cx.new(|cx| InputState::new(window, cx).placeholder("发个友善的弹幕见证当下"));
            // Enter 走 Input 自己的 Enter action（外层补了 key_context("Input") 才匹配），
            // 由它发出 PressEnter。Shift+Enter 不发送。
            // 中文输入法组合中的 Enter 应由输入法先消费（用于选词）；
            // 这条依赖平台 IME 行为，真机选词场景还要再验一次。
            cx.subscribe(&state, |view, _state, event: &InputEvent, cx| {
                if let InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } = event
                {
                    eprintln!("[input] Enter → 发送");
                    view.submit_requested = true;
                    cx.notify();
                }
                if matches!(event, InputEvent::Blur) {
                    // 输入框失焦后不再阻止控制栏自动隐藏。
                    view.input_focused = false;
                    cx.notify();
                }
            })
            .detach();
            eprintln!("[input] InputState 已创建");
            self.danmaku_input = Some(state);
        }

        let t = self.theme.clone();
        // Windows：把最新一帧包成 GPUI 贴图，并回收上一张纹理。
        #[cfg(target_os = "windows")]
        self.sync_video_image(window);
        // 窗口失焦：立即收起，不依赖「下一帧」（暂停时没有视频帧驱动 render）。
        if !self.activation_observed {
            self.activation_observed = true;
            // 放到本帧之后注册：observe_window_activation 会立刻回调一次，
            // 直接在 render 里注册会造成重入。
            cx.defer_in(window, |view, window, cx| {
                view.activation_observer =
                    Some(cx.observe_window_activation(window, |view, window, cx| {
                        if !window.is_window_active() {
                            // 失焦时输入框也拿不到键盘，直接收起。
                            view.input_focused = false;
                            view.hide_controls(cx);
                        }
                    }));
            });
        }
        if !window.is_window_active() && self.controls_visible {
            self.controls_visible = false;
            self.controls_timer_generation = self.controls_timer_generation.wrapping_add(1);
            self.controls_timer_running = false;
        }
        // 窗口可见性（最小化 / 被完全遮挡）：视频线程停帧，UI 也别再空转。
        //
        // 失焦**不**走这里：失焦时窗口仍然可见，视频必须照常按标准帧率播，
        // 否则「点了别的应用之后本应用变卡」。真正看不见了才降载。
        if !self.visibility_observed {
            self.visibility_observed = true;
            cx.defer_in(window, |view, window, cx| {
                view.visibility_observer =
                    Some(cx.observe_window_visibility(window, |view, visibility, _window, cx| {
                        view.set_window_visible(visibility.is_visible(), cx);
                    }));
            });
        }
        // 入场目标宽度跟随当前窗口；封面起点仍是点击时冻结的那一份。
        if let Some(source) = self.entrance_source.as_mut() {
            source.root_size = window.viewport_size();
        }
        // 控制区元素提前建好：可见时才挂树，隐藏时只留一条 3px 进度线。
        let watch_badge = self.controls_visible.then(|| self.watch_badge(cx));
        let bar: Option<AnyElement> = if self.controls_visible {
            let (video_left, video_width) = self.video_bar_geometry(window);
            Some(
                control_bar(
                    &t,
                    &self.state,
                    self.danmaku_input.as_ref(),
                    self.danmaku_sending,
                    self.source
                        .as_ref()
                        .map(|source| source.quality.as_str())
                        .unwrap_or(""),
                    video_left,
                    video_width,
                    &self.progress_track_bounds,
                    cx,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.toggle_playback(window, cx);
                    }),
                )
                .into_any_element(),
            )
        } else {
            None
        };
        let mini_progress = (!self.controls_visible).then(|| self.mini_progress(&t, window, cx));
        // 弹幕只在「播放层」里跑：右侧面板那 200/400px 不穿弹幕（设计第 14 节）。
        let danmaku_width = playback_layer_width(window, self.panel_width());
        // 浮窗内部：绝对铺满窗口。窗口本身负责圆角/裁剪/阴影/遮挡。
        let content = div()
            .id("player-content")
            .absolute()
            .inset_0()
            .bg(rgb(0x0B0F14))
            // ① 视频层：只占红框（播放层）宽度 = 窗口 − 右侧侧栏 200；
            //    侧栏是常驻占位，不算播放层。Contain 在这个红框里居中。
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .right(px(INFO_PANEL_COLLAPSED))
                    .child({
                        #[cfg(target_os = "macos")]
                        {
                            video_layer(&self.frame, self.current.as_ref())
                        }
                        #[cfg(target_os = "windows")]
                        {
                            video_layer(&self.frame, self.current_image.as_ref())
                        }
                        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                        {
                            video_layer(&self.frame)
                        }
                    }),
            )
            // ①b 弹幕层：gpui-fast 已解决 surface 与文字的合成冲突，
            // 直接 canvas 绘制即可（见附录 D 事实 16）。
            // 左下角眼睛按钮关掉后整层不挂树。
            .children(if self.danmaku_visible {
                Some(self.danmaku_layer(danmaku_width))
            } else {
                None
            })
            // ①c 取流状态：加载中 / 失败说明，不遮挡控制栏与右侧面板
            .children(
                if self.loading_source {
                    Some("正在获取播放地址…".to_string())
                } else {
                    self.source_error.clone()
                }
                .map(|text| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .bottom_0()
                        .right(px(INFO_PANEL_COLLAPSED))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(rgb(0xFFFFFF))
                        .child(text)
                        .into_any_element()
                }),
            )
            // ①e 暂停态中央播放按钮
            .child(self.center_play(&t, cx))
            // ② 控制栏黑框 + 左下角标：只在鼠标活动时挂树；隐藏时留一条 3px 进度线。
            .children(watch_badge)
            // ③ 控制栏右端压在面板/视频边界上，仍保持可点击。
            .children(bar)
            .children(mini_progress)
            // ④ 信息面板：通高浮层，从播放页层顶一直到底
            .child(self.info_panel(&t, window, cx))
            // ⑤ 骑边界按钮：面板之后挂载 → 压在面板之上（GPUI 没有 z-index）
            .child(self.panel_toggle(&t, cx))
            // ⑥ 互动反馈 toast：窗口底部居中，浮在视频与右侧面板之上。
            .children(self.interaction_toast(&t))
            // 鼠标移动 / 点击都算活动：把控制栏叫回来并重新计时 5 秒。
            .on_mouse_move(cx.listener(|view, _: &MouseMoveEvent, window, cx| {
                view.note_activity(window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, window, cx| {
                    view.note_activity(window, cx);
                }),
            )
            // 鼠标离开播放器（去了顶栏 / 页面 / 其它窗口）：控制栏直接收起。
            .on_hover(cx.listener(|view, hovered: &bool, window, cx| {
                if !*hovered {
                    // 用当前光标位置：`window.mouse_position()` 停在最后一次
                    // 事件上，鼠标已经移出窗口时它还是旧位置。
                    let position = cursor_position(window, cx);
                    // 播放中每帧重绘会让 hover 偶发 false；指针仍在播放器里就忽略。
                    if !view.pointer_in_player(position, window) {
                        // 指针真的离开播放器（含移进 16px 边距）：解除滚动保持，
                        // 让 220ms 延迟收起照常生效。
                        view.panel_last_scroll = None;
                        view.hide_controls(cx);
                        view.set_panel_hover(false, window, cx);
                    }
                }
            }))
            // 全局鼠标位置监听：不依赖 hover 命中，面板收/展以它为准。
            .child(self.pointer_tracker(cx))
            .into_any_element();

        // 浮窗外侧点击层：从顶栏下方开始，顶栏本身保持可用。
        let backdrop = div()
            .id("player-backdrop")
            .absolute()
            .top(px(TOP_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            // 极低透明度背景：确保这层一定参与命中测试（纯 alpha=0 的 div
            // 在部分路径下会被当成不绘制元素，点击直接落到下层封面卡）。
            .bg(rgba(0x00000001))
            // 挡住下层卡片/页面：点浮窗外侧只关闭，不触发新卡片。
            .occlude()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                // 关闭浮窗的这一拍不要再把 click 传给下层封面卡。
                cx.stop_propagation();
                this.request_close(window, cx);
            }));

        // 页面遮罩：入场时从透明压到 62%，把「旧页面」压暗；
        // 播放器内容同时在后段淡入，避免卡片放大结束的一瞬间硬切界面。
        // 关闭时反向淡出，页面重新亮起。
        let scrim = div()
            .absolute()
            .top(px(TOP_BAR_HEIGHT))
            .left_0()
            .right_0()
            // 底边停在播放器下沿，Dock 保持原亮度。
            .bottom(px(PLAYER_BOTTOM_INSET))
            .bg(rgb(0x000000));
        let scrim = if self.closing {
            scrim
                .with_animation(
                    format!("player-scrim-close-{}", self.closing_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_EXIT_MS)),
                    |el, p| el.opacity(PLAYER_SCRIM_OPACITY * (1.0 - p)),
                )
                .into_any_element()
        } else {
            scrim
                .with_animation(
                    format!("player-scrim-{}", self.entrance_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_ENTRANCE_MS)),
                    |el, p| el.opacity(PLAYER_SCRIM_OPACITY * p),
                )
                .into_any_element()
        };

        // 关闭缩回时只保留画面：当前视频帧优先，没有帧就用封面海报。
        // 控制栏/面板不挂树，窗口缩小时不会被重排得乱跳。
        let closing_visual: Option<AnyElement> = if self.closing {
            #[cfg(target_os = "macos")]
            {
                if self.current.is_some() {
                    Some(video_layer(&self.frame, self.current.as_ref()).into_any_element())
                } else {
                    self.entrance_source
                        .as_ref()
                        .map(|source| cover_poster(&t, source))
                }
            }
            #[cfg(target_os = "windows")]
            {
                if self.current_image.is_some() {
                    Some(video_layer(&self.frame, self.current_image.as_ref()).into_any_element())
                } else {
                    self.entrance_source
                        .as_ref()
                        .map(|source| cover_poster(&t, source))
                }
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                self.entrance_source
                    .as_ref()
                    .map(|source| cover_poster(&t, source))
            }
        } else {
            None
        };

        // 卡片打开：用同一套窗口外壳，从点击时的封面几何展开到目标尺寸。
        // 动画进度来自稳定 id 的 Animation 时钟；render 里绝不新建起点。
        let window: AnyElement = if self.closing {
            let source = self
                .entrance_source
                .clone()
                .expect("关闭缩回必须有封面快照");
            let exit = PlayerExit {
                theme: t.clone(),
                source,
                visual: closing_visual,
                progress: 0.0,
            };
            exit.with_animation(
                format!("player-exit-{}", self.closing_generation),
                Animation::new(std::time::Duration::from_millis(PLAYER_EXIT_MS)).with_easing(|t| t),
                |el, progress| PlayerExit { progress, ..el },
            )
            .into_any_element()
        } else if let Some(source) = self.entrance_source.clone() {
            let entrance = PlayerEntrance {
                theme: t.clone(),
                source,
                content: Some(content),
                focus_handle: self.focus_handle.clone(),
                weak: cx.entity().downgrade(),
                progress: 0.0,
            };
            entrance
                .with_animation(
                    format!("player-entrance-{}", self.entrance_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_ENTRANCE_MS))
                        .with_easing(|t| t),
                    |el, progress| PlayerEntrance { progress, ..el },
                )
                .into_any_element()
        } else {
            div()
                .id("player-window")
                .absolute()
                // 跟随窗口拉伸：顶在顶栏下方 16，底在 Dock 上方 20。
                .top(px(TOP_BAR_HEIGHT + SPACE_4))
                .left(px(SPACE_4))
                .right(px(SPACE_4))
                .bottom(px(PLAYER_BOTTOM_INSET))
                .rounded(px(CARD_RADIUS))
                .overflow_hidden()
                .bg(rgb(0x0B0F14))
                .border_1()
                .border_color(t.surfaces.border_popover)
                .shadow(t.shadows.shadow_glass_modal.clone())
                // 浮窗本体截断命中的穿透；子控件仍在更上层。
                .occlude()
                .key_context("Player")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        // 回车：分享面板开着时复制链接（面板上有对应的键位提示）。
                        "enter" => {
                            if this.handle_enter(cx) {
                                cx.stop_propagation();
                            }
                        }
                        "escape" => {
                            // 弹层开着时 Esc 只关弹层（APG：Esc 关闭并把焦点还给触发按钮），
                            // 别一按就进「再按一次退出播放器」——用户想关的是面前这张卡。
                            if this.close_action_overlays(cx) {
                                cx.stop_propagation();
                                return;
                            }
                            if this.escape_hint {
                                this.request_close(window, cx);
                            } else {
                                this.arm_escape_hint(cx);
                            }
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .child(content)
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .child(backdrop)
            .child(scrim)
            .child(window)
            // 第一次 Esc 提示放在浮窗之外：顶栏右侧。
            .children(self.escape_hint_overlay(&t, cx))
            // 评论图片查看器：压在播放器全部内容之上（含侧栏）。

    }
}

// ================= 弹幕层 =================

impl PlayerView {
    /// 把输入框里的文本发到 B 站（`/x/v2/dm/post`），**成功之后才上屏**。
    ///
    /// 以前这里是「本地回显，不接服务端」：字打出去、屏幕上飞过去，服务端一条都没有，
    /// 重新进视频就消失。现在走真写接口：
    /// - 未登录 / 没有 `bili_jct` → 直接提示，不发请求，也不假装成功；
    /// - 失败（风控、等级不足、频率过快…）→ 文本留在输入框里，弹一句服务端的原话；
    /// - 成功 → 才插进上屏列表与「弹幕」Tab，并清空输入框。
    ///
    /// 时间取当前播放位置（毫秒），末尾夹到时长内 —— 超出时长服务端会回
    /// `36714 弹幕发送时间不合法`。
    fn send_danmaku(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.danmaku_sending {
            return;
        }
        let Some(st) = self.danmaku_input.clone() else {
            return;
        };
        let text = st.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再发弹幕".into(), true, cx);
            return;
        };
        if session.bili_jct.trim().is_empty() {
            self.show_interaction_note("登录会话已失效，请重新登录后再发弹幕".into(), true, cx);
            return;
        }
        let Some(source) = self.source.clone() else {
            self.show_interaction_note("视频还没加载完，稍后再发".into(), true, cx);
            return;
        };
        if source.cid <= 0 {
            self.show_interaction_note("当前视频没有弹幕区，无法发送".into(), true, cx);
            return;
        }
        let chars = text.chars().count();
        let max_chars = yk_core::interaction::MAX_DANMAKU_CHARS;
        if chars > max_chars {
            self.show_interaction_note(
                format!("弹幕最长 {max_chars} 字，当前 {chars} 字"),
                true,
                cx,
            );
            return;
        }

        // 播放位置 → 毫秒；时长已知时夹到「时长 - 100ms」，避免末尾几帧被判时间不合法。
        let raw_ms = (self.play_time.max(0.0) * 1000.0).round() as u64;
        let progress_ms = if source.duration_secs > 0 {
            raw_ms.min(source.duration_secs.saturating_mul(1000).saturating_sub(100))
        } else {
            raw_ms
        };

        let (bvid, cid) = (source.bvid.clone(), source.cid);
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        self.danmaku_sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let text_for_send = text.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_core::interaction::send_danmaku(
                        &client,
                        &cookie,
                        &csrf,
                        &bvid,
                        cid,
                        &text_for_send,
                        progress_ms,
                    )
                })
                .await;
            let _ = this.update_in(cx, move |view, window, cx| {
                view.danmaku_sending = false;
                match result {
                    Ok(()) => {
                        // 上屏时间用「此刻」而不是发送时：一次请求的往返里视频又走了几百毫秒，
                        // 用旧时间会让刚发的弹幕直接从屏幕中间冒出来。
                        let shown_at = view.play_time + 0.05;
                        let width = playback_layer_width(window, view.panel_width());
                        view.insert_sent_danmaku(shown_at, &text, width);
                        if let Some(st) = view.danmaku_input.clone() {
                            st.update(cx, |st, cx| st.set_value("", window, cx));
                        }
                        view.show_interaction_note("弹幕已发送".into(), false, cx);
                    }
                    Err(error) => {
                        // 失败不清输入框：字还在，用户改一改就能重发；
                        // 清掉反而会让人以为发出去了（这正是旧实现的毛病）。
                        eprintln!("[danmaku] 发送失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 把刚发送成功的弹幕插进上屏列表与「弹幕」Tab 列表（都保持时间序）。
    fn insert_sent_danmaku(&mut self, time: f64, text: &str, area_width: f32) {
        let d = Danmaku {
            time,
            mode: DanmakuMode::Scroll,
            font_size: 25.0,
            color: (0xFF, 0xFF, 0xFF),
            width: Danmaku::estimate_width(text, 25.0),
            text: text.to_string(),
        };
        // 预分配器状态停在「整段视频排完」，这里按当前时刻找空轨道；
        // 再按时间插回两个列表，保证时间序（面板列表也按时间展示）。
        let lane = self.pick_free_scroll_lane(time, area_width);
        let placed = PlacedDanmaku {
            data: d.clone(),
            lane: DanmakuLane::Scroll(lane),
        };
        let at = self
            .danmaku
            .partition_point(|item| item.data.time <= d.time);
        self.danmaku.insert(at, placed);
        let at = self.danmaku_all.partition_point(|item| item.time <= d.time);
        self.danmaku_all.insert(at, d);
        eprintln!(
            "[danmaku] 已发送，轨道 {lane}，共 {} 条",
            self.danmaku_all.len()
        );
    }
}

impl PlayerView {
    /// 当前时刻该显示的滚动弹幕。
    ///
    /// 每帧只为「可见窗口」内的弹幕生成元素，不是全量 —— 一场直播可能有上万条，
    /// 全量遍历会直接掉帧。窗口前半段留 15 秒是因为一条弹幕最长要飞这么久。
    fn danmaku_layer(&self, area_width: f32) -> impl IntoElement {
        let now = self.play_time;

        // 先在普通 Rust 里把「这一帧要画什么」算好，交给 paint 闭包。
        // 闭包必须 'static，所以数据要 move 进去。
        struct Drawn {
            text: String,
            size: f32,
            color: gpui::Hsla,
            width: f32,
            /// 滚动弹幕的 x；固定弹幕在 paint 里按画布宽度居中。
            scroll_x: Option<f32>,
            /// 滚动弹幕的 y。
            y: f32,
            /// 固定弹幕：(是否顶部, 槽位)。
            fixed: Option<(bool, usize)>,
        }

        // 时间序已经排好，用二分只取「可能还停在屏幕上」的一段；
        // 每帧全量遍历上万条会白烧 CPU。
        let start = self
            .danmaku
            .partition_point(|p| p.data.time < now - SCROLL_LOOKBACK);
        let end = self.danmaku.partition_point(|p| p.data.time <= now);
        let drawn: Vec<Drawn> = self.danmaku[start..end]
            .iter()
            .filter_map(|placed| {
                let d = &placed.data;
                match placed.lane {
                    DanmakuLane::Scroll(lane) => {
                        if !self.lanes.visible_at_in(d, now, area_width) {
                            return None;
                        }
                        Some(Drawn {
                            text: d.text.clone(),
                            size: d.font_size,
                            color: rgb(d.rgb()).into(),
                            width: d.width,
                            scroll_x: Some(self.lanes.x_at_in(d, now, area_width)),
                            y: 8.0 + lane as f32 * SCROLL_LANE_HEIGHT,
                            fixed: None,
                        })
                    }
                    DanmakuLane::Top(slot) => {
                        if !fixed_danmaku_visible(d, now) {
                            return None;
                        }
                        Some(Drawn {
                            text: d.text.clone(),
                            size: d.font_size,
                            color: rgb(d.rgb()).into(),
                            width: d.width,
                            scroll_x: None,
                            y: 0.0,
                            fixed: Some((true, slot)),
                        })
                    }
                    DanmakuLane::Bottom(slot) => {
                        if !fixed_danmaku_visible(d, now) {
                            return None;
                        }
                        Some(Drawn {
                            text: d.text.clone(),
                            size: d.font_size,
                            color: rgb(d.rgb()).into(),
                            width: d.width,
                            scroll_x: None,
                            y: 0.0,
                            fixed: Some((false, slot)),
                        })
                    }
                }
            })
            // 不要在这里按条数截断。
            //
            // 旧代码是 `.take(80)`，而 `self.danmaku` 按时间升序排列 —— 截掉的
            // 永远是**最新**的 80 条之后的那批，也就是刚要从右边缘进场的那批。
            // 密弹幕（同一秒几十条短弹幕）时同屏可见能到 100–300 条，被截的
            // 弹幕要等老弹幕离场才进绘制集合，那时它已经飞到画面中间了 ——
            // 表现就是「弹幕走到中间才冒出来」。
            //
            // 同屏条数本身由轨道分配约束（`MAX_SCROLL_LANES` × 每条轨道能并存的条数，
            // 上限约 600 条极端情况），实测 100 条/秒的单字弹幕一帧构建也只要 ~3ms，
            // 不需要再按条数封顶。
            .collect();

        // canvas：直接走底层 paint，不经过 div + text 的常规元素路径。
        // 常规路径下文字会破坏 surface 的合成（实测画面全黑），
        // 这条是文档第 14 节指定的方案 ②。
        canvas(
            |_bounds, _window, _cx| (),
            move |bounds, _state, window, cx| {
                let area_width = bounds.size.width.as_f32();
                let area_height = bounds.size.height.as_f32();
                // `ShapedLine::paint` 的 origin 是**窗口坐标**，不是画布内坐标。
                // 弹幕层只盖播放层（左上 = 窗口左 16 / 顶栏下 16），不补
                // `bounds.origin` 整层会整体上移一条顶栏、左移 16px，
                // 看起来就是「弹幕浮在画面上半屏」。
                let origin = bounds.origin;
                let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
                for d in &drawn {
                    let (x, y) = match d.fixed {
                        Some((top, slot)) => {
                            let x = ((area_width - d.width) * 0.5).max(8.0);
                            let y = if top {
                                8.0 + slot as f32 * SCROLL_LANE_HEIGHT
                            } else {
                                (area_height - 40.0 - slot as f32 * SCROLL_LANE_HEIGHT).max(8.0)
                            };
                            (x, y)
                        }
                        None => (d.scroll_x.unwrap_or(0.0), d.y),
                    };
                    // 设计稿要求「1px 深色描边 + 投影」，而 ShapedLine 的
                    // 颜色烘在 layout 里、paint 不能重新着色。这里把同一条文本
                    // shape 两次：一次深色（描边/投影），一次原色（字身）。
                    // 描边色必须是深色（#0B0F14），白字 + 白描边在白画面上会糊。
                    let stroke = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: rgba(0x0B0F14D9).into(),
                        ..Default::default()
                    };
                    let stroke_line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[stroke],
                        None,
                    );
                    let line_height = px(d.size * 1.25);

                    // 投影：右下 1,2 px 先画一层，再画 8 方向 1px 描边。
                    let _ = stroke_line.paint(
                        at(x + 1.0, y + 2.0),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                    for (dx, dy) in [
                        (-1.0, 0.0),
                        (1.0, 0.0),
                        (0.0, -1.0),
                        (0.0, 1.0),
                        (-1.0, -1.0),
                        (1.0, -1.0),
                        (-1.0, 1.0),
                        (1.0, 1.0),
                    ] {
                        let _ = stroke_line.paint(
                            at(x + dx, y + dy),
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }

                    // 字身
                    let run = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: d.color,
                        ..Default::default()
                    };
                    let line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[run],
                        None,
                    );
                    let _ = line.paint(at(x, y), line_height, TextAlign::Left, None, window, cx);
                }
            },
        )
        .absolute()
        // 只覆盖播放层：右侧信息面板展开 400 / 收起 200，都不属于弹幕区。
        .top_0()
        .left_0()
        .bottom_0()
        .right(px(self.panel_width()))
    }

    /// 播放 / 暂停：控制栏与画面中央按钮共用。
    fn toggle_playback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 点播放 = 放弃输入焦点，恢复渲染循环。
        self.input_focused = false;
        self.state.toggle_play();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.set_paused(!self.state.playing);
        }
        if self.state.playing {
            // 起播即藏：黑框不挡画面；鼠标一动会重新浮出。
            window.focus(&self.focus_handle, cx);
            self.hide_controls(cx);
        } else {
            // 暂停态把控制栏叫回来，方便继续操作。
            self.note_activity(window, cx);
        }
        window.refresh();
        cx.notify();
    }

    /// 暂停态的中央播放按钮（设计稿的大圆按钮；播放中不挂树）。
    fn center_play(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        if self.source.is_none() || self.state.playing {
            return div().into_any_element();
        }
        div()
            .absolute()
            .top_0()
            .left_0()
            .bottom_0()
            .right(px(INFO_PANEL_COLLAPSED))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("player-center-play")
                    .w(px(84.))
                    .h(px(84.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgba(0x0000006B))
                    .border_1()
                    .border_color(rgba(0xFFFFFF4D))
                    .cursor_pointer()
                    .hover(|style| style.bg(t.palette.theme_primary_60))
                    .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                        view.toggle_playback(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::Play)
                            .size(px(34.))
                            .text_color(rgb(0xFFFFFF)),
                    ),
            )
            .into_any_element()
    }

    /// 控制栏隐藏时的简略态：底部 3px 进度线（40px 热区负责把控制栏叫回来）。
    fn mini_progress(&self, t: &YkTheme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.state.progress().clamp(0.0, 1.0) as f32;
        let (left, width) = self.video_bar_geometry(window);
        // 40px hover 热区仍铺满播放层，视觉上的 3px 进度线与视频同宽。
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(40.))
            .flex()
            .items_end()
            .bg(rgba(0x00000001))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                view.note_pointer_move(event.position, window, cx);
            }))
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .bottom_0()
                    .w(px(width))
                    .h(px(3.))
                    .bg(rgba(0xFFFFFF2E))
                    .child(
                        div()
                            .h_full()
                            .w(relative(progress))
                            .bg(t.palette.theme_primary),
                    ),
            )
            .into_any_element()
    }

    /// 画面左下角：在线人数 + 弹幕开关 + 弹幕设置。
    fn watch_badge(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.source.is_none() {
            return div().into_any_element();
        }
        let color = hsla(0.0, 0.0, 1.0, 0.88);
        let eye = if self.danmaku_visible {
            IconName::Eye
        } else {
            IconName::EyeOff
        };
        div()
            .absolute()
            .left(px(SPACE_4))
            .bottom(px(CTRL_H + 10.))
            .child(
                div()
                    .px(px(SPACE_2))
                    .py(px(3.))
                    .rounded(px(RADIUS_HALF))
                    .bg(rgba(0x00000073))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(color)
                    .child(div().child("1人正在看"))
                    .child(overlay_icon(
                        "player-danmaku-toggle",
                        eye,
                        color,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.danmaku_visible = !view.danmaku_visible;
                            cx.notify();
                        }),
                    ))
                    .child(overlay_icon(
                        "player-danmaku-settings",
                        IconName::Captions,
                        color,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            // 弹幕设置入口：展开面板并切到「弹幕」Tab。
                            view.tab = 1;
                            view.panel_collapse_generation =
                                view.panel_collapse_generation.wrapping_add(1);
                            if !view.expanded {
                                view.expanded = true;
                                view.panel_generation = view.panel_generation.wrapping_add(1);
                            }
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    /// 骑在面板左边界上的圆形按钮：展开态 ×、收起态 ‹。
    fn panel_toggle(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        // 骑在贴右边界的面板左边界上：right = 面板宽度 - 一半按钮。
        let to_right = self.panel_width() - 16.;
        let from_right = if self.panel_generation == 0 {
            to_right
        } else if self.expanded {
            INFO_PANEL_COLLAPSED - 16.
        } else {
            INFO_PANEL_EXPANDED - 16.
        };
        div()
            .id("player-panel-toggle")
            .absolute()
            .top(px(SPACE_2))
            .right(px(to_right))
            .w(px(32.))
            .h(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(rgba(0x14161AB8))
            .border_1()
            .border_color(t.surfaces.border_popover)
            .cursor_pointer()
            .hover(|style| style.bg(palette().theme_primary))
            // 按钮不再挂 on_hover：展开 / 收起由真实指针坐标驱动，
            // 这里只保留点击切换，避免播放中 hover 重算把面板反复收起。
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.expanded = !view.expanded;
                view.panel_collapse_generation = view.panel_collapse_generation.wrapping_add(1);
                view.panel_generation = view.panel_generation.wrapping_add(1);
                view.panel_hover_grace_until = if view.expanded {
                    None
                } else {
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(400))
                };
                cx.notify();
            }))
            .child(
                Icon::new(if self.expanded {
                    IconName::X
                } else {
                    IconName::ChevronLeft
                })
                .size(px(ICON_SIZE_SM))
                .text_color(rgb(0xFFFFFF)),
            )
            .with_animation(
                format!("player-panel-btn-{}", self.panel_generation),
                Animation::new(std::time::Duration::from_millis(200)).with_easing(ease_standard()),
                move |el, delta| el.right(px(from_right + (to_right - from_right) * delta)),
            )
            .into_any_element()
    }

    /// 面板 Tab：评论 / 弹幕 / 选集。「选集」只在多 P 时出现。
    fn panel_tabs(&self) -> Vec<&'static str> {
        let multi = self
            .source
            .as_ref()
            .map(|source| source.pages.len() > 1)
            .unwrap_or(false);
        if multi {
            vec!["评论", "弹幕", "选集"]
        } else {
            vec!["评论", "弹幕"]
        }
    }

    /// 信息面板：标题 → 元信息 → UP 主 → 互动 → 标签 → Tab → 内容。
    ///
    /// 内容按 400px 排版，收起态只把右半裁掉（D-05：收起仍是 200px 半宽）。
    fn info_panel(&mut self, t: &YkTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 头像探针的句柄先取出来，避免在元素链里再借 self
        let avatar_bounds = self.up_avatar_bounds.clone();
        let scale_factor = window.scale_factor();
        let s = &t.surfaces;
        let p = &t.palette;
        let source = self.source.as_ref();

        let title = match source {
            Some(source) if !source.title.is_empty() => source.title.clone(),
            _ if self.loading_source => "正在获取播放地址…".to_string(),
            _ if self.source_error.is_some() => "播放失败".to_string(),
            _ => "视频信息".to_string(),
        };

        let head = div()
            .flex()
            .items_start()
            .pt(px(SPACE_3))
            .pb(px(SPACE_2))
            .pl(px(28.))
            .pr(px(SPACE_3))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .line_height(px(LINE_HEIGHT_TITLE))
                    .text_size(px(FONT_SIZE_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .line_clamp(2)
                    .child(title),
            );

        let meta = source.map(|source| {
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_4))
                .px(px(SPACE_4))
                .pb(px(SPACE_3))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child(meta_stat(IconName::Play, format::count(source.view_count)))
                .child(meta_stat(
                    IconName::MessageCircle,
                    format::count(source.danmaku_count),
                ))
                .when(source.pubdate > 0, |el| {
                    el.child(div().child(format::date_time(source.pubdate)))
                })
        });

        let up = source.map(|source| {
            let face_url = if source.owner_face.is_empty() {
                String::new()
            } else {
                asset_url(&source.owner_face, fitted_width(46.0, scale_factor, 0))
            };
            let name = if source.owner_name.is_empty() {
                "未知 UP 主".to_string()
            } else {
                source.owner_name.clone()
            };
            div()
                .px(px(SPACE_4))
                .pb(px(SPACE_3))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(SPACE_3))
                        .child(
                            // 点头像 = 打开 UP 主浮窗（12.9 节）。头像本身是视觉焦点，
                            // 顺手把「这人是谁」做成一跳可达，不打断正在播的视频。
                            div()
                                .id("player-up-avatar")
                                .relative()
                                // 探针：记下头像的屏幕 bounds，浮窗从它长出来
                                .child(
                                    canvas(
                                        {
                                            let sink = avatar_bounds.clone();
                                            move |bounds, _window, _cx| sink.set(Some(bounds))
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                                .w(px(46.))
                                .h(px(46.))
                                .flex_shrink_0()
                                .rounded_full()
                                .overflow_hidden()
                                .cursor_pointer()
                                .hover(|style| style.opacity(0.88))
                                .bg(linear_gradient(
                                    135.,
                                    linear_color_stop(p.bili_blue, 0.0),
                                    linear_color_stop(p.bili_pink, 1.0),
                                ))
                                .when(!face_url.is_empty(), |el| {
                                    el.child(
                                        img(ImageSource::from(face_url.clone()))
                                            .absolute()
                                            .inset_0()
                                            .w_full()
                                            .h_full()
                                            .object_fit(ObjectFit::Cover),
                                    )
                                })
                                .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    let Some(source) = view.source.as_ref() else {
                                        return;
                                    };
                                    let mid = source.owner_mid;
                                    if mid <= 0 {
                                        view.show_interaction_note(
                                            "没拿到 UP 主 uid，无法打开空间".into(),
                                            true,
                                            cx,
                                        );
                                        return;
                                    }
                                    let name = source.owner_name.clone();
                                    let face = source.owner_face.clone();
                                    let opener = view.up_avatar_bounds.get().unwrap_or_else(|| {
                                        Bounds {
                                            origin: point(px(0.0), px(TOP_BAR_HEIGHT)),
                                            size: size(px(46.0), px(46.0)),
                                        }
                                    });
                                    if let Some(handler) = view.up_profile_handler.as_ref() {
                                        handler(cx, window, mid, name, face, opener);
                                    }
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(SPACE_2))
                                        .child(
                                            div()
                                                .min_w(px(0.))
                                                .truncate()
                                                .text_size(px(FONT_SIZE_CONTROL))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(s.text_primary)
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .px(px(6.))
                                                .h(px(18.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(BADGE_RADIUS))
                                                .border_1()
                                                .border_color(s.border_strong)
                                                .text_size(px(FONT_SIZE_MICRO))
                                                .text_color(s.text_quaternary)
                                                .child("发消息"),
                                        ),
                                )
                                // UP 主自己的签名放在昵称正下方，和视频简介分开。
                                .when(!source.owner_sign.trim().is_empty(), |el| {
                                    el.child(
                                        div()
                                            .mt(px(2.))
                                            .line_height(px(16.))
                                            .text_size(px(FONT_SIZE_MICRO))
                                            .text_color(s.text_quaternary)
                                            .line_clamp(2)
                                            .child(source.owner_sign.clone()),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_2))
                        .mt(px(SPACE_3))
                        .child(
                            div()
                                .id("player-charge")
                                .flex_shrink_0()
                                .h(px(30.))
                                .px(px(SPACE_4))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_MD))
                                .bg(p.theme_primary)
                                .cursor_pointer()
                                .hover(|style| style.opacity(0.9))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .child("充电"),
                        )
                        .child(
                            div()
                                .id("player-follow")
                                .flex_1()
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(5.))
                                .rounded(px(RADIUS_MD))
                                .bg(s.fill_2)
                                .when(self.follow_busy, |el| el.opacity(0.55))
                                .cursor_pointer()
                                .hover(|style| style.bg(s.fill_3))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(if self.followed {
                                    s.text_tertiary
                                } else {
                                    s.text_secondary
                                })
                                .child(
                                    Icon::new(if self.followed {
                                        IconName::UserCheck
                                    } else {
                                        IconName::UserPlus
                                    })
                                    .size(px(ICON_SIZE_SM))
                                    .text_color(if self.followed {
                                        s.text_tertiary
                                    } else {
                                        s.text_secondary
                                    }),
                                )
                                .child(if self.follow_busy {
                                    "处理中…"
                                } else if self.followed {
                                    "已关注"
                                } else {
                                    "关注"
                                })
                                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                                    view.toggle_follow(cx);
                                })),
                        )
                        .when_some(self.owner_fans, |el, fans| {
                            // 粉丝数来自 `/x/relation/stat`：紧跟关注按钮，
                            // 关注 / 取关成功后跟着 ±1。
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(FONT_SIZE_MICRO))
                                    .text_color(s.text_quaternary)
                                    .child(format!("{} 粉丝", format::count(fans))),
                            )
                        }),
                )
        });

        // 视频简介单独一行，放在 UP 信息块之后：不塞进 UP 昵称下面的窄列，
        // 也不会把「发消息 / 关注」这类 UP 操作挤成两行。
        let desc_row = source.and_then(|source| {
            let desc = source.desc.trim().to_string();
            if desc.is_empty() {
                return None;
            }
            let weak = cx.entity().downgrade();
            let copy_notice = Rc::new(move |cx: &mut App| {
                let _ = weak.update(cx, |view, cx| {
                    view.show_interaction_note("已复制".into(), false, cx);
                });
            });
            Some(
                div().w_full().px(px(SPACE_4)).pb(px(SPACE_3)).child(
                    div()
                        .id("player-video-desc")
                        .w(px(PLAYER_INFO_PANEL_WIDTH - SPACE_4 * 2.0))
                        .child(
                            gpui::base::SelectableText::new("player-video-desc-text", desc.clone())
                                .text_style(gpui::TextStyleRefinement {
                                    font_size: Some(px(FONT_SIZE_CAPTION).into()),
                                    line_height: Some(px(18.).into()),
                                    color: Some(s.text_tertiary),
                                    line_clamp: Some(3),
                                    ..Default::default()
                                }),
                        )
                        .context_menu(text_copy_menu(desc, copy_notice)),
                ),
            )
        });

        // 互动行：点赞 / 投币 / 收藏 / 分享。
        // 计数为 0 时显示动作名（B 站的做法），有计数时只显示数字。
        let like_count = source.map(|source| source.like_count).unwrap_or(0);
        let coin_count = source.map(|source| source.coin_count).unwrap_or(0);
        let fav_count = source.map(|source| source.favorite_count).unwrap_or(0);
        let share_count = source.map(|source| source.share_count).unwrap_or(0);
        let like_label = if like_count > 0 {
            format::count(like_count)
        } else {
            "点赞".to_string()
        };
        let coin_label = if coin_count > 0 {
            format::count(coin_count)
        } else {
            "投币".to_string()
        };
        let fav_label = if fav_count > 0 {
            format::count(fav_count)
        } else {
            "收藏".to_string()
        };
        let share_label = if share_count > 0 {
            format::count(share_count)
        } else {
            "分享".to_string()
        };
        let action_bounds = self.action_row_bounds.clone();
        let actions = div()
            // 顶边线按 400 宽铺满（面板内容就是 400 宽排版），按钮区另外
            // 固定 200 —— 见下面的说明。
            .w(px(PLAYER_INFO_PANEL_WIDTH))
            .relative()
            .border_t_1()
            .border_color(s.border_subtle)
            .child(
                div()
                    // 按钮区固定 200 宽：面板内容按 400 宽排版，收起态右半
                    // 会被裁掉；按钮区若跟着 400 排，收藏 / 分享两个按钮就整排
                    // 落在窗口外，点了没反应。
                    // 宽度也**不能**跟着展开态变：侧栏一 hover 展开，四个按钮
                    // 就会重新均分、整体挪位，用户瞄准的按钮会从指针下滑走，
                    // 一点就点空（甚至点到隔壁按钮）。
                    .w(px(INFO_PANEL_COLLAPSED))
                    .flex()
                    .items_center()
                    .justify_around()
                    .px(px(SPACE_2))
                    .py(px(2.))
                    .child(panel_action_button(
                        t,
                        "player-action-like",
                        IconName::ThumbsUp,
                        like_label,
                        self.interaction.liked,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_like(cx);
                        }),
                    ))
                    .child(panel_action_button(
                        t,
                        "player-action-coin",
                        IconName::Coins,
                        coin_label,
                        self.interaction.coined,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_coin_panel(cx);
                        }),
                    ))
                    .child(panel_action_button(
                        t,
                        "player-action-fav",
                        IconName::Star,
                        fav_label,
                        self.interaction.favoured,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_fav_panel(cx);
                        }),
                    ))
                    .child(panel_action_button(
                        t,
                        "player-action-share",
                        IconName::Share,
                        share_label,
                        self.share_panel_open,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_share_panel(cx);
                        }),
                    )),
            )
            // 互动行的实际几何由这层透明 canvas 在 prepaint 时写回。
            // 弹层按它挂到窗口上，所以展开面板不会再把下面的内容顶下去。
            .child(
                canvas(
                    move |bounds, _, _| action_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );

        let mut tags: Vec<String> = if !self.tags.is_empty() {
            self.tags.clone()
        } else {
            source
                .map(|source| {
                    if source.category.is_empty() {
                        Vec::new()
                    } else {
                        vec![source.category.clone()]
                    }
                })
                .unwrap_or_default()
        };
        tags.truncate(3);
        let tags_row = (!tags.is_empty()).then(|| {
            let mut row = div()
                .flex()
                .flex_wrap()
                .gap(px(SPACE_2))
                .px(px(SPACE_4))
                .py(px(SPACE_2))
                .border_t_1()
                .border_color(s.border_subtle);
            for tag in tags {
                row = row.child(
                    div()
                        .px(px(10.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(s.fill_2)
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(tag),
                );
            }
            row
        });

        let tabs = self.panel_tabs();
        let active = self.tab.min(tabs.len().saturating_sub(1));
        let mut tabs_row = div()
            .flex()
            .border_t_1()
            .border_color(s.border_subtle)
            .flex_shrink_0();
        for (index, name) in tabs.iter().enumerate() {
            let on = index == active;
            tabs_row = tabs_row.child(
                div()
                    .id(("player-tab", index))
                    .flex_1()
                    .h(px(42.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(if on {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if on { p.theme_primary } else { s.text_tertiary })
                    .child(*name)
                    .child(div().w(px(28.)).h(px(2.)).rounded_full().bg(if on {
                        p.theme_primary
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    }))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                        view.tab = index;
                        cx.notify();
                    })),
            );
        }

        // 整个面板只有一个滚动区：图 1 那一段（标题 / UP / 互动 / 标签 / Tab）
        // 不再固定，和下面的评论一起滚。内层单独包一列，避免 flex 子项被压缩后
        // 高度不足、overflow_y_scroll 反而滚不动。
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .child(head)
            .children(meta)
            .children(up)
            .children(desc_row)
            .child(actions)
            .children(tags_row)
            .child(tabs_row)
            .child(self.tab_content(t, active, window, cx));

        let body = div()
            .id("player-info-scroll")
            .w(px(PLAYER_INFO_PANEL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.panel_scroll)
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, window, cx| {
                // 滚动事件自带窗口内坐标，用它把「指针还在面板里」钉住：
                // 播放中每帧重绘会让 hover 偶发 false，滚动时不能被它误收起。
                view.panel_last_scroll = Some(std::time::Instant::now());
                view.last_pointer = event.position;
                view.panel_collapse_generation = view.panel_collapse_generation.wrapping_add(1);
                view.panel_hover_grace_until = None;
                if !view.expanded {
                    view.expanded = true;
                    view.panel_generation = view.panel_generation.wrapping_add(1);
                    cx.notify();
                }
                // 滚到底部附近就补下一批评论（每批 1 页 20 条）。
                view.comment_section
                    .maybe_load_more(&view.panel_scroll, cx);
                // 评论是窗口化渲染：可见区随滚动变化，每滚一下都要重算窗口，
                // 否则滚出已创建的 ~27 行之后会看到空白。
                if view.tab == 0 {
                    cx.notify();
                }
                view.note_activity(window, cx);
            }))
            .child(content);

        // 「回到顶部」浮出判定与输入条让位都在共享组件里，这里只喂滚动位置。
        self.comment_section.sync_scroll(&self.panel_scroll);
        if !self.expanded {
            self.comment_section.back_to_top = false;
        }
        // 评论输入条只活在「展开 + 评论 Tab」这一帧。
        let composer_visible = self.tab == 0 && self.expanded;

        let to_width = self.panel_width();
        let from_width = if self.panel_generation == 0 {
            to_width
        } else if self.expanded {
            INFO_PANEL_COLLAPSED
        } else {
            INFO_PANEL_EXPANDED
        };
        div()
            .id("info-panel")
            .absolute()
            .top(px(0.))
            // 侧栏贴着窗口右边界；视频区右缘不铺 hover 热区，避免误触发展开。
            .right(px(0.))
            // 高度通高：从播放页层顶部一直拉到底。
            .bottom(px(0.))
            .w(px(to_width))
            .overflow_hidden()
            // 浮在视频上的面板：GPUI 没有元素级 backdrop blur，用不透明表面。
            .bg(s.surface_elevated_solid)
            .when(self.expanded, |el| {
                el.border_l_1().border_color(s.border_subtle)
            })
            // hover 只负责「进入即展开」的兜底；收起一律由真实指针坐标决定。
            // 播放中每帧重绘都会重算 hover，不能让它驱动收起。
            .on_hover(cx.listener(|view, hovered: &bool, window, cx| {
                // 用当前光标位置判断：`window.mouse_position()` 停在最后一次
                // 事件上，面板尺寸动画把它顶到光标底下时会拿旧位置判定，
                // 于是面板在「指针早已移开」的情况下自己弹回来。
                if *hovered && view.pointer_in_panel_zone(cursor_position(window, cx), window) {
                    view.set_panel_hover(true, window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                view.note_pointer_move(event.position, window, cx);
            }))
            .child(body)
            // 互动弹层：deferred 画在所有面板内容之后（GPUI 没有 z-index）。
            .children(self.action_overlay(t, window, cx))
            // 滚动后出现：浮在内容之上，右下角 16px 内边距。
            // 收起时也占住这个子节点位置（渲染成 0 尺寸），别让兄弟节点
            // 数量随滚动变化 —— 子树被重建会让里面的动画从头再播一遍。
            .child(
                self.comment_section
                    .back_to_top(t, &self.panel_scroll, cx),
            )
            // 评论 Tab 的浮动输入条：与动态详情同一份组件。
            .children(composer_visible.then(|| {
                let width = self.panel_width();
                self.comment_section.composer(t, window, cx, width)
            }))
            .with_animation(
                format!("player-panel-{}", self.panel_generation),
                Animation::new(std::time::Duration::from_millis(200)).with_easing(ease_standard()),
                move |el, delta| {
                    el.w(px(from_width + (to_width - from_width) * delta))
                },
            )
            .into_any_element()
    }

    /// 投币 / 分享浮层的宽度：固定 200（与互动按钮区同宽）。
    ///
    /// 不能跟着侧栏展开态变 —— 宽度一变，里面的按钮就会挪位，
    /// 用户刚展开就点会点空。
    fn action_panel_width(&self) -> f32 {
        INFO_PANEL_COLLAPSED
    }

    /// 三个互动弹层（投币 / 收藏夹 / 分享）的浮层容器。
    ///
    /// 走 `deferred + anchored` 而不是插进文档流：
    /// - **不占布局**：以前展开面板会把标签、评论区整段往下顶，收起时又跳回来；
    /// - **画在最上层**：GPUI 没有 z-index，`deferred` 让它在所有兄弟节点之后绘制；
    /// - **位置跟着互动行走**：bounds 由互动行 prepaint 时记录，滚动时下一帧自动跟上。
    fn action_overlay(
        &self,
        t: &YkTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // 开着的那个优先；退场动画期间按缓存的 kind 继续画同一张卡。
        let kind = if self.fav_panel_open {
            ActionOverlay::Fav
        } else if self.coin_panel_open {
            ActionOverlay::Coin
        } else if self.share_panel_open {
            ActionOverlay::Share
        } else {
            self.overlay_closing_kind?
        };
        let closing = self.overlay_closing_kind.is_some();
        // 首帧还没量到互动行：这一帧先不画，下一帧就有位置了。
        let bounds = self.action_row_bounds.get()?;
        // 互动行滚出面板可视区后不再画：浮层是按窗口坐标挂的，
        // 不拦一下会跟着滚到视频画面上，看起来像凭空飘着一张卡。
        let viewport = window.viewport_size();
        let panel_top = px(TOP_BAR_HEIGHT + SPACE_4);
        let panel_bottom = viewport.height - px(PLAYER_BOTTOM_INSET);
        if bounds.origin.y + bounds.size.height < panel_top || bounds.origin.y > panel_bottom {
            return None;
        }
        let panel = match kind {
            ActionOverlay::Fav => self.fav_panel(t, cx),
            ActionOverlay::Coin => self.coin_panel(t, cx),
            ActionOverlay::Share => self.share_panel(t, cx),
        }?;

        // 入场：淡入 + 从上方 6px 落下来；退场反向。GPUI 没有 transform，
        // 位移用负 margin 做，卡片本身不带 margin，anchor 的定位不受影响。
        let animation = if closing {
            Animation::new(DURATION_FAST).with_easing(ease_standard())
        } else {
            Animation::new(DURATION_FAST).with_easing(ease_entrance())
        };
        let animated = div().child(panel).with_animation(
            format!(
                "player-overlay-{}-{}",
                if closing { "out" } else { "in" },
                self.overlay_generation
            ),
            animation,
            move |el, delta| {
                let progress = if closing { 1.0 - delta } else { delta };
                let progress = progress.clamp(0.0, 1.0);
                el.opacity(progress).mt(px(-6.0 * (1.0 - progress)))
            },
        );

        Some(
            deferred(
                // 整窗的点击层垫在浮层下面：点任何别的地方都先把浮层收起来。
                // 浮层卡片自己带 occlude，点卡片上的按钮不会被这层吃掉。
                anchored()
                    .position(point(px(0.), px(0.)))
                    .child(
                        div()
                            .id("player-overlay-catcher")
                            .w(viewport.width)
                            .h(viewport.height)
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                                    // 外点只负责关浮层，这一次点击不再落到下面的按钮上。
                                    view.close_action_overlays(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                anchored()
                                    // 与面板文字列对齐（16）；下方放不下时自动翻到上方。
                                    .position(point(
                                        bounds.origin.x + px(SPACE_4),
                                        bounds.origin.y + bounds.size.height + px(SPACE_1),
                                    ))
                                    .snap_to_window_with_margin(px(SPACE_4))
                                    .child(animated),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }

    /// 投币 / 分享这两个小浮层的壳：圆角卡片 + 细边 + 阴影。
    ///
    /// 宽度跟互动按钮区一样是 200：这两个面板只有两三个控件，
    /// 再宽反而和上面的按钮对不齐（收藏夹是清单，另走 368 的卡片）。
    fn action_popover(
        t: &YkTheme,
        id: &'static str,
        width: f32,
        content: AnyElement,
    ) -> AnyElement {
        let s = &t.surfaces;
        div()
            .id(id)
            .w(px(width))
            .px(px(SPACE_3))
            .py(px(SPACE_2))
            .rounded(px(RADIUS_LG))
            // 和收藏夹卡片共用同一档弹层表面：三个面板是一个家族，
            // 不能一个像卡片、另外两个像侧栏里多出来的一行。
            .bg(s.surface_popover)
            .border_1()
            .border_color(s.border_popover)
            .shadow(t.shadows.shadow_glass_elevated.clone())
            // 浮层是实心的：点空白处也不该穿透到下面的标签 / 评论。
            .occlude()
            .child(content)
            .into_any_element()
    }

    /// 投币面板：1 枚 / 2 枚 + 「同时点赞」。
    fn coin_panel(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 退场动画期间 bool 已经清零，但这一帧还得把卡画出来。
        if !self.coin_panel_open && self.overlay_closing_kind != Some(ActionOverlay::Coin) {
            return None;
        }
        let s = &t.surfaces;
        let p = &t.palette;
        let busy = self.interaction_busy;
        let row = div()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .child(
                div()
                    .id("player-coin-1")
                    .flex_1()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_MD))
                    .bg(p.theme_primary)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child("投 1 枚")
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        let also_like = view.coin_also_like;
                        view.do_coin(1, also_like, cx);
                    })),
            )
            .child(
                div()
                    .id("player-coin-2")
                    .flex_1()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_MD))
                    .bg(s.fill_3)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child("投 2 枚")
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        let also_like = view.coin_also_like;
                        view.do_coin(2, also_like, cx);
                    })),
            );
        let also_like = div()
            .id("player-coin-also-like")
            .mt(px(SPACE_2))
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .cursor_pointer()
            .text_size(px(FONT_SIZE_MICRO))
            .text_color(s.text_secondary)
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded(px(RADIUS_HALF))
                    .border_1()
                    .border_color(if self.coin_also_like {
                        p.theme_primary
                    } else {
                        s.border_strong
                    })
                    .bg(if self.coin_also_like {
                        p.theme_primary
                    } else {
                        s.fill_3
                    }),
            )
            .child("同时点赞")
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.coin_also_like = !view.coin_also_like;
                cx.notify();
            }));
        Some(Self::action_popover(
            t,
            "player-coin-panel",
            self.action_panel_width(),
            div().child(row).child(also_like).into_any_element(),
        ))
    }

    /// 收藏夹面板：列出自己的收藏夹，点一下收藏 / 取消收藏。
    ///
    /// 按「多选清单」重做（设计 §13.3 弹出层 / §13.7 勾选）：
    /// - 卡片抬一层：`surface_popover` + `border_popover` + 悬浮阴影，
    ///   压在侧栏内容之上读，不再和普通列表行糊成一片；
    /// - 行高 32（与菜单项同规格），左侧勾选圈表示是否已收藏，数量右对齐。
    ///   原先把「标题 / 数量 / 收藏」三列硬塞进 200 宽，标题和动作词互相裁切；
    /// - 行内不再写「收藏 / 已收藏」：勾选圈 + 底色已经说明状态，
    ///   重复一遍动作词只会挤掉标题；
    /// - 超过一屏先折叠，别把标签 / 评论整个顶出视野；加载 / 空 / 失败三态齐备。
    fn fav_panel(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 退场动画期间 bool 已经清零，但这一帧还得把卡画出来。
        if !self.fav_panel_open && self.overlay_closing_kind != Some(ActionOverlay::Fav) {
            return None;
        }
        let s = &t.surfaces;
        let p = &t.palette;
        let busy = self.interaction_busy;
        let selected = self
            .fav_folders
            .iter()
            .filter(|folder| folder.contains_video())
            .count();

        let close = div()
            .id("player-fav-close")
            .w(px(22.))
            .h(px(22.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_HALF))
            .cursor_pointer()
            .hover(|style| style.bg(s.fill_3))
            .child(
                Icon::new(IconName::X)
                    .size(px(ICON_SIZE_SM - 2.))
                    .text_color(s.text_tertiary),
            )
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.fav_panel_open = false;
                cx.notify();
            }));

        let header = div()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .h(px(32.))
            .px(px(SPACE_2))
            .child(
                div()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child("收藏夹"),
            )
            .child(div().flex_1())
            .children((selected > 0).then(|| {
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(p.theme_primary)
                    .child(format!("已收藏 {selected} 个"))
            }))
            .child(close);

        let divider = div().h(px(1.)).w_full().bg(s.border_subtle);

        let body: AnyElement = if self.fav_loading && self.fav_folders.is_empty() {
            // 骨架行按真实行高占位：数据回来时面板不会「跳一下」。
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .py(px(SPACE_1))
                .children((0..3).map(|_| {
                    div()
                        .h(px(32.))
                        .w_full()
                        .rounded(px(RADIUS_MD))
                        .bg(s.skeleton_brush)
                        .opacity(0.6)
                        .into_any_element()
                }))
                .into_any_element()
        } else if let Some(error) = self
            .fav_error
            .as_ref()
            .filter(|_| self.fav_folders.is_empty())
        {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(SPACE_1))
                .py(px(SPACE_4))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_secondary)
                        .child("收藏夹没加载出来"),
                )
                .child(
                    div()
                        .max_w(px(280.))
                        .truncate()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(error.clone()),
                )
                .child(
                    div()
                        .id("player-fav-retry")
                        .mt(px(SPACE_1))
                        .px(px(SPACE_3))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_HALF))
                        .cursor_pointer()
                        .bg(p.theme_primary_10)
                        .hover(|style| style.bg(p.theme_primary_20))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(p.theme_primary)
                        .child("重试")
                        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.load_fav_folders(cx);
                        })),
                )
                .into_any_element()
        } else if self.fav_folders.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(SPACE_1))
                .py(px(SPACE_5))
                .child(
                    Icon::new(IconName::FolderOpen)
                        .size(px(ICON_SIZE_LG))
                        .text_color(s.text_quaternary),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_secondary)
                        .child("还没有收藏夹"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child("在网页版建好收藏夹后会同步到这里"),
                )
                .into_any_element()
        } else {
            let total = self.fav_folders.len();
            let shown = if self.fav_list_expanded {
                total
            } else {
                total.min(FAV_PANEL_VISIBLE_ROWS)
            };
            let mut list = div().flex().flex_col().gap(px(SPACE_1)).py(px(SPACE_1));
            for folder in self.fav_folders.iter().take(shown) {
                list = list.child(Self::fav_row(t, folder, busy, cx));
            }
            let mut stack = div().flex().flex_col().child(list);
            if total > FAV_PANEL_VISIBLE_ROWS {
                let expanded = self.fav_list_expanded;
                stack = stack.child(
                    div()
                        .id("player-fav-more")
                        .h(px(28.))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(SPACE_1))
                        .rounded(px(RADIUS_HALF))
                        .cursor_pointer()
                        .hover(|style| style.bg(s.fill_3))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(if expanded {
                            "收起".to_string()
                        } else {
                            format!("展开全部 {total} 个")
                        })
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(12.))
                            .text_color(s.text_quaternary),
                        )
                        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.fav_list_expanded = !view.fav_list_expanded;
                            cx.notify();
                        })),
                );
            }
            stack.into_any_element()
        };

        Some(
            div()
                .id("player-fav-panel")
                .w(px(PLAYER_INFO_PANEL_WIDTH - SPACE_4 * 2.0))
                .p(px(SPACE_1))
                .flex()
                .flex_col()
                .rounded(px(RADIUS_XL))
                .bg(s.surface_popover)
                .border_1()
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_glass_elevated.clone())
                .occlude()
                .child(header)
                .child(divider)
                .child(body)
                .into_any_element(),
        )
    }

    /// 收藏夹行：左侧勾选圈 + 标题 + 右侧数量，整行可点，点一下加 / 取消。
    ///
    /// 勾选圈用形状（实心对勾 / 空心圆）而不是只靠颜色区分选中态，
    /// 色觉障碍用户也能看出来哪几个夹子已经收藏。
    fn fav_row(
        t: &YkTheme,
        folder: &yk_core::FavFolder,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let selected = folder.contains_video();
        let folder_id = folder.id;
        let hover_bg = if selected {
            p.theme_primary_20
        } else {
            s.fill_3
        };
        div()
            .id(SharedString::from(format!("player-fav-folder-{folder_id}")))
            .h(px(32.))
            .w_full()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .px(px(SPACE_2))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .text_size(px(FONT_SIZE_CONTROL))
            .when(selected, |el| el.bg(p.theme_primary_10))
            .when(!busy, |el| el.hover(move |style| style.bg(hover_bg)))
            .when(busy, |el| el.opacity(0.55))
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .when(selected, |el| {
                        el.bg(p.theme_primary)
                            .border_1()
                            .border_color(p.theme_primary)
                    })
                    .when(!selected, |el| el.border_1().border_color(s.border_strong))
                    .children(selected.then(|| {
                        Icon::new(IconName::Check)
                            .size(px(10.))
                            .text_color(rgb(0xFFFFFF))
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(if selected {
                        s.text_primary
                    } else {
                        s.text_secondary
                    })
                    .child(folder.title.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_tertiary)
                    .child(fav_count_label(folder.media_count)),
            )
            .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                view.do_fav(folder_id, !selected, cx);
            }))
            .into_any_element()
    }

    /// 分享面板：哔哩哔哩动态 / QQ / 复制链接。
    fn share_panel(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 退场动画期间 bool 已经清零，但这一帧还得把卡画出来。
        if !self.share_panel_open && self.overlay_closing_kind != Some(ActionOverlay::Share) {
            return None;
        }
        let s = &t.surfaces;
        let title = self
            .source
            .as_ref()
            .map(|source| source.title.clone())
            .unwrap_or_default();
        let list = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_1))
            .child(
                div()
                    .id("player-share-dynamic")
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .px(px(SPACE_2))
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_primary)
                    .child(
                        Icon::new(IconName::Share)
                            .size(px(ICON_SIZE_SM))
                            .text_color(s.text_secondary),
                    )
                    .child(div().min_w(px(0.)).truncate().child("分享到哔哩哔哩动态"))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.share_to_dynamic(cx);
                    })),
            )
            .child(
                div()
                    .id("player-share-qq")
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .px(px(SPACE_2))
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_primary)
                    .child(
                        div()
                            .w(px(16.))
                            .h(px(16.))
                            .rounded(px(RADIUS_HALF))
                            .bg(rgb(0x12B7F5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child("Q"),
                    )
                    .child(div().min_w(px(0.)).truncate().child("分享到 QQ"))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.share_to_qq(cx);
                    })),
            )
            .child(
                div()
                    .id("player-share-copy")
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .px(px(SPACE_2))
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_primary)
                    .child(
                        Icon::new(IconName::Link)
                            .size(px(ICON_SIZE_SM))
                            .text_color(s.text_secondary),
                    )
                    .child(div().min_w(px(0.)).truncate().child("复制链接"))
                    // 回车键位提示：分享面板开着时回车 = 复制链接。
                    .child(div().flex_1().min_w(px(0.)))
                    .child(enter_key_hint())
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        view.copy_video_link(cx);
                    })),
            );
        let head = if title.is_empty() {
            div().into_any_element()
        } else {
            div()
                .pb(px(SPACE_1))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_quaternary)
                .line_clamp(2)
                .child(title)
                .into_any_element()
        };
        Some(Self::action_popover(
            t,
            "player-share-panel",
            self.action_panel_width(),
            div().child(head).child(list).into_any_element(),
        ))
    }

    /// 分享面板开着时按回车 = 复制链接。
    ///
    /// 面板上「复制链接」行尾挂着一个回车键位提示，这个键位得是真的能按 ——
    /// 键盘可达是设计基线里的硬性底线（见 ux-design-preview 第 13.13 节）。
    fn handle_enter(&mut self, cx: &mut Context<Self>) -> bool {
        // 分享面板没开时不抢回车：输入框里的回车（发弹幕）不能被这里截走。
        if !self.share_panel_open || self.input_focused {
            return false;
        }
        self.copy_video_link(cx);
        true
    }

    /// 互动反馈 toast：窗口底部居中的小胶囊，成功 / 失败共用。
    ///
    /// 浮在播放层与右侧面板之上，不参与面板排版，所以复制 / 点赞后
    /// 按钮不会因为提示出现而被顶开。位置抬到控制栏之上，
    /// 控制栏显示或隐藏时都不会压住它。
    ///
    /// 缩放用「字号 / 内边距 / 高度同比插值」实现：这一版 GPUI 没有
    /// 元素级 transform，等比改这几个尺寸在视觉上就是整体缩放。
    fn interaction_toast(&self, t: &YkTheme) -> Option<AnyElement> {
        let (text, is_error) = self.interaction_note.clone()?;
        let s = &t.surfaces;
        let closing = self.interaction_note_closing;
        let scale_from = TOAST_SCALE_FROM;
        // 入场 / 退场各用一个独立的动画 id：切到退场时动画从头开始，
        // 但首帧 scale 正好是 1，和入场结束态无缝衔接。
        let (animation_id, animation) = if closing {
            (
                format!("player-toast-out-{}", self.interaction_note_generation),
                Animation::new(DURATION_FAST).with_easing(ease_standard()),
            )
        } else {
            (
                format!("player-toast-in-{}", self.interaction_note_generation),
                Animation::new(DURATION_FAST).with_easing(ease_entrance()),
            )
        };
        let pill = status::toast_pill(t, text)
            .text_color(if is_error {
                t.palette.status_error
            } else {
                s.text_secondary
            })
            .with_animation(animation_id, animation, move |el, delta| {
                let factor = if closing {
                    1.0 - (1.0 - scale_from) * delta
                } else {
                    scale_from + (1.0 - scale_from) * delta
                };
                let opacity = if closing { 1.0 - delta } else { delta };
                el.max_w(px(status::TOAST_MAX_WIDTH * factor))
                    .min_h(px(status::TOAST_HEIGHT * factor))
                    .px(px(status::TOAST_PADDING_X * factor))
                    .py(px(SPACE_2 * factor))
                    .text_size(px(FONT_SIZE_BODY * factor))
                    .opacity(opacity.clamp(0.0, 1.0))
            });
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                // 贴近控制栏上沿：比旧版再往下 12px，但仍留 4px 间隙，
                // 不会盖住弹幕输入 / 发送按钮。
                .bottom(px(CTRL_H + SPACE_1))
                .flex()
                .justify_center()
                .child(pill)
                .into_any_element(),
        )
    }

    /// 面板 Tab 内容：评论 / 弹幕 / 选集。
    fn tab_content(
        &mut self,
        t: &YkTheme,
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match active {
            0 => self.comments_tab(t, window, cx),
            1 => self.danmaku_tab(t),
            _ => self.pages_tab(t),
        }
    }

    /// 评论 Tab。
    ///
    /// **窗口化渲染**：评论行按「行高前缀和」绝对定位，只创建当前可见区
    /// （上下各多 `COMMENT_OVERSCAN`）内的行。翻到 1000 条也只存在 ~30 行元素，
    /// 不会因为条目过多在滚动时掉帧。
    fn comments_tab(
        &mut self,
        t: &YkTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let total = if self.comment_section.list.total > 0 {
            self.comment_section.list.total
        } else {
            self.source
                .as_ref()
                .map(|source| source.reply_count)
                .unwrap_or(0)
        };
        let retry = self
            .source
            .as_ref()
            .map(|source| (source.aid, source.bvid.clone()));

        // 底部留出浮动评论输入框的高度，最后一条评论能滚到输入框上方。
        // 底部同时给「回到顶部」按钮和浮动输入框留出空间。
        let mut list = div().w_full().px(px(SPACE_4)).pb(px(116.));
        match &self.comments {
            CommentState::Idle => {}
            CommentState::Loading => {
                for _ in 0..3 {
                    list = list.child(comment_skeleton(t));
                }
            }
            CommentState::Error(error) => {
                list = list.child(panel_empty_line(t, "评论加载失败")).child(
                    div()
                        .w_full()
                        .pb(px(SPACE_4))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .line_clamp(2)
                                .child(error.clone()),
                        ),
                );
                if let Some((aid, bvid)) = retry {
                    list = list.child(
                        div().w_full().flex().justify_center().child(
                            div()
                                .id("player-comments-retry")
                                .px(px(SPACE_3))
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_MD))
                                .bg(s.fill_2)
                                .cursor_pointer()
                                .hover(|style| style.bg(s.fill_3))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(p.theme_primary)
                                .child("重试")
                                .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                                    view.spawn_video_meta(aid, bvid.clone(), cx);
                                })),
                        ),
                    );
                }
            }
            CommentState::Ready => {
                if self.comment_section.list.items.is_empty() {
                    // 首批都没有：只有服务端明确说到头了，才说「没有更多」。
                    let text = if self.comment_section.list.is_end {
                        "没有更多评论"
                    } else {
                        "评论加载中…"
                    };
                    list = list.child(panel_empty_line(t, text));
                } else {
                    let text_width = crate::comment_section::text_width(PLAYER_INFO_PANEL_WIDTH);
                    let weak = cx.entity().downgrade();
                    let copy_notice = Rc::new(move |cx: &mut App| {
                        let _ = weak.update(cx, |view, cx| {
                            view.show_interaction_note("已复制".into(), false, cx);
                        });
                    });
                    let handler = self.image_handler.clone();
                    let on_image = Rc::new(
                        move |urls: Vec<String>,
                              index: usize,
                              window: &mut Window,
                              cx: &mut App| {
                            if let Some(handler) = handler.as_ref() {
                                handler(cx, window, urls, index);
                            }
                        },
                    ) as Rc<dyn Fn(Vec<String>, usize, &mut Window, &mut App)>;
                    list = list.child(self.comment_section.body(
                        t,
                        window,
                        &self.panel_scroll,
                        text_width,
                        "player-comment",
                        copy_notice,
                        Some(on_image),
                        cx,
                    ));
                }
            }
        }

        self.comment_section
            .maybe_load_more(&self.panel_scroll, cx);

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(SPACE_2))
                    .px(px(SPACE_4))
                    .pb(px(SPACE_2))
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(s.text_primary)
                            .child(format!("评论 {}", format::count(total))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(SPACE_2))
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_quaternary)
                            .child(
                                div()
                                    .text_color(s.text_primary)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("最热"),
                            )
                            .child(div().child("|"))
                            .child(div().child("最新")),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    fn danmaku_tab(&self, t: &YkTheme) -> AnyElement {
        match &self.danmaku_state {
            DanmakuState::Idle => return panel_empty_line(t, "暂无弹幕").into_any_element(),
            DanmakuState::Loading => {
                return panel_empty_line(t, "弹幕加载中…").into_any_element();
            }
            DanmakuState::Error(error) => {
                return panel_empty_line(t, &format!("弹幕加载失败：{error}")).into_any_element();
            }
            DanmakuState::Ready => {}
        }
        if self.danmaku_all.is_empty() {
            return panel_empty_line(t, "暂无弹幕").into_any_element();
        }

        let s = &t.surfaces;
        let mut list = div().w_full().px(px(SPACE_4)).pb(px(SPACE_4));
        for danmaku in self.danmaku_all.iter().take(60) {
            let mode = match danmaku.mode {
                DanmakuMode::Top => Some("顶"),
                DanmakuMode::Bottom => Some("底"),
                DanmakuMode::Scroll => None,
            };
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(SPACE_2))
                    .py(px(5.))
                    .border_b_1()
                    .border_color(s.border_subtle)
                    .child(
                        div()
                            .w(px(40.))
                            .flex_shrink_0()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_quaternary)
                            .child(format::duration(danmaku.time.max(0.0) as u64)),
                    )
                    .when_some(mode, |el, mode| {
                        el.child(
                            div()
                                .w(px(16.))
                                .flex_shrink_0()
                                .text_center()
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(s.text_quaternary)
                                .child(mode),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_secondary)
                            .line_clamp(2)
                            .child(danmaku.text.clone()),
                    ),
            );
        }
        list.into_any_element()
    }

    fn pages_tab(&self, t: &YkTheme) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let source = self.source.as_ref();
        let current = source.map(|source| source.current_page).unwrap_or(1);
        let pages = source.map(|source| source.pages.as_slice()).unwrap_or(&[]);
        let mut list = div()
            .w_full()
            .px(px(SPACE_4))
            .pb(px(SPACE_4))
            .flex()
            .flex_col()
            .gap(px(4.));
        if pages.is_empty() {
            list = list.child(panel_empty_line(t, "暂无分 P 信息"));
        } else {
            for page in pages {
                let on = page.page == current;
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_2))
                        .px(px(SPACE_2))
                        .py(px(SPACE_2))
                        .rounded(px(RADIUS_MD))
                        .when(on, |el| el.bg(s.fill_2))
                        .child(
                            div()
                                .w(px(16.))
                                .flex_shrink_0()
                                .text_color(p.theme_primary)
                                .when(on, |el| {
                                    el.child(
                                        Icon::new(IconName::Check)
                                            .size(px(14.))
                                            .text_color(p.theme_primary),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(if on { s.text_primary } else { s.text_secondary })
                                .child(format!("P{} · {}", page.page, page.part)),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .child(if page.duration > 0 {
                                    format::duration(page.duration)
                                } else {
                                    String::new()
                                }),
                        ),
                );
            }
        }
        list.into_any_element()
    }
}

/// 左下角浮层的小图标按钮（24 热区，16 图标）。
fn overlay_icon(
    id: &'static str,
    icon: IconName,
    color: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(24.))
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF24)))
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
        .on_click(on_click)
}

/// 元信息行的一项：12px 图标 + 计数。
fn meta_stat(icon: IconName, text: String) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .child(Icon::new(icon).size(px(12.)))
        .child(text)
}

/// 复制链接的文本形态：`链接 + 换行 + 标题`。
///
/// 只贴一条裸链接，粘到聊天框里对方看不出是什么视频；带上标题一行，
/// 既认得出内容，链接本身也还能点开（和 B 站网页版复制的形态一致）。
/// 标题为空时退回裸链接，不留一个空行。
fn share_link_text(link: &str, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        link.to_string()
    } else {
        format!("{link}
{title}")
    }
}

/// 三个互动弹层。退场动画期间 bool 已经清零，靠它才知道该重画哪一张卡。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionOverlay {
    Coin,
    Fav,
    Share,
}

/// 收藏夹条数文案：四位以内给「N 个」，过万用「万」单位，别让数字把标题挤掉。
fn fav_count_label(count: u64) -> String {
    if count >= 10_000 {
        format::count(count)
    } else {
        format!("{count} 个")
    }
}

/// 互动行的一项：图标在上、计数/文案在下。
/// 互动按钮：图标 + 数字；已经互动过（点赞 / 投币 / 收藏）用主色高亮。
fn panel_action_button(
    t: &YkTheme,
    id: &'static str,
    icon: IconName,
    label: String,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let color = if active {
        p.theme_primary
    } else {
        s.text_tertiary
    };
    div()
        // id 必须每个按钮都不同：GPUI 用元素 id 做事件索引，
        // 四个按钮共用一个 id 会互相覆盖，点击落不到对应的回调上。
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.))
        .px(px(SPACE_2))
        .py(px(SPACE_1))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2))
        .text_size(px(FONT_SIZE_MICRO))
        .text_color(color)
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
        .child(label)
        .on_click(on_click)
}

/// 视频页面链接（分享用）。
fn video_page_url(bvid: &str) -> String {
    format!("https://www.bilibili.com/video/{}", bvid.trim())
}

/// 腾讯 QQ 分享卡片链接。
///
/// QQ 桌面版注册的 `tencent://` / `mqqapi://` 只能跳到指定聊天对象，
/// 没有「选好友分享」的入口；网页端的分享卡片才是腾讯官方给的那条路。
fn qq_share_url(link: &str, title: &str, desc: &str) -> String {
    format!(
        "https://connect.qq.com/widget/shareqq/index.html?url={}&title={}&desc={}&summary={}",
        percent_encode(link),
        percent_encode(title),
        percent_encode(desc),
        percent_encode(desc)
    )
}

/// 极简 percent encode：只保留 URL 里的安全字符，其余按字节转义。
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 用系统默认程序打开链接（macOS 用 `open`，其它平台用 `xdg-open`）。
fn open_in_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// 评论加载骨架：圆头像 + 三行灰条。
fn comment_skeleton(t: &YkTheme) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_start()
        .gap(px(SPACE_2))
        .py(px(SPACE_2))
        .border_b_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .flex_shrink_0()
                .rounded_full()
                .bg(s.fill_2),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .w(relative(0.36))
                        .h(px(10.))
                        .rounded_full()
                        .bg(s.fill_2),
                )
                .child(
                    div()
                        .w(relative(0.92))
                        .h(px(12.))
                        .rounded_full()
                        .bg(s.fill_1),
                )
                .child(
                    div()
                        .w(relative(0.64))
                        .h(px(12.))
                        .rounded_full()
                        .bg(s.fill_1),
                ),
        )
}

/// 面板里的空态 / 加载文案。
fn panel_empty_line(t: &YkTheme, text: &str) -> impl IntoElement {
    div()
        .w_full()
        .py(px(SPACE_6))
        .text_center()
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(t.surfaces.text_quaternary)
        .child(text.to_string())
}

/// 占位弹幕（真实数据要从 `/x/v1/dm/list.so` 拉 XML，属于后续）。
/// 给顶部 / 底部固定弹幕找一个空槽位。
///
/// 槽位在这个时间点已被占满时返回 `None`，调用方把那一条留在列表里、
/// 不上屏 —— 比硬挤在同一行、文字互相盖住要好。
fn take_fixed_slot(free: &mut [f64], time: f64) -> Option<usize> {
    let slot = free.iter().position(|end| time >= *end)?;
    free[slot] = time + FIXED_DANMAKU_DURATION;
    Some(slot)
}

/// 顶部 / 底部固定弹幕在 `now` 是否还该显示。
fn fixed_danmaku_visible(d: &Danmaku, now: f64) -> bool {
    now >= d.time && now <= d.time + FIXED_DANMAKU_DURATION
}

// ================= 视频层 =================

/// macOS：CVPixelBuffer 交给 GPUI 的 `surface()`，与控件同管线合成。
///
/// 其它平台（含 iOS）：`surface()` 在 gpui-pre 0.3.9 里只在 Apple 桌面端可用，
/// 这里先渲染占位。iOS 的真实链路（libmpv 解码 → 纹理）属于 M8 的后续工作，
/// 解码方案已定：**libmpv**（跨桌面 + 移动端同一套）。
#[cfg(target_os = "macos")]
fn video_layer(
    frame: &TestFrame,
    current: Option<&objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>>,
) -> impl IntoElement {
    // 有真实帧就用真实的，否则退回 spike 的测试帧
    let buffer = match current {
        Some(b) => b.clone(),
        None => frame.buffer(),
    };
    // 等比 letterbox（L-2）：`surface()` 的 object_fit 默认就是 Contain。
    // 注意 surface 自身带有 CVPixelBuffer 的固有尺寸，直接当普通 child 会贴在
    // 左上角；这里用满尺寸 flex 容器把它居中，视频才真正落在播放器中心。
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(surface(buffer).object_fit(ObjectFit::Contain).size_full())
}

/// Windows：BGRA 帧包成 `RenderImage`，用 `img()` 贴到播放层。
///
/// 视频帧按 Contain 等比缩放；容器满尺寸并居中，和 macOS 的 `surface()`
/// 保持同一套 letterbox 行为。
#[cfg(target_os = "windows")]
fn video_layer(
    _frame: &TestFrame,
    current: Option<&std::sync::Arc<RenderImage>>,
) -> AnyElement {
    let content = match current {
        Some(image) => img(ImageSource::Render(image.clone()))
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        None => div().size_full().bg(rgb(0x101014)).into_any_element(),
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(content)
        .into_any_element()
}

/// 其它平台（含 iOS）：`surface()` 在 gpui-pre 0.3.9 里只在 Apple 桌面端可用，
/// Windows 走上面的贴图路径，其余平台先渲染占位。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn video_layer(_frame: &TestFrame) -> impl IntoElement {
    div().absolute().inset_0().bg(rgb(0x101014)).child(
        div()
            .absolute()
            .left(px(12.))
            .bottom(px(12.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(RADIUS_SM))
            .bg(rgba(0x000000AA))
            .text_size(px(FONT_SIZE_MICRO))
            .text_color(rgb(0xFFFFFF))
            .child("视频层占位 · 该平台待接 libmpv"),
    )
}

// ================= 控制栏 =================

/// 悬浮控制栏：弹幕输入 38 + 操作条 50 = 88。
///
/// 横向固定在 `left:0 / right:200px`（收起态面板宽度）——播放器宽度不随面板伸缩；
/// 面板展开到 400 时允许盖住右端，这是用户主动展开换来的代价。
fn control_bar(
    t: &YkTheme,
    state: &PlayerState,
    danmaku_input: Option<&Entity<InputState>>,
    danmaku_sending: bool,
    quality: &str,
    video_left: f32,
    video_width: f32,
    progress_track_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
    on_play: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (time_now, time_total) = state.time_texts();
    let progress = state.progress().clamp(0.0, 1.0) as f32;
    let s = &t.surfaces;
    let p = &t.palette;
    let icon_color = hsla(0.0, 0.0, 1.0, 0.86);
    let rate_text = format!("倍速 {}x", trim_rate(state.rate));
    let volume_icon = if state.muted || state.volume <= 0.001 {
        IconName::VolumeX
    } else {
        IconName::Volume2
    };
    let quality_text = if quality.is_empty() {
        "画质".to_string()
    } else {
        quality.to_string()
    };

    div()
        .absolute()
        // 控制条跟视频实际显示宽度对齐并水平居中；面板展开时允许盖住右端。
        .left(px(video_left))
        .w(px(video_width))
        .bottom_0()
        .h(px(CTRL_H))
        .flex()
        .flex_col()
        // 黑框只包住控制栏本体：88px 实底 + 1px 顶边，不再铺 208px 渐变遮罩。
        .bg(rgba(0x0B0F14E6))
        .border_t_1()
        .border_color(rgba(0xFFFFFF14))
        .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
            view.note_pointer_move(event.position, window, cx);
        }))
        // ---- 上半：弹幕礼仪 + 输入 + 发送 38 ----
        .child(
            div()
                .h(px(DANMAKU_INPUT_H))
                .w_full()
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.45))
                        .child("弹幕礼仪 ›"),
                )
                .child(match danmaku_input {
                    Some(st) => {
                        // Input 元素本身不 track_focus，键盘事件不会路由过去。
                        // 必须由外层接管焦点：track_focus 让元素参与焦点系统，
                        // 点击时显式 window.focus() 把输入焦点交给它。
                        let handle = st.read(cx).focus_handle(cx);
                        div()
                            .id("danmaku-input")
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(26.))
                            // Enter 的绑定是 `KeyBinding::new("enter", Enter, Some("Input"))`；
                            // 焦点落在这个外层 div 上，必须补 key_context 才能匹配到 Input 的 action。
                            .key_context("Input")
                            .track_focus(&handle)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let handle = handle.clone();
                                    move |view, _ev: &MouseDownEvent, window, cx| {
                                        // 已经聚焦就什么都不做 ——
                                        // 重复 window.focus + notify 会让 Input 重建，
                                        // 表现为「点一下已输入内容的输入框，内容被清掉」。
                                        if handle.is_focused(window) {
                                            return;
                                        }
                                        view.input_focused = true;
                                        window.focus(&handle, cx);
                                        cx.notify();
                                    }
                                }),
                            )
                            // Input 自己不撑满时，hitbox 只有文字那么宽 ——
                            // 点击空白处不会聚焦。这里让它撑满整条。
                            .child(
                                Input::new(st)
                                    .appearance(false)
                                    .bordered(false)
                                    .w_full()
                                    .h_full(),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(26.))
                        .px(px(SPACE_2))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0xFFFFFF1A))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.55))
                        .child("发个友善的弹幕见证当下")
                        .into_any_element(),
                })
                .child(
                    div()
                        .id("danmaku-send")
                        .px(px(SPACE_3))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(p.theme_primary)
                        // 发送中置灰：请求没回来前再点没有意义（send_danmaku 里也会挡）。
                        .opacity(if danmaku_sending { 0.5 } else { 1.0 })
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(rgb(0xFFFFFF))
                        .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                            view.send_danmaku(window, cx);
                        }))
                        .child(if danmaku_sending { "发送中" } else { "发送" }),
                ),
        )
        // ---- 下半：播放 / 时间 / 进度 / 画质 / 倍速 / 图标 50 ----
        .child(
            div()
                .h(px(50.))
                .w_full()
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .id("play-toggle")
                        .w(px(32.))
                        .h(px(32.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0xFFFFFF1A)))
                        .on_click(on_play)
                        .child(
                            Icon::new(if state.playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            })
                            .size(px(ICON_SIZE_SM))
                            .text_color(rgb(0xFFFFFF)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.85))
                        .child(format!("{time_now} / {time_total}")),
                )
                // 进度条：左侧已播 + 右侧未播，滑块骑在交界处。
                // 轨道自身记录 bounds，按下 / 拖动时把鼠标 x 换算成 0..1。
                .child({
                    let track_bounds = progress_track_bounds.clone();
                    div()
                        .id("player-progress-track")
                        .relative()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(12.))
                        .cursor_pointer()
                        // 按下 / 拖动由 `pointer_tracker` 的全局监听处理，
                        // 不走 hover 命中，避免播放中 hover 抖动导致点不中。
                        // 整条底轨：同时用来量轨道宽度。
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(px(4.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgba(0xFFFFFF33)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(4.))
                                .h(px(4.))
                                .w(relative(progress))
                                .rounded_full()
                                .bg(p.theme_primary)
                                .child(
                                    div()
                                        .absolute()
                                        .right(px(-6.))
                                        .top(px(-4.))
                                        .w(px(12.))
                                        .h(px(12.))
                                        .rounded_full()
                                        .bg(rgb(0xFFFFFF)),
                                ),
                        )
                        // 轨道 bounds 由这层透明 canvas 在 prepaint 时写回。
                        .child(
                            canvas(
                                move |bounds, _, _| track_bounds.set(Some(bounds)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                })
                .child(ctrl_pill(quality_text))
                .child(ctrl_pill(rate_text))
                .child(ctrl_icon(volume_icon, icon_color))
                .child(ctrl_icon(IconName::Captions, icon_color))
                .child(ctrl_icon(IconName::Settings, icon_color))
                .child(ctrl_icon(IconName::Maximize, icon_color)),
        )
        .text_color(s.text_on_accent)
}

/// 倍速文案：整数也保留一位小数（`1.0x`），非整数按实际给出（`1.25x`）。
fn trim_rate(rate: f64) -> String {
    if (rate.fract()).abs() < 1e-6 {
        format!("{rate:.1}")
    } else {
        let text = format!("{rate:.2}");
        text.trim_end_matches('0').to_string()
    }
}

/// 控制栏右侧胶囊（画质 / 倍速）。
fn ctrl_pill(label: String) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .h(px(28.))
        .px(px(SPACE_2))
        .flex()
        .items_center()
        .rounded(px(RADIUS_HALF))
        .bg(rgba(0xFFFFFF1F))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF33)))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(hsla(0.0, 0.0, 1.0, 0.90))
        .child(label)
}

/// 控制栏右侧图标按钮（音量 / 字幕 / 设置 / 全屏）。
fn ctrl_icon(icon: IconName, color: Hsla) -> impl IntoElement {
    div()
        .w(px(32.))
        .h(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_HALF))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF1A)))
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
}

impl crate::comment_section::CommentHost for PlayerView {
    fn comments(&mut self) -> &mut crate::comment_section::Section {
        &mut self.comment_section
    }

    fn comment_notice(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.show_interaction_note(text, error, cx);
    }

    fn comment_focus_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cursor_local, percent_encode, qq_share_url, share_link_text, video_page_url,
        COMMENT_PAGE_SIZE, COMMENT_REQUEST_INTERVAL,
    };
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
