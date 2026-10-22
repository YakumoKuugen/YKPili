# libmpv 跨平台可用性 / 集成方式 / 已知限制调研

> 面向场景：**.NET / Avalonia 应用嵌入 libmpv 作为播放后端**
> 调研时间：本轮 web 检索（官方文档、GitHub 源码、NuGet API、社区 issue）
> 标注约定：**【官方】** = mpv 官方文档/源码明确说明；**【社区】** = 第三方构建或绑定；**【不确定】** = 仅间接证据

---

## 0. 结论速览

| 平台 | 可行性 | 官方预编译 | 推荐渲染路径 | 主要坑 |
|---|---|---|---|---|
| Windows | 高 | ❌（第三方 shinchiro / zhongfly / 官方 CI） | `mpv_render_context` + `MPV_RENDER_API_TYPE_OPENGL`（ANGLE）或 0.42 新增 `d3d11` 后端；或 `wid` 嵌入 HWND | 无官方二进制，需自带 `libmpv-2.dll` |
| Linux | 高 | 发行版包（`libmpv.so`） | render API OpenGL（GLX/EGL）或 `wid` 嵌入 X11/Wayland | 发行版版本老旧（<0.40） |
| macOS | 中高 | ❌（官方 CI / media-kit / IINA 提供） | **render API 只能 OpenGL**；`wid` 嵌入 NSView 走 cocoa-cb（Metal via MoltenVK） | Apple 已弃 OpenGL；mpv 已删除自家 OpenGL cocoa 后端 |
| iOS | 中（社区方案成熟） | ❌ | render API OpenGL ES，或 `wid` + UIView | 需自建 xcframework；无官方支持 |
| Android | 中 | ❌ | `wid` + `android.view.Surface`（主流），或 render API OpenGL ES | mpv-android **不是 AAR 库**，只提供参考实现 |

.NET 绑定里唯一**同时**覆盖 Windows/macOS/Linux + Avalonia + 暴露 render API 的活跃选择是
`HanumanInstitute.LibMpv` / `HanumanInstitute.LibMpv.Avalonia`（同仓库 mysteryx93/LibMpv-OpenGL）。

---

## 1. 官方预编译二进制：mpv 官方**不**提供 libmpv 预编译库

**【官方】** mpv 安装页明确写：所有二进制包都是非官方第三方构建，官方 CI 构建主要供测试、可能缺功能。

- 安装页：<https://mpv.io/installation/>
  - Windows：shinchiro/mpv-winbuild-cmake、zhongfly/mpv-winbuild、官方 GitHub CI `git-release`、MSYS2
  - macOS：官方 GitHub CI `git-release`、stolendata、MacPorts、Homebrew（"without application bundles"）
  - Linux：各发行版包（Arch/Gentoo 官方包、deb-multimedia、ubuntu/debian apt 源、mpv-build）
  - 移动端：只列了 Google Play 上的 mpv-android App，**完全没有 libmpv 库的分发**

### 1.1 各平台文件名与获取方式

| 平台 | 文件名 | 来源 |
|---|---|---|
| Windows | `libmpv-2.dll`（+ `libmpv.def`/头文件在 `mpv-dev-*.7z`） | shinchiro/mpv-winbuild-cmake releases、zhongfly/mpv-winbuild releases（`mpv-dev-x86_64-<date>.7z`） |
| Linux | `libmpv.so.2` / `libmpv.so` | 发行版 `mpv-libs` / `libmpv-dev`；Fedora 包名 `mpv-libs`（<https://packages.fedoraproject.org/pkgs/mpv/mpv-libs/index.html>） |
| macOS | `libmpv.dylib`（另有 `libmpv.2.dylib`） | 无官方分发；社区构建：media-kit/libmpv-darwin-build、IINA deps-buildscripts、Homebrew 装 mpv 后取 dylib |
| iOS | `.xcframework` / 静态库 | media-kit/libmpv-darwin-build、MPVKit、stps/mpv-ios-scripts |
| Android | `.so`（`libmpv.so`） | 需自行用 NDK 编译，参考 mpv-android buildscripts；无官方预制 |

### 1.2 .NET 项目通常怎么拿 libmpv

三条现实路径（按省事程度排序）：

1. **直接用带 native 资产的 NuGet 包**（最省事）
   - `HanumanInstitute.LibMpv.Avalonia` 0.10.1 —— 其仓库 `MpvDll/` 目录内置 win-x64/win-x86/osx-x64/osx-arm64/android-arm/android-arm64/android-x64 的本地库（源码树证据：<https://github.com/mysteryx93/LibMpv-OpenGL/tree/main/MpvDll>）
   - `Endpne.LibMPV.Windows`（仅 Windows，随包带 `libmpv-2.dll` 语义，版本 0.41.0）
   - 注意：**没有任何 NuGet 包内置 macOS/iOS 的完整 libmpv + 依赖树**，macOS 仍需自己提供 dylib
2. **自己下载二进制 + `RuntimeIdentifier` 资产打包**（最可控）
   - Windows：shinchiro/zhongfly 的 `mpv-dev-*.7z`
   - macOS：media-kit/libmpv-darwin-build 的 release 资产（如 `libmpv-libs_v0.7.3_macos-universal-video-default.tar.gz`），或本机 `brew install mpv` 后从 `/opt/homebrew/lib/libmpv.dylib` 取
