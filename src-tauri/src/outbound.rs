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
use crate::state::Snapshot;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 单条钩子的执行上限。HTTP 有 ureq 自己的超时；命令靠这个兜底。
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// 触发所有匹配的钩子。**不阻塞**：每条都丢进 `spawn_blocking`。
pub fn fire(hooks: &[Hook], snapshot: &Snapshot) {
    let aggregate = snapshot.aggregate.name();

    for hook in hooks.iter().filter(|hook| hook.matches(aggregate)) {
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

        tauri::async_runtime::spawn_blocking(move || {
            let result = match action {
                HookAction::Http { url } => post(&url, &payload),
                HookAction::Command { command } => run_command(&command),
            };
            if let Err(err) = result {
                eprintln!("[signalhub] 钩子执行失败: {err}");
            }
        });
    }
}

#[derive(serde::Serialize)]
struct Payload<'a> {
    snapshot: &'a Snapshot,
    /// 触发时刻（epoch 毫秒）。接收方多半要记时间，而 since_ms 是状态起点。
    at_ms: u64,
}

fn post(url: &str, payload: &str) -> Result<(), String> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3)))
        .timeout_per_call(Some(HOOK_TIMEOUT))
        .build();

    ureq::Agent::new_with_config(config)
        .post(url.trim())
        .header("Content-Type", "application/json")
        .send(payload)
        .map(|_| ())
        .map_err(|err| format!("POST {url} 失败: {err}"))
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
            action,
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
}
