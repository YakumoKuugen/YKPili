//! 应用外壳状态与会话持久化。
//!
//! 对应旧 `YKPili/ViewModels/MainViewModel.cs`：登录态决定进登录页还是首页，
//! 会话写进本地 `session.json`，下次启动免登录。

use std::path::PathBuf;

use crate::models::UserSession;

/// 当前显示哪一屏。登录成功从 `Login` 切到 `Home`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Home,
}

/// 应用外壳状态（不含 UI，纯粹是「当前在哪一屏、有没有会话」）。
#[derive(Debug, Clone)]
pub struct AppShell {
    screen: Screen,
    session: Option<UserSession>,
}

impl Default for AppShell {
    fn default() -> Self {
        Self::new()
    }
}

impl AppShell {
    /// 未登录时停在登录页。启动后若 `restore` 成功会自动切到首页。
    pub fn new() -> Self {
        Self { screen: Screen::Login, session: None }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn session(&self) -> Option<&UserSession> {
        self.session.as_ref()
    }

    pub fn is_logged_in(&self) -> bool {
        self.session.as_ref().map(|s| s.is_valid()).unwrap_or(false)
    }

    /// Cookie 头（未登录为空串）。所有带登录态的请求都用它。
    pub fn cookie(&self) -> String {
        self.session
            .as_ref()
            .map(|s| s.cookie_header())
            .unwrap_or_default()
    }

    /// 扫码登录成功。
    pub fn on_login_success(&mut self, session: UserSession) {
        self.session = Some(session);
        self.screen = Screen::Home;
    }

    /// 从磁盘恢复会话。只有 `is_valid()` 的会话才会切到首页。
    pub fn restore(&mut self, session: UserSession) {
        if session.is_valid() {
            self.session = Some(session);
            self.screen = Screen::Home;
        }
    }

    /// 退出登录：回到登录页并清空会话。
    pub fn logout(&mut self) {
        self.session = None;
        self.screen = Screen::Login;
    }
}

/// `session.json` 的路径。
///
/// macOS：`~/Library/Application Support/YKPili/session.json`
/// 其它平台：系统临时目录下的 `YKPili/session.json`（够用，且不污染用户目录）
pub fn session_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library/Application Support/YKPili/session.json");
        }
    }
    std::env::temp_dir().join("YKPili").join("session.json")
}

/// 写入会话。失败不致命（下次大不了重新登录）。
pub fn save(path: &std::path::Path, session: &UserSession) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

/// 读取会话。文件缺失或损坏都返回 `None`。
pub fn load(path: &std::path::Path) -> Option<UserSession> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_session() -> UserSession {
        UserSession {
            dede_user_id: 42,
            sess_data: "sess".into(),
            bili_jct: "jct".into(),
            nickname: "测试用户".into(),
            is_logged_in: true,
            ..Default::default()
        }
    }

    #[test]
    fn starts_on_login_screen() {
        let app = AppShell::new();
        assert_eq!(app.screen(), Screen::Login);
        assert!(!app.is_logged_in());
        assert_eq!(app.cookie(), "");
    }

    #[test]
    fn login_success_switches_to_home() {
        let mut app = AppShell::new();
        app.on_login_success(valid_session());
        assert_eq!(app.screen(), Screen::Home);
        assert!(app.is_logged_in());
        assert_eq!(app.cookie(), "SESSDATA=sess; bili_jct=jct; DedeUserID=42");
    }

    #[test]
    fn invalid_session_does_not_enter_home() {
        let mut app = AppShell::new();
        app.restore(UserSession { sess_data: "x".into(), is_logged_in: false, ..Default::default() });
        assert_eq!(app.screen(), Screen::Login, "无效会话不应进入首页");
    }

    #[test]
    fn valid_session_restores_home() {
        let mut app = AppShell::new();
        app.restore(valid_session());
        assert_eq!(app.screen(), Screen::Home);
    }

    #[test]
    fn logout_returns_to_login() {
        let mut app = AppShell::new();
        app.on_login_success(valid_session());
        app.logout();
        assert_eq!(app.screen(), Screen::Login);
        assert!(app.session().is_none());
    }

    #[test]
    fn session_roundtrip() {
        let dir = std::env::temp_dir().join(format!("yk-session-{}", std::process::id()));
        let path = dir.join("session.json");
        let s = valid_session();
        save(&path, &s).unwrap();
        let back = load(&path).expect("应能读回");
        assert_eq!(back, s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 兼容旧 C# 版写下的 PascalCase 文件。
    #[test]
    fn reads_legacy_pascalcase_json() {
        let dir = std::env::temp_dir().join(format!("yk-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        std::fs::write(
            &path,
            r#"{"DedeUserId":7,"SessData":"abc","BiliJct":"def","Nickname":"老用户","FaceUrl":"","IsLoggedIn":true,"ImgKey":null,"SubKey":null}"#,
        )
        .unwrap();
        let s = load(&path).expect("旧格式应可读");
        assert_eq!(s.dede_user_id, 7);
        assert_eq!(s.sess_data, "abc");
        assert!(s.is_valid());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_returns_none() {
        let p = std::env::temp_dir().join("yk-not-exist-xyz.json");
        assert!(load(&p).is_none());
        let dir = std::env::temp_dir().join(format!("yk-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        std::fs::write(&path, b"{ broken").unwrap();
        assert!(load(&path).is_none(), "损坏的文件不应导致 panic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_path_is_under_app_dir() {
        let p = session_path();
        assert!(p.to_string_lossy().contains("YKPili"));
        assert!(p.ends_with("session.json"));
    }
}
