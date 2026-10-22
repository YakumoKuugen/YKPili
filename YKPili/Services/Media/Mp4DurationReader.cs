// 引入基础类型与 IO
using System;
using System.IO;

namespace YKPili.Services.Media;

/// <summary>
/// Mp4DurationReader:从 MP4 文件的 mvhd box 读取媒体时长。
///
/// 为什么不用 AVFoundation / MediaFoundation:
///   平台播放器普遍把时长查询做成异步 API(需要等待 status 就绪),
///   在播放器界面里既要处理回调时序、又要处理"查询失败/尚未就绪"的降级路径。
///   而 mvhd 是 MP4 容器里一个固定格式的头部 box,
///   用纯托管代码直接读既同步又可靠,也便于跨平台复用与单元验证。
///
/// 兼容性:
///   适用于普通 MP4 与 fMP4(DASH 分片)。
///   实测 B 站 DASH 视频(ftyp 为 iso5/iso6/av01/dash)的 mvhd 带有正确的
///   timescale 与 duration,并非为 0。
///   遇到读不到 / timescale 为 0 / duration 为 0 的情况返回 null,
///   由调用方降级为"只显示已播放时间"。
/// </summary>
public static class Mp4DurationReader
{
    /// <summary>mvhd box 的类型标记</summary>
    private static readonly byte[] MvhdType = "mvhd"u8.ToArray();
    /// <summary>moov box 的类型标记(容器)</summary>
    private static readonly byte[] MoovType = "moov"u8.ToArray();
    /// <summary>mdhd box 的类型标记(媒体头,作为 mvhd 缺失时的备选)</summary>
    private static readonly byte[] MdhdType = "mdhd"u8.ToArray();

    /// <summary>
    /// 尝试读取时长。成功返回时长,失败返回 null(调用方据此降级)。
    /// </summary>
    public static TimeSpan? TryRead(string path)
    {
        try
        {
            if (string.IsNullOrEmpty(path) || !File.Exists(path))
                return null;

            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            return TryReadFromStream(fs);
        }
        catch
        {
            // 任何解析异常都视为"无法得知时长",交由界面降级处理
            return null;
        }
    }

    private static TimeSpan? TryReadFromStream(Stream stream)
    {
        // 先定位 moov 容器,再在其内部找 mvhd。
        // moov 可能在文件开头(未分片的 MP4)或结尾(部分录制文件),因此两种情况都要覆盖:
        // 先在文件头部扫描顶层 box;找不到再从尾部回扫。
        var moov = FindBox(stream, MoovType, 0, stream.Length);
        var mvhd = moov.HasValue
            ? FindBox(stream, MvhdType, moov.Value.PayloadStart, moov.Value.End)
            : FindBox(stream, MvhdType, 0, stream.Length);

        // 有些流没有 mvhd(纯音频片段等),退回读第一个 mdhd
        if (!mvhd.HasValue)
            mvhd = FindBox(stream, MdhdType, 0, stream.Length);

        if (!mvhd.HasValue)
            return null;

        return ReadTimeScaleAndDuration(stream, mvhd.Value.PayloadStart, mvhd.Value.End);
    }

    /// <summary>
    /// 在 [start, end) 范围内按顺序遍历 box,返回第一个类型匹配的 box 范围。
    /// MP4 的 box 布局:[4 字节大小][4 字节类型][负载...]
    ///   size == 1 时,紧随其后有 8 字节的 largesize
    ///   size == 0 表示该 box 延伸到文件末尾
    /// </summary>
    private static (long PayloadStart, long End)? FindBox(Stream stream, byte[] type, long start, long end)
    {
        long pos = start;
        var header = new byte[8];
        var typeBuf = new byte[4];

        while (pos + 8 <= end)
        {
            stream.Position = pos;
            if (stream.Read(header, 0, 8) != 8) return null;

            long size = ReadUInt32BE(header, 0);
            Array.Copy(header, 4, typeBuf, 0, 4);

            long payloadStart = pos + 8;
            long boxEnd;

            if (size == 1)
            {
                // 64 位长度
                var large = new byte[8];
                if (stream.Read(large, 0, 8) != 8) return null;
                size = (long)ReadUInt64BE(large, 0);
                payloadStart = pos + 16;
                boxEnd = size > 0 ? pos + size : end;
            }
            else if (size == 0)
            {
                boxEnd = end;
            }
            else
            {
                boxEnd = pos + size;
            }

            if (boxEnd <= pos) return null;   // 损坏的大小,避免死循环
            if (boxEnd > end) boxEnd = end;

            if (Matches(typeBuf, type))
                return (payloadStart, boxEnd);

            pos = boxEnd;
        }

        return null;
    }

    /// <summary>
    /// 读取 mvhd / mdhd 负载中的 timescale 与 duration。
    /// 两者的字段布局一致:
    ///   version(1) + flags(3) + [creation(4|8)] + [modification(4|8)] + timescale(4) + duration(4|8)
    /// </summary>
    private static TimeSpan? ReadTimeScaleAndDuration(Stream stream, long payloadStart, long end)
    {
        if (payloadStart + 4 > end) return null;

        stream.Position = payloadStart;
        var head = new byte[4];
        if (stream.Read(head, 0, 4) != 4) return null;

        int version = head[0];
        // version 0: creation/modification 各 4 字节;version 1: 各 8 字节
        int skip = version == 1 ? 16 : 8;

        long tsPos = payloadStart + 4 + skip;
        if (tsPos + 4 > end) return null;

        stream.Position = tsPos;
        var buf = new byte[12];
        int need = version == 1 ? 12 : 8;   // timescale(4) + duration(4 或 8)
        if (stream.Read(buf, 0, need) != need) return null;

        uint timescale = ReadUInt32BE(buf, 0);
        if (timescale == 0) return null;

        ulong duration = version == 1 ? ReadUInt64BE(buf, 4) : ReadUInt32BE(buf, 4);
        if (duration == 0) return null;

        // 部分流用 0xFFFFFFFF 表示"时长未知"
        if (version == 0 && duration == uint.MaxValue) return null;

        return TimeSpan.FromSeconds((double)duration / timescale);
    }

    private static bool Matches(byte[] a, byte[] b)
    {
        if (a.Length < b.Length) return false;
        for (int i = 0; i < b.Length; i++)
            if (a[i] != b[i]) return false;
        return true;
    }

    private static uint ReadUInt32BE(byte[] b, int offset)
        => ((uint)b[offset] << 24) | ((uint)b[offset + 1] << 16) | ((uint)b[offset + 2] << 8) | b[offset + 3];

    private static ulong ReadUInt64BE(byte[] b, int offset)
        => ((ulong)ReadUInt32BE(b, offset) << 32) | ReadUInt32BE(b, offset + 4);
}
