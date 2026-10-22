// 引入 IDisposable 与 Avalonia.Controls
using System;
using Avalonia.Controls;

namespace YKPili.Platforms;

/// <summary>
/// NullMediaPlayer:空实现(默认占位)
/// 在不支持原生嵌入的平台上作为默认实现,所有方法均为 no-op,避免空引用异常。
/// </summary>
public class NullMediaPlayer : IPlatformMediaPlayer
{
    /// <summary>始终未在播放</summary>
    public bool IsPlaying => false;

    /// <summary>空实现:音量读写不产生效果</summary>
    public double Volume { get; set; } = 1.0;

    /// <summary>空实现:速率读写不产生效果</summary>
    public double Rate { get; set; } = 1.0;

    /// <summary>空实现:无位置信息(界面据此降级)</summary>
    public TimeSpan Position => TimeSpan.Zero;

    /// <summary>空实现:无时长信息(界面据此降级)</summary>
    public TimeSpan Duration => TimeSpan.Zero;

    /// <summary>无原生宿主控件</summary>
    public NativeControlHost? NativeControlHost => null;

    /// <summary>空操作:触发布局</summary>
    public void TriggerLayout() { }

    /// <summary>事件:为空实现保留事件字段,便于订阅端在未初始化时安全挂空订阅</summary>
    public event Action? PlaybackStarted;
    public event Action? PlaybackStopped;
    public event Action<string>? PlaybackError;

    // 全部方法均为 no-op,占位实现
    public void Play(string url, string? audioUrl = null, string? title = null,
                     string? userAgent = null, string? referer = null, string? packageName = null) { }
    public void Pause() { }
    public void Resume() { }
    public void Stop() { }
    public void ResetProgress() { }
    public void Dispose() { }
}
