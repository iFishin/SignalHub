//! CLI 的全部文案（中文 / 英文各一份）。
//!
//! **只有 CLI 用**：由 `src/bin/signalhub.rs` 用 `#[path]` 包含，故意**不**挂进
//! `lib.rs` 的模块树 —— 那样 app 侧根本不会编译这个文件，也就不会因为「这些文案
//! 没人用」冒出一片 dead_code 警告（而 CI 现在是 `-D warnings`）。
//!
//! 和 app 侧的 `lang.rs` 是一对：那边画托盘菜单和系统通知，这边印 `--help`、报错
//! 和各子命令的输出。
//!
//! **文案里的 `{}` 是占位符**，由 [`fill`] 从左往右替换。用运行时模板而不是
//! `format!` 是为了让两种语言能并排放进同一张表（`format!` 要求格式串是字面量）。
//! 代价是占位符写错编译器看不住 —— 所以 `fill` 里有一句 `debug_assert` 逐个对数，
//! 而测试跑在 debug 下，CI 会撞上。

use crate::config::Lang;

/// CLI 自己那套文案。
///
/// 收在一个结构里而不是散成一堆函数，理由和 `lang::NativeText` 一样：换语言时
/// 要翻的东西**正好就是这些**，有一处能一次拿全就不会漏。漏掉一条的表现是
/// 「英文界面上冒出一句中文」—— 不报错、也没人会发现，是这类改动里最难自查的问题。
pub struct CliText {
    // ---- 参数与用法 ----
    pub missing_value: &'static str,
    pub port_not_number: &'static str,
    pub unknown_option: &'static str,
    pub bad_duration: &'static str,
    pub duration_too_big: &'static str,
    pub no_command: &'static str,
    pub hook_needs_agent: &'static str,
    /// 两个 `{}`：子命令名（出现两次）
    pub agent_needed: &'static str,
    pub monitor_needs_on_off: &'static str,
    /// 「什么都没给」时的占位。写 `None` 会把 Rust 的 Debug 输出漏给用户看
    pub nothing: &'static str,
    /// 两个 `{}`：子命令名、收到的值
    pub only_hooks: &'static str,
    pub too_many_args: &'static str,

    // ---- 找端点 ----
    pub app_not_running: &'static str,
    /// `{}` 是 `app_not_running`
    pub no_port_file: &'static str,
    pub no_token_file: &'static str,
    pub read_port_failed: &'static str,
    pub read_token_failed: &'static str,

    // ---- HTTP ----
    pub request_timeout: &'static str,
    pub request_failed: &'static str,
    pub read_response_failed: &'static str,
    /// `{}` 状态码、`{}` 服务端给的说明
    pub server_error: &'static str,

    // ---- 上报 ----
    /// `{}` 状态名、`{}` 聚合、`{}` 会话数、`{}` 「无变化」或空
    pub report_ok: &'static str,
    pub no_change: &'static str,

    // ---- status ----
    pub status_aggregate: &'static str,
    pub status_paused: &'static str,
    pub status_hint: &'static str,
    pub status_sessions: &'static str,
    /// 会话行尾的智能体，`{}` 是名字
    pub session_agent: &'static str,
    /// 会话行尾的消息，`{}` 是内容（**含引号**，中英的引号不一样）
    pub session_message: &'static str,

    // ---- watch ----
    pub watch_until_hint: &'static str,
    pub watch_follow_hint: &'static str,
    /// `{}` 聚合、`{}` 会话数
    pub watch_line: &'static str,
    pub watch_timeout: &'static str,
    /// `{}` 收到的值、`{}` 可用状态
    pub until_unknown: &'static str,

    // ---- clear / monitor ----
    /// `{}` 会话、`{}` 「已清除 / 本就不存在」、`{}` 聚合
    pub clear_line: &'static str,
    pub clear_removed: &'static str,
    pub clear_absent: &'static str,
    /// `{}` 「已暂停 / 已恢复」、`{}` 聚合
    pub monitor_line: &'static str,
    pub monitor_paused: &'static str,
    pub monitor_resumed: &'static str,

