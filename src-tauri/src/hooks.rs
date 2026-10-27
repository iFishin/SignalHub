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
    Opencode,
    Pi,
    Hermes,
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
            "opencode" | "open-code" => Agent::Opencode,
            // Pi 同时是 @mariozechner/pi-coding-agent（命令就叫 `pi`）
            "pi" | "pi-agent" | "pi-coding-agent" => Agent::Pi,
            "hermes" | "hermes-agent" => Agent::Hermes,
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
            // 官方写法就是全小写的 `opencode`（不是 OpenCode），照抄，免得用户
            // 在两个名字之间对不上。
            Agent::Opencode => "opencode",
            Agent::Pi => "Pi",
            Agent::Hermes => "Hermes",
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
            // 插件式：opencode 启动时自动加载这个目录下的所有 js/ts 文件，所以
            // 「装」就是写一个我们自己的文件，「卸」就是删掉它（见 `InstallKind`）。
            Agent::Opencode => ".config/opencode/plugins/signalhub.js",
            // Pi 的扩展目录：启动时自动发现这里的 *.ts（见官方 extensions 文档）
            Agent::Pi => ".pi/agent/extensions/signalhub.ts",
            // Hermes 的插件是**一个目录**（里面有清单和代码），整个都是我们建的 ——
            // 所以这里的路径是目录，卸载时整删（同 Goose）。
            Agent::Hermes => ".hermes/plugins/signalhub",
        }
    }
}

/// 全部智能体，按加入顺序。`parse` 的报错信息和 CLI 的 help 都用它，
/// 免得新增一个之后忘改其中一处、报错里少列一个。
pub const ALL_NAMES: [&str; 8] = [
    "claude-code",
    "codex",
    "cursor",
    "gemini",
    "goose",
    "opencode",
    "pi",
    "hermes",
];

impl Agent {
    /// 我们往对方那儿放东西的**方式**。
    ///
    /// 抽象这一层而不是给每家写一套：现有的 5 家全是「读一份 JSON、往 `hooks` 键里
    /// 插条目」，而插件式的几家（写一个属于我们自己的文件、对方启动时自动加载）
    /// 共用一份实现，`events()` 那种逐事件的差异完全不参与。
    ///
    /// 为什么只挑「有自己插件目录」的家：往**共享配置**里插条目必须先解析用户的
    /// 文件。JSON 能整份解析后重写（`install_at` 就是），YAML 做不到 —— 重写会丢掉
    /// 注释和格式，那是不可接受的数据损坏。而插件目录里的文件**全是我们建的**，
    /// 解析这一步直接不存在，卸载就是删文件。
    fn install_kind(self) -> InstallKind {
        match self {
            Agent::Opencode | Agent::Pi | Agent::Hermes => InstallKind::PluginFile,
            Agent::ClaudeCode | Agent::Codex | Agent::Cursor | Agent::Gemini | Agent::Goose => {
                InstallKind::JsonHooks
            }
        }
    }

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
            Agent::Opencode => ".config/opencode",
            Agent::Pi => ".pi/agent",
            Agent::Hermes => ".hermes",
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
            Agent::Opencode => &["opencode"],
            Agent::Pi => &["pi"],
            Agent::Hermes => &["hermes"],
        }
    }
}

/// 见 [`Agent::install_kind`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// 往用户共享的一份 JSON 里插 `hooks` 条目。
    JsonHooks,
    /// 往对方自动加载的插件目录里放**我们自己的一个文件**
    /// （路径见 `Agent::config_relative_path`）。
    PluginFile,
}

/// 生成的插件源码里那一行标记的前缀。
///
/// 安装判据、卸载、以及「配置里那条命令还解析得到吗」全都只认它，**不解析对方的
/// 格式** —— 那些格式会随版本变，而这一行是我们自己写的。注释号（`//` / `#`）刻意
/// 不含在里面：找的是这一行里的这个子串，几种语言共用一份实现。
const PLUGIN_MARKER: &str = "signalhub-hook:";

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
    if agent.install_kind() == InstallKind::PluginFile {
        let Ok(path) = config_path(agent) else {
            return false;
        };
        return plugin_command_at(&path).is_some();
    }

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

/// 配置里**实际写着**的那条命令（不是我们现在会写的那条）。
///
/// 两者的差别正是最值得诊断的一件事：命令是某次装进去的，之后 app 被挪走、或者
/// 那个 sidecar 被删掉，配置里就留下一条**跑不起来也看不出来**的命令 —— 智能体
/// 不报错（它只是执行了一个不存在的程序，然后照常继续），灯也不亮。两边都很安静，
/// 而 README 里恰好警告过这件事「二进制一挪动 hook 就会静默失效」。
///
/// 见 [`command_resolves`] 怎么判断它还跑不跑得起来。
pub fn installed_command(agent: Agent) -> Option<String> {
    let path = config_path(agent).ok()?;
    match agent.install_kind() {
        // 插件式里那条命令就是**这个 CLI 自己**（插件拼出 `signalhub <状态> …`），
        // 不带 `hook <agent>` 那一段 —— 诊断要报的是「它指向的那个程序还在不在」。
        //
        // 注意这里**不能**先 `read_to_string`：Hermes 的家是一个目录，读它会直接
        // 失败，那条诊断就永远报不出来。
        InstallKind::PluginFile => plugin_command_at(&path),
        InstallKind::JsonHooks => {
            let text = fs::read_to_string(&path).ok()?;
            installed_command_in(&text)
        }
    }
}

/// 从一份配置内容里摘出我们那条 handler 的 `command`。
///
/// 抽成纯函数是为了能单测 —— 理由和 `config_has_our_hooks` 一样（那一个的注释
/// 写了：`is_installed` 依赖 `$HOME`，测试里改 `HOME` 是进程级的会打架）。
fn installed_command_in(text: &str) -> Option<String> {
    let root: Value = serde_json::from_str(text).ok()?;
    let hooks = root.get("hooks")?.as_object()?;
    for groups in hooks.values() {
        let groups = groups.as_array()?;
        let (group_index, handler_index) = locate_our_handler(groups)?;
        // 两种形状：Claude Code 那种 `{hooks: [{hooks: [...]}]}` 和直接一层 handler
        let handler = match handler_index {
            Some(index) => groups.get(group_index)?.get("hooks")?.get(index)?,
            None => groups.get(group_index)?,
        };
        if let Some(command) = handler.get("command").and_then(Value::as_str) {
            return Some(command.to_string());
        }
    }
    None
}

/// 一条 hook 命令里的程序**现在还解析得到吗**。
///
/// 只看程序那一段（第一个词，去掉引号）：带路径分隔符就按文件找，否则按 `PATH` 找。
/// 参数不参与判断 —— 要回答的只是「这条命令还跑不跑得起来」。
///
/// 复用 [`first_token`] / [`binary_name`]：这两件事（怎么切第一个词、怎么从路径里
/// 取出程序名）在「认不认得出是我们写的」那条路上已经各有一份了，再写一遍就会分叉。
pub fn command_resolves(command: &str) -> bool {
    let Some(program) = first_token(command).filter(|name| !name.is_empty()) else {
        return false;
    };
    if program.contains(['/', '\\']) {
        is_executable(Path::new(program))
    } else {
        // 剥掉 `.exe` 再交给 `on_path`：`candidates` 自己会把 `.exe` 补回去
        on_path(binary_name(program)).is_some()
    }
}

/// 装过之后，配置里那条命令是不是「当前版本该写」的那条（issue #19）。
///
/// `installed` 是配置里**实际写着**的那条（[`installed_command`]），`expected` 是
/// 这个版本会写的那条（[`resolve_hook_command`]）。两条不一致 = 升级后没重装：
/// 功能还在，行为停在旧版本 —— 典型就是 Claude Code 的 `SIGNALHUB_PID="$PPID"`
/// 前缀（0.3.2 起才有），没重装的人界面上看到「已接入」，灯却回到「沉默」。
///
/// **比较宽容在哪**：只把程序名 canonicalize 一次（[`command_key`]）—— PATH 上的
/// 裸名和同目录的绝对路径算同一条命令，用户调过 PATH 不会被误报。其余部分照旧比：
/// 参数变了、环境变量前缀多了/少了，都是「装的不是这一版」。
///
/// **不自动改用户配置**：这个判据只决定界面上要不要提示「重新接入」，写入仍然由
/// 用户点按钮触发（见 `lib.rs` 的 `detect_agents`）。
pub fn command_stale(installed: &str, expected: &str) -> bool {
    command_key(installed) != command_key(expected)
}

/// 一条命令的可比 key。
///
/// 按 shell 规则切词（支持引号、前导的环境变量赋值），然后：
///
/// - 环境变量赋值（`SIGNALHUB_PID="$PPID"`）记成 `env:NAME=VALUE`。**它是命令的
///   一部分**：进程号从哪来正是这一版要改的东西，不能当噪声抹掉。
/// - 裸名程序（`signalhub`）resolve 到 PATH 上的绝对路径；带路径的程序按原样。
///
/// 只要 key 有一处不同，两条命令就是两条不同的命令。
fn command_key(command: &str) -> String {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        // 读一个 shell 词：引号内的空白不是分隔符
        let mut word = String::new();
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                } else {
                    word.push(c);
                }
            } else if c == '"' || c == '\'' {
                quote = Some(c);
            } else if c.is_whitespace() {
                break;
            } else {
                word.push(c);
            }
            i += 1;
        }
        tokens.push(command_word_key(&word));
    }
    tokens.join("\u{1}")
}

/// 一个 shell 词的 key：环境变量赋值 / 程序名 / 普通参数。
fn command_word_key(word: &str) -> String {
    if let Some((name, value)) = word.split_once('=') {
        // 只认合法的 shell 变量名：`--foo=bar` 这种普通参数不按赋值处理
        let valid = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            return format!("env:{name}={value}");
        }
    }
    if word.contains(['/', '\\']) {
        // 带路径的程序：按原样（引号已在切词时剥掉）
        format!("path:{word}")
    } else {
        // 裸名：能 resolve 就换成绝对路径 —— 裸名和绝对路径是同一个程序
        match on_path(binary_name(word)) {
            Some(path) => format!("cmd:{}", path.display()),
            None => format!("cmd:{word}"),
        }
    }
}

