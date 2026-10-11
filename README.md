# SignalHub

跨平台的桌面悬浮状态指示器。一个始终置顶、可拖拽的透明圆点，用颜色和明暗节奏表示当前状态。

优先适配 AI 智能体（Claude Code / Codex / Cursor），但**不绑定智能体**——任何程序只要能发一个 HTTP 请求就能上报状态。

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

优先级 **红 > 黄 > 绿**：一个会话在等授权时，别的会话报「正在执行」不会把红灯顶掉；等授权的那一路自己恢复工作后，红又会自然消失。多会话取最高优先级，灯球上最多同时画 3 个点（一个会话一个）。

三处不靠颜色的信息通道：

- **形状**：`permission` / `error` 是圆角方形，其余是圆形。红绿是色盲最难分的一对，而这两个恰好最需要你注意。
- **细环**：`stale` 点上的细环按剩余宽限期缩短，一眼看出「沉默多久了」。
- **明暗**：默认开启「安静模式」——空闲 / 沉默 / 离线时圆点几乎透明，只有真正有活动或需要你注意时才显形，鼠标悬停恢复。可在设置里关掉。

**需要你注意时弹系统通知**：聚合新变成「需要授权 / 需要关注 / 出错」时弹一条，正文是项目名加智能体自己写的说明。只在跃迁时弹一次，不是每次状态变化都弹——一轮工具调用会触发几十次推送，每次都弹就成了骚扰。

**也能转发到别处**：设置里填一个 URL（ntfy、Slack、自建接口都行），同一个触发时机把状态 POST 过去，用于手机推送或接进团队频道。异步发送、带超时，目标挂掉不影响本地。载荷格式见 [docs/api.md §4.9](docs/api.md)。

**点击圆点**可以就地展开一张会话列表（再点一次收起），一眼看清每个项目各自在什么状态。悬浮窗也会记住自己被你拖到的位置。设置面板顶部还会显示今日各状态的累计时长。

**每个状态都有静默超时**（`working` 90 秒、`thinking` 60 秒、`idle` 600 秒…）。超时后会话不会直接消失，而是转入 `stale`（灰、常亮），再过 5 分钟才真正删除。这样「智能体崩溃了/不说话了」和「智能体空闲待命」是两种不同的显示——不会出现「智能体早就没了，灯还在闪」的情况。

## 运行

需要 Node.js 与 Rust 工具链。

```bash
npm install
npm run tauri dev
```

首次启动会在 `~/.signalhub/` 下生成 `port` 和 `token` 两个文件。窗口无边框、不占任务栏也不进 Dock，**退出请用菜单栏的托盘图标**。

## 上报状态

HTTP 接口（默认 `127.0.0.1:41320`，端口以 `~/.signalhub/port` 为准）：

```bash
curl -s -X POST "http://127.0.0.1:$(cat ~/.signalhub/port)/v1/state" \
  -H "Authorization: Bearer $(cat ~/.signalhub/token)" \
  -H 'Content-Type: application/json' \
  -d '{"state":"permission","session":"demo","message":"需要批准"}'
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
signalhub install hooks claude-code      # 也支持 codex / cursor
signalhub install hooks claude-code --dry-run   # 先看看会改什么
```

它会往智能体的配置文件里写 hook（Claude Code 是 `~/.claude/settings.json`）。**只动 `hooks` 这一个键**，其余内容原样保留，写之前自动备份到 `*.signalhub-backup`；重复执行不会产生重复条目，命令变了会原地更新；配置不是合法 JSON 时会直接中止、绝不覆盖。卸载用 `signalhub uninstall hooks claude-code`。

装完重启智能体即可生效。之后提问 → 绿快闪、调工具 → 绿慢闪、需要授权 → 红闪、答完亮一下再回常亮。

建议先把 CLI 放进 PATH，这样写进配置的是可移植的裸名字而不是绝对路径：

```bash
ln -sfn "$(pwd)/src-tauri/target/debug/signalhub" ~/.local/bin/signalhub
signalhub install hooks claude-code
```

绝对路径也能用，但二进制一挪动 hook 就会静默失效（智能体会话里什么都看不到），排查起来很费劲。

完整接口规范见 [docs/api.md](docs/api.md)。架构与设计取舍见 [docs/architecture.md](docs/architecture.md)。

## 进度

已完成：悬浮灯球、状态聚合与 TTL、HTTP 上报接口、CLI（含 `watch`）、设置面板、hook 自动接线（Claude Code / Codex / Cursor）、系统通知、窗口位置记忆、点击展开会话列表、单实例、开机自启、全局快捷键、点击穿透。

待做：出站 webhook / ntfy 转发、今日状态时长统计、把 CLI 作为 sidecar 打进 app bundle（分发才需要）。
