// 引入系统基础类型与运行时互操作
using System;
using System.Runtime.InteropServices;

namespace YKPili.Desktop.Platforms.Mac;

// ================================================================
// MacMediaPlayer.ObjC - ObjC 互操作层
//
// 包含:
//   1. P/Invoke 声明(objc_msgSend 的多种重载)
//   2. CMTime 结构体与相关 P/Invoke
//   3. 缓存的 ObjC Selector(避免重复 sel_registerName)
//   4. NSString / NSURL 辅助函数
//
// 本类通过 partial 与 MacMediaPlayer.cs 共同组成完整的播放器。
// ================================================================
public partial class MacMediaPlayer
{
    // ---- 缓存的 Selector(避免每次调用都重新注册) ----
    private static IntPtr _selRate;
    private static IntPtr _selSetRate;
    private static IntPtr _selEnqueue;
    private static IntPtr _selFlush;
    private static IntPtr _selReady;
    private static IntPtr _selStatus;
    private static IntPtr _selCopyNext;
    private static IntPtr _selAddOutput;
    private static IntPtr _selStartReading;
    private static IntPtr _selTracks;
    private static IntPtr _selCount;
    private static IntPtr _selObjectAtIndex;
    private static IntPtr _selMediaType;
    private static IntPtr _selIsEqualToString;
    private static IntPtr _selAssetReaderWithAsset;
    private static IntPtr _selTrackOutputWithTrack;
    private static IntPtr _selAddRenderer;
    private static IntPtr _selFileURLWithPath;
    private static IntPtr _selTimebase;
    private static IntPtr _selSetRateTime;
    private static bool _selectorsCached;

    // ---- 系统库路径 ----
    private const string FoundationLib = "/System/Library/Frameworks/Foundation.framework/Foundation";
    private const string CoreFoundationLib = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";

    // ================================================================
    // objc_msgSend P/Invoke 重载
    // 不同的参数/返回值组合需要单独的 extern 声明
    // ================================================================
    [DllImport(FoundationLib)]
    private static extern IntPtr objc_getClass(string className);
    [DllImport(FoundationLib)]
    private static extern IntPtr sel_registerName(string name);

    // 通用 objc_msgSend:接收者 + selector
    [DllImport(FoundationLib)]
    private static extern IntPtr objc_msgSend(IntPtr receiver, IntPtr selector);
    // 带一个 IntPtr 参数
    [DllImport(FoundationLib)]
    private static extern IntPtr objc_msgSend(IntPtr receiver, IntPtr selector, IntPtr arg1);
    // 无返回值
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern void objc_msgSend_void(IntPtr receiver, IntPtr selector);
    // 单个 string 参数
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern IntPtr objc_msgSend_str(IntPtr receiver, IntPtr selector, string arg1);
    // 两个 IntPtr 参数
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern IntPtr objc_msgSend_ptr(IntPtr receiver, IntPtr selector, IntPtr arg1, IntPtr arg2);
    // 返回 float
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern float objc_msgSend_float_ret(IntPtr receiver, IntPtr selector);
    // 返回 bool
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern bool objc_msgSend_bool_ret(IntPtr receiver, IntPtr selector);
    // 一个 IntPtr 参数,返回 bool
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern bool objc_msgSend_bool_ret_ptr(IntPtr receiver, IntPtr selector, IntPtr arg1);
    // 返回 nint(平台相关整型)
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern nint objc_msgSend_nint_ret(IntPtr receiver, IntPtr selector);
    // void + 一个 IntPtr 参数
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern void objc_msgSend_void_ptr(IntPtr receiver, IntPtr selector, IntPtr arg1);
    // void + float 参数
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern void objc_msgSend_void_float(IntPtr receiver, IntPtr selector, float arg1);
    // void + float + IntPtr 参数
    [DllImport(FoundationLib, EntryPoint = "objc_msgSend")]
    private static extern void objc_msgSend_void_float_ptr(IntPtr receiver, IntPtr selector, float arg1, IntPtr arg2);

