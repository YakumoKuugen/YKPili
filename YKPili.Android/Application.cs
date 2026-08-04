// 引入 Android 原生类型
using Android.App;
using Android.Runtime;
// 引入 Avalonia 与 Avalonia.Android(提供 Android 后端)
using Avalonia;
using Avalonia.Android;

namespace YKPili.Android
{
    /// <summary>
    /// YKPili.Android 应用入口。
    /// 继承自 AvaloniaAndroidApplication&lt;App&gt;,把 Avalonia 应用挂接到 Android Application 生命周期。
    /// </summary>
    [Application]
    public class Application : AvaloniaAndroidApplication<App>
    {
        protected Application(nint javaReference, JniHandleOwnership transfer) : base(javaReference, transfer)
        {
        }

        /// <summary>自定义 AppBuilder:加载 Inter 字体(由 Avalonia.Fonts.Inter 提供)</summary>
        protected override AppBuilder CustomizeAppBuilder(AppBuilder builder)
        {
            return base.CustomizeAppBuilder(builder)
            .WithInterFont();
        }
    }
}
