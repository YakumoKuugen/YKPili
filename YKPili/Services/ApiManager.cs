// 引入系统基础类型与 HTTP 客户端
using System;
using System.Net.Http;
// 引入模型与具体 API 实现
using YKPili.Models;
using YKPili.Services.Bilibili;

namespace YKPili.Services;

/// <summary>
/// ApiManager:统一 API 管理器实现
/// 负责管理共享的 HttpClient 与登录会话,并按需懒加载具体 API(Bilibili)。
/// </summary>
public class ApiManager : IApiManager
{
    // 共享 HTTP 客户端,所有 API 调用复用同一实例(避免重复创建连接)
    private readonly HttpClient _httpClient;

    // B 站 API 实例,首次访问时懒加载
    private IBilibiliApi? _bilibili;

    /// <summary>B 站 API 访问入口(??= 实现懒加载)</summary>
    public IBilibiliApi? Bilibili => _bilibili ??= new BilibiliApi(_httpClient);

    /// <summary>当前登录会话</summary>
    public UserSession? CurrentSession { get; private set; }

    public ApiManager()
    {
        // 初始化共享 HttpClient:15 秒超时
        _httpClient = new HttpClient
        {
            Timeout = TimeSpan.FromSeconds(15)
        };
        // 添加默认 User-Agent,模拟主流浏览器,部分接口会对 UA 做校验
        _httpClient.DefaultRequestHeaders.Add("User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36");
    }

    /// <summary>写入登录会话</summary>
    public void SetSession(UserSession session)
    {
        CurrentSession = session;
    }

    /// <summary>清除登录会话</summary>
    public void ClearSession()
    {
        CurrentSession = null;
    }
}
