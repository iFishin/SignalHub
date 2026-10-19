//! 按平台挑 shell 跑一条命令。**只有这一处**知道「Windows 上没有 `sh`」。
//!
//! 这个函数存在的理由是它被写错过两次：`outbound::run_command` 一直有平台分支，
//! 而接入页的「试一下」（`lib.rs` 的 `try_agent`）直接 `Command::new("sh")` ——
//! 于是 Windows 上那条探测**必然**失败，看起来却像「接入坏了」（issue #5）。
//! 两处各写一遍就是这种情况的温床，所以收拢到这里。

use std::process::Command;

/// 用「跑 `line`」的方式建一个子进程，还没启动。
///
/// 走 `sh -c` / `cmd /C`：配置里写的是 shell 片段（管道、重定向、`&&`），不是
/// argv。调用方接着设置 stdin / stdout / stderr。
pub fn command(line: &str) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd");
        process.args(["/C", line]);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        process.args(["-c", line]);
        process
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 选中的 program 是**平台事实**，不是实现细节：Windows 上跑 `sh` 是
    /// `NotFound`，而那个错误会被界面显示成「命令跑不起来」。
    #[test]
    fn picks_the_shell_this_platform_actually_has() {
        let process = command("echo hi");
        let program = process.get_program().to_string_lossy().to_string();
        #[cfg(windows)]
        assert_eq!(program, "cmd");
        #[cfg(not(windows))]
        assert_eq!(program, "sh");
        // 第一个参数是执行开关，第二个才是命令本身
        assert_eq!(process.get_args().count(), 2);
    }

    /// 光比较 program 名字证明不了它真能跑 —— 这条真的执行一次。
    #[test]
    fn the_shell_actually_runs_a_command() {
        let output = command("echo signalhub-shell-ok")
            .output()
            .expect("启动 shell 失败");
        assert!(output.status.success(), "shell 返回非零");
        assert!(String::from_utf8_lossy(&output.stdout).contains("signalhub-shell-ok"));
    }
}
