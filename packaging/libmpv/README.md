# libmpv 裁剪（只编译启用的功能）

随包分发的 `libmpv-2.dll` / `libmpv.2.dylib` 由 YKPili 自己按
[`features.toml`](features.toml) 编译，不再直接塞上游全量构建。

## 为什么要自己编

libmpv 是**单体库**：上游 `mpv-dev`（shinchiro）把 FFmpeg 的全量编解码器、滤镜、
`yt-dlp`、DVD/蓝光、Lua/JS 脚本、各种音频/视频输出全编进一个 121 MB 的 DLL。
应用实际只用到：

| 能力 | 应用里的出处 |
| --- | --- |
| `vo=libmpv` + `MPV_RENDER_API_TYPE_SW` | `app/crates/yk-media/src/lib.rs` |
| `ao=wasapi` / `avfoundation` | 同上（`YK_AO` 可覆盖） |
| `hwdec=auto-copy-safe` / `auto` | 同上，帧要回读到 CPU，只用 copy-back |
| H.264 / HEVC / AV1，AAC / EC-3 / FLAC | 哔哩哔哩 `playurl` 的 DASH 编码 |
| `loadfile` + `audio-add`（独立音轨） | 同上 |
| HTTP(S) 直链 + Referer/UA | 同上 |

从预编译 DLL 里删不掉这些用不到的代码（PE 里改不了链接结果），唯一可行的做法是
`--disable-everything` 重编。清单里没列的能力一律不编译。

## 一键构建

### Windows（MSYS2 UCRT64）

**不需要手动装 MSYS2**：打包或运行 `build-libmpv.sh` 时，脚本发现没有工具链会
自动调用 [`scripts/install-msys2.ps1`](../../scripts/install-msys2.ps1)：

- 下载固定版本 `msys2-base-x86_64-20260927.sfx.exe`（SHA256 校验）；
- 静默解压到 `%LOCALAPPDATA%\ykpili\msys64`（不需要管理员权限）；
- 初始化 keyring / 包库，用 pacman 装 gcc、meson、ninja、pkgconf、nasm、
  libass、libplacebo、harfbuzz、freetype、fribidi、dav1d（首次约 400–600 MB）；
- 把根目录记到 `%LOCALAPPDATA%\ykpili\msys2-root.txt`，之后直接复用。

已经装过 MSYS2 的话优先用现成的：`MSYS2_ROOT` → 状态文件 → `C:\msys64` →
`%LOCALAPPDATA%\ykpili\msys64`。只补缺的包，不会重复下载。

```powershell
# 打包（会自动准备工具链 + 裁剪 libmpv）
.\scripts\package-windows.ps1
```

想单独跑安装/编译也可以：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-msys2.ps1
```

产物：`packaging/windows/vendor/libmpv-2.dll` + `libmpv-2.dll.build.json`
（指纹），以及共享版 FFmpeg（`avcodec-*.dll` / `avformat-*.dll` / …）、
`libass-*.dll` / `libplacebo-*.dll` 等运行时依赖和 `libmpv-deps.txt` 清单。
打包脚本会把它们一起装进绿色包/安装包。

### macOS

```bash
brew install meson ninja pkgconf nasm libass libplacebo harfbuzz freetype fribidi dav1d
./scripts/build-libmpv.sh --platform macos   # package-macos.sh 打包时会自动调用
```

产物：`packaging/macos/vendor/libmpv.2.dylib`。打包脚本仍用
`scripts/bundle-libmpv.py` 把 Homebrew 依赖闭包（libass/libplacebo/…）拷进
`Contents/Frameworks`，并把 install_name 改写成 `@loader_path/…`。

### 只想看会执行什么

```bash
./scripts/build-libmpv.sh --print-only            # 打印下载/configure/meson 命令
python3 scripts/libmpv_features.py plan --platform windows   # 打印启用了哪些能力
```

## 构建参数从哪来

`scripts/libmpv_features.py` 把清单翻译成两套参数，构建脚本不再手写开关：

- **FFmpeg**：`--disable-everything` 起步，只 `--enable-decoder/demuxer/parser/
  protocol/filter/bsf/hwaccel` 清单里的项；TLS 用系统后端（Windows `schannel`，
  macOS `securetransport`）。链接形态：Windows 共享 DLL（省掉 `pkg-config --static`
  的私有依赖坑），macOS 静态进 `libmpv.dylib`。
- **mpv**：`-Dlibmpv=true -Dcplayer=false`，关掉 lua/js/cplugins、DVD/蓝光、
  GL/Vulkan/VAAPI/CUDA、除目标平台外的所有 AO/VO；macOS 额外关掉
  `cocoa`/`swift-build`（不用 `wid`，也就不需要 Swift 工具链）。

清单与参数都会参与**指纹**计算：改了 `features.toml` 而没重新编译，打包脚本
会直接指出来（见下）。

## 打包时的校验

```bash
python3 scripts/libmpv_features.py verify --platform windows \
    --file packaging/windows/vendor/libmpv-2.dll
