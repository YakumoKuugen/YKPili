# macOS 零拷贝视频播放器实战：踩坑全记录

> **面向场景**：在 macOS 上用 **AVFoundation 解复用 + VideoToolbox 硬解 + IOSurface 零拷贝**，
> 把视频帧直接导入 wgpu，再用 **mpv-wgpu**（纯渲染器）做色彩/缩放，最后自绘上屏。
> 目标是做一个**自己完全掌控渲染管线**的播放器内核 —— 不依赖系统播放器控件，
> 也不依赖 libmpv 的窗口系统。
>
> **验证工程**：`demos/spike-mpv-wgpu/`（约 1300 行 Rust，可运行）
> **测试素材**：4K H.264，3840×2160 @ 25.000 fps，时长 1174.9s
>
> **配套文档**：libmpv 本身的跨平台可用性见 [`libmpv-跨平台调研.md`](./libmpv-跨平台调研.md)

---

## 0. 先澄清一件事：这个 demo 里没有 libmpv

标题里常被笼统称作「libmpv 播放器」，但**本工程一行 libmpv 都没链接**。三条路线的取舍如下：

| 方案 | 含 libmpv | 渲染方式 | 结论 |
|---|---|---|---|
| **libmpv + render API** | ✅ | libmpv 自己解复用/解码/渲染，我们只管把 GL 纹理贴出来 | 跨平台最省事，但 macOS 上 render API 只剩 OpenGL（Apple 已弃），且窗口/事件循环得交给它 |
| **mpv-wgpu-player** | ✅（走 `rsmpv`） | `MPV_RENDER_API_TYPE_SW` → CPU 内存 `rgb0` → **每帧 1:1 上传** | ❌ **未采用**。4K 帧 24MB，每帧一次 CPU 拷贝 + PCIe 上传，纯浪费 |
| **mpv-wgpu（纯渲染器）+ 自建解码** | ❌ | 我们拿 GPU 纹理，它只做色彩转换/缩放/tone mapping | ✅ **采用**。依赖只有 `wgpu` / `bytemuck` / `thiserror` |

`mpv-wgpu` 0.1.0 提供的是 `vo=gpu` 的 **picture chain**（色彩矩阵、chroma location、EOTF、缩放、tone mapping、dither、overlay），
**不包含任何解复用器和解码器**。所以解码这件事得自己来 —— 在 macOS 上就是 AVFoundation。

最终链路：

```
AVAssetReader（解复用 + VideoToolbox 硬解）
   └─ CVPixelBuffer（IOSurface-backed, BGRA）
         │  wgpu-external-frame::io_surface    ← 零拷贝，Metal 直接别名这块显存
         ▼
      wgpu::Texture (Bgra8Unorm)
         │  mpv_wgpu::Renderer                 ← 色彩转换 / tone mapping
         ▼
      中间纹理 (Rgba8Unorm, 视频尺寸)
         │  blit（等比居中 + viewport）
         ▼
      surface (Bgra8Unorm) → 屏幕

音频另走一条：
AVAssetReader → 压缩 CMSampleBuffer → AVSampleBufferAudioRenderer
                                            ↑
              AVSampleBufferRenderSynchronizer.currentTime()  ← 视频对齐到这里（主时钟）
```

---

## 1. 零拷贝链路

### 1.1 macOS 创建不出 'RGBA' 的 CVPixelBuffer

`CVPixelBufferCreate` 传 `kCVPixelFormatType_32RGBA` 返回 **-6680**（`kCVReturnInvalidPixelFormat`）。

```rust
// ❌ 失败：status = -6680
CVPixelBufferCreate(None, w, h, 0x52474241 /* 'RGBA' */, attrs, &mut out);

// ✅ 只能走 BGRA
CVPixelBufferCreate(None, w, h, 0x42475241 /* 'BGRA' */, attrs, &mut out);
```

**推论**：不要假设「内存布局是 RGBA 还是 BGRA」可以随便挑。macOS 的视频管线原生就是 BGRA，顺着它走。

