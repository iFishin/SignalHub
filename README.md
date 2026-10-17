# SignalHub

**中文** · [English](README.en.md)

跨平台的桌面悬浮状态指示器。一组始终置顶、可拖拽的透明状态牌，用颜色和明暗节奏表示当前状态；左键点一下可以折叠成一颗单纯的灯。

优先适配 AI 智能体：**Claude Code / Codex / Cursor / Gemini CLI / Goose 可以一键接入**（把 hook 写进它们自己的配置文件），**Pi Agent 只能命令行上报**（在自己脚本里调一次 `signalhub`）。但**不绑定智能体**——任何程序只要能发一个 HTTP 请求就能上报状态。

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

**分组**由上报方决定 —— 上报时带一个可选的 `group`（HTTP 请求体里的 `group` 字段，或命令行的 `--group`）：

```bash
curl -s -X POST ... -d '{"state":"working","session":"a","group":"work"}'
curl -s -X POST ... -d '{"state":"error","session":"b","group":"ci"}'

signalhub working --session a --group work
signalhub error --session b --group ci
```

于是灯球上出现两块牌（最紧急的排在最前），全局聚合取各组里最高的那个。不带 `group` 就归 `default`，行为与单组完全一样。装出来的 hook 命令不带 `--group`，所以接上的智能体现在仍然全在 `default` 组里 —— 多分组是给显式要多组的场景准备的。

**上报方还能声明自己是哪一类信号源**（`--source agent|system|custom`，缺省按名字推）：接入页据此把「收到过上报、但不在智能体清单里」的那些分成**系统**和**自定义**两组，好让「盯系统状态的守护脚本」和「我自己的小工具」不至于混在一起。`signalhub hook` 报的是 `agent`；`signalhub run` 包的命令、以及你手写的 `signalhub working` 落在 `custom`；`--source system` 就是系统那一组（内置采集器将来会用它）。**它只影响接入页怎么分组** —— 灯的聚合、分组、超时、通知都与之无关，填错或不填，灯的表现一模一样。

不想每次上手写 `group` 的话，设置面板 → 接入里有个**按工作目录分组**的开关：打开后，没带 `group` 的上报按各自的 `cwd` 自动分组，一个目录一块牌，每个项目各占一行。上报自己带了 `group` 的仍然听它的，所以两种用法可以混着来。组名就是那条路径，嫌长可以去「外观 → 各组的显示」改名。**默认是关的**，所以升级不会让任何人的灯突然多出几块牌来。

**每组的显示名和位置也能调**（设置面板 → 外观 → 各组的显示）：组名是上报方给的标识（`build-1` 这种），可以改成「构建」；也可以把某个组**置顶**，让它永远排在灯上最前面 —— 没置顶的组仍然按紧急程度排，所以一盏红灯不会被挤到绿灯下面。这两样只影响显示：接口里的组名、隐藏 / 静音的匹配都不变。

三处不靠颜色的信息通道：

- **形状**：默认 `permission` / `error` 的灯是圆角方形，其余是圆形。红绿是色盲最难分的一对，而这两个恰好最需要你注意。
- **细环**：`stale` 的灯上是细环，按剩余宽限期缩短，一眼看出「沉默多久了」。
- **明暗**：默认开启「安静模式」——空闲 / 沉默 / 离线时整块牌几乎透明，只有真正有活动或需要你注意时才显形，鼠标悬停恢复。可在设置里关掉。

**外观可调**（设置面板 → 外观）：四套预设（单灯 / 紧凑 / 标准 / 醒目）一键定调，也可以逐项调——形态（展开成牌 / 折叠成一颗灯）、大小（整块牌的尺度）、形状、配色方案、背景透明度、闪烁节奏，以及牌上显示哪几段（组名 / 状态 / 已持续 / 消息）。

**每个状态都能单独调**（设置面板 → 灯效）：颜色、形状、节奏三样各自覆盖，没写的跟随「外观」页那几项全局默认；另有四套灯效预设（跟随全局 / 形状区分 / 节奏分级 / 只在需要时闪）一键定调。

**需要你注意时弹系统通知**：聚合新变成「需要授权 / 需要关注 / 出错」时弹一条，正文是项目名加智能体自己写的说明。只在跃迁时弹一次，不是每次状态变化都弹——一轮工具调用会触发几十次推送，每次都弹就成了骚扰。状态要**持续**够久（默认 5 秒，可调）才提醒，滤掉一秒内进出 `permission` 那种一闪而过。可以给通知带上提示音（macOS / Windows 上生效），可以**按组静音**（只影响通知和声音，灯照常显示），也可以设一段**勿扰时段**（只按小时，跨午夜也认；这段时间不弹也不出声，而且不补发——早上醒来收到一堆昨晚的通知正是它要避免的）。通知上还有两个按钮：**打开项目目录**和**静音该组**——不用自己回终端里找是哪个项目。

