//! macOS 深链适配。
//!
//! - 自定义 Scheme：`Info.plist` 的 `CFBundleURLTypes` 注册后，AppKit 会调用
//!   `application:openURLs:`。GPUI 已经实现了这个 selector，主程序只需调用
//!   `Application::on_open_urls` 把 URL 转进本组件的队列。
//! - Universal Link：AppKit 走的是 `application:continueUserActivity:restorationHandler:`，
//!   GPUI 0.8 尚未实现。这里在运行时给 `GPUIApplicationDelegate` 补一个方法，
//!   从 `NSUserActivity.webpageURL` 取 URL，再走同一条队列。
//!
//! **前提**：Universal Link 只对「应用开发者控制的域名」生效。B 站域名不属于
//! 本项目，所以不能为 `bilibili.com` 注册 Universal Link；见模块文档。

use std::ffi::{c_char, CStr};
use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel};

use crate::queue::{self, LinkSource};

static INSTALLED: OnceLock<()> = OnceLock::new();

/// 等价的 Objective-C 签名：
/// `- (BOOL)application:(NSApplication *)app continueUserActivity:(NSUserActivity *)activity restorationHandler:(void (^)(NSArray *))handler`
unsafe extern "C-unwind" fn continue_user_activity(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    activity: *mut AnyObject,
    _restoration_handler: *mut AnyObject,
) -> bool {
    if activity.is_null() {
        return false;
    }

    // NSUserActivity.webpageURL -> NSURL
    let url: *mut AnyObject = unsafe { msg_send![&*activity, webpageURL] };
    if url.is_null() {
        return false;
    }

    // NSURL.absoluteString -> NSString
    let absolute: *mut AnyObject = unsafe { msg_send![&*url, absoluteString] };
    if absolute.is_null() {
        return false;
    }

    // NSString.UTF8String -> const char *
    let bytes: *const c_char = unsafe { msg_send![&*absolute, UTF8String] };
    if bytes.is_null() {
        return false;
    }

    match unsafe { CStr::from_ptr(bytes) }.to_str() {
        Ok(link) => {
            eprintln!("[deeplink] Universal Link：{link}");
            queue::ingest(link.to_string(), LinkSource::MacUniversalLink);
            true
        }
        Err(_) => false,
    }
}

/// 给 GPUI 的 AppDelegate 注入 Universal Link 回调。幂等。
pub fn install() {
    // 开发时 `cargo run -- ykpili://...` 也会走命令行；正式 .app 由
    // `application:openURLs:` 投递，不冲突（队列会去重）。
    if let Some(link) =
        crate::first_link_in_args(std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()))
    {
        eprintln!("[deeplink] macOS 命令行深链：{link}");
        queue::ingest(link, LinkSource::MacOpenUrl);
    }

    if INSTALLED.get().is_some() {
        return;
    }

    // GPUI 用 #[ctor] 在进程启动时注册 GPUIApplicationDelegate；
    // 如果这里找不到，说明 GPUI 版本换了类名或尚未初始化，本次跳过。
    let Some(class) = AnyClass::get(c"GPUIApplicationDelegate") else {
        eprintln!(
            "[deeplink] 找不到 GPUIApplicationDelegate，Universal Link 回调未注入（自定义 Scheme 不受影响）"
        );
        return;
    };

    let selector = sel!(application:continueUserActivity:restorationHandler:);
    let types = c"B@:@@@?";
    // objc2 0.6 起 `Imp` 是不可空的函数指针（`runtime::Imp`），
    // 已弃用的 `ffi::IMP` 是 `Option<...>`，不能再直接传给 `class_addMethod`。
    let imp: objc2::runtime::Imp = unsafe {
        std::mem::transmute::<
            unsafe extern "C-unwind" fn(
                *mut AnyObject,
                Sel,
                *mut AnyObject,
                *mut AnyObject,
                *mut AnyObject,
            ) -> bool,
            objc2::runtime::Imp,
        >(continue_user_activity)
    };

    let added = unsafe {
        objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            selector,
            imp,
            types.as_ptr(),
        )
    };
    if added.as_bool() {
        eprintln!("[deeplink] 已注入 Universal Link 回调");
    } else {
        eprintln!("[deeplink] Universal Link 回调已存在，跳过");
    }
    let _ = INSTALLED.set(());
}
