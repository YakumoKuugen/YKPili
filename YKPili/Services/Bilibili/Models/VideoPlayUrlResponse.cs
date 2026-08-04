// 引入集合类型与 JSON 序列化属性
using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace YKPili.Services.Bilibili.Models;

/// <summary>
/// VideoPlayUrlResponse:视频播放地址接口响应
/// 接口 https://api.bilibili.com/x/player/wbi/playurl 返回的播放信息。
/// 现代接口优先返回 DASH(分视频/音轨的流),老接口可能返回 durl(单一 mp4 切片)。
/// </summary>
public class VideoPlayUrlResponse
{
    /// <summary>业务状态码(0 表示成功)</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>播放数据载荷</summary>
    [JsonPropertyName("data")]
    public PlayUrlData? Data { get; set; }
}

/// <summary>播放数据载荷</summary>
public class PlayUrlData
{
    /// <summary>DASH 数据(视频流 + 音频流,推荐场景)</summary>
    [JsonPropertyName("dash")]
    public DashData? Dash { get; set; }

    /// <summary>老格式单文件切片(非 DASH 模式时使用)</summary>
    [JsonPropertyName("durl")]
    public List<DurlItem> Durl { get; set; } = [];
}

/// <summary>DASH 视频与音频流集合</summary>
public class DashData
{
    /// <summary>视频流列表(不同清晰度/编码)</summary>
    [JsonPropertyName("video")]
    public List<DashStream> Video { get; set; } = [];

    /// <summary>音频流列表(不同码率/编码)</summary>
    [JsonPropertyName("audio")]
    public List<DashStream> Audio { get; set; } = [];
}

/// <summary>
/// DashStream:单个视频或音频流
/// B 站的字段命名在不同接口里有些混乱:新版是 baseUrl,旧版是 base_url。
/// 下方 Url 属性优先返回 baseUrl,缺失时回退到 FallbackUrl。
/// </summary>
public class DashStream
{
    /// <summary>流 ID(标识清晰度/编码组合)</summary>
    [JsonPropertyName("id")]
    public int Id { get; set; }

    /// <summary>新版字段:流 URL</summary>
    [JsonPropertyName("baseUrl")]
    public string BaseUrl { get; set; } = string.Empty;

    /// <summary>旧版字段:流 URL(回退用)</summary>
    [JsonPropertyName("base_url")]
    public string FallbackUrl { get; set; } = string.Empty;

    /// <summary>编解码器名称(如 avc1.640028 / hev1.1.6.L120.90 / av01.0.04M.08)</summary>
    [JsonPropertyName("codecs")]
    public string Codecs { get; set; } = string.Empty;

    /// <summary>编解码 ID(数值)</summary>
    [JsonPropertyName("codec_id")]
    public int CodecId { get; set; }

    /// <summary>视频宽度(音频流为 0)</summary>
    [JsonPropertyName("width")]
    public int Width { get; set; }

    /// <summary>视频高度(音频流为 0)</summary>
    [JsonPropertyName("height")]
    public int Height { get; set; }

    /// <summary>带宽(字节/秒)</summary>
    [JsonPropertyName("bandwidth")]
    public long Bandwidth { get; set; }

    /// <summary>MIME 类型(部分新接口返回)</summary>
    [JsonPropertyName("mimeType")]
    public string MimeType { get; set; } = string.Empty;

    /// <summary>便捷属性:实际播放 URL(优先 baseUrl,其次 base_url)</summary>
    public string Url => !string.IsNullOrEmpty(BaseUrl) ? BaseUrl : FallbackUrl;
}

/// <summary>老格式 durl 列表中的单个切片项</summary>
public class DurlItem
{
    /// <summary>切片 URL</summary>
    [JsonPropertyName("url")]
    public string Url { get; set; } = string.Empty;
}
