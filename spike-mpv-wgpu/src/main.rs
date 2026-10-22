//! mpv-wgpu + wgpu-external-frame 零拷贝视频渲染验证
//!
//! 架构（macOS）：
//!   CVPixelBuffer (IOSurface-backed, RGBA)     ← 视频解码器的原生输出形态
//!      ↓ wgpu_external_frame::io_surface        ← 零拷贝：Metal 直接别名这块显存
//!   wgpu::Texture (Rgba8Unorm)
//!      ↓ mpv_wgpu::Renderer                     ← 色彩/缩放/tone mapping（vo=gpu 的 picture chain）
//!   surface texture → 屏幕
//!
//! 第一阶段用**手工构造**的 IOSurface，把变量收敛到「这条零拷贝链路是否成立」，
//! 与解码器无关；跑通后把来源换成 VideoToolbox 的 CVPixelBuffer 即可。

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use mpv_wgpu::{
    ChromaSiting, Coefficients, Draw, Encoding, Equalizer, Levels, Picture, PixelRect, Plane,
    PlaneBits, PlaneSource, QuarterTurn, Renderer, Transfer,
};
use wgpu_external_frame::io_surface::{PackedFormat, PackedIoSurfaceFrame};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// 窗口默认尺寸。视频是 4K，窗口越大缩放比越小、锯齿越轻。
/// 用 SPIKE_SIZE=WxH 覆盖，例如在 5K 屏上测 1:1：SPIKE_SIZE=3840x2160
fn window_size() -> (u32, u32) {
    if let Ok(v) = std::env::var("SPIKE_SIZE") {
        if let Some((w, h)) = v.split_once(['x', 'X']) {
            if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                return (w.max(64), h.max(64));
            }
        }
    }
    (1920, 1080)
}

/// 设为 0 表示一直播放到文件结束或手动关窗；非 0 则渲染该帧数后打印统计并退出。
/// 可用环境变量 SPIKE_MAX_FRAMES 覆盖（便于自动化测试）。
/// SPIKE_NO_THROTTLE=1 时不按帧率 sleep，用于测裸渲染能力
fn no_throttle() -> bool {
    std::env::var("SPIKE_NO_THROTTLE").is_ok()
}

fn max_frames() -> u32 {
    std::env::var("SPIKE_MAX_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

const VIDEO_PATH: &str =
    "/Volumes/Bangumi/凡人修仙传/#凡人修仙传 #4K  第 179 集 @douman.mp4";

/// 按原始宽高比把视频适配进窗口，居中，多余部分留黑边。
/// 返回 (x, y, width, height)，单位是目标纹理的像素。
///
/// 窗口比例被系统取整后很难与视频比例**精确**一致（macOS 的
/// `contentAspectRatio` 允许约 1% 偏差），这会换来几像素的细黑边。
/// 这种亚像素级差异直接撑满更好：1% 的拉伸肉眼不可见，细黑边却看得出来。
fn fit_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32, u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w, dst_h);
    }
    let src_aspect = src_w as f64 / src_h as f64;
    let dst_aspect = dst_w as f64 / dst_h as f64;

    let rect = if dst_aspect > src_aspect {
        // 窗口比视频更宽：高度撑满，左右留黑边（pillarbox）
        let h = dst_h;
        let w = ((dst_h as f64 * src_aspect).round() as u32).clamp(1, dst_w);
        ((dst_w - w) / 2, 0, w, h)
    } else {
        // 窗口比视频更高：宽度撑满，上下留黑边（letterbox）
        let w = dst_w;
        let h = ((dst_w as f64 / src_aspect).round() as u32).clamp(1, dst_h);
        (0, (dst_h - h) / 2, w, h)
    };

    let (x, y, w, h) = rect;
    let slack_x = (dst_w - w) as f64 / dst_w as f64;
    let slack_y = (dst_h - h) as f64 / dst_h as f64;
    // 比例差异在 2% 以内就当作「窗口已经锁定为视频比例」，直接铺满
    if slack_x <= 0.02 && slack_y <= 0.02 {
        return (0, 0, dst_w, dst_h);
    }
    let _ = (x, y);
    rect
}

// ---------------------------------------------------------------- 解码器
//
// 用 AVFoundation 的 AVAssetReader 完成「MP4 解复用 + VideoToolbox 硬解 +
// IOSurface 承载」三件事，零系统依赖，输出正是零拷贝导入需要的 CVPixelBuffer。

