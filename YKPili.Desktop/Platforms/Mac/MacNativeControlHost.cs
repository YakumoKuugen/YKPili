// 引入基础类型、诊断、运行时互操作、线程
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;
// 引入 Avalonia 控件与平台句柄
using Avalonia.Controls;
using Avalonia.Platform;

namespace YKPili.Desktop.Platforms.Mac;

// ================================================================
// MacNativeControlHost
// 把一个 NSView(IntPtr)嵌入到 Avalonia 可视化树中的桥接控件。
// 关键:
//   1. 在 CreateNativeControlCore 中 retain NSView(增加引用计数);
//   2. 在 DestroyNativeControlCore 中 release NSView(减少引用计数);
//   3. TriggerLayout 用于主动通知 NSView 重新布局(尺寸变化时使用)。
// ================================================================
public class MacNativeControlHost : NativeControlHost
{
    // 来自外部的 NSView 句柄(由 MacMediaPlayer 创建)
    private readonly IntPtr _nsViewHandle;
    // 缓存的 ObjC selector
    private static readonly IntPtr _selRetain = sel_registerName_internal("retain");
    private static readonly IntPtr _selRelease = sel_registerName_internal("release");
    private static readonly IntPtr _selLayout = sel_registerName_internal("layout");
    private static readonly IntPtr _selSetNeedsDisplay = sel_registerName_internal("setNeedsDisplay:");
    // 用于观察引用计数(调试)
    private int _nativeRefCount;

    public MacNativeControlHost(IntPtr nsViewHandle)
    {
        _nsViewHandle = nsViewHandle;
    }

    /// <summary>Avalonia 框架要求返回一个原生句柄时调用:retain NSView 防止被释放</summary>
    protected override IPlatformHandle CreateNativeControlCore(IPlatformHandle parent)
    {
        objc_msgSend_void_internal(_nsViewHandle, _selRetain);
        Interlocked.Increment(ref _nativeRefCount);
        Trace.WriteLine($"[MacNativeHost] CreateNative, refCount={_nativeRefCount}");
        return new PlatformHandle(_nsViewHandle, "NSView");
    }

    /// <summary>Avalonia 不再需要该原生控件时调用:release NSView</summary>
    protected override void DestroyNativeControlCore(IPlatformHandle control)
    {
        Interlocked.Decrement(ref _nativeRefCount);
        objc_msgSend_void_internal(_nsViewHandle, _selRelease);
        Trace.WriteLine($"[MacNativeHost] DestroyNative, refCount={_nativeRefCount}");
    }

    /// <summary>主动触发原生视图重新布局</summary>
    public void TriggerLayout()
    {
        objc_msgSend_void_internal(_nsViewHandle, _selLayout);
        objc_msgSend_void1_internal(_nsViewHandle, _selSetNeedsDisplay, (IntPtr)1);
        Trace.WriteLine("[MacNativeHost] layout + setNeedsDisplay");
    }

    // ---- P/Invoke 声明 ----
    // Foundation 框架的 objc_msgSend 与 sel_registerName
    [DllImport("/System/Library/Frameworks/Foundation.framework/Foundation", EntryPoint = "objc_msgSend")]
    private static extern void objc_msgSend_void_internal(IntPtr receiver, IntPtr selector);
    [DllImport("/System/Library/Frameworks/Foundation.framework/Foundation", EntryPoint = "objc_msgSend")]
    private static extern void objc_msgSend_void1_internal(IntPtr receiver, IntPtr selector, IntPtr arg1);
    [DllImport("/System/Library/Frameworks/Foundation.framework/Foundation", EntryPoint = "sel_registerName")]
    private static extern IntPtr sel_registerName_internal(string name);
}
