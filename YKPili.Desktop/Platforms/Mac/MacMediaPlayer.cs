// 引入基础类型、诊断、IO、HTTP、异步、互操作
using System;
using System.Diagnostics;
using System.IO;
using System.Net.Http;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;
// 引入 Avalonia Controls
using Avalonia.Controls;
// 引入项目内平台抽象与缓存
using YKPili.Platforms;
using YKPili.Services.Cache;

namespace YKPili.Desktop.Platforms.Mac;

/// <summary>
/// MacMediaPlayer:macOS 平台播放器实现
/// 使用 AVFoundation 的 AVSampleBufferRenderSynchronizer + AVSampleBufferAudioRenderer
/// + AVSampleBufferDisplayLayer 进行音视频分离解码与同步,
/// 配合 AVAssetReader 读取本地已下载的文件,实现"渐进式缓冲播放"。
///
/// 注意:
///   1. NSView 的生命周期由 Avalonia 的 NativeControlHost 接管,本类持有 IntPtr 引用;
///   2. 在切换视频源时,必须重建 synchronizer + renderers 才能清除
///      AVSampleBufferDisplayLayer 内部的"最后显示 PTS"缓存(否则首帧会被丢弃);
///   3. 下载的视频/音频文件按 packageName(BVID) 分类存放在 CacheManager 中。
/// </summary>
public partial class MacMediaPlayer : IPlatformMediaPlayer
{
    // 用于取消正在进行的下载/入队任务
    private CancellationTokenSource? _downloadCts;
    // 嵌入到 Avalonia 树中的原生宿主
    private MacNativeControlHost? _host;
    // 当前是否正在播放
    private bool _isPlaying;
    // Dispose 守卫(避免重复释放)
    private bool _disposed;
    // 渲染器是否已初始化
    private bool _initialized;

    // ---- AVFoundation 核心对象(原生指针) ----
    // AVSampleBufferRenderSynchronizer:音视频同步器
    private IntPtr _synchronizer;
    // AVSampleBufferAudioRenderer:音频渲染器
    private IntPtr _audioRenderer;
    // AVSampleBufferDisplayLayer:视频显示层(继承自 CALayer)
    private IntPtr _videoDisplayLayer;
    // 承载视频层的 NSView(由本类 alloc/init,生命周期跟随本类 Dispose)
    private IntPtr _nsView;

    // 缓存包名(BVID)用于按包分类磁盘缓存
    private string? _packageName;

    // 下载缓存文件的绝对路径(每次播放更新)
    private string? _videoCachePath;
    private string? _audioCachePath;

    /// <summary>触发原生视图重新布局(尺寸变化时调用)</summary>
    public void TriggerLayout() => _host?.TriggerLayout();

    // ---- 播放参数与进度 ----
    // 音量与速率保存在实例上:即使渲染器尚未就绪(下载中)也要能记录用户设定,
    // 待初始化完成后统一应用。
    private double _volume = 1.0;
    private double _rate = 1.0;
    // 时长解析结果缓存:Duration 会被进度条按帧读取,不能每次都去读文件
    private string? _durationCachePath;
    private TimeSpan? _durationCache;

    /// <summary>音量(0.0 - 1.0)</summary>
    public double Volume
    {
        get => _volume;
        set
        {
            _volume = Math.Clamp(value, 0.0, 1.0);
            if (_initialized && _audioRenderer != IntPtr.Zero)
            {
                objc_msgSend_void_float(_audioRenderer, _selSetVolume, (float)_volume);
                Trace.WriteLine($"[MacPlayer] Volume applied: {_volume:F2}");
            }
        }
    }

    /// <summary>
    /// 播放速率(1.0 为正常)。暂停状态下只记录数值,Resume 时按该值恢复,
    /// 因此"暂停 → 改倍速 → 恢复"不会退回 1.0。
    /// </summary>
    public double Rate
    {
        get => _rate;
        set
        {
            _rate = Math.Clamp(value, 0.25, 4.0);
            if (_initialized && _isPlaying)
                objc_msgSend_void_float(_synchronizer, _selSetRate, (float)_rate);
            Trace.WriteLine($"[MacPlayer] Rate = {_rate:F2} (playing={_isPlaying})");
        }
    }

