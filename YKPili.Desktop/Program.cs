// 引入基础类型、HTTP、代理
using System;
using System.Net;
using System.Net.Http;
// 引入 Avalonia 启动 API
using Avalonia;
// 引入 Avalonia MCP 诊断(用于 IDE 集成)
using AvaloniaMcp.Diagnostics;
// 引入 macOS 平台播放器
using YKPili.Desktop.Platforms.Mac;
// 引入 YKPili.Views(用于注册播放器工厂)
using YKPili.Views;

namespace YKPili.Desktop;

/// <summary>
/// Program:桌面端入口
/// 配置 Avalonia 启动选项、注册平台特定服务(Mac 原生播放器),然后启动应用。
/// </summary>
sealed class Program
{
    /// <summary>主入口:STA 线程下启动 Avalonia 桌面生命周期</summary>
    [STAThread]
    public static void Main(string[] args) => BuildAvaloniaApp()
        .StartWithClassicDesktopLifetime(args);

    /// <summary>配置 Avalonia AppBuilder</summary>
    public static AppBuilder BuildAvaloniaApp()
    {
        // 直连 B 站 API 与 CDN,绕过系统代理(避免被代理污染)
        HttpClient.DefaultProxy = new WebProxy();

        // 注册 macOS 系统播放器工厂
        // VideoPlayerView 会通过该工厂懒加载 MacMediaPlayer
        VideoPlayerView.PlayerFactory = () => new MacMediaPlayer();

        return AppBuilder.Configure<App>()
            .UsePlatformDetect() // 根据当前 OS 加载对应 Avalonia 后端
#if DEBUG
            // 仅 Debug 模式启用开发者工具(DevTools)
            .WithDeveloperTools()
#endif
            .WithInterFont()   // 加载 Inter 字体
            .LogToTrace()      // 日志输出到 Trace
            .UseMcpDiagnostics(); // 启用 AvaloniaMcp 诊断(供 Codex / 调试工具访问)
    }
}
