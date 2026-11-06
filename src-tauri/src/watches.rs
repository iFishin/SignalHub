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

use crate::events::Event;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

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

/// 「不用再等了」的三种情况：`done` / `error` 是会话自己说的，`end` 是它被删掉
/// （到期或手动清除）—— 后者也该叫一声，否则用户会一直以为还在跑。
fn is_final(event: &Event) -> bool {
    event.kind == "end" || event.state == "done" || event.state == "error"
}

/// 每帧事件流过后调一次。读不到 app state（setup 还没跑完）就安静跳过：
/// 那时前端也还没法点「盯住」，不存在漏掉一次通知的问题。
pub fn fire(app: &AppHandle, events: &[Event]) {
    let Some(watches) = app.try_state::<Watches>() else {
        return;
    };
    let lang = app.state::<crate::lang::LocalLang>().get();
    for (label, state_name) in watches.take_fired(events) {
        let state = crate::state::Signal::from_name(&state_name);
        let (title, body) = crate::lang::watch_notice(lang, state, &label);
        crate::alerts::send_message(app, &title, &body);
    }
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