    /// <summary>
    /// 当前播放位置:读取同步器 timebase 的当前时间。
    /// timebase 未就绪或时间无效时返回 Zero,由界面据此降级。
    /// </summary>
    public TimeSpan Position
    {
        get
        {
            if (!_initialized || _synchronizer == IntPtr.Zero) return TimeSpan.Zero;
            try
            {
                var timebase = objc_msgSend(_synchronizer, _selTimebase);
                if (timebase == IntPtr.Zero) return TimeSpan.Zero;

                var cm = CMTimebaseGetTime(timebase);
                // TimeScale 为 0 表示该 CMTime 无效(尚未开始播放)
                if (cm.TimeScale == 0) return TimeSpan.Zero;

                var seconds = (double)cm.Value / cm.TimeScale;
                return seconds > 0 ? TimeSpan.FromSeconds(seconds) : TimeSpan.Zero;
            }
            catch
            {
                return TimeSpan.Zero;
            }
        }
    }

    /// <summary>
    /// 媒体总时长:解析本地缓存 MP4 的 mvhd box。
    /// 结果按文件路径缓存;解析失败返回 Zero(界面降级为只显示已播放时间)。
    /// </summary>
    public TimeSpan Duration
    {
        get
        {
            var path = _videoCachePath;
            if (_durationCachePath != path)
            {
                _durationCachePath = path;
                _durationCache = string.IsNullOrEmpty(path)
                    ? null
                    : YKPili.Services.Media.Mp4DurationReader.TryRead(path);
                Trace.WriteLine($"[MacPlayer] Duration resolved: {_durationCache?.TotalSeconds:F2}s ({path})");
            }
            return _durationCache ?? TimeSpan.Zero;
        }
    }

    /// <summary>当前是否在播放</summary>
    public bool IsPlaying => _isPlaying;

    /// <summary>原生宿主(首次访问时完成初始化)</summary>
    public NativeControlHost? NativeControlHost
    {
        get
        {
            if (!_initialized) EnsureInitialized();
            return _host;
        }
    }

    /// <summary>播放开始事件</summary>
    public event Action? PlaybackStarted;

    /// <summary>播放停止事件</summary>
    public event Action? PlaybackStopped;

    /// <summary>播放错误事件(参数为错误信息)</summary>
    public event Action<string>? PlaybackError;

    public MacMediaPlayer()
    {
        // 在构造时一次性缓存所有 ObjC selector,避免后续热路径中重复注册
        CacheSelectors();
    }

    // ================================================================
    // Initialize:AVSampleBufferRenderSynchronizer + renderers + NSView
    // ================================================================
    /// <summary>
    /// 首次访问 NativeControlHost 时调用,完成所有原生对象的创建与连接。
    /// NSView 由本类 alloc/init,生命周期跨越多次播放,只在 Dispose 中释放。
    /// </summary>
    private void EnsureInitialized()
    {
        if (_initialized) return;
        _initialized = true;

        Trace.WriteLine("[MacPlayer] Creating AVSampleBufferRenderSynchronizer...");

        // NSView 生命周期由本类管理(其引用计数随 Avalonia 的 NativeControlHost 增减)
        _nsView = objc_msgSend(objc_msgSend(objc_getClass("NSView"), sel_registerName("alloc")), sel_registerName("init"));
        // 启用 layer-backed 渲染
        objc_msgSend_ptr(_nsView, sel_registerName("setWantsLayer:"), (IntPtr)1, IntPtr.Zero);

        CreateRenderers();

        // 把视频显示层挂到 NSView 上作为 backing layer
        objc_msgSend_ptr(_nsView, sel_registerName("setLayer:"), _videoDisplayLayer, IntPtr.Zero);
        Trace.WriteLine("[MacPlayer] NSView created with VideoDisplayLayer");

        // 创建 Avalonia 原生宿主
        _host = new MacNativeControlHost(_nsView);
        Trace.WriteLine("[MacPlayer] AVSampleBufferRenderSynchronizer initialized");
    }

