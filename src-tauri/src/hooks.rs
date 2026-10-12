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
    Gemini,
    Goose,
}

impl Agent {
    pub fn parse(raw: &str) -> Result<Self, String> {
        // 归一化：大小写、下划线、以及常见的简称都接受
        let key = raw.trim().to_ascii_lowercase().replace('_', "-");
        Ok(match key.as_str() {
            "claude-code" | "claude" | "claudecode" => Agent::ClaudeCode,
            "codex" | "codex-cli" => Agent::Codex,
            "cursor" => Agent::Cursor,
            "gemini" | "gemini-cli" => Agent::Gemini,
            "goose" => Agent::Goose,
            other => {
                return Err(format!(
                    "不认识的智能体 {other:?}；支持 {}",
                    ALL_NAMES.join(" / ")
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
            Agent::Gemini => "Gemini CLI",
            Agent::Goose => "Goose",
        }
    }

    /// hook 配置相对主目录的路径。
    ///
    /// Codex 也可以用 config.toml 写 hook，但这里刻意用独立的 hooks.json：
    /// 改 TOML 需要保留格式和注释，而 JSON 可以整份解析后重写，安全得多。
    ///
    /// Goose 是**插件目录**而不是共享配置：我们要建
    /// `~/.agents/plugins/signalhub/`，里面放 `plugin.json`（清单）和
    /// `hooks/hooks.json`（hook 定义）。这条路比改共享配置更安全 ——
    /// 那两个文件都是我们自己建的，不存在「解析不了用户手写的内容」这回事。
    fn config_relative_path(self) -> &'static str {
        match self {
            Agent::ClaudeCode => ".claude/settings.json",
            Agent::Codex => ".codex/hooks.json",
            Agent::Cursor => ".cursor/hooks.json",
            Agent::Gemini => ".gemini/settings.json",
            Agent::Goose => ".agents/plugins/signalhub/hooks/hooks.json",
        }
    }
}

/// 全部智能体，按加入顺序。`parse` 的报错信息和 CLI 的 help 都用它，
/// 免得新增一个之后忘改其中一处、报错里少列一个。
pub const ALL_NAMES: [&str; 5] = ["claude-code", "codex", "cursor", "gemini", "goose"];

impl Agent {
    /// 这个智能体在机器上留下的**目录**痕迹，用来判断它装没装。
    ///
    /// 与 `config_relative_path` 分开：那个是我们**写** hook 的地方，这个是我们
    /// **看**它在不在的地方，两者不总是同一个 —— Goose 的 hook 装在我们自己的
    /// 插件目录里，而它本体的痕迹在 `~/.config/goose`。
    fn marker_dir(self) -> &'static str {
        match self {
            Agent::ClaudeCode => ".claude",
            Agent::Codex => ".codex",
            Agent::Cursor => ".cursor",
            Agent::Gemini => ".gemini",
            Agent::Goose => ".config/goose",
        }
    }

    /// PATH 上的命令名。Windows 上会自动补 `.exe`（见 `candidates`）。
    fn command_names(self) -> &'static [&'static str] {
        match self {
            Agent::ClaudeCode => &["claude"],
            Agent::Codex => &["codex"],
            // Cursor 的命令行工具新旧两个名字都用过
            Agent::Cursor => &["cursor-agent", "cursor"],
            Agent::Gemini => &["gemini"],
            Agent::Goose => &["goose"],
        }
    }
}

/// 这台机器上有没有这个智能体。
///
/// **看的是痕迹，不是注册表** —— 没有哪个智能体会把自己登记到系统里，能用的信号
/// 只有两个：它的配置目录在不在、它的命令在不在 PATH 上。
///
/// 判错的代价很小：检测只决定界面上「检测到」那一行怎么显示，装的时候照装不误。
/// 所以宁可宽松（配置目录在就算），也不要因为路径记错而把一个装了的智能体报成没装。
pub fn detect(agent: Agent) -> bool {
    let dir_exists = crate::config::home_dir()
        .map(|home| home.join(agent.marker_dir()).is_dir())
        .unwrap_or(false);
    dir_exists
        || agent
            .command_names()
            .iter()
            .any(|name| on_path(name).is_some())
}

/// 我们给它装过 hook 没有。
///
/// 判据是「配置文件里有没有我们写的 handler」，**不是「配置文件在不在」** ——
/// 那个文件本来就可能存在（用户自己配的别的东西），光看它会把没装的说成装了。
pub fn is_installed(agent: Agent) -> bool {
    // Goose 是插件：整个目录都是我们建的，看它在不在就够了
    if let Some(dir) = plugin_dir(agent) {
        return dir.is_dir();
    }

    let Ok(path) = config_path(agent) else {
        return false;
    };
    fs::read_to_string(&path)
        .map(|text| config_has_our_hooks(&text))
        .unwrap_or(false)
}

/// 一份配置内容里有没有我们写的 handler。
///
/// 抽成纯函数是为了能单测：`is_installed` 自己依赖 `$HOME`，而测试里改 `HOME`
/// 是进程级的、和并行跑的其它用例会打架。
fn config_has_our_hooks(text: &str) -> bool {
    let Ok(root) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    root.get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| {
            hooks.values().any(|groups| {
                groups
                    .as_array()
                    .is_some_and(|groups| locate_our_handler(groups).is_some())
            })
        })
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
        // Gemini CLI：PascalCase，和 Claude Code 同构（docs/hooks/reference.md）。
        //
        // **只装用得上的那几个。** BeforeModel / AfterModel / BeforeToolSelection
        // 是每次模型调用都触发的，装上去纯属噪音 —— 灯会一直在闪而信息量为零。
        // 同一个理由，Claude Code 那边也刻意忽略了一长串事件。
        Agent::Gemini => &[
            ("SessionStart", None),
            ("BeforeAgent", None),
            ("BeforeTool", None),
            ("AfterTool", None),
            ("Notification", None),
            ("PreCompress", None),
            ("AfterAgent", None),
            ("SessionEnd", None),
        ],
        // Goose 的**插件** hook。事件名和 payload 字段都自成一格：事件名在
        // `event` 而不是 `hook_event_name`，工作目录在 `working_dir` 而不是
        // `cwd`（见 CLI 里的 map_goose）。
        //
        // BeforeReadFile / AfterFileEdit / Before|AfterShellExecution 刻意不装：
        // 读文件是高频事件，装了灯会一直闪。Goose 也明确说了它**不发**
        // SubagentStart / SubagentStop，所以那两个不用考虑。
        Agent::Goose => &[
            ("SessionStart", None),
            ("UserPromptSubmit", None),
            ("PreToolUse", None),
            ("PostToolUse", None),
            ("PostToolUseFailure", None),
            ("Stop", None),
            ("SessionEnd", None),
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
    /// Goose 专用的插件清单路径。其余智能体没有这个文件。
    pub manifest: Option<PathBuf>,
}

// ---- 对外入口 --------------------------------------------------------------

pub fn config_path(agent: Agent) -> Result<PathBuf, String> {
    crate::config::home_dir()
        .map(|home| home.join(agent.config_relative_path()))
        .ok_or_else(|| "找不到用户主目录".to_string())
}

/// 我们自己的 CLI 文件名。装了包的用户手上**没有** `signalhub` 这个命令，只有
/// 跟着 bundle 一起进去的这个文件。
const CLI_BINARY: &str = if cfg!(windows) {
    "signalhub.exe"
} else {
    "signalhub"
};

/// 要写进配置的 hook 命令。
///
/// 三条路，按优先级：
///
/// 1. **PATH 上的裸名字 `signalhub`** —— 最可移植，用户重装或挪动 app 都不会失效，
///    所以只要它在就用它。
/// 2. **和当前可执行文件同目录的那个 CLI** —— 装了包的用户走这条：sidecar 就放在
///    主二进制旁边（macOS 是 `Contents/MacOS/`，Windows 是 exe 旁边）。`tauri dev`
///    也走这条，因为 CLI 和 app 都落在 `target/debug/` 里。
/// 3. 都没有就退回裸名字 —— 至少是一条读得懂的命令。
///
/// **绝不能用 `current_exe()` 兜底**（这里原来就是那么写的）：那是 GUI 二进制，它
/// 不认 `hook` 参数，而单实例插件会把它变成「把已经开着的窗口叫到前面」—— 不报
/// 状态，也不报错。
pub fn resolve_hook_command(agent: Agent) -> String {
    let program = match on_path("signalhub") {
        Some(_) => "signalhub".to_string(),
        None => bundled_cli().unwrap_or_else(|| "signalhub".to_string()),
    };
    // 含空格的路径必须引起来，否则被 shell 拆成两个参数
    let program = if program.chars().any(char::is_whitespace) {
        format!("\"{program}\"")
    } else {
        program
    };
    format!("{program} hook {}", agent_name_for_cli(agent))
}

/// 和当前可执行文件放在一起的那个 CLI，在的话给出它的绝对路径。
///
/// 找不到就返回 `None`，由调用方决定怎么兜底 —— 这个函数只回答「在不在」。
fn bundled_cli() -> Option<String> {
    let sibling = cli_beside(&std::env::current_exe().ok()?)?;
    sibling.exists().then(|| sibling.display().to_string())
}

/// 给定一个可执行文件路径，算出「同目录的 CLI」应该在哪。
///
/// 抽成纯函数是为了能单测 —— `bundled_cli` 自己依赖 `current_exe()`，测试里改不了
/// 当前进程的可执行文件路径。
fn cli_beside(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    Some(exe.parent()?.join(CLI_BINARY))
}

fn agent_name_for_cli(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude-code",
        Agent::Codex => "codex",
        Agent::Cursor => "cursor",
        Agent::Gemini => "gemini",
        Agent::Goose => "goose",
    }
}