### 1.2 必须 IOSurface-backed，否则零拷贝无从谈起

创建时要带上 `kCVPixelBufferIOSurfacePropertiesKey`（空字典即可），否则 `CVPixelBufferGetIOSurface` 返回 null：

```rust
let attributes = CFDictionary::from_slices(
    &[unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
    &[&*CFDictionary::<CFType, CFType>::empty()],
);
```

### 1.3 NSDictionary 传 key：用字符串字面量绕开桥接

用 `CFString` 常量做 `NSDictionary` 的 key 会撞上 `CFString ↔ NSString` 的桥接问题。
这两个 key 的字符串值恰好与常量名后缀一致，直接用字面量最省事：

```rust
let key_format    = NSString::from_str("PixelFormatType");      // kCVPixelBufferPixelFormatTypeKey
let key_iosurface = NSString::from_str("IOSurfaceProperties");  // kCVPixelBufferIOSurfacePropertiesKey
```

### 1.4 生命周期：CVPixelBuffer 必须活得比 wgpu::Texture 久

`wgpu-external-frame` 导入出来的是**对同一块显存的别名**，不是拷贝。
底下的 IOSurface 被回收，纹理就成了悬空引用。所以要么用 `with_owner`，要么自己按顺序持有：

```rust
// 帧所有权交给 App 持有，且**在替换时立即释放上一帧**
let _previous = self._frame_owner.replace(decoded);
```

**注意释放顺序**：`imported` 纹理必须先于 `CVPixelBuffer` 释放。

---

## 2. wgpu 30 的 API 变更（编译期就能撞上）

wgpu 30 相对 0.20 时代改动很大，逐个踩过来：

| 位置 | 变更 |
|---|---|
| `Instance::new` | 用 `Instance::default()`（`InstanceDescriptor` 不再实现 `Default`） |
| `RequestAdapterOptions` | 新增必填 `apply_limit_buckets` |
| `DeviceDescriptor` | 新增必填 `experimental_features` |
| `SurfaceConfiguration` | 新增必填 `color_space` |
| `get_current_texture()` | 返回 `CurrentSurfaceTexture` 枚举：`Success` / `Suboptimal` / `Occluded` / `Timeout` / `Outdated` / `Lost` / `Validation` |
| `present` | 从 `Surface` 移到 **`Queue::present(frame)`** |
| `PipelineLayoutDescriptor` | 新增 `immediate_size: 0` |
| `PipelineLayoutDescriptor.bind_group_layouts` | 类型变成 `&[Option<&BindGroupLayout>]`，要写 `&[Some(&layout)]` |
| `RenderPipelineDescriptor` | 新增 `multiview_mask: None` |
| `RenderPassDescriptor` | 新增 `multiview_mask: None` |
| `RenderPassColorAttachment` | 新增 `depth_slice: None` |

`Occluded` 要当「这帧不用画」而不是错误处理 —— 窗口被完全遮挡时它会持续返回。

---

## 3. mpv-wgpu 的用法与陷阱

### 3.1 `Encoding::Gamma8` 要求目标是 `Rgba8Unorm`，但 macOS surface 没有这个格式

实测 macOS Metal surface 支持的格式：

```
[Bgra8UnormSrgb, Bgra8Unorm, Rgba16Float, Rgb10a2Unorm]
```

**没有 `Rgba8Unorm`**。直接渲染到 surface 会报 `target format does not match the encoding`。

**解法**：分两步 —— 先渲染进一个 `Rgba8Unorm` **中间纹理**，再 blit 到 surface。
这个中间纹理顺带解决了缩放和黑边问题（见 §4）。

### 3.2 ⚠️ 不要在 blit shader 里做通道 swizzle

这是本项目最贵的一个错误：画面「所有红色都变成蓝色」。

