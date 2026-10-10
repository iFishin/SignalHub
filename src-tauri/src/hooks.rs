//! 智能体 hook 的安装与卸载。
//!
//! 这个模块被 app 和 CLI 共用，所以不能依赖 Tauri。CLI 通过 `#[path]` 直接
//! 包含它的源码，理由同 config.rs。
//!
//! 三条安全底线，改这里之前先读：
//!
//! 1. **只动 `hooks` 这一个键。** 用户可能在同一个文件里放了 env / theme /
//!    别的工具的 hook，一律原样保留。
//! 2. **解析失败就中止，绝不覆盖。** 把一个手写的 settings.json 弄坏的代价，
//!    远大于功能暂时不可用。所以要写之前先把旧的整份读进来解析一遍。
//! 3. **幂等。** 重复安装不能产生重复条目，否则每次工具调用会跑 N 次上报；
//!    重复卸载也不能报错。

use serde_json::{json, Map, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
}

impl Agent {
    pub fn parse(raw: &str) -> Result<Self, String> {
        // 归一化：大小写、下划线、以及常见的简称都接受
        let key = raw.trim().to_ascii_lowercase().replace('_', "-");
        Ok(match key.as_str() {
            "claude-code" | "claude" | "claudecode" => Agent::ClaudeCode,
            "codex" | "codex-cli" => Agent::Codex,
            "cursor" => Agent::Cursor,
            other => {
                return Err(format!(
                    "不认识的智能体 {other:?}；支持 claude-code / codex / cursor"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
        }
    }

    /// hook 配置相对主目录的路径。
    ///
    /// Codex 也可以用 config.toml 写 hook，但这里刻意用独立的 hooks.json：
    /// 改 TOML 需要保留格式和注释，而 JSON 可以整份解析后重写，安全得多。
    fn config_relative_path(self) -> &'static str {
        match self {
            Agent::ClaudeCode => ".claude/settings.json",
            Agent::Codex => ".codex/hooks.json",
            Agent::Cursor => ".cursor/hooks.json",
        }
    }
}

/// 事件名清单。[`须写 matcher`]: 只有 Claude Code 的 SessionStart 需要
/// —— 不加限制的话，上下文压缩时触发的 SessionStart 会被当成「会话开始」，
/// 在智能体仍工作时把灯报成 idle。
fn events(agent: Agent) -> &'static [(&'static str, Option<&'static str>)] {
    match agent {
        Agent::ClaudeCode => &[
            ("SessionStart", Some("startup|resume")),
            ("UserPromptSubmit", None),
            ("PreToolUse", None),
            ("PostToolUse", None),
            ("PostToolUseFailure", None),
            ("PermissionRequest", None),
            ("PermissionDenied", None),
            ("Notification", None),
            ("SubagentStart", None),
            ("SubagentStop", None),
            ("PreCompact", None),
            ("PostCompact", None),
            ("Stop", None),
            ("StopFailure", None),
            ("TeammateIdle", None),
            ("SessionEnd", None),
        ],
        Agent::Codex => &[
            ("SessionStart", None),
            ("UserPromptSubmit", None),
            ("PreToolUse", None),
            ("PermissionRequest", None),
            ("PostToolUse", None),
            ("PreCompact", None),
            ("PostCompact", None),
            ("SubagentStart", None),
            ("SubagentStop", None),
            ("Stop", None),
            ("Interrupt", None),
            ("SessionEnd", None),
        ],
        Agent::Cursor => &[
            ("sessionStart", None),
            ("beforeSubmitPrompt", None),
            ("preToolUse", None),
            ("postToolUse", None),
            ("postToolUseFailure", None),
            ("beforeShellExecution", None),
            ("afterShellExecution", None),
            ("afterAgentThought", None),
            ("subagentStart", None),
            ("subagentStop", None),
            ("preCompact", None),
            ("afterAgentResponse", None),
            ("stop", None),
            ("sessionEnd", None),
        ],
    }
}

#[derive(Debug)]
pub struct Report {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    /// 安装时：写进配置的那条命令。演练模式下用它来预览。
    pub command: Option<String>,
    /// 本次新写入的事件
    pub added: Vec<String>,
    /// 已经装过、命令也相同，跳过的（幂等）
    pub already: Vec<String>,
    /// 已经装过但写入的命令变了，原地更新的事件
    pub updated: Vec<String>,
    /// 本次移除的事件（整个键被删掉的）
    pub removed: Vec<String>,
    /// 总共摘掉多少个 handler。
    ///
    /// 与 `removed` 不同：一组里可能同时有我们的和别人的 handler，摘掉我们的
    /// 之后整组仍在，此时 `removed` 为空但确实改了文件。判断「有没有改动」
    /// 要看这个，不能看 `removed` 是否为空。
    pub removed_handlers: usize,
    /// true 表示只演算、没有落盘
    pub dry_run: bool,
}

// ---- 对外入口 --------------------------------------------------------------

pub fn config_path(agent: Agent) -> Result<PathBuf, String> {
    crate::config::home_dir()
        .map(|home| home.join(agent.config_relative_path()))
        .ok_or_else(|| "找不到用户主目录".to_string())
}

/// 要写进配置的 hook 命令。
///
/// 优先用裸名字 `signalhub`：它更可移植，用户重装或挪动二进制都不会失效。
/// 但 PATH 里没有时就必须写绝对路径，否则装完不生效——那是最难排查的一类问题。
/// 所以这里是「能用裸名字就用，否则退回当前可执行文件的绝对路径」。
pub fn resolve_hook_command(agent: Agent) -> String {
    let program = match on_path("signalhub") {
        Some(_) => "signalhub".to_string(),
        None => std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "signalhub".to_string()),
    };
    // 含空格的路径必须引起来，否则被 shell 拆成两个参数
    let program = if program.chars().any(char::is_whitespace) {
        format!("\"{program}\"")
    } else {
        program
    };
    format!("{program} hook {}", agent_name_for_cli(agent))
}

fn agent_name_for_cli(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude-code",
        Agent::Codex => "codex",
        Agent::Cursor => "cursor",
    }
}