/// Goose 插件的目录 —— 就是我们建的那个，卸载时整份删掉。
///
/// 只有 Goose 是「自己的目录」，其余几家都是在用户的共享配置里插一条。
/// 所以这里返回 `Option`：`None` 表示这个智能体没有自己的目录可清理。
fn plugin_dir(agent: Agent) -> Option<PathBuf> {
    let path = config_path(agent).ok()?;
    match agent {
        // hooks.json → hooks/ → signalhub/
        Agent::Goose => Some(path.parent()?.parent()?.to_path_buf()),
        _ => None,
    }
}

/// Goose 的插件清单。内容固定，没有任何来自用户的输入。
const GOOSE_MANIFEST: &str = r#"{
  "name": "signalhub",
  "version": "0.1.0",
  "description": "Report this session's status to SignalHub"
}
"#;

pub fn install(agent: Agent, dry_run: bool) -> Result<Report, String> {
    let path = config_path(agent)?;
    let mut report = install_at(agent, &resolve_hook_command(agent), &path, dry_run)?;

    // Goose 的 hook 必须放在一个**插件**里，光有 hooks.json 它不会加载 ——
    // 清单是插件的身份证。演练模式不落盘，所以这里也跳过。
    if agent == Agent::Goose && !dry_run {
        let Some(dir) = plugin_dir(agent) else {
            return Err("算不出 Goose 插件目录".to_string());
        };
        let manifest = dir.join("plugin.json");
        // 原子写 + 建目录：`atomic_write` 要往这个目录里放临时文件，目录不在会直接失败。
        // 清单是插件的身份证，写坏了 Goose 会静默不加载这个插件。
        if let Err(err) = fs::create_dir_all(&dir) {
            return Err(format!("创建插件目录 {} 失败: {err}", dir.display()));
        }
        if let Err(err) = crate::config::atomic_write(&manifest, GOOSE_MANIFEST, None) {
            return Err(format!("写插件清单 {} 失败: {err}", manifest.display()));
        }
        report.manifest = Some(manifest);
    }
    Ok(report)
}