`PackedFormat::Bgra8` 创建出来的纹理格式是 `Bgra8Unorm`，
**wgpu 在采样时已经把格式语义处理掉了**，`textureSample(...).rgb` 拿到的就是正确的 (R, G, B)。
我又在 shader 里手动 `.bgra` 转了一次，等于转了两遍。

```wgsl
// ❌ 红蓝互换
let c = textureSample(src_tex, src_samp, in.uv).bgra;

// ✅ 格式语义由 wgpu 负责
let c = textureSample(src_tex, src_samp, in.uv);
```

**排查手法**：画面出现通道错乱时，先怀疑「多转了一次」，而不是「少转了一次」。

### 3.3 缩放质量弱于 mpv 本体：无 correct-downscaling

`mpv-wgpu/src/cubic.rs` 里 `kind_for_axis` 固定是 **4-tap**（放大 CatmullRom / 缩小 Hermite），
**没有 correct-downscaling** 路径。

后果：4K 片源缩到 720p 窗口（3:1）时欠采样严重，**锯齿明显**；
缩到 1920×1080（2:1）时就基本看不出来了。

**结论**：配小窗口时，4K 片源需要自己先做多级 2:1 预降采样（尚未实现）。

### 3.4 其它 API 细节

- `Picture::Rgba(Plane)` 是单平面；`Picture::Yuv { y, u, v }` 要求 U/V **分离**的纹理
- `PlaneSource::Texture(&TextureView)` 的格式由纹理决定，`Plane.bits` 字段被**忽略**
- `dest: PixelRect` 是渲染目标区域，配合 `target: &Texture` 使用

---

## 4. 窗口缩放（本项目坑最密集的部分）

需求：**窗口比例跟随视频比例、拖拽像系统播放器一样跟手、且拖拽时画面继续播放、不能有 3:1 缩放的那种锯齿。**

### 4.1 `NSWindow.contentAspectRatio` 在 winit 窗口上失效

```rust
ns_window.setContentAspectRatio(NSSize { width: 16.0, height: 9.0 });
```

**读回正常，但拖拽时完全不约束**。实测拖拽时窗口比例在 1.345 ~ 2.330 之间乱跳。
调换「先 setContentAspectRatio 再 request_inner_size」的顺序也没用。

**唯一可靠的解法：装 `NSWindowDelegate`，实现 `windowWillResize:toSize:`。**

```rust
#[unsafe(method(windowWillResize:toSize:))]
fn window_will_resize(&self, sender: &NSWindow, size: NSSize) -> NSSize { ... }
```

顺带的好处：**锚点交给 AppKit 维护**（见 4.3）。

### 4.2 🔥 最大的坑：`windowWillResize:toSize:` 收发的都是 frame 尺寸，不是 content 尺寸

**现象**：比例锁是装上了，但拖拽时终端一直在刷「出现黑边」，屏幕左右各挂着 28px 黑边。

**根因反推**。日志里 window 比例稳定漂在 1.839 ~ 1.907，而不是 1.7778：

```
resize 1586x860 (比例 1.844) → 画面 1529x860 @(28,0) 出现黑边
resize 1719x935 (比例 1.839) → 画面 1641x923 @(28,0) 出现黑边
```

拿两个点反解标题栏高度：

| 物理宽 | 实测 content 高 | 按 **frame** 比例算出的高 | 差值 |
|---|---|---|---|
| 1586 | 860 | 1586 / 1.7778 = 892 | **32** |
| 1719 | 935 | 1719 / 1.7778 = 967 | **32** |

差值恒为 32pt —— 就是**标题栏高度**。
`windowWillResize:toSize:` 的入参和返回值都是 **窗口外框（frame，含标题栏）**，
而我们想锁的是**内容区（content）**比例。锁外框 ⇒ 扣掉标题栏后 content 必然偏「宽」3~7% ⇒ `fit_rect` 只好每帧切黑边。

**修法**：量出标题栏高度再加回去。

