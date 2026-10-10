use crate::notify::Notifier;
use crate::state::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 检查间隔。TTL 精度够用，同时不会让空闲进程频繁唤醒。
const TICK: Duration = Duration::from_millis(250);

/// 定期丢弃过期会话并重算聚合。
/// 没有这个任务，TTL 就只是写在数据里、没人执行的规则。
pub fn spawn(notify: Notifier, store: Arc<Mutex<Store>>) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        loop {
            tick.tick().await;
            let (changed, snapshot) = {
                let mut guard = store.lock().expect("state store poisoned");
                let changed = guard.expire(std::time::Instant::now());
                (changed, guard.snapshot())
            };
            if changed {
                notify(&snapshot);
            }
        }
    });
}
