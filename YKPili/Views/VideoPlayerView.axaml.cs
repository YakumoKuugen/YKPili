// 引入基础类型与异步任务
using System;
using System.Threading.Tasks;
// 引入 Avalonia 基础类型
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
// 引入项目内平台抽象与 ViewModel
using YKPili.Platforms;
using YKPili.ViewModels;

namespace YKPili.Views;

/// <summary>
/// VideoPlayerView:视频播放器视图
/// 把 IPlatformMediaPlayer(目前为 macOS 的 AVSampleBufferDisplayLayer)
/// 通过 Avalonia 的 NativeControlHost 嵌入到 UI 树中,
/// 并响应 VM 的属性变化驱动播放/暂停/显隐/动画。
/// </summary>
public partial class VideoPlayerView : UserControl
{
    // 由 Desktop 启动时注册的播放器工厂(不同平台可注入不同的实现)
    private IPlatformMediaPlayer? _player;
    // 绑定的 ViewModel
    private VideoPlayerViewModel? _vm;
    // 是否已经订阅过 Overlay/Panel/VideoArea 的事件(只挂一次)
    private bool _eventsAttached;
    // 滑入动画进行中标志
    private bool _animating;
    // 动画期间被推迟的播放请求(动画结束后再播放)
    private (string? videoUrl, string? audioUrl)? _pendingPlayback;

    /// <summary>
    /// 播放器工厂:由平台启动入口(如 YKPili.Desktop.Program)注册。
    /// 这里只声明,不构造,避免在跨平台代码中引用平台特定类型。
    /// </summary>
    public static Func<IPlatformMediaPlayer>? PlayerFactory { get; set; }

    public VideoPlayerView()
    {
        InitializeComponent();
    }

    /// <summary>
    /// 懒加载播放器与原生宿主。
    /// 只创建一次,之后所有播放复用同一个 NSView / AVPlayer 实例。
    /// </summary>
    private void EnsurePlayerAndHost()
    {
        if (_player != null) return;
        if (PlayerFactory == null) return;

        _player = PlayerFactory();
        _player.PlaybackError += err =>
        {
            if (_vm != null) _vm.StatusText = err;
        };

        // 首次访问 NativeControlHost 会触发原生视图创建(AVPlayer/NSView 等)
        var host = _player.NativeControlHost;
        if (host != null)
        {
            host.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Stretch;
            host.VerticalAlignment = Avalonia.Layout.VerticalAlignment.Stretch;
            // 插入到 Children[0],使其位于所有覆盖层(Loading/暂停/暗色)之下
            VideoArea.Children.Insert(0, host);
        }
    }

    // DataContext 变化时订阅 VM 属性变化与挂载事件
    protected override void OnDataContextChanged(EventArgs e)
    {
        base.OnDataContextChanged(e);

        if (DataContext is VideoPlayerViewModel vm)
        {
            _vm = vm;
            vm.PropertyChanged += OnVmPropertyChanged;

            // 首次创建播放器 + 原生宿主(之后保留在树中)
            EnsurePlayerAndHost();

            if (!_eventsAttached)
            {
                _eventsAttached = true;
                // 点击 Overlay 空白处关闭播放器
                Overlay.PointerPressed += (_, _) =>
                {
                    _vm?.CloseCommand.Execute(null);
                };
                // 点击 Panel 时不让事件冒泡到 Overlay
                Panel.PointerPressed += (_, e) => e.Handled = true;
                // 点击视频区切换播放/暂停
                VideoArea.PointerPressed += (_, e) =>
                {
                    _vm?.TogglePlayPauseCommand.Execute(null);
                    e.Handled = true;
                };
            }
        }
    }

    /// <summary>
    /// 响应 VM 属性变化:
    ///   - VideoUrl:有新地址时启动播放
    ///   - IsPlaying:转发给原生播放器暂停/恢复
    ///   - IsVisible:true 时把宿主放回面板并播放滑入动画
    ///   - IsVisible:false 时停止播放
    /// </summary>
    private void OnVmPropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (_vm == null || string.IsNullOrEmpty(e.PropertyName)) return;

