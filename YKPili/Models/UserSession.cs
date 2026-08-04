// 引入 List 泛型,用于构造 Cookie 头字符串
using System.Collections.Generic;

namespace YKPili.Models;

/// <summary>
/// UserSession:用户登录会话信息
/// 持久化在本地(LocalAppData 下的 session.json),用于下次启动时免登录恢复。
/// </summary>
public class UserSession
{
    /// <summary>B 站用户 ID(DedeUserID 字段值)</summary>
    public long DedeUserId { get; set; }

    /// <summary>SESSDATA:B 站登录态的核心 Cookie</summary>
    public string SessData { get; set; } = string.Empty;

    /// <summary>bili_jct:CSRF Token,部分写操作接口需要随请求携带</summary>
    public string BiliJct { get; set; } = string.Empty;

    /// <summary>用户昵称</summary>
    public string Nickname { get; set; } = string.Empty;

    /// <summary>用户头像 URL</summary>
    public string FaceUrl { get; set; } = string.Empty;

    /// <summary>是否已登录(冗余字段,便于外部快速判断)</summary>
    public bool IsLoggedIn { get; set; }

    /// <summary>WBI 签名用 img_key(从用户信息接口的 wbi_img.img_url 提取)</summary>
    public string? ImgKey { get; set; }

    /// <summary>WBI 签名用 sub_key(从用户信息接口的 wbi_img.sub_url 提取)</summary>
    public string? SubKey { get; set; }

    /// <summary>
    /// 将当前会话序列化为标准的 Cookie 请求头字符串。
    /// 形如: "SESSDATA=xxx; bili_jct=xxx; DedeUserID=xxx"
    /// </summary>
    public string ToCookieHeader()
    {
        var parts = new List<string>();
        if (!string.IsNullOrEmpty(SessData))
            parts.Add($"SESSDATA={SessData}");
        if (!string.IsNullOrEmpty(BiliJct))
            parts.Add($"bili_jct={BiliJct}");
        if (DedeUserId > 0)
            parts.Add($"DedeUserID={DedeUserId}");
        // 使用 "; " 分隔,符合 HTTP Cookie 头的格式约定
        return string.Join("; ", parts);
    }
}
