//! 用户自定义的智能体预设（`~/.signalhub/agents.json`）。
//!
//! **为什么要有这个东西**：`Agent` 是一张编译期写死的表，每加一家就要改代码、
//! 发版。而「冷门智能体」的长尾极长 —— 但它们里的大多数是 Claude Code 的克隆
//! （事件名、matcher、stdin 字段全都照抄），差异只有**配置路径和名字**。这一类
//! 完全不需要改代码：让用户自己声明「它和谁同构、配置写哪儿、叫什么」就够了。
//!
//! **能力边界要说清楚**：这个文件只覆盖「格式同构、路径不同」。格式真不一样的
//! （hooks 写在 YAML 里、事件名是另一套）还是得加一个 `Agent` 变体 —— 那不是
//! 配置能解决的问题，是代码。`family` 只接受**往共享 JSON 里插条目**的那几家。
//!
//! **加载策略是「部分成功」**：一条写错不该让整份文件失效。所以 [`load`] 同时返回
//! 认得的条目和读不懂的原因，界面把原因原样摆出来 —— 静默忽略是这类「配置文件」
//! 功能最坏的失败方式：用户改了文件，什么都没发生，也没有任何提示。

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

use crate::hooks::{self, Agent, InstallKind};

/// 一条自定义预设。
///
/// 字段名就是配置文件里的键名，所以刻意保持简短 —— 用户在编辑器里手写它们。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAgent {
    /// 上报用的名字，也是 CLI 的 `--agent` 值（`my-agent`）。
    ///
    /// 归一化成小写 + 连字符，且**不许和内置的十家重名** —— 重名会让「这盏灯是谁
    /// 亮的」永远说不清。
    pub id: String,
    /// 界面上显示的名字（`My Agent`）。
    pub name: String,
    /// 格式家族：和哪一家同构。决定事件表、映射规则、条目形状。
    pub family: Agent,
    /// hook 配置写在哪：相对主目录，或绝对路径。
    pub config: String,
    /// 判断「这台机器上有没有它」的目录痕迹，相对主目录或绝对路径。
    ///
    /// 留空也可以 —— 那就只按 `command` 在 PATH 上找；两个都留空则永远显示
    /// 「没检测到」（但仍然装得动，检测本来就不决定能不能装）。
    pub marker: Option<String>,
    /// PATH 上的命令名，用来判断它装没装。
    pub command: Option<String>,
}

impl CustomAgent {
    /// 配置文件落盘的绝对路径。
    pub fn config_path(&self) -> Result<PathBuf, String> {
        resolve_path(&self.config, "config")
    }

    /// 目录痕迹的绝对路径。
    pub fn marker_path(&self) -> Option<PathBuf> {
        self.marker.as_deref().and_then(|raw| resolve_path(raw, "marker").ok())
    }
}

/// 相对路径按主目录展开；绝对路径原样用。
///
/// **拒绝 `..`**：用户自己机器上的路径本来不该设限，但 `..` 出现在这里几乎总是
/// 手滑（`../../etc/settings.json`），而我们要往那个路径**写文件**。
fn resolve_path(raw: &str, what: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(format!("{what} 不能为空"));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!("{what} 里不能有 `..`：{raw}"));
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let home = crate::config::home_dir().ok_or_else(|| "找不到用户主目录".to_string())?;
    Ok(home.join(path))
}

/// 一次加载的结果。
#[derive(Debug, Default)]
pub struct Loaded {
    /// 认得的条目，顺序与文件里一致。
    pub agents: Vec<CustomAgent>,
    /// 读不懂的地方，每条一句话，带上是第几条 —— 界面原样摆出来。
    pub problems: Vec<String>,
}

