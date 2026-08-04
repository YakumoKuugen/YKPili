// 引入基础类型、IO、JSON、异步任务
using System;
using System.IO;
using System.Text.Json;
using System.Threading.Tasks;
// 引入 CommunityToolkit.Mvvm 源生成器
using CommunityToolkit.Mvvm.ComponentModel;
// 引入项目模型与服务
using YKPili.Models;
using YKPili.Services;

namespace YKPili.ViewModels;

/// <summary>
/// MainViewModel:主窗口的根 ViewModel
/// 聚合子 ViewModel(Home / Login / VideoPlayer),协调登录态切换、
/// 推荐流加载、Cookie 持久化与恢复等横切关注点。
/// </summary>
public partial class MainViewModel : ViewModelBase
{
    /// <summary>对外暴露的 API 管理器(供子 VM 共享同一会话)</summary>
    public IApiManager ApiManager { get; }

    /// <summary>首页推荐 ViewModel</summary>
    public HomeViewModel Home { get; }

    /// <summary>登录页 ViewModel</summary>
    public LoginViewModel Login { get; }

    // 视频播放器 ViewModel(对外暴露为可空属性,允许后续切换)
    private VideoPlayerViewModel? _videoPlayer;
    public VideoPlayerViewModel? VideoPlayer
    {
        get => _videoPlayer;
        set { _videoPlayer = value; OnPropertyChanged(); }
    }

    /// <summary>是否已登录(用于在 MainView 中切换 Login/Home 视图)</summary>
    [ObservableProperty]
    private bool _isLoggedIn;

