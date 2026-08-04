// 引入系统与加密/编码相关类型
using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Threading.Tasks;
// 引入响应模型
using YKPili.Services.Bilibili.Models;
// 别名:全局命名空间下的 UserSession
using UserSession = global::YKPili.Models.UserSession;

namespace YKPili.Services.Bilibili;

/// <summary>
/// BilibiliApi:B 站接口的具体实现
/// 关键能力:
///   1. 二维码登录流程(申请 + 轮询 + 提取 Set-Cookie)
///   2. WBI 签名算法(mixin_key + w_rid),用于通过风控校验
///   3. 推荐流、视频详情、播放地址等核心 API 调用
/// </summary>
public class BilibiliApi : IBilibiliApi
{
    private readonly HttpClient _httpClient;

    // ---- 接口地址常量 ----
    private const string QrCodeGenerateUrl =
        "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";
    private const string QrCodePollUrl =
        "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";
    private const string UserInfoUrl =
        "https://api.bilibili.com/x/web-interface/nav";
    private const string RecommendUrl =
        "https://api.bilibili.com/x/web-interface/wbi/index/top/feed/rcmd";

    // ---- WBI 签名核心:置换表 ----
    // 该表是 B 站前端混淆算法固定的 64 位置换序列,用于从 nav 接口返回的
    // img_key + sub_key 提取 mixin key。
    private static readonly int[] MixinKeyEncTab =
    [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35,
        27, 43, 5, 49, 33, 9, 42, 19, 29, 28, 14, 39, 12, 38, 41, 13,
        37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 52, 44, 34
    ];

    // JSON 序列化配置:忽略属性名大小写,提高反序列化容错性
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    public BilibiliApi(HttpClient httpClient)
    {
        _httpClient = httpClient;
    }

    /// <summary>申请二维码登录:获取二维码 URL 与唯一 key</summary>
    public async Task<LoginQrCodeResponse> GenerateQrCodeAsync()
    {
        var response = await _httpClient.GetAsync(QrCodeGenerateUrl);
        response.EnsureSuccessStatusCode();
        var json = await response.Content.ReadAsStringAsync();
        return JsonSerializer.Deserialize<LoginQrCodeResponse>(json, JsonOptions) ?? new();
    }

    /// <summary>
    /// 轮询登录状态。
    /// 当 pollResponse.Code == 0 且 Data.Code == 0 时表示登录成功,
    /// 并从响应头的 Set-Cookie 中提取 SESSDATA / bili_jct / DedeUserID。
    /// </summary>
    public async Task<(LoginPollResponse PollResponse, UserSession? Session)> PollLoginAsync(
        string qrcodeKey)
    {
        var response = await _httpClient.GetAsync(
            $"{QrCodePollUrl}?qrcode_key={Uri.EscapeDataString(qrcodeKey)}");
        response.EnsureSuccessStatusCode();

        var json = await response.Content.ReadAsStringAsync();
        var pollResponse = JsonSerializer.Deserialize<LoginPollResponse>(json, JsonOptions) ?? new();

        UserSession? session = null;

        // 登录成功的标志:外层 code == 0 且内层 data.code == 0
        if (pollResponse.Code == 0 && pollResponse.Data?.Code == 0)
        {
            session = ExtractSessionFromCookies(response);
            if (session != null)
                session.IsLoggedIn = true;
        }

        return (pollResponse, session);
    }

    /// <summary>通过 SESSDATA 获取用户信息(含 WBI 签名所需的 img_key/sub_key)</summary>
    public async Task<BilibiliUserInfoResponse> GetUserInfoAsync(string sessdata)
    {
        var request = new HttpRequestMessage(HttpMethod.Get, UserInfoUrl);
        // 必须带上 SESSDATA Cookie 才会返回已登录信息
        request.Headers.Add("Cookie", $"SESSDATA={sessdata}");
        // 多数 B 站接口对 Referer 有强制校验
        request.Headers.Add("Referer", "https://www.bilibili.com/");

        var response = await _httpClient.SendAsync(request);
        response.EnsureSuccessStatusCode();

        var json = await response.Content.ReadAsStringAsync();
        return JsonSerializer.Deserialize<BilibiliUserInfoResponse>(json, JsonOptions) ?? new();
    }

