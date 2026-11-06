# 待办与已知边界

> 2026-11-06 整理。**这份清单把散在 7 份 plan 文档、README 和 architecture 里的
> 「还没做 / 还没验 / 刻意不做」收拢到一处** —— 之前 README 的「待做」只写了签名
> 与公证一条，严重低估了实际欠账。它不是施工图：每条都标了出处，动手前回原文档
> 看「为什么」。状态以代码为准；文档之间打架的地方集中在 §5。

## 0. 怎么读

| 分类 | 含义 |
|---|---|
| **待实现**（§1） | 确实没做、也没有人说过不做 |
| **待验收 / 外部条件**（§2） | 代码写了，或需要证书 / 真机；没有实证就别对外说「已完成」 |
| **平台限制**（§3） | 只在部分平台可用，是已知降级，不是 bug |
| **刻意不做**（§4） | 文档里写明的设计取舍，**不要当欠账补** |

不用 checkbox：项目文档习惯用表格 + 状态列；勾选框会给一份没人维护的清单伪造进度感。
`docs/` 只写中文（见 `architecture.md` §3.20），这份也一样。

改动状态时同步两处：本文件，以及 README / README.en 的「进度」章节（它们指向这里）。

---

## 1. 待实现

### 1.1 审批与「不在桌前」

| 项 | 现状 | 出处 |
|---|---|---|
| **A5 跨机审批** | `remote.rs` 只拉对端 `GET /v1/state`，不拉 `/v1/approvals`，也不回传决定 | `plan-approval.md:180`、`:224-226`；`README.md:288` |
| **A6 精确白名单「记住这个目标」** | 完全没有；**条件式**：文档说 A1–A5 跑下来重复率不高就不做 | `plan-approval.md:193-204` |
| **A8 审批统计** | 部分实现：次数 / 允许·拒绝·超时 / 最长等待已有（`digest.ts:27-34`），缺**中位等待**与**比例** | `plan-approval.md:208-211` |
| **A7 子项「新文件 / 覆盖」标签** | `ApprovalBubble.tsx` 只渲染详情文本，没有 `stat` 判断 | `plan-approval.md:127-129` |
| **Away approvals：PWA + 手机主动推送** | 面板上审批（A4，`web_approve`）**已完成**；ntfy / Bark / PushDeer 转发预设**已完成**；缺 manifest、service worker、推送订阅 | `plan-round5.md:56-57`；全库无 PWA 关键字 |
| **A1 内存环形缓冲 500 条** | 刻意改成**只有文件落盘**，命令层的 500 只是读取上限 | `plan-approval.md:41-44` |

> ⚠️ **A5 有文档冲突**：`plan-approval.md:224` 写「下一步是 A5」，
> 而 `plan-multi-machine.md:274` 与 `architecture.md:935` 写「不做远程审批」。
> 开工前先定「谁有权点」，见 §5。

### 1.2 离开回来摘要的三个子项

`plan-round5.md:54-55`：事件流水（否则离开期间出现过又消失的会话看不到）、
`git diff` 摘要、每行「等了多久」改成悬停提示（现在是行内文字）。
第一版摘要**已落地**（`architecture.md` §3.25.1），这三件没有。

### 1.3 plan-round4 里没排期的提案

这些**都不在** `plan-round4.md:211-229` 的 17 项实施记录表里 —— 是最容易被漏掉的一批：

| 编号 | 项 | 现状 / 出处 |
|---|---|---|
| C1 | 灯上动作扩展（长按静音 10 分钟、菜单加专注 / 离开） | v0.5.0 的手势重做是**另一种形状**（`plan-round5.md:66-67`）；`plan-round4.md:128-130` |
| C5 | permission 与 error 不同提示音、可试听 | 提示音仍是单一 `Settings::sound`（`architecture.md:394`）；`plan-round4.md:145` |
| E3 | 命令面板 ⌘K | `plan-round4.md:168` |
| E4 | 接入页空状态引导 | `plan-round4.md:169`；README 的「看演示」是演示模式，不是这项 |
| E5 | i18n key 校验测试 | `plan-round4.md:170`；目前只靠 `tsc` 的 `Record<Lang, Messages>` 兜底，没有运行时测试 |
| E6 | Playwright 视觉回归（`toHaveScreenshot`） | `plan-round4.md:171` |
| B2 | 其余内置采集器 | 只做了「会议中」+「备份（Time Machine）」两个（`plan-round4.md:593-630`）；低电量 / 屏幕锁定 / 媒体播放等候选未做 |
| D3 | 通知带动作按钮 | **代码已做**（`architecture.md:738`、`:1258`），但**真机未验**，见 §2 |

D2（ntfy / Bark 预设）已作为**转发 preset** 落地（`plan-hub-round2.md:429-450`），
别误记成待办。「第二送达通道」这个提法与转发预设不是一回事。