    // session.json 持久化路径(LocalAppData/YKPili/session.json)
    private static readonly string CookieFilePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "YKPili", "session.json");

    public MainViewModel()
    {
        // 构造共享的 API 管理器与各子 ViewModel
        ApiManager = new ApiManager();
        Home = new HomeViewModel();
        Login = new LoginViewModel(ApiManager);
        VideoPlayer = new VideoPlayerViewModel(ApiManager);

        // 订阅子 VM 的事件
        Login.LoginSucceeded += async () => await OnLoginSuccessAsync();
        Home.VideoPlayRequested += OnVideoPlayRequested;
        Home.LoadMoreRequested += async () => await LoadMoreVideosAsync();

        // 启动时尝试从本地 cookie 恢复登录态
        _ = TryRestoreSessionAsync();
    }

    /// <summary>从磁盘恢复 Cookie(若存在),成功则直接进入首页</summary>
    private async Task TryRestoreSessionAsync()
    {
        try
        {
            if (!File.Exists(CookieFilePath)) return;

            var json = await File.ReadAllTextAsync(CookieFilePath);
            var session = JsonSerializer.Deserialize<UserSession>(json);
            if (session == null || string.IsNullOrEmpty(session.SessData)) return;

            System.Diagnostics.Trace.WriteLine($"[Bilibili] Restored session: {session.Nickname}");
            ApiManager.SetSession(session);
            IsLoggedIn = true;
            await LoadRecommendVideosAsync();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[Bilibili] Restore session failed: {ex.Message}");
        }
    }

    /// <summary>登录成功后的回调:切到首页并加载推荐</summary>
    private async Task OnLoginSuccessAsync()
    {
        IsLoggedIn = true;
        await LoadRecommendVideosAsync();
    }

    /// <summary>
    /// 加载推荐流:
    ///   1) 调用 nav 接口拿 WBI 签名所需的 img_key/sub_key 与用户信息
    ///   2) 把会话信息写回磁盘
    ///   3) 调用推荐接口,把结果交给 HomeViewModel
    /// </summary>
    private async Task LoadRecommendVideosAsync()
    {
        try
        {
            var session = ApiManager.CurrentSession;
            if (session == null || ApiManager.Bilibili == null) return;

            // 1. 拉取用户信息以获取 WBI 密钥
            var userInfo = await ApiManager.Bilibili.GetUserInfoAsync(session.SessData);
            System.Diagnostics.Trace.WriteLine($"[Bilibili] UserInfo: code={userInfo.Code} login={userInfo.Data?.IsLogin} name={userInfo.Data?.Uname} hasWbi={userInfo.Data?.WbiImg != null}");

            if (userInfo.Data?.WbiImg != null)
            {
                // img_key/sub_key 是 URL 中的文件名部分(去掉扩展名)
                var imgUrl = userInfo.Data.WbiImg.ImgUrl;
                var subUrl = userInfo.Data.WbiImg.SubUrl;
                session.ImgKey = ExtractFileNameWithoutExtension(imgUrl);
                session.SubKey = ExtractFileNameWithoutExtension(subUrl);

                // 顺手补全昵称与头像
                if (!string.IsNullOrEmpty(userInfo.Data.Uname))
                    session.Nickname = userInfo.Data.Uname;
                if (!string.IsNullOrEmpty(userInfo.Data.Face))
                    session.FaceUrl = userInfo.Data.Face;
            }

            // 2. 持久化会话
            await SaveSessionAsync(session);

            // 3. 拉取推荐流
            var rec = await ApiManager.Bilibili.GetRecommendVideosAsync(session);
            System.Diagnostics.Trace.WriteLine($"[Bilibili] Recommend: code={rec.Code} count={rec.Data?.Items?.Count ?? 0}");

            if (rec.Code == 0 && rec.Data?.Items is { Count: > 0 })
            {
                // 调试:打印前 3 条标题
                for (int i = 0; i < Math.Min(3, rec.Data.Items.Count); i++)
                    System.Diagnostics.Trace.WriteLine($"[Bilibili]   [{i}] {rec.Data.Items[i].Title}");
                Home.LoadRecommendVideos(rec.Data.Items);
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[Bilibili] ERROR: {ex}");
        }
    }

    /// <summary>滚动到底部时由 HomeViewModel 触发:追加更多推荐</summary>
    private async Task LoadMoreVideosAsync()
    {
        try
        {
            var session = ApiManager.CurrentSession;
            if (session == null || ApiManager.Bilibili == null) return;

            System.Diagnostics.Trace.WriteLine($"[Bilibili] Loading more videos (current count: {Home.ItemCount})...");
            var rec = await ApiManager.Bilibili.GetRecommendVideosAsync(session);
            System.Diagnostics.Trace.WriteLine($"[Bilibili] LoadMore: code={rec.Code} count={rec.Data?.Items?.Count ?? 0}");

            if (rec.Code == 0 && rec.Data?.Items is { Count: > 0 })
                Home.AppendRecommendVideos(rec.Data.Items);
        }
        catch (Exception ex)
        {
            // 出错时也通知 HomeViewModel 解除加载中状态
            Home.OnLoadMoreCompleted();
            System.Diagnostics.Trace.WriteLine($"[Bilibili] LoadMore ERROR: {ex.Message}");
        }
    }

    /// <summary>把会话以 JSON 写入 LocalAppData/YKPili/session.json</summary>
    private static async Task SaveSessionAsync(UserSession session)
    {
        try
        {
            var dir = Path.GetDirectoryName(CookieFilePath)!;
            Directory.CreateDirectory(dir);
            var json = JsonSerializer.Serialize(session);
            await File.WriteAllTextAsync(CookieFilePath, json);
        }
        catch { }
    }

    // 单个视频被点击的回调:转发给播放器 VM
    private async void OnVideoPlayRequested(CoverItem item)
    {
        if (VideoPlayer != null)
            await VideoPlayer.PlayVideoAsync(item);
    }

    // 从 URL 中截取文件名(去掉扩展名)——WBI 签名时使用
    private static string ExtractFileNameWithoutExtension(string url)
    {
        var lastSlash = url.LastIndexOf('/');
        var withoutQuery = lastSlash >= 0 ? url[(lastSlash + 1)..] : url;
        var dotIndex = withoutQuery.IndexOf('.');
        return dotIndex >= 0 ? withoutQuery[..dotIndex] : withoutQuery;
    }
}
