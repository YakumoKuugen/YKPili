using System.Text.Json.Serialization;

namespace YKPili.Services.Bilibili.Models;

/// <summary>
/// LoginPollResponse:登录二维码轮询接口响应
/// 接口 https://passport.bilibili.com/x/passport-login/web/qrcode/poll 的响应模型。
/// 注意:此接口有两层 code:
///   - 外层 code:接口是否成功调用
///   - 内层 data.code:扫码状态码(0=成功 / 86038=过期 / 86090=已扫 / 86101=未扫)
/// </summary>
public class LoginPollResponse
{
    /// <summary>外层业务状态码</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>内层数据(扫码状态)</summary>
    [JsonPropertyName("data")]
    public PollData? Data { get; set; }

    /// <summary>外层消息</summary>
    [JsonPropertyName("message")]
    public string Message { get; set; } = string.Empty;
}

/// <summary>登录轮询内层数据</summary>
public class PollData
{
    /// <summary>扫码状态码</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>状态描述</summary>
    [JsonPropertyName("message")]
    public string Message { get; set; } = string.Empty;

    /// <summary>登录成功后的跳转 URL(包含登录态 Cookie,备用)</summary>
    [JsonPropertyName("url")]
    public string Url { get; set; } = string.Empty;
}
