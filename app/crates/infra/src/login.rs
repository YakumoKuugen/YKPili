//! 扫码登录的状态机。
//!
//! 对应旧 `YKPili/ViewModels/LoginViewModel.cs` 的轮询逻辑，但**去掉了 async 与网络**：
//! 这一层只回答「收到这个响应之后，状态变成什么、还要不要继续轮询」。
//! 定时器与 HTTP 由调用方负责（GPUI 侧是 `cx.spawn` + `background_executor().timer`）。
//!
//! B 站扫码登录的四个内层状态码（写在官方接口文档里，别猜）：
//!
//! | code | 含义 | 动作 |
//! |---|---|---|
//! | `86101` | 未扫码 | 继续轮询 |
//! | `86090` | 已扫码，等用户在 App 确认 | 继续轮询 |
//! | `86038` | 二维码已过期 | 停止，提示重新生成 |
//! | `0` | 登录成功 | 停止，写入会话 |

use crate::api::models::LoginPollResponse;
use crate::models::{LoginState, UserSession};

/// 轮询间隔（毫秒）。与旧实现一致。
pub const POLL_INTERVAL_MS: u64 = 2000;

/// 本轮轮询之后该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollAction {
    /// 继续轮询。
    Continue,
    /// 停止轮询（成功 / 过期 / 不可恢复错误）。
    Stop,
}

/// 登录流程状态机。
#[derive(Debug, Clone)]
pub struct LoginFlow {
    state: LoginState,
    status: String,
    qrcode_key: Option<String>,
}

impl Default for LoginFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginFlow {
    pub fn new() -> Self {
        Self { state: LoginState::Idle, status: String::new(), qrcode_key: None }
    }

    pub fn state(&self) -> LoginState {
        self.state
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn qrcode_key(&self) -> Option<&str> {
        self.qrcode_key.as_deref()
    }

    /// 二维码已生成。
    pub fn on_qrcode_loaded(&mut self, key: &str) {
        self.qrcode_key = Some(key.to_string());
        self.state = LoginState::QrCodeLoaded;
        self.status = "请使用 B 站 App 扫码".into();
    }

    /// 二维码生成失败。
    pub fn on_qrcode_failed(&mut self, message: &str) {
        self.state = LoginState::LoginFailed;
        self.status = format!("生成二维码失败：{message}");
    }

    /// 处理一次轮询响应。
    ///
    /// 返回是否继续轮询；成功时会通过 `session` 参数回传会话。
    pub fn on_poll(
        &mut self,
        resp: &LoginPollResponse,
    ) -> (PollAction, Option<UserSession>) {
        // 外层 code 非 0：接口本身出错（限流、风控、参数问题）
        if resp.code != 0 {
            self.state = LoginState::LoginFailed;
            self.status = format!("接口错误：{}", resp.message);
            return (PollAction::Stop, None);
        }

        let inner = resp.data.as_ref().map(|d| d.code).unwrap_or(-1);
        match inner {
            0 => {
                self.state = LoginState::LoginSuccess;
                self.status = "登录成功".into();
                // 真正的 Cookie 在 Set-Cookie 头里，由 HTTP 层解析后填入。
                let session = UserSession {
                    is_logged_in: true,
                    ..Default::default()
                };
                (PollAction::Stop, Some(session))
            }
            86_038 => {
                self.state = LoginState::QrCodeExpired;
                self.status = "二维码已过期，请重新生成".into();
                (PollAction::Stop, None)
            }
            86_090 => {
                self.state = LoginState::QrCodeScanned;
                self.status = "已扫码，请在 App 上确认".into();
                (PollAction::Continue, None)
            }
            86_101 => {
                // 未扫码：保持当前状态，不覆盖提示文案
                if self.status.is_empty() {
                    self.status = "等待扫码".into();
                }
                (PollAction::Continue, None)
            }
            other => {
                // 未知内层码：不当作致命错误，继续轮询，但把码记下来便于排查
                self.status = format!("未知状态码 {other}，继续等待");
                (PollAction::Continue, None)
            }
        }
    }

    /// 回到初始态（取消登录 / 重新生成前）。
    pub fn reset(&mut self) {
        self.state = LoginState::Idle;
        self.status.clear();
        self.qrcode_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::PollData;

    fn resp(outer: i64, inner: i64) -> LoginPollResponse {
        LoginPollResponse {
            code: outer,
            data: Some(PollData {
                code: inner,
                message: String::new(),
                url: String::new(),
            }),
            message: "outer".into(),
        }
    }

    #[test]
    fn waiting_for_scan_keeps_polling() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        assert_eq!(f.state(), LoginState::QrCodeLoaded);

        let (action, _) = f.on_poll(&resp(0, 86_101));
        assert_eq!(action, PollAction::Continue);
        assert_eq!(f.state(), LoginState::QrCodeLoaded, "未扫码不应改变状态");
    }

    #[test]
    fn scanned_updates_state_and_continues() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        let (action, _) = f.on_poll(&resp(0, 86_090));
        assert_eq!(action, PollAction::Continue);
        assert_eq!(f.state(), LoginState::QrCodeScanned);
        assert!(f.status().contains("确认"));
    }

    #[test]
    fn expired_stops_polling() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        let (action, _) = f.on_poll(&resp(0, 86_038));
        assert_eq!(action, PollAction::Stop);
        assert_eq!(f.state(), LoginState::QrCodeExpired);
    }

    #[test]
    fn success_stops_and_returns_session() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        let (action, session) = f.on_poll(&resp(0, 0));
        assert_eq!(action, PollAction::Stop);
        assert_eq!(f.state(), LoginState::LoginSuccess);
        let s = session.expect("成功时应有会话");
        assert!(s.is_logged_in);
    }

    #[test]
    fn outer_error_stops_and_reports() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        let (action, session) = f.on_poll(&resp(-352, 0));
        assert_eq!(action, PollAction::Stop);
        assert_eq!(f.state(), LoginState::LoginFailed);
        assert!(f.status().contains("-352") || f.status().contains("接口错误"));
        assert!(session.is_none());
    }

    #[test]
    fn unknown_inner_code_keeps_waiting() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        let (action, _) = f.on_poll(&resp(0, 99_999));
        assert_eq!(action, PollAction::Continue, "未知码不该终止登录");
        assert!(f.status().contains("99999"));
    }

    #[test]
    fn reset_clears_everything() {
        let mut f = LoginFlow::new();
        f.on_qrcode_loaded("key");
        let _ = f.on_poll(&resp(0, 86_038));
        f.reset();
        assert_eq!(f.state(), LoginState::Idle);
        assert!(f.status().is_empty());
        assert!(f.qrcode_key().is_none());
    }

    #[test]
    fn poll_interval_matches_legacy() {
        assert_eq!(POLL_INTERVAL_MS, 2000);
    }
}
