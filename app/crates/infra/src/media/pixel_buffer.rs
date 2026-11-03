//! NV12 → `CVPixelBuffer`（macOS）。
//!
//! GPUI 的 `surface()` 只吃 `CVPixelBuffer`，且有两条硬要求（spike 从源码确认）：
//! 1. 必须是 **NV12 双平面**（`420YpCbCr8BiPlanarFullRange`），否则会被 `assert_eq!` 跳过；
//! 2. 必须 **IOSurface-backed**，否则 `CVMetalTextureCache` 建不出纹理。

use std::ptr::{self, NonNull};

use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVPixelBufferIOSurfacePropertiesKey,
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};

use crate::convert::Nv12Frame;

/// 把一帧 NV12 填进新建的 IOSurface-backed `CVPixelBuffer`。
pub fn nv12_to_pixel_buffer(frame: &Nv12Frame) -> CFRetained<CVPixelBuffer> {
    let width = frame.width as usize;
    let height = frame.height as usize;

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

        // ---- 平面 0：Y ----
        let y_base = CVPixelBufferGetBaseAddressOfPlane(&buffer, 0).cast::<u8>();
        let y_stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 0);
        let y_w = CVPixelBufferGetWidthOfPlane(&buffer, 0);
        let y_h = CVPixelBufferGetHeightOfPlane(&buffer, 0);
        for row in 0..y_h.min(height) {
            let src = &frame.y[row * width..(row + 1) * width];
            let dst = y_base.add(row * y_stride);
            ptr::copy_nonoverlapping(src.as_ptr(), dst, y_w.min(width));
        }

        // ---- 平面 1：UV（交错，半分辨率）----
        let uv_base = CVPixelBufferGetBaseAddressOfPlane(&buffer, 1).cast::<u8>();
        let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 1);
        let uv_w = CVPixelBufferGetWidthOfPlane(&buffer, 1);
        let uv_h = CVPixelBufferGetHeightOfPlane(&buffer, 1);
        let src_stride_bytes = uv_w * 2;
        for row in 0..uv_h.min(frame.uv.len() / src_stride_bytes.max(1)) {
            let src = &frame.uv[row * src_stride_bytes..(row + 1) * src_stride_bytes];
            let dst = uv_base.add(row * uv_stride);
            ptr::copy_nonoverlapping(src.as_ptr(), dst, src_stride_bytes.min(uv_stride));
        }

        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
    }

    buffer
}
