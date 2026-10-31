//! GPUI 的 HTTP 客户端（给图片加载器用）。
//!
//! **为什么必须有这个文件**：GPUI 默认装的是 `NullHttpClient`，
//! 它的 `send()` 直接 `bail!("No HttpClient available")` ——
//! 所以 `img("https://...")` 永远不会发出请求，且**静默失败**
//! （图片元素保留背景色，不报错）。这正是封面图一直显示占位色块的原因。
//!
//! 换上这个客户端之后，`img()` / 头像 / 一切走 GPUI 资源加载的 URI 都能拉图。
//!
//! 图床（`i0.hdslb.com` 等）的实测结论：
//! - 不带 Referer：**200**（放行）
//! - Referer 属于 bilibili 域：**200**
//! - Referer 是别的站：**403**
//!
//! 所以这里**不主动加 Referer**，避免反而触发 403；只带浏览器 UA。
//!
//! 这一层同时承担**图片磁盘缓存**：URL → 压缩字节，命中后不再访问图床。
//! 解码后的 BGRA 仍由页面图片缓存管理（`page_image_cache.rs`），所以：
//! - 内存：只留可视 + 一行卡片的解码帧；
//! - 硬盘：留压缩图，滚动回来时直接解码，不重复消耗流量。

use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use futures::FutureExt;
use gpui::http_client::http::{HeaderValue, Request, Response};
use gpui::http_client::{AsyncBody, HttpClient, Url};
use gpui::{Image as GpuiImage, ImageFormat};
use yk_core::image_cache::ImageDiskCache;

/// 浏览器 UA。B 站图床对空 UA 也放行，但带上更稳（部分 CDN 会拦）。
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 进程内共享一份图片磁盘缓存，避免每次构造客户端都扫描目录。
static IMAGE_CACHE: OnceLock<Arc<ImageDiskCache>> = OnceLock::new();

/// 设置页共享同一个磁盘缓存实例，拿到统计与清理入口。
pub fn shared_image_cache() -> Arc<ImageDiskCache> {
    image_cache()
}

fn image_cache() -> Arc<ImageDiskCache> {
    IMAGE_CACHE
        .get_or_init(|| Arc::new(ImageDiskCache::default_cache()))
        .clone()
}

pub struct YkHttpClient {
    ua: HeaderValue,
    cache: Arc<ImageDiskCache>,
}

impl YkHttpClient {
    pub fn new() -> Self {
        Self {
            ua: HeaderValue::from_static(UA),
            cache: image_cache(),
        }
    }

    pub fn shared() -> Arc<dyn HttpClient> {
        Arc::new(Self::new())
    }
}

impl Default for YkHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for YkHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.ua)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let uri = req.uri().to_string();
        let cache = self.cache.clone();

        // reqwest 的 blocking 版本要独占线程，不能在 gpui 的执行器上直接跑，
        // 所以丢进独立线程 + oneshot 转成 future。
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = fetch_with_cache(&uri, &cache);
            eprintln!(
                "[img] {} -> {} ({}ms)",
                &uri[uri.len().saturating_sub(48)..],
                match &result {
                    Ok(r) => format!("HTTP {}", r.status()),
                    Err(e) => format!("ERR {e}"),
                },
                started.elapsed().as_millis()
            );
            let _ = tx.send(result);
        });

        async move {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("图片请求线程意外退出")),
            }
        }
        .boxed()
    }
}

