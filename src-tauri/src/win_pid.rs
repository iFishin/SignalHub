//! Windows 上给命令式 hook 找「智能体自己的进程号」。
//!
//! ## 为什么需要这份代码
//!
//! `proc::alive` 的 Windows 分支（`OpenProcess` + `WaitForSingleObject`）本来就能
//! 探活，服务端的 `Report.pid` 也不分平台；缺的只是**没人把 pid 送进来**。
//! Unix 侧是靠 hook 命令前缀 `SIGNALHUB_PID="$PPID"` 送的 —— 那是实测过的一条链
//! （Claude Code 用 `/bin/sh -c` 跑 hook，`$PPID` 就是 claude 自己，见 `hooks.rs`
//! 的 `resolve_hook_command`）。Windows 没有 `$PPID`，`cfg!(unix)` 把前缀挡掉了，
//! 于是命令式接入只能吃「工具开始时给 30 分钟」那条兜底。
//!
//! ## 为什么不改 hook 命令、改成 CLI 自己找
//!
//! 改 `resolve_hook_command` 会换掉写进各家配置文件里的命令文本：`command_stale`
//! 是按文本比较的，所有 Windows 用户会被提示「已装的是旧版、请重装」，而安装
//! 命令的 shell 方言各家还没实测过。**CLI 自己沿父进程链找**则完全不碰已装的
//! 配置，老 hook 一行不改就升级，Unix 侧也一个字节都不受影响。
//!
//! ## 保守原则（比代码本身重要）
//!
//! 猜错的代价不对称：把 `cmd.exe` 这种短命 shell 的 pid 当成智能体，服务端下一拍
//! 就会按「进程没了」把一块**正亮着的牌立刻抹掉** —— 比现在的沉默更糟。所以：
//! - 只取祖先链上**第一个不是 shell / 系统进程**的祖先，再往上（终端、explorer）
//!   一律不认：拿它们当 pid 会让会话永远「活着」，同样不对；
//! - 链断了、看不懂、进程表打不开、走到系统进程 —— 一律返回 `None`，回落原来的
//!   30 分钟兜底，也就是**最坏和改之前完全一样**；
//! - 插件式接入和 Unix 命令式仍然优先用 `SIGNALHUB_PID`（插件传的就是自己的
//!   `process.pid`，最准），见 CLI 的 `hook_pid()`。
//!
//! ⚠️ **Windows 上各家的父进程派生方式至今没有真机实测过**（2026-11-06）。这份
//! 排除表是按常见 `cmd.exe /c` 链推的保守近似；真机验之前不要把它说成「已验证的
//! 接入能力」，也不要放宽条件去多认几个进程。

/// 进程表里的一行。`exe` 已小写化（见 [`normalize_exe`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
}

/// 一条祖先链最多看几层。够覆盖 `CLI → cmd → shell → agent → terminal`，也
/// 防止进程表里出现环时一路走到天亮。
const MAX_CHAIN: usize = 8;

/// 从「自己 → 父 → 祖父 …」这条链上挑出智能体。
///
/// `chain[0]` **必须是 CLI 自己**；返回的是第一个既不是 shell、也不是系统 /
/// 桌面宿主的祖先的 pid。空链、只有自己、断在 shell 上、撞到系统进程都是 `None`。
pub fn pick_agent_pid(chain: &[ProcEntry]) -> Option<u32> {
    let self_pid = chain.first()?.pid;
    for entry in chain.iter().skip(1) {
        // 自环 / 0 号进程说明进程表读出来的链不可信，别拿它去删会话。
        if entry.pid == 0 || entry.pid == self_pid {
            return None;
        }
        if is_shell(&entry.exe) {
            continue;
        }
        if is_system_or_desktop(&entry.exe) {
            return None;
        }
        return Some(entry.pid);
    }
    None
}