pub fn install(agent: Agent, dry_run: bool) -> Result<Report, String> {
    let path = config_path(agent)?;
    install_at(agent, &resolve_hook_command(agent), &path, dry_run)
}

pub fn uninstall(agent: Agent, dry_run: bool) -> Result<Report, String> {
    let path = config_path(agent)?;
    uninstall_at(&path, dry_run)
}

// ---- 实现 ------------------------------------------------------------------

fn install_at(agent: Agent, command: &str, path: &Path, dry_run: bool) -> Result<Report, String> {
    let existing: Option<String> = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("读取 {} 失败: {err}", path.display())),
    };

    // 先解析再动手：解析不了就整件事中止，绝不覆盖
    let mut root: Map<String, Value> = match &existing {
        None => Map::new(),
        Some(text) if text.trim().is_empty() => Map::new(),
        Some(text) => serde_json::from_str::<Value>(text)
            .map_err(|err| {
                format!(
                    "{} 不是合法 JSON（{err}），已中止以免破坏你的配置。\
                     请先修好它，或手动添加 hook。",
                    path.display()
                )
            })?
            .as_object()
            .ok_or_else(|| format!("{} 的顶层不是 JSON 对象，已中止", path.display()))?
            .clone(),
    };

    let mut added = Vec::new();
    let mut already = Vec::new();
    let mut updated = Vec::new();

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        return Err(format!(
            "{} 里的 \"hooks\" 不是对象，已中止以免破坏你的配置",
            path.display()
        ));
    }
    let hooks = hooks.as_object_mut().expect("刚确认过是对象");

    for (event, matcher) in events(agent) {
        let groups = hooks.entry((*event).to_string()).or_insert_with(|| json!([]));
        let Some(groups) = groups.as_array_mut() else {
            return Err(format!(
                "{} 里 hooks.{event} 不是数组，已中止以免破坏你的配置",
                path.display()
            ));
        };

        // 已经装过了？装过但命令不同，就原地更新而不是跳过。
        //
        // 这正是「把 signalhub 放进 PATH 后重跑本命令」要走的路径。若在这里
        // 简单地 continue，那条提示就是假的：条目会被判定为「已存在」，
        // 绝对路径永远换不成裸名字。
        if let Some((group, handler)) = locate_our_handler(groups) {
            let slot = match handler {
                Some(index) => &mut groups[group]["hooks"][index],
                None => &mut groups[group],
            };
            let current = slot
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            if current == command {
                already.push((*event).to_string());
            } else {
                slot["command"] = json!(command);
                updated.push((*event).to_string());
            }
            continue;
        }

        groups.push(handler_group(agent, command, *matcher));
        added.push((*event).to_string());
    }

    // Cursor 的 hooks.json 强制要求一个 version 字段
    if agent == Agent::Cursor {
        root.entry("version").or_insert_with(|| json!(1));
    }

    if added.is_empty() && updated.is_empty() {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            command: Some(command.to_string()),
            added,
            already,
            updated,
            removed: Vec::new(),
            removed_handlers: 0,
            dry_run,
        });
    }

    let backup = if dry_run { None } else { write_backup(path, existing.as_deref())? };
    if !dry_run {
        write_json(path, &root)?;
    }

    Ok(Report {
        path: path.to_path_buf(),
        backup,
        command: Some(command.to_string()),
        added,
        already,
        updated,
        removed: Vec::new(),
        removed_handlers: 0,
        dry_run,
    })
}