    /// <summary>
    /// 创建一组新的 synchronizer + 音频渲染器 + 视频显示层并连接。
    /// 由 EnsureInitialized(首次使用)与 ResetRenderersForNewPlayback(每次新播放)调用。
    /// </summary>
    private void CreateRenderers()
    {
        // ---- 同步器 ----
        var syncClass = objc_getClass("AVSampleBufferRenderSynchronizer");
        _synchronizer = objc_msgSend(objc_msgSend(syncClass, sel_registerName("alloc")), sel_registerName("init"));
        Trace.WriteLine($"[MacPlayer] Synchronizer created: 0x{_synchronizer:X}");

        // ---- 音频渲染器 ----
        var audioClass = objc_getClass("AVSampleBufferAudioRenderer");
        _audioRenderer = objc_msgSend(objc_msgSend(audioClass, sel_registerName("alloc")), sel_registerName("init"));
        Trace.WriteLine($"[MacPlayer] AudioRenderer created: 0x{_audioRenderer:X}");

        // 音频加入同步器
        objc_msgSend_void_ptr(_synchronizer, _selAddRenderer, _audioRenderer);
        Trace.WriteLine("[MacPlayer] AudioRenderer added to synchronizer");

        // ---- 视频显示层(CALayer 子类,可作为 AVQueuedSampleBufferRendering)----
        var layerClass = objc_getClass("AVSampleBufferDisplayLayer");
        _videoDisplayLayer = objc_msgSend(objc_msgSend(layerClass, sel_registerName("alloc")), sel_registerName("init"));
        Trace.WriteLine($"[MacPlayer] VideoDisplayLayer created: 0x{_videoDisplayLayer:X}");

        // 视频比例:等比缩放并保持比例
        objc_msgSend(_videoDisplayLayer, sel_registerName("setVideoGravity:"), ToNSString("AVLayerVideoGravityResizeAspect"));
        Trace.WriteLine("[MacPlayer] VideoDisplayLayer videoGravity set");

        // 视频层加入同步器
        objc_msgSend_void_ptr(_synchronizer, _selAddRenderer, _videoDisplayLayer);
        Trace.WriteLine("[MacPlayer] VideoDisplayLayer added to synchronizer");
    }

    /// <summary>
    /// 销毁当前 synchronizer 与 renderers。NSView 保留(它必须比 Avalonia 的 NativeControlHost 活得更久)。
    /// 调用方负责随后调用 CreateRenderers()(并把新显示层挂回 NSView),或在 Dispose 中彻底释放 NSView。
    /// 注意:AVSampleBufferRenderSynchronizer 没有 removeRenderer:方法,
    /// 想要重置渲染器对 timebase 的引用,只能销毁重建。
    /// </summary>
    private void ReleaseRenderers()
    {
        if (_synchronizer == IntPtr.Zero) return;

        // 先把 display layer 从 NSView 摘下,
        // 否则 NSView 仍持有它的强引用,后续 release 会泄漏。
        objc_msgSend_ptr(_nsView, sel_registerName("setLayer:"), IntPtr.Zero, IntPtr.Zero);

        // 释放顺序:synchronizer 持有两个 renderer,所以先 release synchronizer,
        // 再 release renderer 自身,符合引用计数语义。
        objc_msgSend_void(_synchronizer, sel_registerName("release"));
        objc_msgSend_void(_videoDisplayLayer, sel_registerName("release"));
        objc_msgSend_void(_audioRenderer, sel_registerName("release"));

        _synchronizer = IntPtr.Zero;
        _videoDisplayLayer = IntPtr.Zero;
        _audioRenderer = IntPtr.Zero;
    }

    /// <summary>
    /// 在两次播放之间重建 synchronizer 与 renderers,并把新显示层挂回 NSView。
    /// 这是修复"第二次播放只有声音、画面卡死"问题的关键:
    /// AVSampleBufferDisplayLayer 内部缓存了一个"最后显示的 PTS"指针,
    /// 该指针在 flush() 与 synchronizer 的 setRate:time: 重置后仍然存在,
    /// 导致新样本的 PTS=0 被误判为"已经播过"而丢弃。
    /// 唯一清除该缓存的方式就是重建显示层(避开私有 API)。
    /// </summary>
    private void ResetRenderersForNewPlayback()
    {
        Trace.WriteLine("[MacPlayer] Resetting renderers for new playback");
        ReleaseRenderers();
        CreateRenderers();
        objc_msgSend_ptr(_nsView, sel_registerName("setLayer:"), _videoDisplayLayer, IntPtr.Zero);
    }

    // ================================================================
    // Play
    // ================================================================
    /// <summary>
    /// 开始播放。
    /// 流程:Stop 旧播放 -> 初始化渲染器 -> 启动下载任务(后台) -> 启动渐进式播放。
    /// </summary>
    public void Play(string url, string? audioUrl, string? title, string? userAgent, string? referer, string? packageName = null)
    {
        Trace.WriteLine("[MacPlayer] Play() called");
        Stop();

        EnsureInitialized();

        _packageName = packageName;
        _downloadCts = new CancellationTokenSource();
        // UA 与 Referer 是 B 站 CDN 必填,缺一不可
        var ua = userAgent ?? "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";
        var refr = referer ?? "https://www.bilibili.com/";

        _ = PlayWithSynchronizerAsync(url, audioUrl, ua, refr, _downloadCts.Token);
    }