fn fetch_with_cache(uri: &str, cache: &ImageDiskCache) -> anyhow::Result<Response<AsyncBody>> {
    if let Some(bytes) = cache.get(uri) {
        eprintln!(
            "[img-cache] hit {} ({} KB)",
            &uri[uri.len().saturating_sub(48)..],
            bytes.len() / 1024
        );
        return response_from_bytes(bytes, Some("application/octet-stream"));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let resp = client.get(uri).header("User-Agent", UA).send()?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut builder = Response::builder().status(status.as_u16());
    for (k, v) in resp.headers() {
        builder = builder.header(k, v);
    }
    let bytes = resp.bytes()?.to_vec();
    let should_cache = resp_status_is_success(status)
        && !bytes.is_empty()
        && content_type
            .as_deref()
            .map(|value| value.starts_with("image/") || value.contains("octet-stream"))
            .unwrap_or(true);
    if should_cache {
        cache.put(uri, &bytes);
    }
    Ok(builder.body(AsyncBody::from_bytes(bytes.into()))?)
}

/// 读取一张图片的原始字节，并识别出 GPUI 能写进剪贴板的格式。
///
/// 优先命中磁盘缓存（和 `img()` 共用同一份），未命中时走一次阻塞请求。
/// 供「复制图片」这类需要原始压缩数据的场景使用；解码后的 `RenderImage`
/// 仍然由 `img()` / 页面图片缓存管理。
pub fn read_image_bytes(uri: &str) -> anyhow::Result<(Vec<u8>, ImageFormat)> {
    let uri = uri.trim();
    if uri.is_empty() {
        anyhow::bail!("图片地址为空");
    }
    let cache = image_cache();
    let bytes = match cache.get(uri) {
        Some(bytes) => bytes,
        None => {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?;
            let resp = client.get(uri).header("User-Agent", UA).send()?;
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("图片请求返回 HTTP {status}");
            }
            let bytes = resp.bytes()?.to_vec();
            if bytes.is_empty() {
                anyhow::bail!("图片内容为空");
            }
            cache.put(uri, &bytes);
            bytes
        }
    };
    let format = sniff_image_format(&bytes)
        .ok_or_else(|| anyhow::anyhow!("无法识别的图片格式（可能是 AVIF）"))?;
    Ok((bytes, format))
}

/// 按文件头识别图片格式。
///
/// B 站图床加 `@480w.webp` 后缀后一般返回 WebP，但原图也可能是 PNG / JPEG / GIF；
/// 剪贴板需要准确的格式标记，不能只看 URL 后缀。
pub fn sniff_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else {
        None
    }
}

/// 把原始图片字节包装成剪贴板条目（由平台层写系统剪贴板）。
pub fn clipboard_image(bytes: Vec<u8>, format: ImageFormat) -> gpui::ClipboardItem {
    let image = GpuiImage::from_bytes(format, bytes);
    gpui::ClipboardItem::new_image(&image)
}

/// 把图片写进系统剪贴板。
///
/// Windows 上优先走 [`crate::win_clipboard`]：GPUI 的 Windows 后端只写注册格式
/// `PNG`，不写 `CF_DIB` —— 微信 / QQ / Word 这些只认 CF_DIB 的程序粘贴出来是空的，
/// 看起来就像「点了复制但没复制上」。补一份 DIB 之后两边都能贴。
/// 其它平台（以及 Windows 上解码失败时）继续走 GPUI 自己的实现。
pub fn copy_image_to_clipboard(cx: &mut gpui::App, bytes: Vec<u8>, format: ImageFormat) {
    #[cfg(target_os = "windows")]
    if crate::win_clipboard::write_image(&bytes, format) {
        return;
    }
    cx.write_to_clipboard(clipboard_image(bytes, format));
}

fn resp_status_is_success(status: reqwest::StatusCode) -> bool {
    status.is_success()
}

fn response_from_bytes(
    bytes: Vec<u8>,
    content_type: Option<&str>,
) -> anyhow::Result<Response<AsyncBody>> {
    let mut builder = Response::builder().status(200);
    if let Some(content_type) = content_type {
        builder = builder.header("Content-Type", content_type);
    }
    Ok(builder.body(AsyncBody::from_bytes(bytes.into()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_image_headers() {
        assert_eq!(
            sniff_image_format(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            sniff_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(sniff_image_format(b"GIF89a...."), Some(ImageFormat::Gif));
        assert_eq!(
            sniff_image_format(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(sniff_image_format(b"BM...."), Some(ImageFormat::Bmp));
        assert_eq!(sniff_image_format(b"not an image"), None);
    }
}
