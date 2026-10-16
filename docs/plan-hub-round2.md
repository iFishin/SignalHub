# 信号中枢第二轮升级计划（写给实施的智能体）

> 2026-10 起草。四个需求，全部来自用户实测后的反馈：
>
> 1. **智能体一键接入覆盖更多家** —— Hermes / Pi / opencode 等热门 Agent 都没有一键接入按钮。
> 2. **状态灯要能删除** —— 智能体被突然关掉，灯却还挂着，看着难受。
> 3. **展开的信号中枢上要能直接改标签名** —— 现在只能绕到设置里改。
> 4. **通知转发「疑似不可用且鸡肋」** —— 要调研别人怎么做，改得真能用。
>
> 本文按「可以直接开工」的标准写（同 `plan-lamp-expansion.md`）：每项给目标、方案、
> 锚点、坑、验收。**行号是起草时的快照，会漂移 —— 以函数名 / 常量名为锚。**

---

## 0. 先读这一节

### 0.1 沿用旧计划的七条红线

`plan-lamp-expansion.md` §0.2 的七条全部继续有效。与本轮最相关的四条，摘抄如下：

1. **几何是两处真值**（Rust 的 `sign_width` / `sign_height` / `collapsed_window_size`
   与 `lamp.css` 的 `calc(var(--size) * …)`）。R3 要在牌上挂新控件，**R3 的每一处
   CSS 尺寸都必须同步 Rust**，或者说 —— 更省事的做法是：**新控件不许改变牌的宽高**。
   见 §3.2 的方案（绝对定位，不参与 flex 布局）。
2. **常驻窗口的动画只动 opacity**（`lamp.css` 文件头）。R2 / R3 的悬停显隐也只能动
   `opacity`（可以加 `visibility`，别再动 transform）。
3. **新能力默认关**，开关写进 `Settings`（红线 4 的四件事：`Default` 给对值 +
   `clamped()` 夹紧 + 前端 `DEFAULTS` 同步 + 导出/导入往返测试）。
4. **给 `Settings` 加字段靠容器级 `#[serde(default)]`**，别再逐字段标。

### 0.2 本轮新增的两条

8. **R1 生成的插件文件是「我们的文件」，但跑在别人的进程里。** 三条硬约束：
   - 只写**我们自己**的路径（`hooks.rs` 的 `plugin_dir` 思路），绝不合并 / 改写用户的
     共享配置。Hermes / Pi / opencode 三家都有「自动加载一个目录下所有文件」的机制，
     这就是为什么选它们而不是别的家（见 §1.1 调研结论）。
   - 生成的代码里**不许有阻塞调用**：不能 `execFileSync`、不能 `await`。用户开着
     `--watch` 跑 CI 的时候，智能体的每一轮工具调用都会打我们一次；卡一下就是真卡住。
   - 生成的代码里**必须有一行标记**（`signalhub-hook: <命令>`），安装判据、
     「命令还解析得到吗」的诊断、卸载都靠它，不靠解析别人的格式。
9. **出站转发宁可「少发」也不能「发成别的意思」。** 一条转发失败只许写 stderr 和
   日志，不许影响状态机；一条钩子失败不许影响另一条（这是 `outbound.rs` 文件头
   已有的约束，R4 只是不破坏它）。

### 0.3 现状速查（开工前先看这几行）

| 事实 | 位置 |
|---|---|
| 出站转发的**通路是通的**（`post` 真把 JSON 发出去了，非 2xx 会带状态码回来） | `outbound.rs::post`；本轮已补两条端到端测试 |
| 出站转发只在**聚合跃迁**或**某组跃迁**时触发 | `notify.rs::compose`（`transitioned \|\| !group_changes.is_empty()`）|
| 出站执行结果**不落盘**，重启后界面显示「从未触发」 | `outbound.rs` 的 `OutboundLog`（内存环形缓冲）|
| 组的清状态已经有：右键菜单「清除该组的状态」 | `menu.rs::clear_group`（`CLEAR`）|
| 删除单个会话已经有：HTTP `DELETE /v1/sessions/{id}` | `server.rs` / `state.rs::remove_session` |
| 改名已经有：设置 →「外观」→「各组的显示」 | `SettingsPanel.tsx::setGroupMeta`（`group_meta`）|
| 智能体接入已有 5 家：全部「往共享 JSON 里插一条 hooks」 | `hooks.rs`（`claude-code` / `codex` / `cursor` / `gemini` / `goose`）|

---

## 1. R1 智能体一键接入：再加三家（opencode / Pi / Hermes）

### 1.1 调研结论（决定「选谁」和「怎么装」）