mod decoder {
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAsset, AVAssetReader, AVAssetReaderTrackOutput, AVMediaTypeAudio, AVMediaTypeVideo,
    };
    use objc2_core_foundation::{CFRetained, CFType};
    use objc2_core_media::CMSampleBuffer;
    use objc2_core_video::CVPixelBuffer;
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
    use std::ptr::NonNull;

    /// 'BGRA'：macOS 上唯一实用且能创建成功的 32 位格式
    const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = 0x4247_5241;

    pub struct Decoder {
        reader: objc2::rc::Retained<AVAssetReader>,
        video: objc2::rc::Retained<AVAssetReaderTrackOutput>,
        /// 音频轨输出。outputSettings 传 nil，让它原样吐出压缩音频包（AAC）——
        /// 这正是 `AVSampleBufferAudioRenderer` 想要的输入，省掉整条 PCM 转换链。
        audio: Option<objc2::rc::Retained<AVAssetReaderTrackOutput>>,
        frame_rate: f32,
        natural_size: (f64, f64),
    }

    impl Decoder {
        pub fn open(path: &str) -> Result<Self, String> {
            unsafe {
                let ns_path = NSString::from_str(path);
                let url = NSURL::fileURLWithPath(&ns_path);
                let asset = AVAsset::assetWithURL(&url);

                let media_type = AVMediaTypeVideo.ok_or("AVMediaTypeVideo 缺失")?;
                let tracks = asset.tracksWithMediaType(media_type);
                let track = tracks.firstObject().ok_or("没有视频轨")?;
                let frame_rate = track.nominalFrameRate();
                let (tw, th) = (track.naturalSize().width, track.naturalSize().height);
                println!(
                    "[spike] 视频轨: {tw}x{th} @ {frame_rate:.3} fps, 时长 {:.1}s",
                    track.timeRange().duration.seconds()
                );

                // 输出 BGRA + IOSurface 承载（零拷贝前提）。
                // key 用字符串字面量，避开 CFString↔NSString 的桥接：
                //   kCVPixelBufferPixelFormatTypeKey  = "PixelFormatType"
                //   kCVPixelBufferIOSurfacePropertiesKey = "IOSurfaceProperties"
                let key_format = NSString::from_str("PixelFormatType");
                let key_iosurface = NSString::from_str("IOSurfaceProperties");
                let empty = NSDictionary::<NSString, AnyObject>::from_slices(
                    &[] as &[&NSString],
                    &[] as &[&AnyObject],
                );
                let bgra = NSNumber::new_u32(K_CV_PIXEL_FORMAT_TYPE_32_BGRA);
                let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                    &[&*key_format, &*key_iosurface],
                    &[&*bgra, &*empty],
                );

                let reader = AVAssetReader::assetReaderWithAsset_error(&asset)
                    .map_err(|e| format!("AVAssetReader: {e:?}"))?;
                let video =
                    AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                        &track,
                        Some(&settings),
                    );
                reader.addOutput(&video);

                // 音频轨：挂上，但**不设** outputSettings —— 让 reader 原样输出压缩包
                let audio = {
                    let audio_type = AVMediaTypeAudio.ok_or("AVMediaTypeAudio 缺失")?;
                    let tracks = asset.tracksWithMediaType(audio_type);
                    tracks.firstObject().map(|atrack| {
                        let out =
                            AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                                &atrack,
                                None,
                            );
                        reader.addOutput(&out);
                        println!(
                            "[spike] 音频轨已挂载（压缩包直通 → AVSampleBufferAudioRenderer），时长 {:.1}s",
                            atrack.timeRange().duration.seconds()
                        );
                        out
                    })
                };
                if audio.is_none() {
                    println!("[spike] ⚠️ 没有音频轨，这一段将无声播放");
                }

                if !reader.startReading() {
                    return Err("startReading 返回 false".into());
                }
                Ok(Self { reader, video, audio, frame_rate, natural_size: (tw, th) })
            }
        }

        pub fn frame_rate(&self) -> f32 {
            self.frame_rate
        }

        pub fn natural_size(&self) -> (f64, f64) {
            self.natural_size
        }

        /// 取下一位视频帧，**同时给出它的显示时刻（秒）**。
        ///
        /// 之前是按「第几帧 ÷ 标称帧率」推算时间，只对恒定帧率成立；
        /// 片源一有 VFR 就会累积漂移。用 PTS 才是真正的播放时钟。
        pub fn next_video(&self) -> Option<(CFRetained<CVPixelBuffer>, f64)> {
            unsafe {
                let sample = self.video.copyNextSampleBuffer()?;
                let pts = sample.presentation_time_stamp().seconds();
                let image = sample.image_buffer()?;
                // CVImageBuffer 与 CVPixelBuffer 是 toll-free bridged；
                // retain 一次把所有权交给返回值（image 自身的引用照常释放）。
                let ptr = CFRetained::as_ptr(&image).cast::<CVPixelBuffer>();
                Some((CFRetained::retain(ptr), pts))
            }
        }

        /// 取下一位音频包。保持压缩格式，交给系统渲染器解码播放。
        pub fn next_audio(&self) -> Option<objc2::rc::Retained<CMSampleBuffer>> {
            unsafe { self.audio.as_ref()?.copyNextSampleBuffer() }
        }

        /// 这个片源有没有音频
        pub fn has_audio(&self) -> bool {
            self.audio.is_some()
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe { self.reader.cancelReading() };
        }
    }

    // 让编译器确认这些导入都被用到（避免 unused 警告变成噪音）
    #[allow(dead_code)]
    fn _assert_types(_: &CFType, _: NonNull<u8>) {}
}

/// 音频输出：把压缩音频包交给系统渲染器，并借它所在的时间轴当**主时钟**。
///
/// 这是 Apple 为「自己解复用」的播放器准备的组合拳：
///   · `AVSampleBufferAudioRenderer` —— 直接吃 `CMSampleBuffer`，自己解码、自己播放
///   · `AVSampleBufferRenderSynchronizer` —— 维护一条统一时间轴，
///     `currentTime()` 就是「现在播到第几秒」
///
/// 最大的好处是**不用自己实现音频时钟**。真要自己做，得数已播放样本数、
/// 补偿设备输出延迟、处理欠载重启 —— 全是坑。这里视频只要问同步器「现在几点」，
/// 把每一帧对齐过去，音视频天然不会漂。
mod audio {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_av_foundation::{
        AVQueuedSampleBufferRendering, AVSampleBufferAudioRenderer,
        AVSampleBufferRenderSynchronizer,
    };
    use objc2_core_media::{CMSampleBuffer, CMTime};