    // CoreFoundation:释放 CF 对象
    [DllImport(CoreFoundationLib)]
    private static extern void CFRelease(IntPtr cf);

    // ================================================================
    // CoreMedia 类型与 P/Invoke
    // ================================================================
    private const string CoreMediaLib = "/System/Library/Frameworks/CoreMedia.framework/CoreMedia";

    // CMTime:C 兼容的时间结构(值/时间刻度/标志/纪元)
    [StructLayout(LayoutKind.Sequential)]
    private struct CMTime
    {
        public long Value;
        public int TimeScale;
        public uint Flags;
        public long Epoch;
    }

    // kCMTimeFlags_Valid = 1(表示该 CMTime 有效)
    // CMTimeZero:值为 0 的有效时间,用于"重置到开头"
    private static readonly CMTime CMTimeZero = new() { Value = 0, TimeScale = 1, Flags = 1, Epoch = 0 };

    [DllImport(CoreMediaLib)]
    private static extern int CMTimebaseSetTime(IntPtr timebase, CMTime time);

    [DllImport(CoreMediaLib)]
    private static extern int CMTimebaseSetRate(IntPtr timebase, double rate);

    // ================================================================
    // Selector 缓存:首次访问时一次性注册所有需要用到的 ObjC 方法名
    // ================================================================
    private static void CacheSelectors()
    {
        if (_selectorsCached) return;
        _selectorsCached = true;
        _selRate = sel_registerName("rate");
        _selSetRate = sel_registerName("setRate:");
        _selEnqueue = sel_registerName("enqueueSampleBuffer:");
        _selFlush = sel_registerName("flush");
        _selReady = sel_registerName("isReadyForMoreMediaData");
        _selStatus = sel_registerName("status");
        _selCopyNext = sel_registerName("copyNextSampleBuffer");
        _selAddOutput = sel_registerName("addOutput:");
        _selStartReading = sel_registerName("startReading");
        _selTracks = sel_registerName("tracks");
        _selCount = sel_registerName("count");
        _selObjectAtIndex = sel_registerName("objectAtIndex:");
        _selMediaType = sel_registerName("mediaType");
        _selIsEqualToString = sel_registerName("isEqualToString:");
        _selAssetReaderWithAsset = sel_registerName("assetReaderWithAsset:error:");
        _selTrackOutputWithTrack = sel_registerName("assetReaderTrackOutputWithTrack:outputSettings:");
        _selAddRenderer = sel_registerName("addRenderer:");
        _selFileURLWithPath = sel_registerName("fileURLWithPath:");
        _selTimebase = sel_registerName("timebase");
        _selSetRateTime = sel_registerName("setRate:time:");
    }

    // ================================================================
    // 辅助函数:创建 NSString / NSURL
    // ================================================================
    private static IntPtr ToNSString(string s) => objc_msgSend_str(objc_getClass("NSString"), sel_registerName("stringWithUTF8String:"), s);
    private static IntPtr ToNSURL(string s) => objc_msgSend_ptr(objc_getClass("NSURL"), sel_registerName("URLWithString:"), ToNSString(s), IntPtr.Zero);
    private static IntPtr ToNSFileURL(string path) => objc_msgSend_ptr(objc_getClass("NSURL"), _selFileURLWithPath, ToNSString(path), IntPtr.Zero);

    /// <summary>从 NSString 读取 UTF-8 C 字符串并返回托管 string</summary>
    private static string? GetNSStringValue(IntPtr nsString)
    {
        if (nsString == IntPtr.Zero) return "(null)";
        var utf8 = objc_msgSend(nsString, sel_registerName("UTF8String"));
        if (utf8 == IntPtr.Zero) return "(null)";
        return Marshal.PtrToStringUTF8(utf8) ?? "(empty)";
    }
}
