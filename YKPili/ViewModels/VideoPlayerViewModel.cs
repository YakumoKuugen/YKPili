// 引入基础类型、LINQ 与异步任务
using System;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading.Tasks;
// 引入 CommunityToolkit.Mvvm 源生成器
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
// 引入 Avalonia 的计时器(用于按帧刷新播放进度)
using Avalonia.Threading;
// 引入项目内模型、服务、缓存与 B 站模型
using YKPili.Models;
using YKPili.Platforms;
using YKPili.Services;
using YKPili.Services.Bilibili.Models;
using YKPili.Services.Cache;

namespace YKPili.ViewModels;

/// <summary>
/// VideoPlayerViewModel:视频播放器页 ViewModel
/// 负责根据 CoverItem(bvid)获取视频信息与播放地址,选择最佳音视频轨道,
/// 并将 URL 暴露给 View 触发原生播放器播放;
/// 同时维护控制栏所需的状态(进度、时长、音量、倍速、控制栏显隐与 HUD 提示)。
/// </summary>
public partial class VideoPlayerViewModel : ViewModelBase
{
    private readonly IApiManager _apiManager;

    // 平台播放器引用:仅用于**读取**进度与时长。
    // 音量/倍速等下发仍走"属性变化 → View 监听 → 调用播放器"这条链路,
    // 与既有的 IsPlaying 处理方式保持一致,避免两套驱动逻辑。
    private IPlatformMediaPlayer? _mediaPlayer;

    // 进度刷新定时器(只在播放中运行)
    private DispatcherTimer? _positionTimer;

    // HUD 自动隐藏的令牌:连续操作时,旧的延迟回调不应把新提示提前关掉
    private int _hudToken;

    /// <summary>
    /// 注入平台播放器。由 View 在创建原生播放器后调用。
    /// </summary>
    public void AttachPlayer(IPlatformMediaPlayer player)
    {
        _mediaPlayer = player;
        // 应用上次记住的音量与倍速
        LoadPreferences();
    }

    /// <summary>播放器是否可见(控制浮层整体显隐)</summary>
    [ObservableProperty]
    private bool _isVisible;

    /// <summary>是否正在播放</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(PlayPauseGlyph))]
    private bool _isPlaying;

    /// <summary>是否处于加载中(显示 Loading 提示)</summary>
    [ObservableProperty]
    private bool _isLoading;

    /// <summary>是否显示暂停遮罩(用户点击暂停时为 true)</summary>
    [ObservableProperty]
    private bool _isPausedOverlay;

    /// <summary>当前播放视频的标题</summary>
    [ObservableProperty]
    private string _title = string.Empty;

    /// <summary>当前播放视频的包名(实际为 bvid,用于缓存分目录)</summary>
    [ObservableProperty]
    private string _packageName = string.Empty;

    /// <summary>状态文本(显示在 Loading 框中)</summary>
    [ObservableProperty]
    private string _statusText = string.Empty;

    /// <summary>视频流 URL(DASH 模式下的视频分片 URL)</summary>
    [ObservableProperty]
    private string? _videoUrl;

    /// <summary>音频流 URL(DASH 模式下的音频分片 URL)</summary>
    [ObservableProperty]
    private string? _audioUrl;

    // 调试用:AudioUrl 变化时打印日志
    partial void OnAudioUrlChanged(string? value)
    {
        System.Diagnostics.Trace.WriteLine($"[VideoPlayer] AudioUrl set: hasAudio={value != null}");
    }

    /// <summary>暗色遮罩的不透明度(0 = 播放,0.5 = 暂停时压暗画面)</summary>
    [ObservableProperty]
    private double _dimOpacity;

    // ================================================================
    // 控制栏状态
    //
    // 参考 BewlyCat 播放器交互的两个要点:
    //   1. 进度与时长常驻可读,操作即时反馈;
    //   2. 反馈以**居中 HUD** 呈现(黑底圆角、短暂淡入淡出),而不是靠弹窗或文字条。
    //
    // 时长来自平台解析;若平台给不出(返回 Zero),则 HasDuration=false,
    // 界面隐藏进度条、只显示已播放时间,而不是显示一个永远不动的 0%。
    // ================================================================

    /// <summary>已播放秒数</summary>
    [ObservableProperty]
    private double _positionSeconds;

    /// <summary>总时长秒数;为 0 表示平台未提供</summary>
    [ObservableProperty]
    private double _durationSeconds;

    /// <summary>是否已知总时长(未知时界面降级)</summary>
    [ObservableProperty]
    private bool _hasDuration;