    pub struct AudioOutput {
        renderer: Retained<AVSampleBufferAudioRenderer>,
        synchronizer: Retained<AVSampleBufferRenderSynchronizer>,
    }

    impl AudioOutput {
        pub fn new() -> Self {
            unsafe {
                let renderer = AVSampleBufferAudioRenderer::new();
                let synchronizer = AVSampleBufferRenderSynchronizer::new();
                synchronizer.addRenderer(ProtocolObject::from_ref(&*renderer));
                Self { renderer, synchronizer }
            }
        }

        /// 缓冲区没满才能继续喂；返回 false 说明解码已经跑到播放前面了
        pub fn is_ready(&self) -> bool {
            unsafe { self.renderer.isReadyForMoreMediaData() }
        }

        pub fn enqueue(&self, sample: &CMSampleBuffer) {
            unsafe { self.renderer.enqueueSampleBuffer(sample) }
        }

        /// 启动播放：时间轴从 0 秒起，速率 1.0
        pub fn play(&self) {
            unsafe { self.synchronizer.setRate_time(1.0, CMTime::new(0, 1)) }
        }

        /// 主时钟（秒）。视频的每一帧都对齐到它。
        pub fn current_time(&self) -> f64 {
            unsafe { self.synchronizer.currentTime().seconds() }
        }
    }
}

/// IOSurface 的 CoreVideo 格式码 'BGRA'。
/// 注：macOS 能创建 'BGRA'，但**不支持创建 'RGBA'**（返回 kCVReturnInvalidPixelFormat -6680）。
const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = 0x4247_5241;

// ---------------------------------------------------------------- 合成一帧

/// 构造一帧 RGBA 测试图像，**带 IOSurface 属性**（零拷贝导入的前提）。
///
/// 画面：横向红渐变 + 纵向绿渐变，左上角蓝块、右下角白块 → 用于确认方向与通道顺序。
fn make_rgba_frame(width: usize, height: usize) -> objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer> {
    use objc2_core_foundation::{CFDictionary, CFType};
    use objc2_core_video::{
        kCVPixelBufferIOSurfacePropertiesKey, CVPixelBuffer, CVPixelBufferCreate,
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };

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
            K_CV_PIXEL_FORMAT_TYPE_32_BGRA,
            Some(attributes.as_opaque()),
            NonNull::from(&mut out),
        )
    };
    assert_eq!(status, 0, "CVPixelBufferCreate 失败 status={status}");
    let buffer = unsafe { objc2_core_foundation::CFRetained::from_raw(NonNull::new(out).unwrap()) };

    unsafe {
        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
        let base = CVPixelBufferGetBaseAddress(&buffer) as *mut u8;
        let stride = CVPixelBufferGetBytesPerRow(&buffer);
        let box_size = 80.min(width / 8).min(height / 8);
        for y in 0..height {
            for x in 0..width {
                let off = y * stride + x * 4;
                let mut r = (x * 255 / width.max(1)) as u8;
                let mut g = (y * 255 / height.max(1)) as u8;
                let mut b = 60u8;
                if x < box_size && y < box_size {
                    r = 0; g = 80; b = 255;          // 左上：蓝
                } else if x + box_size >= width && y + box_size >= height {
                    r = 255; g = 255; b = 255;       // 右下：白
                }
                // BGRA 内存布局（macOS 上的实际布局）
                *base.add(off) = b;
                *base.add(off + 1) = g;
                *base.add(off + 2) = r;
                *base.add(off + 3) = 255;
            }
        }
        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
    }
    buffer
}

// ---------------------------------------------------------------- 应用状态

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// mpv-wgpu 的渲染目标（Rgba8Unorm），再 blit 到 surface
    intermediate: wgpu::Texture,
    intermediate_view: wgpu::TextureView,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
}

/// mpv-wgpu 的渲染目标（Rgba8Unorm）。窗口尺寸变化时需要重建。
fn create_intermediate(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("intermediate-rgba8"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    (tex, view)
}


/// 全屏三角形 blit：把中间纹理（Rgba8Unorm）搬到 surface。
/// 通道顺序由 wgpu 在写入 Bgra8Unorm 目标时自动处理。
const BLIT_WGSL: &str = r#"
struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VertexOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VertexOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    // NDC → uv；y 轴翻转到纹理坐标（左上原点）
    out.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    // 用带过滤的采样，缩放到窗口尺寸时才有正确的降采样
    return textureSample(src_tex, src_samp, in.uv);
}
"#;