    // ---- 安装 / 卸载 ----
    /// `{}` 智能体名、`{}` 「已安装 / 已移除 / 演练」
    pub install_title: &'static str,
    pub installed: &'static str,
    pub uninstalled: &'static str,
    pub dry_run_suffix: &'static str,
    pub install_command: &'static str,
    pub install_config: &'static str,
    pub install_backup: &'static str,
    /// `{}` 个数
    pub install_added: &'static str,
    pub install_updated: &'static str,
    pub install_skipped: &'static str,
    /// `{}` 个数
    pub install_removed_handlers: &'static str,
    pub install_no_change: &'static str,
    pub dry_run_footer: &'static str,
    /// `{}` 智能体名
    pub install_restart: &'static str,
    pub path_warning_1: &'static str,
    pub path_warning_2: &'static str,
    pub path_warning_3: &'static str,
    pub uninstall_nothing: &'static str,

    // ---- run ----
    pub run_needs_command: &'static str,
    /// `{}` 失败原因
    pub run_report_failed: &'static str,
    /// `{}` 命令、`{}` 错误
    pub run_spawn_failed: &'static str,
    pub run_wait_failed: &'static str,
    pub run_wait_thread_died: &'static str,

    // ---- hook（stdin） ----
    pub read_stdin_failed: &'static str,
    pub stdin_not_json: &'static str,
    /// hook 里那个「失败: …」，写进 hooks.log
    pub hook_failed: &'static str,

    // ---- 帮助 ----
    /// 两个 `{}`：版本号、智能体名单
    pub help: &'static str,
}

