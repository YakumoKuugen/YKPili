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
//! 播放层右缘另有一条 88px 透明 hover 热区（覆盖在视频上、控制栏上方），用于唤出侧栏。
//! 播放器底边在 Dock 上方 20px（距窗口底部 92），宽度/高度随窗口缩放。
//!
//! **控制栏自动隐藏**：起播、窗口失焦、有焦点但鼠标 5s 不动时隐藏；
//! 鼠标移动 / 点击后重新浮出。隐藏时只剩底部 3px 进度线。

use gpui::assets::IconName;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{FocusHandle, InteractiveElement, StatefulInteractiveElement, *};
use yk_core::danmaku::{Danmaku, DanmakuSet, LaneAllocator};
use yk_core::dynamic::{asset_url, fitted_width, CommentTarget, DynamicComment};
use yk_core::format;
use yk_core::player::PlayerState;
use yk_ui::theme::*;

use crate::video::TestFrame;

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
/// 播放层右缘的 hover 热区宽度（覆盖在视频右侧、控制栏上方）。
/// 侧栏常驻右侧 200；播放层/控制栏宽度都是「窗口 − 200」，不再额外缩 88。
/// 88 只是控制栏高度 / 这条热区的宽度，不是控制栏与侧栏之间的空隙。
const PANEL_HOVER_WIDTH: f32 = 88.0;

/// 播放器下边界距窗口底部的距离。
///
/// Dock 高 52（40 + 6×2）+ 下边距 20 = 顶部在 72；播放器底边再留 20px 呼吸：
/// 72 + 20 = 92。窗口缩放时播放器跟随拉伸，不再固定 600 高。
const PLAYER_BOTTOM_INSET: f32 = 92.0;
/// 从封面展开：一条连续曲线走完，避免两段缓动在交界处出现速度断点。
/// 280ms 向 macOS Quick Look 的快速预览节奏靠拢，尾部只留很短的过冲回弹。
const PLAYER_ENTRANCE_MS: u64 = 280;
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
    Ready(Vec<DynamicComment>),
    Error(String),
}

