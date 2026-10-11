# SignalHub

**中文** · [English](README.en.md)

跨平台的桌面悬浮状态指示器。一组始终置顶、可拖拽的透明状态牌，用颜色和明暗节奏表示当前状态；左键点一下可以折叠成一颗单纯的灯。

优先适配 AI 智能体（Claude Code / Codex / Cursor / Gemini CLI / Goose），但**不绑定智能体**——任何程序只要能发一个 HTTP 请求就能上报状态。

## 状态

| 状态 | 视觉 | 含义 |
|---|---|---|
| `idle` | 绿、常亮 | 空闲 |
| `thinking` | 绿、快闪 | 正在推理 |
| `working` | 绿、慢闪 | 正在执行 |
| `done` | 绿、常亮 + 闪一下 | 完成（30s 后该会话消失） |
| `attention` | 黄、闪 | 需要关注 |
| `permission` | 红、平滑闪 | 需要授权 |
| `error` | 红、频闪 | 出错 / 阻塞 |
| `stale` | 灰、常亮 | 有会话，但它不说话了 |
| `offline` | 灰、更淡 | 一个会话都没有 |
| `off` | 灰、更淡 | 监控被暂停 |

优先级 **红 > 黄 > 绿**：一组在等授权时，别的组报「正在执行」不会把红灯顶掉；等授权的那一组自己恢复工作后，红又会自然消失。全局取各组里最高的优先级。

**每块牌**是一块深色圆角牌（和应用图标同一套语言）：彩点 + 组名 + 状态 + 已持续，有消息时多一行。

**分组**由上报方决定 —— 请求里带一个可选的 `group`：

```bash
curl -s -X POST ... -d '{"state":"working","session":"a","group":"work"}'
curl -s -X POST ... -d '{"state":"error","session":"b","group":"ci"}'
```

于是灯球上出现两块牌（最紧急的排在最前），全局聚合取各组里最高的那个。不传 `group` 就归 `default`，行为与单组完全一样。

三处不靠颜色的信息通道：

- **形状**：默认 `permission` / `error` 的灯是圆角方形，其余是圆形。红绿是色盲最难分的一对，而这两个恰好最需要你注意。
- **细环**：`stale` 的灯上是细环，按剩余宽限期缩短，一眼看出「沉默多久了」。
- **明暗**：默认开启「安静模式」——空闲 / 沉默 / 离线时整块牌几乎透明，只有真正有活动或需要你注意时才显形，鼠标悬停恢复。可在设置里关掉。

**外观可调**（设置面板 → 外观）：四套预设（单灯 / 紧凑 / 标准 / 醒目）一键定调，也可以逐项调——形态（展开成牌 / 折叠成一颗灯）、大小（整块牌的尺度）、形状、配色方案、背景透明度、闪烁节奏，以及牌上显示哪几段（组名 / 状态 / 已持续 / 消息）。

**每个状态都能单独调**（设置面板 → 灯效）：颜色、形状、节奏三样各自覆盖，没写的跟随「外观」页那几项全局默认；另有四套灯效预设（跟随全局 / 形状区分 / 节奏分级 / 只在需要时闪）一键定调。

**需要你注意时弹系统通知**：聚合新变成「需要授权 / 需要关注 / 出错」时弹一条，正文是项目名加智能体自己写的说明。只在跃迁时弹一次，不是每次状态变化都弹——一轮工具调用会触发几十次推送，每次都弹就成了骚扰。状态要**持续**够久（默认 5 秒，可调）才提醒，滤掉一秒内进出 `permission` 那种一闪而过。可以给通知带上提示音（macOS / Windows 上生效），可以**按组静音**（只影响通知和声音，灯照常显示），也可以设一段**勿扰时段**（只按小时，跨午夜也认；这段时间不弹也不出声，而且不补发——早上醒来收到一堆昨晚的通知正是它要避免的）。通知上还有两个按钮：**打开项目目录**和**静音该组**——不用自己回终端里找是哪个项目。

**也能转发到别处**：在「钩子」页配置若干条出站钩子 —— 状态变化时 POST 到某个地址（ntfy、Slack、自建接口）或跑一条本地命令，每条可以只对指定状态生效。异步执行、带超时，目标挂掉不影响本地。格式与安全说明见 [docs/api.md §4.9](docs/api.md)。

**左键点一下**在「一叠牌」和「一颗灯」之间切换，选择会记住。**右键点某一块牌**可以隐藏该组、静音该组、清掉该组的状态、切形态或打开设置。悬浮窗会记住自己被你拖到的位置。设置面板顶部显示今日各状态的累计时长，「状态」页还有一条近 30 分钟的时间线（每段的宽度就是它的时长）。

