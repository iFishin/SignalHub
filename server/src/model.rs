//! 线上形状：落盘、请求、响应都用这里的结构。
//!
//! 所有「后来才加」的字段一律 `#[serde(default)]`：服务器升级不该因为数据目录里
//! 有一份旧格式的文件就起不来。

use serde::{Deserialize, Serialize};

/// 一个放置点 —— 用户在哪篇文章 / 哪个页面贴了链接。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    /// 给人看的名字，例如「博客：SignalHub 介绍」。**可以不填** —— 见 `auto_label`。
    pub label: String,
    /// 备注：放在哪个文件 / 哪个位置。只出现在客户端来源列表里。
    /// 用户没填时，由第一条带 `context` 的留言回填。
    #[serde(default)]
    pub note: Option<String>,
    /// URL 里的令牌。**是凭证**：`sources.json` 权限 0600。
    pub token: String,
    pub created_ms: u64,
    /// 吊销后公开端点立即 404；已收的留言保留可拉。
    #[serde(default)]
    pub revoked: bool,
    /// 名字是不是服务器生成的占位（`link-xxxx`）。是的话，第一条带标题的留言
    /// 会把它换成真正的标题 —— 用户只点「新建链接」，并不知道智能体从哪篇
    /// 文章过来。回填一次就置回 false，免得名字随每条留言变。
    #[serde(default)]
    pub auto_label: bool,
}

/// 一条留言。append-only —— 收进来就不再改。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 全局单调递增，同时是客户端拉取的游标。
    pub seq: u64,
    pub id: String,
    pub source_id: String,
    pub received_ms: u64,
    /// 来源 IP。排障 / 追溯滥用用，不展示在客户端列表里。
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub title: Option<String>,
    pub text: String,
    /// 自报的智能体名字（claude / chatgpt / …）。不可信，只当提示。
    #[serde(default)]
    pub agent: Option<String>,
    /// 自报的「从哪读到的链接」。它是 `open` 按钮的目标，所以客户端要校验 scheme。
    #[serde(default)]
    pub context: Option<String>,
}

/// 公开收信端点的请求体。字段全部可选、进入后统一校验 —— 这样缺失字段的错误
/// 由我们自己给 JSON 提示，而不是 axum 的纯文本 422。
#[derive(Debug, Default, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
}

/// 新建来源的请求体。
#[derive(Debug, Deserialize)]
pub struct NewSource {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// 来源 + 统计。给客户端「来源管理」列表用。
#[derive(Debug, Clone, Serialize)]
pub struct SourceView {
    pub id: String,
    pub label: String,
    pub note: Option<String>,
    pub token: String,
    /// 相对路径（`/c/<token>`）。**完整 URL 由客户端拼自己配的域名** ——
    /// 服务器不知道自己被哪个域名访问（反代后面、可能还有多个域名）。
    pub link: String,
    pub created_ms: u64,
    pub revoked: bool,
    /// 名字仍是占位（还没被留言回填）。客户端据此显示「等第一条留言命名」。
    pub auto_label: bool,
    pub total: usize,
    /// 近 24 小时收到几条。和防滥用配额同口径（滚动窗口，不按自然日）。
    pub today: usize,
    pub last_received_ms: u64,
}

/// 拉取留言的响应。
#[derive(Debug, Serialize)]
pub struct MessagesPage {
    pub messages: Vec<Message>,
    pub next_after: u64,
}

/// 统一的错误体。`error` 是给程序看的稳定码，`message` 是给人看的。
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}
