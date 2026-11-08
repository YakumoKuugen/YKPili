//! 错误日志行为验收：只在报错时落盘、每次启动清空、panic 也记录。
//!
//! 单独一个测试二进制：`YK_LOG_DIR` 是进程级环境变量，避免和其他测试
//! 并发改环境变量导致偶发失败。

use std::fs;

fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ykpili-error-log-{}", std::process::id()))
}

#[test]
fn error_log_only_writes_on_error_and_is_cleared_on_startup() {
    let dir = temp_dir();
    let _ = fs::remove_dir_all(&dir);
    std::env::set_var("YK_LOG_DIR", &dir);

    // 启动：清空上一次，但没有报错时不应该留下空文件。
    yk_infra::error_log::init();
    let path = yk_infra::error_log::path();
    assert_eq!(path, dir.join("player.log"));
    assert!(!path.exists(), "没有报错时不应创建日志文件");

    // 报错：写入一行，带 scope 与时间戳前缀。
    yk_infra::error_log::error("player", "取流失败：测试错误");
    let text = fs::read_to_string(&path).expect("报错后应有日志文件");
    assert!(
        text.contains("[player] 取流失败：测试错误"),
        "日志内容不对：{text}"
    );
    assert_eq!(text.lines().count(), 1, "一次报错只应写一行：{text}");

    // 再次启动：旧日志被清空。
    yk_infra::error_log::init();
    assert!(!path.exists(), "启动时应删除上一次会话的日志");

    // panic 也要落盘（panic hook 会先写日志再走默认输出）。
    let _ = std::panic::catch_unwind(|| panic!("日志测试 panic"));
    let text = fs::read_to_string(&path).expect("panic 后应有日志文件");
    assert!(text.contains("[panic]"), "panic 未写入日志：{text}");

    let _ = fs::remove_dir_all(&dir);
}