        switch (e.PropertyName)
        {
            case nameof(VideoPlayerViewModel.VideoUrl) when _vm.VideoUrl != null:
                System.Diagnostics.Trace.WriteLine($"[VideoPlayerView] VideoUrl changed: {_vm.VideoUrl[..Math.Min(50, _vm.VideoUrl.Length)]}...");
                if (_animating)
                {
                    // 滑入动画进行中,推迟播放
                    System.Diagnostics.Trace.WriteLine("[VideoPlayerView] Animation in progress, deferring playback");
                    _pendingPlayback = (_vm.VideoUrl, _vm.AudioUrl);
                }
                else
                {
                    StartPlayback(_vm.VideoUrl, _vm.AudioUrl);
                }
                break;
            case nameof(VideoPlayerViewModel.IsPlaying) when _player != null:
                if (_vm.IsPlaying) _player.Resume();
                else _player.Pause();
                break;
            case nameof(VideoPlayerViewModel.IsVisible) when _vm.IsVisible:
                EnsureHostInPanel();
                SlideIn();
                break;
            case nameof(VideoPlayerViewModel.IsVisible) when !_vm.IsVisible:
                StopPlayback();
                break;
        }
    }

    /// <summary>
    /// 启动一次播放:
    ///   1) 先停止旧的播放并重置进度
    ///   2) 确保原生宿主在 VideoArea 中
    ///   3) 调用平台播放器 Play,带 User-Agent 与 Referer(B 站 CDN 必需)
    /// </summary>
    private void StartPlayback(string? videoUrl, string? audioUrl)
    {
        System.Diagnostics.Trace.WriteLine("[VideoPlayerView] StartPlayback begin");
        StopPlayback();
        _player?.ResetProgress();
        System.Diagnostics.Trace.WriteLine("[VideoPlayerView] StopPlayback done, calling EnsureHostInPanel");
        EnsureHostInPanel();

        if (videoUrl == null || _player == null)
        {
            System.Diagnostics.Trace.WriteLine($"[VideoPlayerView] StartPlayback abort: url={videoUrl != null} player={_player != null}");
            return;
        }

        // B 站 CDN 会对 UA 与 Referer 做强校验,缺一不可
        const string userAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        const string referer = "https://www.bilibili.com/";

        System.Diagnostics.Trace.WriteLine($"[VideoPlayerView] Calling Play: hasVideo=1 hasAudio={(audioUrl != null ? 1 : 0)} videoUrl={videoUrl[..Math.Min(60, videoUrl.Length)]}... audioUrl={audioUrl?[..Math.Min(60, audioUrl.Length)] ?? "null"}... bvid={_vm?.PackageName}");
        _player.Play(videoUrl, audioUrl, _vm?.Title, userAgent, referer, _vm?.PackageName);
        System.Diagnostics.Trace.WriteLine("[VideoPlayerView] Play returned");
    }

    /// <summary>停止播放(NativeControlHost 保留以便复用)</summary>
    private void StopPlayback()
    {
        _player?.Stop();
        // NativeControlHost stays in the tree — reused across videos
    }

    /// <summary>
    /// 确保 NativeControlHost 存在于 VideoArea 中,并主动重新布局。
    /// 同时输出各级坐标用于调试(面板/视频区/原生宿主的相对与绝对位置)。
    /// </summary>
    private void EnsureHostInPanel()
    {
        if (_player?.NativeControlHost is not { } host)
        {
            System.Diagnostics.Trace.WriteLine("[VideoPlayerView] EnsureHostInPanel: no host");
            return;
        }

        bool wasMissing = !VideoArea.Children.Contains(host);
        if (wasMissing)
        {
            System.Diagnostics.Trace.WriteLine("[VideoPlayerView] Host missing from panel, re-adding");
            host.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Stretch;
            host.VerticalAlignment = Avalonia.Layout.VerticalAlignment.Stretch;
            VideoArea.Children.Insert(0, host);
        }

        VideoArea.UpdateLayout();

        // ---- 调试:打印 Panel 坐标(相对 Overlay) ----
        var panelBounds = Panel.Bounds;
        var panelTransform = Panel.RenderTransform as Avalonia.Media.TranslateTransform;
        var panelOffsetX = panelTransform?.X ?? 0;
        var panelOffsetY = panelTransform?.Y ?? 0;
        System.Diagnostics.Trace.WriteLine(
            $"[VideoPlayerView] 播放窗口(Panel) Bounds(相对Overlay): X={panelBounds.X:F0} Y={panelBounds.Y:F0} W={panelBounds.Width:F0} H={panelBounds.Height:F0} | RenderTransform偏移: X={panelOffsetX:F0} Y={panelOffsetY:F0}");

        // ---- 调试:打印 VideoArea 坐标(相对 Panel 内部 Grid) ----
        var size = VideoArea.Bounds.Size;
        var videoAreaBounds = VideoArea.Bounds;
        System.Diagnostics.Trace.WriteLine(
            $"[VideoPlayerView] VideoArea Bounds(相对Panel): X={videoAreaBounds.X:F0} Y={videoAreaBounds.Y:F0} W={size.Width:F0} H={size.Height:F0}");

        if (size.Width > 0 && size.Height > 0)
        {
            // 主动测量/排列,确保原生视图获得正确尺寸
            host.Measure(size);
            host.Arrange(new Avalonia.Rect(size));
            _player.TriggerLayout();

            // ---- 调试:打印 NativeControlHost 坐标(相对 VideoArea) ----
            var hostBounds = host.Bounds;
            System.Diagnostics.Trace.WriteLine(
                $"[VideoPlayerView] 嵌入视频层(NativeControlHost) Bounds(相对VideoArea): X={hostBounds.X:F0} Y={hostBounds.Y:F0} W={hostBounds.Width:F0} H={hostBounds.Height:F0}");

            // ---- 调试:坐标链验证(把相对坐标累加成绝对位置) ----
            var hostAbsX = panelBounds.X + panelOffsetX + videoAreaBounds.X + hostBounds.X;
            var hostAbsY = panelBounds.Y + panelOffsetY + videoAreaBounds.Y + hostBounds.Y;
            System.Diagnostics.Trace.WriteLine(
                $"[VideoPlayerView] 坐标链: Panel({panelBounds.X:F0}+{panelOffsetX:F0}) + VideoArea({videoAreaBounds.X:F0}) + Host({hostBounds.X:F0}) = 窗口绝对位置 X={hostAbsX:F0}");
            System.Diagnostics.Trace.WriteLine(
                $"[VideoPlayerView] 坐标链: Panel({panelBounds.Y:F0}+{panelOffsetY:F0}) + VideoArea({videoAreaBounds.Y:F0}) + Host({hostBounds.Y:F0}) = 窗口绝对位置 Y={hostAbsY:F0} 尺寸={hostBounds.Width:F0}x{hostBounds.Height:F0}");
            System.Diagnostics.Trace.WriteLine(
                $"[VideoPlayerView] 验证结论: VideoArea跟随Panel={videoAreaBounds.X == 0 && videoAreaBounds.Y > 0}, NativeControlHost跟随VideoArea={hostBounds.X == 0 && hostBounds.Y == 0}");
        }
        else
        {
            System.Diagnostics.Trace.WriteLine("[VideoPlayerView] VideoArea has zero size, skipping host layout");
        }
    }

    /// <summary>
    /// 滑入动画:Panel 从屏幕下方以 ease-out 缓动滑到屏幕中央。
    /// 注意:这里使用 Margin 动画而不是 RenderTransform,
    /// 是因为 RenderTransform 只会影响 Avalonia 渲染层,
    /// 而 NSView(Avalonia 用 NativeControlHost 承载)需要跟随布局位置更新。
    /// </summary>
    private void SlideIn()
    {
        var from = Panel.Bounds.Height;
        // 把 Panel 暂时"推"到屏幕下方之外
        Panel.Margin = new Avalonia.Thickness(0, from, 0, 0);
        _animating = true;
        System.Diagnostics.Trace.WriteLine($"[VideoPlayerView] SlideIn start, from={from:F0}");

        var duration = 350;
        var startTime = Environment.TickCount64;
        var timer = new Avalonia.Threading.DispatcherTimer(
            TimeSpan.FromMilliseconds(16),
            Avalonia.Threading.DispatcherPriority.Render,
            (s, _) =>
            {
                var elapsed = Environment.TickCount64 - startTime;
                var progress = Math.Min(1.0, (double)elapsed / duration);
                // ease-out 缓动:cubic
                var eased = 1.0 - Math.Pow(1.0 - progress, 3);
                Panel.Margin = new Avalonia.Thickness(0, from * (1 - eased), 0, 0);
                if (progress >= 1.0)
                {
                    // 动画结束:复位 Margin,清空标志
                    Panel.Margin = new Avalonia.Thickness(0);
                    ((Avalonia.Threading.DispatcherTimer)s!).Stop();
                    _animating = false;
                    System.Diagnostics.Trace.WriteLine("[VideoPlayerView] SlideIn complete");

                    // 播放被推迟的请求
                    if (_pendingPlayback is { } pending)
                    {
                        _pendingPlayback = null;
                        System.Diagnostics.Trace.WriteLine("[VideoPlayerView] Executing deferred playback");
                        StartPlayback(pending.videoUrl, pending.audioUrl);
                    }
                }
            });
        timer.Start();
    }
}