**只选「有一个自己的插件目录、启动时自动加载」的家。** 理由：往共享配置里插东西
必须解析用户的文件（JSON 能整份重写所以还行，YAML 会丢注释和格式 —— 那是不可接受的
数据损坏），而插件目录里的文件**全是我们自己建的**，卸载就是删文件，误伤的代价为零。
这是 `hooks.rs` 文件头那三条安全底线的自然延伸。

| Agent | 装在哪（**我们的文件**） | 机制 | 每家的证据 |
|---|---|---|---|
| **opencode** | `~/.config/opencode/plugins/signalhub.js` | 插件目录自动加载；`event` 流 + 具名 hook | 官方 Plugins 文档：「Place JavaScript or TypeScript files in the plugin directory … are automatically loaded at startup」 |
| **Pi** | `~/.pi/agent/extensions/signalhub.ts` | 扩展目录自动发现；`pi.on(event, cb)` | 官方 extensions.md：「Extensions are auto-discovered from `~/.pi/agent/extensions/*.ts`」 |
| **Hermes** | `~/.hermes/plugins/signalhub/{plugin.yaml,__init__.py}` | 插件目录；`ctx.register_hook()` | 官方 plugins 文档：「Drop a directory into `~/.hermes/plugins/` with a `plugin.yaml` and Python code」 |

三家都**不需要装任何依赖**（opencode 插件用 `node:child_process`；Pi 扩展同理；
Hermes 插件用标准库 `subprocess`），也都不需要我们改它们的配置。

**刻意不做的几家**（写进 `docs/api.md` 的「已知边界」，别让下一个人再调研一遍）：

- **Droid（Factory）/ Qwen Code / iFlow**：它们都有 `hooks.json` / `settings.json` 这一
  类**共享 JSON 配置**，形状看着像 Claude Code 那一族，但**事件名我们没有逐条核实过**。
  接进来的方式是复用现有的 JSON 路径（几乎零成本），但必须先把事件名核实到官方文档
  级别。**留给下一轮**，本轮不猜。
- **VS Code 系（Cline / Roo / Continue / Windsurf / Kilo）**：没有「一个目录丢文件就
  自动加载」的扩展点，接入需要给用户装一个 VS Code 扩展 —— 那不是「一键」。
  用户手里已经有 `signalhub run`（docs/api.md §4.13）这条通用路。

### 1.2 方案：在 `hooks.rs` 里加「安装方式」这一层

现在 `install` / `uninstall` / `is_installed` / `installed_command` 全都**假定**
「读一份 JSON、往 `hooks` 键里插条目」。加一个新枚举，把这层抽象出来：

```rust
/// 我们往对方那儿放东西的**方式**。不是「哪个 agent」，而是「怎么装」——
/// 三家插件式的新 agent 共用同一份实现，`events()` 那种逐 agent 的差异完全不参与。
enum InstallKind {
    /// 往用户共享的一份 JSON 里插 `hooks` 条目（Claude Code / Codex / Cursor / Gemini）。
    /// Goose 也走这条：它的 hooks.json 形状相同，只是额外多一个插件清单。
    JsonHooks,
    /// 往对方自动加载的插件目录里放**我们自己的文件**。不需要解析任何已有的内容，
    /// 卸载 = 删文件。`file()` 给出要写的文件名。
    PluginFile { file: &'static str },
}

impl Agent {
    fn install_kind(self) -> InstallKind { … }

    /// 插件式安装要写的文件内容（把解析好的命令嵌进去）。
    /// 纯函数：**输入只有 `command`，没有环境**，所以能单测。
    fn plugin_source(self, command: &str) -> &'static str { … }  // 见 §1.3

    /// 生成的源码里的标记行。安装判据、诊断、卸载都只认它。
    fn plugin_marker(self) -> &'static str { … }
}
```

改造点（**逐个函数，别漏**）：

- `install`：`JsonHooks` 走原路；`PluginFile` → 建目录 + `atomic_write(file, source)`
  （复用 `crate::config::atomic_write`；Goose 那份清单的写法就是先例）。
  `Report.added` 填 `["plugin"]`（界面只显示「已安装 / 事件列表」，不关心具体形状）。
  演练模式（`dry_run`）**不落盘**，只回报 path。
- `uninstall`：`PluginFile` → 删文件；**目录空了才删目录**（Hermes 的
  `plugins/signalhub/` 是我们独有的，可以整删，照 Goose 的先例）。
- `is_installed`：`PluginFile` → 文件存在 **且** 内容里有 `plugin_marker()`。
- `installed_command`：`PluginFile` → 从标记行后面摘出命令
  （`installed_command_in` 是 JSON 版，新增一个 `plugin_command_in(text, marker)`，
  同样做成**纯函数**以便单测）。
- `detect` / `command_names` / `marker_dir` / `ALL_NAMES`（`[&str; 5]` → `[&str; 8]`）
  / `parse` 的别名 / `name()`。