pub struct PlayerView {
    theme: YkTheme,
    frame: TestFrame,
    /// 视频渲染线程（解码与转换都在它上面，UI 线程只收帧）
    #[cfg(target_os = "macos")]
    worker: Option<crate::video_worker::FrameWorker>,
    /// 最近一帧（由 worker 产出）
    #[cfg(target_os = "macos")]
    current: Option<objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>>,
    /// 播放循环是否已启动（worker 创建后同帧启动一次，不能等下一次 render）
    #[cfg(target_os = "macos")]
    loop_started: bool,
    /// 面板是否展开（hover 唤起）。
    expanded: bool,
    /// 当前 Tab：0 评论 / 1 弹幕 / 2 选集。
    tab: usize,
    /// 播放状态（音量/倍速/进度/全屏）全部来自 yk-core 的 PlayerState。
    state: PlayerState,
    /// 弹幕：已按时间排序，且每条都预分配了轨道
    danmaku: Vec<(Danmaku, usize)>,
    lanes: LaneAllocator,
    /// 最近一次从 mpv 读到的播放位置（秒），弹幕位置靠它算
    play_time: f64,
    /// 弹幕输入框状态（首次 render 时创建，那时才有 window）
    danmaku_input: Option<Entity<InputState>>,
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
    /// 关闭回调：`false` = 开始缩回，`true` = 缩回结束可移除实体。
    /// AppRoot 在开始时先恢复分类条/网格，让卡片回到封面快照的坐标。
    close_handler: Option<Box<dyn Fn(&mut App, bool)>>,
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
    /// 评论加载状态（打开视频后异步拉第一页）。
    comments: CommentState,
    /// 元数据请求代次：换源后旧请求的结果直接丢弃。
    meta_generation: u64,
    /// 左下角眼睛按钮：关掉后不再绘制弹幕层。
    danmaku_visible: bool,
    /// 关注按钮的本地态（本轮不写服务端，只做视觉反馈）。
    followed: bool,
    /// 面板 hover 延迟收起的代际：鼠标掠过边界不会闪收。
    panel_collapse_generation: u64,
    /// 面板宽度动画代际：每次展开 / 收起换一个 id，200ms EaseStandard 重新起跑。
    panel_generation: u64,
    /// 面板刚收起时的短暂抑制：鼠标还停在面板左缘/热区时不立刻重新弹出。
    panel_hover_grace_until: Option<std::time::Instant>,
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
            #[cfg(target_os = "macos")]
            worker: None,
            tried_worker: false,
            source: None,
            loading_source: false,
            source_error: None,
            history_report: None,
            load_generation: 0,
            #[cfg(target_os = "macos")]
            current: None,
            #[cfg(target_os = "macos")]
            loop_started: false,
            // 沉浸优先：播放浮窗默认收起信息面板（右侧 200 hover 热区唤出）。
            expanded: false,
            tab: 0,
            danmaku: {
                let set = mock_danmaku();
                // 预分配轨道：一次算完，播放时只查时间
                let mut lanes = LaneAllocator::new(1280.0, 220.0);
                let mut out = Vec::with_capacity(set.len());
                for d in set.items() {
                    if d.mode.is_scroll() {
                        let lane = lanes.assign(d);
                        out.push((d.clone(), lane));
                    }
                }
                out
            },
            lanes: LaneAllocator::new(1280.0, 220.0),
            play_time: 0.0,
            danmaku_input: None,
            submit_requested: false,
            input_focused: false,
            focus_handle: cx.focus_handle().tab_stop(true),
            needs_focus: true,
            escape_hint: false,
            escape_hint_generation: 0,
            entrance_source: None,
            entrance_generation: 0,
            closing: false,
            closing_generation: 0,
            close_handler: None,
            resume: yk_core::resume::ResumeStore::load_default(),
            resume_seed: None,
            start_position: 0.0,
            resume_key: None,
            last_resume_save: 0.0,
            tags: Vec::new(),
            comments: CommentState::Idle,
            meta_generation: 0,
            danmaku_visible: true,
            followed: false,
            panel_collapse_generation: 0,
            panel_generation: 0,
            panel_hover_grace_until: None,
            last_pointer: point(px(0.), px(0.)),
            controls_visible: true,
            last_activity: std::time::Instant::now(),
            controls_timer_generation: 0,
            controls_timer_running: false,
            activation_observed: false,
            activation_observer: None,
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
        #[cfg(target_os = "macos")]
        {
            self.worker = None;
            self.current = None;
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
        self.needs_focus = true;
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);
        // 续播上下文重置：起播位置要等取流成功（拿到时长）才能定。
        self.resume_seed = seed;
        self.start_position = 0.0;
        self.resume_key = None;
        self.last_resume_save = 0.0;
        // 信息面板数据跟着换源重置：旧视频的标签/评论不能落到新视频上。
        self.tags.clear();
        self.comments = CommentState::Idle;
        self.meta_generation = self.meta_generation.wrapping_add(1);
        self.followed = false;
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
                    // 等历史查询的结果，但最多等 2 秒。
                    let remote_progress = history_rx
                        .recv_timeout(std::time::Duration::from_secs(2))
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
                        view.source = Some(src);
                        view.begin_history_report(aid, bvid.clone(), cid, start);
                        // 标签 / 评论不阻塞起播：源挂上之后再拉，回来一轮重绘。
                        view.spawn_video_meta(aid, bvid, cx);
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

