# 深链与 Universal Link

本文对应两个交付物：

| 位置 | 作用 |
| --- | --- |
| `app/crates/deeplink`（`yk-deeplink`） | 跨平台深链组件：URL 解析、协议注册、单实例转发、macOS Universal Link 回调注入 |
| `userscripts/ykpili-deeplink.user.js` | 浏览器用户脚本：拦截 B 站视频链接，转成 `ykpili://` 拉起客户端 |
| `app/crates/components/src/deeplink.rs` | GPUI 提示气泡：告知用户「已接管 / 正在解析 / 无法识别」 |

---

## 1. 统一链接模型

组件只认三种东西：

```text
ykpili://video/BV1xx411c7mD?p=2&t=90      自有 Scheme（推荐）
ykpili://open?url=<encoded URL>            通用入口（短链 / 转发）
ykpili://search?keyword=关键词              站内搜索
```

同时接受 B 站网页链接：

```text
https://www.bilibili.com/video/BV1xx411c7mD/?p=2&t=90
https://m.bilibili.com/video/BV1xx411c7mD
https://www.bilibili.com/list/...?bvid=BV1xx411c7mD
https://b23.tv/xxxxx                        （先跟一次 HTTP 跳转）
```

解析规则：

- `BV` 号必须是 12 位（`BV` + 10 位字母数字）；`av123` 会先通过详情接口换成 BV。
- `p` 从 1 开始，会透传给播放器选分 P；`t` / `start` 支持 `90`、`1m30s`、`01:30`、`1h2m3s`。
- 非 B 站域名、番剧、直播会明确返回「不支持」并在客户端弹提示，不会误开播放器。

---

## 2. Windows：注册表 + 命名管道

### 2.1 协议注册

应用启动时（以及 NSIS 安装时）写入 **当前用户** 注册表，不需要管理员权限：

```text
HKCU\Software\Classes\ykpili
    (Default)                         = URL:YKPili Protocol
    URL Protocol                      = ""
    DefaultIcon\(Default)             = "C:\...\ykpili.exe",0
    shell\open\command\(Default)      = "C:\...\ykpili.exe" "%1"
```

关闭自动注册（测试 / 绿色版不想改注册表）：

```powershell
$env:YKPILI_NO_PROTOCOL_REGISTER = "1"
```

手动检查：

```powershell
reg query HKCU\Software\Classes\ykpili /s
```

### 2.2 单实例转发

浏览器唤起 `ykpili://...` 时，系统会启动一个新的 `ykpili.exe`。流程：

1. `main` 最开始调用 `yk_deeplink::forward_link_before_ui()`；
2. 从命令行取出 URL；
3. 用命名互斥体 `Local\YKPili.DeepLink.Singleton.v1` 判断是否已有实例；
4. 已有实例 → 连接 `\\.\pipe\ykpili-deeplink-v1-<用户名>`，写入 URL 后退出；
5. 旧实例的管道线程读入队列 → GPUI 主线程轮询 → 弹到前台并播放。

管道只允许本机同用户连接（`PIPE_REJECT_REMOTE_CLIENTS`），不监听端口，不触发防火墙。

### 2.3 测试

```powershell
# 先启动应用
cargo run --manifest-path app/Cargo.toml -p yk-app --bin ykpili

# 另开一个终端（或直接在浏览器地址栏）
Start-Process "ykpili://video/BV1xx411c7mD?p=2&t=30"

# 也可以直接带参数启动；如果已有实例，会走管道转发
.\app\target\debug\ykpili.exe "ykpili://video/BV1xx411c7mD?t=30"
```

预期：旧窗口弹到前台，播放器开始取流，顶部出现「已从浏览器转到 YKPili」气泡。

---

## 3. macOS：URL Scheme + Universal Link

### 3.1 自定义 Scheme（默认可用）

`packaging/macos/Info.plist` 已声明：

```xml
<key>CFBundleURLTypes</key>
<array>
  <dict>
    <key>CFBundleURLSchemes</key>
    <array><string>ykpili</string></array>
  </dict>
</array>
```