pub const ZH: CliText = CliText {
    missing_value: "{} 缺少取值",
    port_not_number: "--port 需要数字，收到 {}",
    unknown_option: "未知选项 {}，用 --help 看用法",
    bad_duration: "无法解析时长 {}，试试 30s / 5m / 1h",
    duration_too_big: "时长 {} 过大",
    no_command: "缺少状态或子命令，用 --help 看用法",
    hook_needs_agent: "hook 需要智能体名，例如 signalhub hook claude-code",
    agent_needed: "{} 需要智能体名，例如 signalhub {} claude-code",
    monitor_needs_on_off: "monitor 需要 on 或 off，收到 {}",
    nothing: "（没给）",
    only_hooks: "{} 只支持 hooks，收到 {}",
    too_many_args: "多余的参数: {}",

    app_not_running: "SignalHub 没有在运行",
    no_port_file: "{}（找不到 ~/.signalhub/port）",
    no_token_file: "{}（找不到 ~/.signalhub/token）",
    read_port_failed: "读取端口失败: {}",
    read_token_failed: "读取 token 失败: {}",

    request_timeout: "请求超时 —— app 可能在运行但卡住了",
    request_failed: "请求失败: {}",
    read_response_failed: "读取响应失败: {}",
    server_error: "服务返回 {}: {}",

    report_ok: "{} → 聚合 {}（{} 个会话{}）",
    no_change: "，无变化",

    status_aggregate: "聚合: {}",
    status_paused: "监控: 已暂停",
    status_hint: "提示: {}",
    status_sessions: "会话: {}",
    session_agent: "  {}",
    session_message: "  「{}」",

    watch_until_hint: "等待聚合变为 {}…（Ctrl-C 取消）",
    watch_follow_hint: "跟随状态变化（Ctrl-C 退出）",
    watch_line: "{}  {} 个会话",
    watch_timeout: "等待超时：仍是 {}，未变成 {}",
    until_unknown: "--until {} 不是已知状态；可用: {}",

    clear_line: "会话 {} {}（聚合 {}）",
    clear_removed: "已清除",
    clear_absent: "本就不存在",
    monitor_line: "监控{}（聚合 {}）",
    monitor_paused: "已暂停",
    monitor_resumed: "已恢复",

    install_title: "{} 的 hook{}",
    installed: " 已安装",
    uninstalled: " 已移除",
    dry_run_suffix: "（演练，未写入任何文件）",
    install_command: "  命令  {}",
    install_config: "  配置  {}",
    install_backup: "  备份  {}",
    install_added: "  新增  {} 个事件",
    install_updated: "  更新  {} 个事件（写入的命令变了）",
    install_skipped: "  跳过  {} 个（已存在且命令相同）",
    install_removed_handlers: "  移除  {} 个 handler",
    install_no_change: "无需改动。",
    dry_run_footer: "以上为演练结果；去掉 --dry-run 才会真正写入。",
    install_restart: "重启 {} 后生效。",
    path_warning_1: "提示：signalhub 不在 PATH 中，已写入绝对路径。",
    path_warning_2: "      若移动或删除了该二进制，hook 会静默失效。",
    path_warning_3: "      把它放进 PATH（例如软链到 /usr/local/bin）后重新执行本命令即可改为裸名字。",
    uninstall_nothing: "没有找到本工具装过的 hook，无需改动。",

    run_needs_command: "run 需要一条命令，写在 -- 之后，例如 signalhub run --agent aider -- aider",
    run_report_failed: "上报失败（{}）；命令照跑",
    run_spawn_failed: "起不了 {}：{}",
    run_wait_failed: "等 {} 结束失败：{}",
    run_wait_thread_died: "等待子进程的线程意外结束了",

    read_stdin_failed: "读取 stdin 失败: {}",
    stdin_not_json: "stdin 不是合法 JSON: {}",
    hook_failed: "失败: {}",

    help: "\
signalhub {} —— 桌面悬浮状态灯的上报客户端

用法:
  signalhub <状态> [选项]            上报状态
  signalhub hook <智能体>            从 stdin 读智能体 hook 的 JSON 并上报
  signalhub status [--json]          查询当前聚合状态
  signalhub watch [选项]             跟随状态变化（终端里盯着，或 CI 里等状态）
  signalhub run [选项] -- <命令>     把一条命令包起来上报（通用适配，见下）
  signalhub clear --session <id>     清除某个会话
  signalhub monitor on|off           全局暂停 / 恢复监控
  signalhub on | off                 monitor 的简写
  signalhub install hooks <智能体>   把 hook 写进智能体的配置文件
  signalhub uninstall hooks <智能体> 移除本工具写进去的 hook

状态:
  idle thinking working done attention permission error off
  off 作用在会话上等价于清除该会话；全局暂停请用 monitor off。
  状态名接受别名，如 blocked→error、completed→done、permission_request→permission。

选项:
  --session <id>    会话标识，默认 default
  --agent <name>    上报方名字，用于 UI 归因
  --cwd <path>      项目路径
  --group <name>    归属的组，默认 default。多分组时灯上每组一块牌
  --message <text>  悬停提示文本
  --ttl <时长>      覆盖默认过期时间，如 30s / 5m / 1h / 90（裸数字按秒）
  --json            status 输出原始 JSON
  --dry-run         install / uninstall 只打印将要做的改动，不写文件
  --interval <时长>  watch 的轮询间隔，默认 500ms
  --until <状态>     watch 等到聚合变成该状态就退出 0（如 --until done）
  --timeout <时长>   watch 的等待上限，超时退出 1
  --port <n>        覆盖端口自动发现
  --never-fail      任何错误都以退出码 0 结束（hook 模式隐含此行为）
  -h, --help        显示本帮助
  -V, --version     显示版本

退出码:
  0  成功
  1  参数错误或服务端返回错误
  3  SignalHub 没有在运行

  run 是唯一例外：它把被包命令的退出码原样传出去。包一层不能改被包命令的语义，
  否则 CI 里 `signalhub run -- make test` 会永远是绿的。

通用适配（signalhub run）:
  任何跑在终端里的工具都能这么接，不需要知道它自己的 hook 格式：

    signalhub run --agent aider -- aider --model sonnet

  开始前报 working，跑的过程中每 20 秒续期一次，退出后报 done（退出码 0）
  或 error（非零）。stdout / stderr 原样透传；灯没开的时候命令照跑。

  被包命令自己的选项写在 -- 之后 —— 那之后的一切都原样交给它，不会被当成
  我们的选项解析。

智能体: {}
  install hooks 会改动它们的配置文件，写之前自动备份，且只动 hooks 这一个键。
  （Goose 例外：它是插件，装的是自己的目录，不碰你的共享配置。）
",

};

