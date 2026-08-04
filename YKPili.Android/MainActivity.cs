// 引入 Android Activity 与配置变更相关类型
using Android.App;
using Android.Content.PM;
// 引入 Avalonia 与 Avalonia.Android
using Avalonia;
using Avalonia.Android;

namespace YKPili.Android;

/// <summary>
/// YKPili.Android 主 Activity(单 Activity 应用)。
/// 继承 AvaloniaMainActivity,启动后自动加载 YKPili.App 并把 MainView 显示到全屏。
/// </summary>
[Activity(
    Label = "YKPili.Android",           // 应用在启动器中显示的标签
    Theme = "@style/MyTheme.NoActionBar", // 主题(无 ActionBar,与 Avalonia 渲染配合)
    Icon = "@drawable/icon",            // 应用图标
    MainLauncher = true,                // 标记为启动入口
    // 自行处理配置变更,避免 Android 因旋转/尺寸变化重建 Activity
    // (Avalonia 内部会响应这些变化重新布局)
    ConfigurationChanges = ConfigChanges.Orientation | ConfigChanges.ScreenSize | ConfigChanges.UiMode)]
public class MainActivity : AvaloniaMainActivity
{
}
