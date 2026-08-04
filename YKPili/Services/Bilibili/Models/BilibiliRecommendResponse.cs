// 引入集合类型与 JSON 序列化属性
using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace YKPili.Services.Bilibili.Models;

/// <summary>
/// BilibiliRecommendResponse:首页推荐接口顶层响应
/// B 站推荐流接口 https://api.bilibili.com/x/web-interface/wbi/index/top/feed/rcmd 的响应模型。
/// </summary>
public class BilibiliRecommendResponse
{
    /// <summary>业务状态码(0 表示成功)</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>推荐数据载荷</summary>
    [JsonPropertyName("data")]
    public RecommendData? Data { get; set; }
}

/// <summary>推荐数据载荷:包含视频条目</summary>
public class RecommendData
{
    /// <summary>推荐视频列表</summary>
    [JsonPropertyName("item")]
    public List<RecommendVideoItem> Items { get; set; } = [];
}

/// <summary>单个推荐视频条目</summary>
public class RecommendVideoItem
{
    /// <summary>视频 avid(数字 ID)</summary>
    [JsonPropertyName("id")]
    public long Id { get; set; }

    /// <summary>视频 bvid(字符串 ID,用于人类阅读)</summary>
    [JsonPropertyName("bvid")]
    public string Bvid { get; set; } = string.Empty;

    /// <summary>视频标题(原始 HTML 实体,使用方需自行解码)</summary>
    [JsonPropertyName("title")]
    public string Title { get; set; } = string.Empty;

    /// <summary>封面图 URL</summary>
    [JsonPropertyName("pic")]
    public string Pic { get; set; } = string.Empty;

    /// <summary>UP 主信息</summary>
    [JsonPropertyName("owner")]
    public VideoOwner? Owner { get; set; }

    /// <summary>统计数据(播放量、弹幕数等)</summary>
    [JsonPropertyName("stat")]
    public VideoStat? Stat { get; set; }

    /// <summary>视频简介</summary>
    [JsonPropertyName("desc")]
    public string Desc { get; set; } = string.Empty;
}

/// <summary>UP 主信息</summary>
public class VideoOwner
{
    /// <summary>UP 主昵称</summary>
    [JsonPropertyName("name")]
    public string Name { get; set; } = string.Empty;

    /// <summary>UP 主头像 URL</summary>
    [JsonPropertyName("face")]
    public string Face { get; set; } = string.Empty;
}

/// <summary>视频统计数据</summary>
public class VideoStat
{
    /// <summary>播放量</summary>
    [JsonPropertyName("view")]
    public long View { get; set; }

    /// <summary>弹幕数</summary>
    [JsonPropertyName("danmaku")]
    public long Danmaku { get; set; }
}
