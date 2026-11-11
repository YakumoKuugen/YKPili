# 浏览器用户脚本

`ykpili-deeplink.user.js`：在浏览器里打开 B 站视频时，用 `ykpili://`
转交给 [YKPili](../../README.md) 客户端播放。

## 安装

1. 安装 [Tampermonkey](https://www.tampermonkey.net/) 或 Violentmonkey；
2. 新建脚本，粘贴本目录的 `ykpili-deeplink.user.js`，保存；
3. 打开任意 B 站视频页。

脚本会在所有网页上运行，但只处理 B 站视频链接（`bilibili.com/video/`、
`?bvid=`、`b23.tv` / `bili2233.cn`）：外站里的 B 站链接会被接管，
普通链接不受影响。

直接打开视频页时，脚本在 `document-start` 就调用 `window.stop()` 截断
后续 HTML / JS 加载，显示一个极简的「正在打开 YKPili」页面并尝试唤起客户端，
不会先加载完整的 B 站播放器。用户脚本无法取消浏览器已经发出的第一个 HTML
请求；要做到完全零请求需要浏览器扩展的请求拦截能力。

脚本菜单里可以切换：

- **自动接管**（默认）：打开视频页自动尝试唤起客户端，点击站内视频链接也接管；
- **只拦截点击**：只有点击视频链接才接管；
- **关闭**：完全不拦截。

按住 **Alt** 点击视频链接可临时交给浏览器；视频页加 `?ykpili=0` 可关闭本页自动接管。

打开客户端、浏览器窗口失焦后，脚本会在约 0.5 秒后关闭当前标签；
如果客户端没有成功打开（浏览器一直有焦点），则不会关闭。浏览器不允许
关闭窗口里的最后一个标签，这种情况需要手动关闭。

菜单里可以开关「打开后关闭标签」。

## 前置条件

- 客户端至少启动过一次（Windows 会把 `ykpili://` 注册到当前用户注册表；
  macOS 需要打包成 `.app`，裸二进制没有 Bundle 注册）；
- 短链 `b23.tv` 需要客户端能联网解析跳转。

完整说明（Windows 命名管道、macOS Universal Link、AASA 配置）见
[`docs/深链与UniversalLink.md`](../../docs/深链与UniversalLink.md)。
