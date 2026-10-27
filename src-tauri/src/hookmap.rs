//! hook payload → 状态 的映射表，以及从 payload 里取字段的宽容解析。
//!
//! **为什么单独一个文件**：这张表有两个读者 —— CLI 的 `signalhub hook`（命令式接入）
//! 和应用的 `POST /v1/hooks/{agent}`（HTTP 式接入，见 `server.rs`）。抄一份到两边
//! 就是两份真值，而它们漂移的表现是**同一个事件在两种接入方式下亮不同的灯** ——
//! 最难查、也最没道理的一类 bug。
//!
//! 这里只放**与语言无关**的东西：状态名、字段名、动词分类。文案（「执行 / 读 / 找」）
//! 留在各自的文案表里（CLI 的 `cli_text.rs`、应用的 `lang.rs`）—— 它们的使用者不同。

use serde_json::Value;
/// 这一行本来就窄，再长也会被 CSS 截断；在源头截短是为了让**消息流**和通知正文
/// 也别被一条长命令撑开。
pub const TARGET_MAX: usize = 32;
pub const TOOL_NAME_MAX: usize = 24;

/// 工具动词的**分类**，不是文案。
///
/// 分类放共享模块、词放各自的文案表，是因为这句话要进灯的状态、系统通知和消息流，
/// 而那三处都得说用户的语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// 跑命令
    Exec,
    /// 改文件
    Edit,
    /// 读文件
    Read,
    /// 搜索 / 列目录
    Search,
    /// 上网
    Net,
}

/// 工具名 → 动词分类。**用精确匹配而不是包含关键字**：`TodoWrite` 里有 "write"、
/// `ExitPlanMode` 里有 "plan"，按关键字来会一路认错，而认错比认不出来更糟。
///
/// 表里的名字是按各家文档和实际事件收的；漏了哪个只会让那个工具显示成原名。
pub fn tool_verb(name: &str) -> Option<Verb> {
    // `run-terminal-cmd` / `Run Terminal Cmd` 都归一到下划线小写
    let normalized = name.to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "bash" | "bashoutput" | "killshell" | "killbash" | "shell" | "localshell" | "exec"
        | "run_terminal_cmd" | "execute_command" | "run_command" | "run_shell_command" => {
            Some(Verb::Exec)
        }
        "edit" | "write" | "multiedit" | "notebookedit" | "str_replace" | "str_replace_editor"
        | "apply_patch" | "edit_file" | "create_file" | "write_file" | "replace" => {
            Some(Verb::Edit)
        }
        "read" | "read_file" | "view" | "cat" | "readfile" => Some(Verb::Read),
        "grep" | "grep_search" | "glob" | "search" | "search_files" | "search_file_content"
        | "codebase_search" | "list_dir" | "ls" | "find" => Some(Verb::Search),
        "webfetch" | "web_fetch" | "websearch" | "web_search" | "fetch" => Some(Verb::Net),
        _ => None,
    }
}

/// 从 `tool_input` 里挑一个能代表「在干嘛」的短词。按可信度排：路径和搜索词最稳定，
/// 命令要清洗，URL 要拆主机名。
pub fn tool_target(input: &Value) -> Option<String> {
    // 路径：只取末段 —— 整条路径又长又会把牌上别的信息挤掉，而末段才是有区分度的
    for key in ["file_path", "filePath", "path", "target_file", "notebook_path"] {
        if let Some(raw) = input[key].as_str() {
            let name = crate::config::project_name(raw.trim());
            if !name.is_empty() && name != "/" && name != "\\" {
                return Some(clip(&name, TARGET_MAX));
            }
        }
    }
    for key in ["pattern", "query", "regex"] {
        if let Some(raw) = input[key].as_str() {
            let text = raw.trim();
            if !text.is_empty() {
                return Some(clip(text, TARGET_MAX));
            }
        }
    }
    for key in ["command", "cmd"] {
        if let Some(word) = input[key].as_str().and_then(first_word) {
            return Some(clip(&word, TARGET_MAX));
        }
    }
    input["url"].as_str().and_then(host_of).map(|host| clip(&host, TARGET_MAX))
}

/// 一条命令的「第一个实词」：跳过 `A=1` 这类环境变量赋值，只留可执行文件名。
///
/// 这是那条**不把整条命令打出去**的规则的落点 —— `API_KEY=xxx curl ...` 里
/// 有意义的只是 `curl`。
pub fn first_word(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .find(|word| !word.contains('='))
        .map(crate::config::project_name)
        .filter(|word| !word.is_empty())
}

/// URL 的主机名。`https://docs.rs/tauri/latest` → `docs.rs`。
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    (!host.is_empty()).then(|| host.to_string())
}

/// 截断。**按字符而不是字节** —— 按字节切会把一个汉字切成半个，那是无效 UTF-8。
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