impl Loaded {
    /// 按 id 找一个。`id` 应当已经是归一化过的（见 [`normalize_id`]）。
    pub fn find(&self, id: &str) -> Option<&CustomAgent> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

/// 配置文件的位置：`~/.signalhub/agents.json`。
///
/// 和 `settings.json` 同一个目录（[`crate::config::data_dir`]）—— 「配置都在
/// `~/.signalhub` 里」是用户已经建立起来的直觉，另起一处只会让人找不到。
pub fn file_path() -> Result<PathBuf, String> {
    crate::config::data_dir()
        .map(|dir| dir.join("agents.json"))
        .map_err(|err| format!("找不到配置目录: {err}"))
}

/// 读一次。文件不存在**不是错误** —— 大多数用户不会有这个文件。
pub fn load() -> Loaded {
    let Ok(path) = file_path() else {
        return Loaded::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Loaded::default(),
        Err(err) => Loaded {
            agents: Vec::new(),
            problems: vec![format!("读不了 {}：{err}", path.display())],
        },
    }
}

/// 按 id 找一条自定义预设。`id` 会先归一化。
pub fn find(id: &str) -> Option<CustomAgent> {
    let id = normalize_id(id);
    load().find(&id).cloned()
}

/// 这个 id 属于哪一家的**格式**：内置的按内置算，自定义的看它的 `family`。
///
/// CLI 的 `signalhub hook <id>` 拿它决定用哪张映射表 —— 内置与自定义在这里合流，
/// 于是映射表本身**一行都不用改**。
///
/// **别因为「app 里没人调用」就删掉**：本文件同时被 CLI 用 `#[path]` 编进自己的
/// 二进制（见 `bin/signalhub.rs` 开头），那边的 `hook` 路径每次工具调用都要走这里。
/// 两份编译单元的使用面不同，而 CI 是 `-D warnings` —— 只对这一个函数放行，
/// 比给整个模块盖 `#[allow]` 更能挡住以后真正的死代码。
#[allow(dead_code)]
pub fn family_of(id: &str) -> Option<Agent> {
    let id = normalize_id(id);
    if let Ok(agent) = Agent::parse(&id) {
        return Some(agent);
    }
    find(&id).map(|agent| agent.family)
}

/// id 的规范形式：小写、下划线换连字符、去掉首尾空白。
///
/// 和 `Agent::parse` 的归一化**同一套规则** —— 两处不一致的话，`My_Agent` 会在
/// 一边认得、另一边认不得。
pub fn normalize_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('_', "-")
}

/// 能当 `family` 用的几家：**只收「往共享 JSON 里插 hooks 条目」的那一类**。
///
/// 从 `ALL_NAMES` 推出来而不是另写一份清单 —— 加一家内置的，这里自动多一个可选项。
pub fn families() -> Vec<Agent> {
    hooks::ALL_NAMES
        .iter()
        .filter_map(|name| Agent::parse(name).ok())
        .filter(|agent| hooks::install_kind(*agent) == InstallKind::JsonHooks)
        .collect()
}

/// 一条预设在文件里的样子。`serde` 只用来**读**，写模板用 [`example`]。
#[derive(Deserialize)]
struct RawAgent {
    id: String,
    name: String,
    family: String,
    config: String,
    #[serde(default)]
    marker: Option<String>,
    #[serde(default)]
    command: Option<String>,
}

/// 解析一份配置内容。纯函数，能单测。
///
/// 顶层既接受 `{"agents": [...]}` 也接受裸数组 —— 手写的时候少一层括号总是好的，
/// 而且两种写法在别的工具里都见过。
pub fn parse(text: &str) -> Loaded {
    let mut out = Loaded::default();
    if text.trim().is_empty() {
        return out;
    }

    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            out.problems.push(format!("不是合法 JSON：{err}"));
            return out;
        }
    };
    // 顶层：对象取 `agents`，数组当整份清单
    let list = match &value {
        serde_json::Value::Array(_) => value.clone(),
        serde_json::Value::Object(map) => match map.get("agents") {
            Some(agents) => agents.clone(),
            // 有对象但没有 `agents`：多半是用户直接写了一条（少了外层）
            None => {
                out.problems
                    .push("顶层是对象，但里面没有 \"agents\" 数组".to_string());
                return out;
            }
        },
        _ => {
            out.problems
                .push("顶层既不是对象也不是数组".to_string());
            return out;
        }
    };

    let entries: Vec<RawAgent> = match serde_json::from_value(list) {
        Ok(entries) => entries,
        Err(err) => {
            out.problems.push(format!("agents 里有条目读不懂：{err}"));
            return out;
        }
    };

    let families = families();
    let mut seen: Vec<String> = Vec::new();

    for (index, raw) in entries.into_iter().enumerate() {
        // 报错带序号，用户才知道去改第几条（从 1 数起，和编辑器对得上）
        let at = format!("第 {} 条", index + 1);
        match validate(raw, &families) {
            Ok(agent) => {
                if seen.contains(&agent.id) {
                    out.problems
                        .push(format!("{at}：id \"{}\" 和前面某一条重复了", agent.id));
                    continue;
                }
                seen.push(agent.id.clone());
                out.agents.push(agent);
            }
            Err(reason) => out.problems.push(format!("{at}：{reason}")),
        }
    }

    out
}

