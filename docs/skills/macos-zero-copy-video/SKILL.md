---
name: macos-zero-copy-video
description: 在 macOS 上构建或排查自绘视频播放器内核：AVFoundation/VideoToolbox 解码 → IOSurface 零拷贝导入 wgpu → mpv-wgpu 渲染 → 等比重绘上屏，音频走 AVSampleBufferAudioRenderer，播放时钟走 AVSampleBufferRenderSynchronizer。也用于诊断这类播放器的具体症状：画面红蓝互换、4K 缩小窗口时锯齿、窗口缩放出现黑边、拖拽卡顿、拖拽时窗口比例漂移、播放速度比片源快、音视频不同步、CVPixelBuffer 导入 wgpu 失败、NSWindow 宽高比锁不住、色彩通道错乱。Use when building or debugging macOS native video playback with CVPixelBuffer/IOSurface zero-copy into wgpu/WebGPU/Metal, mpv-wgpu renderer, AVAssetReader decoding, AVSampleBufferAudioRenderer audio, or NSWindow aspect-ratio locking — or when the user reports video that plays too fast, stutters while resizing, shows black bars, drifts out of aspect ratio, or has swapped color channels.
---

# macOS 自绘视频播放器（零拷贝）

用 AVFoundation 解复用、VideoToolbox 硬解，把视频帧经 IOSurface **零拷贝**导入 wgpu，
再用 mpv-wgpu 做色彩/缩放，自己 blit 上屏。

```
AVAssetReader ──► CVPixelBuffer (IOSurface, BGRA)
                      │ wgpu-external-frame  ← 零拷贝
                      ▼
                  wgpu::Texture ──► mpv_wgpu::Renderer ──► 中间纹理 (Rgba8Unorm, 视频尺寸)
                                                                │ blit（等比居中）
                                                                ▼
                                                        surface (Bgra8Unorm)

音频：AVAssetReader ──► 压缩 CMSampleBuffer ──► AVSampleBufferAudioRenderer
                                                      ↑
                    AVSampleBufferRenderSynchronizer.currentTime()  ← 主时钟
```

**先读这一节再看代码**：这套组合里最容易出错的不是 GPU，而是**坐标系**（frame vs content）
和**线程/事件循环**（sleep vs WaitUntil vs request_redraw）。绝大多数「画面上的怪现象」根因在别处。

---

## 一、先确认路线

| 方案 | 含 libmpv | 渲染路径 | 何时选 |
|---|---|---|---|
| libmpv + render API | ✅ | libmpv 全包，我们只贴纹理 | 要跨平台、能接受 macOS 只剩 OpenGL 后端 |
| mpv-wgpu-player | ✅ | `MPV_RENDER_API_TYPE_SW` → CPU `rgb0` → **每帧全量上传** | ❌ 基本不要选。4K 帧 24MB/帧，纯浪费 |
| **mpv-wgpu（纯渲染器）+ 自建解码** | ❌ | 我们给 GPU 纹理，它做色彩/缩放/tone mapping | ✅ 能自己掌控解码、时钟、UI 叠层时 |

`mpv-wgpu` 只提供 `vo=gpu` 的 picture chain（色彩矩阵、chroma location、EOTF、缩放、tone mapping、dither、overlay），
**不含解复用器和解码器** —— 解码必须自己来（macOS 上就是 AVFoundation）。

---

## 二、症状 → 根因速查

排障时**先在这里找症状**，不要直接读渲染代码。

