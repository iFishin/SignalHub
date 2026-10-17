//! 「一键诊断」：把排查需要的东西拼成一段可复制的文本。
//!
//! **为什么要有**：接入出问题时，需要的信息散在三处 —— 接入页的红色提示、系统页的
//! hook 日志、指南页的 curl —— 而最该被看到的那条往往不在同一个屏幕上。用户问到
//! 「怎么还是接不上」时，让他在三个页签之间来回翻，是把排查成本推给了他。这个模块
//! 把「我们知道的全部」一次拼出来：复制一段就能贴出去（或自己留着对比）。
//!
//! **只读、不写、不改任何东西** —— 它是给人看的快照，不是修复动作。所以它读得到
//! 什么就写什么，读不到就明说「读不到」，绝不猜。

use crate::config;
use crate::hooks::{self, Agent, InstallKind};
use crate::receipts::Receipts;
use crate::spool;
use std::fmt::Write as _;
use std::path::Path;

/// 拼出诊断文本。`filter` 给 `Some` 时只写那一个智能体。
pub fn text(receipts: &Receipts, filter: Option<Agent>) -> String {
    let mut out = String::new();
    // 头部：先给出「这份东西是什么时候、在哪个版本上取的」——
    // 没有这两条，贴出来的东西过几天就没法对照了。
    let _ = writeln!(out, "SignalHub 诊断 ({})", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "时间 {} UTC", config::utc_hms());
    let _ = writeln!(out, "平台 {}", std::env::consts::OS);

    // ---- 服务 ----
    let _ = writeln!(out, "\n== 上报服务 ==");
    match config::read_port() {
        Ok(port) => {
            let _ = writeln!(out, "端口 {port}（写在 ~/.signalhub/port）");
        }
        Err(err) => {
            let _ = writeln!(out, "端口 读不到：{err}");
        }
    }
    let _ = writeln!(
        out,
        "token {}",
        match config::read_token() {
            Ok(token) => format!("已生成（{} 位）", token.len()),
            Err(err) => format!("读不到：{err}"),
        }
    );

    // ---- 待补报 ----
    //
    // 这一条容易被忽略但很关键：**应用没跑的时候，钩子的事件会攒在盘上**，等应用起来
    // 再补。所以「灯不亮」有一种可能是「东西都在 spool 里，而应用刚起来/没起来」。
    let _ = writeln!(out, "\n== 待补报（应用没跑时攒下的）==");
    match spool::path() {
        Some(path) if path.exists() => {
            let (records, bad) = spool::read_all(&path);
            let _ = writeln!(out, "{} 条待补报，{bad} 条读不出", records.len());
        }
        Some(_) => {
            let _ = writeln!(out, "没有（正常）");
        }
        None => {
            let _ = writeln!(out, "读不到 spool 路径");
        }
    }

    // ---- 智能体 ----
    let receipts = receipts.view();
    for name in hooks::ALL_NAMES {
        let Ok(agent) = Agent::parse(name) else {
            continue;
        };
        if filter.is_some_and(|want| want != agent) {
            continue;
        }
        let _ = writeln!(out, "\n== {} ({name}) ==", agent.name());
        let _ = writeln!(
            out,
            "检测到 {} / 已接入 {}",
            hooks::detect(agent),
            hooks::is_installed(agent)
        );
        let kind = match hooks::install_kind(agent) {
            InstallKind::JsonHooks => "往配置 JSON 里插 hook 条目",
            InstallKind::PluginFile => "往插件目录放我们自己的文件",
        };
        let _ = writeln!(out, "接入方式 {kind}");
        // **哪一种写法**也要说：两种模式写进同一个配置，而它们的行为不同
        // （HTTP 式不怕 PATH 问题、但开了局域网访问时会关掉）
        match hooks::installed_http(agent) {
            Some(true) => {
                let _ = writeln!(out, "  写法 HTTP —— 不需要命令；开了局域网访问时这个端点会关掉");
            }
            Some(false) => {
                let _ = writeln!(out, "  写法 命令 —— 需要那条命令跑得起来（见下面那两行）");
            }
            None => {}
        }
        write_config(&mut out, agent);
        write_config_command(&mut out, agent);
        write_plugin_shape(&mut out, agent);
        match receipts.iter().find(|receipt| receipt.agent == name) {
            Some(receipt) => {
                let _ = writeln!(
                    out,
                    "最近上报 {} 前（状态 {}，本次运行 {} 条{}）",
                    seconds_text(receipt.last_epoch_ms),
                    receipt.state,
                    receipt.count,
                    // 类别只在**不是**智能体时报出来：那才是需要解释的情况
                    if receipt.source == crate::state::DEFAULT_SOURCE {
                        String::new()
                    } else {
                        format!("，类型 {}", source_text(&receipt.source))
                    }
                );
            }
            None => {
                let _ = writeln!(out, "最近上报 从没收到过（回执里没有它）");
            }
        }
    }

    // ---- 其它信号源 ----
    //
    // 不在上面清单里的上报（用户自己的脚本、另一台机器、手写的一条 curl，以及将来
    // 的系统采集器）。**单独一节而不是并进去**：上面那一节每一行都在说「这个智能体
    // 接得怎么样」，而这一类我们无从接入 —— 但它们是「上报通道是通的」的活证据，
    // 也是「我的脚本到底报没报」唯一的答案。
    let others: Vec<&crate::receipts::ReceiptView> = receipts
        .iter()
        .filter(|receipt| !hooks::ALL_NAMES.contains(&receipt.agent.as_str()))
        .collect();
    if !others.is_empty() {
        let _ = writeln!(out, "\n== 其它信号源（不在上面清单里的）==");
        for receipt in others {
            let _ = writeln!(
                out,
                "{} · {} · 最近上报 {} 前（状态 {}，本次运行 {} 条）",
                receipt.agent,
                source_text(&receipt.source),
                seconds_text(receipt.last_epoch_ms),
                receipt.state,
                receipt.count
            );
        }
    }

    // ---- 最近的 hook 调用 ----
    let _ = writeln!(out, "\n== 最近的 hook 调用（该 agent 的最后 10 行）==");
    let mut lines: Vec<String> = config::hook_log_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| {
            text.lines()
                .filter(|line| match filter {
                    Some(agent) => line.contains(hooks::cli_name(agent)),
                    None => true,
                })
                .rev()
                .take(10)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    lines.reverse();
    if lines.is_empty() {
        let _ = writeln!(out, "（没有）—— 一次都没跑过，或者跑过但一行都没留下");
    } else {
        for line in lines {
            // **脱敏**：这段文本是设计给人复制出去的（贴 issue、发给别人），
            // 而日志里记着出站钩子的完整 URL —— 里面有推送服务的密钥。
            let _ = writeln!(out, "{}", redact(&line));
        }
    }

    out
}

/// 把一行日志里的**隐私**抹掉，只留形状。
///
/// **为什么必须做**：诊断文本是设计给人复制出去的，而 `hooks.log` 里记着出站钩子的
/// 完整 URL（`…/message/push?pushkey=…`）。一段让人「复制这段去求助」的文本，不该顺手
/// 把用户的密钥一起送出去。
///
/// 抹的是**值**，不是整条信息：端点在哪、成没成功、什么时候发的都还看得见，排查能力
/// 不损失。刻意放在这里而不是改日志写入：日志是排查工具，「它到底发了什么」必须能
/// 看见（用户要拿它和自己服务器的访问日志对照），要收窄的是「复制出去」这条路。
fn redact(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for (index, token) in line.split(' ').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&redact_token(token));
    }
    // 家目录前缀也收起来：它是「这台机器上这个人」，对排查没用
    shorten_home(out)
}