```rust
#[unsafe(method(windowWillResize:toSize:))]
fn window_will_resize(&self, sender: &NSWindow, size: NSSize) -> NSSize {
    let aspect = self.ivars().aspect.get();
    // 标题栏 = frame 高 − content 高
    let frame = sender.frame();
    let content = sender.contentRectForFrameRect(frame);
    let chrome = (frame.size.height - content.size.height).max(0.0);

    NSSize {
        width: size.width,
        // content_h 满足比例，frame_h = content_h + 标题栏
        height: (size.width / aspect + chrome).round(),
    }
}
```

**用自检代替肉眼**：不必真去拖窗口，把假想的 frame 尺寸直接喂给 delegate，
再扣掉标题栏看 content 比例对不对：

```
[aspect] 换算自检（目标 content 比例 1.7778）:
[aspect]   frame  800×480  → frame  800×482  | content  800×450  = 1.7778 ✅
[aspect]   frame 1200×720  → frame 1200×707  | content 1200×675  = 1.7778 ✅
[aspect]   frame 1600×960  → frame 1600×932  | content 1600×900  = 1.7778 ✅
[aspect]   frame 2000×1200 → frame 2000×1157 | content 2000×1125 = 1.7778 ✅
```

输入故意给 0.6 这种错误比例，四组全被拉回 1.7778 才算通过。

### 4.3 `request_inner_size` 的锚点固定在左上角

用它在拖拽过程中主动校正尺寸，会出现**「拖左上角时右下角跟着跑」**。

**改用 delegate**：`windowWillResize:toSize:` 只返回目标尺寸，锚点由 AppKit 自己维护，四个角都正常。

### 4.4 拖拽卡顿 —— 真凶是 `thread::sleep`

**错误方向**：我一开始一直在优化「每帧做多少事」（中间纹理、surface 重配、渲染细节），
但真问题是**「谁占着主线程」**。

逐个排除的过程：

| 假设 | 结论 |
|---|---|
| 中间纹理跟着窗口尺寸重建 | 是问题之一，改为**中间纹理绑视频尺寸**，窗口怎么缩放都不重新分配显存 |
| `surface.configure` 太频繁 | 是问题之一，加 120ms 静默期防抖 |
| 拖拽时冻结渲染 | ❌ 用户明确否决：**「mac 的播放器拖动缩放窗口大小时还能继续播放」** |
| 裸渲染算力不够 | ❌ 实测 **132.6 fps / 7.5ms 每帧**，算力绰绰有余 |
| **渲染循环里的 `std::thread::sleep`** | ✅ **根因**。它把主线程按住，窗口事件只能排队，手感被拖成「视频帧率」 |

**修法**：用事件循环的等待超时替代 sleep。

```rust
// ❌ 主线程被按住，拖拽事件排队
std::thread::sleep(until_next_frame);

// ✅ 只让循环空转，事件随时能插进来
event_loop.set_control_flow(ControlFlow::WaitUntil(next_frame_at));
```

### 4.5 `WaitUntil` 之后又 `request_redraw()`，定时器形同虚设

**现象**：视频播放特别快（实测 63.8 / 48.4 fps，而片源是 25 fps）。

```rust
// ❌ 刚设完等待超时，又把重绘请求贴上去了
event_loop.set_control_flow(ControlFlow::WaitUntil(target));
w.request_redraw();   // ← 这一行让限速完全失效
```

### 4.6 光靠 `WaitUntil` 节流还不够，必须加渲染入口的时间门控

把 4.5 的 `request_redraw()` 删掉后，帧率仍是 63.8 / 48.4 fps ——
因为**窗口系统自己仍会高频触发 `RedrawRequested`**，这些重绘绕过了等待。

**必须在渲染入口再拦一道**：

```rust
WindowEvent::RedrawRequested => {
    if let Some(next) = self.next_frame_at {
        if Instant::now() < next {
            event_loop.set_control_flow(ControlFlow::WaitUntil(next));
            return;   // 没到点，直接不干活
        }
    }
    ...
}
```

**二者缺一不可**：`WaitUntil` 负责「别空转」，入口门控负责「重绘请求再多也只按点出帧」。
修完后实测 **60 帧 / 2.40s = 25.0 fps**，精确。