    /// <summary>
    /// 渐进式缓冲播放:
    ///   1. 启动下载(后台)并追踪进度
    ///   2. 缓冲达到 10% 尝试 AVAssetReader 启动播放
    ///   3. 下载继续,播放继续入队新样本
    ///   4. 播放消耗 50% 缓冲区时触发"下一段"信号
    /// </summary>
    private async Task PlayWithSynchronizerAsync(string videoUrl, string? audioUrl,
        string userAgent, string referer, CancellationToken ct)
    {
        try
        {
            // ---- 1. 准备缓存目录 ----
            string? cacheDir = null;
            if (!string.IsNullOrEmpty(_packageName))
            {
                var cacheManager = CacheManager.Instance;
                // PreparePackage 会清空旧文件,准备好本次播放用的目录
                cacheDir = cacheManager.PreparePackage(_packageName);
                Trace.WriteLine($"[MacPlayer] Using cache dir: {cacheDir}");
            }

            if (cacheDir == null)
            {
                // 没有包名(理论上不会发生)则退回到临时目录
                cacheDir = Path.GetTempPath();
                Trace.WriteLine("[MacPlayer] No package name, using temp dir");
            }

            // ---- 2. 共享进度追踪(在闭包中加锁更新) ----
            double videoProgress, audioProgress;
            videoProgress = audioProgress = 0;
            var progressLock = new object();
            bool hasAudio = audioUrl != null;

            double GetCombinedProgress()
            {
                lock (progressLock)
                    return hasAudio ? Math.Min(videoProgress, audioProgress) : videoProgress;
            }

            // ---- 3. 启动并行下载(后台) ----
            _videoCachePath = Path.Combine(cacheDir, "video.mp4");
            var videoDownloadTask = DownloadToFileAsync(videoUrl, _videoCachePath, userAgent, referer, ct,
                p => { lock (progressLock) videoProgress = p; });

            Task? audioDownloadTask = null;
            if (hasAudio)
            {
                _audioCachePath = Path.Combine(cacheDir, "audio.m4a");
                audioDownloadTask = DownloadToFileAsync(audioUrl!, _audioCachePath!, userAgent, referer, ct,
                    p => { lock (progressLock) audioProgress = p; });
            }

            // ---- 4. 等待缓冲达到 10% ----
            while (GetCombinedProgress() < 0.10 && !ct.IsCancellationRequested)
                await Task.Delay(200, ct);

            Trace.WriteLine($"[MacPlayer] Buffer 10% reached (video={videoProgress:P1}, audio={audioProgress:P1})");

            // ---- 5. AVAssetReader 重试循环(尝试在部分文件上启动) ----
            bool readerStarted = false;
            double lastAttemptProgress = 0.10;
            int videoEnqueuedCount = 0;
            int audioEnqueuedCount = 0;

            while (!readerStarted && !ct.IsCancellationRequested)
            {
                // 轻量验证:尝试在部分文件上启动 AVAssetReader 并读取一个样本
                var videoOk = TryReadSamples(_videoCachePath, "vide", _videoDisplayLayer, "视频", ct, out _);
                var audioOk = !hasAudio || TryReadSamples(_audioCachePath!, "soun", _audioRenderer, "音频", ct, out _);

                if (videoOk && audioOk)
                {
                    readerStarted = true;
                    var startPct = GetCombinedProgress();
                    Trace.WriteLine($"[MacPlayer] AVAssetReader started at {startPct:P1} buffer — enqueuing initial samples");

                    // 一次性入队当前部分文件的所有可用样本
                    videoEnqueuedCount = EnqueueSamples(_videoCachePath, "vide", _videoDisplayLayer, "视频", 0, ct);
                    if (hasAudio)
                        audioEnqueuedCount = EnqueueSamples(_audioCachePath!, "soun", _audioRenderer, "音频", 0, ct);

                    // 启动同步播放:使用当前设定的速率与音量(支持倍速与音量记忆)
                    Trace.WriteLine($"[MacPlayer] >>> Starting playback at {startPct:P1} buffer (video={videoEnqueuedCount} initial samples, audio={audioEnqueuedCount} initial samples, rate={_rate:F2}, volume={_volume:F2})");
                    if (_audioRenderer != IntPtr.Zero)
                        objc_msgSend_void_float(_audioRenderer, _selSetVolume, (float)_volume);
                    objc_msgSend_void_float(_synchronizer, _selSetRate, (float)_rate);
                    _isPlaying = true;
                    PlaybackStarted?.Invoke();
                }
                else
                {
                    // 等待 +5% 缓冲后重试
                    double target = lastAttemptProgress + 0.05;
                    while (GetCombinedProgress() < target && !ct.IsCancellationRequested)
                        await Task.Delay(300, ct);
                    lastAttemptProgress = GetCombinedProgress();
                    Trace.WriteLine($"[MacPlayer] Retrying AVAssetReader at {lastAttemptProgress:P1} buffer");
                }
            }

            if (!readerStarted) return;

            // ---- 6. 等待下载 100% 完成 ----
            Trace.WriteLine("[MacPlayer] Waiting for full download while playing...");
            await videoDownloadTask;
            if (audioDownloadTask != null) await audioDownloadTask;
            Trace.WriteLine("[MacPlayer] Download 100% complete");

            // ---- 7. 缓冲消耗过半触发"下一段下载"信号 ----
            {
                double consumedFraction = videoEnqueuedCount > 0
                    ? (double)videoEnqueuedCount / Math.Max(videoEnqueuedCount * 2, 100)
                    : 0;
                double consumedRatio = consumedFraction > 0.5 ? 0.5 : consumedFraction; // 保守估计
                Trace.WriteLine($"[MacPlayer] ▸ Buffer consumption estimate: ~{consumedRatio:P0} of initial cache consumed — next segment trigger (download already complete)");
            }

            // ---- 8. 最终入队:在完整文件上做最后一轮入队(跳过首批已入队的样本) ----
            Trace.WriteLine("[MacPlayer] Final enqueue on complete file...");
            videoEnqueuedCount = EnqueueSamples(_videoCachePath, "vide", _videoDisplayLayer, "视频",
                videoEnqueuedCount, ct);
            if (hasAudio)
                audioEnqueuedCount = EnqueueSamples(_audioCachePath!, "soun", _audioRenderer, "音频",
                    audioEnqueuedCount, ct);

            Trace.WriteLine($"[MacPlayer] Final enqueue done: video={videoEnqueuedCount} total, audio={audioEnqueuedCount} total");

            // ---- 9. 执行缓存大小限制(LRU 淘汰) ----
            if (!string.IsNullOrEmpty(_packageName))
                CacheManager.Instance.EnforceSizeLimit();

            Trace.WriteLine("[MacPlayer] Play() 完成");
        }
        catch (OperationCanceledException)
        {
            Trace.WriteLine("[MacPlayer] Play cancelled");
        }
        catch (Exception ex)
        {
            Trace.WriteLine($"[MacPlayer] Play error: {ex.GetType().Name}: {ex.Message}\n{ex.StackTrace}");
            PlaybackError?.Invoke(ex.Message);
        }
    }