fn uninstall_at(path: &Path, dry_run: bool) -> Result<Report, String> {
    let empty = |dry_run| Report {
        path: path.to_path_buf(),
        backup: None,
        command: None,
        added: Vec::new(),
        already: Vec::new(),
        updated: Vec::new(),
        removed: Vec::new(),
        removed_handlers: 0,
        dry_run,
    };

    let Ok(text) = fs::read_to_string(path) else {
        // 文件不存在就已经是「卸载完了」，不是错误
        return Ok(empty(dry_run));
    };

    let mut root: Map<String, Value> = serde_json::from_str::<Value>(&text)
        .map_err(|err| {
            format!("{} 不是合法 JSON（{err}），已中止以免破坏你的配置", path.display())
        })?
        .as_object()
        .ok_or_else(|| format!("{} 的顶层不是 JSON 对象，已中止", path.display()))?
        .clone();

    // 用改前/改后的值比较来决定要不要落盘。数计数器容易漏掉某一种改动形式
    // （比如摘掉一组里的 handler 但整组保留），直接比整块值不会漏。
    let before = root.get("hooks").cloned();

    let mut removed = Vec::new();
    let mut removed_handlers = 0usize;

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        hooks.retain(|event, groups| {
            let Some(groups) = groups.as_array_mut() else {
                // 结构不是我们认识的形状，原样留着
                return true;
            };
            let had_any = !groups.is_empty();
            let handlers_before = count_handlers(groups);

            // 逐个 matcher-group 处理：只摘掉自己的 handler。不能因为一组里
            // 同时还有别人的 handler 就把整组删掉。
            let mut kept = Vec::with_capacity(groups.len());
            for group in groups.drain(..) {
                if let Some(group) = prune_group(group) {
                    kept.push(group);
                }
            }
            *groups = kept;
            removed_handlers += handlers_before - count_handlers(groups);

            if !groups.is_empty() {
                return true;
            }
            // 空了就整键移除；只把本来有内容、却被我们清空的键记进 removed，
            // 免得把一个原本就是空数组的键也算成「被移除」
            if had_any {
                removed.push(event.clone());
            }
            false
        });

        if hooks.is_empty() {
            root.remove("hooks");
        }
    }

    if root.get("hooks").cloned() == before {
        return Ok(empty(dry_run));
    }

    let backup = if dry_run { None } else { write_backup(path, Some(&text))? };
    if !dry_run {
        write_json(path, &root)?;
    }

    Ok(Report {
        path: path.to_path_buf(),
        backup,
        command: None,
        added: Vec::new(),
        already: Vec::new(),
        updated: Vec::new(),
        removed,
        removed_handlers,
        dry_run,
    })
}

/// 一组里 handler 的个数。兼容两种形状：嵌套的 `hooks: [...]` 和 Cursor
/// 那种「一组即一个 handler」。
fn count_handlers(groups: &[Value]) -> usize {
    groups
        .iter()
        .map(|group| match group.get("hooks").and_then(Value::as_array) {
            Some(handlers) => handlers.len(),
            None => 1,
        })
        .sum()
}