    /// 拉信息面板的标签与评论第一页。
    ///
    /// 起播不依赖这两个请求：取流成功、面板已有标题/UP 主之后再后台补。
    /// 失败只影响面板里的对应区块，不把整个播放器打成错误态。
    fn spawn_video_meta(&mut self, aid: i64, bvid: String, cx: &mut Context<Self>) {
        if aid <= 0 && bvid.trim().is_empty() {
            self.comments = CommentState::Ready(Vec::new());
            return;
        }
        self.meta_generation = self.meta_generation.wrapping_add(1);
        let generation = self.meta_generation;
        self.comments = CommentState::Loading;
        let cookie = current_session_cookie();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_core::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    let tags = if bvid.trim().is_empty() {
                        Vec::new()
                    } else {
                        match yk_core::api::client::fetch_video_tags(&client, &bvid, &cookie) {
                            Ok(tags) => tags,
                            Err(error) => {
                                eprintln!("[player] 标签加载失败：{error}");
                                Vec::new()
                            }
                        }
                    };
                    // 评论 oid 用 aid；没有 aid 的视频不请求，直接给空列表。
                    let comments = if aid > 0 {
                        let target = CommentTarget {
                            oid: aid.to_string(),
                            kind: 1,
                        };
                        yk_core::api::client::fetch_dynamic_comments(&client, &cookie, &target)
                    } else {
                        Ok(Vec::new())
                    };
                    Ok::<_, String>((tags, comments))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.meta_generation != generation {
                    return;
                }
                match result {
                    Ok((tags, comments)) => {
                        if !tags.is_empty() {
                            view.tags = tags;
                        }
                        view.comments = match comments {
                            Ok(list) => CommentState::Ready(list),
                            Err(error) => {
                                eprintln!("[player] 评论加载失败：{error}");
                                CommentState::Error(error)
                            }
                        };
                    }
                    Err(error) => {
                        eprintln!("[player] 评论加载失败：{error}");
                        view.comments = CommentState::Error(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 面板 hover：进入立刻展开；离开延迟 220ms 再收，避免掠边界闪收。
    fn set_panel_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
            if !self.expanded {
                self.expanded = true;
                self.panel_generation = self.panel_generation.wrapping_add(1);
                cx.notify();
            }
            return;
        }
        self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
        let generation = self.panel_collapse_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.panel_collapse_generation != generation || !view.expanded {
                    return;
                }
                view.expanded = false;
                view.panel_generation = view.panel_generation.wrapping_add(1);
                // 鼠标可能还停在面板左缘/热区：400ms 内不响应，避免反复弹出。
                view.panel_hover_grace_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
                cx.notify();
            });
        })
        .detach();
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
                    // 正在输入弹幕时不自动隐藏，否则输入框会从眼前消失。
                    if view.input_focused {
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
    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App, bool) + 'static) {
        self.close_handler = Some(Box::new(handler));
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

    fn panel_width(&self) -> f32 {
        if self.expanded {
            INFO_PANEL_EXPANDED
        } else {
            INFO_PANEL_COLLAPSED
        }
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

        shell.child(cover_poster(&theme, &source))
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
        eprintln!("[M3] PlayerView::render 被调用");
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
        #[cfg(target_os = "macos")]
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
        // libmpv 已在自动播放，这里只负责「把当前帧搬到 surface 上」。
        #[cfg(target_os = "macos")]
        if !self.loop_started && self.worker.is_some() {
            self.loop_started = true;
            cx.spawn(async move |this, cx| {
                let mut received = 0u64;
                let mut idle_ticks = 0u32;
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    // UI 线程只做「取一帧 + 通知重绘」，不碰任何解码/转换。
                    // 这就是输入与播放互不耦合的关键：即使正在打字，
                    // 渲染线程照常出帧，UI 也只是轻量地更新一下。
                    let alive = this.update(cx, |view, cx| {
                        let mut got = false;
                        if let Some(w) = view.worker.as_ref() {
                            if let Some((buf, t)) = w.latest() {
                                view.current = Some(buf);
                                view.play_time = t;
                                view.state.position_secs = t;
                                got = true;
                                // 播放中定期把进度落盘：进程被杀 / 崩溃时
                                // 也不至于丢掉续播位置。
                                if t - view.last_resume_save >= 30.0 {
                                    view.last_resume_save = t;
                                    view.save_resume_position();
                                }
                            }
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
        // 入场目标宽度跟随当前窗口；封面起点仍是点击时冻结的那一份。
        if let Some(source) = self.entrance_source.as_mut() {
            source.root_size = window.viewport_size();
        }
        // 控制区元素提前建好：可见时才挂树，隐藏时只留一条 3px 进度线。
        let watch_badge = self.controls_visible.then(|| self.watch_badge(cx));
        let bar: Option<AnyElement> = if self.controls_visible {
            Some(
                control_bar(
                    &t,
                    &self.state,
                    self.danmaku_input.as_ref(),
                    self.source
                        .as_ref()
                        .map(|source| source.quality.as_str())
                        .unwrap_or(""),
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
        let mini_progress = (!self.controls_visible).then(|| self.mini_progress(&t, cx));
        let panel_hover = (!self.expanded).then(|| self.panel_hover_zone(cx));
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
                        #[cfg(not(target_os = "macos"))]
                        {
                            video_layer(&self.frame)
                        }
                    }),
            )
            // ①b 弹幕层：gpui-fast 已解决 surface 与文字的合成冲突，
            // 直接 canvas 绘制即可（见附录 D 事实 16）。
            // 左下角眼睛按钮关掉后整层不挂树。
            .children(if self.danmaku_visible {
                Some(self.danmaku_layer())
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
            // ③ 视频右缘与面板左缘之间让出的 88px 是唤出热区。
            //    先挂热区、后挂控制栏：控制栏右端 88 的控件压在热区之上可点击。
            .children(panel_hover)
            .children(bar)
            .children(mini_progress)
            // ④ 信息面板：通高浮层，从播放页层顶一直到底
            .child(self.info_panel(&t, window.scale_factor(), cx))
            // ⑤ 骑边界按钮：面板之后挂载 → 压在面板之上（GPUI 没有 z-index）
            .child(self.panel_toggle(&t, cx))
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
            .on_hover(cx.listener(|view, hovered: &bool, _window, cx| {
                if !*hovered {
                    view.hide_controls(cx);
                }
            }))
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
            #[cfg(not(target_os = "macos"))]
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
                    if event.keystroke.key != "escape" {
                        return;
                    }
                    if this.escape_hint {
                        this.request_close(window, cx);
                    } else {
                        this.arm_escape_hint(cx);
                    }
                    cx.stop_propagation();
                }))
                .child(content)
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .child(backdrop)
            .child(window)
            // 第一次 Esc 提示放在浮窗之外：顶栏右侧。
            .children(self.escape_hint_overlay(&t, cx))
    }
}

// ================= 弹幕层 =================

impl PlayerView {
    /// 当前时刻该显示的滚动弹幕。
    ///
    /// 每帧只为「可见窗口」内的弹幕生成元素，不是全量 —— 一场直播可能有上万条，
    /// 全量遍历会直接掉帧。窗口前半段留 15 秒是因为一条弹幕最长要飞这么久。
    /// 把输入框里的文本作为一条弹幕发出去（本地回显，不接服务端）。
    fn send_danmaku(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.danmaku_input.as_ref() else {
            return;
        };
        let text = st.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }

        // 时间取当前播放位置（稍加提前量，让它马上进入视野而不是刚好在右边缘外）
        let time = self.play_time + 0.05;
        let d = Danmaku {
            time,
            mode: yk_core::danmaku::DanmakuMode::Scroll,
            font_size: 25.0,
            color: (0xFF, 0xFF, 0xFF),
            width: Danmaku::estimate_width(&text, 25.0),
            text,
        };
        // 分配轨道后再入列；新弹幕的时间一定比已有的大（当前时刻），
        // 所以追加到末尾不会破坏「从早到晚」的顺序。
        let lane = self.lanes.assign(&d);
        self.danmaku.push((d, lane));
        eprintln!(
            "[danmaku] 已发送，轨道 {lane}，共 {} 条",
            self.danmaku.len()
        );

        // 清空输入框（set_value 在 gpui-base 的 InputState 上，三参）
        if let Some(st) = self.danmaku_input.as_ref() {
            st.update(cx, |st, cx| st.set_value("", window, cx));
        }
        cx.notify();
    }

    fn danmaku_layer(&self) -> impl IntoElement {
        let now = self.play_time;

        // 先在普通 Rust 里把「这一帧要画什么」算好，交给 paint 闭包。
        // 闭包必须 'static，所以数据要 move 进去。
        struct Drawn {
            x: f32,
            y: f32,
            text: String,
            size: f32,
            color: gpui::Hsla,
        }
        let drawn: Vec<Drawn> = self
            .danmaku
            .iter()
            .filter(|(d, _)| self.lanes.visible_at(d, now))
            .take(80)
            .map(|(d, lane)| Drawn {
                x: self.lanes.x_at(d, now),
                y: 8.0 + *lane as f32 * 30.0,
                text: d.text.clone(),
                size: d.font_size,
                color: rgb(d.rgb()).into(),
            })
            .collect();

        // canvas：直接走底层 paint，不经过 div + text 的常规元素路径。
        // 常规路径下文字会破坏 surface 的合成（实测画面全黑），
        // 这条是文档第 14 节指定的方案 ②。
        canvas(
            |_bounds, _window, _cx| (),
            move |_bounds, _state, window, cx| {
                for d in &drawn {
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
                        point(px(d.x + 1.0), px(d.y + 2.0)),
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
                            point(px(d.x + dx), px(d.y + dy)),
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
                    let _ = line.paint(
                        point(px(d.x), px(d.y)),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            },
        )
        .absolute()
        .inset_0()
    }

    /// 播放 / 暂停：控制栏与画面中央按钮共用。
    fn toggle_playback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 点播放 = 放弃输入焦点，恢复渲染循环。
        self.input_focused = false;
        self.state.toggle_play();
        #[cfg(target_os = "macos")]
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

    /// 面板右侧横向让出的 88px hover 热区：鼠标进入即展开面板。
    ///
    /// 这个 88 是**横向**让位，不是面板底部的 88；面板高度是 top:0 → bottom:0 通高。
    fn panel_hover_zone(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("player-panel-hover")
            .absolute()
            .top_0()
            // 热区右边界对齐侧栏左边缘；宽度 88，覆盖播放层右缘。
            .right(px(INFO_PANEL_COLLAPSED))
            // 高度固定到控制栏上沿：不随控制栏显隐变化，也不去抢控制栏区域的
            // hover（否则控制栏隐藏 → 热区接管 → 控制栏浮出 → 热区被盖住 → 循环）。
            .bottom(px(CTRL_H))
            .w(px(PANEL_HOVER_WIDTH))
            .bg(rgba(0x00000001))
            .on_hover(cx.listener(|view, hovered: &bool, window, cx| {
                if !*hovered {
                    // 从热区移出：按正常面板移出规则延迟收起。
                    view.set_panel_hover(false, cx);
                    return;
                }
                if !window.is_window_active() {
                    return;
                }
                view.note_activity(window, cx);
                view.set_panel_hover(true, cx);
            }))
            .on_mouse_move(cx.listener(|view, _: &MouseMoveEvent, window, cx| {
                view.note_activity(window, cx);
            }))
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                // 触控 / 点击兜底：hover 不可用时也能打开面板。
                if !view.expanded {
                    view.expanded = true;
                    view.panel_generation = view.panel_generation.wrapping_add(1);
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// 控制栏隐藏时的简略态：底部 3px 进度线（40px 热区负责把控制栏叫回来）。
    fn mini_progress(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.state.progress().clamp(0.0, 1.0) as f32;
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
                div().w_full().h(px(3.)).bg(rgba(0xFFFFFF2E)).child(
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
            // 按钮骑在边界上：展开时悬停它不要触发 220ms 收起；
            // 收起时它只是点击入口，不靠 hover 自动展开（否则点击会被自己展开再关掉）。
            .on_hover(cx.listener(|view, hovered: &bool, _window, cx| {
                if *hovered {
                    if view.expanded {
                        view.panel_collapse_generation =
                            view.panel_collapse_generation.wrapping_add(1);
                    }
                } else if view.expanded {
                    view.set_panel_hover(false, cx);
                }
            }))
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
    fn info_panel(&self, t: &YkTheme, scale_factor: f32, cx: &mut Context<Self>) -> AnyElement {
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
            let desc = source.desc.clone();
            div()
                .px(px(SPACE_4))
                .pb(px(SPACE_3))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(SPACE_3))
                        .child(
                            div()
                                .relative()
                                .w(px(46.))
                                .h(px(46.))
                                .flex_shrink_0()
                                .rounded_full()
                                .overflow_hidden()
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
                                }),
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
                                .when(!desc.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .line_height(px(18.))
                                            .text_size(px(FONT_SIZE_CAPTION))
                                            .text_color(s.text_tertiary)
                                            .line_clamp(2)
                                            .child(desc),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
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
                                .cursor_pointer()
                                .hover(|style| style.bg(s.fill_3))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_secondary)
                                .child(
                                    Icon::new(IconName::List)
                                        .size(px(ICON_SIZE_SM))
                                        .text_color(s.text_secondary),
                                )
                                .child(if self.followed { "已关注" } else { "关注" })
                                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                                    view.followed = !view.followed;
                                    cx.notify();
                                })),
                        ),
                )
        });

        let actions = source.map(|source| {
            div()
                .flex()
                .items_center()
                .justify_around()
                .px(px(SPACE_4))
                .py(px(2.))
                .border_t_1()
                .border_color(s.border_subtle)
                .child(panel_action(
                    t,
                    IconName::ThumbsUp,
                    if source.like_count > 0 {
                        format::count(source.like_count)
                    } else {
                        "点赞".to_string()
                    },
                ))
                .child(panel_action(
                    t,
                    IconName::Coins,
                    if source.coin_count > 0 {
                        format::count(source.coin_count)
                    } else {
                        "投币".to_string()
                    },
                ))
                .child(panel_action(
                    t,
                    IconName::Star,
                    if source.favorite_count > 0 {
                        format::count(source.favorite_count)
                    } else {
                        "收藏".to_string()
                    },
                ))
                .child(panel_action(
                    t,
                    IconName::Share,
                    if source.share_count > 0 {
                        format::count(source.share_count)
                    } else {
                        "分享".to_string()
                    },
                ))
        });

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

        let body = div()
            .w(px(PLAYER_INFO_PANEL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .child(head)
            .children(meta)
            .children(up)
            .children(actions)
            .children(tags_row)
            .child(tabs_row)
            .child(self.tab_content(t, active, cx));

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
            // 侧栏贴着窗口右边界；让出 88px 的是左侧的视频区，不是面板。
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
            // 面板本体即 hover 热区（D-05）：进入展开，移出延迟 220ms 收回。
            .on_hover(cx.listener(|view, hovered: &bool, _window, cx| {
                view.set_panel_hover(*hovered, cx);
            }))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                view.note_pointer_move(event.position, window, cx);
            }))
            .child(body)
            .with_animation(
                format!("player-panel-{}", self.panel_generation),
                Animation::new(std::time::Duration::from_millis(200)).with_easing(ease_standard()),
                move |el, delta| el.w(px(from_width + (to_width - from_width) * delta)),
            )
            .into_any_element()
    }

    /// 面板 Tab 内容：评论 / 弹幕 / 选集。
    fn tab_content(&self, t: &YkTheme, active: usize, cx: &mut Context<Self>) -> AnyElement {
        match active {
            0 => self.comments_tab(t, cx),
            1 => self.danmaku_tab(t),
            _ => self.pages_tab(t),
        }
    }

    fn comments_tab(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let total = self
            .source
            .as_ref()
            .map(|source| source.reply_count)
            .unwrap_or(0);
        let retry = self
            .source
            .as_ref()
            .map(|source| (source.aid, source.bvid.clone()));

        let mut list = div()
            .id("player-comments-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(SPACE_4))
            .pb(px(SPACE_4));
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
            CommentState::Ready(items) if items.is_empty() => {
                list = list.child(panel_empty_line(t, "没有更多评论"));
            }
            CommentState::Ready(items) => {
                for comment in items {
                    list = list.child(comment_item(t, comment));
                }
                list = list.child(panel_empty_line(t, "没有更多评论"));
            }
        }

        div()
            .flex_1()
            .min_h(px(0.))
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
        let s = &t.surfaces;
        let mut list = div()
            .id("player-danmaku-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(SPACE_4))
            .pb(px(SPACE_4));
        let mut any = false;
        for (danmaku, _) in self.danmaku.iter().take(40) {
            any = true;
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
        if !any {
            list = list.child(panel_empty_line(t, "暂无弹幕"));
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
            .id("player-pages-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
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

/// 互动行的一项：图标在上、计数/文案在下。
fn panel_action(t: &YkTheme, icon: IconName, label: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.))
        .px(px(SPACE_2))
        .py(px(SPACE_1))
        .rounded(px(RADIUS_MD))
        .text_size(px(FONT_SIZE_MICRO))
        .text_color(t.surfaces.text_tertiary)
        .child(
            Icon::new(icon)
                .size(px(ICON_SIZE_SM))
                .text_color(t.surfaces.text_tertiary),
        )
        .child(label)
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

/// 信息面板的一条评论。
fn comment_item(t: &YkTheme, comment: &DynamicComment) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
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
                .overflow_hidden()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.0),
                    linear_color_stop(p.bili_pink, 1.0),
                ))
                .when(!comment.face.is_empty(), |el| {
                    el.child(
                        img(ImageSource::from(comment.face.clone()))
                            .w(px(28.))
                            .h(px(28.))
                            .object_fit(ObjectFit::Cover),
                    )
                }),
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
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_quaternary)
                        .truncate()
                        .child(comment.author.clone()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(s.text_primary)
                        .line_height(px(20.))
                        .line_clamp(3)
                        .child(comment.text.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_3))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(comment.time.clone())
                        .when(comment.like > 0, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.))
                                    .child(Icon::new(IconName::ThumbsUp).size(px(11.)))
                                    .child(format::count(comment.like)),
                            )
                        }),
                ),
        )
}

