// 引入系统基础类型与 Avalonia 数据模板相关接口
using System;
using System.Diagnostics.CodeAnalysis;
using Avalonia.Controls;
using Avalonia.Controls.Templates;
// 引入项目的 ViewModel 命名空间,ViewLocator 需要根据 ViewModel 反射查找 View
using YKPili.ViewModels;

namespace YKPili;

/// <summary>
/// ViewLocator:ViewModel -> View 解析器
/// 给定一个 ViewModel 实例,根据类型名约定(将 "ViewModel" 后缀替换为 "View")
/// 自动找到对应的 View 类型并创建实例。这是 MVVM 模式中"约定优于配置"的实现。
/// </summary>
[RequiresUnreferencedCode(
    "Default implementation of ViewLocator involves reflection which may be trimmed away.",
    Url = "https://docs.avaloniaui.net/docs/concepts/view-locator")]
public class ViewLocator : IDataTemplate
{
    // 构建控件:根据传入的 ViewModel 实例,反射查找并实例化对应的 View
    public Control? Build(object? param)
    {
        // 入参为空时直接返回 null,表示无匹配
        if (param is null)
            return null;

        // 通过反射约定:ViewModel 名称 -> 替换为 View 后缀
        // 例如:HomeViewModel -> YKPili.Views.HomeView
        var name = param.GetType().FullName!.Replace("ViewModel", "View", StringComparison.Ordinal);
        var type = Type.GetType(name);

        if (type != null)
        {
            // 通过反射创建 View 实例并返回
            return (Control)Activator.CreateInstance(type)!;
        }

        // 找不到对应 View 时,显示一个文本占位,便于在开发期快速发现配置问题
        return new TextBlock { Text = "Not Found: " + name };
    }

    // 匹配条件:仅对 ViewModelBase 派生类型生效
    public bool Match(object? data)
    {
        return data is ViewModelBase;
    }
}