3. **从 mpv 源码 / mpv-build 自编**（最麻烦，但 LGPL 可控 + 可裁减解码器）

---

## 2. macOS 专项

### 2.1 渲染后端现状（关键、容易踩坑）

mpv **没有** `--gpu-api=metal` 这个取值。源码事实：

- `video/out/gpu/context.c`（<https://github.com/mpv-player/mpv/blob/master/video/out/gpu/context.c>）里的 context 列表只有：
  - Direct3D 11：`ra_ctx_d3d11`
  - Vulkan：`vulkan_win` / `vulkan_wayland` / `vulkan_xlib` / **`vulkan_mac`** / `vulkan_android` / `vulkan_display`
  - OpenGL：`android` / `angle` / `wgl` / `dxgl` / `wayland_egl` / `x11_egl` / `glx` / `drm_egl`
  - **没有任何 cocoa/OpenGL mac 上下文**
- `video/out/vulkan/context_mac.m`：`.type = "vulkan"`, `.name = "macvk"`, `.description = "mac/Vulkan (via Metal)"`，通过 `VK_EXT_metal_surface` 把 `CAMetalLayer` 交给 MoltenVK。
  → 即 **macOS 上 `--gpu-api=vulkan --gpu-context=macvk` 才是"Metal 路径"（Vulkan 经 MoltenVK 落到 Metal）**。
- `video/out/mac_common.swift` + `video/out/mac/metal_layer.swift`：macOS 的 cocoa-cb 后端使用 `MetalLayer: CAMetalLayer`（`rgba16Float`、`edrMetadata`、`wantsExtendedDynamicRangeContent` → 有 HDR/EDR 能力）。
- `video/out/mac/gl_layer.swift`：仍存在 `GLLayer: CAOpenGLLayer`（CGL，走 `kCGLOGLPVersion_3_2_Core`），说明 libmpv 内部 OpenGL 路径在 macOS 上还在。

**【官方】libmpv render API 的 GPU 后端只有三种**（`include/mpv/render.h`，<https://github.com/mpv-player/mpv/blob/master/include/mpv/render.h>）：

```
#define MPV_RENDER_API_TYPE_D3D11 "d3d11"   // mpv 0.42 新增（API 2.6）
#define MPV_RENDER_API_TYPE_OPENGL "opengl"
#define MPV_RENDER_API_TYPE_SW "sw"         // 软件渲染
```

**结论：**
- `--vo=gpu` / `--vo=gpu-next` 在 macOS 历史上用 OpenGL，**现在只剩 Vulkan(MoltenVK)→Metal 一条**；mpv 0.37 起移除了 OpenGL cocoa 后端。
  - 证据：commit `fc4db187` "cocoa: remove OpenGL cocoa backend … deprecated in 0.29, lot of bugs, completely unmaintained, can't properly playback anything anymore on the newest macOS"（<https://github.com/mpv-player/mpv/commit/fc4db187d0ac538cb5cd9fcd7120f4ee25df4690>）
  - 历史运行日志印证：`[vo/gpu] opengl cocoa backend is deprecated, use vo=libmpv instead`（MacPorts ticket #69268，<https://trac.macports.org/ticket/69268>）
- **想让 libmpv 把视频画到你自己的纹理上（render API），macOS 上只能选 `MPV_RENDER_API_TYPE_OPENGL`**（或 `sw` 软件渲染兜底）。**没有 Metal/Vulkan 版 render API。**
- 不想自己管 GL 上下文，可以用 `--wid` 把 libmpv 的 layer 嵌进你的 NSView：此时走 `HAVE_MACOS_COCOA_CB` → `cocoa_init_cocoa_cb()` → MacCommon/MetalLayer，由 mpv 自己决定后端（现代构建即 Metal/MoltenVK）。
- `vo=libmpv` 在 macOS 上"表现得像普通 VO（cocoa-cb）"（`DOCS/man/vo.rst` 的 `libmpv` 条目），但它需要一个 render context；命令行直接 `vo=libmpv` 会报 `[vo/libmpv] No render context set.`（ticket #69268 实测）。

### 2.2 OpenGL 被 Apple 废弃 —— 还能用吗？

