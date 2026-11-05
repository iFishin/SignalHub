import type { Lang } from "./i18n";
import { REPORTABLE_SIGNALS } from "./types";

/**
 * 一段**给别的智能体看的**接入提示词。
 *
 * 痛点：第一次用的人打开「接入」页，如果自己用的智能体不在那十家里，眼前只有
 * 「打开配置文件」和一个 JSON 例子 —— 得先读 docs、判断自己跟哪一家同构、再手写
 * 字段。而这件事恰好是**那个智能体自己最清楚**的：它知道自己的 hook 配置在哪、
 * 事件叫什么、stdin 传什么。所以把说明反过来写给 agent，让人只做「复制 + 粘贴」。
 *
 * **这里不存任何真值。** 内置清单、可用 family、配置路径、数据目录、端口全部由
 * 调用方从现有 IPC 传进来（`detect_agents` / `custom_agents_info` / `get_endpoint`）。
 * 抄一份常量到这个文件里，就意味着加一家适配时要记得改两处 —— 而这个项目的
 * 接入清单本来只存在于 Rust 的 `hooks::ALL_NAMES` 一处。同理状态词表直接用
 * `REPORTABLE_SIGNALS`，不另写一份。唯一写死的是 `signalhub add-agent` 这个
 * CLI 子命令名：它是对外接口而不是运行时数据，改它本来就要同步文档与提示词。
 *
 * **也不接收 token。** 提示词只告诉 agent 去 `<dir>/token` 读，不把凭据明文写进
 * 一段会被粘进聊天框、日志和 shell 历史的文本里。
 */
export interface OnboardingContext {
  lang: Lang;
  /** 内置适配的 id：`detect_agents` 里 `custom === false` 的那些 */
  builtin: string[];
  /** 能当 `family` 用的名字：`custom_agents_info` 的 `families`（Rust 从 ALL_NAMES 推出） */
  families: string[];
  /** `~/.signalhub/agents.json` 的绝对路径 */
  agentsFile: string | null;
  /** `~/.signalhub`，token / port / hooks.log 都在里面 */
  dataDir: string | null;
  /** 当前上报端口。null 时提示词改让 agent 去读 `port` 文件 */
  port: number | null;
}

/** 数据还没从 IPC 回来（或不在 Tauri 里）时的说法。**不编造清单**，只让人去界面上看。 */
const FALLBACK_BUILTIN_ZH = "以 SignalHub「接入」页列出的为准";
const FALLBACK_BUILTIN_EN = "see the list on SignalHub's Connect page";
const FALLBACK_FAMILIES_ZH = "以 SignalHub「接入」页「自定义智能体」一节列出的为准";
const FALLBACK_FAMILIES_EN = "see Custom agents on SignalHub's Connect page";
/** 只在拿不到真实目录时用；拼子文件名必须用它本身，不能带后面那句 Windows 说明。 */
const FALLBACK_DIR = "~/.signalhub";

function orFallback(values: string[], fallback: string): string {
  const clean = values.filter((v) => v.trim().length > 0);
  return clean.length > 0 ? clean.join(" / ") : fallback;
}

/** Windows 传进来的是反斜杠路径 —— 拼子文件名时跟着它，别混出 `C:\...\dir/port`。 */
function under(dir: string, name: string): string {
  return dir.includes("\\") ? `${dir}\\${name}` : `${dir}/${name}`;
}

function factPaths(ctx: OnboardingContext) {
  const rawDir = ctx.dataDir || FALLBACK_DIR;
  // 真实路径已经自带平台信息；只有回退到 `~/.signalhub` 时才补那句 Windows 说明。
  const dir = ctx.dataDir ? rawDir : `${rawDir} (Windows: %USERPROFILE%\\.signalhub)`;
  return {
    dir,
    agentsFile: ctx.agentsFile || under(rawDir, "agents.json"),
    portFile: under(rawDir, "port"),
    tokenFile: under(rawDir, "token"),
    logFile: under(rawDir, "hooks.log"),
    /** 取不到就留占位符：正文里另有一句告诉 agent 去 `port` 文件读，不必伪造成 shell 命令 */
    port: ctx.port != null ? String(ctx.port) : "<port>",
  };
}

