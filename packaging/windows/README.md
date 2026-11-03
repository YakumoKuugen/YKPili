# Windows 打包说明

把 `app/crates/app` 的 `ykpili` 打成 Windows 绿色包与 NSIS 安装包。

## 一键打包

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package-windows.ps1
```

产物：

- `dist/YKPili-<版本>-windows-x64/`（解压即用的目录）
- `dist/YKPili-<版本>-windows-x64.zip`
- `dist/YKPili-<版本>-windows-x64-setup.exe`（NSIS 安装包）

脚本做五件事：

1. `cargo build --release -p yk-app --bin ykpili`；
2. 按 `packaging/libmpv/features.toml` 裁剪编译 `libmpv-2.dll`（已有最新产物则跳过），
   并校验指纹 + 体积 + 导出符号；
3. 建暂存目录，放 exe、`libmpv-2.dll`、它的运行时依赖、许可证、说明；
4. 用 7-Zip（或 `Compress-Archive`）压成 zip；
5. 调用便携版 NSIS 生成 `setup.exe`（找不到 `makensis` 时跳过并提示）。

## 常用参数

```powershell
.\scripts\package-windows.ps1 -SkipBuild          # 复用已有 release 产物
.\scripts\package-windows.ps1 -DebugBuild         # debug 构建，验证打包流程
.\scripts\package-windows.ps1 -NoInstaller        # 只出 zip
.\scripts\package-windows.ps1 -Version 0.1.0      # 覆盖版本号
```

## 工具链

- Rust **1.95+**（`gpui-fast` 使用了 `std::hint::cold_path`，1.94 会编译失败）。
  脚本优先用 `cargo +1.99.1`，没有该工具链时退回默认 `cargo` 并检查版本。
- Visual Studio 2022 + Windows SDK（`rc.exe`，用于 exe 图标与版本信息）。
- 7-Zip（可选，压缩率更好）。
- NSIS 便携版：解压到 `packaging/windows/tools/nsis-*/Bin/makensis.exe`。

## libmpv

**不再直接使用上游全量 `mpv-dev` DLL**，而是按
[`packaging/libmpv/features.toml`](../libmpv/features.toml) 裁剪编译，只包含
YKPili 播放链路启用的编解码器/解复用器/滤镜/音频输出。

- 编译：`./scripts/build-libmpv.sh --platform windows`（MSYS2 UCRT64）；
  打包脚本默认会自动调用，不需要额外参数；
- 放置：`packaging/windows/vendor/libmpv-2.dll` + `libmpv-2.dll.build.json`（指纹），
  以及 FFmpeg 共享库（`avcodec-*.dll` 等）、`libass-*.dll` / `libplacebo-*.dll`、
  `libmpv-deps.txt`；
- 校验：`python scripts/libmpv_features.py verify --platform windows --file ...`；
- 运行时查找顺序：`YK_LIBMPV` → exe 同目录 → `%ProgramFiles%\mpv` → PATH。

完整流程与「为什么不能直接裁预编译 DLL」见
[`packaging/libmpv/README.md`](../libmpv/README.md)。

### 没有裁剪产物 / 工具链缺失

打包脚本默认就会调用 `scripts/build-libmpv.sh` 重编，不需要额外参数：

- **没装 MSYS2**：自动下载固定版本并解压到 `%LOCALAPPDATA%\ykpili\msys64`，
  再用 pacman 装 gcc/meson/libass/libplacebo 等（首次约 400–600 MB，之后复用）；
- 已经装了 MSYS2（`MSYS2_ROOT` / `C:\msys64`）：直接用，只补缺的包；
- 已有裁剪产物且指纹匹配：跳过编译，直接复用；
- 强制重编：`./scripts/build-libmpv.sh --platform windows --force`（或 `--clean`）。

安装过程也可以单独跑：
`powershell -ExecutionPolicy Bypass -File .\scripts\install-msys2.ps1`。

实测体积（2026-11，MSYS2 UCRT64 构建）：`libmpv-2.dll` 本体 **2.8 MB**，加上
FFmpeg 共享库、libass/libplacebo/harfbuzz 等运行时 DLL 共 **约 41 MB**；
同版本全量单个 DLL 是 121 MB。zip 体积约为全量构建的一半左右（debug 包实测
30.3 MB；release 会更小）。

## 许可证

裁剪版把 mpv（GPL-2.0-or-later）与 FFmpeg（未启用 GPL 编码器，LGPL-2.1-or-later）
静态链进 `libmpv-2.dll`，另带 libass / libplacebo 等 LGPL 动态库。随包必须附：

- `licenses/GPL-2.0.txt`
- `licenses/MPV-Copyright.txt`
- `licenses/SOURCE.txt`（源码获取方式与构建清单）

对外分发时请确认整体许可符合 GPL；动态链接 libmpv 不能规避 GPL 构建的义务。

## 图标

`packaging/windows/ykpili.ico` 由 `packaging/macos/AppIcon.icns` 的 1024px 图
转换而来（16/24/32/48/64/128/256 多尺寸）。重新生成：

```powershell
python -c "from PIL import Image; im=Image.open('packaging/macos/AppIcon.icns').convert('RGBA'); im.save('packaging/windows/ykpili.ico', sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])"
```

`app/crates/app/build.rs` 用 `winresource` 把图标与版本信息写进 exe。

## 安装包行为

- 全新安装默认装到 `%LOCALAPPDATA%\Programs\YKPili`，不触发 UAC；
- **升级时默认沿用上次安装目录**：`.onInit` 依次读
  `HKCU\Software\YKPili\InstallDir`、卸载表的 `InstallLocation`、
  常见目录，最后扫描所有固定磁盘的 `X:\Program Files\YKPili` /
  `X:\Program Files (x86)\YKPili` / `X:\YKPili`（只认带 `ykpili.exe` 的目录）。
  用户显式传 `/D=...` 时不会被覆盖；
- 开始菜单 + 桌面快捷方式；
- 卸载程序在安装目录，注册表写在 `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili`；
- 卸载不删除 `%APPDATA%\YKPili` 与缓存。

## 常见问题

- **找不到 libmpv**：确认 `libmpv-2.dll` 与 `ykpili.exe` 同目录；或设 `YK_LIBMPV`。
- **窗口全黑 / 没有画面**：看 debug 构建的控制台日志里 `[video]` 与 `[mpv]`；
  确认显卡支持 D3D11，远程桌面/虚拟机可能没有硬件加速。
- **rc.exe 找不到**：安装 Visual Studio 2022 的“使用 C++ 的桌面开发”工作负载。