- **`events()` 对三家返回空**（它们是插件式，不用逐事件清单）。`events()` 的返回值在
  `install_at` 里被遍历 —— 所以必须保证 `PluginFile` **不走到** `install_at`。

`marker_dir` / `command_names` 取值：

| Agent | `marker_dir` | `command_names` | `name()` |
|---|---|---|---|
| opencode | `.config/opencode` | `["opencode"]` | `opencode` |
| pi | `.pi/agent` | `["pi"]` | `Pi` |
| hermes | `.hermes` | `["hermes"]` | `Hermes` |

### 1.3 三份生成源码（要点，实施时按这个写）

**共同点**：顶部一行标记 `// signalhub-hook: <命令>`（Hermes 用 `#`），下面一个
`report(state, session, cwd, message?)` 帮助函数，派生一个**不等待**的子进程：

```
signalhub <state> --agent <名字> --session <id> --cwd <目录> [--message <文本>]
```

`state == "clear"` 时改调 `signalhub clear --agent <名字> --session <id>` —— 复用现有
CLI，**不为任何一家新增 payload 映射**（这是选「命令行 + flags」而不是「给 CLI 喂
对方格式的 JSON」的理由：`bin/signalhub.rs::map_event` 那张表不用动一个字）。

**opencode**（JS，ESM）：

```js
import { spawn } from "node:child_process"
// signalhub-hook: <命令>
const CLI = "<命令>", AGENT = "opencode"
function report(state, session, cwd, message) { /* spawn + unref，stdin/out/err 全 ignore */ }

export const SignalHub = async ({ directory }) => ({
  event: async ({ event }) => {
    const p = event?.properties ?? {}
    const session = p.sessionID ?? p.info?.id ?? ""
    switch (event?.type) {
      case "session.created":   return report("idle", session, directory)
      case "permission.asked":  return report("permission", session, directory)
      case "permission.replied":return report("working", session, directory)
      case "session.error":     return report("error", session, directory, p.error?.message)
      case "session.idle":      return report("done", session, directory)
      case "session.deleted":   return report("clear", session, directory)
      default: return
    }
  },
  "tool.execute.before": async (input) => report("working", input?.sessionID ?? "", directory),
  "tool.execute.after":  async (input) => report("working", input?.sessionID ?? "", directory),
})
```

- **`session.status` 不用**：它的 payload 形状（`busy` / `idle` 的确切字段）不在官方
  Plugins 文档里，猜错会得到一个「永远在做某事」的灯。宁可少一个 `thinking` 态。
- `permission.replied` → `working`：用户点了「允许」，那一轮继续跑。
- 没有 `thinking`。**这是刻意的**：宁缺勿错，`docs/api.md` 里写清楚。

**Pi**（TS 扩展，jiti 加载，无编译）：

```ts
import type { ExtensionAPI } from "@mariozechner/pi-coding-agent"
import { spawn } from "node:child_process"
// signalhub-hook: <命令>

export default function (pi: ExtensionAPI) {
  const sid = (ctx: any) => String(ctx?.sessionManager?.getSessionFile?.() ?? "")
  pi.on("session_start", (_e, ctx) => report("idle", sid(ctx), ctx?.cwd))
  pi.on("before_agent_start", (_e, ctx) => report("thinking", sid(ctx), ctx?.cwd))
  pi.on("tool_execution_start", (_e, ctx) => report("working", sid(ctx), ctx?.cwd))
  pi.on("agent_end", (_e, ctx) => report("done", sid(ctx), ctx?.cwd))
  pi.on("session_shutdown", (_e, ctx) => report("clear", sid(ctx), ctx?.cwd))
}
```

- `import type` 在 jiti 里**被整个擦掉**（不做类型检查），所以没装
  `@mariozechner/pi-coding-agent` 也不会炸；`ctx` 用 `any` 接住，不吃版本差异。
- 会话 id 用 `sessionManager.getSessionFile()` 的**路径**：它是每次会话唯一的，
  且 `/new` `/resume` 之后会变 —— 正好对应「一个会话一块牌」。
- **不订 `message_update` / `turn_end`**：那是最热的路径，插件跑一次就打一次进程，
  没必要。

**Hermes**（Python 插件，标准库）：**一个目录 + 两份文件**
（`plugin.yaml` + `__init__.py`），所以机制要支持「多文件」这一形状（实施时的补充，
见 §1.6）。

```python
# signalhub-hook: <命令>
def register(ctx):
    ctx.register_hook("on_session_start", lambda session_id="", **kw: report("idle", session_id))
    ctx.register_hook("pre_llm_call",    lambda session_id="", **kw: report("thinking", session_id))
    ctx.register_hook("pre_tool_call",   lambda session_id="", **kw: report("working", session_id))
    ctx.register_hook("post_llm_call",   lambda session_id="", **kw: report("done", session_id))
    ctx.register_hook("on_session_end",  lambda session_id="", **kw: report("done", session_id))
```

