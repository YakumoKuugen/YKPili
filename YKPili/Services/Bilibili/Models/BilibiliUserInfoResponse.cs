using System.Text.Json.Serialization;

namespace YKPili.Services.Bilibili.Models;

/// <summary>
/// BilibiliUserInfoResponse:用户导航信息接口响应
/// 接口 https://api.bilibili.com/x/web-interface/nav 同时返回登录态与 WBI 签名所需的密钥。
/// </summary>
public class BilibiliUserInfoResponse
{
    /// <summary>业务状态码(0 表示成功)</summary>
    [JsonPropertyName("code")]
    public int Code { get; set; }

    /// <summary>用户数据载荷</summary>
    [JsonPropertyName("data")]
    public UserInfoData? Data { get; set; }
}

/// <summary>用户数据载荷</summary>
public class UserInfoData
{
    /// <summary>当前 SESSDATA 是否仍处于登录状态</summary>
    [JsonPropertyName("isLogin")]
    public bool IsLogin { get; set; }

    /// <summary>用户 mid(数字 ID)</summary>
    [JsonPropertyName("mid")]
    public long Mid { get; set; }

    /// <summary>用户昵称</summary>
    [JsonPropertyName("uname")]
    public string Uname { get; set; } = string.Empty;

    /// <summary>用户头像 URL</summary>
    [JsonPropertyName("face")]
    public string Face { get; set; } = string.Empty;

    /// <summary>WBI 签名密钥对(用于接口风控签名)</summary>
    [JsonPropertyName("wbi_img")]
    public WbiImg? WbiImg { get; set; }
}

/// <summary>WBI 签名密钥对</summary>
public class WbiImg
{
    /// <summary>img_key 来源 URL(取文件名部分作为 img_key)</summary>
    [JsonPropertyName("img_url")]
    public string ImgUrl { get; set; } = string.Empty;

    /// <summary>sub_key 来源 URL(取文件名部分作为 sub_key)</summary>
    [JsonPropertyName("sub_url")]
    public string SubUrl { get; set; } = string.Empty;
}
