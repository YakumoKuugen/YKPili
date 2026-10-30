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
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use yk_media::MpvPlayer;

/// 一帧 BGRA 画面（字节序与 GPUI `RenderImage` 一致）。
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4`，B/G/R/A。
    pub bgra: Vec<u8>,
    /// 单调递增帧号：UI 用它判断「有没有换帧」，避免同一帧重复建贴图。
    pub serial: u64,
}

struct SendFrame(VideoFrame, f64);

enum WorkerCommand {
    Pause(bool),
    Seek(f64),
}

/// 渲染线程句柄。
pub struct FrameWorker {
    rx: Receiver<SendFrame>,
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
        let (tx, rx) = mpsc::channel::<SendFrame>();
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
                while !stop_in_thread.load(Ordering::Relaxed) {
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
                        }
                    }
                    player.drain_events();
                    let t = player.get_property_double("time-pos").unwrap_or(0.0);
                    // 结束信号每轮都算：播到结尾后可能没有新帧。
                    let eof_reached = player.get_property_flag("eof-reached").unwrap_or(false);
                    let duration = player.get_property_double("duration").unwrap_or(0.0);
                    eof_in_thread.store(
                        yk_core::player::is_playback_finished(t, duration, eof_reached),
                        Ordering::Relaxed,
                    );
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
                            // 队列里积压的旧帧由 UI 侧的 latest() 丢弃。
                            if tx.send(SendFrame(out, t)).is_err() {
                                break;
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
                    std::thread::sleep(Duration::from_millis(16));
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

    /// 播放是否已到结尾（UI 侧据此自动关闭播放器）。
    pub fn finished(&self) -> bool {
        self.eof.load(Ordering::Relaxed)
    }

    /// 取最新一帧（丢弃中间积压的）。没新帧时返回 `None`。
    pub fn latest(&self) -> Option<(VideoFrame, f64)> {
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(f) => latest = Some((f.0, f.1)),
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