/// 校验并归一化一条。返回的 `id` 已经过 [`normalize_id`]。
fn validate(raw: RawAgent, families: &[Agent]) -> Result<CustomAgent, String> {
    let id = normalize_id(&raw.id);
    if id.is_empty() {
        return Err("id 不能为空".to_string());
    }
    if id.len() > 32 {
        return Err(format!("id 太长（{} 个字符，上限 32）：{id}", id.len()));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
    {
        return Err(format!("id 只能用字母、数字、`.` 和 `-`：{id}"));
    }
    // 和内置重名会让「这盏灯是谁亮的」永远说不清 —— 而内置的那几家人人都可能
    // 真装上，不是理论上的冲突。
    if Agent::parse(&id).is_ok() {
        return Err(format!("id \"{id}\" 和内置的一家重名了，换一个"));
    }

    let name = raw.name.trim();
    if name.is_empty() {
        return Err("name 不能为空".to_string());
    }

    let family = Agent::parse(&raw.family).map_err(|_| {
        format!(
            "不认识的 family \"{}\"；只能是 {}（插件式的几家暂不支持）",
            raw.family,
            families
                .iter()
                .map(|agent| hooks::cli_name(*agent))
                .collect::<Vec<_>>()
                .join(" / ")
        )
    })?;
    if !families.contains(&family) {
        return Err(format!(
            "\"{}\" 是插件式接入，暂不支持自定义；family 只能是 {}",
            raw.family,
            families
                .iter()
                .map(|agent| hooks::cli_name(*agent))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }

    // 路径先验一遍：错在这里比错在「点了接入才报错」友好得多
    resolve_path(&raw.config, "config")?;
    if let Some(marker) = raw.marker.as_deref() {
        resolve_path(marker, "marker")?;
    }
    let command = raw.command.as_deref().map(str::trim).filter(|c| !c.is_empty());
    if let Some(command) = command
        && command.chars().any(char::is_whitespace)
    {
        return Err(format!("command 只能是命令名（不带参数、不带空格）：{command}"));
    }

    Ok(CustomAgent {
        id,
        name: name.chars().take(48).collect(),
        family,
        config: raw.config.trim().to_string(),
        marker: raw
            .marker
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string),
        command: command.map(str::to_string),
    })
}

/// 新建文件时写进去的内容。
///
/// **带一个 `_example` 而不是直接给一条真条目**：直接给真条目的话，用户第一次
/// 打开这个文件就会发现 SignalHub 已经在提示接入一个他根本没有的 `my-agent`。
/// 顶层的未知键一律忽略，所以 `_example` 纯粹是写在文件里的说明书 —— 要用的时候
/// 把它挪进 `agents` 数组即可。
pub fn example() -> String {
    r#"{
  "_说明": "把 _example 里那份抄进 agents 数组，改成你的智能体的路径即可。字段说明见 docs/api.md §4.18。",
  "_example": {
    "id": "my-agent",
    "name": "My Agent",
    "family": "claude-code",
    "config": ".my-agent/settings.json",
    "marker": ".my-agent",
    "command": "my-agent"
  },
  "agents": []
}
"#
    .to_string()
}

/// 想写进去的一条预设。**写入专用的输入**：和读文件用的 [`RawAgent`] 分开，
/// 因为写入还要额外回答「同 id 已存在时要不要覆盖」。
#[derive(Debug, Clone)]
pub struct AddRequest {
    pub id: String,
    pub name: String,
    pub family: String,
    pub config: String,
    pub marker: Option<String>,
    pub command: Option<String>,
}

/// 一次写入到底做了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteOutcome {
    Added,
    /// 同一条已经在了 —— 幂等，连文件都不碰
    Unchanged,
    /// 同 id 但内容不同，显式要求覆盖
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddResult {
    /// 归一化之后的 id（调用方传进来的可能是 `My_Agent`）
    pub id: String,
    pub outcome: WriteOutcome,
    /// 写到了哪个文件，回执里要说清楚
    pub path: String,
}

/// 一条预设写进文件时的样子。可选字段留空就**不写这一行** —— 手写时它们最容易
/// 被抄成空串，而加载器虽然认空串，文件里平白多两行只会让人以为必填。
fn entry_value(agent: &CustomAgent) -> serde_json::Value {
    let mut entry = serde_json::Map::new();
    entry.insert("id".into(), serde_json::Value::String(agent.id.clone()));
    entry.insert("name".into(), serde_json::Value::String(agent.name.clone()));
    entry.insert(
        "family".into(),
        serde_json::Value::String(hooks::cli_name(agent.family).to_string()),
    );
    entry.insert("config".into(), serde_json::Value::String(agent.config.clone()));
    if let Some(marker) = &agent.marker {
        entry.insert("marker".into(), serde_json::Value::String(marker.clone()));
    }
    if let Some(command) = &agent.command {
        entry.insert("command".into(), serde_json::Value::String(command.clone()));
    }
    serde_json::Value::Object(entry)
}

/// 把一条预设合并进已有的文件内容。**纯函数**，所有分支都能单测。
///
/// `forbidden` 是「不许把 hook 配置指向的目录」（SignalHub 自己的数据目录）；
/// 传 `None` 表示不检查，测试里用它避开真实文件系统。
pub fn insert(
    existing: Option<&str>,
    request: &AddRequest,
    replace: bool,
    forbidden: Option<&Path>,
) -> Result<(String, WriteOutcome), String> {
    let families = families();
    let raw = RawAgent {
        id: request.id.clone(),
        name: request.name.clone(),
        family: request.family.clone(),
        config: request.config.clone(),
        marker: request.marker.clone(),
        command: request.command.clone(),
    };
    // 和 load() 走同一份 validate：写进去的东西必须立刻就能被读出来，不能
    // 「命令报成功、接入页却摆一条错误」。
    let agent = validate(raw, &families)?;

    // hook 配置是**别人的**文件。让它指向 SignalHub 的数据目录，等于允许一次
    // `install hooks` 往 config.json / token / agents.json 里插条目 —— 那条路上
    // 写的是我们的配置，不是智能体的。手写文件时这只能靠文档提醒，命令入口
    // 就该把它变成硬约束。
    if let Some(dir) = forbidden {
        let target = agent.config_path()?;
        if target.starts_with(dir) {
            return Err(format!(
                "config 不能指向 SignalHub 的数据目录（{}）：{}。请填你自己的 hook 配置文件路径",
                dir.display(),
                agent.config
            ));
        }
    }

    // 读不懂的旧文件绝不覆盖：一次「添加」不能悄悄抹掉用户手写的全部条目。
    let mut root: serde_json::Value = match existing {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text)
            .map_err(|err| format!("现有的 agents.json 不是合法 JSON，不会覆盖它：{err}"))?,
        _ => serde_json::from_str(&example())
            .map_err(|err| format!("内置模板不是合法 JSON：{err}"))?,
    };

    // 保持原有形态：裸数组仍是裸数组，对象仍带它原来的 `_说明` / `_example`。
    // 按 Value 改而不是反序列化成 Vec<CustomAgent> 再重写，是为了**原样保留**
    // 暂时读不懂的条目和任何未知键 —— 它们也是用户写的东西。
    let list: &mut Vec<serde_json::Value> = match &mut root {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(map) => match map.get_mut("agents") {
            Some(serde_json::Value::Array(items)) => items,
            Some(_) => return Err("顶层 \"agents\" 不是数组，不会覆盖它".to_string()),
            None => return Err("顶层是对象但没有 \"agents\" 数组，不会覆盖它".to_string()),
        },
        _ => return Err("顶层既不是对象也不是数组，不会覆盖它".to_string()),
    };

    let position = list.iter().position(|item| {
        item.get("id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| normalize_id(id) == agent.id)
    });

    let outcome = if let Some(index) = position {
        // 旧条目也过一遍同样的校验；只有「读得懂 + 完全一样」才算幂等。读不懂的
        // 旧条目不能当成「已存在」放过去 —— 那会把一条坏配置永远留在文件里。
        let same = serde_json::from_value::<RawAgent>(list[index].clone())
            .ok()
            .and_then(|old| validate(old, &families).ok())
            .is_some_and(|old| old == agent);
        if same {
            return Ok((existing.unwrap_or_default().to_string(), WriteOutcome::Unchanged));
        }
        if !replace {
            return Err(format!(
                "id \"{}\" 已经存在，而且内容不一样。确认要覆盖：面板里勾「覆盖同名条目」后重提，CLI 里加 --replace。",
                agent.id
            ));
        }
        list[index] = entry_value(&agent);
        WriteOutcome::Replaced
    } else {
        list.push(entry_value(&agent));
        WriteOutcome::Added
    };

    let text = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("序列化 agents.json 失败：{err}"))?;
    Ok((format!("{text}\n"), outcome))
}

