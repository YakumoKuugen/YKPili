// 引入基础类型与异步、HTTP 相关命名空间
using System;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
// 引入 Avalonia 的 Bitmap 类型(用于显示二维码图片)
using Avalonia.Media.Imaging;
// 引入 CommunityToolkit.Mvvm 的源生成器
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
// 引入项目内的模型与服务
using YKPili.Models;
using YKPili.Services;

namespace YKPili.ViewModels;

/// <summary>
/// LoginViewModel:登录页 ViewModel
/// 负责驱动二维码登录流程:申请二维码 -> 显示二维码 -> 轮询扫码状态 -> 通知登录成功。
/// </summary>
public partial class LoginViewModel : ViewModelBase
{
    // 注入的 API 管理器(用于访问 Bilibili 接口)
    private readonly IApiManager _apiManager;
    // 单独使用的 HttpClient(用于下载二维码图片,与 ApiManager 内的分离避免互相干扰)
    private readonly HttpClient _httpClient;
    // 轮询任务的取消令牌源
    private CancellationTokenSource? _pollCts;

    /// <summary>当前登录流程状态</summary>
    [ObservableProperty]
    private LoginState _loginState = LoginState.Idle;

    /// <summary>二维码原始 URL(由 B 站返回的登录确认页 URL)</summary>
    [ObservableProperty]
    private string _qrCodeUrl = string.Empty;

    /// <summary>已渲染好的二维码图片(Bitmap),可直接绑定到 Image.Source</summary>
    [ObservableProperty]
    private Bitmap? _qrCodeImage;

    /// <summary>状态消息文本(显示在登录页底部)</summary>
    [ObservableProperty]
    private string _statusMessage = "Click 'Login' to start QR code login.";

    /// <summary>是否正在轮询(用于控制 UI 中 Login/Cancel 按钮的显隐)</summary>
    [ObservableProperty]
    private bool _isPolling;

    /// <summary>登录成功事件(由 MainViewModel 订阅,用于切换界面并加载推荐数据)</summary>
    public event Action? LoginSucceeded;

    // 设计时/默认构造:用内置 ApiManager
    public LoginViewModel() : this(new ApiManager()) { }

    // 运行时构造:支持依赖注入传入
    public LoginViewModel(IApiManager apiManager)
    {
        _apiManager = apiManager;
        _httpClient = new HttpClient();
    }

    /// <summary>开始登录:申请二维码、显示图片、启动轮询</summary>
    [RelayCommand]
    private async Task StartLoginAsync()
    {
        try
        {
            LoginState = LoginState.Idle;
            StatusMessage = "Generating QR code...";

            // 调用 B 站接口申请二维码
            var qrResult = await _apiManager.Bilibili!.GenerateQrCodeAsync();

            if (qrResult.Code != 0 || qrResult.Data == null)
            {
                StatusMessage = $"Failed: {qrResult.Message}";
                LoginState = LoginState.LoginFailed;
                return;
            }

            // 保存 URL 与 key
            QrCodeUrl = qrResult.Data.Url;
            LoginState = LoginState.QrCodeLoaded;
            StatusMessage = "Scan the QR code with Bilibili app.";

            // 后台下载二维码图片(不 await,避免阻塞主流程)
            _ = LoadQrCodeImageAsync(qrResult.Data.Url);

            // 若之前已有轮询,先取消
            _pollCts?.Cancel();
            _pollCts = new CancellationTokenSource();
            // 启动新的轮询任务
            _ = PollLoginAsync(qrResult.Data.QrcodeKey, _pollCts.Token);
        }
        catch (Exception ex)
        {
            StatusMessage = $"Error: {ex.Message}";
            LoginState = LoginState.LoginFailed;
        }
    }

    /// <summary>
    /// 轮询扫码状态:每 2 秒请求一次,直到成功、过期或被取消。
    /// 根据 B 站返回的内层 code 区分不同阶段:
    ///   86101 = 未扫码(继续轮询)
    ///   86090 = 已扫码(等待确认)
    ///   86038 = 二维码已过期
    ///   0     = 登录成功
    /// </summary>
    private async Task PollLoginAsync(string qrcodeKey, CancellationToken ct)
    {
        IsPolling = true;

        try
        {
            while (!ct.IsCancellationRequested)
            {
                // 2 秒一次,可在被取消时立刻跳出
                await Task.Delay(2000, ct);

                var (pollResult, session) = await _apiManager.Bilibili!.PollLoginAsync(qrcodeKey);

                switch (pollResult.Data?.Code)
                {
                    case 0:
                        // 登录成功
                        LoginState = LoginState.LoginSuccess;
                        StatusMessage = "Login successful!";
                        if (session != null)
                        {
                            _apiManager.SetSession(session);
                        }
                        IsPolling = false;
                        // 通知 MainViewModel 切换界面
                        LoginSucceeded?.Invoke();
                        return;

                    case 86038:
                        // 二维码已过期
                        LoginState = LoginState.QrCodeExpired;
                        StatusMessage = "QR code expired. Generate a new one.";
                        IsPolling = false;
                        return;

                    case 86090:
                        // 已扫描,等待用户在手机上确认
                        LoginState = LoginState.QrCodeScanned;
                        StatusMessage = "Scanned! Confirm on Bilibili app...";
                        break;

                    case 86101:
                        // 未扫码,继续轮询(无操作)
                        break;

                    default:
                        // 其他错误:仅在接口本身失败时提示
                        if (pollResult.Code != 0)
                            StatusMessage = $"Error: {pollResult.Message}";
                        break;
                }
            }
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            StatusMessage = $"Error: {ex.Message}";
            LoginState = LoginState.LoginFailed;
        }
        finally
        {
            IsPolling = false;
        }
    }

    /// <summary>取消登录:停止轮询、重置状态、清空二维码</summary>
    [RelayCommand]
    private void CancelLogin()
    {
        _pollCts?.Cancel();
        IsPolling = false;
        LoginState = LoginState.Idle;
        StatusMessage = "Login cancelled.";
        QrCodeUrl = string.Empty;
        QrCodeImage = null;
    }

    /// <summary>
    /// 下载二维码图片。
    /// 注意:B 站 generate 接口返回的 url 字段是登录确认页(HTML),并非图片地址,
    /// 所以这里借助第三方二维码生成服务(goqr.me 的 api.qrserver.com)渲染为 PNG。
    /// </summary>
    private async Task LoadQrCodeImageAsync(string loginUrl)
    {
        try
        {
            var qrImageUrl = "https://api.qrserver.com/v1/create-qr-code/?size=200x200&data="
                + Uri.EscapeDataString(loginUrl);
            var response = await _httpClient.GetAsync(qrImageUrl);
            response.EnsureSuccessStatusCode();
            var stream = await response.Content.ReadAsStreamAsync();
            // 直接从流构造 Bitmap 并绑定到 UI
            QrCodeImage = new Bitmap(stream);
        }
        catch (Exception ex)
        {
            StatusMessage = $"Failed to load QR image: {ex.Message}";
        }
    }
}
