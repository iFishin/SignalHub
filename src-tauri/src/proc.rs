//! 进程还在不在 —— 只用来看「会话是没人上报，还是进程真的没了」。
//!
//! 判据刻意只有一个：`kill(pid, 0)` 不发送任何信号，只做一次存在性/权限检查。
//! **不去读进程名、不读启动时间**：那需要 `sysctl` / `proc_pidinfo` 这类平台
//! 私有接口，而 pid 被复用只可能发生在「进程刚死、下一个 tick 之前就被复用」
//! 这 250ms 里（macOS 的 pid 是顺序分配的，睡觉醒来也不会跳）。为这点概率
//! 换一堆平台代码不划算。
//!
//! **只在 Unix 上实现。** Windows 上 `Report.pid` 在入库时就被丢掉了（见
//! `state::effective_pid`），这里恒为 `true`：即使有人绕过那道闸，后果也只是
//! 退回「一直空闲」而不会误删会话。

/// 这个 pid 对应的进程还活着吗。
///
/// `EPERM`（1）也算活着：进程在，只是不归我们管 —— 我们的智能体都是同一个
/// 用户起的，正常不会走到这里，但把「没权限」当成「死了」会误删会话。
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    // pid 0 是「当前进程组」，1 是 launchd/init —— 都不是上报方，别拿它们
    // 当「活着」的理由。
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    // SAFETY: kill 是 POSIX 标准调用；sig=0 不发信号，只做错误检查。
    let result = unsafe { kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
}

/// 非 Unix：`Report.pid` 入库前就被丢掉了，这条永远用不上。见文件头。
#[cfg(not(unix))]
pub fn alive(_pid: u32) -> bool {
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_is_alive() {
        assert!(alive(std::process::id()));
    }

    #[test]
    fn a_pid_that_cannot_exist_is_not_alive() {
        // 0 不是任何进程；i32::MAX 远大于系统的 pid 上限。
        assert!(!alive(0));
        assert!(!alive(i32::MAX as u32));
    }
}
