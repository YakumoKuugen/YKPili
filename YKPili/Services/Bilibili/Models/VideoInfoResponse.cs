// 引入集合类型与 JSON 序列化属性
using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace YKPili.Services.Bilibili.Models;

/// <summary>
/// VideoInfoResponse:视频详情接口响应
/// 接口 https://api.bilibili.com/x/web-interface/wbi/view 的响应模型。
/// </summary>
public class VideoInfoResponse
{
    /// <summary>业务状态码(0 表示成功)</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>视频数据载荷</summary>
    [JsonPropertyName("data")]
    public VideoInfoData? Data { get; set; }
}

/// <summary>视频数据载荷</summary>
public class VideoInfoData
{
    /// <summary>视频 bvid</summary>
    [JsonPropertyName("bvid")]
    public string Bvid { get; set; } = string.Empty;

    /// <summary>视频 avid(数字 ID)</summary>
    [JsonPropertyName("aid")]
    public long Aid { get; set; }

    /// <summary>视频 cid(用于请求播放地址)</summary>
    [JsonPropertyName("cid")]
    public long Cid { get; set; }

    /// <summary>分 P 列表(多 P 视频需要根据用户选择取对应 cid)</summary>
    [JsonPropertyName("pages")]
    public List<VideoPage> Pages { get; set; } = [];

    /// <summary>视频标题</summary>
    [JsonPropertyName("title")]
    public string Title { get; set; } = string.Empty;
}

/// <summary>单个分 P 信息</summary>
public class VideoPage
{
    /// <summary>该分 P 的 cid</summary>
    [JsonPropertyName("cid")]
    public long Cid { get; set; }

    /// <summary>分 P 序号(从 1 开始)</summary>
    [JsonPropertyName("page")]
    public int Page { get; set; }

    /// <summary>分 P 标题</summary>
    [JsonPropertyName("part")]
    public string Part { get; set; } = string.Empty;
}