/// 从一组里摘掉我们的 handler。返回 None 表示这一组空了、应整组删除。
fn prune_group(group: Value) -> Option<Value> {
    let mut group = group;
    if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
        handlers.retain(|handler| !is_our_handler(handler));
        if handlers.is_empty() {
            return None;
        }
        return Some(group);
    }
    // Cursor 的形状：这一组本身就是 handler
    if is_our_handler(&group) {
        return None;
    }
    Some(group)
}

/// 定位已有的属于我们的 handler。返回 `(组下标, 组内 handler 下标)`；
/// 第二个为 None 表示 Cursor 那种「一组即一个 handler」的扁平形状。
///
/// 用下标而不是直接返回 `&mut Value` 是因为「在 `iter_mut()` 循环里 return
/// 引用」在稳定版借用检查下过不了。
fn locate_our_handler(groups: &[Value]) -> Option<(usize, Option<usize>)> {
    for (group_index, group) in groups.iter().enumerate() {
        if is_our_handler(group) {
            return Some((group_index, None));
        }
        let found = group
            .get("hooks")
            .and_then(Value::as_array)
            .and_then(|handlers| handlers.iter().position(is_our_handler));
        if let Some(handler_index) = found {
            return Some((group_index, Some(handler_index)));
        }
    }
    None
}

fn is_our_handler(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_our_hook_command)
}

/// 判断一条命令是不是我们自己写进去的。
///
/// 只认「第一个词是 signalhub 二进制」且「参数里有 hook」。这样既不会误删
/// 用户自己的 hook，也能在二进制路径变化后（重装、挪目录）照样认得出来
/// —— 所以不能拿完整命令串做相等比较。
fn is_our_hook_command(command: &str) -> bool {
    let Some(program) = first_token(command) else {
        return false;
    };
    binary_name(program) == "signalhub"
        && command.split_whitespace().any(|token| token == "hook")
}

/// 取出可执行文件名，两种路径分隔符都认。
///
/// 刻意不用 `Path::file_name()`：它只认当前平台的分隔符，于是 Windows 风格的
/// 路径在 Unix 上解析不出来（反之亦然）。手写或从别处抄来的 hook 命令里
/// 出现另一种平台的写法是可能的，这里不该有平台差异。
fn binary_name(program: &str) -> &str {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// 取命令的第一个词，能正确处理带引号的路径。
fn first_token(command: &str) -> Option<&str> {
    let trimmed = command.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        return rest.split('"').next();
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        return rest.split('\'').next();
    }
    trimmed.split_whitespace().next()
}

/// 构造一个 matcher-group。三种智能体的形状不同：
/// Claude Code / Codex 是 `{ matcher?, hooks: [ { type, command, async } ] }`，
/// Cursor 是 `{ command, timeout }` 直接作为一组。
fn handler_group(agent: Agent, command: &str, matcher: Option<&str>) -> Value {
    match agent {
        // `async: true` 是 type=command 独有的字段。它让 hook 在后台跑、不阻塞
        // 智能体。注意异步 hook 不受 timeout 约束（见 docs/api.md §4.4）。
        Agent::ClaudeCode | Agent::Codex => {
            let mut group = Map::new();
            if let Some(matcher) = matcher {
                group.insert("matcher".into(), json!(matcher));
            }
            group.insert(
                "hooks".into(),
                json!([{ "type": "command", "command": command, "async": true }]),
            );
            Value::Object(group)
        }
        // Cursor 这边 preToolUse 之类是会被等待的，所以超时保护有意义
        Agent::Cursor => json!({ "command": command, "timeout": 5 }),
    }
}

// ---- 落盘 ------------------------------------------------------------------

fn write_backup(path: &Path, existing: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(existing) = existing else {
        // 原本没有这个文件，没什么可备份的
        return Ok(None);
    };
    let backup = PathBuf::from(format!("{}.signalhub-backup", path.display()));
    fs::write(&backup, existing)
        .map_err(|err| format!("写备份 {} 失败: {err}", backup.display()))?;
    Ok(Some(backup))
}

