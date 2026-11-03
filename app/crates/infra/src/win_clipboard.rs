//! Windows 剪贴板写图片：自己补一份 CF_DIB。
//!
//! **为什么不能只用 GPUI 的 `write_to_clipboard(ClipboardItem::new_image(..))`**：
//! gpui-fast-windows 的实现只写注册格式 `PNG` / `JFIF` / `GIF`，**从不写 CF_DIB**。
//! 微信、QQ、Word、资源管理器这类程序粘贴时只认 `CF_DIB`，于是表现成
//! 「点了复制，但剪贴板里什么都没有」——而且它先 `EmptyClipboard()`，
//! 连原来剪贴板里的内容也一起清掉了。
//!
//! 这里用 Win32 直接写：
//! - `CF_DIB`：32 位 BI_RGB、自下而上、alpha 固定 255（很多程序把 alpha=0 当全透明）；
//! - 再额外给一份注册格式 `PNG`，浏览器 / 部分编辑器走这条更快。
//!
//! 解码用 `image` crate：Windows 目标下 gpui-fast-windows 已经把
//! png / jpeg / webp / gif / bmp 几个 feature 打开了（同一版本、feature 取并集）。

use std::ffi::c_void;

use gpui::ImageFormat;

#[repr(C)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(hwnd: *mut c_void) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
    fn CloseClipboard() -> i32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(mem: *mut c_void) -> i32;
    fn GlobalFree(mem: *mut c_void) -> *mut c_void;
}

const CF_DIB: u32 = 8;
const GMEM_MOVEABLE: u32 = 0x0002;

/// 把图片写进剪贴板；成功返回 `true`。
///
/// 失败时调用方退回 GPUI 的路径（至少不会比现在更差）。
pub fn write_image(bytes: &[u8], format: ImageFormat) -> bool {
    let Some(rgba) = decode(bytes, format) else {
        return false;
    };
    let png = encode_png(&rgba);

    if !open_retry() {
        return false;
    }
    let mut ok = false;
    unsafe {
        if EmptyClipboard() != 0 {
            ok = set_clipboard(dib_bytes(&rgba), CF_DIB);
            if let Some(png) = png {
                let registered = register_format("PNG");
                if registered != 0 {
                    // PNG 只是锦上添花：失败不影响 CF_DIB 已经写进去。
                    let _ = set_clipboard(png, registered);
                }
            }
            // 动图再额外给一份原始 GIF：CF_DIB 只能是静态首帧，
            // 支持 GIF 的程序（浏览器 / 部分 IM）还能拿到动画。
            if matches!(format, ImageFormat::Gif) {
                let gif = register_format("GIF");
                if gif != 0 {
                    let _ = set_clipboard(bytes.to_vec(), gif);
                }
            }
        }
        CloseClipboard();
    }
    ok
}

fn decode(bytes: &[u8], format: ImageFormat) -> Option<image::RgbaImage> {
    let format = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        _ => return None,
    };
    image::load_from_memory_with_format(bytes, format)
        .ok()
        .map(|image| image.to_rgba8())
}

fn encode_png(rgba: &image::RgbaImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// 按 CF_DIB 的要求拼 `BITMAPINFOHEADER + 像素`。
///
/// 32 位 BI_RGB：行序自下而上、像素 BGRA。alpha 一律写 255 ——
/// 不少程序把 BI_RGB 的 alpha 当 0 处理，原样写会贴出一张全透明的图。
fn dib_bytes(rgba: &image::RgbaImage) -> Vec<u8> {
    let (width, height) = rgba.dimensions();
    let header = BitmapInfoHeader {
        bi_size: 40,
        bi_width: width as i32,
        bi_height: height as i32,
        bi_planes: 1,
        bi_bit_count: 32,
        bi_compression: 0,
        bi_size_image: width.saturating_mul(height).saturating_mul(4),
        bi_x_pels_per_meter: 0,
        bi_y_pels_per_meter: 0,
        bi_clr_used: 0,
        bi_clr_important: 0,
    };
    let mut out = Vec::with_capacity(40 + header.bi_size_image as usize);
    out.extend_from_slice(&header.bi_size.to_le_bytes());
    out.extend_from_slice(&header.bi_width.to_le_bytes());
    out.extend_from_slice(&header.bi_height.to_le_bytes());
    out.extend_from_slice(&header.bi_planes.to_le_bytes());
    out.extend_from_slice(&header.bi_bit_count.to_le_bytes());
    out.extend_from_slice(&header.bi_compression.to_le_bytes());
    out.extend_from_slice(&header.bi_size_image.to_le_bytes());
    out.extend_from_slice(&header.bi_x_pels_per_meter.to_le_bytes());
    out.extend_from_slice(&header.bi_y_pels_per_meter.to_le_bytes());
    out.extend_from_slice(&header.bi_clr_used.to_le_bytes());
    out.extend_from_slice(&header.bi_clr_important.to_le_bytes());

    let src = rgba.as_raw();
    let stride = width as usize * 4;
    for row in (0..height as usize).rev() {
        let start = row * stride;
        for pixel in src[start..start + stride].chunks_exact(4) {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xFF]);
        }
    }
    out
}

fn open_retry() -> bool {
    // 剪贴板是全局独占资源：别的程序正在用的时候 OpenClipboard 会失败，
    // 短暂重试几次比直接放弃靠谱。
    for _ in 0..10 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn register_format(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
}

/// 写一块全局内存进剪贴板。成功后内存归系统所有，**不能再 Free**。
fn set_clipboard(data: Vec<u8>, format: u32) -> bool {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, data.len());
        if mem.is_null() {
            return false;
        }
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            GlobalFree(mem);
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        GlobalUnlock(mem);
        if SetClipboardData(format, mem).is_null() {
            GlobalFree(mem);
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 图：左上红、右上绿、左下蓝、右下白。
    fn sample() -> image::RgbaImage {
        image::RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, // 左上
                0, 255, 0, 255, // 右上
                0, 0, 255, 255, // 左下
                255, 255, 255, 255, // 右下
            ],
        )
        .unwrap()
    }

    /// 真机验收：写一张 2×2 的 PNG 进剪贴板，再让外部工具看格式。
    ///
    /// 默认 `#[ignore]`：剪贴板是全局资源，CI 上没有交互式桌面时容易失败，
    /// 需要手动跑 `cargo test -p yk-app --lib -- --ignored clipboard`。
    #[test]
    #[ignore]
    fn writes_image_to_system_clipboard() {
        let mut png = Vec::new();
        sample()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let format = crate::http::sniff_image_format(&png).expect("PNG 头识别");
        assert!(write_image(&png, format), "写剪贴板失败");
    }

    #[test]
    fn dib_header_and_pixel_order() {
        let dib = dib_bytes(&sample());
        assert_eq!(dib.len(), 40 + 2 * 2 * 4);
        // 头字段
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(dib[12..14].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
        // 自下而上重排：第一行是原图最下面一行（蓝、白），BGRA 顺序。
        assert_eq!(&dib[40..44], &[255, 0, 0, 0xFF], "首像素应是左下角的蓝");
        assert_eq!(&dib[44..48], &[255, 255, 255, 0xFF], "次像素应是右下角的白");
        assert_eq!(&dib[48..52], &[0, 0, 255, 0xFF], "第三像素应是左上角的红");
        assert_eq!(&dib[52..56], &[0, 255, 0, 0xFF], "第四像素应是右上角的绿");
        // alpha 一律 255：BI_RGB 下很多程序把 0 当全透明。
        assert!(dib[40..].chunks_exact(4).all(|px| px[3] == 0xFF));
    }
}