- **能用**：macOS 的 OpenGL 4.1/CGL 仍以 deprecated 状态存在，未被移除；`gl_layer.swift` 仍在用 `CAOpenGLLayer` + `CGLCreateContext`。**但**会有编译期 deprecated 警告，且 Apple 只做安全修复、不做性能/新特性。
- **实际影响（重要，别忽略）**：
  1. **无 Zero-Copy GPU 路径**：LibMpv 的 README 渲染矩阵里 macOS 是 `OpenGL / Zero-Copy GPU: No`（<https://github.com/mysteryx93/LibMpv-OpenGL>）
  2. **HDR/EDR 受限**：真正的 HDR/EDR 输出（`CAEDRMetadata`、`wantsExtendedDynamicRangeContent`）只在 MetalLayer 路径上。走 render API(OpenGL) → 自己的 GL 纹理 → 再交给 Metal 时，HDR 元数据基本要自己处理，通常退化到 SDR/颜色不精确。
  3. **hwdec**：macOS 只有 `videotoolbox`（直出，需要 `--vo=gpu|gpu-next`）和 `videotoolbox-copy`（`DOCS/man/options.rst`）。用 OpenGL render API 时通常要 `videotoolbox-copy` 或依赖 interop；`--vd-lavc-dr`（direct rendering）"`libmpv`: The libmpv render API has optional support"，在 macOS OpenGL 上不要指望。`--gpu-hwdec-interop` 的 auto 对 `libmpv` VO 等价于 `all`，会在建 GL 上下文时加载所有 interop，可能在 macOS 上引入额外开销/失败点。
  4. **颜色/ICC**：macOS 的 ICC/色彩管理依赖 cocoa-cb 路径（`VOCTRL_GET_ICC_PROFILE` 在 cocoa 后端实现）。走 render API 时 ICC 需要自己做 —— render API 只提供 `MPV_RENDER_PARAM_ICC_PROFILE` 让你**塞给** mpv。
- 行业现状参考：media_kit 的 macOS 实现就是"OpenGL 渲染到 OpenGL 纹理，再用 CVPixelBuffer 做 Metal 互操作"，README 里把"直接渲染到 Metal 纹理（用 ANGLE 规避 Apple 废弃的 OpenGL，或等 mpv 原生 Metal API）"列为 **possible improvements（待办）**（<https://github.com/media-kit/media-kit>）。

### 2.3 Homebrew 有没有 libmpv 包？

- **没有独立的 libmpv formula**。`brew install mpv`（formula，非 cask）会装 `mpv` 二进制 + 依赖；其 dylib 在 `$(brew --prefix)/lib/` 下（Apple Silicon：`/opt/homebrew/lib/libmpv.dylib`；Intel：`/usr/local/lib/libmpv.dylib`）。
  - 依据：Homebrew formula 页 License 标注 `GPL-2.0-or-later AND LGPL-2.1-or-later`，Binaries 只列 `mpv`（<https://formulae.brew.sh/formula/mpv>）；LibMpv 的说明也指向 `brew install mpv` 后取 dylib。
- **不能直接引用做分发**：把 `/opt/homebrew/lib/libmpv.dylib` 链进你的 .app 意味着依赖用户机器上的 Homebrew，且 dylib 的 install_name 指向 Homebrew 路径；正确做法是拷贝进 `Contents/Frameworks/` 并 `install_name_tool -change` 重写，同时把它的传递依赖一起打包（libass/libplacebo/ffmpeg 等一大堆 —— 这是最容易被忽略的工作量）。
- 另有 `iina/homebrew-mpv-iina`（IINA 的旧 tap），**已停止维护**，README 明确"From IINA 1.5.0, dependencies are compiled using iina/deps-buildscripts"（<https://github.com/iina/homebrew-mpv-iina>）。

### 2.4 公证（notarization）/ 代码签名

**【通用 Apple 规则 + 下述为工程结论，未找到 mpv 官方专门文档（标为【不确定】的部分已注明）】**

1. `.app` 内**所有** Mach-O（包括你塞进去的 `libmpv.dylib` 及其全部传递依赖 dylib）都必须被签名（Developer ID Application），并且使用 **hardened runtime** + **timestamp**。
2. 建议用 `codesign --deep` 之外的正确顺序：**先签 dylib（内层），再签 .app（外层）**；`--deep` 已不推荐作为唯一手段。
3. 公证要求：整个 bundle 用 `notarytool` 提交、stapling，且**开启 hardened runtime**。
4. **Hardened runtime 的坑（这条最容易炸）**：
   - mpv 在运行时可能需要 `dlopen` 已弃用的 `/System/Library/Frameworks/OpenGL.framework`（`context_cocoa.c` 历史实现就是这么做的）。若你的签名开启了 `com.apple.security.cs.disable-library-validation` 之外的限制，第三方/系统库加载可能被拒。
   - libmpv 会启动子进程（yt-dlp wrapper、subprocess 命令）。在 hardened runtime + App Sandbox 下，`Process`/fork 受限；**沙盒化应用基本要放弃 yt-dlp 功能**。
   - 走 `wid` 嵌入时 mpv 会 hook libmpv 的 cocoa 主线程事件，需要主线程/NSApplication 已初始化。
5. 分发形态建议：
   - 直接分发（Developer ID + 公证）：可行，dylib 全部签名即可。
   - Mac App Store：要 App Sandbox，mpv 的子进程/网络行为需要仔细处理，实践中很少这么做；IINA 就是 Developer ID 直分发。
6. **实际参考**：IINA 打包流程 —— `other/download_libs.sh` 拉预编译 dylibs，Xcode 里加到 "Copy Dylibs" 与 "Link Binary With Libraries" 阶段（<https://github.com/iina/iina>）。这就是"把 libmpv.dylib 及其依赖打进 .app"的标准做法。

---

## 3. iOS

### 3.1 能否构建使用

**能**，但**全是社区方案，官方零支持**。