/// 进程名统一成小写，免得 `CMD.EXE` / `cmd.exe` 两套写法。
pub fn normalize_exe(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// hook 派生产生的**中间层**：这些进程跑完命令就退出，绝不能当探活对象。
/// `node.exe` / `python.exe` 刻意**不在**这里 —— Claude Code、Hermes 这类智能体
/// 本身就是它们，排掉就等于永远找不到。
fn is_shell(exe: &str) -> bool {
    matches!(
        exe,
        "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "conhost.exe"
            | "openconsole.exe"
            | "wsl.exe"
            | "bash.exe"
            | "sh.exe"
            | "signalhub.exe"
    )
}

/// 再往上就是桌面 / 系统了。撞到它们说明这条链里**没有**可识别的智能体，
/// 此时返回 `None` 比继续往上找一个永远活着的进程要诚实。
fn is_system_or_desktop(exe: &str) -> bool {
    matches!(
        exe,
        "explorer.exe"
            | "sihost.exe"
            | "dwm.exe"
            | "windowsterminal.exe"
            | "winlogon.exe"
            | "services.exe"
            | "lsass.exe"
            | "csrss.exe"
            | "svchost.exe"
            | "smss.exe"
            | "system"
            | "[system process]"
            | "idle"
    )
}

/// 拍一张进程表快照，沿父进程链找智能体。
///
/// 只在 Windows 编译：其余平台根本不会走到 `hook_pid()` 的这条分支。
#[cfg(windows)]
pub fn current_agent_pid() -> Option<u32> {
    use std::collections::{HashMap, HashSet};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    // `dwSize` 必须在 Process32FirstW 之前填好。用结构体字面量一次给全，不要写
    // `Default::default()` 再逐字段赋值：clippy 的 `field_reassign_with_default`
    // 在 CI 的 `-D warnings` 下会直接把 Windows 那个 job 判红，而这段只在
    // `#[cfg(windows)]` 下编译，macOS 本地跑 clippy 是抓不到的。
    fn snapshot_entry() -> PROCESSENTRY32W {
        PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        }
    }

    // 每条 early return 都得关掉快照句柄 —— 用 Drop 包一层，别靠人记得。
    struct Snapshot(HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let _guard = Snapshot(handle);

    let mut table: HashMap<u32, (u32, String)> = HashMap::new();
    let mut entry = snapshot_entry();
    if unsafe { Process32FirstW(handle, &mut entry) } == 0 {
        return None;
    }
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let exe = normalize_exe(&String::from_utf16_lossy(&entry.szExeFile[..end]));
        table.insert(entry.th32ProcessID, (entry.th32ParentProcessID, exe));

        entry = snapshot_entry();
        if unsafe { Process32NextW(handle, &mut entry) } == 0 {
            break;
        }
    }

    // 从自己往上走。链断了就停 —— 半截链不足以判断谁才是智能体。
    let mut chain: Vec<ProcEntry> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut current = std::process::id();
    while chain.len() < MAX_CHAIN && seen.insert(current) {
        let Some((ppid, exe)) = table.get(&current) else {
            break;
        };
        chain.push(ProcEntry {
            pid: current,
            ppid: *ppid,
            exe: exe.clone(),
        });
        if *ppid == 0 {
            break;
        }
        current = *ppid;
    }
    pick_agent_pid(&chain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(rows: &[(u32, u32, &str)]) -> Vec<ProcEntry> {
        rows.iter()
            .map(|(pid, ppid, exe)| ProcEntry {
                pid: *pid,
                ppid: *ppid,
                exe: (*exe).to_string(),
            })
            .collect()
    }

    #[test]
    fn a_shell_between_the_cli_and_the_agent_is_skipped() {
        // CLI → cmd.exe /c → node.exe (claude) → terminal
        let rows = chain(&[
            (100, 200, "signalhub.exe"),
            (200, 300, "cmd.exe"),
            (300, 400, "node.exe"),
            (400, 500, "windowsterminal.exe"),
        ]);
        assert_eq!(pick_agent_pid(&rows), Some(300));
    }

    #[test]
    fn a_direct_spawn_does_not_need_any_skipping() {
        assert_eq!(
            pick_agent_pid(&chain(&[(100, 300, "signalhub.exe"), (300, 400, "node.exe")])),
            Some(300)
        );
    }

    #[test]
    fn several_shell_layers_are_all_skipped() {
        let rows = chain(&[
            (100, 200, "signalhub.exe"),
            (200, 250, "cmd.exe"),
            (250, 300, "powershell.exe"),
            (300, 400, "claude.exe"),
        ]);
        assert_eq!(pick_agent_pid(&rows), Some(300));
    }

    #[test]
    fn a_desktop_shell_is_never_reported_as_the_agent() {
        // 从资源管理器手动跑一次 hook 测试时就是这条链；宁可不报 pid。
        assert_eq!(
            pick_agent_pid(&chain(&[
                (100, 200, "signalhub.exe"),
                (200, 300, "cmd.exe"),
                (300, 400, "explorer.exe"),
            ])),
            None
        );
    }

    #[test]
    fn a_chain_that_ends_in_a_shell_or_breaks_is_not_enough() {
        assert_eq!(
            pick_agent_pid(&chain(&[(100, 200, "signalhub.exe"), (200, 0, "cmd.exe")])),
            None
        );
        assert_eq!(pick_agent_pid(&chain(&[(100, 0, "signalhub.exe")])), None);
        assert_eq!(pick_agent_pid(&[]), None);
    }

    #[test]
    fn a_cycle_or_pid_zero_never_becomes_a_pid() {
        assert_eq!(
            pick_agent_pid(&chain(&[(100, 100, "signalhub.exe"), (100, 100, "node.exe")])),
            None
        );
        assert_eq!(
            pick_agent_pid(&chain(&[(100, 0, "signalhub.exe"), (0, 0, "node.exe")])),
            None
        );
    }

    #[test]
    fn exe_names_are_case_insensitive() {
        assert_eq!(normalize_exe(" CMD.EXE "), "cmd.exe");
        assert!(is_shell(&normalize_exe("PowerShell.exe")));
        assert!(!is_shell(&normalize_exe("node.exe")));
        assert!(!is_shell(&normalize_exe("python.exe")));
    }
}
