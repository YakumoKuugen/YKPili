//! GPUI Kit + CVPixelBuffer 叠层最小验证
//!
//! 验证目标：**视频帧（CVPixelBuffer）与 GPUI Kit 控件能否合成在同一个渲染管线里**。
//!
//! 两个从源码里挖出来的硬性要求（GPUI 的 Metal renderer）：
//!   1. surface 的 CVPixelBuffer **必须是 NV12 双平面**（`420YpCbCr8BiPlanarFullRange`），
//!      源码里用 `assert_eq!` 断言，非该格式会被直接跳过。
//!   2. 必须是 **IOSurface-backed**，否则 `CVMetalTextureCache` 建不出纹理，
//!      因此创建时要传 `kCVPixelBufferIOSurfacePropertiesKey`。
//!
//! 验证方式：开启 test-support 后用 `Window::render_to_image()` 离屏读回已渲染帧，
//! 不依赖屏幕录制权限，也不需要窗口可见。

use std::ptr::{self, NonNull};

use gpui_kit::*;

use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVPixelBufferIOSurfacePropertiesKey,
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};

const SHOT_PATH: &str = "/Users/yakumokuugen/Documents/WorkSpaces/YKPili/spike-gpui-kit/shot.png";

/// 构造一帧 NV12 测试图像（IOSurface-backed）：
///   · Y 平面：横向亮度渐变，左上角纯白块、右下角纯黑块 → 用于确认画面方向
///   · UV 平面：左半偏蓝、右半偏红 → 用于确认色度平面也被正确采样
fn make_test_frame(width: usize, height: usize) -> CFRetained<CVPixelBuffer> {
    // ① IOSurface 属性：CVMetalTextureCache 要求 buffer 是 IOSurface-backed
    let attributes = CFDictionary::from_slices(
        &[unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
        &[&*CFDictionary::<CFType, CFType>::empty()],
    );

    let mut out: *mut CVPixelBuffer = ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreate(
            None,
            width,
            height,
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, // ② NV12 双平面
            Some(attributes.as_opaque()),
            NonNull::from(&mut out),
        )
    };
    assert_eq!(status, 0, "CVPixelBufferCreate 失败, status={status}");
    let buffer = unsafe { CFRetained::from_raw(NonNull::new(out).expect("pixel buffer 为 null")) };

    unsafe {
        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());

        // ---- 平面 0：Y（亮度，全分辨率）----
        let y_base = CVPixelBufferGetBaseAddressOfPlane(&buffer, 0).cast::<u8>();
        let y_stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 0);
        let y_w = CVPixelBufferGetWidthOfPlane(&buffer, 0);
        let y_h = CVPixelBufferGetHeightOfPlane(&buffer, 0);
        let box_size = 64.min(y_w / 8).min(y_h / 8);

        for y in 0..y_h {
            for x in 0..y_w {
                // 16..235 是 video range 的合法亮度区间
                let mut v = (16 + x * (235 - 16) / y_w.max(1)) as u8;
                if x < box_size && y < box_size {
                    v = 235; // 左上：白
                } else if x + box_size >= y_w && y + box_size >= y_h {
                    v = 16; // 右下：黑
                }
                *y_base.add(y * y_stride + x) = v;
            }
        }

        // ---- 平面 1：UV（色度，半分辨率，U/V 交错）----
        let uv_base = CVPixelBufferGetBaseAddressOfPlane(&buffer, 1).cast::<u8>();
        let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 1);
        let uv_w = CVPixelBufferGetWidthOfPlane(&buffer, 1);
        let uv_h = CVPixelBufferGetHeightOfPlane(&buffer, 1);

        for y in 0..uv_h {
            for x in 0..uv_w {
                let off = y * uv_stride + x * 2;
                // 左半偏蓝（U 高）、右半偏红（V 高，YUV 里 V 对应红色差）
                let (u, v) = if x < uv_w / 2 { (200u8, 90u8) } else { (90u8, 200u8) };
                *uv_base.add(off) = u;
                *uv_base.add(off + 1) = v;
            }
        }

        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
    }

    buffer
}

struct SpikeView {
    frame: CFRetained<CVPixelBuffer>,
    frames: u32,
    captured: bool,
}

impl Render for SpikeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        if self.frames <= 8 {
            println!("[spike] render 第 {} 帧被调用", self.frames);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }

        // 等前几帧渲染完再抓，确保 surface 已进入 scene
        // 注意：观察到这里第 4 帧后渲染会停住，因此在第 3 帧就抓
        if self.frames >= 3 && !self.captured {
            self.captured = true;
            match window.render_to_image() {
                Ok(img) => {
                    println!("[spike] render_to_image OK, 尺寸 {}x{}", img.width(), img.height());
                    match img.save(SHOT_PATH) {
                        Ok(_) => println!("[spike] ✅ 已保存: {SHOT_PATH}"),
                        Err(e) => println!("[spike] ❌ 保存失败: {e}"),
                    }
                }
                Err(e) => println!("[spike] ❌ render_to_image 失败: {e}"),
            }
            cx.quit();
        }

        div()
            .relative()
            .size_full()
            .bg(rgb(0x101014))
            // ① 视频层：NV12 CVPixelBuffer 交给 GPUI 的 surface 元素
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(surface(self.frame.clone()).size_full()),
            )
            // ② 叠层验证点（左上）
            .child(
                div()
                    .absolute()
                    .top_6()
                    .left_6()
                    .px_4()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(0x00A1D6))
                    .text_color(rgb(0xFFFFFF))
                    .text_lg()
                    .child("UI 控件叠加层 · OVERLAY-OK"),
            )
            // ③ 叠层验证点（右下）
            .child(
                div()
                    .absolute()
                    .bottom_6()
                    .right_6()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0xFB749A))
                    .text_color(rgb(0xFFFFFF))
                    .child("右下角控件"),
            )
    }
}

fn main() {
    application()
        .with_assets(assets::Assets)
        .run(|cx| {
            init(cx);
            let frame = make_test_frame(1280, 720);
            println!(
                "[spike] NV12 CVPixelBuffer 已构造: {}x{} (IOSurface-backed)",
                CVPixelBufferGetWidth(&frame),
                CVPixelBufferGetHeight(&frame)
            );
            open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| SpikeView { frame, frames: 0, captured: false })
            })
            .expect("[spike] open_window 失败");
        });
}
