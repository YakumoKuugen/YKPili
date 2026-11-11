//! Windows 深链适配。
//!
//! 两件事：
//! 1. 把 `ykpili` 协议写进 **当前用户** 注册表（不需要管理员权限）；
//! 2. 用命名管道做单实例转发 —— 浏览器拉起第二个 `ykpili.exe` 时，
//!    新进程把 URL 写进管道就退出，旧进程负责弹到前台并播放。
//!
//! 不用 `WM_COPYDATA`：它要求先找到窗口句柄，而窗口可能还没建好；
//! 命名管道在登录会话内稳定可用，也不受窗口标题 / 类名变化影响。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED,
    ERROR_SUCCESS, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::model::{first_link_in_args, SCHEME};
use crate::queue::{self, DeepLinkHandle, LinkSource};

/// 协议名。和 `SCHEME` 保持一致。
const PROTOCOL_KEY: &str = "Software\\Classes\\ykpili";
/// 管道名前缀。实际名字会拼上用户名，避免同一台机器上的多用户串台。
const PIPE_PREFIX: &str = "\\\\.\\pipe\\ykpili-deeplink-v1";
const MUTEX_NAME: &str = "Local\\YKPili.DeepLink.Singleton.v1";

/// 当前用户的管道名。用户名里的非 ASCII / 特殊字符直接过滤。
fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let safe: String = user
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect();
    if safe.is_empty() {
        PIPE_PREFIX.to_string()
    } else {
        format!("{PIPE_PREFIX}-{safe}")
    }
}

/// 单实例互斥体句柄。必须活到进程结束，Drop 掉就等于允许第二个实例。
static MUTEX_HANDLE: OnceLock<usize> = OnceLock::new();
/// 记录「本进程启动时是否已经有实例在跑」。
static ALREADY_RUNNING: OnceLock<bool> = OnceLock::new();
/// 管道服务只启动一次。
static PIPE_STARTED: OnceLock<()> = OnceLock::new();

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 命令行里的深链。Windows 的参数不保证是 UTF-8，这里做一次有损转换。
fn cli_link() -> Option<String> {
    first_link_in_args(std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()))
}

/// 创建 / 打开命名互斥体，返回值表示「启动时已经有另一个实例」。
fn instance_already_running() -> bool {
    *ALREADY_RUNNING.get_or_init(|| unsafe {
        let name = wide(MUTEX_NAME);
        // CreateMutexW 只在「已存在」时设置 ERROR_ALREADY_EXISTS；
        // 先清零，避免误读上一个 API 留下的 last error。
        SetLastError(0);
        let handle = CreateMutexW(ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return false;
        }
        // 句柄故意泄漏：进程活着，互斥体就活着。
        let _ = MUTEX_HANDLE.set(handle as usize);
        GetLastError() == ERROR_ALREADY_EXISTS
    })
}

/// 启动早期调用：命令行里有深链、且已有实例在跑时，把链接转发过去。
pub fn forward_link_before_ui() -> bool {
    let Some(link) = cli_link() else {
        return false;
    };
    if !instance_already_running() {
        return false;
    }

    // 旧实例可能刚启动、管道还没建好：最多等 3 秒。
    for _ in 0..30 {
        if send_to_primary(&link) {
            eprintln!("[deeplink] 已转发到运行中的实例：{link}");
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[deeplink] 找不到运行中实例的管道，改为当前进程处理：{link}");
    false
}

/// 安装协议注册、消费命令行、启动管道服务。幂等。
pub fn install() {
    // 即使 forward 没走到，也要占住单实例标记。
    let _ = instance_already_running();

    if let Some(link) = cli_link() {
        eprintln!("[deeplink] 命令行深链：{link}");
        queue::ingest(link, LinkSource::WindowsCli);
    }

    if std::env::var_os("YKPILI_NO_PROTOCOL_REGISTER").is_none() {
        match register_protocol() {
            Ok(()) => eprintln!("[deeplink] 已注册 {SCHEME}://（HKCU\\{PROTOCOL_KEY}）"),
            Err(error) => eprintln!("[deeplink] 注册协议失败：{error}"),
        }
    }

    spawn_pipe_server(queue::global().clone());
}

/// 把 `ykpili` 指向当前可执行文件。写在 HKCU，卸载时由 NSIS 清理。
pub fn register_protocol() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("取当前可执行文件失败：{error}"))?;
    let exe = exe.display().to_string();
    let command = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");

    set_reg_string(PROTOCOL_KEY, None, "URL:YKPili Protocol")?;
    set_reg_string(PROTOCOL_KEY, Some("URL Protocol"), "")?;
    set_reg_string(&format!("{PROTOCOL_KEY}\\DefaultIcon"), None, &icon)?;
    set_reg_string(
        &format!("{PROTOCOL_KEY}\\shell\\open\\command"),
        None,
        &command,
    )?;
    Ok(())
}

/// 测试 / 开发用：清掉当前用户下的协议注册。
#[allow(dead_code)]
pub fn unregister_protocol() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    let subkey = wide(PROTOCOL_KEY);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, subkey.as_ptr()) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("RegDeleteTreeW 返回 {status}"))
    }
}

fn set_reg_string(key_path: &str, value_name: Option<&str>, value: &str) -> Result<(), String> {
    let key_path = wide(key_path);
    let mut key: HKEY = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            ptr::null(),
            0,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("RegCreateKeyExW 返回 {status}"));
    }

    let name = value_name.map(wide);
    let data = wide(value);
    let bytes = (data.len() * std::mem::size_of::<u16>()) as u32;
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            bytes,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("RegSetValueExW 返回 {status}"))
    }
}

fn spawn_pipe_server(handle: DeepLinkHandle) {
    if PIPE_STARTED.set(()).is_err() {
        return;
    }
    let result = std::thread::Builder::new()
        .name("ykpili-deeplink-pipe".to_string())
        .spawn(move || pipe_loop(handle));
    if let Err(error) = result {
        eprintln!("[deeplink] 启动管道线程失败：{error}");
    }
}

fn pipe_loop(handle: DeepLinkHandle) {
    loop {
        if let Some(text) = serve_once() {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                eprintln!("[deeplink] 管道收到：{line}");
                handle.push(line, LinkSource::WindowsPipe);
            }
        }
    }
}

/// 建一个管道实例、等一次连接、读到 EOF 后返回内容。
fn serve_once() -> Option<String> {
    let name = wide(&pipe_name());
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        std::thread::sleep(Duration::from_millis(500));
        return None;
    }

    unsafe {
        SetLastError(0);
    }
    let connected = unsafe { ConnectNamedPipe(pipe, ptr::null_mut()) } != 0
        || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
    if !connected {
        unsafe {
            CloseHandle(pipe);
        }
        std::thread::sleep(Duration::from_millis(200));
        return None;
    }

    let mut chunk = [0u8; 4096];
    let mut data = Vec::new();
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                pipe,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..read as usize]);
        if data.len() > 64 * 1024 {
            break;
        }
    }

    unsafe {
        DisconnectNamedPipe(pipe);
        CloseHandle(pipe);
    }

    String::from_utf8(data).ok()
}

/// 连接旧实例的管道并写入一行 URL。
fn send_to_primary(link: &str) -> bool {
    let name = wide(&pipe_name());
    let pipe = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        return false;
    }

    let payload = format!("{link}\n");
    let bytes = payload.as_bytes();
    let mut written = 0u32;
    let ok = unsafe {
        WriteFile(
            pipe,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(pipe);
    }
    ok != 0 && written as usize == bytes.len()
}