- `media-kit/libmpv-darwin-build`（<https://github.com/media-kit/libmpv-darwin-build>）：
  - 提供 `ios-arm64`、`iossimulator-amd64/arm64/universal` × `audio/video` × `default/full/encodersgpl` 的 tar.gz 与 `.xcframework`
  - 最新 release 实测为 **v0.7.3**（GitHub Releases API）
  - 最低版本：iOS `9.0`（ffmpeg 要求），iOS 模拟器 arm64 需 `12.0`（`xcodebuild -create-xcframework` 要求）
  - 明示"compatible with commercial use for playback, and GPL use for encoding"（即 default/full 口味 = LGPL 可商用；`encodersgpl` 口味含 x264/GPL，不可商用）
- `NuvioMedia/MPVKit`（<https://github.com/NuvioMedia/MPVKit>，源于 mpvkit/MPVKit）：Swift Package，支持 ios/macos/tvos/visionos/isimulator/maccatalyst/xros，基于 mpv 0.41.0 / ffmpeg n8.1.1；LGPL 版与 GPL 版分开。
  - 其 README 有一句关键提示：**"Metal support only a patch version (#7857) and does not officially support it yet. Encountering any issues is not strange."** → 再次印证 macOS/iOS 的 Metal 支持非官方。
- `stps/mpv-ios-scripts`（<https://github.com/stps/mpv-ios-scripts>）：`development` / `distribution` 两套模式交叉编译 libmpv + ffmpeg + libass + freetype + harfbuzz + fribidi + uchardet（arm64 / x86_64）。

### 3.2 视频输出如何做

- **render API 可用**：`MPV_RENDER_API_TYPE_OPENGL` 支持 OpenGL ES 2.0+（render.h 原文："OpenGL desktop 2.1 or later … or OpenGLES 2.0 or later"）。iOS 上用 EAGL/CAEAGLLayer 或 `CVPixelBuffer`+Metal 互操作。
- **`sw` 软件渲染**可用但慢，HDR/子母画面等要自己处理。
- **`wid` 嵌入**：`wid` 官方只声明 "works on X11, win32, and macOS only"（`include/mpv/client.h`）。iOS 不在列 —— 实践中 iOS 走 render API 更常见。
- media_kit 的描述最直接：**"On iOS… iOS shares much of its implementation with macOS. Only difference is that OpenGL ES is used instead of OpenGL."**（<https://github.com/media-kit/media-kit>）
- 现成 .NET 侧参考：`vrjure/LibMPV.AutoGen` 仓库里**有 `LibMPVSharp.Avalonia.Demo.iOS` 工程**（Avalonia + iOS），说明这条路有人走过（<https://github.com/vrjure/LibMPV.AutoGen/tree/master/LibMPVSharp.Avalonia.Demo.iOS>）。但该 .NET 绑定 2025-05 后未再推送，需自行验证。

### 3.3 App Store 审核已知问题

**【不确定／社区经验，未见 mpv 官方结论】**

- 未见 libmpv 被 App Store 明确禁止的官方文档；主要风险来自工程与政策层面：
  1. **decoder/DRM**：内置完整 FFmpeg 解码器本身不违规（VLC/Infuse 类应用存在），但播放受 DRM 保护内容会触发 FairPlay 要求。
  2. **动态库加载**：App Store 版本必须使用**静态库或内嵌 framework**（不能再从网络下载可执行代码），因此必须把 libmpv 编成静态库 / xcframework 一起提交 —— media-kit 与 MPVKit 的做法正是如此。
  3. **后台播放 / PiP**：需要相应 entitlement 与 AVAudioSession 配置，否则审核会挑。
  4. **YouTube/yt-dlp 类功能**：会引发"下载受版权内容"审核问题，建议不打包 yt-dlp。
  5. **Bitcode**：`stps/mpv-ios-scripts` 的 distribution 模式曾加 bitcode；Apple 已废弃 bitcode 要求，新版 Xcode 无需。
- 现实参考：Flutter 的 media_kit 明确列出 iOS 9+ 支持并给出测试包，说明 iOS 上 libmpv 工程可行。

---

## 4. Android

### 4.1 libmpv-android 现状

**【官方仓库自述】**（<https://github.com/mpv-android/mpv-android>）：

> "**mpv-android is not a library/module (AAR) you can import into your app.** If you'd like to use libmpv in your app you can use our code as inspiration. The important parts are `MPVLib`(kotlin), `BaseMPVView`(kotlin) and the native code (`app/src/main/jni/`). Native code is built by these scripts (`buildscripts/`)."

即：**它只提供 APK + Java/Kotlin 绑定 + 参考 JNI 实现，不提供 AAR，也没有给 .NET 用的东西。**

构建要求（`buildscripts/README.md`）：
- 仅支持 Linux / macOS 编译原生部分，**Windows（含 WSL）不支持**
- `./download.sh` 装 Android SDK/NDK 并拉源码；`./buildall.sh`；默认只出 32-bit ARM，需要显式 `--arch arm64 / x86 / x86_64`
- 产物是 `.so`（`libmpv.so` 等），可直接被 Java/Kotlin 或 .NET Android 的 `lib/<abi>/` 使用

