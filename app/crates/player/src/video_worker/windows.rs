//! Windows 视频渲染线程：mpv 解码 → BGRA 帧 → UI 线程贴图。
//!
//! 和 macOS 版的区别只有「帧的形态」：这边不建 CVPixelBuffer，直接把 mpv 的
//! `bgra` 缓冲区搬进通道。UI 线程拿到后包成 `RenderImage`，由 GPUI 的
//! DirectX 图集上传到显存。
//!
//! 为什么仍然要独立线程：`mpv_render_context_render` + 3.7 MB 的缓冲区拷贝
//! 每帧要几毫秒，放在 UI 线程会拖慢输入与弹幕动画。UI 线程只做「取最新帧 +
//! 通知重绘」。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use yk_infra::media::MpvPlayer;

/// 一帧 BGRA 画面（字节序与 GPUI `RenderImage` 一致）。
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4`，B/G/R/A。
    pub bgra: Vec<u8>,
    /// 单调递增帧号：UI 用它判断「有没有换帧」，避免同一帧重复建贴图。
    pub serial: u64,
}

/// 视频线程 → UI 线程的一次投递。
pub enum WorkerFrame {
    /// 一帧画面 + 播放位置。
    Frame(VideoFrame, f64),
    /// 只有播放位置：窗口不可见时省掉像素拷贝与贴图上传。
    Position(f64),
}

enum WorkerCommand {
    Pause(bool),
    Seek(f64),
    /// 音量（0.0..=1.0）与静音；解码线程负责下发到 mpv。
    Volume(f64),
    Muted(bool),
    /// 窗口不可见（最小化 / 被完全遮挡）时停帧：mpv 继续播放音频，
    /// 只是不再「渲染 → BGRA 拷贝 → 新建贴图」每一帧，改为只报播放位置。
    RenderPaused(bool),
}

/// 渲染线程句柄。
pub struct FrameWorker {
    rx: Receiver<WorkerFrame>,
    commands: mpsc::Sender<WorkerCommand>,
    stop: Arc<AtomicBool>,
    /// 播放是否已到结尾（mpv `eof-reached`，或 time-pos 贴近 duration）。
    eof: Arc<AtomicBool>,
    pub video: String,
}

impl FrameWorker {
    /// 启动渲染线程。失败（找不到 libmpv / 打不开文件）返回 `None`。
    pub fn spawn(video: String, width: u32, height: u32) -> Option<Self> {
        Self::spawn_with_audio(video, None, width, height, 0.0)
    }

    /// 启动渲染线程；DASH 视频把独立音轨一起交给 mpv。
    ///
    /// `start_secs` = 起播位置（秒，0 = 从头播）。续播用：mpv 打开文件时
    /// 直接定位过去，省掉「先播开头再跳」的一次缓冲。
    pub fn spawn_with_audio(
        video: String,
        audio: Option<String>,
        width: u32,
        height: u32,
        start_secs: f64,
    ) -> Option<Self> {
        // 与 macOS 同一条理由：无界队列遇到 UI 卡顿会按 60fps 堆积整帧 BGRA，
        // 1080p 每帧 8 MB，几秒就是几百 MB。容量 2 + try_send，内存恒定。
        let (tx, rx) = mpsc::sync_channel::<WorkerFrame>(2);
        let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_in_thread = stop.clone();
        let eof = Arc::new(AtomicBool::new(false));
        let eof_in_thread = eof.clone();
        let path = video.clone();

        std::thread::Builder::new()
            .name("yk-video-render".into())
            .spawn(move || {
                let mut player = match MpvPlayer::new_with_start(width, height, start_secs) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[video] 渲染线程启动失败：{e}");
                        return;
                    }
                };
                if let Err(e) = player.load_file_with_audio(&path, audio.as_deref()) {
                    eprintln!("[video] load_file 失败：{e}");
                    return;
                }
                eprintln!(
                    "[video] 渲染线程已启动：{path}{}{}",
                    if audio.is_some() {
                        "（含独立音轨）"
                    } else {
                        ""
                    },
                    if start_secs > 0.0 {
                        format!("（从 {start_secs:.1}s 起播）")
                    } else {
                        String::new()
                    }
                );

                let mut frames_sent = 0u64;
                let mut render_errors = 0u32;
                let mut render_paused = false;
                // 目标帧间隔：60fps。注意这是「**上限**」而不是「固定睡眠」——
                // 单帧渲染 + 色彩转换本身要 10–25ms，再无条件 sleep 16ms 会把
                // 60fps 源压到 ~38fps（实测 Dropped 一路涨）。这里改成
                // 「补齐到目标间隔」：本帧已经超时就立刻进下一帧。
                let frame_budget = Duration::from_micros(16_667);
                while !stop_in_thread.load(Ordering::Relaxed) {
                    let frame_start = std::time::Instant::now();
                    while let Ok(command) = command_rx.try_recv() {
                        match command {
                            WorkerCommand::Pause(paused) => {
                                if let Err(e) = player.set_pause(paused) {
                                    eprintln!("[video] set pause={paused} 失败：{e}");
                                }
                            }
                            WorkerCommand::Seek(secs) => {
                                if let Err(e) = player.seek(secs) {
                                    eprintln!("[video] seek {secs:.1}s 失败：{e}");
                                }
                            }
                            WorkerCommand::Volume(volume) => {
                                if let Err(e) = player.set_volume(volume) {
                                    eprintln!("[video] 设置音量 {volume:.2} 失败：{e}");
                                }
                            }
                            WorkerCommand::Muted(muted) => {
                                if let Err(e) = player.set_muted(muted) {
                                    eprintln!("[video] 设置静音={muted} 失败：{e}");
                                }
                            }
                            WorkerCommand::RenderPaused(paused) => {
                                render_paused = paused;
                            }
                        }
                    }
                    player.drain_events();
                    let t = player.get_property_double("time-pos").unwrap_or(0.0);
                    // 结束信号每轮都算：播到结尾后可能没有新帧。
                    let eof_reached = player.get_property_flag("eof-reached").unwrap_or(false);
                    let duration = player.get_property_double("duration").unwrap_or(0.0);
                    eof_in_thread.store(
                        yk_domain::player::is_playback_finished(t, duration, eof_reached),
                        Ordering::Relaxed,
                    );
                    // 窗口不可见（最小化 / 被完全遮挡）：不出帧。
                    //
                    // mpv 自己的解码与音频照常走（`time-pos` 继续推进），
                    // 但整帧 BGRA 拷贝 + 贴图上传没人看就别做；
                    // 只把播放位置报回去，进度条与续播位置不能停。
                    if render_paused {
                        match tx.try_send(WorkerFrame::Position(t)) {
                            Ok(()) | Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Disconnected(_)) => break,
                        }
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    match player.render_frame_bgra() {
                        Ok(mut frame) => {
                            if frames_sent == 0 {
                                let vw = player.get_property_double("width").unwrap_or(0.0);
                                let vh = player.get_property_double("height").unwrap_or(0.0);
                                eprintln!(
                                    "[video] 首帧 {}x{}（片源 {vw:.0}x{vh:.0}）t={t:.2}",
                                    frame.width, frame.height
                                );
                            }
                            frames_sent += 1;
                            // 续播兜底：`start` 选项没生效时补一刀 seek。
                            if frames_sent == 1 && start_secs > 0.0 && (t - start_secs).abs() > 3.0
                            {
                                eprintln!(
                                    "[video] 起播位置没生效（当前 {t:.1}s，目标 {start_secs:.1}s），补 seek"
                                );
                                if let Err(e) = player.seek(start_secs) {
                                    eprintln!("[video] seek 失败：{e}");
                                }
                            }
                            // mpv 的 bgra 第 4 字节不保证是 255；GPUI 会按
                            // 预乘 alpha 混合，0 会让整帧透明。统一刷成不透明。
                            for alpha in frame.data.chunks_exact_mut(4).map(|px| &mut px[3]) {
                                *alpha = 255;
                            }
                            let out = VideoFrame {
                                width: frame.width,
                                height: frame.height,
                                bgra: frame.data,
                                serial: frames_sent,
                            };
                            // UI 还没取走上一帧就丢掉本帧：绝不积压。
                            match tx.try_send(WorkerFrame::Frame(out, t)) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {}
                                Err(TrySendError::Disconnected(_)) => break,
                            }
                        }
                        Err(e) => {
                            if render_errors < 5 {
                                render_errors += 1;
                                eprintln!(
                                    "[video] render_frame 失败（第 {render_errors} 次）：{e}"
                                );
                            }
                        }
                    }
                    // 补齐到目标帧间隔；本帧已经超时就不睡，直接进下一帧。
                    if let Some(rest) = frame_budget.checked_sub(frame_start.elapsed()) {
                        std::thread::sleep(rest);
                    }
                }
            })
            .ok()?;

        Some(Self {
            rx,
            commands: command_tx,
            stop,
            eof,
            video,
        })
    }

    /// 暂停 / 继续；命令投递给渲染线程，UI 线程不直接碰 mpv。
    pub fn set_paused(&self, paused: bool) {
        let _ = self.commands.send(WorkerCommand::Pause(paused));
    }

    /// 跳转到指定秒数；进度条拖动走这里。
    pub fn seek(&self, position_secs: f64) {
        let _ = self.commands.send(WorkerCommand::Seek(position_secs));
    }

    /// 设置音量（0.0..=1.0），解码线程下发到 mpv。
    pub fn set_volume(&self, volume: f64) {
        let _ = self.commands.send(WorkerCommand::Volume(volume));
    }

    /// 静音开关，解码线程下发到 mpv。
    pub fn set_muted(&self, muted: bool) {
        let _ = self.commands.send(WorkerCommand::Muted(muted));
    }

    /// 窗口不可见（最小化 / 被完全遮挡）时暂停出帧；audio 不受影响。
    pub fn set_render_paused(&self, paused: bool) {
        let _ = self.commands.send(WorkerCommand::RenderPaused(paused));
    }

    /// 播放是否已到结尾（UI 侧据此自动关闭播放器）。
    pub fn finished(&self) -> bool {
        self.eof.load(Ordering::Relaxed)
    }

    /// 取最新一帧（丢弃中间积压的）。没新帧时返回 `None`。
    pub fn latest(&self) -> Option<WorkerFrame> {
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(f) => latest = Some(f),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        latest
    }
}

impl Drop for FrameWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
