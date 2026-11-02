//! 进程还在不在 —— 只用来看「会话是没人上报，还是进程真的没了」。
//!
//! 判据刻意只有一个，每个平台一次存在性检查，**不去读进程名、不读启动时间**：
//!
//! - Unix：`kill(pid, 0)`，不发送任何信号，只做一次存在性/权限检查。
//! - Windows：`OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject(…, 0)`，
//!   进程对象 signaled 就是已退出，还在跑就是 `WAIT_TIMEOUT`。
//!
//! 读进程名要 `sysctl` / `proc_pidinfo` 这类平台私有接口，而 pid 被复用只可能
//! 发生在「进程刚死、下一个 tick 之前就被复用」这 250ms 里（macOS 的 pid 是顺序
//! 分配的，睡觉醒来也不会跳）。为这点概率换一堆平台代码不划算。

/// 这个 pid 对应的进程还活着吗。
///
/// **「探不出来」一律当活着。** 这条判断只用来提前收掉会话，而误判成「死了」
/// 会把一块还该亮着的牌直接抹掉（比多亮一会儿糟得多），所以每一个拿不准的
/// 分支都倒向 `true`。
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    // 0 在 `kill(0, …)` 里指的是「当前进程组」，不是一个进程号；比 i32::MAX
    // 大的值转不成 pid_t。两者都探不了活，直接判死（入库时也会把 0 丢掉，
    // 这里是第二道闸）。
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    // SAFETY: kill 是 POSIX 标准调用；sig=0 不发信号，只做错误检查。
    let result = unsafe { kill(pid as i32, 0) };
    // `EPERM`（1）也算活着：进程在，只是不归我们管 —— 我们的智能体都是同一个
    // 用户起的，正常不会走到这里，但把「没权限」当成「死了」会误删会话。
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
}

/// Windows：进程对象在退出时会变成 signaled，所以「等一下看它 signal 了没」
/// 就是一次干净的存活检查 —— 不读内存、不注入、不需要进程配合。
///
/// 三条分支各自的理由：
///
/// - `OpenProcess` 失败且是 `ERROR_ACCESS_DENIED`：进程**在**，只是不归我们管
///   （提权进程、别的用户）。判死会误删会话。
/// - 失败且是 `ERROR_INVALID_PARAMETER`：这个 pid 不存在。
/// - 其余失败（含 `WAIT_FAILED` 这种意外）：当活着 —— 见函数头。
#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, GetLastError, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    // Windows 没有「进程组 0」这回事，但 0 也从来不是一个合法 pid；入库时已经
    // 过滤过一次，这里是第二道闸（和 Unix 那边对齐）。
    if pid == 0 {
        return false;
    }

    // SAFETY: OpenProcess 只要一个访问掩码和一个 pid，不碰我们自己的内存；
    // 返回的句柄由下面显式关闭。
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        // SAFETY: GetLastError 无参数、无副作用。
        return match unsafe { GetLastError() } {
            ERROR_ACCESS_DENIED => true,
            ERROR_INVALID_PARAMETER => false,
            _ => true,
        };
    }

    // SAFETY: handle 刚由 OpenProcess 得到且非空；超时 0 表示只看当前状态。
    let code = unsafe { WaitForSingleObject(handle, 0) };
    // SAFETY: 句柄是我们的且只用这一次；关掉免得每次 sweep 漏一个 ——
    // sweep 是 250ms 一拍，漏了会稳定增长。
    unsafe { CloseHandle(handle) };

    if code == WAIT_TIMEOUT {
        // 还在跑
        true
    } else if code == WAIT_OBJECT_0 {
        // 已退出（进程对象 signaled）
        false
    } else {
        // WAIT_FAILED 之类：探不出来，按活着处理
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个平台都必须能认出「自己还活着」—— 这条同时钉住了 `alive` 真的接上了
    /// 系统调用，而不是恒返回某个常量。
    #[test]
    fn the_current_process_is_alive() {
        assert!(alive(std::process::id()));
    }

    /// 0 不是任何进程；`i32::MAX` 远大于系统的 pid 上限。
    #[test]
    fn a_pid_that_cannot_exist_is_not_alive() {
        assert!(!alive(0));
        assert!(!alive(i32::MAX as u32));
    }
}