### 4.2 Android 上的渲染方式（源码事实）

- `app/src/main/jni/render.cpp` 只做一件事：`mpv_set_option(g_mpv, "wid", MPV_FORMAT_INT64, &wid)`，`wid` = `android.view.Surface*` 的 JNI 全局引用指针。
  → **主流做法是 `--wid` + `Surface`**，配合 `--hwdec=mediacodec --vo=mediacodec_embed`（或 `--vo=gpu --gpu-context=android` 走 OpenGL）。
- 另有 render API 路线：`MPV_RENDER_API_TYPE_OPENGL` + OpenGL ES 渲染到 `SurfaceTexture`（media_kit Android 就是"设 `--wid` 到 `android.view.Surface`"的路线）。
- 官方文档对应条目：`mediacodec_embed`（`DOCS/man/vo.rst`）、`--gpu-context=android`（`DOCS/man/options.rst`）。

### 4.3 .NET MAUI / Avalonia Android 集成可行性

**结论：可行但难度不低，且没有现成成熟包。**

- 需要的工作：
  1. 交叉编译出 `libmpv.so`（含 ffmpeg 等）→ 放进 `.Android` 项目的 `lib/<abi>/`（用 `AndroidNativeLibrary` / `@(AndroidNativeLibrary)` 引用）
  2. 用 JNI（`Java.Lang.Object` / `Android.Runtime.JNIEnv`）拿到 `Surface` 或 `SurfaceView`/`SurfaceTexture` 的句柄，`mpv_set_option("wid", ptr)`
  3. P/Invoke 到 `libmpv.so`（`DllImport("mpv")`），ABI 是 `arm64-v8a`/`armeabi-v7a`/`x86_64`
- **现成资产（社区）**：
  - `mysteryx93/LibMpv-OpenGL` 仓库内置 `MpvDll/android-arm`、`android-arm64`、`android-x64` 本地库，且 `Src/AndroidSample` 是现成范例，`Src/LibMpv.Avalonia/NativeViewAndroid.cs` 是 Avalonia Android 的嵌入实现（<https://github.com/mysteryx93/LibMpv-OpenGL>）。
    但要注意：其 README 的"Renderers"表只列了 Windows/macOS/Linux（macOS 标 OpenGL、Zero-Copy GPU = No），**没有明确承诺 Android 支持**，Android 属于"仓库里有实现、文档未承诺"的状态 → **【不确定，需自行验证】**。
  - `vrjure/LibMPV.AutoGen` 有 `LibMPVSharp.Avalonia.Demo.Android`，NuGet `LibMPVSharp.Avalonia` 0.0.1-beta，但该包 **2025-04-12 后未更新**。
- 难度评估：中高。主要成本在（a）NDK 交叉编译与 ABI 矩阵、（b）Surface 生命周期与 JNI 引用管理、（c）Avalonia 的 Android 视图嵌入（TextureView/SurfaceView 与 Avalonia 渲染层的合成）。**若只想快速出货，Android 上考虑 ExoPlayer/Media3 或商业 SDK 可能更划算。**

---

## 5. Windows / Linux

### 5.1 Windows

- **需要什么**：`libmpv-2.dll`（x64/x86），来源 shinchiro/zhongfly 的 `mpv-dev-*.7z`，或随 mpv 安装包。
- **渲染方式**：
  - **render API**：`MPV_RENDER_API_TYPE_OPENGL`（实践中配合 ANGLE，因为要把 GL ES 调用翻译到 D3D11，见 media_kit Windows 的实现说明）；或 mpv 0.42 起新增的 `MPV_RENDER_API_TYPE_D3D11` + `mpv_d3d11_init_params` / `MPV_RENDER_PARAM_D3D11_FBO`（`render_d3d11.h`，API 2.6，见 `DOCS/client-api-changes.rst`）—— **这是 .NET/WPF/WinUI 最理想的零拷贝路径，但要求 mpv ≥ 0.42 且绑定必须暴露它**（目前主流 .NET 绑定尚未暴露 D3D11 后端 → 需自己 P/Invoke 或等更新）。
  - **`wid` 嵌入 HWND**：`mpv_set_option("wid", hwnd)`，最简单，但焦点/全屏/输入事件要自己捋；官方示例 `libmpv/csharp` 就是这条路线（"Uses Platform Invoke … native window embedding to show the video in a Windows Forms control"，<https://github.com/mpv-player/mpv-examples/tree/master/libmpv>）。
- **hwdec**：`d3d11va`（需要 `--vo=gpu|gpu-next` + `--gpu-context=d3d11|angle`）、`dxva2`。

### 5.2 Linux

