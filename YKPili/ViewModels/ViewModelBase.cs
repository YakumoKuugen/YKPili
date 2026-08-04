// 引入 CommunityToolkit.Mvvm 的 ObservableObject 基类(包含 INotifyPropertyChanged 实现)
using CommunityToolkit.Mvvm.ComponentModel;

namespace YKPili.ViewModels;

/// <summary>
/// ViewModelBase:所有 ViewModel 的公共基类
/// 继承自 ObservableObject,自动获得属性变更通知能力,
/// 同时可在此处统一添加项目级别的命令/服务等通用扩展。
/// </summary>
public abstract class ViewModelBase : ObservableObject
{
}
