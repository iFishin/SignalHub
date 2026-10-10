# SignalHub

跨平台的桌面悬浮状态指示器。一个始终置顶、可拖拽的透明圆点，用颜色和明暗节奏表示当前状态。

优先适配 AI 智能体（Claude Code / Codex / Cursor），但**不绑定智能体**——任何程序只要能发一个 HTTP 请求就能上报状态。

## 状态

| 状态 | 视觉 | 含义 |
|---|---|---|
| `idle` | 绿、常亮 | 空闲 |
| `thinking` | 绿、快闪 | 正在推理 |
| `working` | 绿、慢闪 | 正在执行 |
| `done` | 绿、常亮 + 闪一下 | 完成（30s 后回 `idle`） |
| `attention` | 黄、闪 | 需要关注 |
| `permission` | 红、平滑闪 | 需要授权 |
| `error` | 红、频闪 | 出错 / 阻塞 |
| `off` | 灰、常灭 | 监控暂停 |

优先级 **红 > 黄 > 绿**：一个会话在等授权时，别的会话报「正在执行」不会把红灯顶掉；等授权的那一路自己恢复工作后，红又会自然消失。多会话取最高优先级。

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
```

完整接口规范见 [docs/api.md](docs/api.md)。架构与设计取舍见 [docs/architecture.md](docs/architecture.md)。

## 接入智能体

```bash
signalhub install hooks claude-code      # 也支持 codex / cursor
signalhub install hooks claude-code --dry-run   # 先看看会改什么
```

它会往智能体的配置文件里写 hook（Claude Code 是 `~/.claude/settings.json`）。**只动 `hooks` 这一个键**，其余内容原样保留，写之前自动备份到 `*.signalhub-backup`，重复执行也不会产生重复条目；配置不是合法 JSON 时会直接中止、绝不覆盖。卸载用 `signalhub uninstall hooks claude-code`。

装完重启智能体即可生效。之后提问 → 绿快闪、调工具 → 绿慢闪、需要授权 → 红闪、答完亮一下再回常亮。

完整接口规范见 [docs/api.md](docs/api.md)。架构与设计取舍见 [docs/architecture.md](docs/architecture.md)。

## 进度

已完成：悬浮灯球、状态聚合与 TTL、HTTP 上报接口、CLI、设置面板、hook 自动接线（Claude Code / Codex / Cursor）。

待做：开机自启、全局快捷键、窗口位置记忆、点击穿透模式。