AppKit 会把 URL 交给 GPUI 的 `application:openURLs:`，主程序在 `main.rs` 注册：

```rust
app.on_open_urls(|urls| {
    for url in urls {
        yk_deeplink::ingest(url, yk_deeplink::LinkSource::MacOpenUrl);
    }
});
```

测试：

```bash
open "ykpili://video/BV1xx411c7mD?p=2&t=30"
```

### 3.2 Universal Link 的硬前提

**Universal Link 只能绑定「应用开发者控制的域名」。** 本项目不能给
`bilibili.com` 注册 Universal Link，因为 AASA 文件必须部署在
`https://bilibili.com/.well-known/apple-app-site-association`，而该域名不属于本项目。

因此：

- B 站链接在浏览器里走 `ykpili://` 自定义协议（用户脚本默认行为）；
- Universal Link 用于本项目自己的分享域名 `go.ykpili.app`；
- 两者最终都进入同一个队列，播放逻辑完全一致。

### 3.3 启用 Universal Link

1. **部署 AASA**：把 `packaging/macos/apple-app-site-association` 里的
   `TEAMID` 换成你的 Apple Team ID，Bundle ID 不是 `com.ykpili.app` 时一并改；
   如果要换掉 `go.ykpili.app`，同步修改
   `app/crates/deeplink/src/model.rs` 的 `UNIVERSAL_LINK_HOST`、entitlements 和测试。
   上传到：

   ```text
   https://go.ykpili.app/.well-known/apple-app-site-association
   ```

   要求：`Content-Type: application/json`、无 301/302、不能带 `.json` 后缀。

2. **签名**：用付费 Apple Developer 账号的证书签名，并带包含
   Associated Domains 的 provisioning profile。把导出的 profile 放到
   `packaging/macos/embedded.provisionprofile`，打包脚本会自动放进
   `Contents/`。`packaging/macos/YKPili.entitlements` 已经写入：

   ```xml
   <key>com.apple.developer.associated-domains</key>
   <array><string>applinks:go.ykpili.app</string></array>
   ```

   `scripts/package-macos.sh --identity "..."` 会自动附加该 entitlements。
   ad-hoc 签名（默认 `--sign -`）不具备 Universal Link 能力，只影响 UL，
   不影响 `ykpili://`。

3. **测试**：Universal Link 只响应用户「点击链接」。把下面链接从备忘录 /
   信息 / 邮件里点开，不要粘贴到地址栏：

   ```text
   https://go.ykpili.app/video/BV1xx411c7mD?p=2&t=30
   ```

   第一次点击如果停在 Safari，检查 AASA 是否可访问、Team ID 是否一致、
   应用是否重新安装过（AASA 有缓存）。

### 3.4 GPUI 未实现 `continueUserActivity`

GPUI 0.8 的 macOS 平台只实现了 `application:openURLs:`，没有实现
`application:continueUserActivity:restorationHandler:`。`yk-deeplink` 在启动时
给 `GPUIApplicationDelegate` 运行时补这个方法，从 `NSUserActivity.webpageURL`
取 URL 后写入同一个队列：

```text
NSUserActivity.webpageURL -> absoluteString -> yk_deeplink 队列
```

注入失败只在日志里提示，不影响自定义 Scheme。

---

## 4. 浏览器用户脚本

### 4.1 安装

1. 安装 Tampermonkey / Violentmonkey；
2. 新建脚本，粘贴 `userscripts/ykpili-deeplink.user.js`；
3. 打开任意 B 站视频页。

### 4.2 三种模式

在脚本菜单里切换（当前模式会显示在菜单标题上）：

| 模式 | 行为 |
| --- | --- |
| 自动接管（默认） | 打开视频页自动尝试唤起客户端；站内点击视频链接也直接接管 |
| 只拦截点击 | 只有点击视频链接才接管，直接打开视频页不自动唤起 |
| 关闭 | 完全不动浏览器行为 |

