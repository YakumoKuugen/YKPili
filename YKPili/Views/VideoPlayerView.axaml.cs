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

    // 控制栏自动隐藏定时器
    private Avalonia.Threading.DispatcherTimer? _controlBarTimer;
    // 非全屏时面板的原始尺寸(进入全屏前记下,退出时还原)
    private double _normalPanelWidth;
    private double _normalPanelHeight;
    private bool _panelSizeCaptured;

    /// <summary>控制栏静止多久后自动隐藏</summary>
    private static readonly TimeSpan ControlBarIdleTimeout = TimeSpan.FromSeconds(2);

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
        // 把播放器交给 VM:VM 需要读进度与时长来驱动进度条与时间显示
        // (音量/倍速仍由本类下发,见 OnVmPropertyChanged)
        _vm?.AttachPlayer(_player);

        // 播放真正开始/结束时同步 UI 状态。
        // 这两个事件由后台线程触发(解码/入队任务里),必须切回 UI 线程再改属性。
        _player.PlaybackStarted += () => Avalonia.Threading.Dispatcher.UIThread.Post(() =>
        {
            if (_vm == null) return;
            _vm.IsPlaying = true;   // 派生暂停遮罩与进度定时器
        });
        _player.PlaybackStopped += () => Avalonia.Threading.Dispatcher.UIThread.Post(() =>
        {
            if (_vm != null) _vm.IsPlaying = false;
        });

        // 首次访问 NativeControlHost 会触发原生视图创建(AVPlayer/NSView 等)
        var host = _player.NativeControlHost;
        if (host != null)
        {
            host.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Stretch;
            host.VerticalAlignment = Avalonia.Layout.VerticalAlignment.Stretch;
            // 放进 VideoSurface(视频渲染区),而不是整个 VideoArea——
            // 该区域已让出控制栏的高度,原生视图不会盖住控制栏
            VideoSurface.Children.Insert(0, host);
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

                // ---- 控制栏自动隐藏 ----
                // 鼠标在画面内移动即显示控制栏并重新计时;
                // 静止 2 秒、或指针移出播放界面,都隐藏背景与按钮(只留底部进度条)。
                // 指针停在控制层上时保持显示,避免"正要点击却消失了"。
                Panel.PointerMoved += (_, _) => ShowControlBarTemporarily();
                Panel.PointerExited += (_, _) => HideControlBarImmediately();
                ControlBar.PointerEntered += (_, _) => CancelControlBarTimeout();
                ControlBar.PointerExited += (_, _) => StartControlBarTimeout();

                // ---- 进度条宽度 ----
                // 填充宽度 = 轨道实际宽度 × 进度。轨道宽度只有布局完成后才知道,
                // 因此既要在进度变化时更新,也要在尺寸变化时重算(窗口/全屏切换)。
                ProgressTrack.SizeChanged += (_, _) => UpdateProgressWidth();
                UpdateProgressWidth();
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
            case nameof(VideoPlayerViewModel.Volume) when _player != null:
                _player.Volume = _vm.Volume;
                break;
            case nameof(VideoPlayerViewModel.PlaybackRate) when _player != null:
                _player.Rate = _vm.PlaybackRate;
                break;
            case nameof(VideoPlayerViewModel.IsFullscreen):
                ApplyFullscreen(_vm.IsFullscreen);
                break;
            case nameof(VideoPlayerViewModel.Progress):
                UpdateProgressWidth();
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

        bool wasMissing = !VideoSurface.Children.Contains(host);
        if (wasMissing)
        {
            System.Diagnostics.Trace.WriteLine("[VideoPlayerView] Host missing from surface, re-adding");
            host.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Stretch;
            host.VerticalAlignment = Avalonia.Layout.VerticalAlignment.Stretch;
            VideoSurface.Children.Insert(0, host);
        }

        VideoArea.UpdateLayout();

        // ---- 调试:打印 Panel 坐标(相对 Overlay) ----
        var panelBounds = Panel.Bounds;
        var panelTransform = Panel.RenderTransform as Avalonia.Media.TranslateTransform;
        var panelOffsetX = panelTransform?.X ?? 0;
        var panelOffsetY = panelTransform?.Y ?? 0;
        System.Diagnostics.Trace.WriteLine(
            $"[VideoPlayerView] 播放窗口(Panel) Bounds(相对Overlay): X={panelBounds.X:F0} Y={panelBounds.Y:F0} W={panelBounds.Width:F0} H={panelBounds.Height:F0} | RenderTransform偏移: X={panelOffsetX:F0} Y={panelOffsetY:F0}");

        // ---- 调试:打印 VideoSurface 坐标(视频渲染区,已让出控制栏高度) ----
        var videoAreaBounds = VideoArea.Bounds;
        var surfaceBounds = VideoSurface.Bounds;
        var size = surfaceBounds.Size;
        System.Diagnostics.Trace.WriteLine(
            $"[VideoPlayerView] VideoArea Bounds(相对Panel): X={videoAreaBounds.X:F0} Y={videoAreaBounds.Y:F0} W={videoAreaBounds.Width:F0} H={videoAreaBounds.Height:F0}");
        System.Diagnostics.Trace.WriteLine(
            $"[VideoPlayerView] VideoSurface Bounds(视频渲染区): X={surfaceBounds.X:F0} Y={surfaceBounds.Y:F0} W={size.Width:F0} H={size.Height:F0} (VideoArea 底部 {videoAreaBounds.Height - surfaceBounds.Height:F0}px 留给控制栏)");

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

    /// <summary>
    /// 按当前进度刷新进度条填充宽度。
    /// 用"实际像素宽度"而不是比例列:细进度条(3px)放在 Grid 的 Auto 行里时,
    /// 比例列会因为可用宽度被压成 0 而整条不可见。
    /// </summary>
    private void UpdateProgressWidth()
    {
        if (_vm == null) return;
        var trackWidth = ProgressTrack.Bounds.Width;
        if (trackWidth <= 0) return;
        ProgressFill.Width = Math.Clamp(trackWidth * _vm.Progress, 0, trackWidth);
    }

    // ================================================================
    // 控制栏自动隐藏
    // ================================================================

    /// <summary>显示控制栏并重新计时(鼠标移动时调用)</summary>
    private void ShowControlBarTemporarily()
    {
        if (_vm == null) return;
        _vm.IsControlBarVisible = true;
        StartControlBarTimeout();
    }

    /// <summary>开始"静止即隐藏"倒计时</summary>
    private void StartControlBarTimeout()
    {
        _controlBarTimer ??= new Avalonia.Threading.DispatcherTimer
        {
            Interval = ControlBarIdleTimeout
        };
        _controlBarTimer.Stop();
        _controlBarTimer.Tick -= OnControlBarTimeout;
        _controlBarTimer.Tick += OnControlBarTimeout;
        _controlBarTimer.Start();
    }

    /// <summary>取消倒计时(指针停在控制栏上时保持显示)</summary>
    private void CancelControlBarTimeout()
    {
        _controlBarTimer?.Stop();
    }

    /// <summary>
    /// 立即隐藏控制栏(指针移出播放界面时调用)。
    /// 只隐藏背景与按钮,进度条由 XAML 独立于该状态,始终贴在画面最下方。
    /// </summary>
    private void HideControlBarImmediately()
    {
        _controlBarTimer?.Stop();
        if (_vm is { IsVisible: true })
            _vm.IsControlBarVisible = false;
    }

    private void OnControlBarTimeout(object? sender, EventArgs e)
    {
        _controlBarTimer?.Stop();
        // 播放器已关闭或没有在播放时不必隐藏(避免留下"空面板"观感)
        if (_vm is { IsVisible: true })
            _vm.IsControlBarVisible = false;
    }

    // ================================================================
    // 键盘快捷键
    //
    // 挂到 TopLevel 而不是自身:焦点可能在控制栏按钮上,
    // 只监听本控件会漏掉这些按键。播放器不可见时直接放行。
    // ================================================================

    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        if (Avalonia.Controls.TopLevel.GetTopLevel(this) is { } top)
            top.KeyDown += OnTopLevelKeyDown;
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnDetachedFromVisualTree(e);
        if (Avalonia.Controls.TopLevel.GetTopLevel(this) is { } top)
            top.KeyDown -= OnTopLevelKeyDown;
        _controlBarTimer?.Stop();
    }

    private void OnTopLevelKeyDown(object? sender, KeyEventArgs e)
    {
        if (_vm is not { IsVisible: true }) return;

        switch (e.Key)
        {
            case Key.Space:
                _vm.TogglePlayPauseCommand.Execute(null);
                e.Handled = true;
                break;

            // 音量:与 B 站一致,↑↓ 每次 5%
            case Key.Up:
                _vm.AdjustVolume(0.05);
                e.Handled = true;
                break;
            case Key.Down:
                _vm.AdjustVolume(-0.05);
                e.Handled = true;
                break;

            case Key.M:
                _vm.ToggleMuteCommand.Execute(null);
                e.Handled = true;
                break;

            case Key.F:
                _vm.ToggleFullscreenCommand.Execute(null);
                e.Handled = true;
                break;

            case Key.Escape:
                // 全屏时先退出全屏,再次按下才关闭播放器
                if (_vm.IsFullscreen) _vm.ToggleFullscreenCommand.Execute(null);
                else _vm.CloseCommand.Execute(null);
                e.Handled = true;
                break;
        }

        // 任何按键都顺带唤醒控制栏
        ShowControlBarTemporarily();
    }

    // ================================================================
    // 全屏
    // ================================================================

    /// <summary>
    /// 全屏:让面板铺满整个浮层;退出时还原为原尺寸。
    /// 首次进入前记下 XAML 里的原始尺寸,避免多次切换后尺寸漂移。
    /// </summary>
    private void ApplyFullscreen(bool fullscreen)
    {
        if (!_panelSizeCaptured)
        {
            _normalPanelWidth = Panel.Width;
            _normalPanelHeight = Panel.Height;
            _panelSizeCaptured = true;
        }

        if (fullscreen)
        {
            Panel.Width = double.NaN;
            Panel.Height = double.NaN;
            Panel.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Stretch;
            Panel.VerticalAlignment = Avalonia.Layout.VerticalAlignment.Stretch;
            Panel.Margin = new Avalonia.Thickness(0);
            Panel.CornerRadius = new Avalonia.CornerRadius(0);
        }
        else
        {
            Panel.Width = _normalPanelWidth;
            Panel.Height = _normalPanelHeight;
            Panel.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Center;
            Panel.VerticalAlignment = Avalonia.Layout.VerticalAlignment.Center;
            Panel.Margin = new Avalonia.Thickness(0);
        }

        // 原生视图尺寸随宿主变化,需要主动通知重新布局
        _player?.TriggerLayout();
    }
}