- **需要什么**：`libmpv.so.2`；发行版 `libmpv-dev`/`mpv-libs`，或 `mpv-build` 自编；注意 Ubuntu 仓库版本常 <0.40（LibMpv 建议用 PPA `ubuntuhandbook1/mpv`）。
- **渲染方式**：
  - render API OpenGL：X11 用 GLX（`--gpu-context=x11`）或 EGL；Wayland 用 `--gpu-context=wayland`；render API 还支持 `MPV_RENDER_PARAM_X11_DISPLAY` / `WL_DISPLAY` / `DRM_DISPLAY_V2` 等（hwdec interop 需要）。
  - `wid` 嵌入 X11 窗口（`mpv_set_option("wid", xid)`）。
  - Zero-Copy GPU：LibMpv 矩阵里 Linux 是 `OpenGL w/ EGL / Yes`（<https://github.com/mysteryx93/LibMpv-OpenGL>）。

---

## 6. .NET 绑定库现状

> 数据来源：NuGet V3 搜索/注册 API（实时查询）+ GitHub API + 各仓库 README

| 库 | 最新版 / 最后更新 | 平台 | Avalonia | 暴露 render API？ | 活跃度 |
|---|---|---|---|---|---|
| **HanumanInstitute.LibMpv** | 0.10.1（累计 5,920 下载） | Win/macOS/Linux（+ 仓库内有 android 本地库） | 有（`.Avalonia` 包） | ✅ **是**，含 OpenGL / Native(wid) / Software 三种 renderer | **活跃**（仓库 pushed 2026-07，61★，MIT） |
| **HanumanInstitute.LibMpv.Avalonia** | 0.10.1（5,975 下载） | 同上 | ✅ 专门为 Avalonia 12 | ✅（通过上面） | 活跃 |
| **LibMPVSharp**（vrjure/LibMPV.AutoGen） | 0.0.2-beta（1,320 下载） | Win/macOS/Linux/Android（demo 还含 iOS） | 有（`.Avalonia` 0.0.1-beta，230 下载） | ✅ 是（仓库含 `Render.cs` 35KB、`Render_gl.cs`） | **半停滞**：仓库 pushed 2025-05-13，Avalonia 包 2025-04-12 后无新版 |
| **Mpv.NET**（hudec117/Mpv.NET-lib-） | 1.1.1（52,663 下载，**已被标记 deprecated / Legacy**） | **仅 Windows**（WinForms/WPF） | ❌ | ❌（走 `wid` 嵌入 HWND） | 已弃用 |
| **mpv.net**（stax76 / mpvnet-player） | 现为**播放器应用**（不是库） | Windows | ❌ | ❌ | 活跃（但形态是 App + 扩展 API） |
| **HanumanInstitute.MpvIpcController**（NuGet id `MpvIpcController`） | 1.0.0（97 下载） | 跨平台 | N/A | ❌ **完全是 JSON IPC**：启动/连接外部 mpv 进程（Named Pipe / Unix socket），不是 libmpv 嵌入 | 维护中，但**不满足"嵌入 + render API"需求** |
| 其他可参考 | `LibMpv.Client`/`LibMpv.Avalonia`/`LibMpv.MVVM`（jinyuttt，1.0.0，文档几乎空白）；`Richasy.MpvKernel`（Win 向 libmpv-2.dll interop）；`YALibMPV.PInvoke.*`（0.1.0，"very untested"）；`Nickvision.MPVSharp`（2023.9.0，Legacy）；`MediaToolkitNet.Mpv`（0.3.1，不带 native） | | | | 不推荐作为主选 |

### 逐条回答"哪些支持 macOS / Avalonia / render API"

- **支持 macOS**：`HanumanInstitute.LibMpv(.Avalonia)` ✅（README 明示 Windows/macOS/Linux，macOS 用 OpenGL、Zero-Copy GPU = No）；`LibMPVSharp` ✅（README 有 Mac 截图，测试于 mac mini m4）；`Mpv.NET` ❌（仅 Windows）
- **支持 Avalonia**：`HanumanInstitute.LibMpv.Avalonia` 0.10.1（**推荐**，Avalonia 12 / .NET 10）；`LibMPVSharp.Avalonia` 0.0.1-beta（旧，Avalonia 11.2.3 时代）
- **暴露 `mpv_render_context`（render API）**：
  - ✅ `HanumanInstitute.LibMpv`（0.10.1）："MPV implementation for .NET including OpenGL, Native and Software renderers"，渲染矩阵见其 README
  - ✅ `LibMPVSharp`：`Render.cs` / `Render_gl.cs`（CppSharp 自动生成，覆盖 render.h / render_gl.h）
  - ❌ `Mpv.NET` 1.1.1、❌ `MpvIpcController`、❌ `mpv.net`

### 推荐决策（.NET / Avalonia）

1. **首选 `HanumanInstitute.LibMpv` + `HanumanInstitute.LibMpv.Avalonia` 0.10.1**（Desktop 三平台，render API 齐全）。约束：**.NET 10+ / Avalonia 12 / libmpv ≥ 0.40**，macOS 需要你自己准备 dylib。
2. 若必须支持 **Avalonia Android/iOS**：走 `LibMPVSharp`（有 Android/iOS demo + Android 本地库）或**自己在 LibMpv 之上补一个平台 View**（Android 用 `wid`+Surface，iOS 用 OpenGL ES render API）。
3. **不要**把 `MpvIpcController` 当作嵌入方案（它是外部进程 IPC）。

---

## 7. 许可证

### 7.1 libmpv / mpv 本体（**官方** `Copyright`，<https://github.com/mpv-player/mpv/blob/master/Copyright>）

