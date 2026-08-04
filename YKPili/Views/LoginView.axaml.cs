using Avalonia.Controls;

namespace YKPili.Views;

/// <summary>
/// LoginView:登录页视图
/// 仅包含 XAML 初始化逻辑,所有业务状态都在 LoginViewModel 中管理。
/// </summary>
public partial class LoginView : UserControl
{
    public LoginView()
    {
        InitializeComponent();
    }
}
