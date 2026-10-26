# reference —— API 速查

配合 [`SKILL.md`](./SKILL.md) 使用。这里放**确切的签名、feature 名、迁移对照**，
避免每次重新翻 crate 源码。

---

## 目录

- [1. 依赖与 feature](#1-依赖与-feature)
- [2. wgpu 30 API 迁移对照](#2-wgpu-30-api-迁移对照)
- [3. mpv-wgpu 用法](#3-mpv-wgpu-用法)
- [4. CoreVideo：CVPixelBuffer / IOSurface](#4-corevideocvpixelbuffer--iosurface)
- [5. CoreMedia：CMSampleBuffer / CMTime](#5-coremediacmsamplebuffer--cmtime)
- [6. AVFoundation：解码与音频](#6-avfoundation解码与音频)
- [7. objc2 0.6 写法](#7-objc2-06-写法)
- [8. 编译错误 → 修法](#8-编译错误--修法)
- [9. 环境与工具](#9-环境与工具)

---

## 1. 依赖与 feature

实测可用的 `Cargo.toml`（Rust 2021）：

```toml
[dependencies]
mpv-wgpu = "0.1.0"
wgpu-external-frame = "0.4.0"
wgpu = "30"
winit = "0.30"
pollster = "0.4"
objc2 = "0.6"

objc2-core-foundation = "0.3.2"
objc2-foundation = "0.3.2"
objc2-io-surface = "0.3"

objc2-core-video = { version = "0.3.2", features = [
    "CVPixelBuffer", "CVImageBuffer", "CVBuffer", "CVBase", "CVReturn",
    "CVPixelBufferIOSurface", "objc2-io-surface",
] }

objc2-core-media = { version = "0.3.2", features = ["CMSampleBuffer", "CMTime", "CMBase"] }

objc2-av-foundation = { version = "0.3.2", features = [
    "AVAsset", "AVAssetTrack", "AVAssetReader", "AVAssetReaderOutput", "AVMediaFormat",
    "AVSampleBufferAudioRenderer", "AVSampleBufferRenderSynchronizer",
    "AVQueuedSampleBufferRendering", "objc2-core-media",
] }

objc2-app-kit = { version = "0.3.2", features = ["NSWindow", "NSView", "NSResponder"] }
```

注意：

- `NSWindowDelegate` **没有独立 feature**，随 `NSWindow` 启用
- `AVQueuedSampleBufferRendering` 必须显式开启，否则 `enqueueSampleBuffer` / `isReadyForMoreMediaData` 不可见
- `objc2-io-surface` 要同时出现在 `objc2-core-video` 的 features 里（`"objc2-io-surface"`）和顶层依赖中

---

## 2. wgpu 30 API 迁移对照

| 位置 | wgpu 30 写法 |
|---|---|
| 实例 | `wgpu::Instance::default()` —— `InstanceDescriptor` 不再实现 `Default` |
| 适配器 | `RequestAdapterOptions` 需 `apply_limit_buckets: Default::default()` |
| 设备 | `DeviceDescriptor` 需 `experimental_features: Default::default()` |
| surface 配置 | `SurfaceConfiguration` 需 `color_space` |
| 取帧 | `get_current_texture() -> CurrentSurfaceTexture` 枚举 |
| 呈现 | `queue.present(frame)` —— 不再是 `surface.present()` |
| 管线布局 | `PipelineLayoutDescriptor` 需 `immediate_size: 0` |
| 绑定组布局 | 类型为 `&[Option<&BindGroupLayout>]`，写 `&[Some(&layout)]` |
| 渲染管线 | `RenderPipelineDescriptor` 需 `multiview_mask: None` |
| 渲染通道 | `RenderPassDescriptor` 需 `multiview_mask: None` |
| 颜色附件 | `RenderPassColorAttachment` 需 `depth_slice: None` |

`CurrentSurfaceTexture` 的处理：

```rust
let surface_frame = match gpu.surface.get_current_texture() {
    wgpu::CurrentSurfaceTexture::Success(f) => Some(f),
    wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
    wgpu::CurrentSurfaceTexture::Occluded => None,   // 窗口被遮挡，这帧不用画
    other => { eprintln!("get_current_texture 未成功: {other:?}"); return false; }
};
```

macOS Metal surface 能力（实测）：

```
formats      = [Bgra8UnormSrgb, Bgra8Unorm, Rgba16Float, Rgb10a2Unorm]
present_modes= [Fifo, Immediate]
```

**没有 `Rgba8Unorm`**，也没有 Mailbox / AutoNoVsync。

---

## 3. mpv-wgpu 用法

`Renderer::new(&device)` 创建，然后每帧构造 `Draw`：

```rust
let draw = Draw {
    picture: Picture::Rgba(Plane {
        width: src_w,
        height: src_h,
        bits: PlaneBits::Eight,          // 对 PlaneSource::Texture 会被忽略
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
    target: &intermediate_texture,
    encoding: Encoding::Gamma8,          // ⚠️ 要求 target 是 Rgba8Unorm
    target_peak_nits: 100.0,
};
renderer.draw(&device, &queue, draw)?;
```

要点：

- `Picture::Rgba(Plane)` 单平面；`Picture::Yuv { y, u, v }` 要求 **U/V 分离**的纹理
- `PlaneSource::Texture(&TextureView)` 的格式由纹理本身决定，`Plane.bits` 不参与
- `Encoding::Gamma8` 要求 target 为 `Rgba8Unorm` → macOS 上必须走中间纹理 + blit
- **不要在 blit shader 里对 `Bgra8Unorm` 采样结果做 swizzle**，格式语义已由 wgpu 处理
- 缩放内核在 `mpv-wgpu/src/cubic.rs`，固定 4-tap（CatmullRom 放大 / Hermite 缩小），**无 correct-downscaling**

---

## 4. CoreVideo：CVPixelBuffer / IOSurface

`CVPixelBuffer` 的格式码是四字符码的整数形式：

| 格式 | 四字符码 | `CVPixelBufferCreate` 可用？ |
|---|---|---|
| `'BGRA'` | `0x42475241` | ✅ |
| `'RGBA'` | `0x52474241` | ❌ 返回 **-6680**（`kCVReturnInvalidPixelFormat`） |

创建（带 IOSurface 承载）：

```rust
let attributes = CFDictionary::from_slices(
    &[unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
    &[&*CFDictionary::<CFType, CFType>::empty()],
);
let mut out: *mut CVPixelBuffer = ptr::null_mut();
let status = unsafe {
    CVPixelBufferCreate(None, w, h, 0x42475241 /* 'BGRA' */,
                        Some(attributes.as_opaque()), NonNull::from(&mut out))
};
assert_eq!(status, 0);
let buffer = unsafe { CFRetained::from_raw(NonNull::new(out).unwrap()) };
```

零拷贝导入：

```rust
use wgpu_external_frame::io_surface::{PackedFormat, PackedIoSurfaceFrame};

let ios = unsafe { CVPixelBufferGetIOSurface(&frame) }.expect("不是 IOSurface-backed");
let ios_frame = unsafe {
    PackedIoSurfaceFrame::retain(
        NonNull::new(CFRetained::as_ptr(&ios) as *mut c_void).unwrap(),
        PackedFormat::Bgra8,
    )
};
let texture: wgpu::Texture = ios_frame.import(&device);
```

**生命周期**：`texture` 是同一块显存的别名，`frame` 必须活得更久。替换帧时同步释放旧帧。

---

## 5. CoreMedia：CMSampleBuffer / CMTime

```rust
// PTS（秒）
let pts: f64 = unsafe { sample.presentation_time_stamp().seconds() };

// 构造时间
let t = unsafe { CMTime::new(0, 1) };   // value=0, timescale=1
```

- `seconds()` 和 `CMTime::new` 都是 **unsafe fn**
- `presentation_time_stamp()` 需要 `CMSampleBuffer` + `CMTime` 两个 feature
- 没有 `CMTime::ZERO` 常量绑定，用 `CMTime::new(0, 1)`

---

## 6. AVFoundation：解码与音频

### 6.1 打开 asset 与轨道

```rust
let url = NSURL::fileURLWithPath(&NSString::from_str(path));
let asset = AVAsset::assetWithURL(&url);

let track = asset.tracksWithMediaType(AVMediaTypeVideo.ok_or("...")?)
                 .firstObject().ok_or("没有视频轨")?;
let frame_rate = track.nominalFrameRate();
let (w, h) = (track.naturalSize().width, track.naturalSize().height);
```

`tracksWithMediaType` 已被标记 deprecated（建议 `loadTracksWithMediaType:completionHandler:`），
同步场景下仍可用，只是会有 warning。

### 6.2 视频轨 → BGRA + IOSurface

```rust
let key_format    = NSString::from_str("PixelFormatType");       // 避开 CFString 桥接
let key_iosurface = NSString::from_str("IOSurfaceProperties");
let empty = NSDictionary::<NSString, AnyObject>::from_slices(&[], &[]);
let bgra  = NSNumber::new_u32(0x42475241);
let settings = NSDictionary::<NSString, AnyObject>::from_slices(
    &[&*key_format, &*key_iosurface], &[&*bgra, &*empty]);

let video = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&track, Some(&settings));
reader.addOutput(&video);
```

### 6.3 音频轨 → 压缩包直通

```rust
// outputSettings 传 None：reader 原样输出压缩音频（AAC），
// AVSampleBufferAudioRenderer 正好能吃，省掉整条 PCM 转换链
let audio = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&atrack, None);
reader.addOutput(&audio);

if !reader.startReading() { /* 处理错误 */ }
```

取数据：

```rust
let sample = output.copyNextSampleBuffer()?;   // 到头返回 None
```

### 6.4 音频播放与主时钟

```rust
use objc2_av_foundation::{
    AVQueuedSampleBufferRendering, AVSampleBufferAudioRenderer,
    AVSampleBufferRenderSynchronizer,
};

let renderer = unsafe { AVSampleBufferAudioRenderer::new() };
let synchronizer = unsafe { AVSampleBufferRenderSynchronizer::new() };
unsafe { synchronizer.addRenderer(ProtocolObject::from_ref(&*renderer)) };

// 喂数据（缓冲未满时）
unsafe { renderer.enqueueSampleBuffer(&sample) };
let ready = unsafe { renderer.isReadyForMoreMediaData() };

// 启动 + 取时钟
unsafe { synchronizer.setRate_time(1.0, CMTime::new(0, 1)) };
let now_secs = unsafe { synchronizer.currentTime().seconds() };
```

签名：

```rust
fn addRenderer(&self, renderer: &ProtocolObject<dyn AVQueuedSampleBufferRendering>);
fn setRate_time(&self, rate: c_float, time: CMTime);
fn currentTime(&self) -> CMTime;

// 来自 AVQueuedSampleBufferRendering 协议
unsafe fn enqueueSampleBuffer(&self, sample_buffer: &CMSampleBuffer);
unsafe fn isReadyForMoreMediaData(&self) -> bool;
unsafe fn hasSufficientMediaDataForReliablePlaybackStart(&self) -> bool;
```

`AVSampleBufferAudioRenderer` 实现了 `AVQueuedSampleBufferRendering`，且**不是** `MainThreadOnly`。

⚠️ **prefill 死锁**：不要用 `is_ready()` 当 `play()` 的前提 —— 填满缓冲后它必然为 `false`，
而时间轴不启动缓冲就不会被消耗，双方互等。正确顺序是「prefill → 直接 play」。

---

## 7. objc2 0.6 写法

### 7.1 `define_class!`

```rust
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2::rc::Retained;
use objc2_foundation::{NSObject, NSObjectProtocol, NSSize};

pub struct Ivars { aspect: Cell<f64> }

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YKPiliAspectRatioDelegate"]
    #[ivars = Ivars]
    pub struct AspectRatioDelegate;          // ← 分号

    unsafe impl NSObjectProtocol for AspectRatioDelegate {}
    unsafe impl NSWindowDelegate for AspectRatioDelegate {}

    impl AspectRatioDelegate {
        #[unsafe(method(windowWillResize:toSize:))]
        fn window_will_resize(&self, sender: &NSWindow, size: NSSize) -> NSSize { ... }
    }
);

impl AspectRatioDelegate {
    pub fn new(mtm: objc2::MainThreadMarker, aspect: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { aspect: Cell::new(aspect) });
        unsafe { msg_send![super(this), init] }
    }
}
```

要点：

- 宏名是 `define_class!`，**不是** `declare_class!`
- 结构体是 `pub struct Name;`（分号），不是花括号
- ivars 必须是独立类型并 `#[ivars = ...]`
- `MainThreadOnly` 类型用 `alloc(mtm)`，需要 `MainThreadMarker`
- `NSObjectProtocol` / `DefinedClass` 要在作用域内
- `NSWindow.delegate` 是**弱引用** → 调用方必须持有 `Retained<...>`

安装 delegate：

```rust
let delegate = AspectRatioDelegate::new(mtm, aspect);
unsafe { ns_window.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
self.aspect_delegate = Some(delegate);   // 自己拿着
```

### 7.2 从 winit `Window` 拿 `NSWindow`

winit 0.30 的 `Window` 没有 `ns_window()`：

```rust
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

let Ok(handle) = window.window_handle() else { return None };
let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return None };
let view: &NSView = unsafe { &*(appkit.ns_view.as_ptr() as *mut NSView) };
let ns_window = unsafe { view.window() }?;      // Option<Retained<NSWindow>>
```

### 7.3 几何类型

`CGPoint` / `CGRect` / `CGSize` **不在** `objc2_foundation` 根下，用别名：

```rust
use objc2_foundation::{NSPoint, NSRect, NSSize};   // ✅ NSPoint = CGPoint
use objc2_foundation::CGPoint;                      // ❌ unresolved import
```

### 7.4 直接调用 delegate 方法（自检用）

```rust
let adjusted: NSSize = unsafe {
    msg_send![&*delegate, windowWillResize: &*ns_window, toSize: proposed]
};
let content = ns_window.contentRectForFrameRect(NSRect {
    origin: NSPoint { x: 0.0, y: 0.0 },
    size: adjusted,
});
```

---

## 8. 编译错误 → 修法

| 错误 | 原因 / 修法 |
|---|---|
| `unresolved import objc2_foundation::CGPoint` | 用 `NSPoint` / `NSSize` / `NSRect` |
| `no method named enqueueSampleBuffer` | 引入 `AVQueuedSampleBufferRendering` trait，并开启同名 feature |
| `expected struct, found ;`（define_class） | 结构体要写 `pub struct Name;` |
| `the trait MainThreadMarker is not satisfied` / `alloc` 缺参 | 类型标了 `MainThreadOnly`，用 `new(mtm, ...)` |
| `bind_group_layouts` 类型不符 | 改成 `&[Some(&layout)]` |
| `missing field multiview_mask` / `depth_slice` / `immediate_size` | wgpu 30 新增字段，补 `None` / `0` |
| `target format does not match the encoding` | `Gamma8` 要 `Rgba8Unorm` target → 走中间纹理 |
| `feature "test-support" not found`（gpui 场景） | 只对平台层 crate 启用，别对整个 facade 启用 |

---

## 9. 环境与工具

| 现象 | 处理 |
|---|---|
| `xcrun: error: unable to find utility "metal"` | `export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`（无需 sudo） |
| `~/.cargo` 被沙箱拒绝 | `export CARGO_HOME="$(pwd)/.cargo-home"` |
| 没有 `timeout` / `ps` | 用后台 job 管理；存活检查用 `pgrep -fl <name>` |
| `screencapture` 失败（屏幕录制权限） | 改用离屏读回 / 日志断言 |
| `error messaging the mach port for IMKCFRunLoopWakeUpReliable` | macOS 输入法在非标准事件循环下的噪音，无害 |
| 磁盘占用暴涨 | `target/` 与 `.cargo-home/` 可安全删除（4K spike 约 1.9G；gpui 类 spike 可达 6G+） |

复现命令（本工程 `demos/spike-mpv-wgpu/`）：

```bash
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
export CARGO_HOME="$(pwd)/.cargo-home"

cargo run --bin decode_test          # 纯解码冒烟测试，不开窗口
SPIKE_SIZE=1600x1000 cargo run       # 播放器
```

| 环境变量 | 作用 |
|---|---|
| `SPIKE_SIZE=WxH` | 初始窗口尺寸（会被自动校正到视频比例） |
| `SPIKE_MAX_FRAMES=N` | 渲染 N 帧后退出并打印平均帧率（0 = 无限） |
| `SPIKE_NO_THROTTLE=1` | 关闭时钟节流，用于裸渲染能力压测 |
