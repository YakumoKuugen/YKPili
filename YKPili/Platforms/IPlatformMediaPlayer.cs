// 引入 IDisposable 接口,平台播放器需要在 Dispose 中释放原生资源
using System;
// 引入 Avalonia.Controls 以便暴露 NativeControlHost
using Avalonia.Controls;

namespace YKPili.Platforms;

/// <summary>
/// IPlatformMediaPlayer:平台原生媒体播放器抽象接口
/// 各平台(目前仅 macOS)实现自己的播放器并嵌入到 Avalonia 的 NativeControlHost 中。
/// 设计原则:把视频解码/渲染交给平台原生能力,UI 侧只关心播放控制与事件。
/// </summary>
public interface IPlatformMediaPlayer : IDisposable
{
    /// <summary>
    /// 开始播放一段视频,可选伴音 URL(用于 B 站 DASH 分离流场景)。
    /// 必传 videoUrl;audioUrl、userAgent、referer、packageName 视场景可选。
    /// </summary>
    void Play(string url, string? audioUrl = null, string? title = null,
              string? userAgent = null, string? referer = null, string? packageName = null);

    /// <summary>暂停播放</summary>
    void Pause();

    /// <summary>恢复播放</summary>
    void Resume();

    /// <summary>停止播放并重置内部状态</summary>
    void Stop();

    /// <summary>重置播放进度到 0(通常在切换视频源前调用)</summary>
    void ResetProgress();

    /// <summary>返回承载原生视频视图的 Avalonia NativeControlHost(可能为 null 表示尚未创建)</summary>
    NativeControlHost? NativeControlHost { get; }

    /// <summary>触发原生视图重新布局(在尺寸变化时调用)</summary>
    void TriggerLayout();

    /// <summary>当前是否处于播放状态</summary>
    bool IsPlaying { get; }

    /// <summary>播放开始事件</summary>
    event Action? PlaybackStarted;

    /// <summary>播放停止事件</summary>
    event Action? PlaybackStopped;

    /// <summary>播放错误事件,参数为错误信息</summary>
    event Action<string>? PlaybackError;
}