/// 校验并**追加**一条预设。CLI 的 `add-agent` 和 app 的 `add_custom_agent` 共用
/// 这一份 —— 两边走同一套校验和 `atomic_write`，不会出现「终端写的和界面写的
/// 格式不一样」。
///
/// **并发边界**：这是 read-modify-write，`atomic_write` 的锁只管本进程；两个进程
/// 同一瞬间各加一条会丢一条。这个入口的用法是人（或智能体）一条一条地加，不是
/// 高频写入。真要并发得先加跨进程文件锁。
pub fn add(request: AddRequest, replace: bool) -> Result<AddResult, String> {
    let path = file_path()?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("读不了 {}：{err}", path.display())),
    };
    // data_dir() 顺带保证目录存在；它同时是「不许把 hook 写进来」的那个目录。
    let dir = crate::config::data_dir()
        .map_err(|err| format!("找不到配置目录: {err}"))?;
    let (text, outcome) = insert(existing.as_deref(), &request, replace, Some(&dir))?;
    if outcome != WriteOutcome::Unchanged {
        crate::config::atomic_write(&path, &text, None)
            .map_err(|err| format!("写 {} 失败: {err}", path.display()))?;
    }
    Ok(AddResult {
        id: normalize_id(&request.id),
        outcome,
        path: path.to_string_lossy().into_owned(),
    })
}

