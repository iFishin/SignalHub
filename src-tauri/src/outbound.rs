//! 出站钩子：聚合变化时按 `when` 过滤后依次执行。
//!
//! 和 `hooks.rs` 的区别：那个是**入站**的（把智能体的 hook 装进它们的配置文件，
//! 让它们能上报进来）；这个是**出站**的（SignalHub 主动往外发或执行东西）。
//! 两个名字都在用「hook」这个词，但方向相反。
//!
//! 三条约束：
//! 1. **必须异步。** 触发点在状态变更路径上（HTTP 上报和 sweep 都会走到），
//!    同步发网络请求或等命令结束会把状态机卡住。
//! 2. **一条钩子失败不影响其它钩子**，也不影响状态机本身。失败只写 stderr。
//! 3. **命令要带超时。** 挂死的钩子会把 tokio 的阻塞线程池占满。

use crate::config::{Hook, HookAction};
use crate::state::{Signal, Snapshot};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 单条钩子的执行上限。HTTP 有 ureq 自己的超时；命令靠这个兜底。
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// 触发所有匹配的钩子。**不阻塞**：每条都丢进 `spawn_blocking`。
///
/// `transitioned` 是全局聚合本次有没有跃迁，`changes` 是本次跃迁了的组 ——
/// 前者给不限定组的钩子用，后者给限定了组的钩子用（见 `config::Hook::fires`）。
pub fn fire(
    hooks: &[Hook],
    snapshot: &Snapshot,
    transitioned: bool,
    changes: &[(String, Signal)],
    log: &OutboundLog,
) {
    let aggregate = snapshot.aggregate.name();

    for hook in hooks
        .iter()
        .filter(|hook| hook.fires(aggregate, transitioned, changes))
    {
        // 还没填目标的钩子不触发 —— 它是编辑器里的中间态，不是错误
        if !hook.action.is_configured() {
            continue;
        }

        let action = hook.action.clone();
        let payload = match serde_json::to_string(&Payload {
            snapshot,
            at_ms: now_ms(),
        }) {
            Ok(payload) => payload,
            Err(err) => {
                eprintln!("[signalhub] 序列化钩子载荷失败: {err}");
                continue;
            }
        };
        let log = log.clone();

        tauri::async_runtime::spawn_blocking(move || {
            let (ok, detail) = match &action {
                HookAction::Http { url } => match post(url, &payload) {
                    Ok(status) => (true, status),
                    Err(err) => (false, err),
                },
                HookAction::Command { command } => match run_command(command) {
                    Ok(()) => (true, String::new()),
                    Err(err) => (false, err),
                },
            };
            record(&log, action.target(), ok, &detail);
        });
    }
}

/// 一条钩子的执行结果。
///
/// 存在的理由很具体：「我的服务器没收到」这件事，光看 stderr 是查不出来的（从
/// Dock 启动的话 stderr 根本没地方去）。界面上要能回答「发出去没有、对方回了几」。
#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    pub at_ms: u64,
    /// 目标（网址或命令）。
    ///
    /// **按目标而不是按下标去对应钩子**：钩子列表可以增删、可以排序，下标会错位，
    /// 于是界面上会显示成「另一条钩子的结果」—— 比不显示更糟。
    pub target: String,
    pub ok: bool,
    /// 成功是 HTTP 状态码（命令为空），失败是原因。
    pub detail: String,
}

/// 最近几次出站调用，从旧到新。
pub type OutboundLog = Arc<Mutex<VecDeque<Attempt>>>;

/// 记多少条。出站钩子本来就不频繁（只在跃迁时触发），200 条够回溯很久。
const LOG_CAPACITY: usize = 200;

/// 记一笔结果：写日志文件、失败时打 stderr、推进环形缓冲。
fn record(log: &OutboundLog, target: String, ok: bool, detail: &str) {
    let at_ms = now_ms();
    let outcome = if ok {
        format!("ok {detail}")
    } else {
        format!("失败: {detail}")
    };

    // 和入站（智能体调 CLI）写的是同一个文件，`→` 是出站的标记
    crate::config::append_hook_log(&format!(
        "{}  → {target:<44} {outcome}\n",
        crate::config::utc_hms()
    ));
    if !ok {
        eprintln!("[signalhub] 钩子执行失败（{target}）: {detail}");
    }

    let mut log = log.lock().expect("outbound log poisoned");
    log.push_back(Attempt {
        at_ms,
        target,
        ok,
        detail: detail.to_string(),
    });
    while log.len() > LOG_CAPACITY {
        log.pop_front();
    }
}

#[derive(serde::Serialize)]
struct Payload<'a> {
    snapshot: &'a Snapshot,
    /// 触发时刻（epoch 毫秒）。接收方多半要记时间，而 since_ms 是状态起点。
    at_ms: u64,
}

/// 发一条 POST。成功时把**状态码**带回来 —— 只报「没报错」区分不了 200 和 500，
/// 而「对方到底收没收到」正是用户要看的。
fn post(url: &str, payload: &str) -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3)))
        .timeout_per_call(Some(HOOK_TIMEOUT))
        .build();

    // ureq 默认把 4xx/5xx 也当成 Err（http_status_as_error），所以 Ok 就意味着 2xx
    ureq::Agent::new_with_config(config)
        .post(url.trim())
        .header("Content-Type", "application/json")
        .send(payload)
        .map(|response| response.status().as_u16().to_string())
        .map_err(|err| err.to_string())
}