**也能转发到别处**：在「钩子」页配置若干条出站钩子 —— 状态变化时 POST 到某个地址（ntfy、Slack、自建接口）或跑一条本地命令，每条可以只对指定状态生效。异步执行、带超时，目标挂掉不影响本地。格式与安全说明见 [docs/api.md §4.9](docs/api.md)。

**桌面之外也有一盏灯**：「转发」页最下面填一台 WLED 的地址，状态色就直接 POST 到它上面（`POST /json/state`），桌边那盏灯跟着状态变色 —— 不看屏幕也知道现在什么情况。发出去的东西**只有颜色**：没有组名、没有会话名、没有消息内容。填完保存就会立刻按当前状态亮一次，那一下就是「通不通」的回答，结果和出站钩子记在同一份日志里（`ok 200 success` = 通了）。默认一台都不配。

**手机上也能看**：设置 → 接入 → 只读 Web 面板 打开后，有一条带 token 的链接（可复制），在手机 / 平板的浏览器里打开就能看灯 —— 状态、每组在做什么、已持续多久，只读，不用装任何东西。**默认关**，而且它和上报服务共用监听地址：只有同时打开「允许局域网访问」，同一 WiFi 下的设备才看得到（那时设置页会提醒「链接别外传，也别在公共 WiFi 上开着」）。

**左键点一下**在「一叠牌」和「一颗灯」之间切换，选择会记住。**右键点某一块牌**可以隐藏该组、静音该组、清掉该组的状态、切形态或打开设置。悬浮窗会记住自己被你拖到的位置。设置面板顶部显示今日各状态的累计时长，「状态」页还有一条近 30 分钟的时间线（每段的宽度就是它的时长）。

**三个全局快捷键**（设置 →「系统」页列着它们，抢不到的那一个也会在那里标出来）：`⌃⌥L` 显示 / 隐藏灯、`⌃⌥M` 静音 / 取消静音「最差的那一组」、`⌃⌥0` 清除最差那一组的状态。Windows / Linux 上是 `Ctrl+Alt+L / M / 0`。「最差的那一组」= 紧急度最高的那一组（和灯上最上面那块牌同一个判据；置顶只改显示顺序）。静音只影响通知和提示音、灯照常显示那一组，所以按下去会弹一条通告说清楚它做了什么。这几个**刻意不做可配** —— 真正的故障是被别的程序占了，而那件事可配也解决不了，所以抢不到的那个会被明确标出来，而不是变成一个按了没反应的键。

**每个状态都有静默超时**（`working` 90 秒、`thinking` 60 秒、`idle` 600 秒…）。超时后会话不会直接消失，而是转入 `stale`（灰、常亮），再过 5 分钟才真正删除。这样「智能体崩溃了/不说话了」和「智能体空闲待命」是两种不同的显示——不会出现「智能体早就没了，灯还在闪」的情况。

## 安装

