//! 「盯住这个任务」：收件箱里点一下，这条会话跑完 / 出错 / 消失时叫你一声。
//!
//! 为什么需要它：告警看的是**全局聚合跃迁**（`alerts.rs`），一个跑了二十分钟的
//! 构建全程都是绿 `working`，聚合一次都没跃迁，于是永远不会通知你「它好了」。
//! 转发钩子配 `when: done` 能凑个近似，但粒度是组、还得先配好一个外发地址 ——
//! 这不是「盯住这一个任务」。
//!
//! **一次性、不落盘**：触发即移除，重启不保留。它盯的是这一次运行，不是一条长期
//! 偏好；做成配置会多出一份要清理的状态，而用户早忘了自己盯过哪个 UUID。
//!
//! 通知走 `alerts::send_message`（和「已进入穿透模式」那些通告同一个出口），
//! 固定不带声音。**不受勿扰时段影响**是刻意的：勿扰说的是「全局别打扰我」，
//! 而这是用户针对单个任务点名要的那一条；两者不是同一层设置。它也不补发 ——
//! 事件过去了就是过去了。
//!
//! 判据挂在事件流水上（`events::tap`），所以 HTTP 上报、hook、TTL 到期、手动
//! 清掉会话都会触发，不用在 `Store::apply` 的每个调用点各写一遍。
//!
//! **远程会话的 `end` 不算终态**：它们靠几秒一次的拉取维持，TTL 一抖就会整批
//! 消失、下一拍整批回来（`remote.rs`）。把这种抖动当「跑完了」既会误报，还会
//! 把订阅提前用掉。真跑完时远程也会先送 `change: done`，那条照样触发。
//!
//! 触发后广播 `watches-changed`：收件箱窗口常驻、只是隐藏，挂载时那次
//! `get_watches` 不会重跑，不广播的话按钮会一直停在「取消盯」。

use crate::events::Event;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone)]
struct Watch {
    /// 收件箱那一行的显示名。通知里要能认出是哪个任务。
    label: String,
}

/// app state：当前盯着的会话。key 是 session id。
#[derive(Default)]
pub struct Watches(Mutex<HashMap<String, Watch>>);

impl Watches {
    pub fn add(&self, session: String, label: String) {
        self.0
            .lock()
            .expect("watches poisoned")
            .insert(session, Watch { label });
    }

    pub fn remove(&self, session: &str) -> bool {
        self.0
            .lock()
            .expect("watches poisoned")
            .remove(session)
            .is_some()
    }

