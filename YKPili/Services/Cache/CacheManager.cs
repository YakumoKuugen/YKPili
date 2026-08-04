// 引入基础类型、IO、LINQ 与 JSON 序列化
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;

namespace YKPili.Services.Cache;

/// <summary>
/// CacheManager:本地磁盘缓存管理器实现
/// 关键设计:
///   1. 以"包(package)"为粒度,每个包一个独立子目录;
///
///   2. 通过 manifest.json 记录每个包的最后访问时间(用于 LRU 淘汰);
///   3. 全局容量上限 500MB,可在构造时自定义;
///   4. 全部 public 操作都通过 _lock 串行化,保证并发安全;
///   5. 单例(Instance),由全局复用。
/// </summary>
public class CacheManager : ICacheManager
{
    // 默认缓存容量:500MB
    private const long DefaultMaxCacheBytes = 500 * 1024 * 1024;
    // 元信息文件名:记录每个包的最后访问时间
    private const string ManifestFileName = "manifest.json";

    // 缓存根目录
    private readonly string _cacheRoot;
    // 容量上限(字节)
    private readonly long _maxCacheBytes;
    // 并发锁
    private readonly object _lock = new();

    // 内存中的包元信息(包名 -> 最后访问时间)
    private Dictionary<string, PackageMeta> _manifest;
    // 元信息文件路径
    private readonly string _manifestPath;

    /// <summary>缓存根目录</summary>
    public string CacheRoot => _cacheRoot;

    /// <summary>单例实例,全局共享</summary>
    public static CacheManager Instance { get; } = new();

    // 默认构造:使用 500MB 上限
    private CacheManager()
        : this(DefaultMaxCacheBytes) { }