- **默认 = GPLv2+**。用 `-Dgpl=false` 配置可构建为 **LGPLv2.1+**。
- 原文关键点：*"mpv as a whole is licensed under the GPL version 2 or later by default. The mpv program is licensed the LGPL version 2 or later if built without using any GPL only files."*
- **警告（原文）**：*"the build system is provided 'as is' and using the `-Dgpl=false` configure switch does not in itself create a LGPLv2.1+ license grant."* → 你必须自己确认构建产物里没有 GPL-only 文件。
- `-Dgpl=false` 会禁用一批功能（原文列出），与你的场景高度相关的：
  - Linux **X11 video output**（`vo_x11.c` / `vo_xv.c`）→ 若你要在 Linux 上开窗口，LGPL 模式下这条受限（但 **libmpv 嵌入 + render API 不受影响**）
  - **NVIDIA/Linux 硬件解码 `vdpau`**（`nvdec` 通常仍可用）
  - `vo_direct3d`（Windows 传统 D3D VO）、jack、DVD/CDDA、DVB、CACA
  - `DOCS/man/`（文档）仍是 GPLv2+
- **客户端 API 头文件是 ISC**（`include/mpv/client.h` 与 `render.h` 顶部，宽松许可，明确允许写外部 wrapper）：*"the client API is licensed under ISC … to enable other wrappers outside of mpv. But keep in mind that the mpv core is by default still GPLv2+ - unless built with -Dgpl=false, which makes it LGPLv2+."*
- **链接的库会影响最终许可**（原文）：*"Linked libraries still can affect the final license (for example if FFmpeg was built as GPL)."*

### 7.2 动态链接（dylib/dll/so）进闭源或开源应用的要求

| 场景 | 要求 |
|---|---|
| 用 **LGPLv2.1+ 构建的 libmpv**，**动态链接** | 允许闭源商用。LGPL 要求：① 动态链接（不静态链接）；② 提供 libmpv 的可替换能力（用户能替换动态库）；③ 附 LGPL 许可文本与版权声明；④ 若修改了 mpv 源码，需公开修改部分。 |
| 用 **LGPL 构建的 libmpv**，**静态链接** | 需要提供**可重新链接的目标文件/足够信息**（relinkable object files 或等效机制），实践中很麻烦 → **强烈建议动态库形态**。 |
| 用 **默认（GPLv2+）构建的 libmpv** | 你的整个应用需以 GPLv2+ 兼容许可发布 → **闭源商用不可行**。 |
| **实际发布陷阱** | 即使 mpv 是 `-Dgpl=false`，**FFmpeg 若用 `--enable-gpl` 编译也会让整体变 GPL**。务必使用 LGPL 版 FFmpeg（不加 `--enable-gpl --enable-nonfree`）。 |

### 7.3 现成二进制/包的许可

- **media-kit/libmpv-darwin-build**（README"Commercial use"表最清晰）：
  - `default` / `full` 口味：mpv `LGPL-2.1 (-Dgpl=false)`、ffmpeg `LGPL-2.1 (--enable-gpl & --enable-nonfree omitted)`、libass ISC、freetype/harfbuzz/fribidi/libpng/mbedtls/uchardet/libxml2/dav1d → **全部 ✅ 可商用**
  - `encodersgpl` 口味：ffmpeg 变 `GPL-2.1`、含 `libx264 (GPL-2.0+)` → **❌ 不可商用**
- **MPVKit**：源码 LGPL v3；默认 bundle LGPL v3；`enable-gpl` 或 `-GPL` 后缀产物变 GPL v3（<https://github.com/NuvioMedia/MPVKit>）
- **Homebrew mpv**：`GPL-2.0-or-later AND LGPL-2.1-or-later` → 取它当分发包**默认按 GPL 对待**
- **.NET 绑定本身**：`HanumanInstitute.LibMpv` / `LibMpv-OpenGL` 是 **MIT**；`LibMPVSharp`/`LibMPV.AutoGen` 是 **MIT**；`MpvIpcController` 是 MIT → 绑定许可宽松，**约束全部来自 libmpv/ffmpeg 本身**

---

## 8. 行动建议（针对 .NET/Avalonia 落地）

1. **确定目标平台集合**：桌面三平台 → 直接上 `HanumanInstitute.LibMpv(.Avalonia)`；含移动端 → 需自建平台 View（或改用 LibMPVSharp 验证 Android/iOS）。
2. **自己准备 LGPL 版二进制并固定版本**：
   - Windows：shinchiro/zhongfly `mpv-dev-*.7z` 的 `libmpv-2.dll`
   - macOS：media-kit `libmpv-libs_*_macos-universal-video-default.tar.gz`（`default` 口味才可商用），或自编 `-Dgpl=false` + LGPL ffmpeg
   - Linux：`libmpv.so.2`（版本 ≥ 0.40）
   - Android/iOS：自编或 media-kit/MPVKit 产物（注意口味与 LGPL/GPL）