```

| 退出码 | 含义 | 打包脚本行为 |
| --- | --- | --- |
| 0 | 指纹、体积、关键导出都正常 | 直接复用，继续打包 |
| 2 | 没有 `.build.json`（没跑过裁剪构建） | 自动调用 `build-libmpv.sh` 重编 |
| 3 | 清单/构建参数改过但产物没重建，或文件被替换 | 自动重编 |
| 4 | 体积超过清单预算 | 自动重编；仍超预算则终止 |
| 5 | 不是可加载的库或缺 `mpv_*` 导出 | 自动重编；仍失败则终止 |

Windows 打包脚本在打包开头自动调用 `build-libmpv.sh`；产物的指纹/体积/导出
校验不通过就直接终止打包（不会悄悄退回全量 DLL）。macOS 同理。

已有最新裁剪产物时会跳过编译，重复打包不会每次都重编。要强制重编：

```bash
./scripts/build-libmpv.sh --platform windows --force
# 或彻底重来
./scripts/build-libmpv.sh --platform windows --clean
```

## 加/减功能

1. 先在应用代码里找到真实调用点，改 `features.toml` 并写清出处；
2. 重新跑 `build-libmpv.sh`；
3. `verify` 通过后，打包即可。

不要为了“以后可能用到”加解码器：每加一个都在给所有用户增加下载体积。

## 已知限制

- **裁剪版只服务 YKPili 的播放链路**：没有 `cplayer`（`mpv.exe`）、没有
  `yt-dlp`、没有截图/录屏、没有本地 DVD/蓝光。要这些功能请用上游全量构建。
- macOS 关闭了 `cocoa`/`cocoa-cb`：只能走 render API（`sw`），不支持 `--wid`
  嵌窗口；视频帧回读 CPU 本来就只需要 copy-back 硬解。
- Windows 裁剪版是**一组 DLL**（libmpv + 共享版 FFmpeg + libass/libplacebo 等），
  不是单个文件。实测：libmpv 本体 2.8 MB，全部运行时 DLL 合计约 41 MB
  （原全量单 DLL 121 MB）。NSIS 用 `File /r` 整目录打包，不需要改文件列表。
  `size_budget_mb` 只约束 libmpv 本体，依赖体积看打包输出的总计。
- 依赖里 `libshaderc_shared.dll`（~10 MB）来自 MSYS2 的 libplacebo；应用走软件
  渲染用不到它，但 libplacebo 的动态库硬链接了它，暂时只能随包带。
- 许可证：mpv 是 GPL-2.0-or-later，FFmpeg 本次为 LGPL-2.1-or-later（未启用 GPL
  编码器），libass/libplacebo 为 LGPL。随包需带 `LICENSES/` 下的文本。