### 4.7 `NSWindow.delegate` 是弱引用

```rust
// delegate 必须自己持有，否则被释放后回调直接消失
self.aspect_delegate = Some(delegate);
```

### 4.8 黑边怎么画

中间纹理 = 视频原始尺寸（1:1，无缩放）；黑边由 **blit 的 viewport + 清屏色**给出：

```rust
let (dx, dy, dw, dh) = fit_rect(src_w, src_h, out_w, out_h);
pass.set_viewport(dx as f32, dy as f32, dw as f32, dh as f32, 0.0, 1.0);
// LoadOp::Clear(Color::BLACK) 负责填掉 viewport 之外的部分
```

`fit_rect` 里还留了 **2% 容差**：窗口比例被系统取整后很难与视频比例精确一致，
差异在 2% 以内就直接铺满 —— 1% 的拉伸肉眼不可见，细黑边却看得出来。

---

## 5. 播放时钟：从「按帧率节流」到「跟随 PTS」

### 5.1 按标称帧率推算时间只对 CFR 成立

```rust
// ❌ 第几帧 ÷ 标称帧率 —— VFR 片源会累积漂移
let target = t0 + Duration::from_secs_f64((frames_rendered + 1) as f64 / fps);
```

改成从 `CMSampleBuffer` 取真实 PTS：

```rust
let pts = sample.presentation_time_stamp().seconds();
```

### 5.2 主时钟交给音频时间轴

自己实现音频时钟要数已播放样本数、补偿设备输出延迟、处理欠载 —— 全是坑。
**Apple 已经有现成的**：

```rust
let renderer = AVSampleBufferAudioRenderer::new();
let synchronizer = AVSampleBufferRenderSynchronizer::new();
synchronizer.addRenderer(ProtocolObject::from_ref(&*renderer));
synchronizer.setRate_time(1.0, CMTime::new(0, 1));

// 现在几点：
let now = synchronizer.currentTime().seconds();
```

视频每一帧只要问一次「现在几点」，PTS 到了才出画：

```rust
let remaining = pts - synchronizer.current_time();
if remaining > 0.002 {
    return Tick::Wait(Duration::from_secs_f64(remaining.min(0.05)));
}
// 到点，出画
```

**实测对齐精度**：

```
[clock] 已渲染    0 帧 | 媒体时钟    0.000s | 该帧 PTS    0.000s | 滞后 0.0ms
[clock] 已渲染  100 帧 | 媒体时钟    4.001s | 该帧 PTS    4.000s | 滞后 1.2ms
[clock] 已渲染  200 帧 | 媒体时钟    8.000s | 该帧 PTS    8.000s | 滞后 0.4ms
```

200 帧 / 8.000s = **25.0 fps**，误差 1ms 级且**不累积**。
无音频的片源退化为「首帧 PTS 为原点的墙上时钟」。

### 5.3 🔥 prefill 死锁：拿 `is_ready()` 当播放前提

音频要提前缓冲若干秒，所以先 prefill 再 `play()`。但：

```rust
// ❌ 死锁
while audio.is_ready() { enqueue(...) }        // 填到缓冲满 → is_ready() 变 false
if audio.is_ready() { audio.play() }           // 永远进不去
// 而时间轴不启动 → 缓冲永远不会被消耗 → 双方互等，一帧都出不来
```

**现象**：日志停在启动阶段，一帧都没渲染，CPU 却在空转。

**修法**：prefill 完直接 `play()`，**不要**用 `is_ready()` 当启动条件。

```rust
self.pump_audio();       // 填满缓冲（或解码到头）
audio.play();            // 直接启动
```

### 5.4 提前解码一帧

「等时钟」的间隙必须手里已经有帧，否则到点了还要等解码：

```rust
if self.pending_video.is_none() {
    let (frame, pts) = decoder.next_video()?;   // 提前量 = 1 帧
    self.pending_video = Some((frame, pts));
}
```

