//! 用系统默认程序打开一个路径或网址。
//!
//! 三个平台各一条命令，**不引依赖** —— 为这一件事装一个 opener 插件不划算。
//!
//! **参数一律作为独立的 argv 传，绝不过 shell。** 有的目标来自智能体上报的
//! `cwd`，是不可信输入，拼进 `sh -c` 就是一个命令注入。（`outbound.rs` 的钩子走
//! shell 是因为那里用户就是要写管道和重定向，这里没有那个需求。）

use std::io;
use std::process::{Child, Command};

/// 在文件管理器里**选中**这个路径，而不是打开它。
///
/// 用户问的通常是「它在哪」，弹一个新窗口反而挡事。
pub fn reveal(path: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let command = {
        let mut command = Command::new("open");
        command.args(["-R", path]);
        command
    };
    // explorer 的选择语法很怪：`/select,<路径>` 必须当成**一个**参数传
    #[cfg(target_os = "windows")]
    let command = {
        let mut command = Command::new("explorer");
        command.arg(format!("/select,{path}"));
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    spawn_detached(command)
}

/// 用系统默认程序打开一个路径或网址（网址会走默认浏览器）。
///
/// 与 `reveal` 的区别是「打开它」而不是「在文件管理器里指出来」。
pub fn open(target: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    // explorer 拿到网址会交给默认浏览器，拿到目录会打开资源管理器 —— 两种都对
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    let mut command = Command::new(program);
    command.arg(target);
    spawn_detached(command)
}

/// 起一个进程，然后在一条后台线程上回收它。
///
/// 直接 `spawn()` 不管的话，子进程退出后会变成僵尸留在进程表里 —— 这是个常驻
/// 应用，攒起来是要出问题的。这里不需要它的结果，所以只回收、不阻塞。
fn spawn_detached(mut command: Command) -> io::Result<()> {
    let mut child: Child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真正值得测的是「起完进程之后回收掉它」——`reveal` / `open` 里唯一的逻辑。
    ///
    /// 刻意**不去调 `reveal` / `open` 本身**：那会真的弹出文件管理器窗口，
    /// 跑一次测试打断人一次。用一个什么都不做的命令来测同一条路径。
    #[test]
    fn a_detached_process_is_spawned_and_reaped() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        };
        #[cfg(not(windows))]
        let command = Command::new("true");

        spawn_detached(command).expect("起一个无害的进程不该失败");
        // 回收在后台线程上，给它一点时间 —— 这里只能验证「不 panic、不报错」，
        // 僵尸进程有没有清掉得看进程表，不值得为此写一条依赖时序的测试
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    /// 目标不存在时**必须报错**，不能静默成功 —— 否则界面上会出现「点了没反应，
    /// 也没有任何提示」
    #[test]
    fn a_missing_program_is_an_error() {
        let command = Command::new("signalhub-definitely-not-a-real-command-xyz");
        assert!(spawn_detached(command).is_err());
    }
}