    /// <summary>已播放时间文本</summary>
    [ObservableProperty]
    private string _positionText = "00:00";

    /// <summary>总时长文本</summary>
    [ObservableProperty]
    private string _durationText = "00:00";

    /// <summary>播放进度 0.0 - 1.0</summary>
    [ObservableProperty]
    private double _progress;

    /// <summary>音量 0.0 - 1.0</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(VolumeGlyph))]
    private double _volume = 1.0;

    /// <summary>是否静音</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(VolumeGlyph))]
    private bool _isMuted;

    /// <summary>音量文本</summary>
    [ObservableProperty]
    private string _volumeText = "100%";

    /// <summary>播放速率(1.0 为正常)</summary>
    [ObservableProperty]
    private double _playbackRate = 1.0;

    /// <summary>倍速文本</summary>
    [ObservableProperty]
    private string _playbackRateText = "1.0x";

    /// <summary>控制栏是否可见(鼠标静止或移出画面后隐藏,仅保留底部进度条)</summary>
    [ObservableProperty]
    private bool _isControlBarVisible = true;

    /// <summary>
    /// 控制栏的不透明度(0/1)。界面用它做渐隐过渡——
    /// 直接绑 IsVisible 只能硬切,而播放器控制栏应当渐隐渐现。
    /// 注意只作用于背景与按钮,**不含进度条**:隐藏后进度条仍贴在画面最下方。
    /// </summary>
    [ObservableProperty]
    private double _controlBarOpacity = 1.0;

    /// <summary>HUD 提示文本</summary>
    [ObservableProperty]
    private string _hudText = string.Empty;

    /// <summary>HUD 是否可见</summary>
    [ObservableProperty]
    private bool _isHudVisible;

    /// <summary>是否全屏(面板铺满窗口)</summary>
    [ObservableProperty]
    private bool _isFullscreen;

    /// <summary>静音前的音量,取消静音时恢复</summary>
    private double _volumeBeforeMute = 1.0;

    /// <summary>可选倍速档位(与 B 站播放器一致的常见取值,由快到慢)</summary>
    public double[] RateOptions { get; } = [2.0, 1.5, 1.25, 1.0, 0.75, 0.5];

    /// <summary>播放/暂停按钮的图标(随播放状态切换)</summary>
    public string PlayPauseGlyph => IsPlaying ? "❚❚" : "▶";

    /// <summary>音量按钮的图标(随音量档位变化)</summary>
    public string VolumeGlyph
        => IsMuted || Volume <= 0.001 ? "🔇"
         : Volume < 0.5 ? "🔉"
         : "🔊";

    // 当前正在播放的封面项(便于后续扩展引用)
    private CoverItem? _currentItem;

    // 调试用:DimOpacity 变化时打印日志
    partial void OnDimOpacityChanged(double value)
    {
        System.Diagnostics.Trace.WriteLine($"[VideoPlayer] DimOpacity = {value}");
    }

    // 默认构造
    public VideoPlayerViewModel() : this(new ApiManager()) { }

    // 依赖注入构造
    public VideoPlayerViewModel(IApiManager apiManager)
    {
        _apiManager = apiManager;
    }