/// 敏感的键名。**按子串匹配**：`pushkey` / `apiKey` / `X-Auth-Token` 都该中。
const SECRET_KEYS: [&str; 9] = [
    "pushkey", "token", "secret", "password", "passwd", "apikey", "api_key", "auth", "signature",
];

fn redact_token(token: &str) -> String {
    // URL 的 query 整个换成 `?…`：诊断要的是「发到哪个端点去了」，而不是参数值
    if token.contains("://") {
        if let Some((head, _)) = token.split_once('?') {
            return format!("{head}?…");
        }
        return token.to_string();
    }
    // `key=value` 形式：只看键名。**键名匹配到就抹值**，宁多抹一点 —— 少抹一次就是
    // 一次真实的泄漏，多抹一次的代价只是少一条线索。
    if let Some((key, _)) = token.split_once('=') {
        let key = key.to_ascii_lowercase();
        if SECRET_KEYS.iter().any(|secret| key.contains(secret)) {
            return format!("{key}=…");
        }
    }
    token.to_string()
}

/// 配置文件：路径、在不在、多大、什么时候改的。
///
/// **时间戳值得带上**：用户说「我点了接入怎么还不行」时，第一个要排掉的可能就是
/// 「那个文件到底被写过没有」——而「两分钟前改过」和「三天前改过」是两件事。
fn write_config(out: &mut String, agent: Agent) {
    let Ok(path) = hooks::config_path(agent) else {
        let _ = writeln!(out, "配置路径 读不出来");
        return;
    };
    let _ = writeln!(out, "配置 {}", short_path(&path));
    match std::fs::metadata(&path) {
        Ok(meta) => {
            let _ = writeln!(out, "  存在 是，{} 字节，改于 {}", meta.len(), mtime(&path));
        }
        Err(err) => {
            let _ = writeln!(out, "  存在 否（{err}）");
        }
    }
}