**每个状态都有静默超时**（`working` 90 秒、`thinking` 60 秒、`idle` 600 秒…）。超时后会话不会直接消失，而是转入 `stale`（灰、常亮），再过 5 分钟才真正删除。这样「智能体崩溃了/不说话了」和「智能体空闲待命」是两种不同的显示——不会出现「智能体早就没了，灯还在闪」的情况。

## 运行

需要 Node.js 与 Rust 工具链。

```bash
npm install
npm run tauri dev
```

首次启动会在 `~/.signalhub/`（Windows 上是 `%USERPROFILE%\.signalhub`）下生成 `port` 和 `token` 两个文件。窗口无边框、不占任务栏也不进 Dock，**退出请用菜单栏的托盘图标**。

## 上报状态

HTTP 接口（默认 `127.0.0.1:41320`，端口以 `~/.signalhub/port` 为准）：

```bash
curl -s -X POST "http://127.0.0.1:$(cat ~/.signalhub/port)/v1/state" \
  -H "Authorization: Bearer $(cat ~/.signalhub/token)" \
  -H 'Content-Type: application/json' \
  -d '{"state":"permission","session":"demo","message":"需要批准"}'
```

Windows 上没有 `$(cat …)` 这套，PowerShell 里等价的是：

```powershell
$dir = "$env:USERPROFILE\.signalhub"
$port = Get-Content "$dir\port"
$token = Get-Content "$dir\token"
Invoke-RestMethod -Method Post "http://127.0.0.1:$port/v1/state" `
  -Headers @{ Authorization = "Bearer $token" } `
  -ContentType 'application/json' `
  -Body '{"state":"permission","session":"demo","message":"需要批准"}'
```

或者用 CLI：

```bash
signalhub working --session build-1 --agent ci --message "compiling"
signalhub status
signalhub watch                                   # 终端里跟随状态变化
signalhub watch --until done --timeout 10m        # CI 里等某个状态
```

完整接口规范见 [docs/api.md](docs/api.md)。架构与设计取舍见 [docs/architecture.md](docs/architecture.md)。

## 接入智能体

```bash
signalhub install hooks claude-code      # 也支持 codex / cursor / gemini / goose
signalhub install hooks claude-code --dry-run   # 先看看会改什么
```

它会往智能体的配置文件里写 hook（Claude Code 是 `~/.claude/settings.json`）。**只动 `hooks` 这一个键**，其余内容原样保留，写之前自动备份到 `*.signalhub-backup`；重复执行不会产生重复条目，命令变了会原地更新；配置不是合法 JSON 时会直接中止、绝不覆盖。卸载用 `signalhub uninstall hooks claude-code`。

**Goose 是个例外**：它走插件机制，所以装的是我们自己的一个目录（`~/.agents/plugins/signalhub/`），不碰你的任何共享配置 —— 卸载时整份删掉。其余几家才是在共享配置里插一条。

装完重启智能体即可生效。之后提问 → 绿快闪、调工具 → 绿慢闪、需要授权 → 红闪、答完亮一下再回常亮。各家的差异（哪家没有「需要授权」事件、哪家的字段名不一样）见 [docs/api.md §4](docs/api.md)。

建议先把 CLI 放进 PATH，这样写进配置的是可移植的裸名字而不是绝对路径：

```bash
ln -sfn "$(pwd)/src-tauri/target/debug/signalhub" ~/.local/bin/signalhub
signalhub install hooks claude-code
```

Windows 上没有软链接这一步，把 `src-tauri\target\debug`（`signalhub.exe` 就在那里）追加进**用户** PATH 即可，加完重开一个终端：

```powershell
$user = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path',
  "$user;$(Resolve-Path src-tauri\target\debug)", 'User')
```

绝对路径也能用，但二进制一挪动 hook 就会静默失效（智能体会话里什么都看不到），排查起来很费劲。

完整接口规范见 [docs/api.md](docs/api.md)。架构与设计取舍见 [docs/architecture.md](docs/architecture.md)。

## 进度

已完成：悬浮灯球、状态聚合与 TTL、HTTP 上报接口、CLI（含 `watch`）、设置面板、hook 自动接线（Claude Code / Codex / Cursor / Gemini CLI / Goose）、系统通知与提示音、通知动作按钮、窗口位置记忆、展开 / 折叠两种形态、每组右键菜单、今日时长统计与近 30 分钟时间线、出站钩子转发、每个状态的颜色 / 形状 / 节奏可单独调、按组静音、勿扰时段、持续多久才提醒、托盘图标跟随状态、单实例、开机自启、全局快捷键、点击穿透。

待做：把 CLI 作为 sidecar 打进 app bundle（分发才需要）。