    /// <summary>
    /// 播放入口:根据传入的 CoverItem(bvid/cid)拉取播放地址并触发播放。
    /// 流程:
    ///   1. 复位状态机
    ///   2. 若 cid 缺失则先请求视频详情
    ///   3. 请求播放地址,挑选最佳音视频流
    ///   4. 赋值给 VideoUrl / AudioUrl,由 View 监听属性变化触发原生播放器
    /// </summary>
    public async Task PlayVideoAsync(CoverItem item)
    {
        System.Diagnostics.Trace.WriteLine($"[VideoPlayer] PlayVideoAsync called: {item.Title}, Bvid={item.Bvid}, Cid={item.Cid}");
        _currentItem = item;
        IsVisible = true;
        IsPlaying = false;
        IsLoading = true;
        IsPausedOverlay = false;
        DimOpacity = 0;
        VideoUrl = null;
        AudioUrl = null;
        // 标题做 HTML 解码(去掉 &amp; 之类的实体)
        Title = System.Net.WebUtility.HtmlDecode(item.Title);
        PackageName = item.Bvid;
        StatusText = "Loading video...";

        try
        {
            var session = _apiManager.CurrentSession;
            if (session == null || _apiManager.Bilibili == null) return;

            // 1) 若 CoverItem 没有携带 cid,则先请求视频详情获取 cid
            long cid = item.Cid;
            System.Diagnostics.Trace.WriteLine($"[VideoPlayer] Fetching CID for {item.Bvid}, session ImgKey={session.ImgKey}, SubKey={session.SubKey}");
            if (cid == 0 && !string.IsNullOrEmpty(item.Bvid))
            {
                var info = await _apiManager.Bilibili.GetVideoInfoAsync(item.Bvid, session);
                System.Diagnostics.Trace.WriteLine($"[VideoPlayer] VideoInfo: code={info.Code}, cid={info.Data?.Cid}");
                if (info.Code == 0 && info.Data != null)
                {
                    // 优先使用顶级 cid;多 P 视频则取第一分 P
                    cid = info.Data.Cid != 0 ? info.Data.Cid
                        : (info.Data.Pages.Count > 0 ? info.Data.Pages[0].Cid : 0);
                    // 写回 CoverItem 以便下次直接使用
                    item.Cid = cid;
                }
            }

            if (cid == 0)
            {
                StatusText = "Failed to get video info";
                return;
            }

            // 2) 请求播放地址
            System.Diagnostics.Trace.WriteLine($"[VideoPlayer] Fetching play URL for {item.Bvid}/{cid}...");
            var playUrl = await _apiManager.Bilibili.GetPlayUrlAsync(item.Bvid, cid, session);
            System.Diagnostics.Trace.WriteLine($"[VideoPlayer] PlayUrl: code={playUrl.Code}, Data={playUrl.Data != null}, Dash={playUrl.Data?.Dash != null}, Video={playUrl.Data?.Dash?.Video?.Count}, Audio={playUrl.Data?.Dash?.Audio?.Count}");

            // 3) 挑选最佳视频流(AV1 > HEVC > AVC,再按带宽降序)
            if (playUrl.Data.Dash?.Video is { Count: > 0 })
            {
                var videos = playUrl.Data.Dash.Video;
                System.Diagnostics.Trace.WriteLine($"[VideoPlayer] {videos.Count} video streams available");
                var selected = PickBestCodec(videos);
                System.Diagnostics.Trace.WriteLine($"[VideoPlayer] Selected: {selected.Codecs} {selected.Width}x{selected.Height}, Url={!string.IsNullOrEmpty(selected.Url)}");

                // 4) 先设置 AudioUrl(注意顺序:VideoUrl 改变会触发播放,
                //    提前设置好 AudioUrl 可保证首次播放就有声音)
                if (playUrl.Data.Dash.Audio is { Count: > 0 })
                {
                    var bestAudio = playUrl.Data.Dash.Audio
                        .OrderByDescending(a => a.Bandwidth)
                        .First();
                    AudioUrl = bestAudio.Url;
                    System.Diagnostics.Trace.WriteLine($"[VideoPlayer] Audio: {bestAudio.Codecs}, Url={!string.IsNullOrEmpty(bestAudio.Url)}");
                }

                System.Diagnostics.Trace.WriteLine($"[VideoPlayer] Setting VideoUrl={selected.Url[..Math.Min(50, selected.Url.Length)]}...");
                VideoUrl = selected.Url;
                System.Diagnostics.Trace.WriteLine("[VideoPlayer] VideoUrl set done");

                IsLoading = false;
                IsPlaying = true;
                StatusText = "";
            }
            else
            {
                StatusText = "No DASH streams available";
            }
        }
        catch (Exception ex)
        {
            StatusText = $"Error: {ex.Message}";
            System.Diagnostics.Trace.WriteLine($"[VideoPlayer] Error: {ex.GetType().Name}: {ex.Message}\n{ex.StackTrace}");
        }
    }

    /// <summary>
    /// 挑选最佳视频流:
    ///   1) 编码优先级:AV1 > HEVC > AVC > 其它
    ///   2) 同编码内按带宽降序
    /// </summary>
    private static DashStream PickBestCodec(System.Collections.Generic.List<DashStream> videos)
    {
        int CodecRank(string codecs)
        {
            if (codecs.Contains("av01", StringComparison.OrdinalIgnoreCase)) return 0;
            if (codecs.Contains("hev", StringComparison.OrdinalIgnoreCase) ||
                codecs.Contains("hvc1", StringComparison.OrdinalIgnoreCase)) return 1;
            if (codecs.Contains("avc", StringComparison.OrdinalIgnoreCase)) return 2;
            return 3;
        }

        return videos
            .Where(v => v.Url.Length > 0)
            .OrderBy(v => CodecRank(v.Codecs))
            .ThenByDescending(v => v.Bandwidth)
            .First();
    }