    /// <summary>获取首页推荐视频列表(需要 WBI 签名)</summary>
    public async Task<BilibiliRecommendResponse> GetRecommendVideosAsync(UserSession session)
    {
        var baseParams = new Dictionary<string, string>
        {
            { "fresh_type", "3" },  // 3 = APP 推荐流
            { "version", "1" },
            { "ps", "30" }          // 每页数量
        };

        // 调用签名工具:基础 URL + 业务参数 + session(用于取 img_key/sub_key)
        var signedUrl = BuildWbiSignedUrl(RecommendUrl, baseParams, session);

        var request = new HttpRequestMessage(HttpMethod.Get, signedUrl);
        request.Headers.Add("Cookie", session.ToCookieHeader());
        request.Headers.Add("Referer", "https://www.bilibili.com/");

        var response = await _httpClient.SendAsync(request);
        response.EnsureSuccessStatusCode();

        var json = await response.Content.ReadAsStringAsync();
        return JsonSerializer.Deserialize<BilibiliRecommendResponse>(json, JsonOptions) ?? new();
    }

    /// <summary>获取视频基础信息(包含 cid、分页列表等)</summary>
    public async Task<VideoInfoResponse> GetVideoInfoAsync(string bvid, UserSession session)
    {
        var baseParams = new Dictionary<string, string>
        {
            { "bvid", bvid }
        };

        var signedUrl = BuildWbiSignedUrl(
            "https://api.bilibili.com/x/web-interface/wbi/view", baseParams, session);

        var request = new HttpRequestMessage(HttpMethod.Get, signedUrl);
        request.Headers.Add("Cookie", session.ToCookieHeader());
        request.Headers.Add("Referer", "https://www.bilibili.com/");

        var response = await _httpClient.SendAsync(request);
        response.EnsureSuccessStatusCode();

        var json = await response.Content.ReadAsStringAsync();
        return JsonSerializer.Deserialize<VideoInfoResponse>(json, JsonOptions) ?? new();
    }

    /// <summary>
    /// 获取视频播放地址。
    /// 参数说明:
    ///   qn=127    -> 请求最高画质
    ///   fnval=4048 -> 返回 DASH 格式(分视频/音轨)
    ///   fourk=1   -> 允许 4K
    /// </summary>
    public async Task<VideoPlayUrlResponse> GetPlayUrlAsync(string bvid, long cid, UserSession session)
    {
        var baseParams = new Dictionary<string, string>
        {
            { "bvid", bvid },
            { "cid", cid.ToString() },
            { "qn", "127" },
            { "fnval", "4048" },
            { "fnver", "0" },
            { "fourk", "1" }
        };

        var signedUrl = BuildWbiSignedUrl(
            "https://api.bilibili.com/x/player/wbi/playurl", baseParams, session);

        var request = new HttpRequestMessage(HttpMethod.Get, signedUrl);
        request.Headers.Add("Cookie", session.ToCookieHeader());
        request.Headers.Add("Referer", "https://www.bilibili.com/");

        var response = await _httpClient.SendAsync(request);
        response.EnsureSuccessStatusCode();

        var json = await response.Content.ReadAsStringAsync();
        return JsonSerializer.Deserialize<VideoPlayUrlResponse>(json, JsonOptions) ?? new();
    }