/**
 * 生成提示词。纯函数，可直接单测；调用方只负责把结果丢进剪贴板。
 *
 * 顺序是刻意的：先让 agent 确认 CLI 在不在（不在 PATH 是真实会卡住的第一步）、
 * 再判断是不是内置（是就一条命令结束，不浪费它读后面的 JSON）、然后才谈自定义，
 * 最后给「格式不通」的兜底。任何一步不确定都要求它**说出来而不是猜** —— 猜错
 * family 的后果是写进一份看起来完全正常、但永远不报状态的 hook。
 */
export function buildOnboardingPrompt(ctx: OnboardingContext): string {
  return ctx.lang === "en" ? english(ctx) : chinese(ctx);
}

function chinese(ctx: OnboardingContext): string {
  const p = factPaths(ctx);
  const builtin = orFallback(ctx.builtin, FALLBACK_BUILTIN_ZH);
  const families = orFallback(ctx.families, FALLBACK_FAMILIES_ZH);
  const states = REPORTABLE_SIGNALS.join(" / ");

  return [
    "帮我接入 SignalHub —— 一个在桌面上用一盏灯显示各智能体工作状态的小应用。它已经装好，并且正在运行。",
    "",
    "我不知道你（也就是我当前正在使用的这个智能体）在不在它的内置适配清单里。请你自己查清楚并完成配置，不要反过来让我去翻文档。",
    "",
    "【第 0 步 · 先确认 CLI 能用】",
    "执行 `command -v signalhub`（Windows PowerShell 用 `Get-Command signalhub`）。",
    `SignalHub 的数据目录是 ${p.dir}，port / token / hooks.log 都在里面。`,
    "找不到就先停在这里：CLI 还没加进 PATH，请直接告诉我，并提示我去 SignalHub 的 README「接入智能体」一节按平台把它加进去。下面的命令先别硬跑。",
    "",
    "【第 1 步 · 你可能在清单里】",
    `内置适配的 id 是：${builtin}。`,
    "如果你正是其中一家（或它的别名），执行：",
    "    signalhub install hooks <你的 id>",
    "想先看会改什么可以加 `--dry-run`。然后告诉我「重启你就生效了」，到这里就结束，不用往下读。",
    "",
    "【第 2 步 · 不在清单里，判断 hook 格式是否同构】",
    "查你自己的配置文档和现有配置文件，确认三件事：",
    "  1. 你的 hook 配置是不是一份 JSON、里面有没有 `hooks` 这个键；",
    "  2. 你的事件名、条目形状、以及传给 hook 命令的 stdin JSON 字段，跟内置哪一家写法一致；",
    "  3. 配置在用户主目录下（推荐，全局生效）还是只在某个项目里。",
    "「同构」指的是机制一样、只是路径和名字不同。如果你的配置是 YAML、事件名完全是另一套、或者你根本不支持 hook，跳到最后的【兜底】。",
    "",
    "【第 3 步 · 登记一条自定义预设】",
    "不要直接编辑 agents.json —— 用命令写入。它会校验字段、原样保留已有条目和未知键、原子替换文件，重复执行也不会写重：",
    "    signalhub add-agent --id <id> --name \"<界面显示名>\" --family <family> --config \"<配置文件路径>\" [--marker \"<目录痕迹>\"] [--command <命令名>]",
    "各参数：",
    "  - --id 小写连字符，上报时的名字，也是 --agent 的值；",
    `  - --family 和哪一家同构，只能是：${families}；`,
    "  - --config 你自己的 hook 配置文件路径，相对主目录或绝对路径；",
    "  - --marker / --command 可选，用来判断这台机器上装没装你。",
    "命令会替你保证这些（写错会直接报错并说明原因，不要绕过它去手改文件）：",
    "  - id 只允许字母、数字、`.`、`-`，最长 32 字符，且不能和内置 id 或它的别名重名；",
    "  - family 只能是上面那几个 —— 插件式接入的那几家不支持自定义；",
    `  - --config 不能含 \`..\`，也不能指向 SignalHub 的数据目录（${p.dir}），必须是你自己的配置文件；`,
    "  - 同一个 id 已存在且内容不同时命令会拒绝；确认要改才加 --replace，重复执行同一条则不会有任何改动；",
    "  - 自定义条目只支持命令式 hook，不要尝试 `--http` 或 `--approve`。",
    `写入成功后这条预设立刻生效（SignalHub 每次读取都会重读 ${p.agentsFile}），不用重启；切回设置窗会自动刷新出这一行。`,
    "如果你完全不能执行 shell 命令，就告诉我：回到 SignalHub「接入」页点「打开配置文件」，照文件里的 `_example` 手写一条，然后我来点那一行的「接入」。但优先走命令 —— 手改最容易覆盖已有条目。",
    "",
    "【第 4 步 · 装 hook 并验证】",
    "执行 `signalhub install hooks <你写的 id>`。",
    "它只会往 config 指向的那份文件的 `hooks` 键里插入 `signalhub hook <id>`，其余键不动，写前自动备份，重复执行不会插入重复条目。",
    "验证：",
    "  - `signalhub diagnose` 看诊断结果；",
    "  - 提醒我重启你（hook 是启动时加载的），再随便跑一次工具调用；",
    `  - SignalHub「接入」页对应那一行应出现「最近上报」回执。没有就看 ${p.logFile}，或用接入页的「复制诊断信息」。`,
    "",
    "【兜底 · 格式不同 / 没有 hook / 装不上】",
    "任何不需要知道你 hook 格式的路都行：",
    `  - 直接 POST http://127.0.0.1:${p.port}/v1/state，带 Authorization: Bearer <token>，请求体形如 {"state":"working","session":"<会话 id>","message":"<一句话>"}；`,
    `  - 或下次启动时改用 \`signalhub run --agent <name> -- <启动你的命令>\` 包一层，由它替你上报（包不住一个已经在跑的进程）。`,
    `token 从 ${p.tokenFile} 读，端口从 ${p.portFile} 读；不要把凭据写进代码或提交进仓库。状态词：${states}。`,
    "",
    "【完成后告诉我】",
    "你走了哪一条路、命令写到了哪个文件、装 hook 成没成功、我还需要做什么（通常只是重启你）。",
    "不确定的地方不要猜：尤其 `family` 拿不准时，把候选和理由列出来让我确认 —— 猜错的结局是一份看起来完全正常、却永远不报状态的 hook。",
    "不要修改 SignalHub 自己的 config.json；也不要绕过 add-agent 直接编辑 agents.json，或改动上面提到的文件之外的东西。",
  ].join("\n");
}