- **`on_session_end` 映射成 `done` 而不是 `clear`** —— 这条是调研里最反直觉的一处，
  必须写在注释里：Hermes 的 `on_session_end` 是「每一次 `run_conversation()` 结束」
  （也就是**每一轮**）都发，不是进程退出才发。映射成 `clear` 会让灯**每轮结束后
  闪没一次**。清理交给 TTL（`stale`）。
- **会话 id 只认 `session_id`，绝不拿 `task_id` 顶替**（起草时写的是 `task_id`，实施
  时在 Hermes 源码里核到它是 `effective_task_id = task_id or str(uuid.uuid4())`，
  **每轮都重来**；用它会在灯上给同一场会话每轮多出一块牌。而 `pre_tool_call` 的调用点
  其实**也传了 `session_id`**，只是没写进文档的参数表）。
- Hermes 没有「需要授权」事件，同 Gemini：只能拿 `post_tool_call` 的结果串猜，本轮不做。
- 工作目录取插件进程的 `os.getcwd()`：hook 参数里没有它，但插件就在那个项目里跑。
- `plugin.yaml` 内容固定、无用户输入。

### 1.4 验收（R1）

- `cargo test`：新增
  - `plugin_source` 的三种输出都**含标记行 + 命令**，且命令里带空格时被引号包住
    （`/Applications/Signal Hub.app/…` 这种路径必须能跑）；
  - `plugin_command_in` 从一份生成的源码里摘回原命令（往返）；
  - `is_installed` 的判据是**标记行**：一份同名的、没有标记行的用户文件不算装上
    （这是「不许把用户自己的插件当成我们装的」那条防线的测试）；
  - `InstallKind` 分类：三家是 `PluginFile`，五家老的是 `JsonHooks`（穷尽 match，
    新增 agent 忘分类编译不过）。
- 演练模式不写盘：`install(agent, true)` 之后文件**不存在**。
- 手工验收（写进 `docs/api.md`）：三家各装一遍、跑一个真实会话，看灯亮；
  `~/.signalhub/hook.log` 里能看到入站记录。
- 前端**不用改**：`detect_agents` 读的是 `ALL_NAMES`，新三家自动出现在「接入」页。

### 1.6 实施时对起草的三处修正

- Hermes 是「一个目录 + 两份文件」，所以 `InstallKind::PluginFile` 从「写一个文件」
  扩成「写一组文件」（`plugin_files`）。判据 / 卸载仍然是**看那个路径自己是什么**：
  文件就删文件、目录就整删 —— 不多加一张表。
- Hermes 的会话 id 用 `session_id`，**不是** `task_id`（见 §1.3 里那段）。
- 三家都补上「不知道会话 id 时绝不发 `clear`」这条守卫：会话名会兜底成 `default`，
  清掉它等于顺手把别的上报方的状态一起抹了。这条守卫有测试盯着三份源码。

### 1.5 拆成 3 个 commit

1. `R1-a` 机制 + opencode：`InstallKind` + `plugin_source` + 单测 + `docs/api.md` §4.15。
2. `R1-b` Pi。
3. `R1-c` Hermes + 已知边界。

**机制和第一个 agent 合并成一个提交**（原计划是分开的）：只有机制而没有 agent 的提交
既没有用户可见的效果，也没法端到端验证 —— 机制的正确性恰恰要靠「一份真实生成的源码」
来证明。opencode 就是那份源码。

---

## 2. R2 删除卡住的状态

### 2.1 目标与边界

用户原话：「状态卡住的情况，比如我突然关闭了某个智能体会话，但是信号灯还是存在的」。

**已有的**：右键菜单「清除该组的状态」（清掉该组**全部**会话）、HTTP
`DELETE /v1/sessions/{id}`。所以缺的不是能力，是**可发现性**和**粒度**：

- 能力藏在原生右键菜单里，展开的牌上看不出能删；
- 只有「清整组」，没有「清一个会话」—— 一组里三个会话、卡住一个时只能全清。

### 2.2 方案

**R2-1 牌上直接删（组粒度）**：展开态的每块牌，悬停时在右上角露出一个 `×`。
点它 = 走**和右键菜单同一条**逻辑（`menu::clear_group`）。

- 把 `menu.rs::clear_group` **上移到 `lib.rs`** 变成 `pub(crate) fn clear_group`
  （菜单和命令两条入口必须走同一个函数，理由同 `set_group_muted` 那条注释），
  `menu.rs` 改成调用它。
- 新增命令 `clear_group_state(group: String)`，前端 `invoke` 它。
- **不做二次确认**：这是低频、幂等的操作，而且智能体下次上报就会回来 ——
  加确认框反而让「想快速清掉」这件事变慢。用 `title` 说清楚后果。
