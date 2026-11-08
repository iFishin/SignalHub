//! SignalHub Call Me —— 自部署收信服务器。
//!
//! 存在的理由：用户把一条链接贴进文章 / 页面，外部 AI 智能体读到后想给的建议
//! 从一个公开的、只写的入口送进来；用户在桌面客户端里读。服务器是**公网的、
//! 无状态的、只写 + 管理读**的小服务，客户端主动拉取（桌面端在 NAT 后，推不进来）。
//!
//! 模块：
//! - [`config`] 环境变量 → 运行参数
//! - [`store`] 数据与磁盘（`sources.json` + `messages.jsonl`）
//! - [`token`] 令牌与短 id
//! - [`model`] 线上形状

pub mod config;
pub mod model;
pub mod store;
pub mod token;

pub use config::Config;
pub use model::{ErrorBody, Incoming, Message, MessagesPage, NewSource, Source, SourceView};
pub use store::{AcceptError, Limits, Store};