    /// <summary>
    /// 切换播放/暂停。只翻转 IsPlaying ——
    /// 暂停遮罩与暗色遮罩由 OnIsPlayingChanged 统一派生,避免多处赋值造成状态漂移。
    /// </summary>
    [RelayCommand]
    private void TogglePlayPause()
    {
        IsPlaying = !IsPlaying;
        ShowHud(IsPlaying ? "播放" : "暂停");
    }

    /// <summary>关闭播放器:复位状态(暂停遮罩/暗色遮罩由 OnIsPlayingChanged 派生)</summary>
    [RelayCommand]
    private void Close()
    {
        StopPositionTimer();
        IsVisible = false;
        IsPlaying = false;
        IsControlBarVisible = true;
        IsHudVisible = false;
        IsFullscreen = false;
        VideoUrl = null;
        AudioUrl = null;
        _currentItem = null;
    }

    // ================================================================
    // 播放进度
    // ================================================================

    /// <summary>
    /// 播放状态变化时同步派生状态与进度定时器。
    ///
    /// 这里把"暂停遮罩"和"暗色遮罩"都交给 IsPlaying 驱动,而不是让它们各自被赋值:
    /// 原先 TogglePlayPause 依据 IsPausedOverlay 判断,而界面图标依据 IsPlaying,
    /// 两个状态源初值不一致(IsPlaying=false 但 IsPausedOverlay=false),
    /// 会出现"实际在播放却显示播放图标""点一次之后图标仍不变化"这类错位。
    /// </summary>
    partial void OnIsPlayingChanged(bool value)
    {
        IsPausedOverlay = !value;
        DimOpacity = value ? 0 : 0.5;

        if (value) StartPositionTimer();
        else StopPositionTimer();
    }

    /// <summary>控制栏显隐时同步透明度,供界面做渐隐过渡</summary>
    partial void OnIsControlBarVisibleChanged(bool value)
    {
        ControlBarOpacity = value ? 1.0 : 0.0;
    }

    /// <summary>
    /// 启动进度刷新。200ms 一次——比按帧刷新省 CPU,
    /// 又足以让进度条与时间显示看起来是连续的。
    /// </summary>
    private void StartPositionTimer()
    {
        _positionTimer ??= CreatePositionTimer();
        _positionTimer.Start();
    }

    private void StopPositionTimer()
    {
        _positionTimer?.Stop();
        // 停止后再采样一次,保证暂停时显示的是准确位置
        RefreshPosition();
    }

