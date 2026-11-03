//! 验证 libmpv FFI：解码一帧并存成 PNG。
//!
//! 从已删除的 `yk-media` crate 迁来；实现现在在 `yk_infra::media`。
//!
//! 用法：
//! ```bash
//! cargo run -p yk-infra --example frame_probe -- /path/to/video.mp4 /tmp/out.png
//! ```

use yk_infra::media::{save_png, MpvPlayer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let video = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/tmp/mpv-test.mp4".into());
    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "/tmp/mpv-frame.png".into());

    let mut player = match MpvPlayer::new(640, 360) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[probe] 创建播放器失败：{e}");
            std::process::exit(1);
        }
    };
    println!("[probe] libmpv 已加载，渲染目标 {:?}", player.dimensions());

    if let Err(e) = player.load_file(&video) {
        eprintln!("[probe] 加载视频失败：{e}");
        std::process::exit(1);
    }
    println!("[probe] 文件已加载：{video}");

    // 给解码器一点时间产出首帧
    std::thread::sleep(std::time::Duration::from_millis(400));

    match player.render_frame() {
        Ok(frame) => {
            // 抽样几个像素，确认不是全黑/全零
            let px = |x: usize, y: usize| -> (u8, u8, u8) {
                let i = (y * frame.width as usize + x) * 4;
                (frame.data[i], frame.data[i + 1], frame.data[i + 2])
            };
            println!(
                "[probe] 渲染成功 {}x{}，样本像素 左上={:?} 中心={:?}",
                frame.width,
                frame.height,
                px(4, 4),
                px(frame.width as usize / 2, frame.height as usize / 2)
            );
            if let Err(e) = save_png(&frame, &out) {
                eprintln!("[probe] 存 PNG 失败：{e}");
                std::process::exit(1);
            }
            println!("[probe] 已保存：{out}");
        }
        Err(e) => {
            eprintln!("[probe] 渲染失败：{e}");
            std::process::exit(1);
        }
    }
}
