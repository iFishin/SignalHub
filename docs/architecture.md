# SignalHub 架构设计

跨平台桌面悬浮状态指示器。优先适配 AI 智能体，但不绑定智能体——任何程序都能通过统一的本地 API 上报状态。

- 框架：Tauri 2（Rust 后端 + React/TypeScript 前端）
- 平台：macOS / Windows / Linux (X11)
- 参考项目：[guan-ops/Agent-Signal-Bar](https://github.com/guan-ops/Agent-Signal-Bar)（原生 macOS/Swift，本项目是其跨平台重实现）

---

## 1. 核心判断

三条贯穿全文的设计决策，先说清楚：

1. **上报通道 = 本地 HTTP + 瘦客户端 CLI。** 不采用 Agent-Signal-Bar 的共享 JSON 文件方案。HTTP 让任何语言一行 `curl` 就能接入，可即时调试、有应答、且天然只允许单一写者，从而彻底消除多写者竞态。
2. **优先级保护实现在「聚合层」，不在「会话层」。** 会话内后写覆盖 + 跨会话取 max + TTL 兜底。见 §3.2——这是最容易设计错的地方。
3. **CLI 内置 `signalhub hook <agent>` 子命令**，直接消费智能体 hook 的 stdin JSON。这一个设计消掉了全部 shell 脚本和 `jq` 依赖，安装 hook 退化成「同一行命令注册到每个事件」。

---

## 2. 整体架构

```
┌──────────────────────────── 外部上报方 ────────────────────────────┐
│  Claude Code hooks    Codex hooks    Cursor hooks    CI / 脚本      │
└───────────────────────────┬────────────────────────────────────────┘
                            │ ① CLI（或直接 curl）
                            ▼
        ┌────────────────────────────────────────────────┐
        │  signalhub CLI (Rust, 无 Tauri 依赖)            │
        │   普通模式: signalhub working --session build-1 │
        │   hook 模式: signalhub hook claude-code ← stdin │
        │   → POST http://127.0.0.1:<port>/v1/state       │
        └────────────────────┬───────────────────────────┘
                             ▼  ② HTTP (127.0.0.1, Bearer token)
┌──────────────────────────── Tauri 进程 ────────────────────────────┐
│  axum Router (/v1/*)                                                │
│         │ ③ report() / clear() / set_paused()                       │
│         ▼                                                           │
│  SignalStore  Arc<Mutex<Store>>        ← 唯一真值来源               │
│    ├─ sessions: HashMap<SessionId, Session>                         │
│    ├─ paused: bool                                                  │
│    └─ aggregate() = max_priority(sessions)                          │
│         │                                                           │
│   ④ TTL sweep (250ms tick)        ⑤ emit_to("main", "signalhub://state")
│         │                                     │                     │
│  ┌──────┴──────┐                    ┌─────────┴──────────┐          │
│  │ Tray 图标    │                    │ 悬浮窗 Webview      │          │
│  │ 右键菜单     │                    │ (React + CSS 动画)   │          │
│  └─────────────┘                    └────────────────────┘          │
│  全局快捷键 → 穿透模式切换   设置窗 (独立 webview)                    │
└────────────────────────────────────────────────────────────────────┘
```

**单一写者原则。** 所有状态变更都必须经过 `Store` 的一个互斥锁。前端和托盘都是纯消费者，从不写状态。这是选择 HTTP 而非文件方案的根本原因——文件方案允许多个进程并发写入，必须自己实现原子写和锁文件，而 HTTP 服务端天然把写操作串行化了。

---

## 3. 状态模型与聚合

### 3.1 Signal 枚举

| 状态 | 优先级 | 静默超时 | 视觉（颜色 / 节奏） | 含义 |
|---|---|---|---|---|
| `error` | 50 | 300s | 红 `#d0342c`、频闪 340ms（`steps` 硬切） | 出错/阻塞 |
| `permission` | 40 | 600s | 红、平滑闪 900ms | 需要授权/确认 |
| `attention` | 30 | 300s | 黄 `#d99a06`、闪 1000ms | 需要关注 |
| `working` | 20 | 90s | 绿 `#16a34a`、慢闪 1600ms | 正在执行工具 |
| `thinking` | 15 | 60s | 绿、快闪 640ms | 正在推理 |
| `done` | 6 | 30s | 绿、常亮 + 一次入场脉冲 380ms | 任务完成（到期直接消失） |
| `idle` | 2 | 600s | 绿、常亮 | 空闲，无任务 |
| `stale` | 1 | 300s | 灰 `#9ca3af`、常亮 | 会话已沉默（派生） |
| `offline` | 0 | — | 灰、常亮、opacity .45 | 没有任何会话（派生） |
| `off` | 0 | — | 灰、常亮、opacity .45 | 监控暂停（派生，全局开关） |

「静默超时」是会话转入 `stale` 的时限；`stale` 那一行则是它被真正删除前的宽限期。`offline` / `off` 不是会话状态，所以没有意义。

**视觉取向是朴素简约**：纯色圆点，没有发光、没有渐变、没有阴影。状态靠**颜色 + 明暗节奏**表达，不靠装饰。颜色值比常规的 500 号色深一档——扁平色块没有描边也没有阴影，太亮的颜色在浅色桌面上会糊掉。

三个关键区分：

- **`off` 不是会话状态，是全局开关。** `paused = true` 时无论多少会话在报绿都显示熄灭灰。CLI 的 `signalhub off` / `on`（或 `monitor off` / `on`）操作的是这个全局位，不是某个会话。
- **`done` 和 `idle` 颜色相同**，靠一次性的入场脉冲 + 30s TTL 收敛来区分。这是刻意的：`done` 是短暂的确认反馈，`idle` 是稳态。
- **`stale` / `offline` / `off` 是派生状态，不能上报。** 它们是聚合出来的结果，不是外部程序该声称的状态；`Signal::parse` 会明确拒绝它们。三者的视觉差异与取舍见 §3.5。

圆点直径默认 **22px**，可在设置里选 16 / 22 / 32。窗口尺寸由 Rust 侧按可见圆点个数和设置算出（见 §3.7），设置变更和会话数变化时都会同步调整。

`Signal::priority()` 返回上表的数值，聚合时取 max。

### 3.2 聚合算法（核心决策）

规则是「红 > 黄 > 绿，高优先级状态不被普通工作事件覆盖」。有两种实现路径：

**方案 A：会话级单调守卫（不采用）**

```rust
// 新事件优先级低于当前状态时丢弃
if new.priority() < current.priority() { return; }
```

看起来正好实现了规则，但有致命场景：

```
会话 A: PreToolUse        → working(20)
会话 A: PermissionRequest → permission(40)   🔴
用户点了「允许」
会话 A: PostToolUse       → working(20)      ← 20 < 40，被守卫丢弃
                            结果：灯一直是红的，直到 600s TTL 过期
```

要修就得引入 `resolve` 标志、事件白名单……复杂度上升且状态机会卡死。

**方案 B：会话内后写覆盖 + 聚合取 max（采用）**

```rust
impl Store {
    /// 会话内部：最新事件直接覆盖（last-write-wins）
    pub fn report_at(&mut self, now: Instant, session: &str, /* ... */) -> bool {
        let before = self.aggregate();
        self.sessions.insert(session.into(), Session { /* ... */ });
        before != self.aggregate()      // 聚合变了才需要推前端
    }

    /// 聚合：跨会话取最高优先级 —— 「红 > 黄 > 绿」就实现在这里
    pub fn aggregate(&self) -> Signal {
        if self.paused { return Signal::Off; }
        self.sessions.values()
            .filter(|s| s.state != Signal::Off)
            .max_by_key(|s| s.state.priority())
            .map(|s| s.state)
            .unwrap_or(Signal::Idle)
    }
}
```

为什么这就够了：

- 规则真正要防的场景是「A 会话在等授权（红），B 会话还在跑工具（绿），绿灯事件不该顶掉红灯」。取 max 天然解决——B 报 `working`(20)，A 仍是 `permission`(40)，聚合保持红。
- 用户批准后 A 自己发 `PostToolUse → working`，A 的红消失，聚合自然回落。**没有停滞状态。**
- 单会话（最常见的场景：只有一个 Claude Code）根本不存在跨会话覆盖问题。
- `done` 靠 TTL 自动回落 `idle`，不需要人为清场。

如果将来发现某个上报方事件过密导致红灯被同行会话的正常进度掩盖，再加一个 `hold: true` 字段（只允许显式清除）即可。v1 不需要。

### 3.3 四个兜底机制（缺一不可）

| 机制 | 作用 | 缺失后果 |
|---|---|---|
| 每状态默认 TTL | 会话静默超时后转 `stale` | 智能体崩溃在 `working`，灯会一直绿闪到 TTL 到期 |
| 全局 sweep 任务（250ms） | 定期检查过期并重算聚合 | 过期了但无人触发重算，前端收不到更新 |
| `SessionEnd` hook → 清除会话 | 正常结束立即清场 | 跑完的会话残留，多会话界面混乱 |
| `stale` 宽限期（5 分钟） | 沉默的会话先标记再删除 | 「智能体不说话了」和「智能体空闲」分不开 |
| 一切都有 TTL | 没有任何状态能永远粘住 | 早先 `idle` 没有 TTL，一个忘关的会话能显示半小时「空闲」 |

**每个状态都有 TTL 是刻意的**（`default_ttl()` 是个全函数，不再返回 `Option`）。早先 `idle` 没有 TTL，靠「30 分钟无事件回收」兜底——那段窗口太长，正是「智能体走了灯还亮着」的成因之一。现在 `idle` 也有 600 秒窗口。

工作类状态的窗口刻意很短（`working` 90 秒、`thinking` 60 秒）：干活时每次工具调用都会触发 hook，静默这么久基本可以断定不在工作。**这个数值直接决定「智能体消失后灯还会错多久」**，是调优时最该动的一个旋钮。

### 3.4 `stale`：把「不说话了」和「空闲」分开

会话静默超过 TTL 后**不再直接消失，而是转入 `stale`**，再保留 `STALE_GRACE`（5 分钟）才真正删除。`done` 是唯一的例外——它的语义就是「完成，回到常态」，到期直接消失。

这一层解决的是个真实问题：智能体崩溃、被强杀、或 `SessionEnd` 没触发时，会话会停在最后一个状态上。没有 `stale` 的话，用户看到的和「智能体正忙」完全一样，只能等 TTL 到期。

`stale` 的优先级**低于 `idle`**：一路会话在报 idle（活着）时，不该被另一路已经沉默的会话盖成灰的。

### 3.5 三档「不在工作」

灰色这一族有三个状态，语义必须分清：

| 状态 | 含义 | 视觉 |
|---|---|---|
| `stale` | 有会话，但它不说话了 | 灰、常亮、不透明 |
| `offline` | 一个会话都没有 | 灰、常亮、更淡 |
| `off` | 用户主动暂停监控 | 灰、常亮、更淡 |

`offline` 与 `off` 视觉一致是**刻意的**：它们都表示「没有在追踪任何东西」，差别只在语义，由 tooltip 和设置面板说明。22px 的圆点里塞不下第三种可区分的灰——与其造一个谁也看不出区别的颜色，不如承认这一点。

`stale` 与它们的不透明度差异是能看出来的，所以保留了。三者都不闪：它们都不是「有活动」。

### 3.6 非颜色编码

颜色不是唯一的信息通道，有三处刻意的例外：

**形状（色盲友好）。** `permission` / `error` 画成圆角方形，其余状态是圆形。红绿是色盲最难分的一对，而这两个恰好是最需要你注意的状态——一种形状区分就解决了关键场景，比换掉整套配色克制。约 8% 的男性分不清红绿，这不是审美问题。

**细环（沉默时长）。** `stale` 圆点上的细环按剩余宽限期缩短，从满圈到消失。数值由服务端算好（`stale_progress`）而不是前端自己算——`age_ms` 是「距上次状态变化」，并不等于「沉默多久」，前端要算就得复刻一遍 TTL 常量。

**明暗（安静模式）。** 空闲 / 沉默 / 离线时整体压到 0.15 不透明度，只有真正有活动或需要你注意时才完全显形；鼠标悬停恢复（否则压暗之后找不到它来拖）。`done` 也算「有事」——它是任务完成的确认，压暗就失去意义了。可在设置里关掉。

### 3.7 多会话：几个圆点

灯球最多同时画 `MAX_DOTS`（3）个圆点，一个会话一个，按优先级排序——所以截断后剩下的恰好是最紧急的那几个。窗口宽度按可见圆点数算：

```
width = dots * size + (dots - 1) * (size / 3) + LAMP_MARGIN
```

**只有一个会话时窗口仍是 `size + LAMP_MARGIN` 的方形**（默认 22+20=42）。多点只在真有多个会话时才占宽——否则一个常驻的宽透明窗口会白白吃掉桌面的点击，而那正是这类工具最不该有的副作用。会话数变化时窗口会跟着变宽变窄，这是刻意的：多一个 agent 就多一份显示空间，语义上是合理的。

几何在 Rust（`Settings::window_size`）和 CSS（`gap: calc(var(--size) / 3)`）各写了一份。浏览器算出的是小数、Rust 是整数除法，最多差 0.67px；一侧留白有 10px，这点误差看不出来，所以不值得把几何再往上传一遍。

---

## 4. 悬浮窗实现

### 4.1 窗口配置

Tauri 2 的配置键是 lowerCamelCase，注意 `skipTaskbar` 的 `b` 是小写（历史遗留）。

```json
{
  "label": "main",
  "url": "index.html?window=light",
  "width": 42, "height": 42,
  "resizable": false,
  "decorations": false,
  "transparent": true,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "shadow": false,
  "focus": false,
  "visible": false,
  "maximizable": false, "minimizable": false, "closable": false
}
```

- `width`/`height` 只是初值：真实尺寸由设置决定（圆点直径 + 20px 边距），启动时和每次改设置都会由 Rust 侧重设一遍。
- **`transparent: true` 在 Tauri 2.12.1+ 不再需要 `macos-private-api` feature**（旧 2.x 需要）。用 `cargo tree -p tauri` 确认版本。
- **`visible: false` + setup 里先套用设置再 `show()`**：否则会先按配置里的默认尺寸闪一下再跳到设置里的尺寸。
- **`closable: false` + `decorations: false` + `skipTaskbar` + Accessory 激活策略 = 用户没有任何办法退出应用。** 必须提供一个退出入口，见 §4.4。

**第二个窗口：设置面板。** 同一个 Vite 应用、同一份 bundle，靠查询参数分流，比配 Vite MPA 多入口简单得多：

```json
{
  "label": "settings",
  "url": "index.html?window=settings",
  "width": 420, "height": 760,
  "resizable": false,
  "center": true,
  "visible": false,
  "maximizable": false
}
```

三个非显然的点：

1. **高度是量出来的，不是猜的。** 用 3 个会话的样本数据实测内容需要 752px（加「安静模式」那一行和说明之前是 631px）。改布局后应该在浏览器里读一次 `scrollHeight - clientHeight` 再定值——把面板宽度固定成 420px 再量，才能避开移动端视口模拟的干扰。
2. **关闭时只隐藏、不销毁**（`WindowEvent::CloseRequested` + `api.prevent_close()`），这样重开是瞬时的。
3. **Accessory 策略下窗口不会主动到前台**，所以打开设置时临时切回 `ActivationPolicy::Regular`，关闭时切回 `Accessory`。副作用是设置打开期间应用会出现在 Dock 里——这反而是对的，用户正在和它交互。
4. **两个窗口都不是透明的**，但 `lamp.css` 里有一份全局的 `html, body { background: transparent }`（灯球窗口需要）。设置面板靠 `html[data-window="settings"]` 属性选择器压过它，这样覆盖关系是确定性的、不依赖两个 CSS 文件的打包顺序。`main.tsx` 负责把 `data-window` 写到 `<html>` 上。

### 4.2 setup：置顶与 Dock 隐藏

```rust
#[cfg(target_os = "macos")]
app.set_activation_policy(tauri::ActivationPolicy::Accessory);  // 不在 Dock / Cmd-Tab 出现

let win = app.get_webview_window("main").unwrap();   // 需要 use tauri::Manager;
win.set_visible_on_all_workspaces(true)?;            // 所有桌面空间可见
win.set_always_on_top(true)?;
win.show()?;
```

`skipTaskbar` 在 macOS 无效，隐藏 Dock 图标必须用 `ActivationPolicy::Accessory`。

### 4.3 拖拽

`data-tauri-drag-region` 在 v2 仍然有效，并新增了 `"deep"` 值——让任意后代元素的点击也能拖动。因为灯球会盖满整个窗口，必须用 `"deep"`：

```html
<div class="root" data-tauri-drag-region="deep">
  <div class="lamp" />
</div>
```

Tauri 只响应鼠标**主键**的按下事件，所以右键可以正常穿透到 `contextmenu`。`A / BUTTON / INPUT / SELECT / TEXTAREA / LABEL / SUMMARY` 会阻止拖拽——自定义右键菜单时不要用这些标签。备选方案是 JS 调 `appWindow.startDragging()`（需要 `core:window:allow-start-dragging` 权限）。

### 4.4 点击穿透：必须警惕的坑

`set_ignore_cursor_events(true)` 让**整个窗口**忽略指针事件——包括拖拽和右键。一旦开启穿透，鼠标就无法再操作它。

**铁律：永远不要在全局快捷键注册成功之前开启穿透。** 逃生通道顺序必须是：注册快捷键 → 提供托盘菜单项 → 才允许切到穿透模式。UI 上切换时也应提示用户按什么键退出。

因为核心诉求是「可拖拽」，v1 **默认不开穿透**，仅作为托盘菜单/快捷键的可选模式。

**退出入口（v1 必需）**：因为窗口无边框、不可关闭且不在任务栏显示，灯球右键菜单必须提供「退出」，走一个自定义 Tauri command（自定义命令不需要额外 capability）：

```rust
#[tauri::command]
fn quit(app: tauri::AppHandle) { app.exit(0); }
```

### 4.5 动画方案选型

| 方案 | 评价 |
|---|---|
| **CSS keyframes（采用）** | GPU 合成，只动 `opacity`/`transform`，零 JS 开销；换状态只改一个属性；天然支持 `prefers-reduced-motion` |
| Canvas / WebGL | 浮窗只有几十像素，过度设计 |
| 帧序列图 / Lottie | 视觉最好，但 v1 成本过高 |

实现方式是把状态映射成根元素的 `data-state`，配色集中在 `styles/state.css` 一份 `[data-state] { --state: … }` 里（灯球和设置面板的会话小圆点共用），动画则按状态挂在 `.lamp` 上。切换状态 = 改一个 DOM 属性。

```
[data-state="working"]  .lamp { animation: pulse 1600ms ease-in-out infinite; }
[data-state="error"]    .lamp { animation: pulse 340ms steps(2, end) infinite; }
```

`error` 用 `steps()` 硬切做出频闪感，与 `permission` 的平滑闪烁区分开——两者颜色相同，节奏是唯一的区分手段。

`error` 用 `steps()` 硬切做出频闪感，与 `permission` 的平滑闪烁区分开。

无障碍：`@media (prefers-reduced-motion: reduce)` 下禁用所有动画，降级为常亮色块。

---

## 5. 模块布局

```
SignalHub/
├─ index.html
├─ vite.config.ts
├─ src/                          ── 前端 (React + TS)
│  ├─ main.tsx                   写 <html data-window>，按顺序引入三个样式表
│  ├─ App.tsx                    ?window=light | settings 分流
│  ├─ components/
│  │  ├─ SignalLamp.tsx          灯球
│  │  └─ SettingsPanel.tsx       设置面板
│  ├─ lib/
│  │  ├─ types.ts                与 Rust 侧 Signal 枚举保持一致的 TS 类型
│  │  ├─ signals.ts              状态中文名（灯球与设置面板共用）
│  │  ├─ env.ts                  inTauri 判定、windowKind
│  │  ├─ useSignal.ts            订阅 signalhub://state + 拉取初始值
│  │  └─ useSettings.ts          订阅 signalhub://settings + 读写设置
│  └─ styles/
│     ├─ state.css               状态配色 tokens（灯球与会话小圆点共用）
│     ├─ lamp.css                灯球布局与动画
│     └─ settings.css            设置面板（含浅色/深色两套变量）
└─ src-tauri/                    ── 后端 (Rust)
   ├─ tauri.conf.json
   ├─ capabilities/default.json
   └─ src/
      ├─ main.rs                 入口（二进制名 signalhub-app）
      ├─ lib.rs                  run()：插件、setup、命令注册
      ├─ state.rs                Signal / Session / Store ← 核心，含单测
      ├─ sweep.rs                TTL 清扫 + 聚合重算
      ├─ notify.rs               通知出口（Notifier）— 让 HTTP 层能脱离 AppHandle 单测
      ├─ config.rs               ~/.signalhub 路径、token、端口文件、用户设置
      ├─ hooks.rs                智能体 hook 的安装/卸载（app 与 CLI 共用）
      ├─ server.rs               axum 路由 + 鉴权 + 端口发现，含 12 个 HTTP 层单测
      ├─ tray.rs                 托盘菜单
      └─ bin/signalhub.rs        CLI ← 用户实际调用的 `signalhub` 命令
```

**关于 `hooks.rs` 为什么不是 `integration/{claude_code,codex,cursor}.rs`：** 原计划按智能体拆三个文件，实际写下来发现三者的差异只在「配置路径 + 事件名清单 + handler 形状」这张表里，而**安装逻辑（保留无关键、幂等、备份、解析失败中止、精确摘除自己的 handler）三家完全一样且是最需要谨慎的部分**。拆成三个文件会把这段逻辑抄三遍，或者抽出一层反而更绕。所以是一个 `hooks.rs`，内部用一个小 `Agent` 枚举和一张事件表区分差异。

**关于两个二进制。** 这个包里有两个 bin：app（`signalhub-app`，源自 `src/main.rs`）和 CLI（`signalhub`，源自 `src/bin/signalhub.rs`）。两者同名会冲突，而 app 的名字用户看不到（通过 .app 启动），CLI 的名字却要写进智能体的 hook 配置里，所以让给 CLI。`[package] default-run = "signalhub-app"` 让 `cargo run`（也就是 `tauri dev`）选 app 那个，`tauri.conf.json` 的 `mainBinaryName` 同步设为 `signalhub-app`。

**关于 `notify.rs`：** 状态变化的通知出口被抽成一个 `Notifier = Arc<dyn Fn(&Snapshot)>`，而不是让各处直接持有 `AppHandle`。原因是 `AppHandle` 在单测里造不出来——有了这层间接，`server.rs` 可以用一个只记数的闭包当通知器，起一个真实监听端口跑完整的 HTTP 层测试，覆盖鉴权、路由、状态码、聚合结果。这个抽象不是为未来预留的，它当场就换来了 12 个能跑的集成测试。

**关于 CLI 的 crate 组织**：CLI 用 `#[path = "../config.rs"] mod config;` 直接包含源码，而不是 `use signalhub_lib::config`——后者会把整个 Tauri 拖进这个本该很小的二进制（实测 3.1MB 且不链接 Tauri）。同样的原因，它不校验状态名，只把状态字符串原样转发给服务端，避免出现两张别名表。将来若构建时间或体积成为问题，再抽成 cargo workspace 里的 `crates/signalhub-cli`。

**state.rs 是关键**：它是纯逻辑、无 I/O、时间可注入（`apply(now, …)`），因此可以完整单测而不需要跑起 Tauri 或等待真实时间流逝。

---

## 6. 路径约定

```
~/.signalhub/
├─ port          实际监听端口（app 启动时写入）
├─ token         32 字节随机 token，权限 0600
├─ config.json   用户设置（圆点大小、是否置顶）
├─ hooks.log     hook 调用日志（每次调用一行，UTC 时间戳，512KB 轮转）
└─ pending.jsonl 可选：app 未运行时的暂存事件，启动时回放（v1.1）
```

主目录只从环境变量解析（`HOME`，Windows 上是 `USERPROFILE`），**不引入 `dirs` 之类的依赖**——CLI 是独立二进制，少一棵依赖树是实打实的收益。代价是两者都缺失时会明确报错而不是静默写到别处，这是可接受的。app 由终端或 launchd 启动时两者必有其一。

---

## 7. 平台注意事项

| 事项 | 说明 |
|---|---|
| macOS 全屏 Space 无法覆盖 | `alwaysOnTop` 不能浮在别的应用的全屏 Space 之上，需要更高的 NSWindow level 和私有 API。所有同类工具的共同限制，**不要在 v1 承诺**。 |
| Windows 透明窗口白闪 | 建议加 `no_redirection_bitmap(true)` |
| Linux/Wayland | 透明窗口与 `shadow` 官方标注不支持。Linux 是硬需求时优先 X11。 |
| `set_shadow` | Linux 不支持 |
| `set_skip_taskbar` | macOS 不支持（用 Accessory 策略替代） |
| 首次运行防火墙弹窗 | 绑定 127.0.0.1 通常不弹，但仍可能提示。 |

---

## 8. MVP 阶段与状态

| 阶段 | 内容 | 状态 |
|---|---|---|
| 0 | 脚手架（Tauri 2 + React + TS） | ✅ 已完成 |
| 1 | 悬浮灯球：透明/无边框/置顶/拖拽 + CSS 动画 | ✅ 已完成 |
| 2 | 状态机 + TTL sweep + 前后端联动 + 单测 | ✅ 已完成 |
| 3 | HTTP 服务：axum 路由、token、端口发现 | ✅ 已完成 |
| 4 | CLI（`hook` 子命令 + 状态上报 + status/clear/monitor） | ✅ 已完成 |
| 5 | hook 自动接线：`install hooks` / `uninstall hooks`（三家智能体） | ✅ 已完成 |
| 6 | 设置面板（圆点大小、置顶、暂停、会话列表） | ✅ 已完成 |
| 6b | 状态模型补全：`stale` / `offline`、每状态 TTL、hook 调用日志 | ✅ 已完成 |
| 6c | 灯球渲染：多会话多点、色盲形状、沉默细环、安静模式 | ✅ 已完成 |
| 6d | 需要时弹系统通知、窗口位置记忆、点击展开会话列表 | ⏳ 待做 |
| 6e | 打磨：开机自启、单实例、全局快捷键、点击穿透 | ⏳ 待做 |
| 7 | 后续：出站 webhook、`watch` 跟随、今日统计 | ⏳ 待做 |

三处偏离原计划的地方，都记在这里以免以后当成疏漏：

- **托盘从 Phase 6 提前到了 Phase 1。** 窗口无边框、不可关闭又不在任务栏显示，没有托盘就没法退出——它不是一个打磨项，是可用性的前提。
- **设置面板从 Phase 6 提前。** 一个不可配置的悬浮窗没法用：圆点多大、要不要置顶、哪些会话在跑，这些都得能看见能改。
- **灯球在第一版之后重做过一次**：最初是径向渐变 + 双层光晕 + 64px，调整为纯色扁平 + 22px。取向见 §3.1。

Phase 5 跑通即代表项目成立：提问 → 绿快闪 → 调工具 → 绿慢闪 → 需要授权 → 红闪 → 批准后回绿 → 答完亮一下 → 30s 后回 idle。

---

## 9. 依赖清单

```toml
# src-tauri/Cargo.toml —— 已实现部分
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
axum = "0.8"                  # 0.8 的路径参数写作 /{name}，0.7 是 /:name
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["time", "net", "io-util", "macros", "rt-multi-thread"] }
getrandom = "0.3"             # 已在 Tauri 依赖树里，复用版本即免一次编译；用于生成 token
ureq = { version = "3", default-features = false }   # CLI 的 HTTP 客户端，关掉默认 TLS

# 计划中（Phase 5–6b）
tauri-plugin-single-instance = "2"
tauri-plugin-window-state = "2"
tauri-plugin-autostart = "2"
tauri-plugin-global-shortcut = "2"
tauri-plugin-store = "2"
```

几点取舍：

- **锁用 `std::sync::Mutex` 而不是 `tokio::sync::Mutex`。** 所有临界区都是纯内存操作、不跨 `await`，同步锁更简单也让 Tauri 命令能保持同步函数签名。将来若临界区里出现 I/O，必须换成异步锁。
- **依赖尽量挑树里已有的版本**，避免同一 crate 出现两个版本、白付一次编译。加依赖前先 `grep Cargo.lock` 看一眼是划算的习惯。
- **`dirs` 一度选了 7.x，后来去掉了**：它拖进 `dirs-sys` → `option-ext`，而主目录解析用环境变量 8 行就够了。CLI 的依赖树越小越好。
- **`ureq` 关掉默认的 `rustls`/`gzip`**：只连本机明文端口，用不上。

Tauri 已内置 tokio，`tauri::async_runtime::spawn` 可直接用，无需自建 runtime。

---

## 10. 参考资源

**实现参考**

- [guan-ops/Agent-Signal-Bar](https://github.com/guan-ops/Agent-Signal-Bar) — 状态模型与聚合语义的源头（`attention-first aggregation across sessions`）。原生 Swift，窗口层无可借鉴。
- [sobol-sudo/tauri-overlay-demo](https://github.com/sobol-sudo/tauri-overlay-demo) — 最贴近需求的窗口层参考：透明、置顶、点击穿透、全局快捷键、防截屏。
- [Alteredproxy/claude-desktop-pet](https://github.com/Alteredproxy/claude-desktop-pet) — 响应 Claude Code 活动的 Tauri 桌宠。
- [tuanldas/Copet](https://github.com/tuanldas/Copet)、[marine841023/duty-on](https://github.com/marine841023/duty-on) — Tauri 2 桌宠，状态映射可借鉴。
- [gene1wood/claude-code-ntfy-notification-hook](https://github.com/gene1wood/claude-code-ntfy-notification-hook) — hook → 外部通知器的桥接写法。

**官方文档**

- 配置参考（窗口键名以此为准）：https://v2.tauri.app/reference/config/
- 窗口自定义 / 拖拽区域：https://v2.tauri.app/learn/window-customization/
- 系统托盘：https://v2.tauri.app/learn/system-tray/
- Capabilities 权限模型：https://v2.tauri.app/security/capabilities/
- 从 Rust 调用前端（`Emitter` trait）：https://v2.tauri.app/develop/calling-frontend/
- `WebviewWindow` API：https://docs.rs/tauri/2/tauri/webview/struct.WebviewWindow.html
- Claude Code hooks：https://code.claude.com/docs/en/hooks

---

## 11. hook 对接的已核实结论与残留风险

以下结论对着 **官方文档 https://code.claude.com/docs/en/hooks** 和**本机安装的 Claude Code 2.1.296** 双向核实过：文档里的每个事件名（含 `PermissionRequest`、`StopFailure`、`PostToolUseFailure`、`TeammateIdle`、`PostCompact`）以及 `asyncRewake`、`disableAllHooks`、`statusMessage`、`hookSpecificOutput`、`agent_needs_input`、`notification_type`、`stop_hook_active` 等字段，都在 2.1.296 的二进制里存在。细节见 [api.md §4](./api.md#4-智能体接入)。

1. **Claude Code 有 33 个 hook 事件**，名字和大小写以该页面的事件表为准。其中 `PermissionRequest`（工具调用需要权限决策时触发）是「需要授权」的主信号，`Notification` 的 `permission_prompt` 是触发更晚的回落路径。映射表和**明确忽略**的事件清单见 api.md §4.2——把忽略项写清楚同样重要，因为 `TaskCreated`/`FileChanged` 这类事件频繁且与「一轮工作」无关，误用会让灯球乱闪。
2. **`"async": true` 是 `type: "command"` 独有字段**，且异步 hook **不受 `timeout` 约束**、**不能做决策**、**stdout 输出畸形 JSON 会崩溃会话**（v2.1.202 之前）。因此 CLI 必须自己限制 HTTP 超时，且 hook 模式下 stdout 必须为空。
3. **Codex 与 Cursor 都有 hook 系统**，字段与 Claude Code 大致同构（Codex 是 PascalCase + `session_id`，Cursor 是 camelCase + `conversation_id`），**不需要日志监听**。三家的配置路径、事件名、字段差异见 api.md §4.6 / §4.7。

仍然残留的风险：

- 事件处于活跃演进中，我们核实的是 2.1.296 这一个版本。映射表是 `match` 语句，将来加分支的成本很低；不需要为了「未来可能有新事件」预先设计抽象。
- Codex 的 `PermissionRequest` 输出 JSON 的确切键名只在解析器里见过（`decision`，未能确认是否为 `permissionDecision`）。我们不上报决策，所以不影响。
- Cursor 没有可直接映射的「需要授权」事件（它的授权是靠 hook 返回 `permission: allow|deny|ask` 实现的，上报型 hook 不返回该字段）。这是 Cursor 侧的能力缺口，不要假装有。
- Codex 的 `SessionEnd` 默认超时只有 1 秒（上限 3 秒），且总是同步执行——这条路径上的 HTTP 调用必须极快，超时要设得比其它事件更短。
