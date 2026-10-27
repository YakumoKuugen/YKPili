//! 视频渲染线程：把 mpv 解码 + 色彩转换 + 建 buffer 全部移出 UI 线程。
//!
//! **为什么必须独立线程**：这条链路每帧要 10–25ms（首帧 700ms+）——
//! mpv render、92 万像素的 RGBA→NV12、新建 IOSurface-backed CVPixelBuffer。
//! 放在 UI 线程会把输入事件、弹幕动画、音频回调全部拖慢。
//!
//! 架构：
//! ```text
//! 渲染线程                       UI 线程
//!   mpv render_frame
//!   → rgba_to_nv12               try_recv() 取最新帧
//!   → nv12_to_pixel_buffer       → surface(buffer)
//!   → channel.send              → notify()
//! ```
//!
//! 关键：**输入与播放互不耦合**。用户可以在播放时输入，也可以暂停，
//! 两者各自独立 —— 这是 UI 线程不再做重活的直接结果。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;

use yk_media::convert::rgba_to_nv12;
use yk_media::pixel_buffer::nv12_to_pixel_buffer;
use yk_media::MpvPlayer;

/// `CFRetained<CVPixelBuffer>` 本身没有 Send 标记。
///
/// 安全性依据：CVPixelBuffer 是 CoreVideo 的引用计数对象，
/// 本身线程安全；这里只是跨线程**移动**所有权，没有并发读写同一块像素内存
/// （渲染线程产出后就不再碰它，UI 线程只把它交给 surface 绘制）。
struct SendBuffer(CFRetained<CVPixelBuffer>, f64);
unsafe impl Send for SendBuffer {}

enum WorkerCommand {
    Pause(bool),
    Seek(f64),
}

/// 渲染线程句柄。
pub struct FrameWorker {
    rx: Receiver<SendBuffer>,
    commands: mpsc::Sender<WorkerCommand>,
    stop: Arc<AtomicBool>,
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
    ///
    /// **不要再用全局 `SPAWNED` 闸门**：点不同卡片时要换源重建 worker，
    /// 旧 worker drop 后必须允许新线程启动；防重复由 `PlayerView` 单实例 +
    /// `worker.is_some()` 保证。
    pub fn spawn_with_audio(
        video: String,
        audio: Option<String>,
        width: u32,
        height: u32,
        start_secs: f64,
    ) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<SendBuffer>();
        let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_in_thread = stop.clone();
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
                    match player.render_frame() {
                        Ok(rgba) => {
                            if frames_sent == 0 {
                                let vw = player.get_property_double("width").unwrap_or(0.0);
                                let vh = player.get_property_double("height").unwrap_or(0.0);
                                eprintln!(
                                    "[video] 首帧 {}x{}（片源 {vw:.0}x{vh:.0}）t={t:.2}",
                                    rgba.width, rgba.height
                                );
                            }
                            frames_sent += 1;
                            // 续播兜底：`start` 选项没生效时（部分片源会忽略）
                            // 补一刀 seek。判断放在首帧之后 —— 这时 time-pos
                            // 才是真实位置，加载完成那一刻读到的可能是 0。
                            if frames_sent == 1 && start_secs > 0.0 && (t - start_secs).abs() > 3.0
                            {
                                eprintln!(
                                    "[video] 起播位置没生效（当前 {t:.1}s，目标 {start_secs:.1}s），补 seek"
                                );
                                if let Err(e) = player.seek(start_secs) {
                                    eprintln!("[video] seek 失败：{e}");
                                }
                            }
                            let nv12 = rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
                            let buf = nv12_to_pixel_buffer(&nv12);
                            // 队列里积压的旧帧直接丢弃：UI 只要最新的
                            if tx.send(SendBuffer(buf, t)).is_err() {
                                break; // UI 侧已释放 channel
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
            video,
        })
    }

    /// 暂停 / 继续；命令投递给渲染线程，UI 线程不直接碰 mpv。
    pub fn set_paused(&self, paused: bool) {
        let _ = self.commands.send(WorkerCommand::Pause(paused));
    }

    /// 跳转到指定秒数；进度条拖动走这里，UI 线程不直接碰 mpv。
    pub fn seek(&self, position_secs: f64) {
        let _ = self.commands.send(WorkerCommand::Seek(position_secs));
    }

    /// 取最新一帧（丢弃中间积压的）。没新帧时返回 `None`。
    ///
    /// 无阻塞：UI 线程每帧调用一次，不应该等渲染线程。
    pub fn latest(&self) -> Option<(CFRetained<CVPixelBuffer>, f64)> {
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(b) => latest = Some((b.0, b.1)),
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
