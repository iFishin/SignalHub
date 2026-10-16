# 信号中枢 4 号计划（提案）：从「Agent 信号灯」到「桌面信号中枢」

> 2026-10-16 起草。**这是一份提案，不是已批准的施工图** —— 等你勾选方向后再逐项开工。
> 目标：让信号中枢不止服务 AI Agent，而是成为**桌面上那个「谁在忙 / 谁在等我 / 系统在干什么」的唯一入口**；
> 同时把「更易用」落在具体的小改动上（第一项就是解决你今天的困惑）。
>
> 调研来源分三类标注：**【已验证】**（我实际打开过官方文档/仓库）、**【调研】**（子代理检索到，附链接）、**【待实测】**（本机可验证但还没测）。

---

## 0. 一句话结论

现在最大的两个「不实用」不是功能少，而是：

1. **接入是黑盒** —— 点完「接入」之后，你看不到「它到底有没有在报」。今天你问「opencode 怎么还是无法接入」，根因就在这里：
   文件是新的、opencode 日志里加载成功、app 自检说「已存在且命令相同」、只是**界面没有把「最近一次上报」显示给你**，而当时 app 还没在跑。
2. **信号源只有 Agent** —— 而 HTTP 上报接口（`POST /v1/state`）本来就是通用的，缺的是「信号源」这个概念和几个系统采集器。

---

## 1. 调研：同类项目值得偷的设计

### 1.1 状态指示类

| 项目 | 链接 | 可偷的设计 |
| --- | --- | --- |
| m1ckc3s/claude-status-bar | https://github.com/m1ckc3s/claude-status-bar | 最差状态优先聚合；无状态进程随会话自启自灭；hook **fail-open**（钩子崩了绝不拦 agent） |
| guan-ops/Agent-Signal-Bar | https://github.com/guan-ops/Agent-Signal-Bar | 十态信号语言；**红黄状态受保护**（不会被低优先级覆盖）；**stale（灰/黄）显式表达「无信号」** |
| AdilHoumadi/claude-status-bar | https://github.com/AdilHoumadi/claude-status-bar | 浮屏 worst-first 红绿灯 + `+N` 溢出 chip；**"no fake data"**（没有真实数据就不画） |
| michalstrnadel/AgentBar | https://github.com/michalstrnadel/AgentBar | 文件协议 + CLI 双通道上报；规则从 `watching` 起步；改配置前**先备份 + 给 diff** |
| swiftbar/SwiftBar | https://github.com/swiftbar/SwiftBar | **纯文本插件协议**（标题 / `---` 分隔 / `param=value`）；`exit 1`→⚠️，**空输出→不显示**（而不是绿灯） |
| JnyJny/busylight | https://github.com/JnyJny/busylight | 跨品牌物理灯统一抽象 + **动词化 API**（on/blink/off）+ 设备 keepalive/重连建模 |
| wled/WLED | https://github.com/wled/WLED | JSON API + preset/playlist；MQTT（Home Assistant 自动发现） |
| Flipper BUSY Bar | https://docs.busy.app/bar/dev/http-api | **声明式元素数组**（display/draw）；USB/蓝牙/云共用一套 HTTP API |
| caronc/apprise | https://github.com/caronc/apprise | 一个 URL schema 打通上百种送达方式（含物理灯）；tag 优先级升级链；**失败隔离** |
| binwiederhier/ntfy | https://github.com/binwiederhier/ntfy | HTTP pub/sub 极简契约 + topic + 自托管，做「第二送达通道」 |
| JnyJny 之外：busylight 生态 | 见上 | 把「物理灯」当第一等输出设备，而不是外挂脚本 |

**共性结论（直接可用）**：
- 「无信号」必须**显式**表达（灰/黄 + 文案），不能默认绿灯 —— 这正是我们今天缺的。
- 聚合要**最差优先**，且**红黄受保护**（不能被 finished 覆盖）—— 现有 `Signal::priority` 已是这个思路，可以在 UI 文案上说明。
- 插件/上报协议越简单越好（stdout 一行 / 一个 POST），**任何脚本都能当信号源**。
- hook 一律 fail-open，超时要短，绝不能因为信号中枢把 agent 挂住。
- 变更前**备份 + diff**（现有 `install hooks` 已有备份思路，可以显式给 diff）。

