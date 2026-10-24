//! 数据源层。
//!
//! 目前只放接口响应模型（纯数据，可离线单测）。
//! HTTP 客户端与 WBI 签名在后续里程碑加入，届时与本模块并列。

pub mod bilibili;
pub mod client;
pub mod models;
pub mod wbi;