/// 事件名清单。[`须写 matcher`]: 只有 Claude Code 的 SessionStart 需要
/// —— 不加限制的话，上下文压缩时触发的 SessionStart 会被当成「会话开始」，
/// 在智能体仍工作时把灯报成 idle。
///
/// **插件式的智能体（`InstallKind::PluginFile`）返回空**：它们不靠事件清单装，
/// 而是由我们生成的那份源码自己在内部把事件映射到 `signalhub <状态>` 上（见
/// `plugin_source`）。这张表只服务于「往共享 JSON 里插条目」那条路。
fn events(agent: Agent) -> &'static [(&'static str, Option<&'static str>)] {
    match agent {
        Agent::Opencode | Agent::Pi | Agent::Hermes => &[],
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
    /// **插件式**安装（`InstallKind::PluginFile`）写下的那个文件。
    ///
    /// 界面据此改口：JSON 类是「新增 N 个事件」，插件类根本没有事件清单这回事，
    /// 说「已写入 <路径>」才对得上用户能去 `cat` 的那个东西。
    pub plugin: Option<PathBuf>,
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
    let base = format!(
        "{} hook {}",
        quote_program(&cli_program()),
        agent_name_for_cli(agent)
    );

    // **只给实测过的一家带进程号。**
    //
    // Claude Code 通过 `/bin/sh -c` 跑命令，所以命令里的 `$PPID` 就是 claude
    // 自己（2026-10-20 用 `claude -p` + `ps -o ppid=` 探针实测）。这个进程号让
    // 灯能区分「只是没上报」和「进程已经没了」。
    //
    // 其余几家没探过它们的 hook 是怎么派生的：万一把一个短命的 shell 当成进程号，
    // 服务端会按「进程没了」把会话立刻收掉 —— 那比现在的「沉默」糟得多。所以
    // 它们继续走老路，等有人实测再加。
    if agent == Agent::ClaudeCode && cfg!(unix) {
        // 前缀赋值把进程号放进**环境变量**，而不是新加一个命令行选项：
        // 用户 PATH 上如果放的是旧版本的 CLI，它不认识新选项，会把整条 hook
        // 判成错误、上报直接断掉；环境变量则会被旧 CLI 原样忽略。
        return format!("SIGNALHUB_PID=\"$PPID\" {base}");
    }

    base
}

/// 我们自己的 CLI 该怎么调，**未加引号**。
///
/// 插件式生成的源码用它（`spawn(program, args)` 直接给 argv，不经过 shell，所以
/// 绝不能带引号 —— 带了就是「找不到名为 `"…"` 的程序」）。写进**配置里的命令行**
/// 那份要过 [`quote_program`]。
fn cli_program() -> String {
    match on_path("signalhub") {
        Some(_) => "signalhub".to_string(),
        None => bundled_cli().unwrap_or_else(|| "signalhub".to_string()),
    }
}

/// 含空格的路径必须引起来，否则会被 shell 拆成两个参数。
fn quote_program(program: &str) -> String {
    if program.chars().any(char::is_whitespace) {
        format!("\"{program}\"")
    } else {
        program.to_string()
    }
}

/// 把一个值放进**双引号字符串字面量**里（JS / TS / Python 三种语言规则一致）。
///
/// 必须转义反斜杠：Windows 的路径 `C:\Users\me\signalhub.exe` 直接塞进
/// `"C:\Users\..."` 里，`\U` 会被当成转义序列 —— 结果是路径悄悄少掉几层反斜杠，
/// 插件跑去执行一个不存在的程序，而**双方都不报错**（这正是 `docs/api.md` 警告过
/// 的那种「静默失效」）。引号也要转义，否则路径里带 `"` 就能把源码写坏。
fn as_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // 换行会把「一行标记」拆成两行，卸载/诊断就找不到了
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
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
        Agent::Opencode => "opencode",
        Agent::Pi => "pi",
        Agent::Hermes => "hermes",
    }
}

/// 公开的接入方式判断，给「试一下」用。
///
/// 为什么调用方需要知道：插件式接入（opencode / Pi / Hermes）**没有可以单独运行的
/// 命令** —— 它的代码跑在对方的进程里，由对方加载、由对方调用。所以「试一下」对这
/// 两种接入的含义不同：命令式能真的把那条命令跑一遍（那才验证得了 PATH、配置、
/// 上报这一整条链路），插件式只能由我们内部注入一条测试信号，**证不了对方会不会
/// 调用我们** —— 而后者正是回执（`receipts.rs`）要回答的问题。
pub fn install_kind(agent: Agent) -> InstallKind {
    agent.install_kind()
}

/// 上报 / 日志里用的短名字（`claude-code`）。
///
/// 和 `Agent::name()`（给人看的品牌名 `Claude Code`）分开：那个出现在牌面文案里，
/// 这个出现在 hooks.log 的行里和上报的 `agent` 字段里。诊断文本要按它去日志里找
/// 这个智能体的行，所以得从外面拿得到。
pub fn cli_name(agent: Agent) -> &'static str {
    agent_name_for_cli(agent)
}

/// 「试一下」用的一条**真实的**事件 payload。
///
/// **必须真的能被 `map_event` 认出来。** 认不出来时那条命令什么也不做（日志里只
/// 留一行 `ignored`）—— 那会让「试一下」变成一次假通过，比没有这个按钮更糟。所以
/// 这张表和 `bin/signalhub.rs` 的映射表是配套的：改映射时一起看一眼。
///
/// 取的是各家「开始一轮 / 正在思考」那一档：它不依赖任何多余的 payload 字段
/// （`Stop` 要看 `status`、`Notification` 要看 `notification_type`），不会因为合成
/// 得不全而被认成别的意思。`None` = 插件式接入，没有能跑的命令。
pub fn probe_payload(agent: Agent) -> Option<String> {
    let event = match agent {
        Agent::ClaudeCode | Agent::Codex | Agent::Goose => "UserPromptSubmit",
        // Cursor 用 camelCase
        Agent::Cursor => "beforeSubmitPrompt",
        // Gemini 也是 PascalCase，但「开始一轮」叫 BeforeAgent
        Agent::Gemini => "BeforeAgent",
        // 插件式：没有能单独跑的命令（`install_kind` 会给出 `PluginFile`）
        Agent::Opencode | Agent::Pi | Agent::Hermes => return None,
    };
    // **会话名固定成 `signalhub-probe`**：它一出现就说明这是试出来的，而不是某个
    // 真实项目在报。
    //
    // 两个事件字段名都写上：Goose 认 `event`，其余认 `hook_event_name`，而
    // `event_name` 是按顺序试的 —— 同一个 payload 里不会两个都有且冲突。
    Some(
        serde_json::json!({
            "hook_event_name": event,
            "event": event,
            "session_id": "signalhub-probe",
            "cwd": probe_cwd(),
        })
        .to_string(),
    )
}

/// 合成 payload 里带的 `cwd`：当前工作目录。
///
/// 为什么要有：上报带 `cwd` 时分组会按目录算（用户开的就是这个行为），试出来的那块
/// 牌就落在**用户自己那个项目组**里，而不是冒出一个没人认识的组。取不到就不写这个
/// 字段（`Report.cwd` 是 `Option`），行为回落到默认分组。
fn probe_cwd() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
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

/// opencode 插件模板。两个占位符由 [`Agent::plugin_source`] 填。
///
/// **形状必须是 V2 的默认导出**（`{ id, setup }`）。V1 那种「具名导出 = 插件」的
/// 写法在 V2 里会被直接拒绝加载（`Plugin must export a default definition with an
/// id and an effect or setup function.`），而且两种形状没法共用一份文件 —— 所以这份
/// 只面向 V2（逐条事件在 opencode 2.0.29 上实测过，见 `docs/api.md` §4.15）。
///
/// 事件名与状态映射改动时**必须一起改 `docs/api.md` §4.15 的那张表**，并且重新
/// 实测：V2 把 V1 的 `session.idle` / `event.properties` 都换掉了，照 V1 的老印象
/// 猜事件名会得到一个永远亮着或永远不亮的灯（而不报错）。
const OPENCODE_PLUGIN: &str = r#"// 由 SignalHub 生成 —— 请勿手改。要更新就回 SignalHub 的「接入」页重新点一次接入。
// %MARKER%
//
// opencode 启动时会自动加载这个目录下所有的 js / ts 文件，于是这份文件就成了
// 「opencode 说什么 → signalhub 命令行」的翻译器。
//
// 这份源码按 **opencode V2 的插件形状**写：默认导出一个 `{ id, setup }` 定义。
// V1 那种「具名导出 = 插件」在 V2 里会被直接拒绝加载，两种形状没法共用一份文件，
// 所以这份只面向 V2（在 opencode 2.0.29 上实测过）。
//
// 改之前先读四条纪律：
//
//  1. **不许阻塞。** 事件流循环里只做一件事：派生一个子进程就撒手（spawn + unref，
//     stdio 全丢掉）。同步等待、await、execFileSync 都可能把 opencode 卡住。
//  2. **不许抛。** 上报失败绝不许影响 opencode 自己：每个事件包在 try 里，子进程的
//     error 事件也吃掉，事件流断了也不往外扔。
//  3. **只映射下面这几个事件。** 名字和形状都来自官方 Plugins 文档并逐条实测过；
//     多映射一个没核实过的事件（`session.status` / `session.step.*` / 逐 token 的
//     `session.reasoning.*` 之类）就可能得到一个「永远在忙」的假灯，或者按 token
//     起进程。也刻意没有「推理中」那一档：V2 没有形状确定的对应事件。
//  4. **只报自己这个目录的会话。** opencode 会**按目录**把这份插件实例化多次（项目
//     目录一份、家目录一份…），而每个实例收到的都是**整个服务端**的事件流。不过滤
//     的话同一个会话会被它按两个不同的 cwd 各报一遍 —— 服务端那边组名和显示名是后
//     写覆盖的，灯上就会串成另一个目录的名字。
import { spawn } from "node:child_process"

const CLI = "%CLI%"
const AGENT = "opencode"

function report(state, session, cwd, message) {
  try {
    // 不知道是哪个会话时**绝不发 clear**：会话名会兜底成 "default"，清掉它等于
    // 顺手把别的上报方的状态一起抹了。
    if (state === "clear" && !session) return
    const args =
      state === "clear"
        ? ["clear", "--agent", AGENT]
        : [state, "--agent", AGENT, "--cwd", cwd]
    if (session) args.push("--session", session)
    if (message) args.push("--message", String(message).slice(0, 200))
    const child = spawn(CLI, args, {
      detached: true,
      windowsHide: true,
      stdio: "ignore",
      // 我们就在智能体自己的进程里，这个 pid 就是「会话还活着」的凭据。
      // 走环境变量而不是命令行选项：旧版 CLI 不认识新选项会把整条上报判成
      // 失败，而环境变量它根本不看。
      env: { ...process.env, SIGNALHUB_PID: String(process.pid) },
    })
    child.on("error", () => {})
    child.unref()
  } catch {
    // 上报永远不许影响 opencode 自己
  }
}

// 失败原因那句话。`session.execution.failed` 带的是 `{ type, message }`；助手消息
// 那类错误则是 `{ name, data: { message } }` —— 两种都认，认不出就只报状态。
function reason(error) {
  if (!error) return ""
  if (typeof error.message === "string") return error.message
  if (typeof error.data?.message === "string") return error.data.message
  return ""
}