### 1.2 Tauri 设计美学（【调研】）

- 案例：**Cap**（https://github.com/CapSoftware/Cap，Tauri v2 + nspanel 浮层 + 独立的录制控制 HUD）、**Handy**（https://github.com/cjpais/Handy，常驻 + 全局快捷键 + push-to-talk）——共同点是**主窗口克制、把「控制面」做成独立小窗**，而不是把所有东西塞进一个窗。
- 可用的包：`window-vibrancy`（macOS 毛玻璃）、`tauri-plugin-positioner`（把窗贴到托盘/边缘）、`tauri-nspanel`（真·NSPanel 浮层，非激活面板）、托盘 API、`global-shortcut`、`autostart`、`store`、`single-instance`、`updater`。
- macOS 限制：Tauri 的托盘是 NSStatusItem，**能**做菜单+图标，**不能**做自定义菜单栏视图（不是 SwiftUI 的 MenuBarExtra）；Windows/Linux（AppIndicator）行为有差异 —— 所以「信息密度」还是得靠悬浮灯窗口，而不是托盘。

### 1.3 前端与能耗（【调研】+【待实测】）

- **透明 + always-on-top 的常驻窗口在 macOS 上可能触发每帧合成**（子代理引到 tauri issue #15471，称 GPU 功耗可达数倍）。**本机没测过，所以写成第一步实测**。
  本项目已有两条相关红线（「动画只动 opacity」「done 不闪」），方向是对的，但**从未量过数字**。
- 事件推送：现在是广播全量快照（`app.emit("signalhub://state", …)`），安静时也在发；可加「**没变就不发** + 合并 100ms」。
- 交互：命令面板（⌘K）对 9 个页签的设置窗收益很大；`prefers-reduced-motion` 已有 ✓。
- 质量：视觉回归（Playwright `toHaveScreenshot` / Vitest）能兜住「几何两处真值」这条红线；i18n key 校验测试。

---

## 2. 现状盘点（避免重复提案）

**已经有**：8 个 agent 接入（Claude Code / Codex / Cursor / Gemini / Goose / opencode / Pi / Hermes）；多信号灯 + 分组 + 拖拽排序 + 就地改名；悬停摘要 + 右键菜单（灯 / 组两级）；暂停；离线补报（spool）；转发（HTTP + 本地命令 + 日志 200 条）；桌面通知 + 声音 + 通知权限；统计 + 历史（按天）；时间线；双语；自动更新（仅 macOS）。

**缺口**（本计划要补的）：
| 缺口 | 影响 |
| --- | --- |
| 接入页没有「回执」（谁在报 / 最后一次多久前） | 就是今天的困惑：接入成功与失败看起来一样 |
| 没有「试一下」 | 无法 30 秒内自证链路通 |
| 状态源只有 agent | 「系统对接」无处落脚 |
| 没有只读 Web 面板 | 不能离开电脑看（手机/平板） |
| 没有任何「反向通道」 | 灯只能看，不能点回去改变什么（审批/专注） |
| 无「人」的信号 | 番茄钟/开会/离开这类场景无法表达 |

---

## 3. 提案 A：让「接入」看得见（最小改动、最高性价比，建议先做）

**A1. 接入回执（每行显示「最近一次上报」）** —— 小
- 目标：接入页每个 agent 行显示 `最近上报 12 秒前 · 今天 348 次` 或 `已接入 · 从未上报`。
- 做法：数据源用已有的 `~/.signalhub/hooks.log`（每次 hook 调用都写一行）；新建一个 `last_seen.rs` 或在 `stats.rs` 里加聚合（agent → last_ms / count_today）；前端 `useAgents` 顺带取。**不引入新数据库**。
- 坑：`hooks.log` 会轮转，解析要容错；「从未上报」和「app 没在跑」要分开说（app 没跑时用 hook 日志也拿不到新的，所以文案要写成「自上次启动以来」）。
- 验收：故意卸载某个 agent → 该行立刻显示「已接入（文件在）· 从未上报」；真的用一次 → 数字在 2 秒内更新。
- **这一条直接回答你今天的问题。**

