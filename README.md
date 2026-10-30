# YKPili

哔哩哔哩第三方客户端，正式实现为 **Rust + GPUI**。

## 仓库结构

| 路径 | 说明 |
| --- | --- |
| `app/` | **正式工程**：Cargo workspace（`yk-app` / `yk-core` / `yk-ui` / `yk-media`） |
| `demos/` | **Demo / 验证工程**：各自独立，有单独的 `Cargo.toml`、`Cargo.lock` 与构建缓存 |
| `docs/` | **文档**：UX 设计预览、调研、截图、技能与归档 |
| `scripts/` | 构建、打包、图标脚本 |
| `packaging/` | 打包资源（`macos/Info.plist`、`macos/AppIcon.icns`、`windows/` 图标与 NSIS 脚本） |
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
powershell -ExecutionPolicy Bypass -File .\scripts\package-windows.ps1

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

## 文档入口

- `docs/ux-design-preview.html` — UX 设计规范与实景预览
- `docs/未完成任务清单.md` — 当前待办
- `docs/windows-移植记录.md` — Windows 平台隔离、视频链路与打包说明
- `packaging/windows/README.md` — Windows 绿色包 / NSIS 安装包
- `docs/macos-零拷贝视频播放器踩坑全记录.md` — 播放内核调研
- `docs/skills/` — 可复用的技能说明与参考
- `docs/archive/` — 历史版本与旧 Avalonia 工程归档
