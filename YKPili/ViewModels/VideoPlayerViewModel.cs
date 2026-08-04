// 引入基础类型、LINQ 与异步任务
using System;
using System.Linq;
using System.Threading.Tasks;
// 引入 CommunityToolkit.Mvvm 源生成器
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
// 引入项目内模型、服务、缓存与 B 站模型
using YKPili.Models;
using YKPili.Services;
using YKPili.Services.Bilibili.Models;
using YKPili.Services.Cache;

namespace YKPili.ViewModels;

/// <summary>
/// VideoPlayerViewModel:视频播放器页 ViewModel
/// 负责根据 CoverItem(bvid)获取视频信息与播放地址,选择最佳音视频轨道,
/// 并将 URL 暴露给 View 触发原生播放器播放。
/// </summary>
public partial class VideoPlayerViewModel : ViewModelBase
{
    private readonly IApiManager _apiManager;

    /// <summary>播放器是否可见(控制浮层整体显隐)</summary>
    [ObservableProperty]
    private bool _isVisible;

    /// <summary>是否正在播放</summary>
    [ObservableProperty]
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

    /// <summary>切换播放/暂停:同时控制 IsPlaying 与暗色遮罩透明度</summary>
    [RelayCommand]
    private void TogglePlayPause()
    {
        if (IsPausedOverlay)
        {
            // 恢复播放
            IsPausedOverlay = false;
            IsPlaying = true;
            DimOpacity = 0;
        }
        else
        {
            // 暂停
            IsPausedOverlay = true;
            IsPlaying = false;
            DimOpacity = 0.5;
        }
    }

    /// <summary>关闭播放器:复位全部状态</summary>
    [RelayCommand]
    private void Close()
    {
        IsVisible = false;
        IsPlaying = false;
        IsPausedOverlay = false;
        VideoUrl = null;
        AudioUrl = null;
        _currentItem = null;
    }
}