**A2. 「试一下」按钮（30 秒自证）** —— 小
- 点击后：app 自己往该 agent 的会话注入 `idle → working → done` 三条信号（或走一次真实的 hook 命令，若已配），灯当场演示一遍。
- 坑：要明确标注「这是内部测试，不代表 agent 真的在跑」；测出的会话不要留在历史里（用 `signalhub-test-*` 会话名 + 测完自动 clear）。

**A3. 一键诊断（可复制文本）** —— 小
- 打包：命令是否在 PATH、配置文件路径 + 内容、最近 20 条 `hooks.log`、app 端口/token 是否存在、当前是否有 app 在跑。现在的 hooks 日志说明已经很好了，缺的是「一键拿到」。
- 顺带：把「没有 app 在跑 → 事件会进 spool，重启后补报」这条已经实现的行为在界面上说出来（今天你遇到的就是这个）。

**A4. Claude Code 默认改用 HTTP hook（`type: "http"`）** —— 中，**但价值最大**
- **【已验证】** Claude Code 支持 `type: "http"` 的 hook：事件 JSON **作为 POST body** 发到 URL，**返回值用 HTTP 响应体 JSON** 表达决定；字段有 `url` / `headers` / `timeout` / `onFailure`；另有 `allowedHttpHookUrls` 白名单（**只有定义了才生效**）。官方文档：https://code.claude.com/docs/en/hooks#http-hook-fields
- **为什么这是「易用」的关键**：现在最大的接入失败源是「命令不在 PATH」（`signalhub` 没装好 / GUI 启动的 PATH 和你 shell 里不一样）。HTTP 方式**不需要任何 PATH 上的东西**，直连 `http://127.0.0.1:PORT/v1/hooks/claude-code`。
- 新增端点：`POST /v1/hooks/{agent}`（**仅绑定 127.0.0.1；report-only；body ≤ 64KB；无 token**，理由见红线）；把 CC 的 `hook_event_name` 映射成状态：`SessionStart→idle`、`UserPromptSubmit→thinking`、`PreToolUse→working`、`PermissionRequest / Notification→permission`、`Stop→done`、`SessionEnd→clear`；`session_id`、`cwd`、`permission_mode` **【已验证】**都在 payload 里（`agent_id/agent_type` 也在，可区分子代理）。
- 坑：① 必须 `timeout: 5` + `onFailure: "continue"`，**绝不能** block agent；② 映射表要按**真实 payload** 逐条验证后再写进文档（沿用本项目「只写验证过的映射」纪律）；③ 保留现有 CLI 方式作为备选，安装时给用户一个「命令方式 / HTTP 方式」选择，不静默改写已有配置。
- 验收：把 `~/.local/bin/signalhub` 临时移走，HTTP 方式仍能点亮灯；CC 里 hook 超时/失败时 agent 行为完全不受影响。

---

## 4. 提案 B：系统对接（不止 Agent）

**B1. 把 `agent` 泛化成「信号源 source」** —— 中
- 数据模型向后兼容：现有上报的 `agent` 字段保留，新增 `source: "system"`（缺省 `"agent"`）。接入页分成三组：**AI Agent / 系统 / 自定义**。
- 好处：一个概念换来后面所有系统采集器的统一入口；`/v1/state` 契约不变。

**B2. 内置采集器（每个 = 一个开关 + 一个测试）** —— 建议先做 2 个样板
- **会议中**（macOS：麦克风/摄像头占用）→ 「会议」组，开会时自动变蓝/静音通知。
- **Time Machine 备份中**（`tmutil status`）→ 「备份」组，跑完自动 done。
- 其它候选（每个都给实现路径 + 可信度）：专注/勿扰（Focus）、屏幕锁定/休眠（自动「离开」）、长命令（给用户一个 `signalhub run -- <cmd>` 包装器）、媒体播放（Linux MPRIS ✓ / Windows SMTC ✓ / macOS 无公开 API，**平台差异**）、低电量、日历事件（EventKit 需授权）。
- 坑：采集器**只读系统状态**，不读文件内容；间隔 ≥ 5s；任何一个采集器崩了不能影响主进程（独立线程 + 吞错 + 记 hooks.log）。

**B3. 只读 Web 面板 `/`** —— 中
- 手机/平板打开 `http://<mac>:<port>/?t=<token>` 看灯 + 最近事件；复用 `/v1/state` 快照，纯静态单页。
- **默认只绑 127.0.0.1**，要局域网访问必须在设置里显式打开（并显示「同一 WiFi 下任何设备都能看」的警告）。这是「拓展使用场景」最直观的一步。