### 1.4 灯与接入的长尾

- **更多 agent 适配器**：内置 10 家。Droid(Factory) / Qwen Code / iFlow 是「留给下一轮，本轮不猜」（`plan-hub-round2.md:79-82`）；**VS Code 系（Cline / Roo / Continue / Windsurf / Kilo）接不了**，见 §4。
- **语义化自由 resize**（拖了就记住）：`plan-lamp-expansion.md:150-157`，「只在用户真要这个手感时做」。
- **P3-5 折叠态悬停出牌面**：同上 `:289`，「复杂度中等，排最后」。
- **只读面板改长连接**：现为 1 秒轮询（`plan-approval.md:176`）。文档**没有**写过「不做 SSE / WebSocket」，只写了「不另起推送通道」，所以这条是「现状」而非明确欠账，别当成被承诺过的功能。

### 1.5 文档自身的同步欠账

- `architecture.md:1230-1286` 的验收矩阵只收录到 **9f**（0.2.x 时代），0.3 / 0.4 的收件箱、审批、多机、演示模式都没进表。
- `plan-round4.md:688 / :706` 与 `state.rs:1499` 仍写着「Windows 上 pid 入库即丢 / 探活只在 Unix 生效」，已被 `proc.rs:47`、`state.rs:343-345`、`plan-round5.md:85` 推翻 —— 那是 0.3.2 时代的历史记录，别据此判断服务端要不要改。
- `plan-round3.md:214` 引用「`api.md` §5 还写着没做」，而 `api.md:1363` 已标「已实现」。

---

## 2. 待验收 / 外部条件

| 项 | 卡在哪 | 出处 |
|---|---|---|
| **签名与公证** | 要 Apple / Windows 证书，不是代码问题；`release.yml` 里只有**注释占位**，没有空 YAML step | `README.md:301`；`plan-round5.md:91` |
| **Linux / Windows 自动更新** | Windows 被上游 `zip ^4` 全量 yank 卡死；Linux 代码路径在、`SUPPORTED` 刻意只写 macOS，没真机验过 | `README.md:303`；`update.rs:43`；`plan-round3.md:386-391` |
| **AppImage 解包跑 `--version`** | macOS 上打不开 squashfs，这轮没做 | `plan-round3.md:402-405` |
| **`releases/latest` 端到端更新** | 要真发一版更高版本才能验；下一次正常发版就是那次验收 | `plan-round3.md:372-374` |
| **Linux 包真机** | 没有 Linux 真机；`.deb` 只解包看过 | `plan-round3.md:39`、`:396` |
| **通知动作按钮** | dev 跑的是裸二进制，`UNUserNotificationCenter` 需要 bundle id；「按钮出现且点了触发」无实证 | `architecture.md:753` |
| **Windows 灯球拖拽** | 窗口配了 `focus: false`，理论可行但没真机试过；**拖动是基本操作，优先验** | `architecture.md:1204` |
| **CSC 真机（#22）** | 适配器写了，没在真机上跑过 | `plan-round5.md:95` |
| **B4 预设 2 / 会议蓝配色（#21）** | 真窗口视觉验收 | `plan-round5.md:94` |
| **多 tab / tmux / VS Code 跳转** | 解析层有单测，端到端要真机逐一点过 | `plan-round5.md:45-46` |
| **Windows 命令式 hook 的 pid** | 2026-11-06 已实现（CLI 自己向上找祖先进程），**但 Windows 真机未验** —— 见 §3 与 `win_pid.rs` 头注释 | `plan-round5.md:94` |

`plan-round4.md` 里还有一串「没验证到的」小项：`:395-400` 真按键 / 辅助功能授权、
`:442-445` 托盘菜单与 25 分钟、`:502-511` 审批气泡、`:555-557` 按住 800ms 手感、
`:627-628` 真实开麦 / 摄像头 TCC、`:671-673` B4 三条规则、`plan-lamp-expansion.md:273-277` 六条真机手测清单。

---

## 3. 平台限制（已知降级，不是欠账）

| 限制 | 现状 | 出处 |
|---|---|---|
| 精确定位终端 | **仅 macOS**；Win / Linux 只能「打开项目目录」 | `jump.rs:45`、`:64-73`；`README.md:99` |
| 系统采集器 | **仅 macOS**，且只有会议 / 备份两个；`supported()` 为假时连开关都不摆 | `collect.rs:72-77`、`:214-219`；`plan-round4.md:610` |
| Linux 提示音 | 恒关（dunst / GNOME 不认 XDG `sound-name`），设置等于没接 | `alerts.rs:263`；`architecture.md:394-396` |
| 从灯上审批 | **只认 Claude Code + HTTP 式接入**；其余智能体（含 WorkBuddy）确认框仍留在自己界面 | `server.rs:550`；`api.md:1181-1192`；`README.md:107` |
| 远程会话 | **只读**：不能审批、不能跳转 | `README.md:288`；`plan-multi-machine.md:274-275` |
| 没有事件流水 | 「离开这段时间」「今日复盘」都基于快照 | `awaySummary.ts:44-47`；`README.md:136` |
| 审批面板的暴露面 | `web_approve` **不是安全边界**，真正的门是 token | `plan-approval.md:163-166`；`README.md:97` |