/// 配置里**实际写着**的那条命令，以及它现在还跑不跑得起来。
///
/// 这两行是「装了却没反应」最直接的证据：命令是某次装进去的，之后应用被挪走，
/// 配置里就留下一条跑不起来的命令 —— 而**智能体执行一个不存在的程序不会报错**。
fn write_config_command(out: &mut String, agent: Agent) {
    if matches!(hooks::install_kind(agent), InstallKind::PluginFile) {
        // 插件式没有「配置里那条命令」这回事 —— 它的上报命令写在我们生成的源码里。
        return;
    }
    if hooks::installed_http(agent) == Some(true) {
        // **没有命令是设计如此**，不是读不出来。写成「读不出来」会把人引到
        // 「是不是我配置写错了」那条岔路上。
        let _ = writeln!(out, "配置里没有命令 —— 这条接入走 HTTP，不需要命令跑得起来");
        return;
    }
    match hooks::installed_command(agent) {
        Some(command) => {
            let _ = writeln!(
                out,
                "配置里的命令 {}\n  现在解析得到 {}",
                // 过一遍 `redact`：命令里可能带家目录（`~/.local/bin/signalhub`，
                // 用户自己包的），而这段文本是往外贴的
                redact(&command),
                hooks::command_resolves(&command)
            );
        }
        None => {
            let _ = writeln!(out, "配置里的命令 没有（没接入，或读不出来）");
        }
    }
}

/// 插件文件的「形状」检查。
///
/// **只对 opencode 做**，因为只有它这里有一个已知的、会**静默失效**的形状差异：
/// V1 的 `export const` 形式会被 V2 拒绝加载（报错在对方的界面上，用户往往只看到
/// 一句「Plugin must export a default definition」）。另外两家的形状检查没有可对照
/// 的真值，硬写只会产生假的告警。
fn write_plugin_shape(out: &mut String, agent: Agent) {
    if !matches!(hooks::install_kind(agent), InstallKind::PluginFile) {
        return;
    }
    let Ok(path) = hooks::config_path(agent) else {
        return;
    };
    // Hermes 是 Python 包（`__init__.py`），形状检查和 opencode 不是一回事
    let Ok(source) = std::fs::read_to_string(&path) else {
        return;
    };
    if let Some(warning) = plugin_shape(agent, &source) {
        let _ = writeln!(out, "  ⚠ {warning}");
    }
}

/// 见 [`write_plugin_shape`]。
fn plugin_shape(agent: Agent, source: &str) -> Option<&'static str> {
    if agent != Agent::Opencode {
        return None;
    }
    if source.contains("export default") {
        None
    } else if source.contains("export const") || source.contains("export function") {
        Some("插件文件是旧形状（V1）。opencode V2 会拒绝加载它 —— 在「接入」里重新点一次就会改写成新形状。")
    } else {
        Some("插件文件认不出形状（既没有 export default，也没有 V1 那种导出）—— 它可能被别的工具改过。")
    }
}

/// 信号源类型的中文说法。认不出来就原样显示 —— **诊断文本不该替上报方编一个类别**。
///
/// 空串是「没声明」（老文件 / 没带这个字段的上报方），按缺省那类算。
fn source_text(source: &str) -> &str {
    match source.trim() {
        "" => "智能体",
        "agent" => "智能体",
        "system" => "系统",
        "custom" => "自定义",
        other => other,
    }
}

/// 相对时间，秒/分/时/天。
fn seconds_text(at_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let seconds = now.saturating_sub(at_ms) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

/// 修改时间，**说成「多久以前」**。
///
/// 为什么是相对的：带上它就是为了回答「我点了接入之后，那个文件到底被写过没有」——
/// 而「2 分钟前」比一个 Unix 秒数直接得多（那个数还得用户自己换算）。换算成日历日期
/// 要引入时区与日历逻辑，这里不值得。
///
/// 拿不到就写「未知」而不是报错：它是排查的旁证，不是关键路径。
fn mtime(path: &Path) -> String {
    let at_ms = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as u64);
    match at_ms {
        Some(at) => format!("{} 前", seconds_text(at)),
        None => "未知".to_string(),
    }
}