pub const EN: CliText = CliText {
    missing_value: "{} needs a value",
    port_not_number: "--port needs a number, got {}",
    unknown_option: "Unknown option {}; see --help",
    bad_duration: "Cannot parse duration {}; try 30s / 5m / 1h",
    duration_too_big: "Duration {} is too large",
    no_command: "Missing a state or subcommand; see --help",
    hook_needs_agent: "hook needs an agent name, e.g. signalhub hook claude-code",
    agent_needed: "{} needs an agent name, e.g. signalhub {} claude-code",
    monitor_needs_on_off: "monitor needs on or off, got {}",
    nothing: "(none)",
    only_hooks: "{} only supports hooks, got {}",
    too_many_args: "Unexpected extra arguments: {}",

    app_not_running: "SignalHub is not running",
    no_port_file: "{} (no ~/.signalhub/port)",
    no_token_file: "{} (no ~/.signalhub/token)",
    read_port_failed: "Could not read the port: {}",
    read_token_failed: "Could not read the token: {}",

    request_timeout: "Request timed out — the app may be running but stuck",
    request_failed: "Request failed: {}",
    read_response_failed: "Could not read the response: {}",
    server_error: "Server returned {}: {}",

    report_ok: "{} → aggregate {} ({} session{})",
    no_change: ", no change",

    status_aggregate: "Aggregate: {}",
    status_paused: "Monitoring: paused",
    status_hint: "Hint: {}",
    status_sessions: "Sessions: {}",
    session_agent: "  {}",
    session_message: "  \"{}\"",

    watch_until_hint: "Waiting for the aggregate to become {}… (Ctrl-C to cancel)",
    watch_follow_hint: "Following state changes (Ctrl-C to quit)",
    watch_line: "{}  {} sessions",
    watch_timeout: "Timed out: still {}, never became {}",
    until_unknown: "--until {} is not a known state; available: {}",

    clear_line: "Session {} {} (aggregate {})",
    clear_removed: "cleared",
    clear_absent: "did not exist",
    monitor_line: "Monitoring {} (aggregate {})",
    monitor_paused: "paused",
    monitor_resumed: "resumed",

    install_title: "{} hooks{}",
    installed: " installed",
    uninstalled: " removed",
    dry_run_suffix: " (dry run, nothing written)",
    install_command: "  command  {}",
    install_config: "  config   {}",
    install_backup: "  backup   {}",
    install_added: "  added    {} events",
    install_updated: "  updated  {} events (the command changed)",
    install_skipped: "  skipped  {} (already present with the same command)",
    install_removed_handlers: "  removed  {} handlers",
    install_no_change: "Nothing to change.",
    dry_run_footer: "That was a dry run; drop --dry-run to actually write.",
    install_restart: "Restart {} to take effect.",
    path_warning_1: "Note: signalhub is not on PATH, so an absolute path was written.",
    path_warning_2: "      If that binary is moved or deleted, the hook fails silently.",
    path_warning_3: "      Put it on PATH (symlink to /usr/local/bin, say) and re-run to use the bare name.",
    uninstall_nothing: "No hooks installed by this tool were found; nothing to change.",

    run_needs_command: "run needs a command, after --, e.g. signalhub run --agent aider -- aider",
    run_report_failed: "Report failed ({}); running the command anyway",
    run_spawn_failed: "Could not start {}: {}",
    run_wait_failed: "Could not wait for {}: {}",
    run_wait_thread_died: "The thread waiting on the child process died unexpectedly",

    read_stdin_failed: "Could not read stdin: {}",
    stdin_not_json: "stdin is not valid JSON: {}",
    hook_failed: "failed: {}",

    help: "\
signalhub {} — reporting client for the desktop status lamp

Usage:
  signalhub <state> [options]        Report a state
  signalhub hook <agent>             Read an agent hook's JSON from stdin and report it
  signalhub status [--json]          Query the current aggregate
  signalhub watch [options]          Follow state changes (watch in a terminal, or wait in CI)
  signalhub run [options] -- <cmd>   Wrap a command and report for it (see below)
  signalhub clear --session <id>     Clear a session
  signalhub monitor on|off           Pause / resume monitoring globally
  signalhub on | off                 Shorthand for monitor
  signalhub install hooks <agent>    Write hooks into an agent's config file
  signalhub uninstall hooks <agent>  Remove the hooks this tool wrote

States:
  idle thinking working done attention permission error off
  Reporting off for a session is the same as clearing it; use monitor off to pause
  everything. Aliases are accepted, e.g. blocked→error, completed→done.

Options:
  --session <id>    Session identifier, default default
  --agent <name>    Reporter name, used for attribution in the UI
  --cwd <path>      Project path
  --group <name>    Group to file this under; default default. One sign per group
  --message <text>  Tooltip text
  --ttl <dur>       Override the default expiry, e.g. 30s / 5m / 1h / 90 (bare = seconds)
  --json            status prints raw JSON
  --dry-run         install / uninstall only print what they would change
  --interval <dur>  watch polling interval, default 500ms
  --until <state>   watch exits 0 once the aggregate becomes that state
  --timeout <dur>   watch gives up after this long, exiting 1
  --port <n>        Override port auto-discovery
  --never-fail      Exit 0 on any error (implied by hook mode)
  -h, --help        Show this help
  -V, --version     Show the version

Exit codes:
  0  success
  1  bad arguments, or the server returned an error
  3  SignalHub is not running

  run is the one exception: it passes the wrapped command's exit code straight
  through. Wrapping must not change what the command means, or
  `signalhub run -- make test` in CI would always be green.

Wrapping any command (signalhub run):
  Any tool that runs in a terminal can be hooked up this way — no need to know its
  own hook format:

    signalhub run --agent aider -- aider --model sonnet

  Reports working before it starts, renews every 20 seconds while it runs, and
  reports done (exit 0) or error (non-zero) when it exits. stdout / stderr pass
  straight through; if the lamp is not running the command runs anyway.

  The wrapped command's own options go after -- : everything past that is handed
  over untouched rather than parsed as ours.

Agents: {}
  install hooks edits their config files; it backs each one up first and only
  touches the hooks key. (Goose is the exception: it is a plugin, so we install
  into its own directory and leave your shared config alone.)
",

};

