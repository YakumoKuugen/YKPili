// 引入 CommunityToolkit.Mvvm 的 ObservableObject,用于实现属性变更通知
using CommunityToolkit.Mvvm.ComponentModel;

namespace YKPili.Models;

/// <summary>
/// CoverItem:首页推荐视频的封面卡片数据模型
/// 继承 ObservableObject 以支持通过源生成器在 [ObservableProperty] 字段上
/// 自动生成带通知的公共属性,便于在 XAML 中进行数据绑定。
/// </summary>
public partial class CoverItem : ObservableObject
{
    /// <summary>在容器池中的索引(用于性能回收复用)</summary>
    public int Index { get; set; }

    /// <summary>视频标题(HTML 已解码)</summary>
    [ObservableProperty]
    private string _title = string.Empty;

    /// <summary>占位色(无封面图时显示的背景色,十六进制字符串,如 "#E57373")</summary>
    [ObservableProperty]
    private string _placeholderColor = "#555555";

    /// <summary>封面图远程地址</summary>
    [ObservableProperty]
    private string _imageUrl = string.Empty;

    /// <summary>B 站视频 ID(用于后续请求详情/播放地址)</summary>
    public string Bvid { get; set; } = string.Empty;

    /// <summary>B 站视频分页 CID(可能在请求详情后回填)</summary>
    public long Cid { get; set; }
}
