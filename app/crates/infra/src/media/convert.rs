//! RGBA → NV12（BT.709 full range）转换。
//!
//! libmpv 的 SW 渲染给出 `rgb0`（每像素 4 字节，第 4 字节是垃圾值）；
//! GPUI 的 `surface()` 要的是 **NV12 双平面** `CVPixelBuffer`
//! （spike 已从 GPUI 源码确认：非该格式会被 `assert_eq!` 跳过）。
//!
//! 为什么是 BT.709 而不是 BT.601：B 站视频基本是 HD/4K，709 是它的标准。
//! full range（不是 video range 16–235）—— 我们用
//! `420YpCbCr8BiPlanarFullRange` 创建像素缓冲，用 video range 公式会整体发灰。
//!
//! NV12 布局：
//! ```text
//! plane 0: Y  Y  Y  Y     ← 每像素 1 字节，全分辨率
//! plane 1: U  V  U  V     ← 每 2×2 像素一个 UV 对，半分辨率交错
//! ```

/// 一帧 NV12 数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12Frame {
    pub width: u32,
    pub height: u32,
    /// Y 平面（full 分辨率）。
    pub y: Vec<u8>,
    /// UV 交错平面（`width/2 * height/2 * 2` 字节）。
    pub uv: Vec<u8>,
}

