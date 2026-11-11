# YKPili

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

哔哩哔哩第三方客户端，正式实现为 **Rust + GPUI**。

## 仓库结构

| 路径 | 说明 |
| --- | --- |
| `app/` | **正式工程**：Cargo workspace（`app` / `theme` / `components` / `player` / `library` / `shell` / `core` / `infra` / `deeplink`，见 `docs/代码架构.md`） |
| `demos/` | **Demo / 验证工程**：各自独立，有单独的 `Cargo.toml`、`Cargo.lock` 与构建缓存 |
| `docs/` | **文档**：UX 设计预览、调研、截图、技能与归档 |
| `scripts/` | 构建、打包、图标脚本 |
| `packaging/` | 打包资源（`macos/Info.plist`、`macos/AppIcon.icns`、`windows/` 图标与 NSIS 脚本） |
| `userscripts/` | 浏览器用户脚本（`ykpili-deeplink.user.js`：B 站视频链接 → 客户端） |
| `dist/` | 构建产物（`.app` / `.dmg` / Windows zip / setup.exe，不入库） |

## 常用命令

```bash
# 编译正式工程
cargo build --manifest-path app/Cargo.toml

# 运行正式工程
cargo run --manifest-path app/Cargo.toml -p yk-app --bin ykpili

# 打 macOS .app / .dmg
./scripts/package-macos.sh

# 打 Windows 绿色包 + NSIS 安装包（需 Rust 1.99.1 / MSVC / Windows SDK）
# 首次会自动准备 MSYS2 工具链并裁剪 libmpv，不需要额外参数
powershell -ExecutionPolicy Bypass -File .\scripts\package-windows.ps1

# 按清单裁剪编译 libmpv（只编启用的功能；没装 MSYS2 会自动安装）
./scripts/build-libmpv.sh --platform windows

# 生成应用图标
./scripts/make-macos-icon.sh
```

也可以先进入 `app/` 再执行 `cargo`：

```bash
cd app
cargo check --workspace
```

## Demo 工程

`demos/` 下每个目录都是独立工程，进入各自目录运行即可，例如：

```bash
cd demos/spike-mpv-wgpu
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
export CARGO_HOME="$(pwd)/.cargo-home"
cargo run --bin decode_test
```

## 故障排查

播放器报错会写入本地日志（正常运行不写）：

- macOS：`~/Library/Caches/YKPili/logs/player.log`
- Windows：`%LOCALAPPDATA%\YKPili\Cache\logs\player.log`

日志在每次启动时清空，只在报错 / panic 时写入；单次运行超过 2 MB 自动截断。

## Roadmap

> 接下来要做的五个方向，按顺序推进；不代表时间承诺，做完一项就把状态改掉。

| # | 方向 | 要做什么 | 状态 |
| --- | --- | --- | --- |
| 1 | 专栏 | 现在专栏只在搜索和 UP 主空间里露出标题摘要，点不进去；要补图文阅读页（正文渲染、代码块、图片查看、评论）。 | 未开始 |
| 2 | 合集 | 视频合集 / 番剧分集：合集列表、连播与进度记忆；代码里目前没有 `ugc_season` 相关实现。 | 未开始 |
| 3 | Linux 桌面 | 打通第三个桌面平台：`packaging/libmpv/features.toml` 只有 `windows` / `macos` 两段，要补 linux 段和 `build-libmpv.sh` 的 linux 构建分支，再做发行包（deb / AppImage）与窗口、输入法适配。 | 未开始 |
| 4 | iOS | 接 `gpui-pre` 的移动端支持（`gpui-pre-mobile`），解决签名与上架、后台播放与音频会话、libmpv 交叉编译。 | 未开始 |
| 5 | Android | 同上再加 NDK 交叉编译、JNI 桥接、生命周期与返回键适配。 | 未开始 |

## 文档入口

- `docs/ux-design-preview.html` — UX 设计规范与实景预览
- `docs/代码架构.md` — crate 分层、归属标准与迁移进度
- `docs/未完成任务清单.md` — 当前待办
- `docs/windows-移植记录.md` — Windows 平台隔离、视频链路与打包说明
- `packaging/windows/README.md` — Windows 绿色包 / NSIS 安装包
- `docs/macos-零拷贝视频播放器踩坑全记录.md` — 播放内核调研
- `docs/macos-音频设备掉线崩溃.md` — 拔耳机闪退的定位与 libmpv 补丁
- `docs/播放器内存排查.md` — 内存涨到 2.7 G 的定位：无界帧队列与每帧分配
- `docs/深链与UniversalLink.md` — Windows 深链 / macOS Universal Link / 浏览器用户脚本
- `docs/skills/` — 可复用的技能说明与参考
- `docs/archive/` — 历史版本与旧 Avalonia 工程归档