**B4. 极简规则（只做 3 个预设，不做表达式引擎）** —— 中
- `permission 持续 > 60s → 手机推送`；`有会话在跑 → 自动暂停通知`；`组=会议 → 灯变蓝 + 静音`。
- 现有「转发」是逐条全量；规则如果做成通用 DSL 会违背「宁可少而精」，所以**只做预设 + 开关**。

---

## 5. 提案 C：交互互动（灯不只是看的）

**C1. 灯上/组上的动作扩展**（右键菜单已有 ✓，加动作）—— 小
- 单击灯 = 清除当前最差组；长按 = 静音该组 10 分钟；菜单加「专注 25 分钟」「离开」。
- 隐藏收益：**让信号中枢对「人」也有用**，不只是 agent。

**C2. 全局快捷键**（`tauri-plugin-global-shortcut`）—— 小
- 默认：`⌥⇧L` 显示/隐藏灯、`⌥⇧M` 静音、`⌥⇧0` 清空。冲突要检测 + 提示占用。

**C3. 从灯上审批（旗舰，但放二期、默认关）** —— 大
- **【已验证】** Claude Code 的 `PreToolUse` / `PermissionRequest` hook 可以返回 `permissionDecision: allow | deny | ask | defer`（还有 `updatedInput` / `permissionDecisionReason`）。文档：https://code.claude.com/docs/en/hooks#permissionrequest
- 玩法：灯变黄 = 有工具调用在等确认 → 点灯弹出小气泡「将执行：`rm -rf build/`」→ 允许/拒绝 → 用 hook 的 HTTP 响应把决定回给 CC。
- **代价必须说清楚**：hook 会**挂住那次工具调用**直到你点击或超时（所以要给短超时，超时自动返回 `ask` 交回 CC 自己问）。
- 安全：① 默认关闭，且**每次开启只对当前会话生效**；② 审批端点要求 token（与 report-only 端点分开）；③ 只允许「白名单动作模式」，拒绝规则永远优先。**这是全计划唯一会改变 agent 行为的功能**，所以放二期。

**C4. 手动状态 / 番茄钟 / 离开** —— 小
- 「专注 25 分钟」= 一个倒计时占一盏灯（到点提醒并转 done）；「离开」一键；「开会中」手动。
- 让信号中枢在**没有任何 agent** 时也能用 —— 这是「拓展场景」的最低成本证明。

**C5. 声音细化** —— 小：permission 与 error 用不同提示音；可试听。

---

## 6. 提案 D：送达与外设

**D1. 物理灯（WLED / busylight / Stream Deck）** —— 中，最有意思
- 一个「外设」页：填 WLED 的 IP → 状态色直接 POST 到它的 JSON API（【调研】https://github.com/wled/WLED）。busylight 用动词化 API（on/blink/off）。
- 隐私红线：**只发状态色，绝不发会话名/路径**。

**D2. 第二送达通道** —— 小
- PushDeer 已有 ✓；补 **ntfy**（可自托管、一条 POST）和 Bark 预设。

**D3. 通知里带动作** —— 视平台能力，macOS 通知按钮受限，退路是「点击通知 → 打开设置/该组」。

---

## 7. 提案 E：易用与性能

| # | 项目 | 成本 | 说明 |
| --- | --- | --- | --- |
| E1 | **量能耗（第一步，数据驱动）** | 小 | 【待实测】用 `top -o power` / `powermetrics` 对比「动画 on/off」「透明窗 vs 不透明」；若显著，考虑静止时完全停动画（done 已是 none ✓）、降低合成层。**没有数字就不动结构** |
| E2 | **事件合帧** | 小 | 快照没变就不 emit + 100ms 合并；顺带减少前端 re-render（`useStats/useTimeline` 是挂载取一次 + 定时兜底，已克制 ✓） |
| E3 | 命令面板 ⌘K | 中 | 设置窗 9 个页签 + 搜索会话 + 执行动作（清除/静音/专注） |
| E4 | 接入页空状态引导 | 小 | 「还没有任何信号源 → 点这里接入第一个」+ A1 的回执 |
| E5 | i18n key 校验测试 | 小 | 中英 key 齐全、无遗漏（现有双语是手写的，值得兜底） |
| E6 | 视觉回归 | 中 | 各状态灯截图比对，兜住「几何两处真值」红线（Playwright `toHaveScreenshot`） |

