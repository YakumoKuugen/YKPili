using Avalonia.Controls;

namespace YKPili.Views;

/// <summary>
/// MainWindow:桌面端主窗口
/// 内部只承载一个 MainView,所有业务逻辑都下放给 ViewModel。
/// </summary>
public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
    }
}