---

## 6. 音频：压缩包直通，不做 PCM 转换

`AVAssetReaderTrackOutput` 的 `outputSettings` 传 **`None`**，reader 就原样吐出压缩音频包（AAC），
而 `AVSampleBufferAudioRenderer` **正好接受压缩包**，自己解码播放：

```rust
// 不设 outputSettings → 压缩包直通，省掉整条 PCM 转换链
let out = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&track, None);
```

对比：如果走 PCM，得自己设 `AVFormatIDKey` / `AVLinearPCMBitDepthKey` / `AVLinearPCMIsFloatKey` …，
再把 `CMBlockBuffer` 拷进 `AVAudioPCMBuffer`，最后接 `AVAudioEngine`。代码量和出错面都大得多。

---

## 7. objc2 0.6 使用要点

### 7.1 宏名与语法

```rust
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]      // 主线程类型必须声明
    #[name = "YKPiliAspectRatioDelegate"]
    #[ivars = Ivars]
    pub struct AspectRatioDelegate;      // ← 分号，不是花括号

    unsafe impl NSObjectProtocol for AspectRatioDelegate {}
    unsafe impl NSWindowDelegate for AspectRatioDelegate {}
    ...
);
```

- 是 `define_class!` 而**不是** `declare_class!`
- ivars 必须是独立类型，用 `#[ivars = ...]` 声明
- 实现 AppKit 协议时，`NSObjectProtocol` 和 `DefinedClass` 都要在作用域内

```rust
pub struct Ivars { aspect: Cell<f64> }

impl AspectRatioDelegate {
    pub fn new(mtm: MainThreadMarker, aspect: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { aspect: Cell::new(aspect) });
        unsafe { msg_send![super(this), init] }
    }
}
```

### 7.2 从 winit `Window` 拿 `NSWindow`

winit 0.30 的 `Window` **没有** `ns_window()`，要走 `raw_window_handle`：

```rust
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

let RawWindowHandle::AppKit(h) = window.window_handle()?.as_raw() else { return None };
let view: &NSView = unsafe { &*(h.ns_view.as_ptr() as *mut NSView) };
let ns_window = unsafe { view.window() }?;
```

### 7.3 几何类型

`CGPoint` **不在** `objc2_foundation` 根下（`NSPoint` / `NSSize` / `NSRect` 才是别名）：

```rust
use objc2_foundation::{NSPoint, NSRect, NSSize};   // ✅
use objc2_foundation::CGPoint;                      // ❌ unresolved import
```

### 7.4 `objc2-app-kit` 的 feature 选择

`NSWindowDelegate` 协议**没有独立 feature**，随 `NSWindow` 一起启用：

```toml
objc2-app-kit = { version = "0.3.2", features = ["NSWindow", "NSView", "NSResponder"] }
```

### 7.5 音视频相关 feature

```toml
objc2-av-foundation = { version = "0.3.2", features = [
  "AVAsset", "AVAssetTrack", "AVAssetReader", "AVAssetReaderOutput", "AVMediaFormat",
  "AVSampleBufferAudioRenderer", "AVSampleBufferRenderSynchronizer",
  "AVQueuedSampleBufferRendering", "objc2-core-media",
] }
objc2-core-media = { version = "0.3.2", features = ["CMSampleBuffer", "CMTime", "CMBase"] }
objc2-core-video  = { version = "0.3.2", features = [
  "CVPixelBuffer", "CVImageBuffer", "CVBuffer", "CVBase", "CVReturn",
  "CVPixelBufferIOSurface", "objc2-io-surface",
] }
```

`enqueueSampleBuffer` / `isReadyForMoreMediaData` 来自 **`AVQueuedSampleBufferRendering` 协议**，
必须把该 trait 引入作用域才能调用。`AVSampleBufferAudioRenderer` 实现了它。

---

## 8. 环境与工具