| 症状 | 根因 | 修法 |
|---|---|---|
| 红蓝互换 / 色彩通道错乱 | blit shader 里**多做了一次 swizzle** | 删掉 `.bgra`。`Bgra8Unorm` 的格式语义已由 wgpu 处理，`textureSample().rgb` 就是正确的 RGB |
| 4K 片源缩到小窗口锯齿严重 | mpv-wgpu 的 cubic 固定 **4-tap**，无 correct-downscaling | 缩放在 2:1 以内可接受；更小需自己多级 2:1 预降采样 |
| 窗口缩放总挂着黑边 | `windowWillResize:toSize:` 收发的是 **frame** 尺寸（含标题栏），锁的是外框比例 | 量出标题栏高度再加回去（见 §四） |
| 拖拽时窗口比例乱漂 | 用了 `NSWindow.contentAspectRatio` —— 在 winit 窗口上**读回正常但不生效** | 改装 `NSWindowDelegate` |
| 拖左上角时右下角跟着跑 | `request_inner_size` 的锚点固定在左上 | 交给 delegate，锚点由 AppKit 维护 |
| 拖拽手感卡 | 渲染循环里有 `thread::sleep`，**按住了主线程** | 换成 `ControlFlow::WaitUntil` |
| 播放速度比片源快（如 48fps vs 25fps） | `set_control_flow(WaitUntil)` 之后又调了 `request_redraw()` | 删掉那句；**并在渲染入口加时间门控**（二者缺一不可） |
| 等时钟/等缓冲时一帧都不出，CPU 却空转 | 拿 `is_ready()` 当 `play()` 的前提 → prefill 填满后恒为 false → 死锁 | prefill 后**直接 play** |
| 音视频漂移 | 用「帧序号 ÷ 标称帧率」推算时间（只对 CFR 成立） | 用 `CMSampleBuffer` 的真实 PTS，对齐到音频时间轴 |
| `CVPixelBufferCreate` 返回 **-6680** | macOS 不支持创建 `'RGBA'` 的 CVPixelBuffer | 改用 `'BGRA'`（`0x42475241`） |
| `CVPixelBufferGetIOSurface` 返回 null | 创建时没带 `IOSurfaceProperties` | 传 `kCVPixelBufferIOSurfacePropertiesKey` 空字典 |
| `target format does not match the encoding` | `Encoding::Gamma8` 要求 `Rgba8Unorm`，macOS surface 没这个格式 | 渲染进 `Rgba8Unorm` 中间纹理，再 blit 到 surface |
| 纹理内容错乱 / 崩溃 | `CVPixelBuffer` 先于导入的 `wgpu::Texture` 被释放 | 显式持有帧，且保证释放顺序 |

---

## 三、实现要点

### 3.1 零拷贝四要素

1. CVPixelBuffer 必须是 **IOSurface-backed**（传 `IOSurfaceProperties` 属性）
2. 像素格式用 **`'BGRA'`**（`0x42475241`），macOS 创建不出 `'RGBA'`
3. `PackedIoSurfaceFrame::retain(ptr, PackedFormat::Bgra8).import(&device)` 拿到 `wgpu::Texture`
4. **生命周期**：帧必须活得比纹理久，替换时立即释放上一帧

```rust
let _previous = self.frame_owner.replace(decoded);   // 旧的在这里释放
```

`NSDictionary` 传 key 时用字符串字面量，避开 `CFString ↔ NSString` 桥接：
`"PixelFormatType"`、`"IOSurfaceProperties"`。

### 3.2 中间纹理：既是格式桥梁，也是缩放/黑边的载体

macOS Metal surface 只有 `[Bgra8UnormSrgb, Bgra8Unorm, Rgba16Float, Rgb10a2Unorm]`，
**没有 `Rgba8Unorm`**，而 `Encoding::Gamma8` 要它。所以分两步走。

**中间纹理的尺寸必须绑视频尺寸，不能绑窗口尺寸** ——
否则每次拖拽都要重新分配显存，这是拖拽卡顿的常见来源之一。

黑边由 blit 的 **viewport + 清屏色**给出：

```rust
let (dx, dy, dw, dh) = fit_rect(src_w, src_h, out_w, out_h);
pass.set_viewport(dx as f32, dy as f32, dw as f32, dh as f32, 0.0, 1.0);
// LoadOp::Clear(Color::BLACK) 填掉 viewport 之外
```

`fit_rect` 留 **2% 容差**：窗口比例被系统取整后很难与视频比例精确一致，
差异在容差内就直接铺满 —— 1% 拉伸肉眼不可见，细黑边却看得出来。

### 3.3 播放时钟

```rust
let renderer = AVSampleBufferAudioRenderer::new();
let synchronizer = AVSampleBufferRenderSynchronizer::new();
synchronizer.addRenderer(ProtocolObject::from_ref(&*renderer));
synchronizer.setRate_time(1.0, CMTime::new(0, 1));

// 每帧：PTS 到了才出画
let remaining = pts - synchronizer.current_time().seconds();
if remaining > 0.002 { return Tick::Wait(Duration::from_secs_f64(remaining.min(0.05))); }
```