pub fn uninstall(agent: Agent, dry_run: bool) -> Result<Report, String> {
    let path = config_path(agent)?;
    let report = uninstall_at(&path, dry_run)?;

    // Goose：整个插件目录都是我们建的（名字就叫 signalhub），所以直接删掉。
    // 留着的话，一个空的插件目录仍会被 goose 加载，只是什么都不做 ——
    // 「卸载了但还在那儿」比彻底删掉更难解释。
    if agent == Agent::Goose
        && !dry_run
        && let Some(dir) = plugin_dir(agent)
        && dir.is_dir()
    {
        fs::remove_dir_all(&dir)
            .map_err(|err| format!("删除插件目录 {} 失败: {err}", dir.display()))?;
    }
    Ok(report)
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
            manifest: None,
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
        manifest: None,
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
        manifest: None,
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
        manifest: None,
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

/// 构造一个 matcher-group。三种形状：
/// Claude Code / Codex 是 `{ matcher?, hooks: [ { type, command, async } ] }`，
/// Gemini / Goose 同构但**没有 `async`**、改带 `timeout`，
/// Cursor 是 `{ command, timeout }` 直接作为一组。
fn handler_group(agent: Agent, command: &str, matcher: Option<&str>) -> Value {
    match agent {
        // Cursor 这边 preToolUse 之类是会被等待的，所以超时保护有意义
        Agent::Cursor => json!({ "command": command, "timeout": 5 }),
        // Gemini / Goose 也会**同步等待** hook（两家都没有 async 开关），所以给
        // 一个明确的超时，免得一个挂住的 hook 把智能体卡在那儿。
        //
        // `async: true` 是 Claude Code 的扩展字段，这两家的文档里都没有 ——
        // 不写。Copilot 的文档甚至明确说了「inline 的 hooks 里有一项校验不过，
        // 整个 hooks 字段作废」，所以对不认识的字段宁可保守。
        Agent::Gemini | Agent::Goose => {
            let mut group = Map::new();
            if let Some(matcher) = matcher {
                group.insert("matcher".into(), json!(matcher));
            }
            group.insert(
                "hooks".into(),
                json!([{
                    "type": "command",
                    "command": command,
                    "timeout": sync_timeout(agent),
                }]),
            );
            Value::Object(group)
        }
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
    }
}

/// 同步 hook 的超时。
///
/// **单位两家不一样**：Gemini 是毫秒（默认 60000），Goose 是秒（默认 30）。
/// 写错单位不会报错，只会得到一个荒谬的值（5 毫秒和 5 秒差三个数量级），
/// 所以放在一个函数里、带注释，别在调用处各写各的。
///
/// 5 秒：本地 HTTP 上报的正常耗时是毫秒级，5 秒已经是很宽松的上限。
fn sync_timeout(agent: Agent) -> u32 {
    match agent {
        Agent::Gemini => 5_000,
        _ => 5,
    }
}

// ---- 落盘 ------------------------------------------------------------------

fn write_backup(path: &Path, existing: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(existing) = existing else {
        // 原本没有这个文件，没什么可备份的
        return Ok(None);
    };
    let backup = PathBuf::from(format!("{}.signalhub-backup", path.display()));
    // 备份也走原子写：它是在改原文件**之前**写的，写坏了就等于没有备份 ——
    // 而它存在的全部意义就是「原文件改坏了还能救回来」。
    crate::config::atomic_write(&backup, existing, None)
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
    // 原子写在这里尤其重要：写的是**用户自己的智能体配置文件**（`~/.claude/settings.json`
    // 之类），就地写坏一份手写的配置，代价远大于这个功能本身。注释里那条安全底线
    // 「解析失败就中止，绝不覆盖」防的是「内容错」，这一条防的是「写到一半」。
    crate::config::atomic_write(path, &text, None)
        .map_err(|err| format!("写 {} 失败: {err}", path.display()))
}

/// `signalhub` 是否已在 PATH 里。CLI 用它决定要不要提醒用户「现在写的是绝对路径」。
pub fn signalhub_on_path() -> bool {
    on_path("signalhub").is_some()
}

// ---- PATH 查找（避免为此引入依赖）------------------------------------------

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| candidates(&dir, name))
        .find(|candidate| is_executable(candidate))
}

