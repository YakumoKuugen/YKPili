using Avalonia.Controls;

namespace YKPili.Views;

/// <summary>
/// MainView:主视图容器
/// 作为登录页/首页/播放器的承载容器,通过 IsVisible 控制三者的切换。
/// </summary>
public partial class MainView : UserControl
{
    public MainView()
    {
        InitializeComponent();
    }
}
