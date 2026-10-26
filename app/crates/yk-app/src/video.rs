//! 视频帧来源（M3 用，接真实解码器前先接测试帧）。
//!
//! GPUI 的 Metal renderer 对 `surface()` 有两条硬要求（spike 已从源码验证）：
//! 1. CVPixelBuffer 必须是 **NV12 双平面**（`420YpCbCr8BiPlanarFullRange`）；
//! 2. 必须 **IOSurface-backed**，否则 `CVMetalTextureCache` 建不出纹理。
//!
//! 这里先构造一帧人造画面用于叠层验证；M3 之后接 libmpv / AVFoundation 解码时，
//! 只要仍然产出满足上述两条的 CVPixelBuffer，视图层不用改。

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
pub use other::*;

#[cfg(target_os = "macos")]
mod mac {
    use std::ptr::{self, NonNull};

    use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
    use objc2_core_video::{
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVPixelBufferIOSurfacePropertiesKey,
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeightOfPlane,
        CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress,
    };

    /// 一帧 IOSurface-backed 的 NV12 画面。
    pub struct TestFrame {
        buffer: CFRetained<CVPixelBuffer>,
    }

    impl TestFrame {
        pub fn new(width: usize, height: usize) -> Self {
            Self { buffer: make(width, height) }
        }

        pub fn buffer(&self) -> CFRetained<CVPixelBuffer> {
            self.buffer.clone()
        }
    }

    impl Clone for TestFrame {
        fn clone(&self) -> Self {
            Self { buffer: self.buffer.clone() }
        }
    }

    fn make(width: usize, height: usize) -> CFRetained<CVPixelBuffer> {
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
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
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
                        v = 235; // 左上：白（确认画面方向）
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
                    // 左半偏蓝（U 高）、右半偏红（V 高）
                    let (u, v) = if x < uv_w / 2 { (200u8, 90u8) } else { (90u8, 200u8) };
                    *uv_base.add(off) = u;
                    *uv_base.add(off + 1) = v;
                }
            }

            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
        }

        buffer
    }
}

#[cfg(not(target_os = "macos"))]
mod other {
    /// 非 macOS 平台暂时没有视频管线；用占位，保证其余布局可在任何平台编译验证。
    #[derive(Clone)]
    pub struct TestFrame;

    impl TestFrame {
        pub fn new(_width: usize, _height: usize) -> Self {
            Self
        }
    }
}