export default {
  id: "signalhub",
  async setup(ctx) {
    // 这个实例属于哪个目录。既用来过滤，也当上报的 cwd。
    const home = ctx.location?.directory ?? ""
    // 会话 id → 它属于哪个目录。opencode 的事件里**只有 `session.created` 带位置**
    // （别的只有会话 id），所以没见过的会话要回问一次 `session.get`；问出来的结果
    // 缓存住 —— 一个会话只问一次。
    const owner = new Map()

    async function ours(session) {
      if (!session) return false
      if (!home) return true // 不知道自己属于哪个目录 → 不过滤（退回「全都报」）
      const known = owner.get(session)
      if (known !== undefined) return known === home
      try {
        const dir = (await ctx.session.get({ sessionID: session }))?.location?.directory
        if (typeof dir === "string" && dir) {
          owner.set(session, dir)
          return dir === home
        }
      } catch {
        // 问不出来就照报：宁可多报一遍，也不要让灯黑着
      }
      return true
    }

    async function onEvent(event) {
      const data = event?.data ?? {}
      const session = typeof data.sessionID === "string" ? data.sessionID : ""
      switch (event?.type) {
        case "session.created": {
          // 位置只有这个事件带 —— 顺手记下来，后面的事件就不用回问了
          const dir = data.location?.directory
          if (typeof dir === "string" && dir && session) owner.set(session, dir)
          if (!dir || !home || dir === home) report("idle", session, home || dir)
          return
        }
        case "session.execution.started":
          if (await ours(session)) report("working", session, home)
          return
        case "session.execution.succeeded":
        case "session.execution.interrupted":
          if (await ours(session)) report("done", session, home)
          return
        case "session.execution.failed":
          if (await ours(session)) report("error", session, home, reason(data.error))
          return
        case "permission.asked":
          if (await ours(session)) report("permission", session, home)
          return
        case "permission.replied":
          if (await ours(session)) report("working", session, home)
          return
        case "session.deleted":
          // 清一条会话**不问位置**：清错最多白清一次（服务端重复清除不报错），
          // 而漏清会留下一盏再也不走的假灯。
          if (session) report("clear", session, home)
          return
        default:
          return
      }
    }

    const controller = new AbortController()
    void (async () => {
      try {
        for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
          try {
            await onEvent(event)
          } catch {
            // 单个事件处理失败绝不许影响 opencode
          }
        }
      } catch {
        // 事件流本身断了（opencode 退出/重连）也不许抛
      }
    })()

    return () => controller.abort()
  },
}
"#;

/// Pi 扩展模板。两个占位符同 [`OPENCODE_PLUGIN`]。
const PI_PLUGIN: &str = r#"// 由 SignalHub 生成 —— 请勿手改。要更新就回 SignalHub 的「接入」页重新点一次接入。
// %MARKER%
//
// pi 启动时会自动发现 ~/.pi/agent/extensions/ 下的扩展，于是这份文件就成了
// 「pi 说什么 → signalhub 命令行」的翻译器。
//
// 改之前先读三条纪律（和 opencode 那份同源）：
//
//  1. **不许阻塞。** 每一次工具调用都会打到这里。同步等待、await、execSync 都会把
//     pi 卡住 —— 用 spawn 派生完就 unref，stdio 全丢掉。
//  2. **不许抛。** 上报失败绝不许影响 pi 自己，所以整段包在 try 里，子进程的
//     error 事件也吃掉。
//  3. **只映射下面这几个事件。** 名字都来自官方 extensions 文档；多映射一个没核实
//     过的事件就可能得到一个「永远在忙」的假灯。
//
// 下面那个 import 是**类型导入**：jiti 转译时会整个擦掉，不做类型检查。所以就算
// 这个项目里没装 @mariozechner/pi-coding-agent，这份扩展也照样加载。
import { spawn } from "node:child_process"
import type { ExtensionAPI } from "@mariozechner/pi-coding-agent"

const CLI = "%CLI%"
const AGENT = "pi"

function report(state, session, cwd, message) {
  try {
    // 不知道是哪个会话时**绝不发 clear**：会话名会兜底成 "default"，
    // 清掉它等于把别的上报方的状态一起抹了。
    if (state === "clear" && !session) return
    const args =
      state === "clear"
        ? ["clear", "--agent", AGENT]
        : [state, "--agent", AGENT, "--cwd", cwd]
    if (session) args.push("--session", session)
    if (message) args.push("--message", String(message).slice(0, 200))
    const child = spawn(CLI, args, {
      detached: true,
      windowsHide: true,
      stdio: "ignore",
      // 我们就在智能体自己的进程里，这个 pid 就是「会话还活着」的凭据。
      // 走环境变量而不是命令行选项：旧版 CLI 不认识新选项会把整条上报判成
      // 失败，而环境变量它根本不看。
      env: { ...process.env, SIGNALHUB_PID: String(process.pid) },
    })
    child.on("error", () => {})
    child.unref()
  } catch {
    // 上报永远不许影响 pi 自己
  }
}

export default function (pi: ExtensionAPI) {
  // 会话 id 优先用 `getSessionId()`（uuidv7，短且唯一）；拿不到就退回会话**文件**
  // 的路径 —— 它同样唯一，只是长一点。
  const session = (ctx: any) =>
    String(
      ctx?.sessionManager?.getSessionId?.() ??
        ctx?.sessionManager?.getSessionFile?.() ??
        "",
    )
  const cwd = (ctx: any) => String(ctx?.cwd ?? process.cwd())

  pi.on("session_start", (_event: any, ctx: any) =>
    report("idle", session(ctx), cwd(ctx)),
  )
  pi.on("before_agent_start", (_event: any, ctx: any) =>
    report("thinking", session(ctx), cwd(ctx)),
  )
  // 只取工具名（`bash` / `edit`），**不碰 event.args**：那里可能有密钥，而这句话
  // 会进灯的状态、系统通知正文和消息流。
  pi.on("tool_execution_start", (event: any, ctx: any) =>
    report("working", session(ctx), cwd(ctx), event?.toolName),
  )
  pi.on("agent_end", (_event: any, ctx: any) =>
    report("done", session(ctx), cwd(ctx)),
  )
  // 换会话（/new、/resume）时，**旧实例**先收到 session_shutdown，之后才是新会话的
  // session_start —— 所以这里清掉的正是刚结束的那个会话。
  pi.on("session_shutdown", (_event: any, ctx: any) =>
    report("clear", session(ctx), cwd(ctx)),
  )
}
"#;

/// Hermes 插件清单。内容固定，没有任何来自用户的输入。
const HERMES_MANIFEST: &str = r#"name: signalhub
version: "0.1.0"
description: Report this Hermes session's status to SignalHub
"#;

/// Hermes 插件模板（写进 `__init__.py`）。两个占位符同 [`OPENCODE_PLUGIN`]。
const HERMES_PLUGIN: &str = r#"# 由 SignalHub 生成 —— 请勿手改。要更新就回 SignalHub 的「接入」页重新点一次接入。
# %MARKER%
"""把 Hermes 的会话生命周期转成一条 `signalhub` 命令行。

Hermes 在 CLI 和网关里都会加载 ~/.hermes/plugins/ 下的插件，于是这份文件就成了
「Hermes 说什么 → signalhub 命令行」的翻译器。

改之前先读四条纪律：

 1. **不许阻塞。** 每一次工具调用都会打到这里。subprocess.run / wait 都会把 Hermes
    卡住 —— 用 Popen 派生完就走，不 wait。
 2. **不许抛。** 上报失败绝不许影响 Hermes 自己，所以整段包在 try 里。
 3. **只映射下面这几个事件。** 名字都来自官方 hooks 文档。
 4. **会话 id 只认 `session_id`，绝不拿 `task_id` 顶替。** 见下面那条注释。
"""

import os
import subprocess
import sys

CLI = "%CLI%"
AGENT = "hermes"


def _report(state, session="", cwd=None, message=None):
    # 不知道是哪个会话时**绝不发 clear**：会话名会兜底成 "default"，
    # 清掉它等于把别的上报方的状态一起抹了。
    if state == "clear" and not session:
        return
    args = [CLI, state, "--agent", AGENT]
    if state != "clear":
        # Hermes 的 hook 参数里**没有**工作目录，但插件就跑在它自己的进程里，
        # 所以 os.getcwd() 就是「在哪个项目里干活」—— 按目录分组靠它。
        args += ["--cwd", cwd or os.getcwd()]
    if session:
        args += ["--session", session]
    if message:
        args += ["--message", str(message)[:200]]
    try:
        kwargs = {}
        # 我们就在 Hermes 进程里。走环境变量而不是命令行选项：旧版 CLI 不认识
        # 新选项会把整条上报判成失败，而环境变量它根本不看。
        kwargs["env"] = {**os.environ, "SIGNALHUB_PID": str(os.getpid())}
        if sys.platform != "win32":
            # 脱离 Hermes 的进程组：它被 Ctrl+C 带走时上报不该跟着死
            kwargs["start_new_session"] = True
        subprocess.Popen(
            args,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            **kwargs,
        )
    except Exception:
        pass


def _register(ctx):
    def on_session_start(session_id="", **kwargs):
        _report("idle", session_id)

    def on_pre_llm_call(session_id="", **kwargs):
        _report("thinking", session_id)

    def on_pre_tool_call(tool_name=None, session_id="", **kwargs):
        # **只取工具名**（terminal / web_search / read_file …），不碰 args：
        # 那里可能有密钥，而这句话会进灯的状态、系统通知正文和消息流。
        #
        # **`task_id` 不是会话 id，别拿它顶替。** 它在 Hermes 里是
        # `effective_task_id = task_id or str(uuid.uuid4())`，**每一轮都会重来**
        # 一次 —— 用它当会话 id 会在灯上给同一场会话每轮多出一块牌。
        _report("working", session_id, message=tool_name)

    def on_post_llm_call(session_id="", **kwargs):
        _report("done", session_id)

    def on_session_end(session_id="", **kwargs):
        # **映射成 done 而不是 clear。** Hermes 的 on_session_end 是
        # 「每一次 run_conversation() 结束」（也就是**每一轮**）都发，不是进程退出
        # 才发；映射成 clear 会让灯每轮结束后闪没一次。收尾交给 TTL（stale）。
        _report("done", session_id)

    ctx.register_hook("on_session_start", on_session_start)
    ctx.register_hook("pre_llm_call", on_pre_llm_call)
    ctx.register_hook("pre_tool_call", on_pre_tool_call)
    ctx.register_hook("post_llm_call", on_post_llm_call)
    ctx.register_hook("on_session_end", on_session_end)


def register(ctx):
    try:
        _register(ctx)
    except Exception:
        # 注册失败也不许把 Hermes 带崩
        pass
"#;

impl Agent {
    /// 插件式安装要写的源码。**纯函数**：输入只有 CLI 的路径，不碰环境，所以
    /// 「标记行对不对、转义对不对、事件映射对不对」全都能单测。
    fn plugin_source(self, program: &str) -> String {
        match self {
            Agent::Opencode => OPENCODE_PLUGIN
                .replace("%MARKER%", &self.plugin_marker_line(program))
                .replace("%CLI%", &as_string_literal(program)),
            Agent::Pi => PI_PLUGIN
                .replace("%MARKER%", &self.plugin_marker_line(program))
                .replace("%CLI%", &as_string_literal(program)),
            Agent::Hermes => HERMES_PLUGIN
                .replace("%MARKER%", &self.plugin_marker_line(program))
                .replace("%CLI%", &as_string_literal(program)),
            _ => unreachable!("这个智能体不是插件式安装（见 install_kind）"),
        }
    }