/// 文件不在就建一份带说明的空文件。返回它的路径。
pub fn ensure_file() -> Result<PathBuf, String> {
    let path = file_path()?;
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| format!("建目录失败: {err}"))?;
        }
        crate::config::atomic_write(&path, &example(), None)
            .map_err(|err| format!("写 {} 失败: {err}", path.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_entry_is_enough() {
        let loaded = parse(
            r#"{"agents":[{"id":"My_Agent","name":"My Agent","family":"claude-code",
                "config":".my-agent/settings.json"}]}"#,
        );
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        let agent = &loaded.agents[0];
        // id 归一化：和 `Agent::parse` 同一套规则
        assert_eq!(agent.id, "my-agent");
        assert_eq!(agent.family, Agent::ClaudeCode);
        assert_eq!(agent.marker, None);
        assert_eq!(agent.command, None);
    }

    #[test]
    fn the_family_decides_the_format() {
        let loaded = parse(
            r#"[{"id":"x","name":"X","family":"workbuddy","config":".x/settings.json"}]"#,
        );
        assert_eq!(loaded.agents[0].family, Agent::WorkBuddy);
    }

    #[test]
    fn plugin_style_families_are_rejected_with_a_reason() {
        // opencode 是插件式：往一个共享 JSON 里插条目这件事对它不成立，
        // 所以宁可在加载时说清楚，也不要装出一个它永远不会加载的文件。
        let loaded = parse(
            r#"[{"id":"x","name":"X","family":"opencode","config":".x/settings.json"}]"#,
        );
        assert!(loaded.agents.is_empty());
        assert!(loaded.problems[0].contains("插件式"), "{:?}", loaded.problems);
    }

    #[test]
    fn a_collision_with_a_builtin_is_refused() {
        // 重名会让「这盏灯是谁亮的」永远说不清
        let loaded = parse(
            r#"[{"id":"claude-code","name":"X","family":"claude-code","config":".x/a.json"}]"#,
        );
        assert!(loaded.agents.is_empty());
        assert!(loaded.problems[0].contains("重名"), "{:?}", loaded.problems);
        // 别名也算重名 —— 它同样是「内置的那一家」
        let alias = parse(
            r#"[{"id":"claude","name":"X","family":"claude-code","config":".x/a.json"}]"#,
        );
        assert!(alias.agents.is_empty(), "{:?}", alias.problems);
    }

    #[test]
    fn one_bad_entry_does_not_hide_the_good_ones() {
        let loaded = parse(
            r#"{"agents":[
                {"id":"good","name":"Good","family":"claude-code","config":".good/s.json"},
                {"id":"bad","name":"Bad","family":"nope","config":".bad/s.json"}
            ]}"#,
        );
        assert_eq!(loaded.agents.len(), 1, "好的一条要留下");
        assert_eq!(loaded.agents[0].id, "good");
        assert_eq!(loaded.problems.len(), 1);
        assert!(loaded.problems[0].starts_with("第 2 条"), "{:?}", loaded.problems);
    }

    #[test]
    fn duplicate_ids_keep_the_first() {
        let loaded = parse(
            r#"{"agents":[
                {"id":"dup","name":"A","family":"claude-code","config":".a/s.json"},
                {"id":"DUP","name":"B","family":"codex","config":".b/s.json"}
            ]}"#,
        );
        assert_eq!(loaded.agents.len(), 1);
        assert_eq!(loaded.agents[0].name, "A");
        assert!(loaded.problems[0].contains("重复"), "{:?}", loaded.problems);
    }

    #[test]
    fn paths_cannot_climb_out() {
        // `..` 几乎总是手滑，而我们要往那个路径**写文件**
        let loaded =
            parse(r#"[{"id":"x","name":"X","family":"claude-code","config":"../../etc/s.json"}]"#);
        assert!(loaded.agents.is_empty());
        assert!(loaded.problems[0].contains(".."), "{:?}", loaded.problems);
    }

    #[test]
    fn a_broken_file_says_so_instead_of_going_quiet() {
        let loaded = parse("{ 这不是 JSON");
        assert!(loaded.agents.is_empty());
        assert!(loaded.problems[0].contains("JSON"), "{:?}", loaded.problems);
    }

    #[test]
    fn an_empty_file_is_not_an_error() {
        let loaded = parse("   ");
        assert!(loaded.agents.is_empty());
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    }

    #[test]
    fn the_template_carries_no_real_agent() {
        // 模板里那条是 `_example`，加载器不看它 —— 否则用户第一次打开文件就会
        // 看到 SignalHub 在提示接入一个他根本没有的智能体。
        let loaded = parse(&example());
        assert!(loaded.agents.is_empty());
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        // 但把它抄进 agents 就能用 —— 说明书里那份必须是**能直接跑**的
        let copied = example().replace("\"agents\": []", "\"agents\": [{ \"id\": \"my-agent\", \"name\": \"My Agent\", \"family\": \"claude-code\", \"config\": \".my-agent/settings.json\" }]");
        assert_eq!(parse(&copied).agents.len(), 1);
    }

    #[test]
    fn families_are_derived_from_the_builtin_list() {
        let families = families();
        assert!(families.contains(&Agent::ClaudeCode));
        assert!(families.contains(&Agent::WorkBuddy));
        // 插件式的三家不在里面
        assert!(!families.contains(&Agent::Opencode));
        assert!(!families.contains(&Agent::Pi));
        assert!(!families.contains(&Agent::Hermes));
    }

    #[test]
    fn the_id_normalizer_matches_the_one_agents_use() {
        // 两处归一化必须一致，否则 `My_Agent` 会在一边认得、另一边认不得。
        // 用内置的名字验：`Agent::parse` 自己也做同一套小写 + 下划线换连字符。
        for raw in ["Claude_Code", " claude-code ", "CLAUDE-CODE"] {
            assert_eq!(normalize_id(raw), "claude-code");
            assert!(Agent::parse(&normalize_id(raw)).is_ok());
        }
        // 非内置的照样归一化，只是认不出是哪一家 —— 那正是自定义预设的入口
        assert_eq!(normalize_id("My_Agent"), "my-agent");
        assert!(Agent::parse("my-agent").is_err());
    }

    // ---- 写入（insert 是纯函数，文件 I/O 留在 add 里）------------------------

    fn request() -> AddRequest {
        AddRequest {
            id: "my-agent".into(),
            name: "My Agent".into(),
            family: "claude-code".into(),
            config: ".my-agent/settings.json".into(),
            marker: None,
            command: None,
        }
    }

    #[test]
    fn insert_creates_a_file_the_reader_accepts() {
        let (text, outcome) = insert(None, &request(), false, None).unwrap();
        assert_eq!(outcome, WriteOutcome::Added);
        assert!(text.contains("_说明"), "新文件仍该带那份说明书");
        let loaded = parse(&text);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded.agents.len(), 1);
        assert_eq!(loaded.agents[0].id, "my-agent");
        // 可选字段留空就不写进条目（模板的 `_example` 里另有一份，别数错）
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(root["agents"][0].get("marker").is_none());
        assert!(root["agents"][0].get("command").is_none());
    }

    #[test]
    fn insert_appends_instead_of_overwriting() {
        let existing = r#"{"_注释":"keep me","agents":[
            {"id":"first","name":"First","family":"codex","config":".first/s.json"}]}"#;
        let (text, outcome) = insert(Some(existing), &request(), false, None).unwrap();
        assert_eq!(outcome, WriteOutcome::Added);
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(root["_注释"], "keep me", "未知键要原样留着");
        let list = root["agents"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], "first");
        assert_eq!(list[1]["id"], "my-agent");
        assert!(parse(&text).problems.is_empty(), "原有条目还得读得懂");
    }

    #[test]
    fn a_bare_array_stays_a_bare_array() {
        let existing = r#"[{"id":"first","name":"First","family":"codex","config":".first/s.json"}]"#;
        let (text, _) = insert(Some(existing), &request(), false, None).unwrap();
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(root.is_array(), "别把用户选的形态改掉");
        assert_eq!(root.as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_broken_file_is_never_overwritten() {
        let err = insert(Some("{ 这不是 JSON"), &request(), false, None).unwrap_err();
        assert!(err.contains("不会覆盖"), "{err}");
        let err = insert(Some(r#"{"_example":{}}"#), &request(), false, None).unwrap_err();
        assert!(err.contains("不会覆盖"), "{err}");
    }

    #[test]
    fn repeating_the_same_request_is_a_no_op() {
        let (first, outcome) = insert(None, &request(), false, None).unwrap();
        assert_eq!(outcome, WriteOutcome::Added);
        let (second, outcome) = insert(Some(&first), &request(), false, None).unwrap();
        assert_eq!(outcome, WriteOutcome::Unchanged);
        assert_eq!(first, second, "连文本都不重写，更不该碰文件");
        // 归一化过的 id 也算同一条
        let upper = AddRequest { id: "My_Agent".into(), ..request() };
        let (_, outcome) = insert(Some(&first), &upper, false, None).unwrap();
        assert_eq!(outcome, WriteOutcome::Unchanged);
    }

    #[test]
    fn a_different_entry_with_the_same_id_needs_explicit_replace() {
        let (first, _) = insert(None, &request(), false, None).unwrap();
        let changed = AddRequest { name: "Renamed".into(), ..request() };
        let err = insert(Some(&first), &changed, false, None).unwrap_err();
        assert!(err.contains("--replace"), "{err}");
        let (replaced, outcome) = insert(Some(&first), &changed, true, None).unwrap();
        assert_eq!(outcome, WriteOutcome::Replaced);
        let loaded = parse(&replaced);
        assert_eq!(loaded.agents.len(), 1, "替换而不是再追加一条重复 id");
        assert_eq!(loaded.agents[0].name, "Renamed");
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    }

    #[test]
    fn writing_goes_through_the_same_validation_as_reading() {
        for bad in [
            AddRequest { family: "nope".into(), ..request() },
            AddRequest { family: "opencode".into(), ..request() },
            AddRequest { id: "claude-code".into(), ..request() },
            AddRequest { config: "../../etc/s.json".into(), ..request() },
            AddRequest { command: Some("my agent".into()), ..request() },
        ] {
            assert!(insert(None, &bad, false, None).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn config_cannot_point_into_the_data_directory() {
        let dir = std::env::temp_dir().join("signalhub-data-for-test");
        let bad = AddRequest {
            config: dir.join("config.json").to_string_lossy().into_owned(),
            ..request()
        };
        let err = insert(None, &bad, false, Some(&dir)).unwrap_err();
        assert!(err.contains("数据目录"), "{err}");
        // 目录外的绝对路径照样能写 —— 这条约束不是「不许用绝对路径」
        let ok = AddRequest {
            config: std::env::temp_dir()
                .join("elsewhere/settings.json")
                .to_string_lossy()
                .into_owned(),
            ..request()
        };
        assert!(insert(None, &ok, false, Some(&dir)).is_ok());
    }

    #[test]
    fn optional_fields_are_written_when_present() {
        let full = AddRequest {
            marker: Some(".my-agent".into()),
            command: Some("my-agent".into()),
            ..request()
        };
        let (text, _) = insert(None, &full, false, None).unwrap();
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(root["agents"][0]["marker"], ".my-agent");
        assert_eq!(root["agents"][0]["command"], "my-agent");
        assert!(parse(&text).problems.is_empty(), "{:?}", parse(&text).problems);
    }
}
