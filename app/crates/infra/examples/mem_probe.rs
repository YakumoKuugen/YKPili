//! 逐帧链路的内存探针：量 RSS 与「每帧 Rust 侧分配量」。
//!
//! 用它验证播放热路径有没有隐藏的分配抖动：
//!   old → `render_frame()`（克隆整帧）+ `rgba_to_nv12()`（新分配平面）
//!   new → `render_frame_ref()`（借用）+ `rgba_to_nv12_into()`（复用平面）
//!
//! 2026-11 实测（1080p）：old 10.88 MB/帧、new 0 MB/帧。后者是现在的
//! `video_worker` 路径，60fps 下少掉约 650 MB/s 的分配/释放抖动。
//!
//! 用法：
//! ```bash
//! cargo run -p yk-infra --example mem_probe -- /tmp/clip.mp4 new 20 1920 1080
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use yk_infra::media::convert::{rgba_to_nv12, rgba_to_nv12_into, Nv12Frame};
use yk_infra::media::pixel_buffer::nv12_to_pixel_buffer;
use yk_infra::media::MpvPlayer;

/// 统计 Rust 侧每帧分配量（CVPixelBuffer 由 CoreVideo 分配，不计入）。
struct Counting;
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(l.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOC_BYTES.fetch_add(n, Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.realloc(p, l, n)
    }
}
#[global_allocator]
static A: Counting = Counting;

fn rss_mb() -> u64 {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    out.parse::<u64>().unwrap_or(0) / 1024
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("用法：mem_probe <video> <render|nv12|full> <secs>");
    let mode = args.next().unwrap_or_else(|| "full".into());
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    let w: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1920);
    let h: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1080);

    let mut player = MpvPlayer::new(w, h).expect("创建播放器失败");
    player.load_file(&path).expect("加载失败");
    println!("[probe] mode={mode} {w}x{h} 起始 rss={}MB", rss_mb());

    let start = Instant::now();
    let mut frames = 0u64;
    let mut last_report = Instant::now();
    let mut last_bytes = ALLOC_BYTES.load(Ordering::Relaxed);
    let mut last_frames = 0u64;
    let frame_budget = Duration::from_micros(16_667);
    let mut scratch = Nv12Frame { width: 0, height: 0, y: Vec::new(), uv: Vec::new() };

    while start.elapsed() < Duration::from_secs(secs) {
        let tick = Instant::now();
        player.drain_events();
        match mode.as_str() {
            // 旧热路径：render_frame 克隆 8 MB，再分配 NV12。
            "old" => {
                if let Ok(frame) = player.render_frame() {
                    let nv12 = rgba_to_nv12(&frame.data, frame.width, frame.height);
                    let _buffer = nv12_to_pixel_buffer(&nv12);
                    frames += 1;
                }
            }
            // 新热路径：借用 + 复用 NV12。
            _ => {
                if let Ok(frame) = player.render_frame_ref() {
                    rgba_to_nv12_into(&mut scratch, frame.data, frame.width, frame.height);
                    let _buffer = nv12_to_pixel_buffer(&scratch);
                    frames += 1;
                }
            }
        }
        if last_report.elapsed() >= Duration::from_secs(1) {
            let bytes = ALLOC_BYTES.load(Ordering::Relaxed);
            let df = frames - last_frames;
            let per_frame = if df > 0 { (bytes - last_bytes) / df as usize } else { 0 };
            println!(
                "[probe] t={:>3}s frames={frames} rss={}MB rust_alloc/frame={:.2}MB",
                start.elapsed().as_secs(),
                rss_mb(),
                per_frame as f64 / 1_048_576.0
            );
            last_report = Instant::now();
            last_bytes = bytes;
            last_frames = frames;
        }
        if let Some(rest) = frame_budget.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    println!("[probe] 结束 frames={frames} rss={}MB", rss_mb());
}