    /// <summary>轻量尝试 AVAssetReader 是否可以在当前文件上启动(只读一个样本验证,不入队)</summary>
    private bool TryReadSamples(string filePath, string mediaType, IntPtr renderer,
        string label, CancellationToken ct, out int readCount)
    {
        readCount = 0;
        var fileUrl = ToNSFileURL(filePath);
        if (fileUrl == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} TryRead: NSURL creation failed");
            return false;
        }

        var asset = objc_msgSend_ptr(objc_getClass("AVURLAsset"),
            sel_registerName("URLAssetWithURL:options:"), fileUrl, IntPtr.Zero);
        if (asset == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} TryRead: AVURLAsset creation failed");
            return false;
        }

        var track = GetTrack(asset, mediaType);
        if (track == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} TryRead: track not found (mediaType={mediaType})");
            return false;
        }

        var reader = objc_msgSend_ptr(objc_getClass("AVAssetReader"), _selAssetReaderWithAsset, asset, IntPtr.Zero);
        if (reader == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} TryRead: AVAssetReader creation failed");
            return false;
        }

        var output = objc_msgSend_ptr(objc_getClass("AVAssetReaderTrackOutput"),
            _selTrackOutputWithTrack, track, IntPtr.Zero);
        objc_msgSend_void_ptr(reader, _selAddOutput, output);

        var started = objc_msgSend_bool_ret(reader, _selStartReading);
        if (!started)
        {
            var status = objc_msgSend_nint_ret(reader, _selStatus);
            Trace.WriteLine($"[MacPlayer] {label} TryRead: startReading failed, status={status}");
            return false;
        }

        // 尝试读取一个样本验证可用性
        var sample = objc_msgSend(output, _selCopyNext);
        if (sample != IntPtr.Zero)
        {
            CFRelease(sample);
            readCount = 1;
            Trace.WriteLine($"[MacPlayer] {label} TryRead: OK — at least 1 sample readable");
        }
        else
        {
            Trace.WriteLine($"[MacPlayer] {label} TryRead: reader started but no samples available yet");
        }

        return true;
    }

    /// <summary>
    /// 从文件中提取样本并入队(支持 skipCount 跳过已入队的样本)。
    /// 仅在完整文件上使用 skipCount;部分文件上 skipCount 应为 0。
    /// 返回总共入队的样本数。
    /// </summary>
    private int EnqueueSamples(string filePath, string mediaType, IntPtr renderer,
        string label, int skipCount, CancellationToken ct)
    {
        var fileUrl = ToNSFileURL(filePath);
        if (fileUrl == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} Enqueue: NSURL creation failed");
            return skipCount;
        }

        var asset = objc_msgSend_ptr(objc_getClass("AVURLAsset"),
            sel_registerName("URLAssetWithURL:options:"), fileUrl, IntPtr.Zero);
        if (asset == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} Enqueue: AVURLAsset creation failed");
            return skipCount;
        }

        var track = GetTrack(asset, mediaType);
        if (track == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} Enqueue: track not found (mediaType={mediaType})");
            return skipCount;
        }

        var reader = objc_msgSend_ptr(objc_getClass("AVAssetReader"), _selAssetReaderWithAsset, asset, IntPtr.Zero);
        if (reader == IntPtr.Zero)
        {
            Trace.WriteLine($"[MacPlayer] {label} Enqueue: AVAssetReader creation failed");
            return skipCount;
        }

        var output = objc_msgSend_ptr(objc_getClass("AVAssetReaderTrackOutput"),
            _selTrackOutputWithTrack, track, IntPtr.Zero);
        objc_msgSend_void_ptr(reader, _selAddOutput, output);

        var started = objc_msgSend_bool_ret(reader, _selStartReading);
        if (!started)
        {
            var errStatus = objc_msgSend_nint_ret(reader, _selStatus);
            Trace.WriteLine($"[MacPlayer] {label} Enqueue: startReading failed, status={errStatus}");
            return skipCount;
        }

        int enqueued = skipCount;
        int skipped = 0;
        int totalRead = 0;

        while (!ct.IsCancellationRequested)
        {
            var sample = objc_msgSend(output, _selCopyNext);
            if (sample == IntPtr.Zero) break;

            totalRead++;
            if (skipped < skipCount)
            {
                // 跳过已被入队的样本(只在完整文件最后一轮使用)
                CFRelease(sample);
                skipped++;
            }
            else
            {
                objc_msgSend_void_ptr(renderer, _selEnqueue, sample);
                CFRelease(sample);
                enqueued++;
            }
        }

        var status = objc_msgSend_nint_ret(reader, _selStatus);
        var newSamples = enqueued - skipCount;
        Trace.WriteLine($"[MacPlayer] {label} Enqueue: totalRead={totalRead}, skipped={skipped}, +{newSamples} new, total={enqueued}, reader status={status}");

        return enqueued;
    }

    // ================================================================
    // Download to cache file with progress reporting
    // ================================================================
    /// <summary>
    /// 异步把远程资源下载到本地文件,并通过 onProgress 回调报告 0~1 的进度。
    /// 使用 ResponseHeadersRead 模式以尽快开始写入磁盘,避免把所有内容缓存在内存中。
    /// </summary>
    private static async Task DownloadToFileAsync(string url, string filePath,
        string userAgent, string referer, CancellationToken ct,
        Action<double>? onProgress = null)
    {
        Trace.WriteLine($"[MacPlayer-DL] Downloading to {filePath}: {url[..Math.Min(60, url.Length)]}...");
        using var client = new HttpClient { Timeout = TimeSpan.FromSeconds(120) };
        client.DefaultRequestHeaders.Add("User-Agent", userAgent);
        client.DefaultRequestHeaders.Add("Referer", referer);

        using var response = await client.GetAsync(url, HttpCompletionOption.ResponseHeadersRead, ct);
        response.EnsureSuccessStatusCode();
        var totalLen = response.Content.Headers.ContentLength;
        Trace.WriteLine($"[MacPlayer-DL] Response: {response.StatusCode}, Content-Length: {totalLen}");

        using var stream = await response.Content.ReadAsStreamAsync(ct);
        using var fileStream = new FileStream(filePath, FileMode.Create, FileAccess.Write, FileShare.Read, 256 * 1024, useAsync: true);

        var buffer = new byte[256 * 1024];
        long totalRead = 0;
        int read;
        while ((read = await stream.ReadAsync(buffer, ct)) > 0)
        {
            await fileStream.WriteAsync(buffer.AsMemory(0, read), ct);
            totalRead += read;
            if (totalLen.HasValue && totalLen.Value > 0)
                onProgress?.Invoke((double)totalRead / totalLen.Value);
        }
        await fileStream.FlushAsync(ct);

        // 长度未知时(无 Content-Length),下载完成即视为 100%
        if (onProgress != null && (!totalLen.HasValue || totalLen.Value <= 0))
            onProgress(1.0);

        var finalSize = fileStream.Length;
        Trace.WriteLine($"[MacPlayer-DL] Download complete: {finalSize / 1024 / 1024}MB → {filePath}");
    }

    // ================================================================
    // Get video / audio track from AVAsset
    // ================================================================
    /// <summary>
    /// 从 AVAsset 中按 mediaType 查找对应的轨道。
    /// 关键:即使对本地文件,也应先调用 loadValuesAsynchronouslyForKeys 等待 "tracks" 加载,
    /// 否则直接访问 tracks 可能拿到 nil。
    /// </summary>
    private static IntPtr GetTrack(IntPtr asset, string mediaType)
    {
        var selLoadValues = sel_registerName("loadValuesAsynchronouslyForKeys:completionHandler:");
        var tracksKey = objc_msgSend(objc_getClass("NSArray"), sel_registerName("arrayWithObject:"), ToNSString("tracks"));
        objc_msgSend_ptr(asset, selLoadValues, tracksKey, IntPtr.Zero);

        var tracks = objc_msgSend(asset, _selTracks);
        Trace.WriteLine($"[MacPlayer] GetTrack: tracks=0x{tracks:X}, mediaType={mediaType}");
        if (tracks == IntPtr.Zero)
        {
            Trace.WriteLine("[MacPlayer] GetTrack: tracks is nil — asset may not have loaded metadata");
            return IntPtr.Zero;
        }

        var count = objc_msgSend_nint_ret(tracks, _selCount);
        Trace.WriteLine($"[MacPlayer] GetTrack: track count={count}");

        for (nint i = 0; i < count; i++)
        {
            var track = objc_msgSend_ptr(tracks, _selObjectAtIndex, (IntPtr)i, IntPtr.Zero);
            if (track == IntPtr.Zero)
            {
                Trace.WriteLine($"[MacPlayer] GetTrack: track[{i}] is nil");
                continue;
            }

            var type = objc_msgSend(track, _selMediaType);
            var typeStr = GetNSStringValue(type);
            Trace.WriteLine($"[MacPlayer] GetTrack: track[{i}] mediaType=\"{typeStr}\"");

            if (type == IntPtr.Zero) continue;

            // 字符串比较(用 isEqualToString:)找到目标类型
            var expected = ToNSString(mediaType);
            var match = objc_msgSend_bool_ret_ptr(type, _selIsEqualToString, expected);
            Trace.WriteLine($"[MacPlayer] GetTrack: track[{i}] match=\"{mediaType}\"={match}");
            if (match) return track;
        }

        Trace.WriteLine($"[MacPlayer] GetTrack: no track matched \"{mediaType}\"");
        return IntPtr.Zero;
    }

    // ================================================================
    // Playback control
    // ================================================================
    /// <summary>暂停:把同步器 rate 设为 0</summary>
    public void Pause()
    {
        Trace.WriteLine("[MacPlayer] Pause: rate=0.0");
        if (_initialized)
            objc_msgSend_void_float(_synchronizer, _selSetRate, 0.0f);
        _isPlaying = false;
    }

    /// <summary>恢复:按当前设定的速率恢复播放(支持倍速),同时补应用音量</summary>
    public void Resume()
    {
        Trace.WriteLine($"[MacPlayer] Resume: rate={_rate:F2}");
        if (_initialized)
        {
            objc_msgSend_void_float(_synchronizer, _selSetRate, (float)_rate);
            if (_audioRenderer != IntPtr.Zero)
                objc_msgSend_void_float(_audioRenderer, _selSetVolume, (float)_volume);
        }
        _isPlaying = true;
    }

    /// <summary>
    /// 停止播放:
    ///   1. 取消在飞的下载/入队
    ///   2. flush + pause 同步器
    ///   3. 重置 timebase
    ///   4. 重建渲染器(关键!见 ResetRenderersForNewPlayback)
    /// </summary>
    public void Stop()
    {
        Trace.WriteLine("[MacPlayer] Stop");

        // 取消任何在飞的下载/入队
        _downloadCts?.Cancel();
        _downloadCts?.Dispose();
        _downloadCts = null;

        // flush + pause 同步器(保留渲染器对象,避免与 Avalonia 的 NativeControlHost 生命周期竞态)
        if (_initialized)
        {
            objc_msgSend_void_float(_synchronizer, _selSetRate, 0.0f);
            objc_msgSend_void(_videoDisplayLayer, _selFlush);
            objc_msgSend_void(_audioRenderer, _selFlush);
        }

        ResetTimebase();

        // 重建 synchronizer + renderers,清除显示层缓存的"最后 PTS"
        if (_initialized) ResetRenderersForNewPlayback();

        CleanupTempFiles();

        _isPlaying = false;
        PlaybackStopped?.Invoke();
        Trace.WriteLine("[MacPlayer] Stop complete");
    }

    /// <summary>
    /// 重置播放进度到 0(切换视频源前调用)。
    /// 与 Stop 一样需要重建渲染器,原因相同。
    /// </summary>
    public void ResetProgress()
    {
        if (!_initialized) return;
        Trace.WriteLine("[MacPlayer] ResetProgress: flushing renderers + resetting timebase");

        objc_msgSend_void_float(_synchronizer, _selSetRate, 0.0f);
        objc_msgSend_void(_videoDisplayLayer, _selFlush);
        objc_msgSend_void(_audioRenderer, _selFlush);

        ResetTimebase();

        // 重建渲染器,原因与 Stop 中相同
        if (_initialized) ResetRenderersForNewPlayback();

        _isPlaying = false;
    }

    /// <summary>
    /// 把 timebase 重置到 0。
    /// 使用 setRate:time: 原子性地同时设置 rate=0 与 time=kCMTimeZero,
    /// 避免手动操作 CMTimebase 时因 CMTimebaseSetRate 异步生效导致的竞态条件。
    /// </summary>
    private void ResetTimebase()
    {
        if (!_initialized) return;

        var cmZero = CMTimeZero;
        var handle = GCHandle.Alloc(cmZero, GCHandleType.Pinned);
        try
        {
            objc_msgSend_void_float_ptr(_synchronizer, _selSetRateTime, 0.0f, handle.AddrOfPinnedObject());
        }
        finally
        {
            handle.Free();
        }
        Trace.WriteLine("[MacPlayer] Timebase reset to zero via setRate:time:");
    }

    /// <summary>
    /// 释放所有原生资源。
    /// 释放顺序很关键:先停播放,再 release synchronizer/renderers,最后 release NSView。
    /// </summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        Trace.WriteLine("[MacPlayer] Dispose");

        _downloadCts?.Cancel();
        _downloadCts?.Dispose();
        _downloadCts = null;

        if (_initialized)
        {
            // 先停播放
            objc_msgSend_void_float(_synchronizer, _selSetRate, 0.0f);

            // ReleaseRenderers 会先把 display layer 从 NSView 摘下,再按引用计数安全顺序释放
            ReleaseRenderers();

            // NSView 是本类完全持有的最后一个原生对象,最后 release 以便先观察到 layer detach
            objc_msgSend_void(_nsView, sel_registerName("release"));
            _nsView = IntPtr.Zero;
        }

        CleanupTempFiles();
    }

    /// <summary>
    /// 清理临时文件引用。
    /// 注意:缓存文件本身保留(下次播放可复用),只清空引用。
    /// </summary>
    private void CleanupTempFiles()
    {
        _videoCachePath = null;
        _audioCachePath = null;
    }
}
