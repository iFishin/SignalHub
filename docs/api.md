# SignalHub 上报接口规范

本文档定义外部程序（智能体、脚本、CI 任务）向 SignalHub 推送状态的两种接口：**HTTP API** 和 **CLI**。CLI 是 HTTP 的瘦客户端，两者语义完全一致。

- 状态模型与聚合算法见 [architecture.md](./architecture.md#3-状态模型与聚合)
- 当前实现进度：**HTTP、CLI、hook 自动接线均已实现并验证**。只有 §5 的降级通道（app 未运行时暂存事件）还没做。
- 本文件里的响应示例、状态码和 hook 配置样例是照着实现写的，两边应保持一致；改动接口时请同步这里。

---

## 1. 状态取值

`state` 字段的合法值（小写）：

| 值 | 视觉 | 含义 |
|---|---|---|
| `idle` | 绿、常亮 | 空闲，无任务 |
| `thinking` | 绿、快闪 | 正在推理 |
| `working` | 绿、慢闪 | 正在执行工具/任务 |
| `done` | 绿、常亮 + 亮脉冲 | 任务完成（30s 后该会话消失） |
| `attention` | 黄、闪 | 需要关注 |
| `permission` | 红、闪 | 需要授权/确认 |
| `error` | 红、快闪 | 出错/阻塞 |
| `off` | 灰、熄灭 | 暂停该会话的监控 |

> **`stale`、`offline`、`off` 这三个是聚合派生出来的，不能上报。** `GET /v1/state` 的
> `aggregate` 字段可能出现它们（`stale` = 有会话但不说话了，`offline` = 一个会话都
> 没有），但把它们当 `state` 上报会得到 400。它们是「观测到的结果」，不是外部程序
> 该声称的状态。

**每个状态都有静默超时**，超时后会话转入 `stale`（「沉默」，仍然显示）而不是消失。所以「上报完就不管」是安全的：`thinking` 600 秒、`working` 300 秒、`idle` 600 秒、`permission` 600 秒、`attention`/`error` 300 秒、`done` 30 秒（到期直接消失）。转入 `stale` 后再保留 30 分钟才真正删除。

**思考 / 执行的窗口给得很宽是故意的**：钩子是按**事件**触发的，而模型思考、或跑一段没有工具调用的长回复时一个事件都不触发 —— 窗口给短了，灯会在智能体正在干活的时候说「没有智能体上报」。长任务仍然建议显式传 `ttl_ms`（比如一条要跑一小时的任务就传 `3600000`），把「多久算沉默」这件事交给自己决定。

**别名（`state` 与 `--state` 位置参数都接受）**，便于对接不同来源的事件名：

| 别名 | 归一化为 |
|---|---|
| `blocked`, `fail`, `failure`, `stuck` | `error` |
| `notification`, `needs_attention` | `attention` |
| `permission_request`, `needs_permission` | `permission` |
| `tool_done`, `tool_use`, `active` | `working` |
| `reasoning`, `processing` | `thinking` |
| `completed`, `complete`, `success` | `done` |
| `pause`, `paused`, `disabled` | `off` |

非法值一律返回 `400`（HTTP）或退出码 `1`（CLI），**不做静默忽略**——写错状态名却看到灯没反应是最难排查的一类问题。

优先级与默认 TTL 见 architecture.md §3.1。简言之：`error` > `permission` > `attention` > `working` > `thinking` > `done` > `idle`。

---

## 2. HTTP API

默认监听 `127.0.0.1`。端口优先取 **41320**，被占用则退到系统分配端口，**两种情况下实际端口都写入 `~/.signalhub/port`**。

### 2.1 鉴权

除 `/v1/health` 外全部需要：

```
Authorization: Bearer <token>
```

`token` 是 app 首次启动生成的 32 字节随机串，写入 `~/.signalhub/token`（权限 `0600`）。

服务端同时**拒绝任何携带 `Origin` 头的请求**——浏览器发起的跨源请求一定带该头，这挡住了「你浏览的某个网页偷偷 POST 到你本地端口」这类本地 CSRF 攻击。

### 2.2 端点

| 方法 | 路径 | 说明 | 鉴权 |
|---|---|---|---|
| GET | `/v1/health` | 存活探测 | 免 |
| GET | `/v1/state` | 当前聚合状态 + 会话列表 | 需要 |
| POST | `/v1/state` | 上报状态（`session` 在 body 里） | 需要 |
| POST | `/v1/sessions/{session}/state` | 上报状态（`session` 在路径里） | 需要 |
| DELETE | `/v1/sessions/{session}` | 清除某个会话 | 需要 |
| GET | `/v1/sessions` | 会话列表 | 需要 |
| POST | `/v1/monitor` | 全局暂停 / 恢复 | 需要 |

> axum 0.8 的路径参数写作 `/{session}`；0.7 写作 `/:session`。

### 2.3 `POST /v1/state`

请求体：

```json
{
  "state":   "working",
  "session": "abc123",
  "group":   "work",
  "agent":   "claude-code",
  "cwd":     "/Users/me/proj",
  "message": "Bash: npm test",
  "ttl_ms":  300000
}
```

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---|---|---|
| `state` | string | ✅ | — | 见 §1。`off` 等价于清除该会话 |
| `session` | string | | `"default"` | 会话标识。多智能体并行时用来区分 |
| `group` | string | | `"default"` | **归属的组**。灯球上每组一块牌（见 §2.9）。不传就归 `default`，单组用户完全不用管 |
| `agent` | string | | `"unknown"` | 上报方名字，用于 UI 归因 |
| `cwd` | string | | `null` | 项目路径，UI 显示成项目名 |
| `message` | string | | `null` | 牌上显示的一行说明。**走 `signalhub hook` 时不用你给** —— 工具事件会自动生成一句，见 §4.14 |
| `ttl_ms` | number | | 按状态默认 | 覆盖默认过期时间；长任务显式传大值 |

响应 `200`：

```json
{ "ok": true, "aggregate": "working", "sessions": 2, "groups": 2, "changed": true }
```

返回聚合结果而非简单 `{"ok":true}`，好处是**一次 `curl` 调用就能看到全局状态**，调试体验显著更好。

### 2.4 `GET /v1/state`

```json
{
  "aggregate": "permission",
  "paused": false,
  "since_ms": 1697030400000,
  "message": "需要批准: rm -rf build",
  "groups": [
    {
      "name": "work",
      "aggregate": "permission",
      "message": "需要批准: rm -rf build",
      "since_ms": 1697030400000,
      "stale_progress": null,
      "sessions": [
        {
          "id": "abc123",
          "state": "permission",
          "agent": "claude-code",
          "cwd": "/Users/me/proj",
          "message": "需要批准: rm -rf build",
          "age_ms": 4200,
          "stale_progress": null
        }
      ]
    }
  ]
}
```

顶层的 `aggregate` 是**所有组里最高的那个**——钩子、托盘都用它，与分组无关。系统通知另算：它看的是**排除掉静音组之后**的同一个东西（见 architecture.md §3.17），所以静音一组不会让托盘变色、也不会让钩子不触发。`message` 来自决定它的那个会话。

**`groups` 由服务端按优先级排好（最紧急在最前，平局按组名）。客户端不要再排** —— 灯球的窗口高度取决于「哪几块牌可见」，两边必须算出同一批。

每组自己的 `aggregate` 是组内取最高优先级；`since_ms` 是该组进入当前状态的时间，牌上的「已持续」由它算。

`stale_progress` 有值时（0.0–1.0，1.0 = 刚转 `stale`，0.0 = 即将被删除）灯球用它画细环：`ring` 形状下环的弧长就是它，所以一个沉默的组会眼看着自己的环一点点缩没。**它由服务端算好**，客户端不必复刻 TTL 常量——`age_ms` 是「距上次状态变化」，并不等于「沉默多久」。

它出现在两处：**组上的** `stale_progress`（取决定聚合那个会话的，灯上的圆点就是因为它才变 `stale`）和每个 `SessionView` 上的同名字段。灯球只用前者——它画的是每组的圆点，不是每路的。

**进度会自己往前走**：`sweep` 每 250ms 醒一次，进度跨过一档就再推一帧（分 16 档，约每 19 秒一档）。不分档而是每次 tick 都推的话，整段宽限期里灯球会一直重排；而常驻窗口每帧重排是这个项目一直在避免的事。

### 2.5 `POST /v1/monitor`

```json
{ "paused": true }
```

响应 `{ "ok": true, "paused": true }`。暂停期间 `GET /v1/state` 的 `aggregate` 恒为 `off`，但会话表原样保留，恢复后立即回到真实状态。

### 2.6 `DELETE /v1/sessions/{session}`

清除会话（智能体正常结束时应调用）。响应：

```json
{ "ok": true, "removed": true, "aggregate": "idle" }
```

会话不存在时 `removed: false` 且仍返回 `200`——清除是幂等操作，重复调用不该是错误。只有**非法状态值**才返回 `400`。

注意 `aggregate` 是清除**之后**重算的结果：如果还有别的会话在报状态，它可能仍是红或黄，而不是 `idle`。

`POST /v1/sessions/{session}/state` 的路径参数会**覆盖**请求体里的 `session` 字段，避免两处不一致时行为难以预测。

### 2.7 分组

上报时带一个 `group` 就是**一块牌**：

```bash
curl -s -X POST ... -d '{"state":"working","session":"a","group":"work"}'
curl -s -X POST ... -d '{"state":"error","session":"b","group":"ci"}'
```

于是灯球上出现两块牌（`ci` 在上，因为它更紧急），全局 `aggregate` 取两者里最高的 `error`。

命令行等价于 `signalhub working --session a --group work`。`--group` 只影响这一条上报；CLI 不认识组名，也不做任何校验 —— 和状态名一样，组名是上报方自己定的字符串。

- 不传 `group` 归 `default`，行为与不分组的版本完全一样。
- 组名由上报方自己定。**同一组的会话会一起聚合**，所以别把不同项目的会话塞进同一个组。
- 同一个会话换个组上报会把它挪过去（和状态一样是后写覆盖）。
- 组是**临时**的：组里最后一个会话消失（TTL 过期或 `DELETE`），这块牌就消失。没有「建组」这一步。

#### 兜底分组（`group_by`）

设置里的「分组方式」有三种取值。**没带 `group` 的上报**由服务端按它兜一个组名：

| 取值 | 一个组是什么 | 组名从哪来 |
|---|---|---|
| `none`（默认） | 所有上报归 `default` | —— |
| `cwd` | 一个工作目录 | `cwd` 的**完整路径**，去掉末尾分隔符 |
| `session` | **一个会话** | `session` 本身 |

几个要点：

- **上报自带的 `group` 优先**，`group_by` 只是兜底、不是覆盖。所以手写 `group` 的用法和这个开关互不干扰 —— 三种取值都一样。
- `cwd` 缺失或就是根目录时退回 `default`。`session` 没有这个问题（它总有值）。组名是完整路径而不是最后一段目录名：`~/a/api` 和 `~/b/api` 撞名会把两个项目混进同一块牌。
- **`session` 适合「同一个智能体并行开了几个窗口 / worktree」**：只按目录分组会把它们挤在同一块牌上，只能看到最紧急的那一个 —— 而它们的进度往往是两码事。代价是组名是上报方给的会话 id，多半是 UUID 那样不好读的串；牌上会自动截断，想去「外观 → 各组的显示」改名也行。
- 这个开关是**服务端**在读，不是 CLI、也不是装出来的钩子命令。装出来的命令是一个固定字符串，写死了就只能是常量；而按目录 / 按会话分组要的是每次调用各自的值。做在服务端还顺带覆盖了 `curl`、CI 和手写脚本 —— 它们不用改一行。
- 只影响**之后**的上报。现存的会话保持原来的组，直到它下次上报（活跃的钩子几秒内就跟上，静默的会话要等 TTL）。

### 2.8 错误码

| 码 | 场景 |
|---|---|
| `400` | `state` 非法；body 不是合法 JSON |
| `401` | 缺 token / token 错误 / 请求带 `Origin` 头 |
| `404` | 路径不存在 |

错误体统一为 `{"error": "<原因>"}`。

### 2.9 curl 示例

> 本节的示例都是 POSIX shell（`$(cat …)`）。**Windows 上把 `~/.signalhub` 换成
> `%USERPROFILE%\.signalhub`**，取文件内容用 `Get-Content`；README 的「上报状态」
> 一节有一份完整的 PowerShell 写法。

```bash
PORT=$(cat ~/.signalhub/port)
TOKEN=$(cat ~/.signalhub/token)

curl -s -X POST "http://127.0.0.1:$PORT/v1/state" \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"state":"permission","session":"demo","message":"需要授权"}'
```

```bash
curl -s "http://127.0.0.1:$(cat ~/.signalhub/port)/v1/health"
```

---

## 3. CLI

```
signalhub <state> [flags]                    上报状态
signalhub hook <agent>                       从 stdin 读智能体 hook JSON 并上报
signalhub status [--json]                    查询当前聚合状态
signalhub watch [选项]                       跟随状态变化，或等某个状态出现
signalhub clear --session <id>               清除会话
signalhub monitor on|off                     全局暂停 / 恢复
signalhub on | off                           monitor 的简写
signalhub run [选项] -- <命令>               把一条命令包起来上报（见 §4.13）
signalhub install hooks <agent>              写入智能体 hook 配置
signalhub uninstall hooks <agent>            移除本工具写进去的 hook
```

**CLI 的文案跟着界面语言走**（`--help`、报错、各子命令的输出都有中英两份）。它拿不到浏览器的 `navigator.language`，所以按可靠性排了三个来源：`config.json` 里**显式**的 `locale` → app 写下的 `~/.signalhub/lang`（界面**实际**显示的语言）→ 环境变量 `LC_ALL` / `LC_MESSAGES` / `LANG`。所以应用跑过一次之后，CLI 说的语言和设置面板上显示的一定一致。

选项可以写在子命令之前或之后，两种写法等价（`signalhub --port 1 status` 与 `signalhub status --port 1`）。

示例：

```bash
signalhub working --session build-1 --agent ci --message "compiling"
signalhub permission --session abc123 --message "需要批准: rm -rf"
signalhub done --session build-1
signalhub error --session ci --ttl 60s
signalhub working --session b --group ci          # 报进 ci 组，灯上单独一块牌
signalhub status
```

### 3.1 flags

| flag | 说明 |
|---|---|
| `--session <id>` | 会话标识，默认 `default` |
| `--agent <name>` | 上报方名字 |
| `--cwd <path>` | 项目路径 |
| `--group <name>` | 归属的组，默认 `default`。见 §2.7 |
| `--message <text>` | 悬停提示文本 |
| `--ttl <dur>` | 覆盖默认 TTL，如 `30s` `5m` `1h` |
| `--json` | `status` / `watch` 输出原始 JSON |
| `--never-fail` | 任何错误都以 0 退出（供 hook 使用） |
| `--port <n>` | 覆盖端口自动发现 |
| `--dry-run` | `install` / `uninstall` 只打印将要做的改动，不写文件 |
| `--interval <dur>` | `watch` 的轮询间隔，默认 500ms |
| `--until <状态>` | `watch` 等到聚合变成该状态就退出 0 |
| `--timeout <dur>` | `watch` 的等待上限，超时退出 1 |

### 3.2 退出码

| 码 | 含义 |
|---|---|
| `0` | 成功 |
| `1` | 参数错误或服务端返回错误 |
| `3` | **app 未运行**（`~/.signalhub/port` 不存在或连接被拒） |

退出码 3 独立于 1，让脚本能区分「我参数写错了」和「app 没开」这两种完全不同的情况。`--never-fail` 时恒返回 0；**`hook` 模式隐含该行为**——上报客户端不该有能力弄坏它监控的对象。

CLI 自己不校验状态名，而是原样转发给服务端：别名表只有一份（`state::Signal::parse`），因此不可能出现 CLI 与服务端理解不一致的情况。写错状态名会拿到服务端的 400 和它的错误文本。

### 3.3 `signalhub watch`

两种用法：终端里开一个窗口盯着，以及在 CI 里等某个状态。

```bash
signalhub watch                                   # 状态变化时打印一行
signalhub watch --until done --timeout 10m        # 等到 done，超时退出 1
signalhub watch --json > states.jsonl             # 每次变化一行 JSON，可回放
```

只在状态**变化**时输出（每 500ms 刷一行会把日志淹掉），提示信息走 stderr，所以 `signalhub watch > 日志` 拿到的是干净的状态时间线。

`--until` 只接受规范状态名（`GET /v1/state` 返回的那一套，含派生的 `stale` / `offline`），不接受别名——它要和 `aggregate` 做字面比较。写错会立刻报错并列出可用值，**不会静默地永远等下去**。这是 CLI 里唯一硬编码状态名的地方，理由就是这个：一个静默的无限等待比一个明确的报错糟糕得多。

### 3.4 `off` 的两个含义（容易混淆，特此明确）

`off` 既是会话状态也是全局开关，两者不是一回事：

| 写法 | 作用范围 | 效果 |
|---|---|---|
| `DELETE /v1/sessions/{id}` 或 `signalhub clear --session <id>` | 单个会话 | 清除该会话，聚合可能仍由别的会话决定 |
| `POST /v1/state` 带 `"state": "off"` | 单个会话 | 同上——会话上的 `off` 就是清除 |
| `POST /v1/monitor` `{"paused": true}` 或 `signalhub monitor off` | **全局** | 聚合恒为 `off`，但会话表原样保留，恢复后立即回到真实状态 |

`signalhub off` / `signalhub on` 是 `monitor off` / `monitor on` 的简写，作用于全局。想清除某个会话请用 `clear`。

**代码里 `off` 永远不会成为某个会话的 state**：`Store::apply` 遇到 `off` 直接转成 `clear`，全局暂停存在独立的 `paused` 位。这样聚合函数不需要在会话层面处理 `off` 这个特例。

---

## 4. 智能体接入

### 4.1 为什么 CLI 要有 `hook` 子命令

如果用 shell 脚本做事件映射，需要 `jq`（macOS 默认不装）、要处理各种转义、要给每个事件各写一份。改为 CLI 内置解析后：

- 零外部依赖，不需要 `jq` / `python`
- 安装 hook = 把**同一行命令**注册到所有事件
- 映射表用 Rust 写，可单测
- hook 模式下 stdout 保持绝对为空（原因见 §4.4 第 3 条），诊断信息只走 stderr 和日志文件

### 4.2 Claude Code 事件映射

事件名以官方文档 https://code.claude.com/docs/en/hooks 为准。该页面记录了 **33 个**事件，下表覆盖我们实际映射的 14 个，以及明确**忽略**的其余部分。

**上报**

| hook 事件 | matcher | SignalHub 状态 | 说明 |
|---|---|---|---|
| `SessionStart` | `startup\|resume` | `idle` | 见下文的 matcher 注意事项 |
| `UserPromptSubmit` | — | `thinking` | 提交 prompt 后、Claude 处理前 |
| `PreToolUse` | `*` | `working` | |
| `PostToolUse` | `*` | `working` | 工具成功 |
| `PostToolUseFailure` | `*` | `attention` | 工具失败，但 Claude 通常会自行恢复，不算 `error` |
| `PermissionRequest` | — | `permission` | **主路径**：权限点即时触发 |
| `Notification` | `permission_prompt` | `permission` | 回落路径，触发明显晚于 `PermissionRequest`（官方文档未给出具体延迟，不写死秒数） |
| `Notification` | `agent_needs_input` | `attention` | |
| `Notification` | `agent_completed` | `done` | |
| `Notification` | `idle_prompt` | `attention` | Claude 停下等你开口 |
| `SubagentStart` | — | `thinking` | |
| `PermissionDenied` | — | `working` | 权限点已解决，红灯应该灭 |
| `Stop` | — | `done` | 一轮回复结束 |
| `StopFailure` | — | `error` | 轮次因 API 错误中断 |
| `PreCompact` / `PostCompact` | — | `thinking` / `working` | 压缩上下文时仍有动作 |
| `TeammateIdle` | — | `attention` | agent-team 队友待输入 |
| `SessionEnd` | — | 清除会话 | |

**明确忽略**（收到这些事件就原样退出，不上报）

`Setup`、`UserPromptExpansion`、`PostToolBatch`、`MessageDisplay`、`TaskCreated`、`TaskCompleted`、`InstructionsLoaded`、`ConfigChange`、`CwdChanged`、`DirectoryAdded`、`FileChanged`、`WorktreeCreate`、`WorktreeRemove`、`PreModelSwitch`、`PostModelSwitch`、`Elicitation`、`ElicitationResult`、**`SubagentStop`**。

**为什么要把「忽略」也写清楚**：`TaskCreated`/`TaskCompleted` 指的是 Claude 内部的待办清单，跟「一轮工作」不是一回事；`FileChanged`/`CwdChanged` 在会话里非常频繁。把它们当活动信号会让灯球乱闪。少报一点比报错好。

**`SubagentStop` 为什么必须忽略**（实测踩到的 bug）：它在 `Stop` 之后约 9 秒还会触发一次。若把它映射成 `working`，就会把刚设好的 `done` 覆盖掉——轮次明明结束了，灯却继续闪绿直到 TTL 到期。而主智能体还在干活时，`PreToolUse`/`PostToolUse` 本来就会持续报 `working`，所以丢掉这个事件几乎没有信息损失。Codex 与 Cursor 的同名事件同理。

**两个 matcher 注意事项**

- `SessionStart` 的 matcher 有 `startup` / `resume` / `clear` / `compact` / `fork`。只有 `startup` 和 `resume` 才该报 `idle`——`compact` 表示上下文压缩，此时智能体**仍在工作中**，报 idle 是错的。
- `SessionEnd` 的 matcher 有 `clear` / `resume` / `logout` / `prompt_input_exit` / `other`，全部都应清除会话。

**字段**

所有事件都在 stdin 上收到共同的 `session_id`、`transcript_path`、`cwd`、`hook_event_name`，多数还带 `permission_mode`。会话 ID 取 `session_id`，`cwd` 一并上报用于多会话归因。

`Notification` 另有 `message`、`title`、`notification_type`。完整的 `notification_type` 取值：`permission_prompt`、`idle_prompt`、`auth_success`、`elicitation_dialog`、`elicitation_url_dialog`、`elicitation_complete`、`elicitation_response`、`agent_needs_input`、`agent_completed`、`quota_auto_resume_fired`、`quota_auto_resume_stale`、`quota_auto_resume_disabled`。

`Stop` 另有 `stop_hook_active`（bool）、`last_assistant_message`、`background_tasks`、`session_crons`。`stop_hook_active` 为 `true` 表示 Claude 已经因 stop hook 在继续——我们不上报决策，所以不影响。

### 4.3 写入的 hook 配置

`signalhub install hooks claude-code` 会**合并**（不是覆盖）以下内容进 `~/.claude/settings.json`，并先备份原文件。注意 hook 条目在各级 settings 之间是**合并**语义，不会互相替换。

```json
{
  "hooks": {
    "SessionStart":      [{ "matcher": "startup|resume", "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "UserPromptSubmit":  [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PreToolUse":        [{ "matcher": "*", "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PostToolUse":       [{ "matcher": "*", "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PostToolUseFailure":[{ "matcher": "*", "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PermissionDenied":  [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "Notification":      [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "SubagentStart":     [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "SubagentStop":      [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "Stop":              [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "StopFailure":       [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PreCompact":        [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "PostCompact":       [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "TeammateIdle":      [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }],
    "SessionEnd":        [{ "hooks": [{ "type": "command", "command": "signalhub hook claude-code", "async": true }] }]
  }
}
```

### 4.4 `async: true` 的语义（已核实，有三条要小心）

官方文档确认 `async` 是 `type: "command"` 独有的字段。它的实际行为：

1. **异步 hook 不受 `timeout` 约束。** 文档原文：一旦异步 hook 在后台跑起来，Claude Code 就不对它执行 `timeout`。所以我们**不能**靠 `timeout` 兜底，必须在 CLI 内部自己限制 HTTP 连接超时（计划用 1 秒），否则一个卡住的 `signalhub` 进程会一直悬着。
2. **异步 hook 不能做决策。** `decision`、`permissionDecision`、`continue` 等字段一律无效。这对我们是好事——上报型 hook 本来就不该影响智能体。
3. **异步 hook 不能在 stdout 输出畸形 JSON。** 文档明确警告：v2.1.202 之前，异步 hook 输出畸形 JSON **会导致会话崩溃**，且每次恢复会话都会重现。

> 结论：`signalhub hook <agent>` 在 hook 模式下 **stdout 必须保持完全为空**，所有诊断信息写 stderr 或日志文件。这要作为 CLI 的硬性约束来测。

另外还有 `asyncRewake`：它**会**被施加 `timeout`，而且退出码 2 能在会话空闲时立刻唤醒 Claude。信号灯不需要这个能力，不用。

不用 `timeout` 字段：既然异步 hook 不执行超时，写上只会误导读者以为有保护。

### 4.5 安装、卸载与安全边界

`signalhub install hooks <agent>` 会改动智能体的配置文件：

| 智能体 | 文件 | 形态 |
|---|---|---|
| Claude Code | `~/.claude/settings.json` | 嵌套 `{ matcher?, hooks: [ { type, command, async } ] }` |
| Codex | `~/.codex/hooks.json` | 同上 |
| Cursor | `~/.cursor/hooks.json` | 扁平 `{ command, timeout }`，且顶层必须有 `version: 1` |

Codex 也可以用 `config.toml` 写 hook，但这里刻意选独立的 `hooks.json`：改 TOML 要保留格式和注释，而 JSON 可以整份解析后重写，安全得多。

**六条安全边界**（都有对应单测）：

1. **只动 `hooks` 一个键。** 同一个文件里的 `env`、`theme`、别的工具的 hook 一律原样保留。
2. **解析失败就中止，绝不覆盖。** 配置不是合法 JSON 时报错退出（码 1），文件一字不改。把一个手写的 `settings.json` 弄坏的代价远大于功能暂时不可用。
3. **幂等。** 已装过且命令相同的事件会跳过，重复执行不会产生重复条目——否则每次工具调用会跑 N 次上报。
4. **命令变了就原地更新。** 已装过但写入的命令不同（换了二进制位置、或从绝对路径改成 PATH 里的裸名字）时，改写那个字段而不是再添一份。
5. **先备份。** 文件原本存在时会写到 `<文件名>.signalhub-backup`；原本不存在则没有可备份的东西，不生成备份文件。
6. **只摘自己的 handler。** 卸载时按「第一个词是 signalhub 二进制且参数含 `hook`」识别自己的条目，所以**二进制路径变了（重装、挪目录）也照样认得出来**。如果某个 matcher-group 里同时还有别人的 handler，只摘掉自己的、整组保留。

第 3 条和第 4 条的区别不是多余的：**只做幂等跳过的话，「把 signalhub 放进 PATH 后重新执行本命令」这条提示就是假的**——条目会被判为「已存在」，绝对路径永远换不成裸名字。这是实现时真实踩到的第二个坑。

第 6 条有个不显然的推论：**事件的整个键被删掉时 `removed` 才有内容，而只摘掉组内一个 handler 时 `removed` 为空。** 判断「有没有改动」必须看摘掉的数量，不能看 `removed` 是否为空——否则会出现「摘了但没落盘」的静默失效。这是第一个坑。

`--dry-run` 只演算不写盘，适合先看一眼会改什么。

**hook 命令写什么。** 优先写裸名字 `signalhub`（可移植，重装或挪动二进制都不失效）；PATH 里没有时退回**当前可执行文件的绝对路径**，保证装完立刻可用。路径含空格时会自动加引号。用绝对路径时 CLI 会额外提醒一句——因为这种失效是静默的（智能体会话里什么都看不到）。

> 实测：`signalhub` 放在 `~/.local/bin` 下即可被 Claude Code 的 hook 执行环境解析到（hook 由 shell 执行，继承用户的 PATH）。所以推荐软链到 `~/.local/bin` 而不是需要 sudo 的 `/usr/local/bin`。装好后重跑 `install hooks` 会自动把已有的绝对路径改成裸名字。

**逃生口。** 要临时彻底关掉所有 hook，不必动配置文件：`claude --settings '{"disableAllHooks": true}'`，或在 settings 里设 `"disableAllHooks": true`。二进制找不到时 `signalhub uninstall hooks <agent>` 也可能失效，此时手动删掉 `hooks` 键即可（有备份文件兜底）。

### 4.6 Codex CLI

配置位置：`~/.codex/hooks.json`、`~/.codex/config.toml`、`<repo>/.codex/hooks.json`、`<repo>/.codex/config.toml`。JSON 与 TOML 两种写法等价：

```json
{"hooks":{"PreToolUse":[{"matcher":"^.*$","hooks":[
  {"type":"command","command":"signalhub hook codex","async":true}]}]}}
```

```toml
[[hooks.PreToolUse]]
matcher = "^.*$"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "signalhub hook codex"
async = true
```

**12 个事件**（PascalCase，与 Claude Code 高度同构）：

| 事件 | 映射 |
|---|---|
| `SessionStart` | `idle` |
| `UserPromptSubmit` | `thinking` |
| `PreToolUse` / `PostToolUse` | `working` |
| `PermissionRequest` | `permission` |
| `SubagentStart` | `thinking` |
| `SubagentStop` | **忽略**（见 §4.2 的说明） |
| `PreCompact` / `PostCompact` | `thinking` / `working` |
| `Stop` | `done` |
| `Interrupt` | 清除会话 |
| `SessionEnd` | 清除会话 |

stdin 字段：`session_id`、`turn_id`、`agent_id`、`agent_type`、`transcript_path`、`cwd`、`hook_event_name`、`model`、`permission_mode`；工具事件另有 `tool_name`、`tool_input`、`tool_use_id`。

差异要注意：Codex 有 `session_id`（不是 `thread-id`——那是老式 `notify` 的字段名）。异步 hook 每会话最多 8 个并发。`SessionEnd` 总是同步执行，默认超时仅 1 秒（上限 3 秒），所以这条路径上的 HTTP 调用必须极快，超时要更短。

### 4.7 Cursor

配置位置：`~/.cursor/hooks.json`（用户级）、`<project>/.cursor/hooks.json`（项目级），另有企业级路径。优先级：企业 → 团队 → 项目 → 用户。**`version` 字段是必填的**：

```json
{"version":1,"hooks":{
  "preToolUse":[{"command":"signalhub hook cursor","timeout":5}],
  "beforeSubmitPrompt":[{"command":"signalhub hook cursor","timeout":5}]
}}
```

这里**保留了 `timeout`，与 Claude Code 那节刻意不同**：Cursor 只有 `sessionStart`、`sessionEnd`、`preCompact` 是 fire-and-forget，`preToolUse`、`beforeSubmitPrompt` 这类是**会被等待**的，所以超时保护有意义。Claude Code 那边因为 `async: true` 让 `timeout` 失效，写上去只会误导。

**事件名是 camelCase**（与另外两家都不同）：

| 事件 | 映射 |
|---|---|
| `sessionStart` | `idle` |
| `beforeSubmitPrompt` | `thinking` |
| `preToolUse` / `postToolUse` | `working` |
| `postToolUseFailure` | `attention` |
| `subagentStart` | `thinking` |
| `subagentStop` | **忽略**（见 §4.2 的说明） |
| `beforeShellExecution` / `afterShellExecution` | `working` |
| `afterAgentThought` | `thinking` |
| `afterAgentResponse` | `done` |
| `stop` (`status: "error"` / `"aborted"`) | `error` |
| `stop` (`status: "completed"`) | `done` |
| `preCompact` | `thinking` |
| `sessionEnd` | 清除会话 |

stdin 字段：`conversation_id`、`generation_id`、`model`、`hook_event_name`、`workspace_roots`、`transcript_path`；`sessionStart`/`sessionEnd` 另有 `session_id`；`preToolUse` 另有 `tool_name`、`tool_input`、`cwd`。会话 ID 优先取 `conversation_id`。

Cursor 的 **`stop` 事件带 `status`（`completed` / `aborted` / `error`）**，这是三家里唯一能直接区分「正常结束」和「异常中断」的，所以 Cursor 的 `error` 判定比 Claude Code 更准（后者只能靠 `StopFailure`）。

只有当 Cursor 的 hook 返回 `permission: allow|deny|ask` 时才涉及授权，而上报型 hook 不返回这个字段，因此**就没有对应的「需要授权」事件可映射**——这是 Cursor 侧的能力缺口，不要在文档里假装有。

### 4.8 hook 调用日志

hook 模式会把每次调用追加到 `~/.signalhub/hooks.log`：

```
15:23:02Z  claude-code  PreToolUse           session=log-test  working
15:23:02Z  claude-code  FileChanged          session=log-test  ignored
15:23:02Z  claude-code  SessionEnd           session=log-test  clear
```

时间戳是 **UTC**（后缀 `Z`），刻意不引入时区依赖。四列分别是时间、智能体、事件名、会话、结果；结果是上报的状态、`clear`、`ignored`（事件被明确忽略）、或 `失败: …`。

**为什么需要它**：出问题时（「灯为什么显示成这样」）要知道**哪些事件真的触发了**。比如「一轮结束时 `Stop` 有没有触发」就只能靠它回答——从状态本身看不出来，因为状态会停在最后一个事件上。被忽略的事件也记，因为「某个事件其实一直在触发、只是被我们忽略了」同样是需要能看见的信息。

日志超过 512KB 会轮转一次（`hooks.log.1`），只保留一代。写日志失败一律忽略——为了记日志而弄坏智能体的 hook 是本末倒置。

**设置面板 →「系统」页会把这个文件的尾巴直接摆出来**（`get_hook_log`，取最后 40 行）。它回答的是「接了但灯不亮」里最难自己查到的那一半：**事件到底有没有从智能体那边打过来**。一行都没有 = 压根没打过来（多半是配置里那条命令跑不起来，见下一段）；有行但都写着失败 = 打过来了、只是没送到应用。

**「接入」页会检查配置里那条命令现在还跑不跑得起来**（`hooks::installed_command` + `command_resolves`）：读回**配置里实际写着**的命令（不是我们现在会写的那条），只看它的程序那一段是否还能解析到。`已接入 && 命令无效` 会显示成一行红色告警，并把命令原文摆出来。

这一段值得单独存在，是因为它是**静默失效**的典型：命令是某次装进去的绝对路径，之后 app 被挪走或 sidecar 被删，配置里看不出任何异常 —— 智能体执行一个不存在的程序**不会报错**（它只是照常继续），灯也不会亮。两边都很安静，用户唯一的线索就是「什么都没发生」。按一次「接入」就会改写成当前路径。

### 4.9 出站钩子（SignalHub 往外发）

这是唯一一个方向相反的接口：配置若干条**钩子**，聚合变化时按条件触发。用于手机推送（ntfy）、团队频道（Slack）、跑脚本、或接进自建服务。

在设置面板的「钩子」页配置，也可以直接写 `~/.signalhub/config.json`：

```json
{
  "hooks": [
    { "when": ["permission", "error"], "type": "http",    "url": "https://ntfy.sh/my-topic" },
    { "when": ["done"],               "type": "command", "command": "say 完成" },
    { "when": [],                     "type": "http",    "url": "http://localhost:9000/ingest" }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `when` | 哪些状态**变成**时触发。空数组 = 任何变化都触发。值是状态名，**含 `stale` / `offline` 这类派生状态**（你可能只想知道「会话沉默了」） |
| `type` | `"http"` 或 `"command"` |
| `url` | `type: "http"` 时的目标，必须 `http://` 或 `https://` 开头 |
| `command` | `type: "command"` 时执行的命令，通过 `sh -c`（Windows 上是 `cmd /C`）执行 |

**触发语义**：只在聚合**发生跃迁**时触发（同一个状态内的变化不算，例如某个会话的说明文本改了）。这一点和系统通知一致——一轮工具调用会触发几十次状态推送，按「每次变化」触发就成了骚扰。

**请求体**（`type: "http"`）：

```json
{
  "snapshot": {
    "aggregate": "permission",
    "paused": false,
    "since_ms": 1791683689000,
    "message": "需要批准: rm -rf build",
    "sessions": [ /* 与 GET /v1/state 的结构完全一致 */ ]
  },
  "at_ms": 1791683690537
}
```

`at_ms` 是触发时刻（epoch 毫秒）——接收方多半要记时间，而 `since_ms` 是状态起点。

**行为约定**：

- **异步**执行，每条钩子独立。一条失败不影响其它钩子，也不影响状态机。
- HTTP 有 3 秒连接 / 10 秒总超时；命令有 10 秒超时，超时会被终止（挂死的钩子不该把 tokio 的阻塞线程池占满）。
- 命令的 stdout / stderr 全部丢弃——钩子是拿来触发副作用的，它的输出没地方去。
- 失败只写 stderr（`[signalhub] 钩子执行失败: …`），不重试。本地状态灯不该因为远端不可达而变复杂。
- **目标留空的钩子不触发也不报错**：那是编辑器里的中间态（刚点「添加」还没填），面板上会标「未填目标」。

> **关于 `type: "command"` 的安全性**：它会执行本地命令。信任模型和 git hooks / npm scripts 一样——配置文件在你自己的主目录里，能改它就能执行任何东西。所以**没有**额外的开关：多一个开关只会给人一种「关掉就安全」的错觉，而配置文件本来就在你手里。不要把别人的 `config.json` 直接拿来用。

**ntfy 示例**：`url` 填 `https://ntfy.sh/<你的主题>` 即可，ntfy 会忽略 JSON body 里不认识的字段并显示一条通知。

### 4.10 不用 CLI 的最小接入

任何程序只要能发 HTTP 就能上报，无需安装 CLI：

```bash
curl -sf -m 1 -X POST "http://127.0.0.1:$(cat ~/.signalhub/port)/v1/state" \
  -H "Authorization: Bearer $(cat ~/.signalhub/token)" \
  -H 'Content-Type: application/json' \
  -d '{"state":"working","session":"'"$MY_SESSION_ID"'"}' || true
```

`-m 1` 保证不阻塞，`-sf` + `|| true` 保证失败不影响调用方。

### 4.11 Gemini CLI

配置位置：`~/.gemini/settings.json`（用户级）、`<repo>/.gemini/settings.json`（项目级）。形状与 Claude Code 同构，但**没有 `async`**（那是 Claude Code 的扩展字段，它的文档里没有），改带 `timeout`：

```json
{"hooks":{"SessionStart":[{"hooks":[
  {"type":"command","command":"signalhub hook gemini","timeout":5000}]}]}}
```

**装 8 个事件**（PascalCase）：

| 事件 | 映射 |
|---|---|
| `SessionStart` | `idle` |
| `BeforeAgent` | `thinking` |
| `PreCompress` | `thinking` |
| `BeforeTool` / `AfterTool` | `working` |
| `Notification` | `attention` |
| `AfterAgent` | `done` |
| `SessionEnd` | 清除会话 |

stdin 字段：`session_id`、`transcript_path`、`cwd`、`hook_event_name`、`timestamp`；`SessionStart` 另有 `source`，工具事件另有 `tool_name`、`tool_input`。

**刻意不装** `BeforeModel`、`AfterModel`、`BeforeToolSelection`：这三个每次模型调用都触发，装上去灯会一直在闪而信息量为零。

两条要注意的：

1. **它没有「需要授权」事件。** 权限点靠 hook 的返回值决定，而上报型 hook 不返回值。所以 `Notification` 是唯一的「有事要你看一眼」信号 —— 但**官方参考里没写它确切的触发条件**，这里按「通知 = 有事」映射成 `attention`（最轻的那一档）。映射偏了的表现是多几盏黄灯，不会漏掉真正的异常。
2. **stdout 必须是纯 JSON。** Gemini 要求 hook「除了最后那段 JSON 之外不得往 stdout 打任何东西」，而 exit 0 时它会把 stdout 当 JSON 解析。所以 `signalhub hook gemini` 会往 stdout 打一个 `{}` —— 这是**全项目唯一一处违反「hook 模式 stdout 必须为空」**的地方，理由写在 `bin/signalhub.rs` 里：空 stdout 算不算合法 JSON 它没写清楚，而 `{}` 在两种解读下都安全（合法的 JSON，且不含任何 decision 字段）。其余几家都明确接受空 stdout，所以不加。

`timeout` 是**毫秒**（默认 60000），这里设 5000。Gemini 的 hook 是**同步**的（没有 async 开关），所以超时保护是必需的。

### 4.12 Goose

**Goose 走的是插件，不是共享配置。** 装的是我们自己的一个目录：

```
~/.agents/plugins/signalhub/
├─ plugin.json          插件清单（Goose 靠它发现插件）
└─ hooks/hooks.json     hook 定义
```

```json
{"hooks":{"PreToolUse":[{"hooks":[
  {"type":"command","command":"signalhub hook goose","timeout":5}]}]}}
```

**这条路比改共享配置更安全**：两个文件都是我们自己建的，不存在「解析不了用户手写的内容」这回事；卸载时整个目录删掉即可（名字就叫 signalhub，不会误伤）。

**装 7 个事件**：

| 事件 | 映射 |
|---|---|
| `SessionStart` | `idle` |
| `UserPromptSubmit` | `thinking` |
| `PreToolUse` / `PostToolUse` | `working` |
| `PostToolUseFailure` | `attention` |
| `Stop` | `done` |
| `SessionEnd` | 清除会话 |

**payload 的字段名自成一格**，这是接它最容易踩岔的地方：

- 事件名在 **`event`**，不是 `hook_event_name`（其余几家都是后者）；
- 工作目录在 **`working_dir`**，不是 `cwd`；
- 会话 id 仍是 `session_id`。

**刻意不装** `BeforeReadFile`、`AfterFileEdit`、`Before|AfterShellExecution`：读文件是高频事件。Goose 也明确说了它**不发** `SubagentStart` / `SubagentStop`，那两个不用考虑。

`timeout` 是**秒**（默认 30），这里设 5 —— 和 Gemini 的毫秒差三个数量级，`hooks.rs` 里专门有个带注释的函数说明这件事。

同样**没有 `PermissionRequest`**：Goose 的授权靠 `PreToolUse` 返回 `decision: "block"` 实现，上报型 hook 不返回值。

### 4.13 `signalhub run`：通用适配，不需要知道对方的 hook 格式

上面每一家都要「配置路径 + 事件名 + handler 形状」三样确切信息，而这三样各家都不同、也一直在变。**`run` 是绕开这件事的那条路**：

```bash
signalhub run --agent aider -- aider --model sonnet
```

它把一条命令包起来上报，不关心对方是什么：

| 时机 | 上报 |
|---|---|
| 命令启动前 | `working` |
| 跑的过程中 | 每 20 秒续期一次（TTL 60 秒） |
| 退出后 | 退出码 0 → `done`，非零 → `error` |

**任何跑在终端里的工具都能这么接。** `--agent` 的名字随便填（服务端不校验，它只是上报体里的一个标识）。

#### 四条设计约束

1. **退出码原样传出去。** 包一层不能改被包命令的语义 —— 否则 CI 里 `signalhub run -- make test` 会永远绿，那是最坏的一种「集成」。这也是唯一一个不遵守本文档开头那张退出码表的子命令。
2. **stdout / stderr 原样透传**（继承父进程），所以管道、重定向、颜色都照旧。
3. **上报失败不影响被包的命令。** 灯没开、端口读不到 —— 命令照跑，只在 stderr 上**提醒一次**（不是每次心跳都提醒，那会把被包命令的输出淹掉）。用户是来干活的，这个顺序不能颠倒。
4. **被包命令自己的选项写在 `--` 之后。** 那之后的一切原样交给它 —— 否则 `--message`、`--ttl` 这种重名选项会被我们吃掉，而且**不报错**，只表现为对方莫名其妙少了个参数。

#### VS Code 系

**跑在集成终端里的**（包括 VS Code 自己的任务）用同一条命令。`tasks.json` 里：

```json
{
  "label": "跑智能体",
  "type": "shell",
  "command": "signalhub run --agent myagent -- ${input:agentCommand}"
}
```

（`input` 那里填实际命令；`${input:...}` 是 VS Code 自己的变量语法，不是我们的。）

**只存在于侧边栏面板里的扩展**（Cline / Roo Code / Continue 这类）**接不了** —— 它没有外部进程可包、也没有可写的 hook 配置。这不是偷懒：通用的办法要么包一个进程、要么写一份配置，而这两样它都不给。要用就得等那个扩展自己提供集成点。

#### 已知边界

- **Ctrl-C 掉包装器**时收尾那段跑不到，所以不会报 `done` —— 会话会在 TTL（60 秒）之后转 `stale`，再过宽限期消失。这是刻意的兜底，不是 bug。
- 心跳是 HTTP 请求，所以**命令跑多久就发多久的请求**（每 20 秒一次，本机回环）。跑一整天的任务会发几千次，但每次都是几字节的本地回环。

### 4.14 牌上的「正在做什么」

`hook` 这条路上，`message` **不用你提供**：工具事件（payload 里带 `tool_name`）会被
自动压成一句话，填进上报的 `message`。

```
PreToolUse  tool_name=Edit  tool_input={file_path: "/proj/src/state.rs"}
            → message = "改 state.rs"

PreToolUse  tool_name=Bash  tool_input={command: "cargo test --all"}
            → message = "执行 cargo"
```

牌上的「消息」那一行默认是开的（窗口高度也为此留着第二档），所以这条让 hook 用户
**开箱就能看到「卡在哪一步」**，不需要每个 agent 都去配一句 `--message`。

**只取一个词，绝不整条打出去。** 这是有意的：`command` 可能带密钥
（`API_KEY=xxx curl ...`），而这句话会进灯的状态、系统通知正文和消息流。所以规则是：

| 来源 | 取什么 | 例 |
|---|---|---|
| 路径类字段（`file_path` / `path` / …） | 末段 | `/a/b/server.rs` → `server.rs` |
| 搜索类字段（`pattern` / `query`） | 原样，截断 | `resolve_group` |
| `command` / `cmd` | **第一个不是 `KEY=value` 的词**，再去掉目录 | `API_KEY=sk-1 npx vite` → `npx` |
| `url` | 只留主机名 | `https://docs.rs/tauri/latest` → `docs.rs` |

一律截断到 32 个字符（按**字符**截，中文不会被切成半个字）。

**认不出来的工具名原样显示**，不按关键字猜动词 —— `TodoWrite` 里有 "write"、
`ExitPlanMode` 里有 "plan"，按关键字来会一路认错，而认错比认不出来更糟。
所以看到 `message` 是 `TodoWrite` 这种，是预期行为，不是 bug。

**没有 `tool_name` 的事件（`SessionStart` / `Stop` / `UserPromptSubmit` …）
根本不发 `message` 这个键。** 服务端是**每次上报整体替换** `message` 的，所以
`PreToolUse` 和 `PostToolUse` 都会带上同一句（这两步的 payload 都有 `tool_name`）；
只带一个的话，牌上那行会在两步之间闪一下空。

要自己给一句话，仍然用 `--message`；命令行上显式给的**优先于**自动生成的这句。

### 4.15 插件式接入（opencode / Pi / Hermes）

有几家智能体**不支持往共享配置里插 hook 条目**，但支持「启动时自动加载某个目录下的
文件」。对这几家，接入方式是**写一份我们自己的插件源码**进去 —— 不需要解析用户的任何
文件，卸载就是删掉那个文件。

这比前几节的 JSON 路线安全得多，所以是**优先选择**：没有「解析失败就中止，绝不覆盖」
那一步（那个文件本来就是我们的），也没有「用户手写的内容被重写」的风险。

| 智能体 | 装在哪 | 对方怎么加载它 |
|---|---|---|
| **opencode** | `~/.config/opencode/plugins/signalhub.js` | 启动时自动加载该目录下所有 js / ts |
| **Pi** | `~/.pi/agent/extensions/signalhub.ts` | 启动时自动发现该目录下的扩展 |
| **Hermes** | `~/.hermes/plugins/signalhub/`（清单 + 代码两份文件） | 启动时扫描 `~/.hermes/plugins/` 下的插件目录 |

前两家的路径是**一个文件**（同名文件可能是别人写的，所以只删那一个）；Hermes 的是一个
**我们独有的目录**，卸载时整删。是文件还是目录不用查表 —— 看那个路径自己是什么就行。

生成的源码里嵌入的是**我们自己解析出来的 CLI 路径**（`install hooks` 时算的那一个，
见 `hooks.rs::cli_program`；PATH 上有 `signalhub` 就用裸名字）。它把对方的事件翻译成
一条命令行：

```
signalhub <状态> --agent <名字> --session <会话> --cwd <目录> [--message <一句话>]
```

**所以这几家不需要新的 payload 映射**：走的是和 `run` / 手抄命令行完全相同的入口。
出站那条路（`signalhub clear --session <id>`）也一样。

**「装了没有」的判据是一行标记**（`signalhub-hook: <命令>`），不是「文件在不在」——
同一个路径上可能躺着用户自己写的插件，光看文件会把没装的说成装了。那一行同时也回答
「这条接入还跑得起来吗」：命令指向的程序被挪走之后，插件不报错、灯也不亮，两边都很
安静（同 §4.5 末尾那条警告）。

#### opencode

事件与状态的映射（只有这几个 —— 名字和形状都来自官方 Plugins 文档）：

| opencode 事件 | 状态 |
|---|---|
| `session.created` | `idle` |
| `permission.asked` | `permission` |
| `permission.replied` | `working` |
| `session.error` | `error`（带 `properties.error.message`） |
| `session.idle` | `done` |
| `session.deleted` | 清除该会话 |
| `tool.execute.before` / `tool.execute.after` | `working` |

**没有 `thinking`**：opencode 没有形状确定的对应事件（`session.status` 里那些字段
官方文档没写全，猜错会得到一个「永远在忙」的假灯）。宁缺勿错。

**三条约束**（生成的源码里也写着）：不许阻塞（每次工具调用都会打到这里，所以用
`spawn` + `unref`，stdio 全丢掉）、不许抛（上报失败绝不影响 opencode）、不许在不知道
会话 id 时发 `clear`（会话名会兜底成 `default`，清掉它等于把别的上报方的状态一起抹了）。

#### Pi

事件与状态的映射（名字来自官方 extensions 文档）：

| Pi 事件 | 状态 |
|---|---|
| `session_start` | `idle` |
| `before_agent_start` | `thinking` |
| `tool_execution_start` | `working`（`--message` 只取工具名，如 `bash`） |
| `agent_end` | `done` |
| `session_shutdown` | 清除该会话 |

**`session_shutdown` 映射成「清除」是对的**（和 Hermes 那边相反，见下一节）：Pi 在
`/new`、`/resume` 时会先给**旧实例**发 `session_shutdown`，之后才是新会话的
`session_start`，所以它清的正是刚结束的那个会话。这条在官方文档里写得很明白（「After
a successful switch, pi emits session_shutdown for the old extension instance…」）。

会话 id 取 `ctx.sessionManager.getSessionId()`（uuidv7）；拿不到就退回
`getSessionFile()` 的**路径** —— 同样唯一，只是长一点。两个都拿不到时按上面那条
「不知道就不发 clear」处理。

`import type { ExtensionAPI }` 是类型导入，jiti 转译时会整个擦掉（不做类型检查），
所以用户没装 `@mariozechner/pi-coding-agent` 也照样加载。

#### Hermes

这一家是**一个目录 + 两份文件**（`plugin.yaml` 清单 + `__init__.py`），整个目录都是
我们建的 —— 所以卸载是整删那个目录（同 Goose），而不是删一个文件。

| Hermes hook | 状态 |
|---|---|
| `on_session_start` | `idle` |
| `pre_llm_call` | `thinking` |
| `pre_tool_call` | `working`（`--message` 只取工具名） |
| `post_llm_call` | `done` |
| `on_session_end` | `done` —— **不是**「清除」，见下 |

**`on_session_end` 映射成 `done` 而不是「清除」**，这条最反直觉，必须写在注释里：
官方文档明说它「fires at the very end of **every** `run_conversation()` call」——
也就是**每一轮**都发，不是进程退出才发。映射成 `clear` 会让灯在每一轮结束后闪没一次。
收尾交给 TTL（会话沉默后转 `stale`，再按 TTL 删除）。

**会话 id 只认 `session_id`，绝不拿 `task_id` 顶替。** `pre_tool_call` 的文档签名里
写的是 `task_id`，看起来像会话 id，但 Hermes 源码里它是
`effective_task_id = task_id or str(uuid.uuid4())`，**每一轮都会重来**一次 —— 拿它当
会话 id，会在灯上给同一场会话每轮多出一块牌。实际上 `pre_tool_call` / `post_tool_call`
的调用点**也传了 `session_id`**（`invoke_hook(..., session_id=session_id or "")`），
只是没写进文档的参数表；官方也建议回调一律接 `**kwargs`，所以用它是安全的。
`session_id` 为空时**不发 `clear`**（会误伤 `default` 组），只会上报一个全局会话。

工作目录取插件进程的 `os.getcwd()`：Hermes 的 hook 参数里没有工作目录，但插件就跑在
它自己的进程里，那个目录就是「在哪个项目里干活」—— 按目录分组（`group_by: cwd`）靠它。

Hermes 没有「需要授权」事件，所以接不了 `permission` 那一档（同 Gemini）。

---

## 5. 降级通道（v1.1，未实现）

app 未运行时，CLI 可以把事件追加到 `~/.signalhub/pending.jsonl`，app 下次启动时回放。这给的是「app 重启不丢状态」，但会让写者从一个变成两个。v1 明确不做，需要用退出码 3 让调用方自己决定。

---

## 6. dev 调试入口（v1 已有）

Phase 1–2 阶段还没有 HTTP 和 CLI，前端内置了调试快捷键：灯球窗口获得焦点后按 `1`–`8` 直接切换状态。

| 键 | 状态 |
|---|---|
| `1` | `idle` |
| `2` | `thinking` |
| `3` | `working` |
| `4` | `done` |
| `5` | `attention` |
| `6` | `permission` |
| `7` | `error` |
| `8` | `off` |

这些快捷键直接调 Rust 侧的 `set_state` 命令，走的是与 HTTP 完全相同的 Store 路径，所以能顺带验证聚合、TTL、事件推送三条链路。仅在 `debug` 构建中注册。