/// 取某一语言的文案。
pub fn text(lang: Lang) -> &'static CliText {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

/// 把模板里的 `{}` 从左往右换成 `args`。
///
/// **占位符和参数的个数必须相等**，否则 debug 下会在这里炸 —— 那是刻意的：接收
/// 一侧写错（文案里多一个 `{}`）会漏出一个字面的 `{}`，而少一个则会静默丢掉一个
/// 参数，两种都很难在界面上注意到。CI 跑的是 debug 测试，所以能挡住。
pub fn fill(template: &str, args: &[&str]) -> String {
    debug_assert_eq!(
        template.matches("{}").count(),
        args.len(),
        "文案 {template:?} 的占位符和参数对不上"
    );

    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    for arg in args {
        match rest.split_once("{}") {
            Some((head, tail)) => {
                out.push_str(head);
                out.push_str(arg);
                rest = tail;
            }
            // 参数比占位符多：上面那句 debug_assert 已经报过了，这里只是别再炸一次
            None => break,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_in_order_and_leaves_the_rest_alone() {
        assert_eq!(fill("a {} b {}", &["1", "2"]), "a 1 b 2");
        assert_eq!(fill("no placeholders", &[]), "no placeholders");
        // 参数里有 `{}` 也不该被再替换一次（是替换结果，不是模板）
        assert_eq!(fill("{}", &["{}"]), "{}");
    }

    /// **两份文案的字段必须一一对应。** 这是一条防漏译的测试：`CliText` 的字段全是
    /// `&'static str`，加一条新的却只往一边填，编译器**不会**说话（另一边还是
    /// 合法的），表现就是英文界面上冒出一句中文。
    ///
    /// 这里比的是「同一字段在两边都非空」，加上一条「英文那份里不许有 CJK」——
    /// 后者才是真正抓得住漏译的那条（把中文抄进英文那一列是最常见的错法）。
    #[test]
    fn both_languages_are_filled_in_and_english_has_no_chinese() {
        let zh_fields = field_values(&ZH);
        let en_fields = field_values(&EN);
        assert_eq!(zh_fields.len(), en_fields.len());

        for ((name, zh), (_, en)) in zh_fields.iter().zip(en_fields.iter()) {
            assert!(!zh.trim().is_empty(), "{name} 中文那份是空的");
            assert!(!en.trim().is_empty(), "{name} 英文那份是空的");
            assert!(
                !en.chars().any(is_cjk),
                "{name} 的英文那份里还有中文：{}",
                &en[..en.len().min(60)]
            );
            // 占位符个数也要一致，否则 `fill` 会在其中一种语言下炸
            assert_eq!(
                zh.matches("{}").count(),
                en.matches("{}").count(),
                "{name} 两边占位符个数不一样"
            );
        }
    }

    fn field_values(t: &CliText) -> Vec<(&'static str, &'static str)> {
        // 手写一遍字段清单：宏能省掉它，但那样就得引入一个宏只为这个测试，
        // 而且字段一多宏本身也会变得不好读。漏掉一个字段的代价是**少检查一条**，
        // 不是检查错 —— 可以接受。
        vec![
            ("missing_value", t.missing_value),
            ("port_not_number", t.port_not_number),
            ("unknown_option", t.unknown_option),
            ("bad_duration", t.bad_duration),
            ("duration_too_big", t.duration_too_big),
            ("no_command", t.no_command),
            ("hook_needs_agent", t.hook_needs_agent),
            ("agent_needed", t.agent_needed),
            ("monitor_needs_on_off", t.monitor_needs_on_off),
            ("nothing", t.nothing),
            ("only_hooks", t.only_hooks),
            ("too_many_args", t.too_many_args),
            ("app_not_running", t.app_not_running),
            ("no_port_file", t.no_port_file),
            ("no_token_file", t.no_token_file),
            ("read_port_failed", t.read_port_failed),
            ("read_token_failed", t.read_token_failed),
            ("request_timeout", t.request_timeout),
            ("request_failed", t.request_failed),
            ("read_response_failed", t.read_response_failed),
            ("server_error", t.server_error),
            ("report_ok", t.report_ok),
            ("no_change", t.no_change),
            ("status_aggregate", t.status_aggregate),
            ("status_paused", t.status_paused),
            ("status_hint", t.status_hint),
            ("status_sessions", t.status_sessions),
            ("session_agent", t.session_agent),
            ("session_message", t.session_message),
            ("watch_until_hint", t.watch_until_hint),
            ("watch_follow_hint", t.watch_follow_hint),
            ("watch_line", t.watch_line),
            ("watch_timeout", t.watch_timeout),
            ("until_unknown", t.until_unknown),
            ("clear_line", t.clear_line),
            ("clear_removed", t.clear_removed),
            ("clear_absent", t.clear_absent),
            ("monitor_line", t.monitor_line),
            ("monitor_paused", t.monitor_paused),
            ("monitor_resumed", t.monitor_resumed),
            ("install_title", t.install_title),
            ("installed", t.installed),
            ("uninstalled", t.uninstalled),
            ("dry_run_suffix", t.dry_run_suffix),
            ("install_command", t.install_command),
            ("install_config", t.install_config),
            ("install_backup", t.install_backup),
            ("install_added", t.install_added),
            ("install_updated", t.install_updated),
            ("install_skipped", t.install_skipped),
            ("install_removed_handlers", t.install_removed_handlers),
            ("install_no_change", t.install_no_change),
            ("dry_run_footer", t.dry_run_footer),
            ("install_restart", t.install_restart),
            ("path_warning_1", t.path_warning_1),
            ("path_warning_2", t.path_warning_2),
            ("path_warning_3", t.path_warning_3),
            ("uninstall_nothing", t.uninstall_nothing),
            ("run_needs_command", t.run_needs_command),
            ("run_report_failed", t.run_report_failed),
            ("run_spawn_failed", t.run_spawn_failed),
            ("run_wait_failed", t.run_wait_failed),
            ("run_wait_thread_died", t.run_wait_thread_died),
            ("read_stdin_failed", t.read_stdin_failed),
            ("stdin_not_json", t.stdin_not_json),
            ("hook_failed", t.hook_failed),
            ("help", t.help),
        ]
    }

    fn is_cjk(c: char) -> bool {
        matches!(c as u32, 0x3000..=0x303f | 0x4e00..=0x9fff | 0xff00..=0xffef)
    }
}
