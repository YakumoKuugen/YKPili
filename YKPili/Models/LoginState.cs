namespace YKPili.Models;

/// <summary>
/// LoginState:登录流程状态枚举
/// 用于驱动登录页 UI 显示与轮询逻辑的状态机。
/// </summary>
public enum LoginState
{
    /// <summary>空闲:初始状态,等待用户点击登录</summary>
    Idle,

    /// <summary>二维码已生成,等待用户扫描</summary>
    QrCodeLoaded,

    /// <summary>二维码已被扫描,等待用户确认(B 站 App 上点确认)</summary>
    QrCodeScanned,

    /// <summary>二维码已过期,需要重新生成</summary>
    QrCodeExpired,

    /// <summary>登录成功</summary>
    LoginSuccess,

    /// <summary>登录失败</summary>
    LoginFailed
}