- **不动设置**：`group_meta` / `hidden_groups` / `muted_groups` 一律保留。
  「删灯」是清状态，不是抹掉用户给这一组起的名 / 置的顶。

**R2-2 会话粒度删**：设置 →「状态」页每个会话行尾加一个 `×`，走新命令
`clear_session(session: String)` → `Store::remove_session`（HTTP `DELETE` 用的就是它）。

- `remove_session` 返回 `(是否移除了, 可见状态是否变化)`，和 `clear_group` 一样
  要**变了才推前端**（`notify::push`）。

### 2.3 坑

- 「没变就不推」：`clear` 一个不存在的会话必须**静默幂等**（HTTP 那条已经在依赖这个
  语义，别在命令里改成报错）。
- 牌上的 `×` **不许改变牌的宽高**（红线 1）：绝对定位，绝对不参与 flex。见 §3.2。
- `×` 的点击必须 `stopPropagation`，否则会冒到 `.lamp-root` 的
  `useDragOrClick` → 变成「折叠/展开」。
- 占位牌（`rows` 里那块 `name === ""` 的）不渲染 `×`：它不对应任何真实的组。

### 2.4 验收

- `cargo test`：`Store::remove_session` 的两条已有语义不回归（移除不存在的会话 →
  `removed=false`、`changed=false`）。
- 前端 `npm test`：给 `lib/lampActions.ts`（若有纯函数）或组件补一条 —— 若没有纯函数
  可测，就用「占位牌不渲染 ×」这条断言。
- 手工：跑一个假上报 → 灯亮 → 点牌上的 `×` → 灯变空态；设置→状态页删单个会话。

---

## 3. R3 在展开的信号中枢上直接改标签名

### 3.1 目标

用户原话：「希望用户可以直接在展开的信号中枢上修改标签名称」。现在只能设置 →
「外观」→「各组的显示」，离「我看到这个名字不对」的场景太远。

### 3.2 方案

**悬停出两枚牌尾小动作**（`✎` 改名 / `×` 删除，R2 的那一枚），点 `✎` 之后
`.sign-name` 原地变成 `<input>`。

- **绝对定位挂在牌右上角**，`position: absolute` + 牌 `position: relative`；
  容器 `pointer-events: none`，两枚按钮自己 `auto`。这样**牌的宽高一个像素都不变**
  —— 红线 1 在这条上唯一的免死金牌是「不参与布局」，用上它。
- 显隐**只动 `opacity`**（红线 2）。
- **不用双击改名**：双击是两次 `click`，会触发两次 `toggle_collapsed`（净效果是
  「闪一下、什么都没变」，很怪）；而且灯的任意位置单击已经有明确含义。
- 编辑态：`<input defaultValue={label}>`，`autoFocus` + 全选。
  - `Enter` / 失焦 → 提交；`Esc` → 取消。
  - 提交：`updateSettings({...settings, group_meta: next})`，`next` 的算法
    **逐字照抄** `SettingsPanel.tsx::setGroupMeta`（空值 = 删除该条目，两样都空就丢）。
    两处算法不一致就会出现「灯上改完在设置里看着不一样」。
  - 输入框的 `onMouseDown` / `onClick` 必须 `stopPropagation`，否则一按就开始拖窗口。
- `show.name === false`（用户关掉了牌名）时**没有名字可点**：这时不渲染 `✎`，
  并在 tooltip 里说明去设置里改。**不要**为了改名把用户关掉的字段强行画出来。
- 改名的对象是**当前渲染这一块的 `group.name`**（原始组名），不是显示名 ——
  `group_meta` 的键是原始组名，这条错了就会给「`build-1`」建一条叫 `build-1` 的
  显示名、而牌上还是原来那个名字。

**顺手做**：设置 →「外观」那一节已经有改名输入框，两处都用同一个纯函数
（`src/lib/groupMeta.ts` 的 `withGroupName(meta, group, name)`），别写第二份。

### 3.3 坑

- `updateSettings` 是**整份替换**：必须用灯这一帧拿到的 `settings` 展开，不能只传
  `{group_meta}`（`set_settings` 会把缺失字段回落到默认值 —— 那等于把所有设置重置）。
- 灯窗失焦（点了别的窗口）时输入框会收到 `blur` → 提交。这是想要的（和 macOS 各处
  一致），但**空输入框不该把名字清掉**：`blur` 提交时若值没变就直接退出编辑态。
- i18n：`renameHint`（「点击改名」）、`renamePlaceholder`；tooltip 里那两句同样要
  zh/en 两份，`Messages` 是穷尽的（漏一条 TS 就报错）。
