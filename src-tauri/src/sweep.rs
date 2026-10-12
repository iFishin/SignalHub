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
            // **只在真的变了之后才建快照，而且仍在同一把锁里。**
            //
            // 快照要给每个组克隆名字和消息、还要排两次序；而空闲时这个循环每秒跑
            // 四次，每次都白建一份没人要的 —— 灯球是常驻的，那点开销会一直跑下去。
            //
            // **不能写成「先解锁、变了再重新加锁」**：两次加锁之间别的线程可以改
            // store，那样推出去的快照和「什么东西变了」这个判断就对不上了。
            let snapshot = {
                let mut guard = store.lock().expect("state store poisoned");
                let changed = guard.expire(std::time::Instant::now());
                if changed { Some(guard.snapshot()) } else { None }
            };
            if let Some(snapshot) = snapshot {
                notify(&snapshot);
            }
        }
    });
}