    private DispatcherTimer CreatePositionTimer()
    {
        var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(200) };
        timer.Tick += (_, _) => RefreshPosition();
        return timer;
    }

    /// <summary>从平台播放器采样一次进度与时长</summary>
    private void RefreshPosition()
    {
        if (_mediaPlayer == null) return;

        var position = _mediaPlayer.Position;
        var duration = _mediaPlayer.Duration;

        PositionSeconds = position.TotalSeconds;
        PositionText = FormatTime(position);

        // 时长可能晚于播放就绪才可解析(例如下载尚未落盘),因此每次都尝试更新
        if (duration > TimeSpan.Zero)
        {
            DurationSeconds = duration.TotalSeconds;
            DurationText = FormatTime(duration);
            HasDuration = true;
        }

        Progress = DurationSeconds > 0
            ? Math.Clamp(PositionSeconds / DurationSeconds, 0, 1)
            : 0;
    }

    /// <summary>
    /// 格式化时间。不足一小时用 mm:ss,超过则用 h:mm:ss
    /// (与 B 站播放器一致,避免长视频显示成 90:00 这种难以速读的形式)。
    /// </summary>
    private static string FormatTime(TimeSpan t)
    {
        if (t < TimeSpan.Zero) t = TimeSpan.Zero;
        return t.TotalHours >= 1
            ? $"{(int)t.TotalHours}:{t.Minutes:D2}:{t.Seconds:D2}"
            : $"{t.Minutes:D2}:{t.Seconds:D2}";
    }

    // ================================================================
    // HUD 提示(参考 BewlyCat:居中、黑底圆角、短暂停留后淡出)
    // ================================================================

    /// <summary>显示一条 HUD 提示,1.1 秒后自动隐藏</summary>
    public async void ShowHud(string text)
    {
        HudText = text;
        IsHudVisible = true;

        // 令牌机制:连续操作时只让最后一次的延迟生效,避免提示闪烁
        var token = ++_hudToken;
        await Task.Delay(1100);
        if (token == _hudToken)
            IsHudVisible = false;
    }

    // ================================================================
    // 音量
    // ================================================================

    partial void OnVolumeChanged(double value)
    {
        var clamped = Math.Clamp(value, 0, 1);
        VolumeText = $"{(int)Math.Round(clamped * 100)}%";
        if (clamped > 0) _volumeBeforeMute = clamped;
        SavePreferences();
    }

    partial void OnIsMutedChanged(bool value)
    {
        if (value)
        {
            // 记住当前音量再静音
            if (Volume > 0) _volumeBeforeMute = Volume;
            SetVolumeSilently(0);
            ShowHud("静音");
        }
        else
        {
            SetVolumeSilently(_volumeBeforeMute > 0 ? _volumeBeforeMute : 1.0);
            ShowHud($"音量 {(int)Math.Round(Volume * 100)}%");
        }
    }

    /// <summary>静音/恢复</summary>
    [RelayCommand]
    private void ToggleMute() => IsMuted = !IsMuted;

    /// <summary>
    /// 调节音量并给出 HUD 反馈。delta 为增量(如 ±0.05)。
    /// 调大音量时自动解除静音。
    /// </summary>
    public void AdjustVolume(double delta)
    {
        var target = Math.Clamp(Volume + delta, 0, 1);
        if (!IsMuted && Volume == 0 && target > 0) { /* 从 0 拉起,走下面的赋值即可 */ }

        // 直接改 Volume 会触发 OnVolumeChanged 的持久化与文本更新
        SetVolumeSilently(target);
        if (IsMuted && target > 0) IsMuted = false;   // 解除静音(不再叠一次 HUD)
        ShowHud($"音量 {(int)Math.Round(Volume * 100)}%");
    }

    /// <summary>设置音量但不触发 HUD/静音副作用(供内部与静音切换使用)</summary>
    private void SetVolumeSilently(double value)
    {
        var clamped = Math.Clamp(value, 0, 1);
        if (Math.Abs(Volume - clamped) < 0.0001) return;
        Volume = clamped;   // 触发 OnVolumeChanged
    }

    // ================================================================
    // 倍速
    // ================================================================

    partial void OnPlaybackRateChanged(double value)
    {
        PlaybackRateText = value.ToString("0.0##") + "x";
        SavePreferences();
    }

    /// <summary>循环切换到下一档倍速</summary>
    [RelayCommand]
    private void CycleRate()
    {
        var idx = Array.IndexOf(RateOptions, PlaybackRate);
        // 未命中(比如从记忆里读到非档位值)时回到正常速度
        var next = idx < 0 ? 1.0 : RateOptions[(idx + 1) % RateOptions.Length];
        SetRate(next);
    }

    /// <summary>设置倍速(带 HUD 反馈),由界面上的倍速菜单调用</summary>
    public void SetRate(double rate)
    {
        PlaybackRate = rate;
        ShowHud(PlaybackRateText);
    }

    // ================================================================
    // 全屏
    // ================================================================

    [RelayCommand]
    private void ToggleFullscreen()
    {
        IsFullscreen = !IsFullscreen;
        ShowHud(IsFullscreen ? "进入全屏" : "退出全屏");
    }

    // ================================================================
    // 偏好持久化(音量 / 倍速)
    // 参考 BewlyCat 的"记忆"设计:把用户习惯留在本地,下次打开即生效。
    // ================================================================

    private static readonly string PrefsPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "YKPili", "player-prefs.json");

    private bool _loadingPreferences;

    private sealed class PlayerPreferences
    {
        public double Volume { get; set; } = 1.0;
        public double Rate { get; set; } = 1.0;
    }

    private void LoadPreferences()
    {
        try
        {
            if (!File.Exists(PrefsPath)) return;
            var prefs = JsonSerializer.Deserialize<PlayerPreferences>(File.ReadAllText(PrefsPath));
            if (prefs == null) return;

            _loadingPreferences = true;
            Volume = Math.Clamp(prefs.Volume, 0, 1);
            PlaybackRate = prefs.Rate <= 0 ? 1.0 : Math.Clamp(prefs.Rate, 0.25, 4.0);
            _loadingPreferences = false;
        }
        catch
        {
            _loadingPreferences = false;
            // 读取失败就用默认值,不影响播放
        }
    }

    /// <summary>
    /// 写回偏好。加载过程中不写,避免把刚读出来的值再覆盖回去。
    /// 采用同步写入:文件极小,且失败也不应影响播放。
    /// </summary>
    private void SavePreferences()
    {
        if (_loadingPreferences) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(PrefsPath)!);
            var prefs = new PlayerPreferences { Volume = Volume, Rate = PlaybackRate };
            File.WriteAllText(PrefsPath, JsonSerializer.Serialize(prefs));
        }
        catch
        {
            // 偏好保存失败无需打扰用户
        }
    }
}