从 [Releases](https://github.com/iFishin/SignalHub/releases) 下载：

- **macOS** —— `.dmg`，拖进「应用程序」。Apple Silicon 选 `aarch64`，Intel 选 `x64`。
- **Windows** —— `-setup.exe`（NSIS 安装器）或 `_x64_portable.zip`（解压即用）。
- **Linux（beta）** —— `.AppImage` 或 `.deb`。透明和置顶在 Wayland 下不一定生效，
  碰上就 `GDK_BACKEND=x11` 启动；没有 FUSE 的环境加 `--appimage-extract-and-run`。

安装包**没有签名**，所以第一次打开会被拦一道。macOS 上**推荐**在终端里跑：

```bash
xattr -dr com.apple.quarantine /Applications/SignalHub.app
```

它递归清掉整个包的隔离标记 —— 应用和打包在里面的 `signalhub` 命令都覆盖到。（右键点图标 →「打开」→ 再点一次也可以，只是不保证覆盖到里面那个命令。）Windows 上在 SmartScreen 里点「更多信息」→「仍要运行」。

`signalhub` 这个命令**已经打包在应用里**，不用另外装 —— 「设置 → 接入」页的一键接入会自己用它。想在终端里直接用，把它加到 PATH：

```bash
ln -sfn /Applications/SignalHub.app/Contents/MacOS/signalhub /usr/local/bin/signalhub
```

**更新**：macOS 版从 0.2.4 起支持**应用内更新**（设置 →「系统」页会显示有没有新版本，「下载并安装」点了才装，默认不自动装）。Linux 和 Windows 这一版还只能手动下载 —— Windows 要用的解包库被上游全量撤下了，Linux 侧还没在真机上验收过（原因写在 [.github/release-body.md](.github/release-body.md)）。

打一个 `v` 开头的 tag 就会自动出四个平台的包，见 [.github/workflows/release.yml](.github/workflows/release.yml)。

## 从源码运行

需要 Node.js 与 Rust 工具链。

Linux 上还要先装一批**系统库**（Tauri 官方那份清单，和 CI 里 release.yml 装的是同一份）。
具体清单见 `.github/workflows/release.yml` 的「装 Linux 系统依赖」那一步；少一个的表现
是构建报一个和缺的库看不出关系的错。

```bash
npm install
npm run tauri dev
```

**跑 `cargo test` 之前要先 `npm run stage-cli`。** CLI 是作为 sidecar 打进应用包的（见 [tauri.conf.json](src-tauri/tauri.conf.json) 的 `externalBin`），而 `tauri-build` 在**构建脚本阶段**就校验那个文件在不在 —— 少了它连 `cargo test` 都编不过。`tauri dev` / `tauri build` 会自己先跑这一步，只有直接调 `cargo` 时才需要手动补一次（`cargo clean` 之后也要重跑）。

首次启动会在 `~/.signalhub/`（Windows 上是 `%USERPROFILE%\.signalhub`）下生成 `port` 和 `token` 两个文件。窗口无边框、不占任务栏也不进 Dock，**退出请用菜单栏的托盘图标**。

## 上报状态

HTTP 接口（默认 `127.0.0.1:41320`，端口以 `~/.signalhub/port` 为准）。**默认只监听本机** —— 别的机器连不进来，这是它默认的安全边界：

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

**别的机器上报**：设置面板 →「接入」里有个「允许局域网访问」的开关（默认关）。打开后服务改绑 `0.0.0.0`，同网段的机器 / 容器 / WSL 就能连进来 —— 代价是**网内任何人拿到 token 就能上报**，所以别在公共 WiFi 上开。改完要重启应用才生效。

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

已完成：悬浮灯球、状态聚合与 TTL、HTTP 上报接口、CLI（含 `watch`）、设置面板、hook 自动接线（Claude Code / Codex / Cursor / Gemini CLI / Goose）、接入自检（「接入」页会检查配置里那条命令现在还跑不跑得起来 —— 那种失效是静默的）、hook 调用日志（「系统」页直接看得到尾巴）、Pi Agent（命令行接入）、`signalhub run` 通用适配（把任意命令包起来上报）、牌上的「正在做什么」（从工具事件自动生成，省得每个 agent 各配一句）、CLI 文案跟随界面语言（`--help` / 报错 / 输出都有中英两份）、系统通知与提示音、通知动作按钮、窗口位置记忆、展开 / 折叠两种形态、每组右键菜单、今日时长统计与近 30 分钟时间线、历史页（近 7 天每天各状态占用了多久，各天共用同一个尺度）、出站钩子转发、每个状态的颜色 / 形状 / 节奏可单独调、按组静音、勿扰时段、持续多久才提醒、卡住了一直提醒（5 / 15 / 30 分钟各再提一次，正文带上已经等了多久）、托盘图标跟随状态、单实例、开机自启、全局快捷键（显示 / 隐藏灯、静音最差那一组、清除最差那一组，抢不到会在设置里标出来）、多分组（上报带 `group` 或 `--group`，也可以按工作目录、或按会话自动兜底 —— 一个会话一块牌）、今日时长的每日落盘（保留 30 天）、配置导出 / 导入、局域网访问开关（默认关）、点击穿透、接入页的信号源分类（AI 智能体 / 系统 / 自定义，上报时用 `--source` 声明）、只读 Web 面板（手机上打开一条链接就能看灯，默认关）、物理灯（WLED —— 状态色直接发到灯上，只发颜色，默认一台都不配）。

待做：签名与公证（步骤位置留在 [.github/workflows/release.yml](.github/workflows/release.yml) 里，拿到证书就能填）。

**自动更新还没接** —— 不是没时间，是有一条查不到确切答案、必须真机验的东西：未签名的 macOS 应用替换 bundle 之后，Gatekeeper 会不会拦。接线步骤（含密钥怎么生成、公钥为什么填内容而不是路径）已经写在 [.github/workflows/release.yml](.github/workflows/release.yml) 顶部，验证通过了就能打开。

## 许可证

MIT，见 [LICENSE](LICENSE)。