**Windows 命令式 hook 的 pid（2026-11-06 变化）**：Windows 的进程探活
（`OpenProcess` + `WaitForSingleObject`）本来就生效，缺的是没人送 pid。现在 CLI 在
Windows 上自己沿父进程链找第一个非 shell 祖先，找不到就**不报 pid、退回 30 分钟
兜底**（和改之前一样，绝不会更糟）。Unix 侧仍走 `SIGNALHUB_PID="$PPID"` 前缀。
**已装的 hook 命令一个字都不用改**，所以也不触发「旧版需重装」提示。
⚠️ 父进程派生方式仍**没有 Windows 真机实测**，见 `win_pid.rs` 文件头。

---

## 4. 刻意不做（设计取舍，别当欠账补）

| 项 | 理由（原文口径） | 出处 |
|---|---|---|
| 表达式 / 规则编辑器 | 「只做预设 + 开关」「宁可少而精」 | `plan-round4.md:120-122`、`:193` |
| 通配白名单 / 自动批准一切 | 配置地狱 + 绕过风险；违背产品立场 | `plan-approval.md:214-215`；`plan-round4.md:451` |
| 快捷键可配 | 真故障是被别的程序占了，可配也解决不了 | `plan-round4.md:380-382`；`README.md:101` |
| 统计趋势线 / 同比 | 这一屏只回答「我这几天在忙什么」，0.5 秒要看懂 | `architecture.md:492` |
| 勿扰延后补发 | 「早上醒来面对一整夜攒下的通知」 | `architecture.md:708` |
| 远程跳转 / 中继 / 公网穿透 | 另一个量级的产品；只在能直连的局域网 / VPN 内工作 | `plan-multi-machine.md:273-277`；`architecture.md:935` |
| 每盏灯独立外观 / 跨组拖拽编组 | 多灯拓展的明确边界 | `plan-lamp-expansion.md:253-262` |
| 「永不过期」TTL 档 | 不提供「永不消失」 | `plan-round3.md:40-41`、`:179` |
| 团队多级审批 / LLM 生成拒绝理由 / 拦 `PreToolUse` | 不是这个产品的用户 / 隐私 / 会拦下本不需批准的事 | `plan-approval.md:216-218` |
| VS Code 系适配器 | 只活在侧边栏面板里，没有可包的进程、也没有可写的 hook 配置 | `plan-hub-round2.md:83-85`；`architecture.md:1374` |
| 通用自定义 agent 里的插件式三家 | `opencode` / `pi` / `hermes` 暂不支持 | `api.md:1330`、`:1352-1354` |
| 不做二次确认 / 不做跨组拖拽编组 | 低频幂等操作 / 复杂度 | `plan-hub-round2.md:293` |

`plan-round4.md:188-194` 还有一排红线：不自动升级不自动安装、HTTP hook 不阻塞
agent、新增外发默认关、采集器只读系统状态、Web 面板默认只绑 127.0.0.1。
`plan-multi-machine.md:124`「**不主动清理**掉线的机器」也是取舍。

---

## 5. 文档之间打架的地方

| # | 冲突 | 处理 |
|---|---|---|
| 1 | `plan-round5.md:47` 说「没有做离开回来后的摘要」、`:51` 又用删除线标「第一版已落地」 | **2026-11-06 已修**：`:47` 改成「第一版已落地、还缺三个子项」 |
| 2 | `plan-approval.md:144` 的 A4「手机审批（已完成）」与 `plan-round5.md:56` 的 Away approvals 被当成同一件事 | **2026-11-06 已修**：两处各加一句区分 —— A4 是「面板上能点」、Away approvals 是「PWA + 主动推送」 |
| 3 | **A5 跨机审批**：`plan-approval.md:224` 写「下一步」，`plan-multi-machine.md:274` / `architecture.md:935` 写「不做远程审批」 | **待决**：先定「谁有权点」。在定下来之前，本清单把它列在 §1 并标注冲突，不擅自当成已批准的施工图 |
| 4 | `README.en.md:111` 写「进程还活着 → 转 idle」，中文 `README.md:117` 与 `state.rs` 都是「保持最后的状态」 | **2026-11-06 已修**：英文版改成与中文一致 |
| 5 | `plan-round3.md:214` 引用「`api.md` §5 还写着没做」，实际已实现 | **2026-11-06 已修**：标明那是起草时的状态 |
| 6 | `architecture.md` 验收矩阵只到 9f | 待补，见 §1.5 |