function english(ctx: OnboardingContext): string {
  const p = factPaths(ctx);
  const builtin = orFallback(ctx.builtin, FALLBACK_BUILTIN_EN);
  const families = orFallback(ctx.families, FALLBACK_FAMILIES_EN);
  const states = REPORTABLE_SIGNALS.join(" / ");

  return [
    "Help me connect to SignalHub, a small desktop app that shows what each coding agent is doing with a lamp. It is installed and running.",
    "",
    "I do not know whether you (the agent I am talking to right now) are on its built-in adapter list. Figure that out and do the setup yourself; do not send me off to read the docs.",
    "",
    "Step 0 - Check the CLI first",
    "Run `command -v signalhub` (on Windows PowerShell: `Get-Command signalhub`).",
    `SignalHub's data directory is ${p.dir}; port, token and hooks.log live there.`,
    "If it is missing, stop here: the CLI is not on PATH yet. Tell me so and point me at the 'Wiring up an agent' section of SignalHub's README for the platform-specific step. Do not run the commands below.",
    "",
    "Step 1 - You may already be supported",
    `Built-in adapter ids: ${builtin}.`,
    "If you are one of them (or one of their aliases), run:",
    "    signalhub install hooks <your id>",
    "Add `--dry-run` to preview the change. Then tell me that restarting you is all that is left, and stop reading here.",
    "",
    "Step 2 - Not on the list: check whether your hook format is compatible",
    "Look at your own config docs and existing config files and determine:",
    "  1. whether your hooks live in a JSON file under a `hooks` key;",
    "  2. which built-in family uses the same event names, entry shape and stdin JSON fields;",
    "  3. whether the config is user-global (preferred) or project-local.",
    "'Compatible' means the mechanism is the same and only the path and name differ. If your config is YAML, uses a different set of events, or has no hook support at all, skip to the Fallback section.",
    "",
    "Step 3 - Register a custom preset",
    "Do not edit agents.json by hand. Use the command: it validates the fields, preserves existing entries and unknown keys, writes atomically, and is idempotent:",
    "    signalhub add-agent --id <id> --name \"<display name>\" --family <family> --config \"<config file path>\" [--marker \"<directory>\"] [--command <name>]",
    "Arguments:",
    "  - --id: lowercase with hyphens; the reported name and the --agent value;",
    `  - --family: which family you mirror; one of: ${families};`,
    "  - --config: path to your own hook config, relative to home or absolute;",
    "  - --marker / --command: optional hints for detecting whether you are installed.",
    "The command enforces these (an invalid value fails loudly; do not work around it by editing the file):",
    "  - id: letters, digits, `.`, `-` only; at most 32 chars; must not collide with a built-in id or alias;",
    "  - family: only the ones listed above; the three plugin-style adapters cannot be used for custom entries;",
    `  - --config must not contain '..' or point into SignalHub's data directory (${p.dir}); it must be your own config file;`,
    "  - an existing id with different contents is refused; add --replace only if you mean to overwrite. Re-running the same command changes nothing;",
    "  - custom entries are command-style hooks only; do not try `--http` or `--approve`.",
    `Once written, the preset is live immediately (SignalHub re-reads ${p.agentsFile} on every lookup); no restart needed. Switching back to the settings window refreshes the row.`,
    "If you cannot run shell commands at all, tell me to open the config file from SignalHub's Connect page, copy its `_example` by hand, and let me click Install on that row. Prefer the command though - hand editing is the easiest way to overwrite existing entries.",
    "",
    "Step 4 - Install the hooks and verify",
    "Run `signalhub install hooks <the id you wrote>`.",
    "It only inserts `signalhub hook <id>` into the `hooks` key of the file named by config, leaves every other key alone, backs the file up first, and is idempotent.",
    "Verify:",
    "  - `signalhub diagnose`;",
    "  - tell me to restart you (hooks load at startup) and run one tool call;",
    `  - the matching row on SignalHub's Connect page should show a recent-report receipt. If not, check ${p.logFile} or use 'Copy diagnostics' on that page.`,
    "",
    "Fallback - different format / no hooks / install fails",
    "Any route that does not need to know your hook format works:",
    `  - POST directly to http://127.0.0.1:${p.port}/v1/state with Authorization: Bearer <token> and a body like {"state":"working","session":"<session id>","message":"<short text>"};`,
    "  - or on the next launch, start yourself through `signalhub run --agent <name> -- <the command that starts you>`, which reports on your behalf (it cannot wrap a process that is already running).",
    `Read the token from ${p.tokenFile} and the port from ${p.portFile}; never commit credentials or paste them into code. States: ${states}.`,
    "",
    "When you are done, tell me",
    "Which route you took, which file the command wrote, whether the hook install succeeded, and what is left for me (usually just restarting you).",
    "Do not guess: if `family` is uncertain, list the candidates with your reasoning and let me pick. A wrong guess produces a hook that looks perfectly fine and never reports anything.",
    "Do not edit SignalHub's own config.json, do not bypass add-agent by editing agents.json directly, and do not touch files outside the ones named above.",
  ].join("\n");
}