    // 自定义上限的构造
    private CacheManager(long maxCacheBytes)
    {
        // 缓存根目录固定在 LocalAppData/YKPili/cache 下
        _cacheRoot = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "YKPili", "cache");
        _maxCacheBytes = maxCacheBytes;
        _manifestPath = Path.Combine(_cacheRoot, ManifestFileName);
        // 启动时加载元信息(若不存在则为空字典)
        _manifest = LoadManifest();
    }

    /// <summary>获取包缓存目录,不存在返回 null(不创建)</summary>
    public string? GetPackagePath(string packageName)
    {
        var dir = GetPackageDir(packageName);
        return Directory.Exists(dir) ? dir : null;
    }

    /// <summary>
    /// 准备包目录:清空旧内容、创建目录、刷新元信息中的最后访问时间。
    /// 调用方拿到路径后即可开始写入新的缓存文件。
    /// </summary>
    public string PreparePackage(string packageName)
    {
        lock (_lock)
        {
            var dir = GetPackageDir(packageName);

            // 清空旧缓存(若存在)
            if (Directory.Exists(dir))
            {
                try
                {
                    Directory.Delete(dir, recursive: true);
                    System.Diagnostics.Trace.WriteLine($"[Cache] Cleared old cache for package: {packageName}");
                }
                catch (Exception ex)
                {
                    System.Diagnostics.Trace.WriteLine($"[Cache] Failed to clear package {packageName}: {ex.Message}");
                }
            }

            // 重新创建空目录
            Directory.CreateDirectory(dir);

            // 刷新元信息
            _manifest[packageName] = new PackageMeta
            {
                LastAccessTime = DateTime.UtcNow
            };
            SaveManifest();

            System.Diagnostics.Trace.WriteLine($"[Cache] Prepared package dir: {dir}");
            return dir;
        }
    }

    /// <summary>移除指定包的缓存(目录与元信息一并删除)</summary>
    public void RemovePackage(string packageName)
    {
        lock (_lock)
        {
            var dir = GetPackageDir(packageName);
            if (Directory.Exists(dir))
            {
                try
                {
                    Directory.Delete(dir, recursive: true);
                    System.Diagnostics.Trace.WriteLine($"[Cache] Removed package: {packageName}");
                }
                catch (Exception ex)
                {
                    System.Diagnostics.Trace.WriteLine($"[Cache] Failed to remove package {packageName}: {ex.Message}");
                }
            }

            // 从元信息中也移除
            _manifest.Remove(packageName);
            SaveManifest();
        }
    }

    /// <summary>清空全部缓存</summary>
    public void ClearAll()
    {
        lock (_lock)
        {
            if (Directory.Exists(_cacheRoot))
            {
                try
                {
                    Directory.Delete(_cacheRoot, recursive: true);
                    System.Diagnostics.Trace.WriteLine("[Cache] All cache cleared");
                }
                catch (Exception ex)
                {
                    System.Diagnostics.Trace.WriteLine($"[Cache] Failed to clear all cache: {ex.Message}");
                }
            }

            _manifest.Clear();
            SaveManifest();
        }
    }

    /// <summary>
    /// 容量超限时按 LRU 策略淘汰最旧的包,直到总占用 ≤ 上限。
    /// 始终保留至少 1 个包(即当前正在使用的包)以避免误删。
    /// </summary>
    public void EnforceSizeLimit()
    {
        lock (_lock)
        {
            var totalSize = ComputeTotalSize();
            System.Diagnostics.Trace.WriteLine($"[Cache] EnforceSizeLimit: total={totalSize / 1024 / 1024}MB, max={_maxCacheBytes / 1024 / 1024}MB");

            // 未超限直接返回
            if (totalSize <= _maxCacheBytes)
                return;

            // 按"最后访问时间"升序排序(最久未用在前)
            var ordered = _manifest
                .OrderBy(kv => kv.Value.LastAccessTime)
                .Select(kv => kv.Key)
                .ToList();

            foreach (var pkg in ordered)
            {
                // 至少保留 1 个包,避免误删当前包
                if (_manifest.Count <= 1)
                    break;

                // 先从总量中减去被淘汰的体积,避免反复 IO 统计
                totalSize -= GetDirectorySize(GetPackageDir(pkg));
                RemovePackage(pkg);
                System.Diagnostics.Trace.WriteLine($"[Cache] Evicted package: {pkg}");

                if (totalSize <= _maxCacheBytes)
                    break;
            }
        }
    }

    /// <summary>计算所有已记录包的总字节数</summary>
    public long GetTotalSize()
    {
        lock (_lock)
        {
            return ComputeTotalSize();
        }
    }

    // 将包名转为安全的目录名(剔除文件系统非法字符)
    private string GetPackageDir(string packageName)
    {
        var safeName = SanitizePath(packageName);
        return Path.Combine(_cacheRoot, safeName);
    }

    // 遍历元信息中所有包累计求和(不包含元信息中未记录的目录)
    private long ComputeTotalSize()
    {
        if (!Directory.Exists(_cacheRoot))
            return 0;

        long total = 0;
        foreach (var pkg in _manifest.Keys)
        {
            total += GetDirectorySize(GetPackageDir(pkg));
        }
        return total;
    }

    // 递归统计目录占用字节数(单文件错误降级为 0,不抛出)
    private static long GetDirectorySize(string path)
    {
        if (!Directory.Exists(path))
            return 0;

        try
        {
            return Directory.GetFiles(path, "*", SearchOption.AllDirectories)
                .Sum(f =>
                {
                    try { return new FileInfo(f).Length; }
                    catch { return 0; }
                });
        }
        catch
        {
            return 0;
        }
    }

    // 把包名里的非法文件名字符全部替换为下划线
    private static string SanitizePath(string name)
    {
        foreach (var c in Path.GetInvalidFileNameChars())
            name = name.Replace(c, '_');
        return name;
    }

    // ================================================================
    // 元信息持久化
    // 启动时尝试从磁盘恢复;每次变更后写回磁盘
    // ================================================================

    // 启动时从磁盘加载元信息(解析失败则返回空字典)
    private Dictionary<string, PackageMeta> LoadManifest()
    {
        try
        {
            if (File.Exists(_manifestPath))
            {
                var json = File.ReadAllText(_manifestPath);
                return JsonSerializer.Deserialize<Dictionary<string, PackageMeta>>(json)
                       ?? new Dictionary<string, PackageMeta>();
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[Cache] Failed to load manifest: {ex.Message}");
        }
        return new Dictionary<string, PackageMeta>();
    }

    // 把当前元信息以 JSON 格式写回磁盘(写失败仅记录日志)
    private void SaveManifest()
    {
        try
        {
            Directory.CreateDirectory(_cacheRoot);
            var json = JsonSerializer.Serialize(_manifest);
            File.WriteAllText(_manifestPath, json);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[Cache] Failed to save manifest: {ex.Message}");
        }
    }

    // 单个包的元信息(目前只有最后访问时间,后续可扩展)
    private class PackageMeta
    {
        public DateTime LastAccessTime { get; set; }
    }
}