---

## 8. 建议的落地顺序（等你点头）

1. **A1 + A2 + A4**（接入回执 / 试一下 / HTTP hook）—— 直接解决今天的困惑，改动都在 `hooks.rs` + 接入页。
2. **E1 + E2**（量能耗 + 合帧）—— 常驻应用的基本功，有数字才改结构。
3. **B1 + B3**（信号源概念 + 只读 Web 面板）—— 「不止 agent」的第一步，可以演示给任何人看。
4. **D1**（WLED 物理灯）—— 最有意思的对外接口，实现很轻。
5. **C2 + C4**（快捷键 + 番茄钟）—— 把「人」也当成信号源。
6. **二期**：C3（灯上审批）、B4（预设规则）、B2 其余采集器。

---

## 9. 红线（延续 + 新增）

1. 不自动升级、不自动安装（已定）。
2. **HTTP hook 必须 `onFailure: "continue"`、`timeout ≤ 5s`，任何情况下不阻塞 agent**；report-only 端点无 token 但**只绑 127.0.0.1**、限 body 大小、只能写状态；审批端点（C3）必须 token。
3. 新增外发（WLED / ntfy / Apprise）**默认关**，且**只发状态，不发会话名/路径/命令内容**。
4. 采集器只读系统状态，不读文件内容；单个采集器失败不影响主进程。
5. Web 面板默认只绑 127.0.0.1，开局域网必须显式确认。
6. 不引入通用规则表达式引擎（宁可少而精）。
7. 几何仍是两处真值（Rust + CSS），改一处必须同一次提交同步另一处。

---

## 10. 三个待定项 —— 已定（2026-10-16）

1. **方向**：按 §8 的顺序**依次全部实施**（不再逐项询问）。
2. **Claude Code 的 HTTP hook**：做，但**不静默改写**已有配置 —— 接入页给「命令方式 / HTTP 方式」二选一，默认仍是命令方式，HTTP 方式作为可选项（它免去「命令不在 PATH」这一整类失败）。
3. **只读 Web 面板与物理灯**：默认关，用户显式打开。

---

## 11. 实施记录（依次推进）

> 每完成一项就更新这里：状态 + 提交号。**每项都要过 gate**（clippy -D warnings、cargo test、npm test、npm run build），
> 不合批提交，便于回退。

| 顺序 | 项 | 状态 | 提交 |
| --- | --- | --- | --- |
| 1 | A1 接入回执（每个信号源「最近上报 X 前」） | 完成 | `a2facf5` |
| 2 | A2 「试一下」（真实调用接入命令 / 插件型走内部注入） | 完成 | `4d51ce2` |
| 3 | A3 一键诊断（可复制文本 / `signalhub diagnose`） | 完成 | `2acde12` |
| 4 | A4 Claude Code HTTP hook（`POST /v1/hooks/{agent}`，接入页可选） | 待办 | |
| 5 | E1 能耗实测（有数字才改结构） | 待办 | |
| 6 | E2 事件合帧（没变就不 emit + 合并） | 待办 | |
| 7 | B1 信号源概念（agent/system/自定义） | 待办 | |
| 8 | B3 只读 Web 面板（默认关） | 待办 | |
| 9 | D1 物理灯（WLED 等，只发状态色） | 待办 | |
| 10 | C2 全局快捷键 | 待办 | |
| 11 | C4 手动状态 / 番茄钟 / 离开 | 待办 | |
| 12 | 领域外的收尾（B4 预设规则、B2 其余采集器、C3 灯上审批） | 二期 | |

**施工中发现的问题**：一律开 GitHub issue 存档（`gh issue create`），再在本表下面记一行「issue #N → 结论」。

- **issue #2 → 已修**：诊断文本会把转发钩子的密钥（`pushkey=…`）一起复制出去。
  修法是**在诊断输出里脱敏**（URL 的 query 只留 `?…`、敏感 `key=value` 只留键名、
  家目录缩成 `~`），并在 `hooks.log` 建文件时改成 **0600**。日志写入本身不动 ——
  日志是排查工具，「它到底发了什么」必须看得见，要收窄的是「复制出去」这条路。