/// 路径里把家目录缩成 `~`。
///
/// **不只是为了好看**：这段文本是设计给人复制出去的，而绝对路径里带着用户名。缩过
/// 之后用户自己照样一眼认得（`~/.config/opencode/plugins/signalhub.js`），贴出去却
/// 不带身份信息。
fn short_path(path: &Path) -> String {
    shorten_home(path.display().to_string())
}

/// 把整段文本里的家目录前缀换成 `~`。
fn shorten_home(text: String) -> String {
    let Some(home) = config::home_dir().filter(|home| !home.as_os_str().is_empty()) else {
        return text;
    };
    text.replace(&home.to_string_lossy().into_owned(), "~")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opencode_plugin_shape_is_called_out() {
        // V2 形状：不告警
        assert_eq!(
            plugin_shape(Agent::Opencode, "export default {\n  id: \"signalhub\",\n};\n"),
            None
        );
        // V1 形状：这是那个**静默失效**的坑，必须指出来
        assert!(
            plugin_shape(Agent::Opencode, "export const signalhub = async () => {};\n").is_some()
        );
        // 被改得认不出来：也说一声，而不是当作没问题
        assert!(plugin_shape(Agent::Opencode, "// 空文件\n").is_some());
    }

    /// 只查 opencode：别的插件式接入（Pi 是 TS、Hermes 是 Python）没有可对照的
    /// 真值，硬查只会产生假告警。
    #[test]
    fn other_plugins_are_not_shape_checked() {
        assert_eq!(plugin_shape(Agent::Pi, "// 空文件\n"), None);
        assert_eq!(plugin_shape(Agent::Hermes, "// 空文件\n"), None);
    }

    #[test]
    fn seconds_read_as_a_duration() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(seconds_text(now), "0s");
        assert_eq!(seconds_text(now - 90_000), "1m");
        assert_eq!(seconds_text(now - 3 * 3_600_000), "3h");
        assert_eq!(seconds_text(now - 2 * 86_400_000), "2d");
    }

    /// **诊断文本里不许出现密钥。** 它是设计给人复制出去求助的，而日志里记着出站
    /// 钩子的完整 URL —— 里面有推送服务的 key（见 issue #2）。
    #[test]
    fn secrets_never_survive_into_the_pasted_text() {
        let line = "04:10:58Z  → https://api2.example.com/message/push?pushkey=SUPERSECRET ok 200";
        let redacted = redact(line);

        assert!(!redacted.contains("SUPERSECRET"), "密钥漏出来了：{redacted}");
        assert!(
            redacted.contains("api2.example.com"),
            "端点该留着（排查要看的正是它）：{redacted}"
        );
        assert!(redacted.contains("ok 200"), "结果该留着：{redacted}");
    }

    /// `key=value` 形式同样要抹；不敏感的键**不许**动（别把有用的信息也抹了）。
    #[test]
    fn sensitive_keys_are_masked_but_ordinary_text_survives() {
        assert_eq!(redact_token("apiKey=abc123"), "apikey=…");
        assert_eq!(redact_token("--token=abc"), "--token=…");
        assert_eq!(redact_token("session=build-1"), "session=build-1");
        assert_eq!(redact_token("working"), "working");
        assert_eq!(redact_token("https://example.com/hook"), "https://example.com/hook");
    }

    /// 家目录缩成 `~`：它是「这台机器上这个人」，对排查没用。
    #[test]
    fn the_home_prefix_is_shortened() {
        let Some(home) = config::home_dir() else {
            return;
        };
        let line = format!("配置 {}/.claude/settings.json", home.display());
        let redacted = redact(&line);
        assert!(redacted.starts_with("配置 ~/"), "{redacted}");
    }

    /// 配置路径那一行也走同样的缩写：绝对路径里带着用户名。
    #[test]
    fn config_paths_lose_the_username() {
        let Some(home) = config::home_dir() else {
            return;
        };
        let path = home.join(".claude/settings.json");
        let text = short_path(&path);
        assert_eq!(text, "~/.claude/settings.json");
        assert!(!text.contains(&home.to_string_lossy().into_owned()));
    }
}