- 折叠态没有牌面 → 没有 `✎`。文档里说明「展开后再改」。

### 3.4 验收

- `npm test`：`withGroupName` 的纯函数测试（改名 / 清空 = 删条目 / 保留 `order`）。
- 手工：展开灯 → 悬停 → `✎` → 改名 → 回车；退出重开仍在（落盘了）；设置里同名同值。

### 3.5 补做：编辑框预填「此刻显示的名字」

**用户原话**：「我发现这个名称好像没有自动探测，这个能自动探测然后填入吗？」
—— 探测是做了的（`state.rs::session_label` 一直在算「目录末段 · 智能体」），
问题在于**点开改名的框是空的**：框里填的是「用户自定义名」那个字段本身，没改过名
时它是空的，探测出来的名字只作为灰色 `placeholder` 挂在后面。于是想微调一个字，
也得把「SignalHub · opencode」整个重敲一遍。

现在两条入口（灯上就地改名、设置 →「外观」→「各组的显示」的输入框）都在**打开的
那一刻**把 `labelOf` 的三级结果（用户起的 → 探测出来的 → 原始组名）填进框里。

**连带的坑：判「没动过」的依据必须跟着变。** 原先拿「提交值 == 已存的自定义名」
判；框里既然可能预填着一个探测值，「点进来又点回去」就会被当成一次改名，把一个
探测出来的名字**钉成**用户自定义名 —— 之后上报方的项目 / 智能体变了，牌上还挂着
旧的那个。判据因此抽成 `lib/groupMeta.ts::nameChanged(seed, next, current)`，两条
入口共用、有单测。清空的语义不变（= 回到跟随自动）。

---

## 4. R4 通知转发：从「能做」到「好用」

### 4.1 调研结论（先说清楚「哪些是真的坏了」）

**通路没坏。** 本轮补的两条端到端测试（`outbound.rs`）证明：`post` 把 JSON 发出去了、
非 2xx 会带着状态码回来。所以「疑似不可用」不是管线问题，是下面四条**体验**问题：

1. **没有测试按钮**：用户配完只能等下一次真实跃迁。等不到就永远不知道是「没触发」
   还是「发失败」—— 这就是「疑似」的来源。
2. **body 是 SignalHub 自己的 JSON schema**：`{"snapshot":{…},"at_ms":…}`。用户照着
   占位符 `https://ntfy.sh/my-topic` 填一条 ntfy，收到的是一大坨 JSON —— ntfy 能收，
   但读者根本不知道看了什么。**绝大多数服务（Slack / Discord / Telegram / 飞书 /
   企业微信 / 钉钉 / Bark）都吃不了这个形状**，所以「转发」看起来什么都没发生。
3. **不能带请求头**：任何要鉴权的地址（`Authorization: Bearer …`、ntfy 的
   `X-Token`）都配不了。
4. **结果不落盘**：`OutboundLog` 是内存环形缓冲，重启后界面显示「从未触发」——
   用户重启一次就以为它坏了。

参考别家做法（`git's` / `npm` 的 hook 都是「跑命令」，而**通知类**做得实用的是
ntfy / Bark / Slack incoming webhook 那一族：**一个 URL + 一行文字**，服务端自己
决定怎么展示）。所以本轮的主题是：**让人能在 30 秒内把状态发到手机上，并且能当场
验证它通了。**

### 4.2 方案

**R4-1 `HookAction::Http` 加两个可选字段**（都 `#[serde(default)]`，老配置不受影响）：

```rust
Http {
    url: String,
    /// 额外请求头。鉴权靠它（`Authorization` / ntfy 的 `X-Token`）。
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// 服务预设：决定 content-type 和 body 形状。`raw` = 现状（我们的 JSON）。
    #[serde(default)]
    preset: HttpPreset,
}
```

`HttpPreset`：`raw`（默认）/ `text` / `ntfy` / `bark` / `slack` / `discord` /
`lark` / `wecom` / `dingtalk` / `telegram`。每个预设就是一个**纯函数**：

```rust
/// 组装一条转发请求。**纯函数**：输入 (预设, 一行摘要, url, headers)，
/// 输出 (最终 url, headers, content-type, body)。全部预设的测试都打在这儿。
fn compose(preset: HttpPreset, summary: &str, url: &str, headers: &BTreeMap<String,String>)
    -> (String, Vec<(String, String)>, &'static str, String)
```