    /// 插件式安装要写的**全部**文件（绝对路径 → 内容）。
    ///
    /// 单文件的家（opencode / Pi）只有一份，路径就是 `config_relative_path` 本身；
    /// Hermes 的插件是「一个目录 + 清单 + 代码」，所以是两份，都在那个目录下。
    fn plugin_files(self, program: &str) -> Result<Vec<(PathBuf, String)>, String> {
        let root = config_path(self)?;
        Ok(match self {
            Agent::Opencode | Agent::Pi => vec![(root, self.plugin_source(program))],
            Agent::Hermes => vec![
                (root.join("plugin.yaml"), HERMES_MANIFEST.to_string()),
                (root.join("__init__.py"), self.plugin_source(program)),
            ],
            _ => unreachable!("这个智能体不是插件式安装（见 install_kind）"),
        })
    }

    /// 生成源码里那一行标记，命令用**带引号**的写法。
    ///
    /// 带引号是为了过 [`command_resolves`]：它按 shell 规则切第一个词，路径里有
    /// 空格时不引起来就只切到一半，诊断会把一个装好的插件报成坏的。
    fn plugin_marker_line(self, program: &str) -> String {
        format!("{PLUGIN_MARKER} {}", quote_program(program))
    }
}

/// 从一份源码里摘出标记行后面的那条命令。
///
/// 抽成纯函数是为了能单测：`is_installed` / `installed_command` 都依赖 `$HOME`，而
/// 测试里改 `HOME` 是进程级的、会和并行跑的其它用例打架（`config_has_our_hooks` 的
/// 注释写了同样的话）。
///
/// **判据是这一行，不是文件在不在**：同一个路径上可能躺着用户自己写的插件，
/// 光看文件会把没装的说成装了。
fn plugin_command_at(root: &std::path::Path) -> Option<String> {
    // 单文件的家（opencode / Pi）：直接看那个文件
    if root.is_file() {
        let text = fs::read_to_string(root).ok()?;
        return plugin_command_in(&text);
    }
    // 目录的家（Hermes）：看目录里**任意一份**文件 —— 目录整个是我们的，
    // 所以「里面有没有我们的标记」就够了，不用记得标记具体写在哪个文件里。
    fs::read_dir(root)
        .ok()?
        .flatten()
        .find_map(|entry| {
            let text = fs::read_to_string(entry.path()).ok()?;
            plugin_command_in(&text)
        })
}

fn plugin_command_in(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.split_once(PLUGIN_MARKER))
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|command| !command.is_empty())
}


pub fn install(agent: Agent, dry_run: bool) -> Result<Report, String> {
    if agent.install_kind() == InstallKind::PluginFile {
        return install_plugin_files(agent, dry_run);
    }

    let path = config_path(agent)?;
    let mut report =
        install_at(agent, &HookSpec::Command(resolve_hook_command(agent)), None, &path, dry_run)?;

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

/// 用 **HTTP 方式**接入（目前只有 Claude Code 支持）。
///
/// 和命令式的差别只有一个：配置里写的是一条本地 URL，而不是一条要跑起来的命令。
/// 于是**「命令不在 PATH 上 / 应用被挪走了」这一整类失败都不存在** —— 那是接入页
/// 红色提示里最常见的一条，也是这个模式存在的全部理由。
///
/// `port` 是**此刻**上报服务真正在用的端口（`config::read_port`）。为什么不留一个
/// 占位符等启动时再改：改用户的配置文件是件该被看见的事，运行中偷偷去改它，
/// 用户就永远不知道自己的配置里写了什么。端口变了重装一次即可 —— 而重装是我们
/// 在界面上明确引导的动作。
pub fn install_http(agent: Agent, port: u16, dry_run: bool) -> Result<Report, String> {
    if agent != Agent::ClaudeCode {
        // 其余几家没有可对照的文档事实：不写「验证过的映射」之外的东西
        return Err("HTTP 方式目前只支持 Claude Code".to_string());
    }
    let url = hook_url(port);
    let path = config_path(agent)?;

    // **装了也会被静默跳过的一种情况**，所以宁可在这里拒绝。见 `url_allowed`。
    if let Some(text) = fs::read_to_string(&path).ok()
        && url_allowed(&text, &url) == Some(false)
    {
        return Err(format!(
            "{} 里定义了 allowedHttpHookUrls，而 {url} 不在里面 —— \
             定义了这个键之后，Claude Code 只会运行列表里匹配的 HTTP hook，\
             其余的一律不跑（那就是「装上了却永远不亮」）。\
             请在 allowedHttpHookUrls 里加上 \"http://127.0.0.1:*\"，\
             或者删掉这个键，然后再点一次。",
            path.display()
        ));
    }

    install_at(agent, &HookSpec::Http { url }, None, &path, dry_run)
}

/// 装「从灯上审批」（C3）：HTTP 式接入，但把 `PermissionRequest` 那一条换成
/// 带 token 的审批端点。
///
/// 为什么是**整份 HTTP 接入 + 一条替换**而不是「加一条」：审批要能工作，前提是
/// 其余事件也在报（灯上得先知道有这么一个会话，黄了才知道是谁在等）。而「加一条」
/// 会让 `PermissionRequest` 上挂着两个 hook —— 同一个确认框打两次接口，其中一次
/// 还是免鉴权的上报端点，读起来没人说得清哪条在起作用。
///
/// 关掉时把 HTTP 式接入再装一遍（`install_http`）就回到只上报 —— 见 `lib.rs`。
pub fn install_approve(
    agent: Agent,
    port: u16,
    token: &str,
    dry_run: bool,
) -> Result<Report, String> {
    if agent != Agent::ClaudeCode {
        // 只有 Claude Code 的文档里写清了「HTTP hook 可以回一个决定」
        return Err("从灯上审批目前只支持 Claude Code".to_string());
    }
    let url = hook_url(port);
    let approve = HookSpec::Approve {
        url: approve_url(port),
        token: token.to_string(),
    };
    let path = config_path(agent)?;

    // 两条地址都要过白名单判定：定义了这个键之后，不在列表里的 hook 会被静默跳过
    // ——「装上了却永远不亮」正是这个项目最不能接受的一种失败（见 `url_allowed`）。
    if let Ok(text) = fs::read_to_string(&path) {
        for candidate in [&url, &approve.as_text()] {
            if url_allowed(&text, candidate) == Some(false) {
                return Err(format!(
                    "{} 里定义了 allowedHttpHookUrls，而 {candidate} 不在里面 —— \
                     定义了这个键之后，Claude Code 只会运行列表里匹配的 HTTP hook，\
                     其余的一律不跑（那就是「装上了却永远不亮」）。\
                     请在 allowedHttpHookUrls 里加上 \"http://127.0.0.1:*\"，\
                     或者删掉这个键，然后再点一次。",
                    path.display()
                ));
            }
        }
    }

    install_at(agent, &HookSpec::Http { url }, Some(&approve), &path, dry_run)
}

/// 用户自己的 HTTP hook 白名单里允不允许这个 URL。
///
/// `None` = 那个键不存在（默认不限制）。**有它就一定要判定**：定义了这个键之后，
/// Claude Code 只会运行列表里匹配的 HTTP hook，其余的一律不跑 —— 装了、一点也不亮、
/// 而且看不出为什么，正是这个项目最不能接受的一种失败。
///
/// 匹配规则照官方文档（`*` 通配、主机名大小写不敏感）。这里刻意做得**比对方更严**：
/// 严的后果是「拒绝安装并告诉用户怎么写」，松的后果是「装上了但永远不亮」。
fn url_allowed(text: &str, url: &str) -> Option<bool> {
    let root: Value = serde_json::from_str(text).ok()?;
    let patterns = root.get("allowedHttpHookUrls")?.as_array()?;
    Some(
        patterns
            .iter()
            .filter_map(Value::as_str)
            .any(|pattern| glob_match(pattern, url)),
    )
}

/// `*` 通配、其余按字面比、整体大小写不敏感。
///
/// 大小写不敏感是官方的说法（就主机名而言），而我们的 URL 全小写 —— 放宽只会让
/// 判定更松一点点，而松的那一点点对「主机名大小写」这种情形恰好是对的。
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();

    // 双指针 + 回溯：`*` 先只吞零个字符，后面不匹配了再回来多吞一个
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while t < text.len() {
        if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// 上报服务的本地端点。**只绑回环**（见 `server::bind_host`），所以写死的
/// `127.0.0.1` 和实际监听地址一致；端口是运行时才知道的。
fn hook_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{HOOK_PATH_PREFIX}claude-code")
}

/// 审批端点的地址（C3）。
fn approve_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/approve/claude-code")
}

/// 写进配置里的超时。**必须比 `approvals::WAIT`（45 秒）长**：反过来的话对方会先
/// 超时，我们那个「等用户点」的窗口就永远走不满。
pub(crate) const APPROVE_TIMEOUT_SECS: u32 = 60;

/// 装进去的是 HTTP 式（`true`）还是命令式（`false`）。没装过就是 `None`。
///
/// 界面要能说出「你在跑哪条路」：两种模式写进同一个配置，用户看得出来才谈得上
/// 换一种、或者知道「为什么我改不了 PATH 也没关系」。
pub fn installed_http(agent: Agent) -> Option<bool> {
    let path = config_path(agent).ok()?;
    let text = fs::read_to_string(path).ok()?;
    installed_http_in(&text)
}

/// 抽成纯函数是为了能单测（理由同 `config_has_our_hooks`：测试里改 `HOME` 是
/// 进程级的，会和并行跑的用例打架）。
fn installed_http_in(text: &str) -> Option<bool> {
    let root: Value = serde_json::from_str(text).ok()?;
    let hooks = &root["hooks"];
    let mut found = false;
    for groups in hooks.as_object()?.values() {
        // 形态有两种：`{ hooks: [ … ] }` 和（Cursor 那种）直接就是 handler
        for group in groups.as_array()? {
            let handlers: Vec<&Value> = match group.get("hooks").and_then(Value::as_array) {
                Some(inner) => inner.iter().collect(),
                None => vec![group],
            };
            for handler in handlers {
                if is_our_http_handler(handler) {
                    return Some(true);
                }
                if is_our_handler(handler) {
                    found = true;
                }
            }
        }
    }
    found.then_some(false)
}

pub fn uninstall(agent: Agent, dry_run: bool) -> Result<Report, String> {
    if agent.install_kind() == InstallKind::PluginFile {
        return uninstall_plugin_file(agent, dry_run);
    }

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

/// 插件式安装：把我们自己的那份源码写进对方自动加载的目录。
///
/// **没有「解析失败就中止，绝不覆盖」这一条** —— 那条底线防的是弄坏**用户的**配置，
/// 而这些文件是我们自己建的。同一个路径上残留着旧版本（或者干脆是用户手抄的一份）
/// 时，重装一次应该得到一个能用的插件，而不是一个坏掉的文件。
fn install_plugin_files(agent: Agent, dry_run: bool) -> Result<Report, String> {
    let program = cli_program();
    let files = agent.plugin_files(&program)?;
    let root = config_path(agent)?;

    let mut report = Report {
        path: root.clone(),
        backup: None,
        // 界面用它显示「这个插件调的是哪个 CLI」；带引号的写法与标记行一致
        command: Some(quote_program(&program)),
        added: Vec::new(),
        already: Vec::new(),
        updated: Vec::new(),
        removed: Vec::new(),
        removed_handlers: 0,
        dry_run,
        manifest: None,
        plugin: Some(root),
    };

    // 逐份比内容：**任何一份**变了就算「更新」。只比第一份会漏掉
    // 「清单没变、代码变了」这种情况（命令指向的路径被挪走时就是这样）。
    let mut changed = false;
    let mut exists = false;
    for (path, source) in &files {
        match fs::read_to_string(path) {
            Ok(text) if text == *source => exists = true,
            Ok(_) => {
                exists = true;
                changed = true;
            }
            Err(_) => changed = true,
        }
    }
    if changed {
        report.added.push("plugin".to_string());
    } else if exists {
        report.already.push("plugin".to_string());
        return Ok(report);
    }

    if dry_run {
        return Ok(report);
    }
    for (path, source) in &files {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("创建 {} 失败: {err}", parent.display()))?;
        }
        // 原子写：这些文件会被对方在执行过程中读到，写一半就是一份语法错误的源码
        crate::config::atomic_write(path, source, None)
            .map_err(|err| format!("写 {} 失败: {err}", path.display()))?;
    }
    Ok(report)
}

