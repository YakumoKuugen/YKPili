// 引入 Avalonia 框架的核心命名空间(Application 生命周期、依赖绑定等)
using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Data.Core;
using Avalonia.Data.Core.Plugins;
using System.Linq;
using Avalonia.Markup.Xaml;
// 引入项目内部的 ViewModel 与 View 命名空间
using YKPili.ViewModels;
using YKPili.Views;

namespace YKPili;

// 应用程序入口类:继承自 Avalonia 的 Application
// 负责初始化 XAML 资源并根据不同的运行平台(桌面/移动/单视图)创建主窗口或主视图
public partial class App : Application
{
    // 初始化方法:加载 App.axaml 中声明的样式、资源与数据模板
    public override void Initialize()
    {
        AvaloniaXamlLoader.Load(this);
    }

    // 框架初始化完成后回调:根据不同的应用生命周期接口选择合适的方式挂载主界面
    public override void OnFrameworkInitializationCompleted()
    {
        // 经典桌面应用生命周期(Windows / macOS / Linux 桌面):创建独立主窗口
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.MainWindow = new MainWindow
            {
                // 将 MainViewModel 作为根 DataContext,使 XAML 通过绑定获取数据
                DataContext = new MainViewModel()
            };
        }
        // Activity 形式的单视图工厂(适用于部分嵌入式或自定义 Activity 场景)
        else if (ApplicationLifetime is IActivityApplicationLifetime singleViewFactoryApplicationLifetime)
        {
            singleViewFactoryApplicationLifetime.MainViewFactory = () => new MainView { DataContext = new MainViewModel() };
        }
        // 移动端或单视图平台(iOS / Android / Browser):以单视图方式启动
        else if (ApplicationLifetime is ISingleViewApplicationLifetime singleViewPlatform)
        {
            singleViewPlatform.MainView = new MainView
            {
                DataContext = new MainViewModel()
            };
        }

        // 调用基类完成剩余的初始化工作
        base.OnFrameworkInitializationCompleted();
    }
}