| 预设 | content-type | body | 备注 |
|---|---|---|---|
| `raw` | `application/json` | `{"snapshot":…,"at_ms":…}` | 现状，向后兼容 |
| `text` | `text/plain` | 一行摘要 | ntfy / Bark / 任何要一行字的地方 |
| `ntfy` | `text/plain` | 一行摘要 | 同上，另加 `Title: SignalHub` |
| `bark` | `text/plain` | 一行摘要 | |
| `slack` | `application/json` | `{"text":…}` | |
| `discord` | `application/json` | `{"content":…}` | |
| `lark` | `application/json` | `{"msg_type":"text","content":{"text":…}}` | 飞书 |
| `wecom` | `application/json` | `{"msgtype":"text","text":{"content":…}}` | 企业微信 |
| `dingtalk` | `application/json` | `{"msgtype":"text","text":{"content":…}}` | 钉钉 |
| `telegram` | `application/x-www-form-urlencoded` | `text=…` + url 查询串原样并进 body | `chat_id` 就写在 URL 上：`…/sendMessage?chat_id=123` |

**R4-2 一行摘要**（Rust，本地化）：`lang.rs` 已有 `signal_label`，直接用。

```
[SignalHub] 🔴 需授权 · work — 等你批准（2m）
```

形态：`[SignalHub] <状态> · <组显示名> — <消息>`（组名省略时只有状态）。前缀
`[SignalHub]` 让收件人一眼知道是谁发的；预设 `raw` 不带这一行（它发的是结构化数据）。
`test` 时前缀换成 `[SignalHub 测试]` —— **测试消息必须看起来就像测试**，否则用户会
以为真出事了。

**R4-3 发送测试按钮**：新命令 `test_hooks()`。复用当前快照，**忽略 `when` / 组过滤 /
跃迁判据**，把每条已配置的钩子都发一遍，结果照常进 `OutboundLog` 和日志文件。
界面就地显示结果（和现有那条 `hook-result` 一样）。

**R4-4 出站结果落盘**：`~/.signalhub/outbound.json`，`record` 时写、启动时读。
沿用「内存环形缓冲 + 落盘一份」的做法（`lamps.rs` 的 `lamp-positions.json` 是先例）。
写失败只打 stderr，不影响转发本身。

**R4-5 界面上把预设说清楚**：类型选 HTTP 时多一个「服务」下拉（默认「自定义 JSON」），
选中后 URL 的 placeholder 换成该服务的样例（`https://ntfy.sh/你的主题`、
`https://api.telegram.org/bot<token>/sendMessage?chat_id=<id>` …）。请求头做成
一个可折叠的 k/v 小列表（只用得上时再展开 —— 朴素审美）。

### 4.3 坑

- **`HookAction` 是 `#[serde(flatten)]` 的 internally-tagged enum**（`type` 字段）。
  给变体加字段没问题，但**老配置里 `{"type":"http","url":"…"}` 必须解析成
  `preset: raw, headers: {}`** —— 这就是那两条 `#[serde(default)]` 的作用，
  在 `an_old_config_without_preset_reads_as_raw` 里盯着。
- `clamped()` 里要**规整 headers**（trim 键值、丢掉空键、去重时以最后一个为准）并
  **限制条数**（比如 16 条）—— 转发目标是不可信输入，一个几千条的 map 会让每次转发
  都变慢。
- **`preset: telegram` 的 URL 解析**：`url` 里可能带 `?chat_id=…`，也可能是裸的
  `…/sendMessage`。两种都要能发（没有 `chat_id` 时 Telegram 会回 400，那条错误正好
  通过「非 2xx 带状态码回来」显示给用户 —— 这就是最好的一条自诊断）。
- `test_hooks` **不许**绕过 `is_configured`（空目标的钩子不发）；也不许改状态机。
- 摘要里的组名要用**用户起的显示名**（`Settings::group_label`）—— 用户在灯上改了名，
  转发里却还是 `build-1`，那就是两处真值打架。
- 转发触发时的**语言**：用 `lang::LocalLang::get()`（前端推过来的），没推过就是默认。

### 4.4 验收

- `cargo test`：
  - 每个预设的 `compose` 一条：content-type、body 形状、url 未被破坏；
  - `telegram` 的 `chat_id` 从 URL 查询串并进 body，且 URL 上**不带**它；
  - 老配置（`{"type":"http","url":…}`，无 `preset` / `headers`）解析成 `raw` + 空表；
  - 导出/导入往返把 `preset` / `headers` 设成**非默认值**；
  - `headers` 的 clamp 与条数上限；
  - 摘要 builder：有消息 / 无消息 / 派生状态（`stale`）三种。
- 手工：一条 ntfy + 一条 Bark，点「发送测试」，手机收到；重启应用后
  「最近一次结果」还在。

### 4.5 拆成 2-3 个 commit（实施了，附三处修正）

1. `R4-a` Rust：`HttpPreset` + `compose` + 摘要 builder + headers + clamp
   （+ 单测）——**外加 `test_all` 和 `test_hooks` 命令**（5ece827）。
2. `R4-b` Rust：出站日志落盘（defee00）。
3. `R4-c` 前端：预设下拉 + headers 编辑器 + 测试按钮 + i18n（234ffdd）。

