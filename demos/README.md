# Demo / 验证工程

这里存放开发过程中用来验证技术路线的独立小工程。每个目录都有自己的
`Cargo.toml`、`Cargo.lock` 和构建缓存，不参与 `app/` 的 workspace。

| 目录 | 内容 |
| --- | --- |
| `spike-gpui-kit/` | GPUI Kit 0.7.3 底座能力验证：窗口、渲染、离屏截图 |
| `spike-gpui-surface/` | GPUI + `core-video` 的 Surface 对接验证 |
| `spike-mpv-wgpu/` | AVFoundation 解复用 + VideoToolbox 硬解 + IOSurface 零拷贝 + wgpu 上屏 |
| `spike-avalonia-handles/` | 旧 Avalonia 路线下原生控件句柄的探测工程（.NET） |

运行方式：进入对应目录后直接 `cargo run`（或按各目录内注释执行）。
`spike-mpv-wgpu` 的具体步骤见
[`docs/macos-零拷贝视频播放器踩坑全记录.md`](../docs/macos-零拷贝视频播放器踩坑全记录.md) 第 10 节。