- **提前解码一帧**存进 `pending_video`，否则「等时钟」的间隙没帧可出
- 每次 `Wait` 设 50ms 上限：音频时钟停住（欠载、切设备）时不至于睡死
- 无音频片源退化为「首帧 PTS 为原点的墙上时钟」

### 3.4 音频：压缩包直通，别转 PCM

`AVAssetReaderTrackOutput` 的 `outputSettings` 传 **`None`**，reader 原样吐出压缩包（AAC），
`AVSampleBufferAudioRenderer` 正好接受，省掉整条 PCM 转换链（`AVFormatIDKey` / `AVLinearPCM*` / `AVAudioPCMBuffer`）。

`enqueueSampleBuffer` 和 `isReadyForMoreMediaData` 来自 **`AVQueuedSampleBufferRendering` 协议**，
必须把该 trait 引入作用域。

---

## 四、窗口等比缩放清单

目标：比例跟随视频、拖拽跟手、拖拽时画面继续播放。

1. **锁比例只能用 `NSWindowDelegate`** —— `contentAspectRatio` 在 winit 窗口上无效
2. **`windowWillResize:toSize:` 收发的是 frame 尺寸**（含标题栏），要锁的是 content 比例：

```rust
#[unsafe(method(windowWillResize:toSize:))]
fn window_will_resize(&self, sender: &NSWindow, size: NSSize) -> NSSize {
    let aspect = self.ivars().aspect.get();
    let frame = sender.frame();
    let content = sender.contentRectForFrameRect(frame);
    let chrome = (frame.size.height - content.size.height).max(0.0);   // 标题栏
    NSSize { width: size.width, height: (size.width / aspect + chrome).round() }
}
```

3. **`NSWindow.delegate` 是弱引用** —— 必须自己持有 delegate
4. **别用 `thread::sleep`** —— 用 `ControlFlow::WaitUntil`
5. **`WaitUntil` 之后不要 `request_redraw()`** —— 会让限速完全失效
6. **渲染入口要再拦一道时间门控** —— 窗口系统仍会高频发 `RedrawRequested`
7. **中间纹理绑视频尺寸** —— 窗口怎么缩放都不重新分配显存

---

## 五、验证手法：让机器自证，别浪费用户的反馈

用户的「拖一下看看」是稀缺资源。优先用下面几种方式自己拿到结论：

**① 把约束逻辑做成可自检的纯函数。** 把假想尺寸喂进去，打印结果，并且
**故意喂错误值**（如 0.6 这种明显比例），看它是否被拉回目标 ——
只喂正确值的话，一个「直接 return 输入」的空实现也能通过。

```
[aspect] 换算自检（目标 content 比例 1.7778）:
[aspect]   frame 1600×960 → frame 1600×932 | content 1600×900 = 1.7778 ✅
```

**② 用日志反推根因。** 「总有黑边」读代码看不出来，但把窗口比例和理论值一对比，
**32pt 的固定差值**立刻暴露了 frame/content 错配。所以关键路径要打印**数值**，不只是成功/失败。

**③ 离屏读回。** `Window::render_to_image()`（gpui）这类 API 可以在**不需要屏幕录制权限**时导出渲染结果 PNG。

**④ 裸渲染压测。** 遇到卡顿先量出硬上限（如 132.6 fps / 7.5ms 每帧），
避免在算力充足时错误地去优化渲染细节。

---

## 六、排查原则

**先问「主线程此刻在等谁」，再问「这一帧算了多少」。**

被用户纠正过的三个同类错误，全都是把「线程/调度问题」误判成「渲染性能问题」：

| 以为是 | 其实是 |
|---|---|
| 中间纹理重建太贵 | 它根本不该跟窗口尺寸走 |
| 需要更高效的等待方式 | `sleep` 直接按住了主线程 |
| 调度器精度不够 | `request_redraw()` 把 `WaitUntil` 抵消了 |

macOS 上还有两条硬约束，别去对抗：present mode 只有 `[Fifo, Immediate]`；surface 没有 `Rgba8Unorm`。

---

## 详细参考

需要具体 API 签名、Cargo feature 列表、wgpu 30 迁移对照、objc2 0.6 写法时，
读 **[`reference.md`](./reference.md)**。

完整实战记录（含全部日志证据与决策过程）见工程的
`docs/macos-零拷贝视频播放器踩坑全记录.md`。