/// 插件式卸载：删掉我们自己写的那份东西。
///
/// 是文件还是目录**看它自己**，不多加一张表：opencode / Pi 的
/// `config_relative_path` 是**一个文件**（同名文件可能是别人的，所以只删这一个），
/// Hermes 的是一个**我们独有的目录**（整删）。前者绝不能顺手删父目录 ——
/// `plugins/` 是对方的目录，里面多半还躺着别人的插件。
fn uninstall_plugin_file(agent: Agent, dry_run: bool) -> Result<Report, String> {
    let path = config_path(agent)?;
    let existed = path.exists();
    if existed && !dry_run {
        let removed = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.map_err(|err| format!("删除 {} 失败: {err}", path.display()))?;
    }
    Ok(Report {
        path: path.clone(),
        backup: None,
        command: None,
        added: Vec::new(),
        already: Vec::new(),
        updated: Vec::new(),
        removed: Vec::new(),
        removed_handlers: usize::from(existed),
        dry_run,
        manifest: None,
        plugin: Some(path),
    })
}

// ---- 实现 ------------------------------------------------------------------

/// `approve` 给 `Some` 时，`PermissionRequest` 那一条改用那条 spec（从灯上审批），
/// 其余事件照旧用 `spec`（普通上报）。**一次写完**：先装上报、再改一条会留下
/// 「改到一半」的状态，而这个功能改的是用户自己的智能体配置。
fn install_at(
    agent: Agent,
    spec: &HookSpec,
    approve: Option<&HookSpec>,
    path: &Path,
    dry_run: bool,
) -> Result<Report, String> {
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
        // 每个事件各用哪条 spec：只有 `PermissionRequest` 那一条会被换成审批端点
        let spec = match (approve, *event) {
            (Some(approve), "PermissionRequest") => approve,
            _ => spec,
        };
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

            if spec.matches(slot) {
                already.push((*event).to_string());
            } else {
                // 从一种形态换成另一种也走这条路（见 `HookSpec::apply`）
                spec.apply(slot, agent);
                updated.push((*event).to_string());
            }
            continue;
        }

        groups.push(handler_group(agent, spec, *matcher));
        added.push((*event).to_string());
    }

    // Cursor 的 hooks.json 强制要求一个 version 字段
    if agent == Agent::Cursor {
        root.entry("version").or_insert_with(|| json!(1));
    }

    // 界面要显示的「写进去的是什么」：装了审批就显示审批那一条（用户刚拨的就是它）
    let shown = approve.unwrap_or(spec).as_text();
    if added.is_empty() && updated.is_empty() {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            command: Some(shown.clone()),
            added,
            already,
            updated,
            removed: Vec::new(),
            removed_handlers: 0,
            dry_run,
            manifest: None,
            plugin: None,
        });
    }

    let backup = if dry_run { None } else { write_backup(path, existing.as_deref())? };
    if !dry_run {
        write_json(path, &root)?;
    }

    Ok(Report {
        path: path.to_path_buf(),
        backup,
        command: Some(shown),
        added,
        already,
        updated,
        removed: Vec::new(),
        removed_handlers: 0,
        dry_run,
        manifest: None,
        plugin: None,
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
        plugin: None,
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
        plugin: None,
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
        || is_our_http_handler(handler)
}

/// HTTP 式 hook 的路径前缀。**刻意不带端口**：端口被占用时会退到系统分配端口，
/// 那时配置里留下的还是旧端口的 URL —— 只认路径就还认得出来（同 `is_our_hook_command`
/// 不拿完整命令串做相等比较的理由：认的是「这是我们写的」，不是「它现在完全正确」）。
const HOOK_PATH_PREFIX: &str = "/v1/hooks/";

/// 审批端点的路径前缀（C3）。
const APPROVE_PATH_PREFIX: &str = "/v1/approve/";

/// 我们写进配置的 HTTP 式 hook（`{"type":"http","url":"…/v1/hooks/claude-code"}`，
/// 或者审批那条 `…/v1/approve/claude-code`）。
///
/// **两条都要认**。只认上报那条的后果不只是「装第二遍会多出一条」，还有
/// **卸载时会把带 token 的那条留在用户配置里** —— 那是这个功能最不该留下的东西。
fn is_our_http_handler(handler: &Value) -> bool {
    handler
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| url.contains(HOOK_PATH_PREFIX) || url.contains(APPROVE_PATH_PREFIX))
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
    let mut trimmed = command.trim();

    // 跳过开头的环境变量赋值（`NAME=value`）。Claude Code 的 hook 命令是
    // `SIGNALHUB_PID="$PPID" signalhub hook ...`，直接取第一个词会把赋值
    // 当成程序名，于是「是不是我们写的」「程序还解析得到吗」都会答错。
    loop {
        let token = trimmed.split_whitespace().next()?;
        let Some((name, _)) = token.split_once('=') else {
            break;
        };
        // 只认合法的 shell 变量名，免得把带 `=` 的普通参数也跳掉
        let mut chars = name.chars();
        let is_name = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_name {
            break;
        }
        trimmed = trimmed[token.len()..].trim_start();
    }

    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        return rest.split('"').next();
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        return rest.split('\'').next();
    }
    trimmed.split_whitespace().next()
}

/// 我们写进别人配置里的那条 hook 长什么样。
///
/// **两种形态写进同一个 JSON 文档**，所以「找到我们的条目 / 更新它 / 判断是不是我们
/// 写的」这套逻辑只能有一份。分开写两遍的话，从一种形态换成另一种时一定会留下一条
/// 两头不靠的条目（见 [`HookSpec::apply`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSpec {
    /// 命令式：调用本机 CLI。Claude Code 之外的家只有这一种。
    Command(String),
    /// HTTP 式：让智能体**直接 POST 到本机端口**。不需要任何命令存在，所以
    /// 「命令跑不起来 / 不在 PATH 上」这一整类失败都不存在（见 `server::hook_report`）。
    Http { url: String },
    /// HTTP 式，但**要 token**，而且这一条会**等**（从灯上审批，见 `approvals.rs`）。
    ///
    /// 和 `Http` 的区别全在 [`HookSpec::body`] 里，而且每一处都是刻意的：
    /// 带 `Authorization`（这一条替用户做决定）、`timeout: 60`（要够人从屏幕上
    /// 挪到灯上点一下）、`onFailure: "continue"`（端点不在时回到终端里那个确认框）。
    Approve { url: String, token: String },
}

impl HookSpec {
    /// 界面和诊断要显示的那一条文本（命令式是命令，HTTP 式是 URL）。
    fn as_text(&self) -> String {
        match self {
            HookSpec::Command(command) => command.clone(),
            HookSpec::Http { url } => url.clone(),
            HookSpec::Approve { url, .. } => url.clone(),
        }
    }

    /// 现有条目和我们这条是不是同一个 —— 决定「已存在」还是「要更新」。
    fn matches(&self, slot: &Value) -> bool {
        match self {
            HookSpec::Command(command) => {
                slot.get("command").and_then(Value::as_str) == Some(command.as_str())
            }
            HookSpec::Http { url } => {
                slot.get("url").and_then(Value::as_str) == Some(url.as_str())
            }
            // 按 URL 判断就够：两条端点不一样，所以「已经装的是哪一种」分得开
            HookSpec::Approve { url, .. } => {
                slot.get("url").and_then(Value::as_str) == Some(url.as_str())
            }
        }
    }

    /// 把现有条目改写成我们这条。
    ///
    /// **整条替换，不是只改那一个字段**：从 HTTP 式换成命令式时，留着 `url` 和
    /// `type: "http"` 会得到一条两头不靠的条目 —— 而智能体多半会按 `type` 走，
    /// 于是「换回命令方式」看起来成功了、实际还在打 HTTP。
    fn apply(&self, slot: &mut Value, agent: Agent) {
        *slot = self.body(agent);
    }

    /// 内侧那一项（`{type, command, async}` / `{type, url, timeout, onFailure}`）。
    fn body(&self, agent: Agent) -> Value {
        match self {
            HookSpec::Command(command) => match agent {
                // `async: true` 是 Claude Code / Codex 的扩展字段，也让 hook 在后台
                // 跑、不阻塞智能体。注意异步 hook 不受 timeout 约束（docs/api.md §4.4）。
                Agent::ClaudeCode | Agent::Codex => {
                    json!({ "type": "command", "command": command, "async": true })
                }
                // Gemini / Goose 会**同步等待** hook（两家都没有 async 开关），所以给
                // 一个明确的超时，免得一个挂住的 hook 把智能体卡在那儿。
                //
                // 对不认识的字段宁可保守：Copilot 的文档明确说过「inline 的 hooks 里
                // 有一项校验不过，整个 hooks 字段作废」。
                Agent::Gemini | Agent::Goose => json!({
                    "type": "command",
                    "command": command,
                    "timeout": sync_timeout(agent),
                }),
                Agent::Cursor => json!({ "command": command, "timeout": 5 }),
                Agent::Opencode | Agent::Pi | Agent::Hermes => {
                    unreachable!("插件式的智能体不构造 hooks 条目")
                }
            },
            HookSpec::Http { url } => {
                // `onFailure: "continue"` + 5 秒：**任何情况下都不能因为我们的端点
                // 不在而阻塞智能体**（见 docs/plan-round4.md §9）。这两项是这一整条
                // 路上的安全底线，不是可调参数。
                json!({
                    "type": "http",
                    "url": url,
                    "timeout": 5,
                    "onFailure": "continue",
                })
            }
            HookSpec::Approve { url, token } => {
                // 这三项和上面那条**故意不一样**，每一条都是这个功能的形状：
                //
                // - `timeout: 60`：这一条会等到有人点（我们的截止时间是 45 秒，
                //   比它短 —— 永远是我们先回话，对方的超时不该触发）。
                // - `onFailure: "continue"`：**端点不在时绝不阻塞智能体**，
                //   回到终端里那个确认框。这是 §9 那条红线的延续，不是例外。
                // - `Authorization`：这一条替用户做决定，所以要有凭证。token 写在
                //   用户主目录的配置里（`~/.claude/settings.json`），不进仓库。
                json!({
                    "type": "http",
                    "url": url,
                    "timeout": APPROVE_TIMEOUT_SECS,
                    "onFailure": "continue",
                    "headers": { "Authorization": format!("Bearer {token}") },
                })
            }
        }
    }
}

