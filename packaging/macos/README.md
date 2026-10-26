# macOS 打包说明

把 `app/crates/yk-app` 的 `ykpili` 打包成可双击的 `.app`。

## 一键打包

```bash
./scripts/package-macos.sh
```

产物：

- `dist/YKPili.app`
- `dist/YKPili-<版本>-macos-<架构>.dmg`（磁盘权限不足时会自动改出 `.zip`）

脚本会做四件事：

1. `cargo build --manifest-path app/Cargo.toml --release -p yk-app --bin ykpili`；
2. 生成标准 `.app` 目录（Info.plist、图标、PkgInfo）；
3. 若本机装了 Homebrew 的 mpv，把 `libmpv` 及整棵依赖树放进
   `Contents/Frameworks`，用 `install_name_tool` 重写成 `@loader_path/...`；
4. 由内到外签名（dylib → .app），最后压成 DMG。

内置 libmpv 后，`.app` 不再依赖用户机器上的 Homebrew；已实测视频解码、
`libavcodec` / `libass` 等依赖都从 `Contents/Frameworks` 加载。

## 常用参数

```bash
./scripts/package-macos.sh --no-dmg       # 只出 .app
./scripts/package-macos.sh --debug         # debug 构建，验证打包流程
./scripts/package-macos.sh --no-libmpv     # 不内置 mpv，运行时用系统安装的
./scripts/package-macos.sh --identity "Developer ID Application: ..."
```

## 版本号

- `CFBundleShortVersionString`：`app/Cargo.toml` 里 `[workspace.package].version`
- `CFBundleVersion`：当前 git commit 数
- Bundle ID：默认 `com.ykpili.app`，可用 `BUNDLE_ID=...` 覆盖

## 签名与分发

默认是 **ad-hoc 签名**（`codesign --sign -`）：

- 本机双击可以直接运行；
- 换一台 Mac 或在网上分发时，Gatekeeper 会提示“无法验证开发者”。
  接收方可右键 → 打开，或执行
  `xattr -dr com.apple.quarantine /Applications/YKPili.app`。

如果要正式分发，需要 Developer ID Application 证书（钥匙串里只有
Apple Development 证书是不够的）：

```bash
./scripts/package-macos.sh --identity "Developer ID Application: Your Name (TEAMID)"

xcrun notarytool submit dist/YKPili-*.dmg \
    --apple-id "you@example.com" --team-id TEAMID --password "app-specific-password" --wait
xcrun stapler staple dist/YKPili-*.dmg
```

带 `--identity` 时脚本会自动加 `--options runtime` 和 `--timestamp`，
满足公证对 hardened runtime 的要求。

## 架构

当前产物是**原生 arm64**（Apple Silicon）。脚本用 `uname -m` 决定 DMG 文件名，
不会自动做 universal。需要 Intel 版本时：

```bash
rustup target add x86_64-apple-darwin
cargo build --manifest-path app/Cargo.toml --release --target x86_64-apple-darwin -p yk-app --bin ykpili
# 再用 lipo 合并两个二进制，或把 bundle 里的 MacOS/ykpili 换成 x86_64 版本
```

## 图标

`packaging/macos/AppIcon.icns` 已生成。改配色/形状后重新生成：

```bash
./scripts/make-macos-icon.sh
```

图标源是 `scripts/make-macos-icon.swift`（品牌蓝 `#00A1D6` → 粉 `#FB749A`
渐变 + 白色播放三角）。


## 关于 mpv 的许可证

Homebrew 的 mpv 是 GPL 构建，`libmpv` 的传递依赖里也有 GPL/LGPL 库。
脚本会把 mpv/ffmpeg 的许可证文本放进 `Contents/Resources/Licenses/`。
如果这个 `.app` 要对外分发，请确认整体许可证符合 GPL 的要求
（源码获取方式等），详见 `docs/libmpv-跨平台调研.md`。
