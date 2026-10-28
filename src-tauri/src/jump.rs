//! 从收件箱跳到会话所在的终端。
//!
//! 目标只有一个：**把用户送到那个正在等他的窗口前**。它不重启智能体、不恢复会话，
//! 只是把焦点切过去 —— 所以失败时的正确行为是「说清楚没找到」，而不是硬开一个新终端
//! 或者随便激活一个应用（那会让人以为跳对了）。
//!
//! 分三层，按可靠性从高到低：
//!
//! 1. **tmux**：pid → tty → pane，选中那个 pane，再按 client tty 找到外层终端标签页；
//! 2. **Terminal.app / iTerm2**：pid → tty → 那个 tab（AppleScript，首次会问一次
//!    「控制终端」的自动化权限）；
//! 3. **兜底**：按父进程链认出是哪个终端 / IDE，激活它；再不行就打开项目目录。
//!
//! 所有外部命令都用 argv、不过 shell；tty 只接受一个很小的字符集（[`valid_tty`]），
//! 因为它是从 `ps` 的输出里读来的，不能当成可信字符串直接拼进脚本。
//!
//! **不猜。** 认不出就返回 [`JumpKind::NotFound`]；只做得到「激活应用」时说
//! [`JumpKind::App`]，让界面能明说只走到哪一步。

use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JumpKind {
    /// 切到了 tmux 的某个 pane。
    Tmux,
    /// 切到了 Terminal / iTerm2 的某个标签页。
    Terminal,
    /// 只激活了应用（认得是哪个终端 / IDE，但没法精确定位标签页）。
    App,
    /// 只打开了项目目录（Finder）。
    Folder,
    /// 什么也没找到。
    NotFound,
    /// 这个平台还没有精确定位（目前只有 macOS）。**只在非 macOS 上构造** ——
    /// 放在这里是为了前端有一个统一的类型，不按平台分叉。
    #[cfg(not(target_os = "macos"))]
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub struct JumpResult {
    pub kind: JumpKind,
    /// 给界面显示的一句话。成功时也要说清「跳到了哪一层」。
    pub detail: String,
}

/// 跳过去。`pid` / `cwd` 来自会话本身，取不到就给 `None`。
pub fn to_session(pid: Option<u32>, cwd: Option<&str>) -> JumpResult {
    #[cfg(target_os = "macos")]
    {
        mac::to_session(pid, cwd)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // 别的平台先只做到「打开项目目录」：精确定位各个终端的标签页是另一套
        // API（Windows Terminal / X11），没实测过的东西不假装能用。
        let _ = pid;
        match cwd.filter(|path| std::path::Path::new(path).is_dir()) {
            Some(path) => match crate::open::open(path) {
                Ok(()) => JumpResult {
                    kind: JumpKind::Unsupported,
                    detail: "已打开项目目录（精确定位终端目前只在 macOS 上）".to_string(),
                },
                Err(err) => JumpResult {
                    kind: JumpKind::NotFound,
                    detail: format!("打开项目目录失败: {err}"),
                },
            },
            None => JumpResult {
                kind: JumpKind::NotFound,
                detail: "这个会话没有可定位的终端或目录".to_string(),
            },
        }
    }
}

/// 把 `ps -o tty=` 的输出变成一个可安全塞进 argv 的设备名。
///
/// macOS 给 `ttys003`，Linux 给 `pts/3`；空、`?`、`??` 都表示「没有控制终端」。
/// 只放行字母数字和 `/ . _ -`，长度封顶 64 —— 它来自外部命令的输出，不能当可信
/// 字符串（虽然下一站是 AppleScript 的参数而不是脚本，但边界就该在这里收窄）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn valid_tty(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_start_matches("/dev/");
    if raw.is_empty() || raw.len() > 64 || raw == "?" || raw == "??" || raw.contains("..") {
        return None;
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        return None;
    }
    Some(raw.to_string())
}