    // ================================================================
    // WBI 签名工具方法
    // 算法步骤:
    //   1) 将 img_key 与 sub_key 拼接,按 MixinKeyEncTab 置换取前 32 位得到 mixin_key
    //   2) 业务参数按 key 字典序排序,过滤特殊字符
    //   3) 追加 wts(秒级时间戳)
    //   4) 拼接 mixin_key 后做 MD5,得到 w_rid
    //   5) 拼成最终 URL: ?...&w_rid=xxx
    // ================================================================
    private static string BuildWbiSignedUrl(
        string baseUrl,
        Dictionary<string, string> queryParams,
        UserSession session)
    {
        // 必须先有 img_key/sub_key,否则提示调用方先调用 GetUserInfoAsync
        if (session.ImgKey == null || session.SubKey == null)
            throw new InvalidOperationException(
                "WBI keys not available. Call GetUserInfoAsync first to obtain img_key and sub_key.");

        var mixinKey = GetMixinKey(session.ImgKey, session.SubKey);
        // 当前秒级时间戳(注意:服务端可能容忍一定时间漂移)
        var currentTimestamp = DateTimeOffset.UtcNow.ToUnixTimeSeconds().ToString();

        // 追加 wts 参数
        queryParams["wts"] = currentTimestamp;

        // 按 key 字典序排序
        var sortedParams = new SortedDictionary<string, string>(queryParams);

        // 拼接查询字符串
        var queryString = new StringBuilder();
        foreach (var kvp in sortedParams)
        {
            if (queryString.Length > 0)
                queryString.Append('&');
            queryString.Append($"{Uri.EscapeDataString(kvp.Key)}={Uri.EscapeDataString(kvp.Value)}");
        }

        // 拼接 mixin_key 后计算 MD5 = w_rid
        var rawForHash = queryString + mixinKey;
        var wRid = ComputeMd5Hash(rawForHash);

        return $"{baseUrl}?{queryString}&w_rid={wRid}";
    }

    // 根据置换表取前 32 位字符
    private static string GetMixinKey(string imgKey, string subKey)
    {
        var raw = imgKey + subKey;
        var chars = new char[32];
        for (int i = 0; i < 32; i++)
            chars[i] = raw[MixinKeyEncTab[i]];
        return new string(chars);
    }

    // 计算字符串的 MD5 哈希(小写 16 进制)
    private static string ComputeMd5Hash(string input)
    {
        var inputBytes = Encoding.UTF8.GetBytes(input);
        var hashBytes = MD5.HashData(inputBytes);
        var sb = new StringBuilder();
        foreach (var b in hashBytes)
            sb.Append(b.ToString("x2"));
        return sb.ToString();
    }

    /// <summary>
    /// 从登录轮询响应的 Set-Cookie 头中提取会话信息。
    /// Set-Cookie 可能同时携带 SESSDATA / bili_jct / DedeUserID 等多个键。
    /// </summary>
    private static UserSession? ExtractSessionFromCookies(HttpResponseMessage response)
    {
        var sessdata = string.Empty;
        var biliJct = string.Empty;
        var dedeUserId = string.Empty;

        if (response.Headers.TryGetValues("Set-Cookie", out var cookieHeaders))
        {
            foreach (var cookieHeader in cookieHeaders)
            {
                // Set-Cookie 头中,多个 cookie 用 ", " 分隔(且同一 cookie 内属性以 ";" 分隔)
                foreach (var cookie in cookieHeader.Split(';', StringSplitOptions.TrimEntries))
                {
                    var eq = cookie.IndexOf('=');
                    if (eq < 0) continue;

                    var key = cookie[..eq].Trim();
                    var value = cookie[(eq + 1)..].Trim();

                    // 仅提取我们关心的三个键
                    switch (key)
                    {
                        case "SESSDATA":
                            sessdata = value;
                            break;
                        case "bili_jct":
                            biliJct = value;
                            break;
                        case "DedeUserID":
                            dedeUserId = value;
                            break;
                    }
                }
            }
        }

        // 必须有 SESSDATA 才算登录成功
        if (string.IsNullOrEmpty(sessdata))
            return null;

        return new UserSession
        {
            SessData = sessdata,
            BiliJct = biliJct,
            DedeUserId = long.TryParse(dedeUserId, out var id) ? id : 0
        };
    }
}