/// 一次「推进播放」的结果。
///
/// 播放节奏现在由**媒体时钟**（音频时间轴）驱动，而不是墙上时钟，
/// 所以「还要等多久」是每轮算出来的，不能再用「每帧固定 1/fps」那一套。
enum Tick {
    /// 还没到该帧的显示时刻，等这么久再来看
    Wait(std::time::Duration),
    /// 渲染了一帧
    Rendered,
    /// 视频放完了
    Eof,
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    renderer: Option<Renderer>,
    /// 导入的零拷贝纹理（按住它才能保证底下的 IOSurface 不被回收）
    imported: Option<wgpu::Texture>,
    /// 持有 CVPixelBuffer，生命周期必须长于 imported 纹理
    _frame_owner: Option<objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>>,
    decoder: Option<decoder::Decoder>,
    /// 音频输出；它所在的同步器同时充当**主时钟**
    audio: Option<audio::AudioOutput>,
    /// 已解码但还没到显示时刻的帧：帧本体 + 它的 PTS（秒）。
    /// 提前解一帧出来，才能在「等时钟」的间隙立刻出画。
    pending_video: Option<(
        objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
        f64,
    )>,
    /// 音频是否已解码到尾
    audio_eof: bool,
    /// 时间轴是否已启动（启动前 currentTime 恒为 0）
    clock_started: bool,
    /// 无音频时退化为墙上时钟，这是它的原点
    media_origin: Option<std::time::Instant>,
    frames_rendered: u32,
    started_at: Option<std::time::Instant>,
    /// 下一帧允许渲染的时刻（时间门控，不依赖事件循环调度）
    next_frame_at: Option<std::time::Instant>,
    /// 拖拽期间只记录目标尺寸；等尺寸稳定后再重建 GPU 资源，
    /// 否则每个 Resized 事件都重建 swapchain 与中间纹理会把 GPU 拖垮。
    /// NSWindow.delegate 是弱引用，必须自己拿着，否则会被释放
    #[cfg(target_os = "macos")]
    aspect_delegate: Option<objc2::rc::Retained<aspect::AspectRatioDelegate>>,
    pending_size: Option<(u32, u32)>,
    last_resize_at: Option<std::time::Instant>,
    done: bool,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            gpu: None,
            renderer: None,
            imported: None,
            _frame_owner: None,
            decoder: None,
            audio: None,
            pending_video: None,
            audio_eof: false,
            clock_started: false,
            media_origin: None,
            frames_rendered: 0,
            started_at: None,
            next_frame_at: None,
            #[cfg(target_os = "macos")]
            aspect_delegate: None,
            pending_size: None,
            last_resize_at: None,
            done: false,
        }
    }

    fn init_gpu(&mut self, window: Arc<Window>) {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("create_surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: Default::default(),
        }))
        .expect("request_adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mpv-wgpu-spike"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: Default::default(),
            trace: wgpu::Trace::Off,
            experimental_features: Default::default(),
        }))
        .expect("request_device");

        let caps = surface.get_capabilities(&adapter);
        // mpv-wgpu 的 Encoding::Gamma8 要求 Rgba8Unorm 目标
        // mpv-wgpu 的 Gamma8 要求渲染目标是 Rgba8Unorm，但 macOS surface 不提供该格式；
        // 因此渲染到中间 Rgba8Unorm 纹理，再 blit 到 surface。
        // surface 选非 sRGB 的 Bgra8Unorm：Gamma8 输出已是编码值，不能再套一次 sRGB。
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
            .unwrap_or(caps.formats[0]);
        println!("[spike] 后端 = {:?}", adapter.get_info().backend);
        println!("[spike] surface 支持的格式: {:?}", caps.formats);
        println!("[spike] 选用格式 = {format:?}");

        // 拖拽窗口时 FIFO 的 vsync 等待会把主线程拖住，优先选不阻塞的模式。
        println!("[spike] surface 支持的 present mode: {:?}", caps.present_modes);
        let present_mode = if caps
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else if caps
            .present_modes
            .contains(&wgpu::PresentMode::AutoNoVsync)
        {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::Fifo
        };
        println!("[spike] 选用 present mode = {present_mode:?}");

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&device, &config);

        // 中间目标：mpv-wgpu 的 Gamma8 只能写 Rgba8Unorm
        let (intermediate, intermediate_view) =
            create_intermediate(&device, config.width, config.height);

        // blit 管线
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit-pl"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit-pipeline"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        self.renderer = Some(Renderer::new(&device).expect("Renderer::new"));
        match decoder::Decoder::open(VIDEO_PATH) {
            Ok(d) => {
                println!("[spike] ✅ 解码器已打开: {VIDEO_PATH}");
                let (vw, vh) = d.natural_size();
                let has_audio = d.has_audio();
                self.decoder = Some(d);
                if has_audio {
                    // 音频渲染器 + 统一的播放时间轴；它的 currentTime() 就是主时钟
                    self.audio = Some(audio::AudioOutput::new());
                }
                if let Some(w) = self.window.as_ref() {
                    // 先把窗口尺寸校正到视频比例。
                    let cur = w.inner_size();
                    let target_h = (cur.width as f64 * vh / vw).round().max(1.0) as u32;
                    println!(
                        "[spike] 校正窗口: {}x{} → {}x{}",
                        cur.width, cur.height, cur.width, target_h
                    );
                    let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(
                        cur.width, target_h,
                    ));
                    // 安装 delegate 锁比例（尺寸校正之后）
                    #[cfg(target_os = "macos")]
                    {
                        self.aspect_delegate = lock_aspect_ratio(w, vw, vh);
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = lock_aspect_ratio(w, vw, vh);
                    }
                }
            }
            Err(e) => {
                eprintln!("[spike] ❌ 解码器打开失败: {e}");
                std::process::exit(1);
            }
        }
        self.gpu = Some(Gpu {
            device, queue, surface, config,
            intermediate, intermediate_view,
            blit_pipeline, blit_layout, blit_sampler,
        });
    }

    /// 取一帧解码结果 → IOSurface → 零拷贝导入为 wgpu 纹理。
    ///
    /// 关键：用 `with_owner` 把 CVPixelBuffer 的生命周期交给 wgpu ——
    /// 纹理及其所有提交都完成后，owner 才会被释放，解码器才能安全回收这一帧。
    /// 没有这一步会撕裂或提前释放。
    /// 注意：`with_owner` 要求 owner 是 Send，而 CFRetained<CVPixelBuffer> 不是，
    /// 因此改由 App 持有当前帧，保证它活过这一帧的 GPU 使用。
    fn import_frame(
        device: &wgpu::Device,
        frame: &objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    ) -> Option<wgpu::Texture> {
        use objc2_core_foundation::CFRetained;
        use objc2_core_video::CVPixelBufferGetIOSurface;
        use objc2_io_surface::IOSurfaceRef;

        let ios: CFRetained<IOSurfaceRef> = CVPixelBufferGetIOSurface(Some(frame))?;
        let surface_ptr: NonNull<c_void> = CFRetained::as_ptr(&ios).cast::<c_void>();

        let ios_frame = unsafe { PackedIoSurfaceFrame::retain(surface_ptr, PackedFormat::Bgra8) };
        Some(ios_frame.import(device))
    }

    /// 尺寸是否仍在变化（拖拽进行中）。
    ///
    /// 拖拽期间我们主动跳过解码与渲染，让窗口系统直接拉伸上一帧 ——
    /// 系统播放器就是这么做的。否则每帧的 present 会和窗口合成抢时机，
    /// 主线程被拖住，手感就发滞。
    fn resize_in_flight(&self) -> bool {
        const SETTLE_MS: u128 = 120;
        self.pending_size.is_some()
            && self
                .last_resize_at
                .map_or(false, |t| t.elapsed().as_millis() < SETTLE_MS)
    }

    /// 尺寸稳定后才重建 surface 与中间纹理。
    ///
    /// 拖拽过程中 `Resized` 会高频触发，这里用一个静默期（SETTLE_MS）过滤掉抖动：
    /// 期间沿用旧渲染目标，由系统把画面拉伸到新窗口，拖完再重建一次。
    fn apply_pending_resize(&mut self) {
        const SETTLE_MS: u128 = 120;
        let Some((w, h)) = self.pending_size else {
            return;
        };
        let settled = self
            .last_resize_at
            .map_or(true, |t| t.elapsed().as_millis() >= SETTLE_MS);
        if !settled {
            return;
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        if gpu.config.width == w && gpu.config.height == h {
            self.pending_size = None;
            return;
        }
        gpu.config.width = w;
        gpu.config.height = h;
        // 只重配 swapchain；中间纹理绑定的是视频尺寸，这里一个字节都不用动
        gpu.surface.configure(&gpu.device, &gpu.config);
        self.pending_size = None;
    }

    /// 尽量把音频渲染器的缓冲喂满。
    ///
    /// 音频必须**跑在视频前面**：它要提前若干秒的数据才能连续出声，
    /// 而视频只需要提前一帧。所以每一轮都优先补音频。
    fn pump_audio(&mut self) {
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        if self.audio_eof {
            return;
        }
        let mut fed = 0u32;
        // fed 上限只是防御性的，正常情况下 is_ready() 会先变 false
        while fed < 256 && audio.is_ready() {
            let Some(sample) = self.decoder.as_ref().and_then(|d| d.next_audio()) else {
                self.audio_eof = true;
                println!("[spike] 音频解码到尾");
                break;
            };
            audio.enqueue(&sample);
            fed += 1;
        }
    }

    /// 推进一次播放。节奏完全由**媒体时钟**决定。
    fn tick(&mut self) -> Tick {
        // ① 音频优先：把缓冲填满
        self.pump_audio();

        // ② 启动时间轴。
        //
        // ⚠️ 这里踩过一个死锁：一开始拿 `is_ready()` 当启动前提（「等缓冲够了再走」），
        //    但 prefill 填满之后 is_ready() 必然变成 false，而时间轴不启动、
        //    缓冲就永远不会被消耗 —— 双方互等，一帧都出不来。
        //    正确顺序是：上一句 pump_audio() 已经把缓冲填满（或解码到头），直接 play。
        if !self.clock_started {
            match self.audio.as_ref() {
                Some(a) => {
                    a.play();
                    println!("[spike] ▶️ 主时钟已启动（音频时间轴作为基准）");
                }
                None => println!("[spike] ▶️ 主时钟已启动（无音频，退化为墙上时钟）"),
            }
            self.clock_started = true;
        }

        // ③ 没有待显示的帧就提前解一帧（提前量 = 1 帧）
        if self.pending_video.is_none() {
            let Some((frame, pts)) = self.decoder.as_ref().and_then(|d| d.next_video()) else {
                return Tick::Eof;
            };
            if self.media_origin.is_none() {
                // 无音频时的墙上时钟原点：让首帧 PTS 对应「现在」
                self.media_origin = Some(
                    std::time::Instant::now() - std::time::Duration::from_secs_f64(pts.max(0.0)),
                );
                println!("[spike] 视频首帧 PTS = {pts:.3}s");
            }
            self.pending_video = Some((frame, pts));
        }

        // ④ 媒体时钟走到哪了：有音频就听音频时间轴，没有就用墙上时钟
        let now_media = match self.audio.as_ref() {
            Some(a) => a.current_time(),
            None => self
                .media_origin
                .map(|o| o.elapsed().as_secs_f64())
                .unwrap_or(0.0),
        };

        // ⑤ 到这一帧的显示时刻了吗
        let pts = self.pending_video.as_ref().map_or(0.0, |(_, p)| *p);
        let remaining = pts - now_media;
        if remaining > 0.002 {
            // 上限 50ms：万一音频时钟停住（欠载、切设备），不至于睡死
            return Tick::Wait(std::time::Duration::from_secs_f64(remaining.min(0.05)));
        }
        if self.frames_rendered % 100 == 0 {
            println!(
                "[clock] 已渲染 {:>4} 帧 | 媒体时钟 {now_media:>8.3}s | 该帧 PTS {pts:>8.3}s | 滞后 {:.1}ms",
                self.frames_rendered,
                -remaining * 1000.0
            );
        }

        // 到点了，出画
        let (frame, _) = self.pending_video.take().expect("上面刚确认过存在");
        if !self.render_frame(frame) {
            return Tick::Eof;
        }
        Tick::Rendered
    }

    /// 把一帧画到屏幕上：零拷贝导入 → mpv-wgpu → 等比 blit。
    fn render_frame(
        &mut self,
        decoded: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    ) -> bool {
        // 持有这一帧（同时释放上一帧），保证底下的 IOSurface 活过本帧的 GPU 使用
        let _previous = self._frame_owner.replace(decoded);

        let Some(gpu) = self.gpu.as_mut() else {
            return false;
        };
        let Some(frame_ref) = self._frame_owner.as_ref() else {
            return false;
        };

        // ② 零拷贝导入
        let Some(texture) = Self::import_frame(&gpu.device, frame_ref) else {
            eprintln!("[spike] ❌ 零拷贝导入失败");
            return false;
        };
        let imported_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (src_w, src_h) = (texture.width(), texture.height());

        // ③ 中间纹理跟随**视频尺寸**，不跟窗口走。
        //    窗口怎么缩放都不需要重新分配显存 —— 只改下面 blit 的目标矩形。
        if gpu.intermediate.width() != src_w || gpu.intermediate.height() != src_h {
            let (tex, view) = create_intermediate(&gpu.device, src_w, src_h);
            gpu.intermediate = tex;
            gpu.intermediate_view = view;
            println!("[spike] 中间纹理对齐视频尺寸 {src_w}x{src_h}");
        }

        // ④ 取 surface
        let surface_frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Occluded => None,
            other => {
                eprintln!("[spike] get_current_texture 未成功: {other:?}");
                return false;
            }
        };
        let (out_w, out_h) = (gpu.config.width, gpu.config.height);

        // ⑤ mpv-wgpu 1:1 渲染进中间纹理（无缩放，色彩/tone mapping 照做）
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        let draw = Draw {
            picture: Picture::Rgba(Plane {
                width: src_w,
                height: src_h,
                bits: PlaneBits::Eight,
                source: PlaneSource::Texture(&imported_view),
            }),
            matrix: Coefficients::Bt709,
            levels: Levels::Limited,
            transfer: Transfer::Srgb,
            siting: ChromaSiting::Center,
            peak_nits: 100.0,
            dest: PixelRect { x: 0, y: 0, width: src_w, height: src_h },
            rotation: QuarterTurn::D0,
            equalizer: Equalizer::default(),
            overlays: &[],
            target: &gpu.intermediate,
            encoding: Encoding::Gamma8,
            target_peak_nits: 100.0,
        };
        if let Err(e) = renderer.draw(&gpu.device, &gpu.queue, draw) {
            eprintln!("[spike] ❌ Renderer::draw 失败: {e}");
            return false;
        }

        // ⑥ blit 到 surface：缩放 + 等比居中，多余部分留黑边。
        //    黑边由清屏色给出，画面只画在 viewport 内 —— 中间纹理完全不用动。
        if let Some(frame) = surface_frame {
            let (dx, dy, dw, dh) = fit_rect(src_w, src_h, out_w, out_h);
            let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit-bg"),
                layout: &gpu.blit_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&gpu.intermediate_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.blit_sampler),
                    },
                ],
            });
            let surface_view = frame.texture.create_view(&Default::default());
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("blit-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &surface_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_viewport(dx as f32, dy as f32, dw as f32, dh as f32, 0.0, 1.0);
                pass.set_pipeline(&gpu.blit_pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            gpu.queue.submit(Some(encoder.finish()));
            gpu.queue.present(frame);
        }

        self.frames_rendered += 1;
        true
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let (win_w, win_h) = window_size();
        let attrs = Window::default_attributes()
            .with_title("mpv-wgpu 4K 零拷贝")
            .with_inner_size(winit::dpi::PhysicalSize::new(win_w, win_h));
        let window = Arc::new(event_loop.create_window(attrs).expect("create_window"));
        self.window = Some(window.clone());
        window.focus_window();
        self.init_gpu(window);
        // 渲染一帧后退出（便于无人值守验证）
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 用事件循环的等待超时来节流，而不是 `thread::sleep`。
    ///
    /// `sleep` 会把主线程按住，期间窗口事件（尤其是拖拽产生的 Resized）
    /// 只能排队等 —— 表现就是「拖窗口的手感变成了视频帧率」。
    /// `WaitUntil` 只让循环空转，事件随时能插进来。
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        schedule_next_frame(self, event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if self.started_at.is_none() {
                    self.started_at = Some(std::time::Instant::now());
                }
                // 时间门控：未到下一帧时刻就不推进播放。
                // 只靠 ControlFlow::WaitUntil 不够 —— 窗口系统仍可能高频要求重绘，
                // 在入口拦一道才不会打乱播放节奏。
                if !no_throttle() && !self.resize_in_flight() {
                    if let Some(next) = self.next_frame_at {
                        let now = std::time::Instant::now();
                        if now < next {
                            event_loop.set_control_flow(
                                winit::event_loop::ControlFlow::WaitUntil(next),
                            );
                            return;
                        }
                    }
                }

                self.apply_pending_resize();

                // 拖拽中：画面交给系统拉伸，但事件循环照常转，所以手感不卡
                if self.resize_in_flight() {
                    self.next_frame_at = None;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }

                match self.tick() {
                    Tick::Wait(d) => {
                        // 还没到该帧的显示时刻：只设唤醒时间，**不**请求重绘
                        self.next_frame_at = Some(std::time::Instant::now() + d);
                        return;
                    }
                    Tick::Eof => {
                        println!(
                            "[spike] 播放结束（解码到尾），共渲染 {} 帧",
                            self.frames_rendered
                        );
                        event_loop.exit();
                        return;
                    }
                    Tick::Rendered => {}
                }

                // 刚出过一帧；下一帧该等多久由下一轮 tick 现算
                self.next_frame_at = None;

                let limit = max_frames();
                if limit != 0 && self.frames_rendered >= limit {
                    if let Some(t0) = self.started_at {
                        let el = t0.elapsed();
                        let fps = self.frames_rendered as f64 / el.as_secs_f64().max(1e-6);
                        println!(
                            "[spike] ==== 共渲染 {} 帧 / {:.2?} → 平均 {:.1} fps ====",
                            self.frames_rendered, el, fps
                        );
                    }
                    event_loop.exit();
                    return;
                }

                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    // 立即重配 swapchain，让画面尺寸始终与窗口一致（不靠系统拉伸）
                    let (vw, vh) = self
                        .decoder
                        .as_ref()
                        .map(|d| d.natural_size())
                        .unwrap_or((16.0, 9.0));
                    let (dx, dy, dw, dh) = fit_rect(
                        vw as u32, vh as u32, size.width, size.height);
                    if dx > 0 || dy > 0 {
                        println!(
                            "[spike] resize {}x{} (比例 {:.3}) → 画面 {}x{} @({dx},{dy}) 出现黑边",
                            size.width,
                            size.height,
                            size.width as f64 / size.height as f64,
                            dw,
                            dh
                        );
                    }
                    if let Some(gpu) = self.gpu.as_mut() {
                        gpu.config.width = size.width;
                        gpu.config.height = size.height;
                        gpu.surface.configure(&gpu.device, &gpu.config);
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

/// 窗口缩放代理：系统在拖拽**过程中**调用 `windowWillResize:toSize:`，
/// 我们只负责把尺寸修正回视频比例，**锚点完全由系统维护**。
///
/// 之前用 `request_inner_size` 主动校正的问题就在这里：那个 API 的锚点固定在
/// 左上角，于是拖左上角时右下角会被推着走。交给 delegate 就没这个问题。
///
/// ⚠️ 踩过的坑：这个回调拿到/返回的都是 **frame** 尺寸（含标题栏），
/// 而窗口比例要锁的是 **content** 尺寸。不换算标题栏，content 比例会偏宽
/// 3%~7%，画面两侧就会一直挂着黑边。细节见下面对 `windowWillResize` 的注释。
mod aspect {
    use objc2::rc::Retained;
    use objc2::{define_class, DefinedClass, MainThreadOnly};
    use objc2_app_kit::{NSWindow, NSWindowDelegate};
    use objc2_foundation::{NSObject, NSObjectProtocol, NSSize};
    use std::cell::Cell;

    /// objc2 0.6 的 ivars 必须单独一个类型，并通过 #[ivars = ...] 声明
    pub struct Ivars {
        aspect: Cell<f64>,
    }

    define_class!(
        // SAFETY: NSObject 没有子类化要求，本类型也不实现 Drop
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "YKPiliAspectRatioDelegate"]
        #[ivars = Ivars]
        pub struct AspectRatioDelegate;

        unsafe impl NSObjectProtocol for AspectRatioDelegate {}
        unsafe impl NSWindowDelegate for AspectRatioDelegate {}

        impl AspectRatioDelegate {
            /// 系统在拖拽**过程中**询问目标尺寸；锚点由 AppKit 自己维护，
            /// 所以不会出现「拖左上角、右下角跟着跑」的问题。
            #[unsafe(method(windowWillResize:toSize:))]
            fn window_will_resize(&self, sender: &NSWindow, size: NSSize) -> NSSize {
                let aspect = self.ivars().aspect.get();
                if aspect <= 0.0 || !size.width.is_finite() || size.width <= 0.0 {
                    return size;
                }

                // ⚠️ 这里的 size 是 **frame** 尺寸（含标题栏），不是 content 尺寸。
                // 直接返回 (w, w/aspect) 锁的是窗口外框比例；扣掉标题栏后
                // content 会明显偏「宽」，画面就永远差一条边。
                //
                // 所以要先量出标题栏（frame 高 − content 高），再把它加回去：
                //   content_h = width / aspect
                //   frame_h   = content_h + 标题栏
                // 这样锁定的才是 content 比例 —— 也就是画面比例。
                let frame = sender.frame();
                let content = sender.contentRectForFrameRect(frame);
                let chrome = (frame.size.height - content.size.height).max(0.0);

                NSSize {
                    width: size.width,
                    height: (size.width / aspect + chrome).round(),
                }
            }
        }
    );

    impl AspectRatioDelegate {
        pub fn new(mtm: objc2::MainThreadMarker, aspect: f64) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(Ivars {
                aspect: Cell::new(aspect),
            });
            unsafe { objc2::msg_send![super(this), init] }
        }
    }
}

/// 把窗口的宽高比锁成视频的比例 —— 系统播放器就是这么做的。
///
/// 用 `NSWindowDelegate` 而不是 `contentAspectRatio`：后者在 winit 建出的窗口上
/// 读回正常却不约束拖拽（实测拖拽时比例照样漂）。delegate 是唯一可靠的手段。
#[cfg(target_os = "macos")]
fn lock_aspect_ratio(window: &Window, video_w: f64, video_h: f64) -> Option<objc2::rc::Retained<aspect::AspectRatioDelegate>> {
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return None;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let view_ptr = appkit.ns_view.as_ptr() as *mut NSView;
    if view_ptr.is_null() {
        return None;
    }
    let view: &NSView = unsafe { &*view_ptr };
    let Some(ns_window) = (unsafe { view.window() }) else {
        return None;
    };

    // MainThreadOnly 类型必须由主线程构造；winit 的事件回调本来就在主线程
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        eprintln!("[spike] 不在主线程，跳过宽高比锁定");
        return None;
    };
    let delegate = aspect::AspectRatioDelegate::new(mtm, video_w / video_h);
    unsafe { ns_window.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate))) };
    println!(
        "[spike] 已安装 NSWindowDelegate，锁定比例 {:.4}（{video_w}:{video_h}）",
        video_w / video_h
    );

    // ---- 换算自检 ----
    //
    // 不必肉眼拖窗口：直接把假想的 **frame** 尺寸喂给 delegate，
    // 再对它返回的 frame 扣掉标题栏，看 content 比例是否命中视频比例。
    // 输入故意用 0.6 这种错误比例，被拉回 1.7778 才算通过。
    {
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let target = video_w / video_h;
        println!("[aspect] 换算自检（目标 content 比例 {target:.4}）:");
        for candidate_w in [800.0f64, 1200.0, 1600.0, 2000.0] {
            let proposed = NSSize {
                width: candidate_w,
                height: candidate_w * 0.6,
            };
            let adjusted: NSSize = unsafe {
                objc2::msg_send![&*delegate, windowWillResize: &*ns_window, toSize: proposed]
            };
            let rect = NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: adjusted,
            };
            let content = ns_window.contentRectForFrameRect(rect);
            let got = content.size.width / content.size.height;
            let mark = if (got - target).abs() < 0.005 { "✅" } else { "❌ 仍会留黑边" };
            println!(
                "[aspect]   frame {:.0}×{:.0} → frame {:.0}×{:.0} | content {:.0}×{:.0} = {got:.4} {mark}",
                proposed.width,
                proposed.height,
                adjusted.width,
                adjusted.height,
                content.size.width,
                content.size.height,
            );
        }
    }

    // NSWindow.delegate 是弱引用，必须由调用方持有，否则会被释放
    Some(delegate)
}

#[cfg(not(target_os = "macos"))]
fn lock_aspect_ratio(_window: &Window, _w: f64, _h: f64) -> Option<()> {
    None
}

/// 决定下一次重绘的时刻。
///
/// 播放节奏现在完全由 `tick()` 里的**媒体时钟**决定，这里只负责把
/// 「离该帧的显示时刻还有多久」翻译成事件循环的等待超时：
/// 到点就请求重绘，没到就设 WaitUntil，并且**不**请求重绘。
fn schedule_next_frame(app: &App, event_loop: &ActiveEventLoop) {
    let Some(w) = app.window.as_ref() else {
        return;
    };

    // 测速模式：尽快出帧
    if no_throttle() {
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
        w.request_redraw();
        return;
    }

    match app.next_frame_at {
        Some(next) => {
            if std::time::Instant::now() >= next {
                w.request_redraw();
            } else {
                // 没到点：**只设等待超时，不请求重绘**。
                // 之前这里多了一句 request_redraw()，导致限速完全失效（视频全速播放）。
                event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next));
            }
        }
        // 没有待办时刻（刚出过一帧 / 还没开始）→ 立刻推进一次
        None => w.request_redraw(),
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("EventLoop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run_app");
}
