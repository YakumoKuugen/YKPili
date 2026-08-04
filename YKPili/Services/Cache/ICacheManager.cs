using System;

namespace YKPili.Services.Cache;

/// <summary>
/// ICacheManager:本地磁盘缓存管理器抽象
/// 缓存以"包(package)"为粒度组织,每个包对应一个独立子目录,
/// 典型使用方式:在开始下载前调用 PreparePackage,写文件后调用 EnforceSizeLimit。
/// </summary>
public interface ICacheManager
{
    /// <summary>缓存根目录绝对路径</summary>
    string CacheRoot { get; }

    /// <summary>
    /// 获取指定包对应的缓存目录(只读,不创建)。
    /// 若该包尚未缓存则返回 null。
    /// </summary>
    string? GetPackagePath(string packageName);

    /// <summary>
    /// 准备某个包的缓存目录:清理旧文件、按需创建目录、刷新最近访问时间。
    /// 返回该包对应的目录路径。建议在下载新数据前调用。
    /// 写完文件后可调用 EnforceSizeLimit 触发容量淘汰。
    /// </summary>
    string PreparePackage(string packageName);

    /// <summary>完全移除某个包的缓存(包括目录与元信息)</summary>
    void RemovePackage(string packageName);

    /// <summary>清空全部缓存</summary>
    void ClearAll();

    /// <summary>
    /// 当总占用超过上限时,按"最近最少访问"顺序淘汰最旧的包,直到容量回到限制以内。
    /// 写完新缓存后建议调用一次。
    /// </summary>
    void EnforceSizeLimit();

    /// <summary>所有已缓存包的总字节数</summary>
    long GetTotalSize();
}
