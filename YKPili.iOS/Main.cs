// 引入 iOS UIKit
using UIKit;

namespace YKPili.iOS;

/// <summary>
/// Application:iOS 应用的托管入口。
/// Main 方法会调用 UIApplication.Main,启动 iOS 应用的 run loop。
/// 如果需要自定义 Application Delegate,可通过 UIApplication.Main 的第三个参数传入。
/// </summary>
public class Application
{
    /// <summary>程序入口:启动 UIApplication 并使用 AppDelegate 作为委托</summary>
    static void Main(string[] args)
    {
        // 如果想使用不同的 Application Delegate,可以在此处指定:
        // UIApplication.Main(args, null, typeof(YourCustomDelegate));
        UIApplication.Main(args, null, typeof(AppDelegate));
    }
}
