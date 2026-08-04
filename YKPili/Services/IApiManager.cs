// 引入模型与具体服务命名空间
using YKPili.Models;
using YKPili.Services.Bilibili;

namespace YKPili.Services;

/// <summary>
/// IApiManager:统一对外的 API 管理器接口
/// 聚合所有第三方/平台 API 的访问入口(目前仅包含 Bilibili),并保存全局登录会话。
/// </summary>
public interface IApiManager
{
    /// <summary>B 站 API 访问入口(懒加载,首次访问时实例化)</summary>
    IBilibiliApi? Bilibili { get; }

    /// <summary>当前登录会话(未登录时为 null)</summary>
    UserSession? CurrentSession { get; }

    /// <summary>设置当前登录会话</summary>
    void SetSession(UserSession session);

    /// <summary>清除当前登录会话(登出)</summary>
    void ClearSession();
}
