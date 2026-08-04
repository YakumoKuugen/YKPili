using System.Text.Json.Serialization;

namespace YKPili.Services.Bilibili.Models;

/// <summary>
/// LoginQrCodeResponse:申请登录二维码接口响应
/// 接口 https://passport.bilibili.com/x/passport-login/web/qrcode/generate 的响应模型。
/// 注意:此接口返回的 url 字段是登录确认页(HTML)的 URL,不是图片地址,
/// 通常需要借助第三方二维码生成服务(如 api.qrserver.com)再渲染为图片。
/// </summary>
public class LoginQrCodeResponse
{
    /// <summary>业务状态码(0 表示成功)</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>二维码数据</summary>
    [JsonPropertyName("data")]
    public QrCodeData? Data { get; set; }

    /// <summary>错误或提示信息</summary>
    [JsonPropertyName("message")]
    public string Message { get; set; } = string.Empty;
}

/// <summary>二维码数据</summary>
public class QrCodeData
{
    /// <summary>登录确认页 URL(需二次生成二维码图片)</summary>
    [JsonPropertyName("url")]
    public string Url { get; set; } = string.Empty;

    /// <summary>二维码唯一 key(轮询登录时使用)</summary>
    [JsonPropertyName("qrcode_key")]
    public string QrcodeKey { get; set; } = string.Empty;
}
