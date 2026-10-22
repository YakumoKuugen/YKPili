//! GPUI + CVPixelBuffer 叠层最小验证
//!
//! 验证目标只有一个：**视频帧（CVPixelBuffer）与 GPUI 控件能否合成在同一个渲染管线里**。
//! 若能同时看到「渐变视频画面」和「叠在上面的蓝色标签」，则链路成立。
//!
//! 这里刻意不接 FFmpeg，用手工构造的 CVPixelBuffer 排除解码变量——
//! FFmpeg 的 VideoToolbox 硬解输出正是同一种 CVPixelBuffer。

use core_video::pixel_buffer::CVPixelBuffer;
use gpui::*;

/// kCVPixelFormatType_32BGRA = 'BGRA'
const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = 0x4247_5241;

/// 构造一帧测试图像：横向红色渐变 + 纵向绿色渐变 + 若干定位色块。
/// 色块用于确认「画面没有被翻转 / 没有串色」——这两个是纹理互操作最常见的坑。
fn make_test_frame(width: usize, height: usize) -> CVPixelBuffer {
    let buffer = CVPixelBuffer::new(K_CV_PIXEL_FORMAT_TYPE_32_BGRA, width, height, None)
        .expect("CVPixelBuffer 创建失败");

    buffer.lock_base_address(0);
    let stride = buffer.get_bytes_per_row();
    let base = buffer.get_base_address() as *mut u8;

    // 判断某个像素是否落在角落色块内（左上=纯白，右下=纯黑，用于验证方向）
    let in_box = |x: usize, y: usize, ox: usize, oy: usize, s: usize| {
        x >= ox && x < ox + s && y >= oy && y < oy + s
    };

    unsafe {
        for y in 0..height {
            for x in 0..width {
                let off = y * stride + x * 4;
                // 默认：横向红渐变 + 纵向绿渐变，蓝固定
                let (mut r, mut g, b) = (
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    160u8,
                );
                // 左上角 白块、右下角 黑块：若画面上下/左右翻转，这两个块的位置会反过来
                let s = 64.min(width / 8).min(height / 8);
                if in_box(x, y, 0, 0, s) {
                    r = 255; g = 255;
                } else if in_box(x, y, width - s, height - s, s) {
                    r = 0; g = 0;
                }
                // BGRA 内存布局
                *base.add(off) = b;
                *base.add(off + 1) = g;
                *base.add(off + 2) = r;
                *base.add(off + 3) = 255;
            }
        }
    }
    buffer.unlock_base_address(0);
    buffer
}

struct SpikeView {
    frame: CVPixelBuffer,
}

impl Render for SpikeView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x101014))
            .child(
                // 视频层：直接把 CVPixelBuffer 交给 GPUI
                div().size_full().child(surface(self.frame.clone())),
            )
            .child(
                // 叠层验证点：这个标签若可见，说明 UI 控件确实合成在视频之上
                div()
                    .absolute()
                    .top_6()
                    .left_6()
                    .px_4()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(0x00A1D6))
                    .text_color(rgb(0xFFFFFF))
                    .child("UI 控件叠加层 · VERIFY-OVERLAY-OK"),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let frame = make_test_frame(1280, 720);
        println!("[spike] CVPixelBuffer 已构造: {}x{}", frame.get_width(), frame.get_height());
        cx.open_window(WindowOptions::default(), move |_window, cx| {
            cx.new(|_cx| SpikeView { frame })
        })
        .expect("[spike] open_window 失败");
        println!("[spike] 窗口已创建 —— 若能看到渐变画面 + 蓝色标签，链路成立");
    });
}