| 问题 | 处理 |
|---|---|
| `xcrun: error: unable to find utility "metal"` | `export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`（不需要 sudo） |
| `~/.cargo` 被沙箱拒绝 | `export CARGO_HOME=$(pwd)/.cargo-home` |
| 没有 `timeout` / `ps` 命令 | 改用后台 job 管理 + `pgrep -fl` |
| `screencapture` 因屏幕录制权限失败 | 改为**离屏读回**或**日志断言**验证，见 §9.3 |
| `error messaging the mach port for IMKCFRunLoopWakeUpReliable` | macOS 输入法在非标准事件循环下的噪音，与播放无关，可忽略 |
| macOS surface present mode 只有 `[Fifo, Immediate]` | 没有 Mailbox / AutoNoVsync，别指望三重缓冲 |

---

## 9. 方法论：比具体 API 更值钱的部分

### 9.1 分清「每帧做多少事」和「谁占着主线程」

这一轮被用户纠正了三次，**三次都指向同一类错误**：

| 我以为是 | 其实是 |
|---|---|
| 中间纹理每帧重建太贵 | 中间纹理**根本不该跟窗口尺寸走**（该绑视频尺寸） |
| 需要更高效的等待方式 | `thread::sleep` **直接按住了主线程** |
| 调度器精度不够 | `request_redraw()` **把 `WaitUntil` 抵消了** |

**教训**：渲染性能问题，先问「主线程此刻在等谁」，再问「这一帧算了多少」。

### 9.2 用日志里的数字反推根因

「拖拽时总有黑边」这件事，靠读代码是看不出来的。
但把日志里的窗口比例和标题栏高度一对比，**32pt 的固定差值**立刻暴露了
`frame` / `content` 坐标系错配 —— 这也是为什么值得在关键路径上打印**数值**而不只是「成功/失败」。

### 9.3 让机器自证，别靠肉眼

用户的反馈是稀缺资源，不该浪费在「拖一下看看」上：

- **换算逻辑自检**：把假想尺寸喂给 delegate，打印结果比例（§4.2）
- **离屏读回**：`Window::render_to_image()` 可以在**不需要屏幕录制权限**的情况下把渲染结果导出 PNG
- **像素级回归**：从中间纹理读回导出 PNG，避免再靠肉眼发现通道/方向错误（尚未实现）

### 9.4 用「反例输入」验证约束

自检时不要只喂正确比例 —— 喂 0.6 这种**明显错误**的输入，看它是否被拉回目标值。
只喂正确值的话，一个「直接 return 输入」的空实现也能通过。

---

## 10. 怎么跑起来

```bash
cd demos/spike-mpv-wgpu
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
export CARGO_HOME="$(pwd)/.cargo-home"

# 纯解码验证（不开窗口，最省事的冒烟测试）
cargo run --bin decode_test

# 播放器
SPIKE_SIZE=1600x1000 cargo run
```

可调环境变量：

| 变量 | 作用 |
|---|---|
| `SPIKE_SIZE=WxH` | 初始窗口尺寸（会被自动校正到视频比例） |
| `SPIKE_MAX_FRAMES=N` | 渲染 N 帧后退出并打印平均帧率（0 = 无限） |
| `SPIKE_NO_THROTTLE=1` | 关闭时钟节流，用于裸渲染能力压测 |

---

## 11. 尚未完成

- **预降采样**：消除 >2:1 缩放时的锯齿（多级 2:1 bilinear）
- **播放控制**：暂停 / 拖动进度 / 变速（`synchronizer.setRate_time` 已具备基础）
- **字幕**：尚未接入
- **像素级回归测试**：中间纹理读回导 PNG
- **异步渲染**：把解码+渲染移出主线程，主线程只做 blit + present

---

## 附：一句话总结

> macOS 上做自绘视频播放器，**难点不在 GPU 而在坐标系和线程** ——
> `frame` vs `content`、`WaitUntil` vs `request_redraw`、`sleep` vs 事件循环，
> 每一对搞错都会表现为「画面上的怪现象」，而根因在别处。