3. **macOS 渲染选型提前定**：
   - 需要 HDR/最佳性能 → 走 `wid` 把 mpv 的 MetalLayer 嵌进 NSView（放弃 render API 的纹理控制）
   - 需要自己合成纹理/叠加 UI → 走 render API + OpenGL（接受无 Zero-Copy、HDR 受限、hwdec 需 copy）
4. **打包/签名清单（macOS）**：收集 dylib 全依赖树 → 拷进 `Contents/Frameworks` → `install_name_tool` 重写 → 内到外 codesign（hardened runtime + timestamp）→ notarytool + staple。
5. **许可证合规**：随包附 LGPLv2.1 文本 + mpv/ffmpeg 版权声明 + 你的构建配置（证明 `-Dgpl=false`、ffmpeg 无 `--enable-gpl`）；保持动态库形态。
6. **尽快做一个 spike**：在 macOS 上验证 `render API(OpenGL) + videotoolbox-copy` 的实际 4K 播放性能与色彩表现 —— 这是最容易出现"结论与预期不符"的地方。

---

## 9. 主要来源链接

**官方**
- mpv 安装页：<https://mpv.io/installation/>
- mpv 手册（OPTIONS / VO）：<https://mpv.io/manual/master/>
- `include/mpv/render.h`（后端清单、线程规则、SW 渲染）：<https://github.com/mpv-player/mpv/blob/master/include/mpv/render.h>
- `include/mpv/client.h`（嵌入方式、wid 平台限制）：<https://github.com/mpv-player/mpv/blob/master/include/mpv/client.h>
- `DOCS/man/vo.rst`（gpu/gpu-next/libmpv/mediacodec_embed）：<https://github.com/mpv-player/mpv/blob/master/DOCS/man/vo.rst>
- `DOCS/man/options.rst`（hwdec 与 gpu-context 约束、vd-lavc-dr）：<https://github.com/mpv-player/mpv/blob/master/DOCS/man/options.rst>
- `DOCS/client-api-changes.rst`（0.42 新增 D3D11 render API，API 2.6）：<https://github.com/mpv-player/mpv/blob/master/DOCS/client-api-changes.rst>
- `Copyright`（GPL/LGPL 条款）：<https://github.com/mpv-player/mpv/blob/master/Copyright>
- `video/out/gpu/context.c` / `video/out/vulkan/context_mac.m` / `video/out/mac_common.swift` / `video/out/mac/gl_layer.swift`
- 移除 OpenGL cocoa 后端 commit：<https://github.com/mpv-player/mpv/commit/fc4db187d0ac538cb5cd9fcd7120f4ee25df4690>
- libmpv 示例（嵌入方式对比，含 csharp）：<https://github.com/mpv-player/mpv-examples/tree/master/libmpv>

**macOS / iOS 社区**
- media-kit/libmpv-darwin-build：<https://github.com/media-kit/libmpv-darwin-build>
- NuvioMedia/MPVKit（含 Metal 非官方说明）：<https://github.com/NuvioMedia/MPVKit>
- stps/mpv-ios-scripts：<https://github.com/stps/mpv-ios-scripts>
- iina/iina（打包流程）、iina/deps-buildscripts、iina/homebrew-mpv-iina（已停维）
- MacPorts ticket #69268（cocoa OpenGL 弃用日志实测）：<https://trac.macports.org/ticket/69268>
- Homebrew mpv formula：<https://formulae.brew.sh/formula/mpv>

**Android**
- mpv-android（"not a library/AAR"）：<https://github.com/mpv-android/mpv-android>
- mpv-android buildscripts：<https://github.com/mpv-android/mpv-android/tree/master/buildscripts>
- mpv-android JNI render（wid+Surface）：<https://github.com/mpv-android/mpv-android/blob/master/app/src/main/jni/render.cpp>
- media-kit Android 说明（wid + SurfaceTexture）

**Windows 二进制**
- shinchiro/mpv-winbuild-cmake releases：<https://github.com/shinchiro/mpv-winbuild-cmake/releases>
- zhongfly/mpv-winbuild releases：<https://github.com/zhongfly/mpv-winbuild/releases>
- 官方 CI：<https://github.com/mpv-player/mpv/releases/tag/git-release>

**.NET 绑定**
- HanumanInstitute.LibMpv：<https://www.nuget.org/packages/HanumanInstitute.LibMpv>
- HanumanInstitute.LibMpv.Avalonia：<https://www.nuget.org/packages/HanumanInstitute.LibMpv.Avalonia>
- 源码 mysteryx93/LibMpv-OpenGL（渲染矩阵、MpvDll 平台目录）：<https://github.com/mysteryx93/LibMpv-OpenGL>
- LibMPVSharp / LibMPV.AutoGen：<https://github.com/vrjure/LibMPV.AutoGen>
- NuGet LibMPVSharp：<https://www.nuget.org/packages/LibMPVSharp>
- Mpv.NET（deprecated）：<https://www.nuget.org/packages/Mpv.NET>
- MpvIpcController：<https://www.nuget.org/packages/MpvIpcController>、<https://github.com/mysteryx93/MpvIpcController>
- Endpne.LibMPV.Windows（Windows native 包）：<https://www.nuget.org/packages/Endpne.LibMPV.Windows>
