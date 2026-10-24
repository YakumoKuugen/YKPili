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

/// 渲染线程句柄。
pub struct FrameWorker {
    rx: Receiver<SendBuffer>,
    stop: Arc<AtomicBool>,
    pub video: String,
}

impl FrameWorker {
    /// 启动渲染线程。失败（找不到 libmpv / 打不开文件）返回 `None`。
    pub fn spawn(video: String, width: u32, height: u32) -> Option<Self> {
        // 防重复：open_window 的 build 闭包在本机被调用了多次（原因待查），
        // 每次都会构造 PlayerView → 每次都会起一个渲染线程，
        // 日志里出现过 14 个线程同时解码同一个文件。这里全局只放行一次。
        static SPAWNED: AtomicBool = AtomicBool::new(false);
        if SPAWNED.swap(true, Ordering::SeqCst) {
            eprintln!("[video] 已有渲染线程，跳过重复启动");
            return None;
        }

        let (tx, rx) = mpsc::channel::<SendBuffer>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_in_thread = stop.clone();
        let path = video.clone();

        std::thread::Builder::new()
            .name("yk-video-render".into())
            .spawn(move || {
                let mut player = match MpvPlayer::new(width, height) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[video] 渲染线程启动失败：{e}");
                        return;
                    }
                };
                if let Err(e) = player.load_file(&path) {
                    eprintln!("[video] load_file 失败：{e}");
                    return;
                }
                eprintln!("[video] 渲染线程已启动：{path}");

                while !stop_in_thread.load(Ordering::Relaxed) {
                    player.drain_events();
                    let t = player.get_property_double("time-pos").unwrap_or(0.0);
                    if let Ok(rgba) = player.render_frame() {
                        let nv12 = rgba_to_nv12(&rgba.data, rgba.width, rgba.height);
                        let buf = nv12_to_pixel_buffer(&nv12);
                        // 队列里积压的旧帧直接丢弃：UI 只要最新的
                        if tx.send(SendBuffer(buf, t)).is_err() {
                            break; // UI 侧已释放 channel
                        }
                    }
                    std::thread::sleep(Duration::from_millis(16));
                }
            })
            .ok()?;

        Some(Self { rx, stop, video })
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