/// 占位弹幕（真实数据要从 `/x/v1/dm/list.so` 拉 XML，属于后续）。
fn mock_danmaku() -> DanmakuSet {
    const TEXTS: [&str; 12] = [
        "前方高能",
        "这也太真实了",
        "哈哈哈哈",
        "有一说一，确实",
        "第一次看到这种",
        "好家伙",
        "这个可以",
        "泪目",
        "up主加油",
        "妙啊",
        "学到了",
        "名场面",
    ];
    let raw = (0..300).map(|i| {
        // 从 -6 秒开始：这样首帧（play_time≈0）就有弹幕在屏幕上，
        // 便于截图验收；真实数据里时间是 0 起。
        let t = (i as f64) * 0.22 - 6.0;
        let color = if i % 7 == 0 { 16777215u32 } else { 65280 };
        let p = format!("{t},1,25,{color},0,0,0,0");
        (p, TEXTS[i % TEXTS.len()].to_string())
    });
    DanmakuSet::from_raw(raw)
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

#[cfg(not(target_os = "macos"))]
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
            .child("视频层占位 · iOS 待接 libmpv"),
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
    quality: &str,
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
        .left_0()
        // 控制条宽度 = 播放层（红框/蓝框）宽度：右端直接贴到侧栏左边缘。
        .right(px(INFO_PANEL_COLLAPSED))
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
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(rgb(0xFFFFFF))
                        .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                            view.send_danmaku(window, cx);
                        }))
                        .child("发送"),
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
                // 进度条：左侧已播 + 右侧未播，滑块骑在交界处
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(12.))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .relative()
                                .flex_shrink_0()
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
                        .child(div().flex_1().h(px(4.)).rounded_full().bg(rgba(0xFFFFFF33))),
                )
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