pub enum Action {
    Report(&'static str),
    Clear,
}

pub fn map_event(agent: &str, payload: &Value) -> Option<Action> {
    let event = event_name(payload)?;
    match agent {
        "claude-code" => map_claude(event, payload),
        // CSC 的载荷与事件名和 Claude Code 完全同构（issue #22），共用一份映射
        "csc" => map_claude(event, payload),
        "codex" => map_codex(event),
        "cursor" => map_cursor(event, payload),
        "gemini" => map_gemini(event),
        "goose" => map_goose(event),
        _ => None,
    }
}

/// payload 里「事件叫什么」的字段名。
///
/// 只有 **Goose 用 `event`**，其余几家都是 `hook_event_name`。这里按顺序试，
/// 而不是按 agent 分支：字段名是 payload 的属性，不是 agent 的属性，多试一个
/// 不会有副作用（同一个 payload 里不会两个都有且冲突）。
pub fn event_name(payload: &Value) -> Option<&str> {
    ["hook_event_name", "event"]
        .iter()
        .find_map(|key| payload[*key].as_str())
}

/// Gemini CLI（docs/hooks/reference.md）。PascalCase，语义与 Claude Code 那一族一致。
///
/// **它没有「需要授权」事件** —— 权限点是靠 hook 的返回值决定的，而上报型 hook
/// 不返回值。所以红色只能靠 `Notification` 兜：那个事件的确切触发条件官方参考
/// 里没写，这里按「通知 = 有事要你看一眼」映射成 `attention`（最轻的那一档）。
/// 映射错了的表现是多几盏黄灯，不会漏掉真正的异常。
pub fn map_gemini(event: &str) -> Option<Action> {
    Some(match event {
        "SessionStart" => Action::Report("idle"),
        "BeforeAgent" | "PreCompress" => Action::Report("thinking"),
        "BeforeTool" | "AfterTool" => Action::Report("working"),
        "AfterAgent" => Action::Report("done"),
        "Notification" => Action::Report("attention"),
        "SessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// Goose 的插件 hook。事件名与 Claude Code 同构。
///
/// 同样**没有 `PermissionRequest`**：Goose 的授权也是靠 PreToolUse 的返回值
/// （`decision: "block"`）实现的，上报型 hook 不返回值。
pub fn map_goose(event: &str) -> Option<Action> {
    Some(match event {
        "SessionStart" => Action::Report("idle"),
        "UserPromptSubmit" => Action::Report("thinking"),
        "PreToolUse" | "PostToolUse" => Action::Report("working"),
        "PostToolUseFailure" => Action::Report("attention"),
        "Stop" => Action::Report("done"),
        "SessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// 映射表见 docs/api.md §4.2，含**明确忽略**的事件清单 ——
/// TaskCreated / FileChanged 这类事件频繁且与「一轮工作」无关，误用会让灯球乱闪。
pub fn map_claude(event: &str, payload: &Value) -> Option<Action> {
    Some(match event {
        "SessionStart" => {
            // 只有 startup / resume 才该报 idle。compact / fork / clear 是会话中途
            // 发生的，此时智能体仍在工作，报 idle 是错的。
            // 字段名在不同版本上可能不同，所以只排除明确知道不该报的那几个，
            // 其余一律按 idle 处理 —— 宁可多报也不要在版本差异下静默丢事件。
            match payload["source"].as_str() {
                Some("compact" | "fork" | "clear") => return None,
                _ => Action::Report("idle"),
            }
        }
        "UserPromptSubmit" | "SubagentStart" | "PreCompact" => Action::Report("thinking"),
        "PreToolUse" | "PostToolUse" | "PermissionDenied" | "PostCompact" => {
            Action::Report("working")
        }
        "PostToolUseFailure" => Action::Report("attention"),
        "PermissionRequest" => Action::Report("permission"),
        "Notification" => match payload["notification_type"].as_str().unwrap_or("") {
            "permission_prompt" => Action::Report("permission"),
            "agent_needs_input" | "idle_prompt" => Action::Report("attention"),
            "agent_completed" => Action::Report("done"),
            _ => return None,
        },
        "Stop" => Action::Report("done"),
        "StopFailure" => Action::Report("error"),
        "TeammateIdle" => Action::Report("attention"),
        "SessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// Codex 用 PascalCase，事件集与 Claude Code 同构（docs/api.md §4.6）。
pub fn map_codex(event: &str) -> Option<Action> {
    Some(match event {
        "SessionStart" => Action::Report("idle"),
        "UserPromptSubmit" | "SubagentStart" | "PreCompact" => Action::Report("thinking"),
        "PreToolUse" | "PostToolUse" | "PostCompact" => Action::Report("working"),
        "PermissionRequest" => Action::Report("permission"),
        "Stop" => Action::Report("done"),
        "Interrupt" | "SessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// Cursor 用 camelCase（docs/api.md §4.7）。
/// 注意它没有可映射的「需要授权」事件 —— 那要靠 hook 返回 permission 字段实现，
/// 上报型 hook 不返回该字段，所以这里确实没有对应项，不是漏了。
pub fn map_cursor(event: &str, payload: &Value) -> Option<Action> {
    Some(match event {
        "sessionStart" => Action::Report("idle"),
        "beforeSubmitPrompt" | "afterAgentThought" | "subagentStart" | "preCompact" => {
            Action::Report("thinking")
        }
        "preToolUse" | "postToolUse" | "beforeShellExecution" | "afterShellExecution" => {
            Action::Report("working")
        }
        "postToolUseFailure" => Action::Report("attention"),
        "afterAgentResponse" => Action::Report("done"),
        // 三家里只有 Cursor 的结束事件带 status，能直接区分正常结束和异常中断
        "stop" => match payload["status"].as_str() {
            Some("error" | "aborted") => Action::Report("error"),
            _ => Action::Report("done"),
        },
        "sessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// 三家用的字段名不同，做宽容匹配。Codex 老式 `notify` 用 kebab 的 `thread-id`。
pub fn session_id(payload: &Value) -> Option<String> {
    ["session_id", "conversation_id", "thread_id", "thread-id"]
        .iter()
        .find_map(|key| payload[*key].as_str())
        .map(str::to_string)
}

/// Claude Code / Codex 用 `cwd`；Cursor 给的是 `workspace_roots` 数组。
/// 各家 payload 里「当前工作目录」的字段名不同，做宽容匹配：
/// Claude Code / Codex / Gemini / Cursor 用 `cwd`，**Goose 用 `working_dir`**，
/// Cursor 的项目级配置用 `workspace_roots` 数组。
pub fn cwd_of(payload: &Value) -> Option<String> {
    payload["cwd"]
        .as_str()
        .or_else(|| payload["working_dir"].as_str())
        .or_else(|| payload["workspace_roots"].as_array()?.first()?.as_str())
        .map(str::to_string)
}

/// 「这次调用到底要干什么」—— 给**灯上那个待批准气泡**看的一行。
///
/// 和 `tool_target` 分开：那个是牌面上的短词（`执行 npm`），要短到不挤掉别的信息；
/// 这个要**看得全**，因为用户是拿它来决定放不放行的。返回 `(一行文字, 有没有被截断)`。
///
/// **被截断就不给「允许」**（见 `approvals::Pending::allowable`）：看不全的东西不该在
/// 灯上一键放行，那种情况下只能拒绝、或者回终端自己看。
///
/// 这里的东西**只进本地界面**：状态上报用的是 `tool_target`（短词），通知和出站钩子
/// 都不会带上它 —— 命令里可能有 `API_KEY=…` 这类东西，而那是「只发状态」那条红线的
/// 范围。终端里的确认框本来也把这些打给用户看，所以本地面板里看得见不算越界。
pub const DETAIL_MAX: usize = 240;

pub fn approval_detail(tool: &str, input: &Value) -> (String, bool) {
    let raw = match tool_verb(tool) {
        // 命令：整条打出来 —— `API_KEY=xxx curl …` 里要决定的就是这一整行
        Some(Verb::Exec) => ["command", "cmd"]
            .iter()
            .find_map(|key| input[*key].as_str())
            .map(str::to_string),
        // 文件类：给路径就够了（`Write` 的 content 可能几万行）
        Some(Verb::Edit | Verb::Read) => ["file_path", "filePath", "path", "notebook_path"]
            .iter()
            .find_map(|key| input[*key].as_str())
            .map(str::to_string),
        // 搜索 / 上网：查询词和 URL
        Some(Verb::Search | Verb::Net) => ["pattern", "query", "url"]
            .iter()
            .find_map(|key| input[*key].as_str())
            .map(str::to_string),
        // 认不出来的工具（含 MCP）：整个输入 —— 短的就够看，长的会被截断
        None => None,
    };

    let text = raw.unwrap_or_else(|| input.to_string());
    let text = text.trim();
    let clipped = clip(text, DETAIL_MAX);
    let truncated = clipped != text;
    (clipped, truncated)
}

#[cfg(test)]
mod approval_detail_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_command_is_shown_whole() {
        let input = json!({ "command": "rm -rf node_modules", "description": "清理" });
        assert_eq!(
            approval_detail("Bash", &input),
            ("rm -rf node_modules".to_string(), false)
        );
    }

    /// 长命令要**说出来**它被截断了 —— 那一句就是「这条不能在灯上放行」的依据。
    #[test]
    fn a_long_command_is_flagged_as_truncated() {
        let long = "echo ".to_string() + &"x".repeat(DETAIL_MAX * 2);
        let (shown, truncated) = approval_detail("Bash", &json!({ "command": long }));
        assert!(truncated);
        assert!(shown.chars().count() <= DETAIL_MAX + 1, "{shown}");
    }

    #[test]
    fn file_tools_show_the_path_not_the_contents() {
        let input = json!({ "file_path": "/proj/src/main.rs", "content": "x".repeat(9999) });
        assert_eq!(
            approval_detail("Write", &input),
            ("/proj/src/main.rs".to_string(), false)
        );
    }

    /// 认不出来的工具：输入小就给整个 JSON（因此**可以**放行），大就截断（不能）。
    #[test]
    fn an_unknown_tool_falls_back_to_the_raw_input() {
        let small = json!({ "memory": "hello" });
        assert_eq!(
            approval_detail("mcp__memory__create", &small),
            (small.to_string(), false)
        );
        let big = json!({ "blob": "y".repeat(DETAIL_MAX * 3) });
        assert!(approval_detail("mcp__memory__create", &big).1);
    }
}