**修正一：`test_all` / `test_hooks` 挪进了 R4-a。** 原计划把它们放 R4-b，但
`lang::text().summary_prefix_test` 只有它用得上 —— 分开的话，R4-a 单独提交过去不了
`clippy -D warnings`（unused）。挪过来之后 R4-a 是「组装层（预设 / 摘要 / 请求头）+
一次性发送」，R4-b 只剩「结果落盘」，两个都自洽。

**修正二：摘要里没有 emoji。** §4.2 里写的是 `🔴 需授权`，实现时去掉了：Rust 这边
没有状态 emoji 表（`lang::signal_label` 只有文字），新加一份就是第三处「状态长什么
样」的真值 —— 而前端的 `signals.ts`、Rust 的 `lang.rs` 已经有两处在同步了，再加一处
只会更难自查。收件人读的是文字。

**修正三：Rust 侧不做「只差大小写」的头名去重。** §4.3 写的是「去重时以最后一个为
准」，但 `BTreeMap` 已经丢掉了用户写下的**顺序**，任何胜者都是任意挑的 —— 与其悄悄
丢掉一条用户明确写下的鉴权头，不如两条都发（服务器会用状态码告诉我们它不接受）。
这条规则挪到了**前端**（R4-c 的 `lib/hookHeaders.ts`）：那里顺序还在，能确定地问用户
一句「这个名字已经有了」。Rust 侧只做去空白、丢空键、上限 16 条（按键的字典序截断，
所以丢哪几条是确定的）。

### 4.6 补充：PushDeer（实施后追加，d0dfc55）

用户追加要求接入 PushDeer。接的过程里撞出一个**比 PushDeer 本身更重要**的问题，所以
把它单记在这里：

- **`200` 也会撒谎。** PushDeer 一个失效的 key 实测回 `200 {"code":80501,"error":"错误的Key"}`，
  而 `post` 原来只把状态码带回来 —— 填错的 key 在面板上是绿色「✓ 200」而手机一声不响，
  正是 §4 开头那条「疑似」里最难查的一类。于是 `post` 之后加了
  `verdict(preset, status, body)`：出错也回 200 的那几家读一眼响应体里的信封，在 200 里
  说了失败就按失败记，并带上服务自己给的原话。顺手覆盖了飞书 / 企业微信 / 钉钉 / Slack
  （它们的信封键名各不相同），而「认不出形状就不当失败」「只读前 8KB」这两条边界写进了
  `docs/architecture.md` §3.10 和单测。`Request` 因此带上 `preset`。
- **PushDeer 的形状有两处实测结论**（不是照文档猜的）：`pushkey` 留在 URL 的查询串上
  （两边都会被读）；只填服务器地址时补 `/message/push`（POST 到根路径实测 405）；正文固定
  `type=text`（它默认按 markdown 渲染，摘要里的 `·` `—` 会被吃成标记）。
- **占位符改成按预设给。** 原来无论选哪个预设，地址框里提示的都是 ntfy —— 光看「PushDeer」
  猜不出该填什么，而填错地址是这一列里最常见的失败。

---

## 5. 实施顺序与提交

**全部完成**（下表右列是实际提交）：

| 顺序 | 提交 | 内容 | 实际 |
|---|---|---|---|
| 1 | `R1-a` | 插件式安装机制（`InstallKind`）+ opencode | 28d14dc |
| 2 | `R1-b` | Pi | af9f4d1 |
| 3 | `R1-c` | Hermes + 文档 | c8e7f35 |
| 4 | `R2` | 牌上删除 + 会话粒度删除 | 69bf509 |
| 5 | `R3` | 牌上就地改名 | 291f3db |
| 6 | `R4-a` | 转发预设 + 摘要 + headers + 测试发送 | 5ece827 |
| 7 | `R4-b` | 结果落盘 | defee00 |
| 8 | `R4-c` | 转发界面的预设与测试 | 234ffdd |
| 9 | `R4-d` | PushDeer 预设 + 「200 里的错误信封」判定 + 占位符按预设（§4.6） | d0dfc55 |
| 10 | `R3-b` | 改名框预填「此刻显示的名字」+ `nameChanged` 判据（§3.5） | 3ebb9fa |

每步一步一提交，提交信息用中文、说清「为什么」；`git commit -- <paths>` 限定路径
（多个会话共用这个检出）；push 用
`GIT_SSH_COMMAND='ssh -i ~/.ssh/id_ed25519 -o IdentitiesOnly=yes'`。

每步必须过：`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`、
`cargo test`（先 `npm run stage-cli`）、`npm test`、`npm run build`。
**不要跑 `cargo fmt`**（仓库不是 rustfmt-clean，跑了会diff 出一大堆无关文件）。