/// 从 `tmux list-panes -a -F '#{pane_id}\t#{pane_tty}'` 的输出里找某个 tty 的 pane。
///
/// 纯函数：真去跑 tmux 的那一层在 macOS 模块里，这一段可以在任何平台单测。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn pane_for_tty(output: &str, tty: &str) -> Option<String> {
    let want = format!("/dev/{tty}");
    for line in output.lines() {
        let Some((id, pane_tty)) = line.split_once('\t') else {
            continue;
        };
        if pane_tty.trim() == want {
            return Some(id.trim().to_string());
        }
    }
    None
}

/// 从 `tmux list-clients -F '#{client_session}\t#{client_tty}'` 的输出里找某个会话的客户端。
///
/// 同一个 tmux server 可能挂着多个终端；优先选**正在显示那个会话**的客户端，
/// 找不到再退回第一个有 tty 的客户端。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn client_for_session(output: &str, session: &str) -> Option<String> {
    let mut first: Option<String> = None;
    for line in output.lines() {
        let Some((name, tty)) = line.split_once('\t') else {
            continue;
        };
        let Some(tty) = valid_tty(tty) else {
            continue;
        };
        if name.trim() == session {
            return Some(tty);
        }
        first.get_or_insert(tty);
    }
    first
}

/// 进程名 → macOS 应用名。只认确定存在、且名字稳定的那些。
///
/// `Code` 是 VS Code 主进程的 comm；`iTerm2` 是 iTerm 的主进程。认不出就 `None`，
/// 绝不按「像不像终端」猜 —— 激活错应用比不激活更让人困惑。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_for_comm(comm: &str) -> Option<&'static str> {
    Some(match comm {
        "Terminal" => "Terminal",
        "iTerm2" => "iTerm",
        "Warp" => "Warp",
        "Alacritty" => "Alacritty",
        "kitty" => "kitty",
        "WezTerm" => "WezTerm",
        "Ghostty" => "Ghostty",
        "Hyper" => "Hyper",
        "Code" => "Visual Studio Code",
        "Cursor" => "Cursor",
        "Windsurf" => "Windsurf",
        "Zed" => "Zed",
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    pub fn to_session(pid: Option<u32>, cwd: Option<&str>) -> JumpResult {
        if let Some(pid) = pid {
            if let Some(tty) = tty_of(pid) {
                if let Some(pane) = tmux_pane(&tty)
                    && select_tmux_pane(&pane)
                {
                    // 外层终端：先按「正在显示这个 tmux 会话」的 client 找，
                    // 找不到再退回第一个 client。
                    if let Some(client) = tmux_client_tty(&pane) {
                        match focus_terminal(&client) {
                            Ok(true) => {
                                return ok(JumpKind::Tmux, "已切到 tmux 里那个 pane");
                            }
                            Ok(false) => {}
                            Err(err) => {
                                // 已经选中了 pane，只是没能把外层窗口带到前面；
                                // 把原因说出来，别假装完全成功。
                                return JumpResult {
                                    kind: JumpKind::Tmux,
                                    detail: format!("已选中 tmux pane，但没能切到外层窗口：{err}"),
                                };
                            }
                        }
                    }
                    return ok(JumpKind::Tmux, "已选中 tmux 里那个 pane");
                }

                match focus_terminal(&tty) {
                    Ok(true) => return ok(JumpKind::Terminal, "已切到那个终端标签页"),
                    Ok(false) => {}
                    Err(err) => {
                        // 精确切换失败时不要直接放弃：下面还会尝试「激活应用」。
                        // 但把原因留着，兜底也失败时用它作为提示。
                        if let Some(result) = activate_for_pid(pid, Some(err)) {
                            return result;
                        }
                    }
                }
            }

            if let Some(result) = activate_for_pid(pid, None) {
                return result;
            }
        }

        fallback(cwd)
    }

    /// 认得出是哪个终端 / IDE 就激活它。`note` 是精确切换失败的原因，拼进
    /// 详情里 —— 「没找到标签页」和「自动化权限被拒」对用户是两件事。
    fn activate_for_pid(pid: u32, note: Option<String>) -> Option<JumpResult> {
        let app = terminal_app_for_pid(pid)?;
        if !activate(app) {
            return None;
        }
        let detail = match note {
            Some(err) => format!("已激活 {app}；没能精确定位：{err}"),
            None => format!("已激活 {app}（没能精确定位到具体标签页）"),
        };
        Some(JumpResult {
            kind: JumpKind::App,
            detail,
        })
    }

    fn fallback(cwd: Option<&str>) -> JumpResult {
        match cwd.filter(|path| std::path::Path::new(path).is_dir()) {
            Some(path) => match crate::open::reveal_dir(path) {
                Ok(()) => JumpResult {
                    kind: JumpKind::Folder,
                    detail: "没找到对应的终端，已在 Finder 中打开项目目录".to_string(),
                },
                Err(err) => JumpResult {
                    kind: JumpKind::NotFound,
                    detail: format!("打开项目目录失败: {err}"),
                },
            },
            None => JumpResult {
                kind: JumpKind::NotFound,
                detail: "这个会话没有可定位的终端或目录".to_string(),
            },
        }
    }

    fn ok(kind: JumpKind, detail: &str) -> JumpResult {
        JumpResult {
            kind,
            detail: detail.to_string(),
        }
    }

    /// `ps -o tty=`：`ttys003` / `pts/3` / 空 / `??`。
    fn tty_of(pid: u32) -> Option<String> {
        let out = Command::new("ps")
            .args(["-o", "tty=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        valid_tty(&String::from_utf8_lossy(&out.stdout))
    }

    fn tmux_pane(tty: &str) -> Option<String> {
        let out = Command::new("tmux")
            .args(["list-panes", "-a", "-F", "#{pane_id}\t#{pane_tty}"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        pane_for_tty(&String::from_utf8_lossy(&out.stdout), tty)
    }

    /// 选中 pane 所在的 session / window，再选中 pane。
    ///
    /// `switch-client` 让**当前客户端**切到那个会话；`select-window` / `select-pane`
    /// 改的是会话自己的当前窗口 —— 两者缺一，多会话时可能只改了别处的状态。
    fn select_tmux_pane(pane: &str) -> bool {
        let Some(session) = tmux_query(&["display-message", "-p", "-t", pane, "#{session_name}"])
        else {
            return false;
        };
        let window = tmux_query(&[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{session_name}:#{window_index}",
        ]);

        let _ = run_ok("tmux", &["switch-client", "-t", &session]);
        if let Some(window) = window {
            let _ = run_ok("tmux", &["select-window", "-t", &window]);
        }
        run_ok("tmux", &["select-pane", "-t", pane])
    }

    /// 找到「正在显示某个 pane 所在会话」的外层终端 tty。
    fn tmux_client_tty(pane: &str) -> Option<String> {
        let session = tmux_query(&["display-message", "-p", "-t", pane, "#{session_name}"])?;
        let out = Command::new("tmux")
            .args(["list-clients", "-F", "#{client_session}\t#{client_tty}"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        client_for_session(&String::from_utf8_lossy(&out.stdout), &session)
    }

    fn tmux_query(args: &[&str]) -> Option<String> {
        let out = Command::new("tmux").args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    }

    fn run_ok(program: &str, args: &[&str]) -> bool {
        Command::new(program)
            .args(args)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 切到 tty 对应的 Terminal / iTerm2 标签页。
    ///
    /// 返回 `Ok(false)` = 这两个终端都没匹配上；`Err` = 自动化权限被拒 / 脚本本身
    /// 出错。两者对用户的含义不同，所以分开。
    fn focus_terminal(tty: &str) -> Result<bool, String> {
        let device = format!("/dev/{tty}");
        let mut errors: Vec<String> = Vec::new();

        if running("Terminal") {
            match run_script(TERMINAL_SCRIPT, &device) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(err) => errors.push(err),
            }
        }
        if running("iTerm2") {
            match run_script(ITERM_SCRIPT, &device) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(err) => errors.push(err),
            }
        }

        if let Some(err) = errors.into_iter().next() {
            Err(err)
        } else {
            Ok(false)
        }
    }

    fn running(app: &str) -> bool {
        Command::new("pgrep")
            .args(["-x", app])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 脚本通过 `argv` 拿 tty，**不拼字符串**；`--` 之后才是参数。
    fn run_script(script: &str, tty: &str) -> Result<bool, String> {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .arg("--")
            .arg(tty)
            .output()
            .map_err(|err| format!("无法运行 osascript: {err}"))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("not authorized")
                || stderr.contains("Not authorized")
                || stderr.contains("1743")
                || stderr.contains("1002")
            {
                return Err(
                    "需要在「系统设置 → 隐私与安全性 → 自动化」里允许 SignalHub 控制终端"
                        .to_string(),
                );
            }
            return Err(format!("终端脚本失败: {}", stderr.trim()));
        }

        Ok(String::from_utf8_lossy(&out.stdout).trim() == "ok")
    }

    fn terminal_app_for_pid(pid: u32) -> Option<&'static str> {
        let mut current = pid;
        for _ in 0..12 {
            if let Some(app) = comm_of(current).and_then(|comm| app_for_comm(&comm)) {
                return Some(app);
            }
            current = parent_of(current)?;
            if current <= 1 {
                break;
            }
        }
        None
    }

    fn comm_of(pid: u32) -> Option<String> {
        let out = Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // `ps comm` 可能给整条路径；只取最后一段。
        comm.rsplit('/').next().map(|part| part.to_string())
    }

    fn parent_of(pid: u32) -> Option<u32> {
        let out = Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    fn activate(app: &str) -> bool {
        Command::new("open")
            .args(["-a", app])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// Terminal.app：`tty of tab` 精确到标签页。
    ///
    /// 用 `on run argv` 接收 tty，而不是把 tty 插进脚本文本里 —— 脚本是常量，
    /// 参数走 argv，注入面为零。
    const TERMINAL_SCRIPT: &str = r#"
on run argv
  set want to item 1 of argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if tty of t is want then
          set selected tab of w to t
          set frontmost of w to true
          activate
          return "ok"
        end if
      end repeat
    end repeat
  end tell
  return "no"
end run
"#;

    /// iTerm2：会话才带 tty，所以多套一层 `sessions of tab`。
    const ITERM_SCRIPT: &str = r#"
on run argv
  set want to item 1 of argv
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if tty of s is want then
            select s
            select t
            select w
            activate
            return "ok"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "no"
end run
"#;
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_real_tty_is_accepted_and_noise_is_not() {
        assert_eq!(valid_tty("ttys003\n"), Some("ttys003".into()));
        assert_eq!(valid_tty("/dev/ttys003"), Some("ttys003".into()));
        assert_eq!(valid_tty("pts/3"), Some("pts/3".into()));

        for raw in ["", "?", "??", "ttys 003", "ttys;rm", "../x", "a".repeat(65).as_str()] {
            assert_eq!(valid_tty(raw), None, "{raw:?} 不该被当成 tty");
        }
    }

    #[test]
    fn a_pane_is_found_by_its_tty() {
        let output = "%0\t/dev/ttys001\n%7\t/dev/ttys003\n";
        assert_eq!(pane_for_tty(output, "ttys003"), Some("%7".into()));
        assert_eq!(pane_for_tty(output, "ttys009"), None);
        // 不带 tab 的行（tmux 版本差异）不能 panic，也不能错配
        assert_eq!(pane_for_tty("%0 /dev/ttys001", "ttys001"), None);
    }

    #[test]
    fn the_client_showing_the_session_wins() {
        let output = "other\t/dev/ttys001\nwork\t/dev/ttys002\n";
        assert_eq!(client_for_session(output, "work"), Some("ttys002".into()));
        // 没有匹配的会话时退回第一个可用客户端
        assert_eq!(client_for_session(output, "missing"), Some("ttys001".into()));
        assert_eq!(client_for_session("", "work"), None);
    }

    #[test]
    fn only_known_app_names_are_activated() {
        assert_eq!(app_for_comm("Terminal"), Some("Terminal"));
        assert_eq!(app_for_comm("iTerm2"), Some("iTerm"));
        assert_eq!(app_for_comm("Code"), Some("Visual Studio Code"));
        assert_eq!(app_for_comm("Cursor"), Some("Cursor"));
        // 认不出来就别猜
        assert_eq!(app_for_comm("bash"), None);
        assert_eq!(app_for_comm("zsh"), None);
    }
}