/// 一个目录里，这个命令可能叫什么。
///
/// **Windows 上必须把 `.exe` 也试一遍。** `Path::is_file()` 不做 PATHEXT 展开，
/// 只找 `signalhub` 的话在 Windows 上永远找不到 —— 后果是 `signalhub_on_path()`
/// 恒为 false、装 hook 时永远写绝对路径（能用），但用户照着文档把二进制放进
/// PATH 也白放，而文档偏偏是这么建议的。
#[cfg(windows)]
fn candidates(dir: &Path, name: &str) -> [PathBuf; 2] {
    [dir.join(format!("{name}.exe")), dir.join(name)]
}

#[cfg(not(windows))]
fn candidates(dir: &Path, name: &str) -> [PathBuf; 1] {
    [dir.join(name)]
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

/// Windows 上 PATH 里的可执行文件带 `.exe`，而 `Path::is_file()` 不做 PATHEXT
/// 展开 —— 少了这一步，「signalhub 已在 PATH 里」永远判成否。这条只有 Windows
/// 的 CI 会跑，本机（macOS）编不进去。
#[cfg(all(windows, test))]
#[test]
fn path_lookup_tries_the_exe_suffix() {
    let names: Vec<String> = candidates(Path::new(r"C:\bin"), "signalhub")
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["signalhub.exe", "signalhub"]);

    // 真的找一个必然存在的命令，确认这条路走得通
    let found = on_path("cmd").or_else(|| on_path("powershell"));
    assert!(found.is_some(), "PATH 里总该有 cmd 或 powershell");
    assert_eq!(
        found.unwrap().extension().and_then(|ext| ext.to_str()),
        Some("exe"),
        "找到的应该是带 .exe 的那个"
    );
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

    // ---- Gemini / Goose -------------------------------------------------------

    /// 两家都是「同步等 hook」的，所以**不能**带 `async`（那是 Claude Code 的
    /// 扩展字段，它们的文档里没有），改带 `timeout`。
    #[test]
    fn gemini_and_goose_get_a_timeout_instead_of_async() {
        for agent in [Agent::Gemini, Agent::Goose] {
            let dir = TempDir::new();
            let path = dir.file("hooks.json");
            install_at(agent, CMD, &path, false).unwrap();

            let root = read(&path);
            for groups in root["hooks"].as_object().unwrap().values() {
                let handler = &groups[0]["hooks"][0];
                assert_eq!(handler["type"], "command", "{agent:?}");
                assert_eq!(handler["command"], CMD, "{agent:?}");
                assert!(handler.get("async").is_none(), "{agent:?} 不该写 async");
                assert!(handler.get("timeout").is_some(), "{agent:?} 该带超时");
            }
        }
    }

    /// 超时的**单位两家不一样**：Gemini 是毫秒、Goose 是秒。写错不会报错，
    /// 只会得到一个差三个数量级的荒谬值，所以单独钉住。
    #[test]
    fn the_two_sync_timeouts_use_their_own_units() {
        assert_eq!(sync_timeout(Agent::Gemini), 5_000, "Gemini 用毫秒");
        assert_eq!(sync_timeout(Agent::Goose), 5, "Goose 用秒");

        let dir = TempDir::new();
        let path = dir.file("hooks.json");
        install_at(Agent::Goose, CMD, &path, false).unwrap();
        assert_eq!(read(&path)["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
    }

    /// Goose 的 hook 必须待在**插件目录**里，光有 hooks.json 它不会加载
    #[test]
    fn goose_hooks_live_inside_its_plugin_directory() {
        assert_eq!(
            Agent::Goose.config_relative_path(),
            ".agents/plugins/signalhub/hooks/hooks.json"
        );
        assert_eq!(Agent::Gemini.config_relative_path(), ".gemini/settings.json");
    }

    /// 每个智能体的配置路径都必须不一样 —— 两个 agent 指向同一个文件的话，
    /// 装第二个会把第一个的条目当成自己的、原地改掉
    #[test]
    fn every_agent_has_its_own_config_path() {
        let mut paths: Vec<&str> = ALL_NAMES
            .iter()
            .map(|name| Agent::parse(name).unwrap().config_relative_path())
            .collect();
        let total = paths.len();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), total, "有智能体共用了同一个配置文件");

        // ALL_NAMES 里的每个名字都要能解析回来，否则 help 里列的是个假名字
        for name in ALL_NAMES {
            let agent = Agent::parse(name).unwrap();
            assert_eq!(agent_name_for_cli(agent), name, "{name} 的 CLI 名字对不上");
        }
    }

    /// 每个智能体都要有「痕迹目录」和「命令名」，否则界面上会有一个永远显示
    /// 「未检测到」的条目
    #[test]
    fn every_agent_has_a_marker_and_a_command() {
        for name in ALL_NAMES {
            let agent = Agent::parse(name).unwrap();
            assert!(!agent.marker_dir().is_empty(), "{name} 缺痕迹目录");
            assert!(!agent.command_names().is_empty(), "{name} 缺命令名");
        }
        // Goose 的痕迹目录和它的 hook 位置**不一样**（hook 装在我们的插件目录里），
        // 这条是刻意的，别在重构时被「统一」掉
        assert_eq!(Agent::Goose.marker_dir(), ".config/goose");
        assert!(Agent::Goose.config_relative_path().contains(".agents/plugins"));
    }

    /// 「装过没有」的判据是**配置里有没有我们的 handler**，不是「配置文件在不在」
    /// —— 那个文件本来就可能存在（用户自己配的别的东西）
    #[test]
    fn installed_is_decided_by_our_handlers_not_by_the_file_existing() {
        assert!(!config_has_our_hooks(""), "空文件 = 没装");
        assert!(!config_has_our_hooks("{}"), "没有 hooks 键 = 没装");
        assert!(!config_has_our_hooks(r#"{"hooks":{}}"#), "空的 hooks = 没装");
        assert!(
            !config_has_our_hooks(
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"someone-elses-tool hook x"}]}]}}"#
            ),
            "别人的 hook 不算我们装的"
        );
        assert!(
            !config_has_our_hooks("not json at all"),
            "解析不了就当没装 —— 不能因为文件坏了就报「已接入」"
        );

        assert!(
            config_has_our_hooks(
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"signalhub hook claude-code"}]}]}}"#
            ),
            "裸名字要认得出来"
        );
        assert!(
            config_has_our_hooks(
                r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"/opt/x/signalhub\" hook codex"}]}]}}"#
            ),
            "带引号的绝对路径也要认得出来"
        );
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

    /// sidecar 和主二进制**在同一个目录里** —— 这是 `bundled_cli` 靠「兄弟文件」
    /// 找到它的前提。Tauri 的 externalBin 就是这么放的（macOS 进 `Contents/MacOS/`，
    /// Windows 放在 exe 旁边）。
    #[test]
    fn the_cli_sits_beside_the_main_binary() {
        let app = std::path::Path::new("/Applications/SignalHub.app/Contents/MacOS/signalhub-app");
        let cli = cli_beside(app).expect("同目录的 CLI 路径");

        assert_eq!(cli.parent(), app.parent(), "必须在同一个目录");
        assert_eq!(cli.file_name().unwrap(), CLI_BINARY);
        // 这条是重点：**不能**是 GUI 二进制。它不认 hook 参数，而单实例插件会把
        // 它变成「把已经开着的窗口叫到前面」—— 静默失效，最难查。
        assert_ne!(cli.file_name().unwrap(), "signalhub-app");
    }

    /// 装出来的命令必须能被我们自己认出来 —— 否则卸载时找不到它、「已经装过没有」
    /// 也会答错。`binary_name` 会剥掉 `.exe`，所以 Windows 上也成立。
    #[test]
    fn the_command_we_write_is_one_we_can_recognize() {
        let cli = cli_beside(std::path::Path::new(
            "/Applications/SignalHub.app/Contents/MacOS/signalhub-app",
        ))
        .unwrap();

        let command = format!("{} hook claude-code", cli.display());
        assert!(is_our_hook_command(&command), "{command}");
        assert_eq!(binary_name(&cli.to_string_lossy()), "signalhub");
    }

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