/// 跑一条 shell 命令。
///
/// 走 `sh -c` / `cmd /C`，所以用户能写管道和重定向（这正是钩子有用的地方）。
/// 输出全部丢弃：钩子是用来触发副作用的，它的 stdout 没地方去。
fn run_command(command: &str) -> Result<(), String> {
    #[cfg(windows)]
    let mut process = {
        let mut process = std::process::Command::new("cmd");
        process.args(["/C", command]);
        process
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut process = std::process::Command::new("sh");
        process.args(["-c", command]);
        process
    };

    let mut child = process
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("启动命令失败（{command}）: {err}"))?;

    // 轮询等它结束，超时就杀掉 —— 挂死的钩子不该把阻塞线程池占满
    let deadline = Instant::now() + HOOK_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                return Err(format!("命令超过 {} 秒未结束，已终止", HOOK_TIMEOUT.as_secs()));
            }
            Err(err) => return Err(format!("等待命令失败: {err}")),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookAction;

    fn hook(when: &[&str], action: HookAction) -> Hook {
        Hook {
            when: when.iter().map(|s| s.to_string()).collect(),
            groups: Vec::new(),
            action,
        }
    }

    fn grouped_hook(when: &[&str], groups: &[&str]) -> Hook {
        Hook {
            when: when.iter().map(|s| s.to_string()).collect(),
            groups: groups.iter().map(|s| s.to_string()).collect(),
            action: HookAction::Http { url: "https://x/y".into() },
        }
    }

    #[test]
    fn an_empty_when_matches_every_state() {
        let any = hook(&[], HookAction::Http { url: "https://x/y".into() });
        for name in crate::state::Signal::ALL_NAMES {
            assert!(any.matches(name), "{name} 应被空 when 匹配");
        }
    }

    #[test]
    fn when_filters_to_the_listed_states_only() {
        let only_urgent = hook(
            &["permission", "error"],
            HookAction::Http { url: "https://x/y".into() },
        );
        assert!(only_urgent.matches("permission"));
        assert!(only_urgent.matches("error"));
        for name in ["idle", "working", "done", "stale", "offline", "off"] {
            assert!(!only_urgent.matches(name), "{name} 不该被匹配");
        }
    }

    /// 派生状态也要能作为过滤条件 —— 用户可能只想知道「会话沉默了」
    #[test]
    fn derived_states_can_be_filtered_on() {
        let stale_only = hook(&["stale"], HookAction::Command { command: "true".into() });
        assert!(stale_only.matches("stale"));
        assert!(!stale_only.matches("idle"));
    }

    // ---- 按组过滤 -------------------------------------------------------------

    /// 不限定组的钩子看**全局聚合的跃迁**。这是原有语义，不能变 —— 改成「任何组
    /// 一变就发」会让用户已经配好的钩子静默变成刷屏。
    #[test]
    fn an_ungrouped_hook_fires_only_on_an_aggregate_transition() {
        let any = hook(&["error"], HookAction::Http { url: "https://x/y".into() });
        let changes = vec![("ci".to_string(), Signal::Error)];

        assert!(any.fires("error", true, &changes), "聚合跃迁到 error 要触发");
        assert!(
            !any.fires("error", false, &changes),
            "聚合没跃迁就不触发 —— 否则任何组一变都会发"
        );
        assert!(!any.fires("idle", true, &changes), "when 不匹配就不触发");
    }

    /// 限定了组的钩子看**那几个组各自的跃迁**，不看全局聚合
    #[test]
    fn a_grouped_hook_fires_on_that_groups_own_transition() {
        let ci_errors = grouped_hook(&["error"], &["ci"]);

        assert!(
            !ci_errors.fires("error", true, &[("deploy".into(), Signal::Error)]),
            "别的组跃迁不该触发"
        );
        assert!(
            ci_errors.fires("working", false, &[("ci".into(), Signal::Error)]),
            "自己跃迁到 error 就触发，不需要全局聚合也跃迁"
        );
        assert!(
            !ci_errors.fires("working", false, &[("ci".into(), Signal::Working)]),
            "自己跃迁了，但状态不在 when 里"
        );
    }

    #[test]
    fn a_grouped_hook_can_watch_several_groups() {
        let both = grouped_hook(&["error"], &["ci", "deploy"]);
        assert!(both.fires("idle", false, &[("deploy".into(), Signal::Error)]));
        assert!(!both.fires("idle", false, &[("docs".into(), Signal::Error)]));
    }

    /// **这条就是按组过滤存在的理由**：一组失败时，别的组更紧急会把这件事从全局
    /// 聚合里整个盖掉 —— 只看聚合的话，ci 的失败会被另一组在等授权这件事吞掉。
    #[test]
    fn a_grouped_hook_is_not_gagged_by_a_more_urgent_group() {
        let ci_errors = grouped_hook(&["error"], &["ci"]);
        assert!(
            ci_errors.fires("permission", false, &[("ci".into(), Signal::Error)]),
            "别的组更紧急不该把 ci 的失败吞掉"
        );
    }
}