/// 构造一个 matcher-group。三种形状：
/// Claude Code / Codex 是 `{ matcher?, hooks: [ { … } ] }`，
/// Gemini / Goose 同构（区别只在内侧那一项，见 [`HookSpec::body`]），
/// Cursor 是 `{ command, timeout }` 直接作为一组。
fn handler_group(agent: Agent, spec: &HookSpec, matcher: Option<&str>) -> Value {
    match agent {
        // 插件式安装不走这条路（它不写共享配置），真走到了就是分类写错了。
        Agent::Opencode | Agent::Pi | Agent::Hermes => {
            unreachable!("插件式的智能体不构造 hooks 条目")
        }
        // Cursor 的 groups 形态不同：没有 `hooks` 数组，命令直接摆在组里
        Agent::Cursor => spec.body(agent),
        Agent::ClaudeCode | Agent::Codex | Agent::Gemini | Agent::Goose => {
            let mut group = Map::new();
            if let Some(matcher) = matcher {
                group.insert("matcher".into(), json!(matcher));
            }
            group.insert("hooks".into(), json!([spec.body(agent)]));
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

        let report = install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
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
        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();

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
        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();

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
            install_at(agent, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();

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
        install_at(Agent::Goose, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
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

    /// 从配置里读回**实际写着**的那条命令 —— 「装了却没反应」的第一个要查的东西。
    #[test]
    fn the_installed_command_can_be_read_back() {
        assert_eq!(
            installed_command_in(
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/opt/x/signalhub hook claude-code"}]}]}}"#
            )
            .as_deref(),
            Some("/opt/x/signalhub hook claude-code")
        );
        // 引号形式（带空格的路径）原样读回来，去掉引号是判断时的事
        assert_eq!(
            installed_command_in(
                r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"/opt/x y/signalhub\" hook codex"}]}]}}"#
            )
            .as_deref(),
            Some("\"/opt/x y/signalhub\" hook codex")
        );
    }

    /// 只有**我们的** handler 才算数：读回来的必须是那条会真正跑起来的命令。
    #[test]
    fn a_foreign_handler_is_never_read_back_as_ours() {
        assert_eq!(
            installed_command_in(
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"someone-elses-tool hook x"}]}]}}"#
            ),
            None
        );
        assert_eq!(installed_command_in("{}"), None);
        // 坏 JSON 不能 panic —— 用户在编辑配置时正好存了个半截的
        assert_eq!(installed_command_in("not json"), None);
        assert_eq!(installed_command_in(""), None);
    }

    /// 命令的第一个词。**引号里的带空格路径要整个取出来** —— 按空格切会切出一个
    /// 不存在的 `/opt/x`，于是把一条好命令判成失效。
    #[test]
    fn the_first_token_of_a_command_handles_quotes() {
        assert_eq!(first_token("signalhub hook claude-code"), Some("signalhub"));
        assert_eq!(first_token("  /a/b/cli  hook  x  "), Some("/a/b/cli"));
        assert_eq!(
            first_token("\"/Users/me/My Apps/signalhub\" hook claude-code"),
            Some("/Users/me/My Apps/signalhub")
        );
        assert_eq!(first_token("'/a b/cli' hook x"), Some("/a b/cli"));
        assert_eq!(first_token("   "), None);
        assert_eq!(first_token(""), None);
        // 开头的环境变量赋值不是程序名（Claude Code 的 hook 就是这么写的）
        assert_eq!(
            first_token(r#"SIGNALHUB_PID="$PPID" signalhub hook claude-code"#),
            Some("signalhub")
        );
        assert_eq!(
            first_token(r#"A=1 B=2 "/Applications/My App/signalhub" hook x"#),
            Some("/Applications/My App/signalhub")
        );
    }

    /// 「这条命令现在还跑得起来吗」：只看程序那一段，参数不参与判断。
    #[test]
    fn a_command_resolves_by_its_program_alone() {
        // 一定存在的：跑这个测试的那个可执行文件自己
        let me = std::env::current_exe().unwrap().display().to_string();
        assert!(command_resolves(&format!("\"{me}\" hook claude-code")));
        assert!(command_resolves(&format!("\"{me}\" --help")));

        // PATH 上的裸名字（三平台都有：sh / cmd）
        let bare = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(command_resolves(&format!("{bare} something")));

        // 挪走的 app 留下的那条：文件没了，但配置里看不出来
        assert!(!command_resolves("/definitely/not/here/signalhub hook claude-code"));
        assert!(!command_resolves(""));
        assert!(!command_resolves("\"\""), "空引号 = 空程序名");
    }

    /// issue #19 的核心：升级前写的命令（没有 `SIGNALHUB_PID` 前缀）和这一版该写的
    /// 不是同一条 —— 界面必须能提示「重新接入」，否则功能看着在、行为停在旧版本。
    #[test]
    fn a_command_without_the_new_env_prefix_is_stale() {
        assert!(command_stale(
            "signalhub hook claude-code",
            "SIGNALHUB_PID=\"$PPID\" signalhub hook claude-code"
        ));
        // 反过来也一样：判断的是「是不是同一条」，不是「谁比谁新」
        assert!(command_stale(
            "SIGNALHUB_PID=\"$PPID\" signalhub hook claude-code",
            "signalhub hook claude-code"
        ));
    }

    /// 同一条命令（安装过一次、没升级）不能报 stale —— 否则每次打开接入页都在
    /// 骗用户「有更新」。空白差异、引号差异都不算不同。
    #[test]
    fn the_same_command_is_not_stale() {
        let command = "SIGNALHUB_PID=\"$PPID\" signalhub hook claude-code";
        assert!(!command_stale(command, command));
        assert!(!command_stale(
            "  signalhub   hook   claude-code  ",
            "signalhub hook claude-code"
        ));
        // 带引号和不带引号是同一条（shell 里等价）
        assert!(!command_stale(
            "\"signalhub\" hook claude-code",
            "signalhub hook claude-code"
        ));
    }

    /// 环境变量的**值**也算数：只差 pid 变量名的命令不是同一条（那是另一个机制）。
    #[test]
    fn env_values_are_part_of_the_command() {
        assert!(command_stale(
            "SIGNALHUB_PID=\"$PPID\" signalhub hook claude-code",
            "SIGNALHUB_PID=\"$PPID_OTHER\" signalhub hook claude-code"
        ));
        // 普通参数里的 `=` 不是环境变量赋值，仍然按参数比
        assert!(command_stale(
            "signalhub hook claude-code --foo=1",
            "signalhub hook claude-code --foo=2"
        ));
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

        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
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

        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
        let first = fs::read_to_string(&path).unwrap();

        let second = install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
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

        install_at(Agent::ClaudeCode, &HookSpec::Command("/old/place/signalhub hook claude-code".to_string()), None, &path, false)
            .unwrap();

        let report = install_at(Agent::ClaudeCode, &HookSpec::Command("signalhub hook claude-code".to_string()), None, &path, false)
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

        install_at(Agent::ClaudeCode, &HookSpec::Command("/old/signalhub hook claude-code".to_string()), None, &path, false).unwrap();
        install_at(Agent::ClaudeCode, &HookSpec::Command("signalhub hook claude-code".to_string()), None, &path, false).unwrap();

        let root = read(&path);
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "同一个事件只该有一组是我们的");
    }

    #[test]
    fn cursor_command_is_updated_in_place_too() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");

        install_at(Agent::Cursor, &HookSpec::Command("/old/signalhub hook cursor".to_string()), None, &path, false).unwrap();
        let report =
            install_at(Agent::Cursor, &HookSpec::Command("/new/signalhub hook cursor".to_string()), None, &path, false).unwrap();

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

        let report = install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
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

        let err = install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap_err();
        assert!(err.contains("不是合法 JSON"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), broken, "内容必须一字未改");
        assert!(!dir.file("settings.json.signalhub-backup").exists());
    }

    #[test]
    fn install_aborts_when_hooks_is_not_an_object() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        fs::write(&path, r#"{"hooks": ["nonsense"]}"#).unwrap();

        assert!(install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"hooks": ["nonsense"]}"#);
    }

    #[test]
    fn dry_run_changes_nothing() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        let report = install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, true).unwrap();
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
        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();

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
        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();

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
        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();

        assert!(!uninstall_at(&path, false).unwrap().removed.is_empty());
        assert!(uninstall_at(&path, false).unwrap().removed.is_empty());
    }

    #[test]
    fn uninstall_recognizes_a_command_with_a_different_binary_path() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        // 用另一个路径装，卸载时应照样认得出来
        install_at(Agent::ClaudeCode, &HookSpec::Command("/home/someone/bin/signalhub hook claude-code".to_string()), None, &path, false)
            .unwrap();
        uninstall_at(&path, false).unwrap();
        assert!(read(&path).get("hooks").is_none());
    }

    // ---- Cursor 的形状不同 ----

    #[test]
    fn cursor_gets_version_and_a_flat_handler_shape() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");

        install_at(Agent::Cursor, &HookSpec::Command("/opt/signalhub hook cursor".to_string()), None, &path, false).unwrap();
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

        install_at(Agent::Cursor, &HookSpec::Command("/opt/signalhub hook cursor".to_string()), None, &path, false).unwrap();
        assert_eq!(read(&path)["version"], 1);
    }

    #[test]
    fn codex_events_are_pascal_case_and_nested() {
        let dir = TempDir::new();
        let path = dir.file("hooks.json");

        install_at(Agent::Codex, &HookSpec::Command("/opt/signalhub hook codex".to_string()), None, &path, false).unwrap();
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
        // 前面带环境变量赋值时也要认得出来（Claude Code 的进程号就是这么带的）
        assert!(is_our_hook_command(
            r#"SIGNALHUB_PID="$PPID" signalhub hook claude-code"#
        ));
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
        assert_eq!(Agent::parse("opencode").unwrap(), Agent::Opencode);
        assert_eq!(Agent::parse("OpenCode").unwrap(), Agent::Opencode);
        assert!(Agent::parse("emacs").is_err());
    }

    /// `ALL_NAMES` 和 `parse` 必须覆盖同一批名字。少写一个的后果很具体：CLI 的
    /// help 里列不出来（或者反过来，列出来却解析不了）。
    #[test]
    fn every_listed_name_parses() {
        for name in ALL_NAMES {
            assert!(Agent::parse(name).is_ok(), "{name} 列在 ALL_NAMES 里却解析不了");
        }
    }

    // ---- 插件式安装 -----------------------------------------------------------

    /// 分类是穷尽 match：新增一个智能体忘分类**编译不过**。这条测试盯的是另一件事
    /// —— 已经分好类的这 6 家有没有分错。
    /// 插件式的这几家。新增一个插件式智能体时**把它加进来**，下面三条测试
    /// （标记行 / 转义 / clear 守卫）就会自动覆盖它。
    const PLUGIN_AGENTS: [Agent; 3] = [Agent::Opencode, Agent::Pi, Agent::Hermes];

    #[test]
    fn the_two_install_kinds_are_classified_explicitly() {
        for agent in PLUGIN_AGENTS {
            assert_eq!(
                agent.install_kind(),
                InstallKind::PluginFile,
                "{agent:?} 是写自己那份插件的，不该走共享 JSON"
            );
        }
        for agent in [
            Agent::ClaudeCode,
            Agent::Codex,
            Agent::Cursor,
            Agent::Gemini,
            Agent::Goose,
        ] {
            assert_eq!(
                agent.install_kind(),
                InstallKind::JsonHooks,
                "{agent:?} 是往共享 JSON 里插条目的，不该被当成插件"
            );
        }
    }

    /// 插件跑在智能体自己的进程里，所以它能给出**最可靠**的进程号。少了它，
    /// 关掉窗口后灯要沉默半小时才消失（进程还在时则永远显示沉默）。
    #[test]
    fn every_generated_plugin_reports_its_own_process_id() {
        for agent in PLUGIN_AGENTS {
            let source = agent.plugin_source("signalhub");
            let has_pid = source.contains("SIGNALHUB_PID")
                && (source.contains("process.pid") || source.contains("os.getpid()"));
            assert!(has_pid, "{agent:?} 的插件没有上报进程号");
        }
    }

    /// 只有实测过 hook 派生方式的那一家能带 `$PPID`。
    ///
    /// 传错 pid（比如短命的 shell）会被服务端当成「进程没了」立刻收掉会话，
    /// 比现在的「沉默」糟得多 —— 所以没探过的一律不带。
    #[test]
    fn only_claude_codes_hook_command_carries_the_process_id() {
        let claude = resolve_hook_command(Agent::ClaudeCode);
        if cfg!(unix) {
            assert!(
                claude.starts_with("SIGNALHUB_PID=\"$PPID\" "),
                "{claude}"
            );
            assert!(is_our_hook_command(&claude), "带环境变量后仍要认得出来");
        }

        for agent in [
            Agent::Codex,
            Agent::Cursor,
            Agent::Gemini,
            Agent::Goose,
            Agent::Opencode,
        ] {
            assert!(
                !resolve_hook_command(agent).contains("SIGNALHUB_PID"),
                "{agent:?} 还没探过 hook 怎么派生，不该带 pid"
            );
        }
    }

    /// 生成的源码里必须有标记行，而且标记行里的命令能被 `command_resolves` 的
    /// 切词规则完整切出来 —— 路径带空格时这一条最容易翻车。
    #[test]
    fn a_generated_plugin_carries_a_recognizable_marker() {
        for agent in PLUGIN_AGENTS {
            for program in [
                "signalhub",
                "/Applications/SignalHub.app/Contents/MacOS/signalhub",
            ] {
                let source = agent.plugin_source(program);
                let marker = agent.plugin_marker_line(program);
                assert!(source.contains(&marker), "{agent:?} 的源码里没有标记行");

                // 摘回来的命令要能过诊断那一关：切出第一个词（去掉引号）就等于原路径
                let recovered = plugin_command_in(&source).unwrap();
                assert_eq!(first_token(&recovered), Some(program));

                // 「不知道会话 id 就不发 clear」这条守卫：会话名会兜底成 default，
                // 清掉它等于顺手把别的上报方的状态一起抹了。生成的每份源码都要有
                // （JS / TS 一种写法，Python 另一种 —— 语义必须一致）。
                let has_guard = source.contains(r#""clear" && !session"#)
                    || source.contains(r#""clear" and not session"#);
                assert!(has_guard, "{agent:?} 生成的源码里少了 clear 守卫");
            }
        }
    }

    /// opencode 的插件源码必须是 **V2 的默认导出形状**（`{ id, setup }`）。
    ///
    /// 这条回归**用户那头不报错**：文件还在、标记行还在，「接入」页照样显示「已接入」，
    /// 只是 opencode 一起动就报 `Server plugin error` 并且灯永远不亮 —— 所以钉死。
    /// V2 的加载器明说：「Plugin must export a default definition with an id and an
    /// effect or setup function.」
    #[test]
    fn the_opencode_plugin_uses_the_v2_definition_shape() {
        let source = Agent::Opencode.plugin_source("signalhub");
        assert!(
            source.contains("export default {"),
            "opencode 的插件得是 V2 的默认导出"
        );
        assert!(
            source.contains(r#"id: "signalhub""#),
            "V2 定义必须带一个稳定的 id"
        );
        assert!(
            source.contains("async setup(ctx)"),
            "V2 定义必须带 setup"
        );
        assert!(
            !source.contains("export const"),
            "具名导出是 V1 的形状，V2 会拒绝加载整份文件"
        );
        // 只报自己这个目录的会话（opencode 按目录把这份插件实例化多次，每个实例都
        // 收得到整个服务端的事件流）—— 取位置靠这一句。
        assert!(
            source.contains("ctx.session.get"),
            "少了「这个会话属于哪个目录」的回问"
        );
    }

    /// 事件名是照着 opencode 2.0.29 实测写下的。V2 把 V1 的 `event.properties` 和
    /// `session.idle` 都换掉了，猜错的话灯**永远亮着或永远不亮**，而两边都不报错。
    #[test]
    fn the_opencode_plugin_maps_the_events_verified_on_v2() {
        let source = Agent::Opencode.plugin_source("signalhub");
        for event in [
            "session.created",
            "session.execution.started",
            "session.execution.succeeded",
            "session.execution.interrupted",
            "session.execution.failed",
            "permission.asked",
            "permission.replied",
            "session.deleted",
        ] {
            assert!(source.contains(event), "少了实测过的事件 {event}");
        }
        // V1 的事件字段挂在 `event.properties` 上，V2 是 `event.data`
        assert!(source.contains("event?.data"), "事件字段该从 data 上取");
        assert!(
            !source.contains("event?.properties"),
            "properties 是 V1 的事件形状"
        );
    }

    /// Windows 的路径里全是反斜杠。不转义的话 `\U` 会被 JS 当成转义序列 ——
    /// 路径悄悄少几层，插件去执行一个不存在的程序，**双方都不报错**。
    #[test]
    fn backslashes_in_the_path_are_escaped_for_the_plugin_source() {
        for agent in PLUGIN_AGENTS {
            let source = agent.plugin_source(r"C:\Program Files\SignalHub\signalhub.exe");
            // 字面量里必须是双写的（`C:\\Program` 在源码里就是 `C:\\Program`），
            // 不绑死某种语言的赋值语法 —— 三种语言的双引号字符串转义规则一致。
            assert!(
                source.contains(r#""C:\\Program Files\\SignalHub\\signalhub.exe""#),
                "{agent:?} 的源码里反斜杠没有双写"
            );
            // 而标记行里给**我们**看的那份是 shell 写法（原样 + 引号），不是字面量
            assert!(
                source.contains(r#"signalhub-hook: "C:\Program Files\SignalHub\signalhub.exe""#),
                "{agent:?} 的标记行不是 shell 写法"
            );
        }
    }

    /// 判据是**标记行**，不是文件在不在。同名路径上躺着用户自己的插件时，不能
    /// 把「没装」说成「装了」—— 那会让卸载按钮去删一个不是我们建的文件。
    #[test]
    fn a_foreign_plugin_file_is_not_counted_as_installed() {
        assert!(plugin_command_in("export const Mine = () => ({})\n").is_none());
        // 只是提到过这个词，没有真的写成一行标记
        assert!(plugin_command_in("// 想接 SignalHub 的话加一行 signalhub-hook:\n").is_none());
    }

    /// 装卸是一个往返：装完认得出，卸完就认不出，而且**只删我们那个文件**。
    #[test]
    fn installing_a_plugin_writes_a_file_and_uninstalling_removes_it() {
        let dir = TempDir::new();
        let path = dir.file("signalhub.js");

        // 直接测两个内部函数（它们要一个 Path，`install` 那条路会去算 `$HOME`）。
        // 插件源里嵌的是**程序自己**（不带 `hook <agent>` 那一段），所以这里给一个
        // 独立的常量，别拿 JSON 那条路用的 `CMD` 混进来。
        const PROGRAM: &str = "/opt/signalhub";
        let source = Agent::Opencode.plugin_source(PROGRAM);
        crate::config::atomic_write(&path, &source, None).unwrap();
        assert_eq!(plugin_command_in(&source).as_deref(), Some(PROGRAM));
        assert!(path.is_file());

        fs::remove_file(&path).unwrap();
        assert!(!path.is_file());
        assert!(plugin_command_in("").is_none());
    }

    /// Hermes 是**一个目录 + 两份文件**：路径必须是那个目录（卸载时整删），
    /// 不是目录里的某一个文件 —— 否则卸载只删掉一半，留下一份没主人的清单。
    #[test]
    fn a_multi_file_plugin_owns_a_whole_directory() {
        let files = Agent::Hermes.plugin_files("/opt/signalhub").unwrap();
        assert_eq!(files.len(), 2, "Hermes 的插件是清单 + 代码两份");

        let root = crate::config::home_dir()
            .unwrap()
            .join(Agent::Hermes.config_relative_path());
        let expected: Vec<&str> = vec!["plugin.yaml", "__init__.py"];
        for (path, _) in &files {
            assert_eq!(path.parent().unwrap(), root, "两份文件都该在同一个目录下");
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(expected.contains(&name), "意外多出来的文件 {name}");
        }
        // 标记行只在**代码**那份里（清单是固定的、没有任何来自命令的信息）。
        // 检测靠的是「目录里任意一份文件有标记」，所以这仍然认得出来。
        let code = files
            .iter()
            .find(|(path, _)| path.ends_with("__init__.py"))
            .unwrap();
        assert!(code.1.contains(PLUGIN_MARKER), "代码那份里没有标记行");

        // 单文件的家：路径就是根本身，别多套一层
        let single = crate::config::home_dir()
            .unwrap()
            .join(Agent::Opencode.config_relative_path());
        assert_eq!(
            Agent::Opencode.plugin_files("/opt/signalhub").unwrap()[0].0,
            single,
            "单文件的家不该被当成目录"
        );
    }

    /// 目录形状的家，`is_installed` / `installed_command` 要看目录里**任意一份**
    /// 文件找标记 —— 目录整个是我们的，不用记得标记具体写在哪个文件里。
    #[test]
    fn a_marker_inside_a_plugin_directory_is_found() {
        let dir = TempDir::new();
        let root = dir.file("signalhub");
        fs::create_dir_all(&root).unwrap();
        // 清单里没有标记，代码里有
        fs::write(root.join("plugin.yaml"), "name: signalhub\n").unwrap();
        let files = Agent::Hermes.plugin_files("/opt/signalhub").unwrap();
        fs::write(root.join("__init__.py"), &files[1].1).unwrap();

        assert_eq!(
            plugin_command_at(&root).as_deref(),
            Some("/opt/signalhub"),
            "目录里那份代码的标记该被认出来"
        );

        // 整个目录都不含标记 → 不是我们装的
        let other = dir.file("someone-else");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("__init__.py"), "def register(ctx): pass\n").unwrap();
        assert!(plugin_command_at(&other).is_none());
        // 路径根本不存在也不能炸
        assert!(plugin_command_at(&dir.file("nope")).is_none());
    }

    /// 空白的标记行后面什么都没有（这种情况只会来自手改），不算装上了。
    #[test]
    fn a_marker_without_a_command_is_not_installed() {
        assert!(plugin_command_in("// signalhub-hook:   \n").is_none());
    }

    #[test]
    fn backup_is_not_written_when_there_is_nothing_to_back_up() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let report = install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
        assert!(report.backup.is_none(), "新建文件没什么可备份的");
    }

    // ---- HTTP 式接入 --------------------------------------------------------

    /// 写进去的那条 URL 必须是**本机回环**。写成 `0.0.0.0` 或者某个内网地址，就等于
    /// 把状态送给别人；而这个端点连 token 都没有（见 `server::hook_report`）。
    #[test]
    fn http_hooks_point_at_loopback_with_the_safety_fields() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let spec = HookSpec::Http { url: hook_url(41320) };
        install_at(Agent::ClaudeCode, &spec, None, &path, false).unwrap();

        let root = read(&path);
        let handler = &root["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(handler["type"], "http");
        assert_eq!(handler["url"], "http://127.0.0.1:41320/v1/hooks/claude-code");
        // 这两项是这条路上的安全底线：端点不在时**不许**拖住智能体
        // （见 docs/plan-round4.md §9）
        assert_eq!(handler["timeout"], 5);
        assert_eq!(handler["onFailure"], "continue");
        // `async` 是命令式才有的字段，HTTP 式写了只会让人以为它有意义
        assert!(handler.get("async").is_none());
        assert!(handler.get("command").is_none());
    }

    /// 端口是运行时才知道的，配置里写死的是**当下**那个。端口变了（原来的被占用）
    /// 之后，旧 URL 仍要认得出来是「我们写的」—— 否则卸载会留下一条永远打不通的
    /// hook，而用户已经在界面上看到「已卸载」了。
    #[test]
    fn our_handlers_are_recognized_even_after_the_port_changes() {
        let stale = json!({ "type": "http", "url": "http://127.0.0.1:9999/v1/hooks/claude-code" });
        assert!(is_our_handler(&stale));

        // 用户自己的 HTTP hook 不能被认成我们的
        let foreign = json!({ "type": "http", "url": "http://127.0.0.1:9999/my/webhook" });
        assert!(!is_our_handler(&foreign));
    }

    /// 从命令式换成 HTTP 式（以及换回来）必须**整条替换**：留着 `url` 或 `command`
    /// 中的另一份，会得到一条两头不靠的条目，而智能体多半按 `type` 走 —— 那时
    /// 「换回命令方式」看起来成功了，实际还在打 HTTP。
    #[test]
    fn switching_between_command_and_http_replaces_the_handler() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");

        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
        let report = install_at(
            Agent::ClaudeCode,
            &HookSpec::Http { url: hook_url(41320) },
            None,
            &path,
            false,
        )
        .unwrap();
        assert!(!report.updated.is_empty(), "换一种形态算更新，不是新装");
        assert!(report.added.is_empty());

        let handler = read(&path)["hooks"]["PreToolUse"][0]["hooks"][0].clone();
        assert_eq!(handler["type"], "http");
        assert!(handler.get("command").is_none(), "命令式的字段必须清掉: {handler}");
        assert_eq!(installed_http_in(&fs::read_to_string(&path).unwrap()), Some(true));

        install_at(Agent::ClaudeCode, &HookSpec::Command(CMD.to_string()), None, &path, false).unwrap();
        let handler = read(&path)["hooks"]["PreToolUse"][0]["hooks"][0].clone();
        assert_eq!(handler["type"], "command");
        assert!(handler.get("url").is_none(), "HTTP 式的字段必须清掉: {handler}");
        assert!(handler.get("onFailure").is_none());
        assert_eq!(installed_http_in(&fs::read_to_string(&path).unwrap()), Some(false));
    }

    /// 审批那一条写进去的样子：**token / 超时 / onFailure 一个都不能错**。
    ///
    /// 写错超时的表现是「灯上的按钮点不动了」（对方先超时，我们那个等用户点的
    /// 窗口永远走不满）；漏了 `onFailure` 则是「端点不在时把智能体卡住」——
    /// 而那是 §9 那条红线明确不许的。
    #[test]
    fn the_approval_handler_carries_the_token_and_a_longer_timeout() {
        let spec = HookSpec::Approve {
            url: approve_url(41320),
            token: "tok-123".to_string(),
        };
        let body = spec.body(Agent::ClaudeCode);
        assert_eq!(body["type"], "http");
        assert_eq!(body["url"], "http://127.0.0.1:41320/v1/approve/claude-code");
        assert_eq!(body["timeout"], APPROVE_TIMEOUT_SECS);
        // 「比 approvals::WAIT 长」那一条关系由 `approvals.rs` 里的用例钉着
        // （这个文件也被 CLI 编进去，看不到 `approvals` 模块）
        assert_eq!(body["onFailure"], "continue", "端点不在时绝不阻塞智能体");
        assert_eq!(body["headers"]["Authorization"], "Bearer tok-123");
    }

    /// **只有 `PermissionRequest` 那一条换成审批端点**，其余事件照旧走上报端点
    /// （它们不带 token，也不该带）。
    #[test]
    fn only_the_permission_request_entry_becomes_the_approval_endpoint() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let approve = HookSpec::Approve {
            url: approve_url(41320),
            token: "tok-123".to_string(),
        };
        install_at(
            Agent::ClaudeCode,
            &HookSpec::Http { url: hook_url(41320) },
            Some(&approve),
            &path,
            false,
        )
        .unwrap();

        let root = read(&path);
        let permission = root["hooks"]["PermissionRequest"][0]["hooks"][0].clone();
        assert_eq!(permission["url"], "http://127.0.0.1:41320/v1/approve/claude-code");
        assert_eq!(permission["headers"]["Authorization"], "Bearer tok-123");

        let other = root["hooks"]["PreToolUse"][0]["hooks"][0].clone();
        assert_eq!(other["url"], "http://127.0.0.1:41320/v1/hooks/claude-code");
        assert!(other.get("headers").is_none(), "上报端点不该带凭证: {other}");
        assert_eq!(other["timeout"], 5, "上报那一条仍然是 5 秒");

        // 再装一遍（幂等）：不该变成两条
        install_at(
            Agent::ClaudeCode,
            &HookSpec::Http { url: hook_url(41320) },
            Some(&approve),
            &path,
            false,
        )
        .unwrap();
        let root = read(&path);
        assert_eq!(root["hooks"]["PermissionRequest"].as_array().unwrap().len(), 1);
    }

    /// 卸载要把审批那条也清掉 —— **带 token 的条目不能留在用户配置里**。
    #[test]
    fn uninstall_removes_the_approval_handler_too() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let approve = HookSpec::Approve {
            url: approve_url(41320),
            token: "tok-123".to_string(),
        };
        install_at(
            Agent::ClaudeCode,
            &HookSpec::Http { url: hook_url(41320) },
            Some(&approve),
            &path,
            false,
        )
        .unwrap();
        assert!(
            fs::read_to_string(&path).unwrap().contains("tok-123"),
            "装上了就该看得见 token（这一条是给用户排查用的）"
        );

        uninstall_at(&path, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("tok-123"), "卸载之后 token 不该还留在配置里: {text}");
        assert!(!text.contains(APPROVE_PATH_PREFIX), "{text}");
    }

    /// 卸载要把 HTTP 式的一并清掉，而且**只清我们的**。
    #[test]
    fn uninstall_removes_http_handlers_and_keeps_foreign_ones() {
        let dir = TempDir::new();
        let path = dir.file("settings.json");
        let foreign = json!({ "hooks": [{ "type": "http", "url": "http://127.0.0.1:9999/mine" }] });
        fs::write(&path, json!({ "hooks": { "PreToolUse": [foreign] } }).to_string()).unwrap();

        install_at(
            Agent::ClaudeCode,
            &HookSpec::Http { url: hook_url(41320) },
            None,
            &path,
            false,
        )
        .unwrap();
        uninstall_at(&path, false).unwrap();

        let root = read(&path);
        let groups = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "用户自己那条要留下: {root}");
        assert_eq!(groups[0]["hooks"][0]["url"], "http://127.0.0.1:9999/mine");
    }

    /// 没装过、或者只有别人写的东西 → `None`（界面据此显示「未接入」）。
    #[test]
    fn installed_http_is_none_when_nothing_of_ours_is_there() {
        assert_eq!(installed_http_in("{}"), None);
        assert_eq!(installed_http_in("这不是 JSON"), None);
        let foreign = json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "echo hi" }] }] } });
        assert_eq!(installed_http_in(&foreign.to_string()), None);
        // Cursor 那种「组本身就是 handler」的形态也要能看出来
        let ours = json!({ "hooks": { "beforeShellExecution": [{ "command": "/opt/signalhub hook cursor" }] } });
        assert_eq!(installed_http_in(&ours.to_string()), Some(false));
    }

    /// 白名单的匹配：`*` 通配、大小写不敏感，别的一律按字面。
    #[test]
    fn the_allowlist_glob_matches_like_the_documented_one() {
        assert!(glob_match("http://127.0.0.1:*", "http://127.0.0.1:41320/v1/hooks/claude-code"));
        assert!(glob_match("http://127.0.0.1:41320/*", "http://127.0.0.1:41320/v1/hooks/claude-code"));
        assert!(glob_match("*", "http://127.0.0.1:1/x"));
        // 主机名大小写不敏感
        assert!(glob_match("http://LOCALHOST:*", "http://localhost:1/x"));
        // **不匹配就不能算通过**：端口不同的模式、别的路径、别的协议都不行
        assert!(!glob_match("http://127.0.0.1:9999/*", "http://127.0.0.1:41320/v1/hooks/claude-code"));
        assert!(!glob_match("http://localhost:*", "http://127.0.0.1:41320/v1/hooks/claude-code"));
        assert!(!glob_match("https://*", "http://127.0.0.1:41320/v1/hooks/claude-code"));
    }

    /// 白名单的三种情况，逐个钉住 —— 尤其是「有这个键但为空」：那是**封掉全部**
    /// HTTP hook，不是「不限制」。
    #[test]
    fn the_allowlist_reads_as_documented() {
        let url = hook_url(41320);
        assert_eq!(url_allowed("{}", &url), None, "没有这个键 = 不限制");
        assert_eq!(url_allowed(r#"{"allowedHttpHookUrls": []}"#, &url), Some(false));
        assert_eq!(
            url_allowed(r#"{"allowedHttpHookUrls": ["http://127.0.0.1:*"]}"#, &url),
            Some(true)
        );
        assert_eq!(
            url_allowed(r#"{"allowedHttpHookUrls": ["https://hooks.example.com/*"]}"#, &url),
            Some(false)
        );
    }

    /// 其余几家**没有** HTTP 方式：没有可对照的文档事实就不写
    /// （见文件头的「只验证过的映射」）。
    #[test]
    fn http_install_is_refused_for_everybody_but_claude_code() {
        for agent in [Agent::Codex, Agent::Cursor, Agent::Gemini, Agent::Goose] {
            let err = install_http(agent, 41320, true).unwrap_err();
            assert!(err.contains("Claude Code"), "{agent:?}: {err}");
        }
    }
}