fn write_json(path: &Path, root: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("创建 {} 失败: {err}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(root)
        .map_err(|err| format!("序列化配置失败: {err}"))?;
    text.push('\n');
    fs::write(path, text).map_err(|err| format!("写 {} 失败: {err}", path.display()))
}

/// `signalhub` 是否已在 PATH 里。CLI 用它决定要不要提醒用户「现在写的是绝对路径」。
pub fn signalhub_on_path() -> bool {
    on_path("signalhub").is_some()
}

// ---- PATH 查找（避免为此引入依赖）------------------------------------------

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// 每个用例一个独立目录，避免相互干扰
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = format!(
                "signalhub-hooks-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let dir = std::env::temp_dir().join(unique);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    const CMD: &str = "/opt/signalhub hook claude-code";

    #[test]
    fn install_creates_the_file_and_every_event() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        let report = install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();
        assert_eq!(report.added.len(), events(Agent::ClaudeCode).len());
        assert!(report.already.is_empty());

        let root = read(&path);
        let hooks = root["hooks"].as_object().unwrap();
        assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["command"], CMD);
        assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["type"], "command");
    }

    /// 异步 hook 不阻塞智能体；漏了它每次工具调用都要等一次 HTTP 往返
    #[test]
    fn claude_code_hooks_are_async() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();

        let root = read(&path);
        for groups in root["hooks"].as_object().unwrap().values() {
            assert_eq!(groups[0]["hooks"][0]["async"], true, "{groups}");
        }
    }

    /// 没有 matcher 限制的话，上下文压缩时的 SessionStart 会在智能体仍在工作时
    /// 把灯报成 idle
    #[test]
    fn session_start_is_limited_to_real_starts() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();

        let root = read(&path);
        assert_eq!(root["hooks"]["SessionStart"][0]["matcher"], "startup|resume");
        assert!(root["hooks"]["Stop"][0].get("matcher").is_none());
    }

    /// 这是最重要的一条：settings.json 里往往还有别的东西
    #[test]
    fn install_preserves_unrelated_keys_and_existing_hooks() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(
            &path,
            r#"{
              "env": { "FOO": "bar" },
              "theme": "dark",
              "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "/usr/local/bin/mine.sh" }] }]
              }
            }"#,
        )
        .unwrap();

        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();
        let root = read(&path);

        assert_eq!(root["env"]["FOO"], "bar", "env 必须原样保留");
        assert_eq!(root["theme"], "dark", "theme 必须原样保留");

        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "应在用户原有的组之后追加，而不是替换");
        assert_eq!(stop[0]["hooks"][0]["command"], "/usr/local/bin/mine.sh");
        assert_eq!(stop[1]["hooks"][0]["command"], CMD);
    }

    #[test]
    fn install_is_idempotent() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();
        let first = fs::read_to_string(&path).unwrap();

        let second = install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();
        assert!(second.added.is_empty(), "第二次不该再加条目");
        assert!(second.updated.is_empty(), "命令没变，不该算更新");
        assert_eq!(second.already.len(), events(Agent::ClaudeCode).len());
        assert_eq!(fs::read_to_string(&path).unwrap(), first, "文件内容不应变化");
    }

    /// 重跑安装时如果命令变了（放进 PATH、换了二进制位置），要原地更新。
    /// 否则「放进 PATH 后重新执行本命令」这条提示就是假的。
    #[test]
    fn install_updates_the_command_when_it_changed() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        install_at(Agent::ClaudeCode, "/old/place/signalhub hook claude-code", &path, false)
            .unwrap();

        let report = install_at(Agent::ClaudeCode, "signalhub hook claude-code", &path, false)
            .unwrap();
        assert!(report.added.is_empty(), "条目已存在，不该重复新增");
        assert_eq!(report.updated.len(), events(Agent::ClaudeCode).len());
        assert!(report.already.is_empty());

        let root = read(&path);
        let hooks = root["hooks"].as_object().unwrap();
        assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["command"], "signalhub hook claude-code");
        assert_eq!(hooks["Stop"][0]["hooks"][0]["command"], "signalhub hook claude-code");
    }

    /// 更新时要顺带清掉重复条目：早期版本可能装过两次
    #[test]
    fn install_does_not_add_a_second_copy_alongside_an_updated_one() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        install_at(Agent::ClaudeCode, "/old/signalhub hook claude-code", &path, false).unwrap();
        install_at(Agent::ClaudeCode, "signalhub hook claude-code", &path, false).unwrap();

        let root = read(&path);
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "同一个事件只该有一组是我们的");
    }

    #[test]
    fn cursor_command_is_updated_in_place_too() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");

        install_at(Agent::Cursor, "/old/signalhub hook cursor", &path, false).unwrap();
        let report =
            install_at(Agent::Cursor, "/new/signalhub hook cursor", &path, false).unwrap();

        assert_eq!(report.updated.len(), events(Agent::Cursor).len());
        let root = read(&path);
        assert_eq!(root["hooks"]["preToolUse"][0]["command"], "/new/signalhub hook cursor");
        assert_eq!(root["hooks"]["preToolUse"][0]["timeout"], 5, "其它字段不该被动");
    }

    #[test]
    fn install_backs_up_an_existing_file_once_content_changes() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(&path, "{\"theme\":\"dark\"}").unwrap();

        let report = install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();
        let backup = report.backup.expect("应产生备份");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "{\"theme\":\"dark\"}");
    }

    /// 破损的配置必须原样不动地留着，等人来修
    #[test]
    fn install_aborts_without_touching_unparseable_json() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let broken = "{ this is not json }";
        fs::write(&path, broken).unwrap();

        let err = install_at(Agent::ClaudeCode, CMD, &path, false).unwrap_err();
        assert!(err.contains("不是合法 JSON"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), broken, "内容必须一字未改");
        assert!(!dir.file("settings.json.signalhub-backup").exists());
    }

    #[test]
    fn install_aborts_when_hooks_is_not_an_object() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(&path, r#"{"hooks": ["nonsense"]}"#).unwrap();

        assert!(install_at(Agent::ClaudeCode, CMD, &path, false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"hooks": ["nonsense"]}"#);
    }

    #[test]
    fn dry_run_changes_nothing() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        let report = install_at(Agent::ClaudeCode, CMD, &path, true).unwrap();
        assert!(report.dry_run);
        assert!(!report.added.is_empty(), "演算也应报告将要做什么");
        assert!(report.backup.is_none());
        assert!(!path.exists(), "dry-run 不该创建文件");
    }

    #[test]
    fn uninstall_removes_only_our_handlers() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(
            &path,
            r#"{
              "theme": "dark",
              "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "/usr/local/bin/mine.sh" }] }]
              }
            }"#,
        )
        .unwrap();
        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();

        let report = uninstall_at(&path, false).unwrap();
        assert!(!report.removed.is_empty());

        let root = read(&path);
        assert_eq!(root["theme"], "dark", "别人的键不能动");
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "只该剩用户自己那一组");
        assert_eq!(stop[0]["hooks"][0]["command"], "/usr/local/bin/mine.sh");
        // 我们装的那 16 个事件里，除 Stop 外其余都该被整个删掉
        assert!(root["hooks"].get("PreToolUse").is_none());
    }

    /// 同一组里既有用户的 handler 也有我们的 —— 只摘掉自己的，整组保留。
    /// 这种情况 `removed`（事件名）会是空的，但文件必须被改写，所以判断
    /// 「有没有改动」只能看 removed_handlers。
    #[test]
    fn uninstall_keeps_a_group_that_still_has_foreign_handlers() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(
            &path,
            format!(
                r#"{{"hooks":{{"Stop":[{{"hooks":[
                    {{"type":"command","command":"/usr/local/bin/mine.sh"}},
                    {{"type":"command","command":"{CMD}"}}
                ]}}]}}}}"#
            ),
        )
        .unwrap();

        let report = uninstall_at(&path, false).unwrap();
        assert_eq!(report.removed_handlers, 1);
        assert!(report.removed.is_empty(), "整组还在，只是少了一个 handler");

        let root = read(&path);
        let hooks = root["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 1, "不该牵连其它事件");
        let stop = hooks["Stop"].as_array().unwrap();
        assert_eq!(stop[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(stop[0]["hooks"][0]["command"], "/usr/local/bin/mine.sh");
    }

    #[test]
    fn uninstall_drops_the_hooks_key_when_it_becomes_empty() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();

        uninstall_at(&path, false).unwrap();
        let root = read(&path);
        assert!(root.get("hooks").is_none(), "hooks 空了就该整键移除");
        assert_eq!(root["theme"], "dark");
    }

    #[test]
    fn uninstall_on_a_missing_file_is_not_an_error() {
        let dir = TempDir::new();
        let report = uninstall_at(&dir.file("nope.json"), false).unwrap();
        assert!(report.removed.is_empty());
    }

    #[test]
    fn uninstall_is_idempotent() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();

        assert!(!uninstall_at(&path, false).unwrap().removed.is_empty());
        assert!(uninstall_at(&path, false).unwrap().removed.is_empty());
    }

    #[test]
    fn uninstall_recognizes_a_command_with_a_different_binary_path() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        // 用另一个路径装，卸载时应照样认得出来
        install_at(Agent::ClaudeCode, "/home/someone/bin/signalhub hook claude-code", &path, false)
            .unwrap();
        uninstall_at(&path, false).unwrap();
        assert!(read(&path).get("hooks").is_none());
    }

    // ---- Cursor 的形状不同 ----

    #[test]
    fn cursor_gets_version_and_a_flat_handler_shape() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");

        install_at(Agent::Cursor, "/opt/signalhub hook cursor", &path, false).unwrap();
        let root = read(&path);

        assert_eq!(root["version"], 1, "Cursor 要求 version 字段");
        assert_eq!(root["hooks"]["preToolUse"][0]["command"], "/opt/signalhub hook cursor");
        assert_eq!(root["hooks"]["preToolUse"][0]["timeout"], 5);
        // Cursor 不需要 "async"，也不该出现嵌套的 hooks 数组
        assert!(root["hooks"]["preToolUse"][0].get("hooks").is_none());
    }

    #[test]
    fn cursor_install_preserves_an_existing_version() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");
        fs::write(&path, r#"{"version":1}"#).unwrap();

        install_at(Agent::Cursor, "/opt/signalhub hook cursor", &path, false).unwrap();
        assert_eq!(read(&path)["version"], 1);
    }

    #[test]
    fn codex_events_are_pascal_case_and_nested() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");

        install_at(Agent::Codex, "/opt/signalhub hook codex", &path, false).unwrap();
        let root = read(&path);
        assert_eq!(root["hooks"]["PreToolUse"][0]["hooks"][0]["async"], true);
        assert_eq!(root["hooks"]["Interrupt"][0]["hooks"][0]["command"], "/opt/signalhub hook codex");
    }

    // ---- 命令识别 ----

    #[test]
    fn our_commands_are_recognized() {
        assert!(is_our_hook_command("signalhub hook claude-code"));
        assert!(is_our_hook_command("/opt/bin/signalhub hook codex"));
        // Windows
        assert!(is_our_hook_command(r"C:\tools\signalhub.exe hook cursor"));
        // 路径含空格时必须引起来 —— resolve_hook_command 就是这么写的
        assert!(is_our_hook_command(r#""/Users/me/a b/signalhub" hook claude-code"#));
    }

    /// 未加引号的含空格路径不是一个有效命令（shell 会把它拆成两个参数），
    /// 所以识别不出来是对的。这条留着是为了说明「不是漏判」。
    #[test]
    fn an_unquoted_path_with_spaces_is_not_recognized() {
        assert!(!is_our_hook_command("/Users/me/a b/signalhub hook cursor"));
    }

    #[test]
    fn foreign_commands_are_not_recognized() {
        for command in [
            "/usr/local/bin/mine.sh",
            "signalhub status",              // 不是 hook 子命令
            "mytool hook claude-code",       // 别的二进制
            "signalhub-hooks hook claude",   // 名字只是前缀相同
            "",
        ] {
            assert!(
                !is_our_hook_command(command),
                "{command:?} 不该被当成我们的 hook"
            );
        }
    }

    #[test]
    fn agent_names_parse_with_aliases() {
        assert_eq!(Agent::parse("claude-code").unwrap(), Agent::ClaudeCode);
        assert_eq!(Agent::parse("Claude_Code").unwrap(), Agent::ClaudeCode);
        assert_eq!(Agent::parse(" claude ").unwrap(), Agent::ClaudeCode);
        assert_eq!(Agent::parse("codex").unwrap(), Agent::Codex);
        assert_eq!(Agent::parse("CURSOR").unwrap(), Agent::Cursor);
        assert!(Agent::parse("emacs").is_err());
    }

    #[test]
    fn backup_is_not_written_when_there_is_nothing_to_back_up() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let report = install_at(Agent::ClaudeCode, CMD, &path, false).unwrap();
        assert!(report.backup.is_none(), "新建文件没什么可备份的");
    }
}