唤起客户端后，浏览器窗口一旦失焦，脚本会在约 0.5 秒后关闭当前标签。
这里只监听「真的切走」而不是盲目定时关闭：客户端没装、浏览器仍有焦点时
标签会保留。浏览器不允许关闭窗口里的最后一个标签，这种场景会保留标签。

「打开后关闭标签」可以在脚本菜单里单独开关（默认开）。

### 4.3 加载前拦截

直接打开 B 站视频页时，脚本在 `document-start` 阶段就介入：

1. 先判断是不是 B 站视频页 / 短链，以及是否被 `?ykpili=0` 或会话内的
   「留在浏览器」放行；
2. 命中后立刻调用 `window.stop()`，中止后续 HTML / JS / 图片的解析与加载；
3. 用一个极简接管页替换页面，并马上尝试唤起客户端；
4. 客户端起来、浏览器失焦后关闭标签；没起来则留在接管页，可手动
   「打开 YKPili」或「继续加载网页」。

用户脚本无法取消浏览器已经发出的第一个 HTML 请求，但可以阻止页面继续
解析和执行，因此不会再加载完整的 B 站播放器。要做到「一个请求都不发」，
需要浏览器扩展的 `webRequest` / `declarativeNetRequest` 能力，普通
用户脚本做不到。

### 4.4 临时跳过

- **按住 Alt 点击**视频链接：这一次交给浏览器；
- 视频页加 `?ykpili=0`：本页不自动接管；
- 弹出的提示里点「留在浏览器」：当前会话内不再接管这个视频。

### 4.5 它做了什么

- 点击事件用捕获阶段拦截，只处理 `bilibili.com/video/`、`?bvid=`、`b23.tv`；
- 脚本会在所有网页上运行（为了接管外站里的 B 站链接），但只有 B 站 / 短链
  页面会做「打开页面自动唤起」；别的站点只在点击 B 站链接时介入；
- 通过 `@grant window.close` 获得关标签权限，但只在浏览器窗口真的失焦后关，
  5 秒内没切走就撤销，避免客户端没装时误关；
- 保留 `p` / `t` 参数，多 P 和时间点不会丢；
- 短链发 `ykpili://open?url=...`，由客户端跟跳转后再播放；
- 用 Shadow DOM 画提示卡，避免被 B 站样式污染；
- 不改页面 DOM、不注入请求、不读取登录态。

### 4.6 调试

浏览器控制台里看：

```js
localStorage.getItem('ykpili:mode')
// 或 Tampermonkey 的 GM 存储
```

脚本没有后台请求，也没有 `@connect`，不需要额外权限。

---

## 5. 安全边界

- Windows 管道只监听本机、拒绝远程客户端；写入内容仍会经过 URL 解析，
  非 B 站链接直接拒绝。
- macOS Universal Link 只接受 AASA 声明的路径；自定义 Scheme 同样经过解析器。
- 用户脚本只读取当前页面 URL 与点击目标，不上传任何数据。
- 深链不会触发登录、投币、评论等写操作，只负责「打开视频 / 搜索」。

---

## 6. 常见问题

**浏览器提示「没有应用可以处理此地址」**
客户端没安装，或 Windows 注册表被清理。先启动一次客户端（启动时会自愈注册），
再执行 `reg query HKCU\Software\Classes\ykpili /s` 检查。

**Windows 弹出了第二个窗口**
说明单实例转发没成功：确认旧实例还在运行、管道名没有被其他程序占用；
看日志里是否有 `[deeplink] 找不到运行中实例的管道`。

**macOS `open ykpili://...` 没反应**
用 `./scripts/package-macos.sh --no-dmg` 重新生成 `.app`，确认
`Info.plist` 里有 `CFBundleURLTypes`；直接跑 `cargo run` 的裸二进制没有
Bundle 注册，Scheme 不会生效。

**Universal Link 一直打开网页**
按 3.3 逐项检查；尤其注意 ad-hoc 签名、AASA 的 Team ID、以及必须从
「链接点击」进入。调试可在 Mac 上执行：

```bash
swcutil dl -d go.ykpili.app   # 查看系统缓存的 AASA
```