    /// 前端拿去决定按钮显示「盯住」还是「取消盯」。
    pub fn ids(&self) -> Vec<String> {
        let guard = self.0.lock().expect("watches poisoned");
        let mut ids: Vec<String> = guard.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// 从表里摘走这一帧里该响的那些 —— **拿走即移除**，一条订阅只叫一次。
    fn take_fired(&self, events: &[Event]) -> Vec<(String, String)> {
        let mut guard = self.0.lock().expect("watches poisoned");
        let mut out = Vec::new();
        for event in events {
            if !is_final(event) {
                continue;
            }
            if let Some(watch) = guard.remove(&event.session) {
                out.push((watch.label, event.state.clone()));
            }
        }
        out
    }
}

/// 「不用再等了」：`done` / `error` 是会话自己说的；本地会话被删掉（`end`，
/// 到期或手动清除）也该叫一声，否则用户会一直以为还在跑。
///
/// 远程的 `end` 例外 —— 见文件头。它只在最后状态不是终态时被忽略：远程会话
/// 真报了 `done` 再消失，`state == "done"` 这一条照样会触发。
fn is_final(event: &Event) -> bool {
    if event.state == "done" || event.state == "error" {
        return true;
    }
    event.kind == "end" && event.machine.is_empty()
}

/// 每帧事件流过后调一次。两个 state 都在 `build()` 链上 `manage`，但通知链可能
/// 在 `setup` 跑完前就送来第一帧 —— 一律用 `try_state` 安静跳过，别让启动期
/// 的一帧把应用带崩（那时前端也还没法点「盯住」，不会漏掉通知）。
///
/// ⚠️ 语言 state 注册的是 `Arc<lang::LocalLang>`（即 `crate::SharedLang`），
/// 不是裸 `LocalLang`。Tauri 按 `TypeId` 取 state，写错类型不会在编译期报错，
/// 而是运行时 `state() called before manage()` panic —— release 下 `panic=abort`
/// 直接退出。取配置里的那一个类型，别照着别的模块抄。
pub fn fire(app: &AppHandle, events: &[Event]) {
    let (Some(watches), Some(lang_state)) = (
        app.try_state::<Watches>(),
        app.try_state::<crate::SharedLang>(),
    ) else {
        return;
    };
    let fired = watches.take_fired(events);
    if fired.is_empty() {
        return;
    }

    let lang = lang_state.get();
    for (label, state_name) in fired {
        let state = crate::state::Signal::from_name(&state_name);
        let (title, body) = crate::lang::watch_notice(lang, state, &label);
        crate::alerts::send_message(app, &title, &body);
    }
    // 订阅已被拿走，让还开着的收件箱把按钮刷回「盯住」。没有 webview 时
    // emit 会失败，那也无所谓 —— 下次打开会重新拉一次全量。
    let _ = app.emit("watches-changed", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, session: &str, state: &str) -> Event {
        Event {
            ts_ms: 1000,
            kind: kind.to_string(),
            session: session.to_string(),
            group: "g".to_string(),
            agent: "claude-code".to_string(),
            machine: String::new(),
            state: state.to_string(),
            cwd: "/work/a".to_string(),
            message: String::new(),
        }
    }

    #[test]
    fn a_watch_fires_once_then_is_gone() {
        let watches = Watches::default();
        watches.add("s1".to_string(), "build".to_string());
        let first = watches.take_fired(&[
            event("change", "s1", "working"),
            event("change", "s1", "done"),
        ]);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, "build");
        assert_eq!(first[0].1, "done");
        assert!(watches.take_fired(&[event("end", "s1", "done")]).is_empty());
    }

    #[test]
    fn a_session_that_vanishes_also_fires() {
        let watches = Watches::default();
        watches.add("s1".to_string(), "build".to_string());
        let out = watches.take_fired(&[event("end", "s1", "working")]);
        assert_eq!(out.len(), 1, "被清掉也要叫一声");
    }

    /// 远程会话靠 TTL 维持，掉一拍就整批消失又回来。这种 `end` 既不能误报
    /// 「跑完了」，也不能把订阅用掉 —— 用户还在等真正的终态。
    #[test]
    fn a_remote_session_dropping_out_does_not_consume_the_watch() {
        let watches = Watches::default();
        watches.add("s1".to_string(), "build".to_string());
        let mut dropped = event("end", "s1", "working");
        dropped.machine = "macbook".to_string();
        assert!(watches.take_fired(&[dropped]).is_empty());
        assert_eq!(watches.ids(), vec!["s1"]);

        // 它真报了 done，那条仍然要响。
        let mut done = event("change", "s1", "done");
        done.machine = "macbook".to_string();
        assert_eq!(watches.take_fired(&[done]).len(), 1);
    }

    #[test]
    fn an_unrelated_session_does_not_fire() {
        let watches = Watches::default();
        watches.add("s1".to_string(), "build".to_string());
        assert!(watches.take_fired(&[event("change", "s2", "done")]).is_empty());
        assert_eq!(watches.ids(), vec!["s1"]);
    }

    #[test]
    fn removing_a_watch_is_idempotent() {
        let watches = Watches::default();
        watches.add("s1".to_string(), "build".to_string());
        assert!(watches.remove("s1"));
        assert!(!watches.remove("s1"));
        assert!(watches.ids().is_empty());
    }

    /// `working` 的整段长任务不该提前把订阅用掉。
    #[test]
    fn a_long_running_session_stays_watched() {
        let watches = Watches::default();
        watches.add("s1".to_string(), "build".to_string());
        for _ in 0..20 {
            assert!(watches.take_fired(&[event("change", "s1", "working")]).is_empty());
        }
        assert!(!is_final(&event("start", "s1", "working")));
        assert_eq!(watches.ids(), vec!["s1"]);
    }
}
