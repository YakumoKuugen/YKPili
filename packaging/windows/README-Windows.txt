YKPili · Windows 版
===================

哔哩哔哩第三方客户端（Rust + GPUI）。

系统要求
--------
- Windows 10 1809 / Windows 11，64 位（x86_64）
- 支持 Direct3D 11 的显卡（集成显卡即可）
- 首次启动需要联网（首页推荐、登录、视频取流）

安装 / 运行
-----------
- 安装包：双击 `YKPili-<版本>-windows-x64-setup.exe`，按提示安装。
  默认装到当前用户目录，不需要管理员权限。
  如果之前装过，安装程序会自动找到上次的安装目录并作为默认值（可手动改）。
- 绿色版：解压 `YKPili-<版本>-windows-x64.zip`，双击 `ykpili.exe`。
  不要单独把 exe 拷走：`libmpv-2.dll` 及其同目录的 `libass-*.dll` 等运行时库
  必须和 exe 放在同一目录。

数据目录
--------
- 登录态 / 设置 / 续播 / 搜索历史：`%APPDATA%\YKPili`
- 图片与推荐流缓存：`%LOCALAPPDATA%\YKPili\Cache`
- 卸载不会删除上面两个目录；要彻底清理请手动删除。

快捷键
------
- `Alt+Q`：第一次显示退出提示，3 秒内再按一次退出（防误触；和 macOS 的 ⌘Q 一致）
- `Esc`：关闭播放浮窗（第一次提示，再按一次关闭）
- `Enter`：发送弹幕

已知限制
--------
- 安装包没有代码签名，Windows SmartScreen 可能提示“未知发布者”。
  选择“更多信息 → 仍要运行”即可。
- 视频走 CPU 回读 + GPU 贴图，1080P 正常；4K 或高码率片源可能掉帧。
- 部分账号写接口（投币等）未在真实账号上做完整回归测试。

开源许可
--------
本程序随包附带按 `LIBMPV-FEATURES.toml` 裁剪编译的 libmpv（mpv 为 GPLv2+，
FFmpeg 为 LGPLv2.1+），以及 libass / libplacebo 等 LGPL 运行时库。
相关许可证文本在 `LICENSES\` 目录；源码获取方式见同目录 `SOURCE.txt`。