#[inline]
fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// BT.709 full-range 的 Y。
#[inline]
fn luma709(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 把 `rgb0` / RGBA 缓冲区转成 NV12。
///
/// 第 4 字节（alpha 或垃圾值）一律忽略 —— libmpv 的 `rgb0` 第 4 字节是未初始化数据，
/// 当 alpha 用会出现随机透明。
///
/// 尺寸为奇数时，UV 平面按向下取整处理（右/下边缘那一个像素不参与色度采样）。
pub fn rgba_to_nv12(data: &[u8], width: u32, height: u32) -> Nv12Frame {
    let mut frame = Nv12Frame { width, height, y: Vec::new(), uv: Vec::new() };
    rgba_to_nv12_into(&mut frame, data, width, height);
    frame
}

/// 同 [`rgba_to_nv12`]，但写进调用方复用的 `Nv12Frame`。
///
/// 逐帧链路用这个：平面容量够时只覆写像素，不再每帧分配 Y/UV 两个 `Vec`
/// （1080p 约 3 MB/帧，4K 约 12 MB/帧）。尺寸变化时 `resize` 自动扩容。
pub fn rgba_to_nv12_into(out: &mut Nv12Frame, data: &[u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    out.width = width;
    out.height = height;
    out.y.resize(w * h, 0);
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    out.uv.resize(cw * ch * 2, 128);
    let y_plane = &mut out.y;
    let uv_plane = &mut out.uv;

    for row in 0..h {
        for col in 0..w {
            let i = (row * w + col) * 4;
            let r = data.get(i).copied().unwrap_or(0) as f32;
            let g = data.get(i + 1).copied().unwrap_or(0) as f32;
            let b = data.get(i + 2).copied().unwrap_or(0) as f32;
            y_plane[row * w + col] = clamp_u8(luma709(r, g, b));
        }
    }

    // 色度：每个 2×2 块取平均后再算 U/V，比「只取左上像素」平滑，边缘不会有彩边。
    for cy in 0..ch {
        for cx in 0..cw {
            let mut sr = 0.0;
            let mut sg = 0.0;
            let mut sb = 0.0;
            let mut n = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (x, y) = (cx * 2 + dx, cy * 2 + dy);
                    if x >= w || y >= h {
                        continue;
                    }
                    let i = (y * w + x) * 4;
                    sr += data.get(i).copied().unwrap_or(0) as f32;
                    sg += data.get(i + 1).copied().unwrap_or(0) as f32;
                    sb += data.get(i + 2).copied().unwrap_or(0) as f32;
                    n += 1.0;
                }
            }
            if n == 0.0 {
                continue;
            }
            let (r, g, b) = (sr / n, sg / n, sb / n);
            let luma = luma709(r, g, b);
            // full-range BT.709 的色差逆变换
            let u = (b - luma) / 1.8556 + 128.0;
            let v = (r - luma) / 1.5748 + 128.0;
            let o = (cy * cw + cx) * 2;
            uv_plane[o] = clamp_u8(u);
            uv_plane[o + 1] = clamp_u8(v);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8, w: u32, h: u32) -> Vec<u8> {
        (0..(w * h)).flat_map(|_| [r, g, b, 0]).collect()
    }

    #[test]
    fn plane_sizes_are_correct() {
        let f = rgba_to_nv12(&solid(0, 0, 0, 640, 360), 640, 360);
        assert_eq!(f.y.len(), 640 * 360);
        assert_eq!(f.uv.len(), (640 / 2) * (360 / 2) * 2);
    }

    /// 纯黑的 Y 应是 0，U/V 应是 128（无色度）。
    #[test]
    fn black_maps_to_zero_luma_and_neutral_chroma() {
        let f = rgba_to_nv12(&solid(0, 0, 0, 4, 4), 4, 4);
        assert!(f.y.iter().all(|&v| v == 0), "黑应映射到 Y=0，实际 {:?}", &f.y[..4]);
        assert!(f.uv.iter().all(|&v| v == 128), "黑应是中性色度，实际 {:?}", &f.uv[..4]);
    }

    #[test]
    fn white_maps_to_full_luma() {
        let f = rgba_to_nv12(&solid(255, 255, 255, 4, 4), 4, 4);
        assert!(f.y.iter().all(|&v| v == 255), "白应映射到 Y=255，实际 {:?}", &f.y[..4]);
    }

    /// 纯红：BT.709 的 Y = 0.2126*255 ≈ 54，V 应明显高于 U。
    #[test]
    fn red_has_expected_luma_and_chroma_direction() {
        let f = rgba_to_nv12(&solid(255, 0, 0, 4, 4), 4, 4);
        assert!((f.y[0] as i32 - 54).abs() <= 2, "红 Y 应约 54，实际 {}", f.y[0]);
        let u = f.uv[0] as i32;
        let v = f.uv[1] as i32;
        assert!(v > 200, "红的 V 应接近 240，实际 {v}");
        assert!(u < 120, "红的 U 应低于中性 128，实际 {u}");
    }

    #[test]
    fn green_and_blue_chroma_directions() {
        // 注意符号：U(Cb) 高 = 偏蓝，V(Cr) 高 = 偏红。
        // 绿两个色差都低（U≈30, V≈55），这里曾经把期望写反过。
        let g = rgba_to_nv12(&solid(0, 255, 0, 4, 4), 4, 4);
        assert!(g.y[0] > 170, "绿 Y 应偏高，实际 {}", g.y[0]);
        assert!(g.uv[0] < 60, "绿的 U 应偏低，实际 {}", g.uv[0]);
        assert!(g.uv[1] < 80, "绿的 V 应偏低，实际 {}", g.uv[1]);

        let b = rgba_to_nv12(&solid(0, 0, 255, 4, 4), 4, 4);
        assert!(b.y[0] < 40, "蓝 Y 应偏低，实际 {}", b.y[0]);
        assert!(b.uv[0] > 200, "蓝的 U 应偏高，实际 {}", b.uv[0]);
    }

    /// 奇数尺寸不能崩，且平面大小按 ceil 计算。
    #[test]
    fn odd_dimensions_are_safe() {
        let f = rgba_to_nv12(&solid(10, 20, 30, 5, 3), 5, 3);
        assert_eq!(f.y.len(), 15);
        assert_eq!(f.uv.len(), 3 * 2 * 2);
    }

    /// 输入缓冲区短于 `w*h*4` 时不能越界读，只是填 0。
    #[test]
    fn short_buffer_degrades_to_black() {
        let f = rgba_to_nv12(&[1, 2, 3, 4], 4, 4);
        assert_eq!(f.y.len(), 16);
        assert_eq!(f.y[15], 0, "越界部分应为 0");
    }
}
