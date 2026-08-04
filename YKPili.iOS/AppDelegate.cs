// 引入 iOS 基础框架
using Foundation;
using UIKit;
// 引入 Avalonia 与 Avalonia.iOS(提供 iOS 后端)
using Avalonia;
using Avalonia.Controls;
using Avalonia.iOS;
using Avalonia.Media;

namespace YKPili.iOS;

/// <summary>
/// AppDelegate:iOS 应用的 UIApplicationDelegate。
/// 该类负责启动 Avalonia 应用的用户界面,并监听 iOS 系统级事件。
/// 通过继承 AvaloniaAppDelegate&lt;App&gt;,框架会自动完成 Avalonia 启动与事件桥接。
/// </summary>
[Register("AppDelegate")]
#pragma warning disable CA1711 // Identifiers should not have incorrect suffix
public partial class AppDelegate : AvaloniaAppDelegate<App>
#pragma warning restore CA1711 // Identifiers should not have incorrect suffix
{
    /// <summary>自定义 AppBuilder:加载 Inter 字体</summary>
    protected override AppBuilder CustomizeAppBuilder(AppBuilder builder)
    {
        return base.CustomizeAppBuilder(builder)
            .WithInterFont();
    }
}
