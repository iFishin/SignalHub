# 信号中枢第三轮升级计划（写给实施的智能体）

前两轮（`plan-lamp-expansion.md` / `plan-hub-round2.md`）已全部落地。本轮由用户在
2026-10-16 勾选，**七项 + 一次设置界面的重设计**，并要求「用户体验优先」。

| # | 项 | 一句话 | 规模 |
|---|---|---|---|
| R1 | **设置面板重排 + UI 重设计** | 分栏的轴统一，页各回答一个问题；补 tooltip / 恢复默认 / 灯预览 | 大（纯前端）|
| R2 | **灯的显示器适应** | 拔掉外接屏后灯会落在屏幕外，然后**再也点不到** | 小（纯 Rust）|
| R3 | **每个状态的停留时长可调** | `done` 现在只有 30s，错过就没了；机制早就在 | 小 |
| R4 | **事件降级通道** | app 没跑时 hook 上报**静默丢弃** | 中 |
| R5 | **自动更新** | 升级通道；步骤已写在 workflow 注释里 | 中 |
| R6 | **Linux 出包** | CI 三平台跑测试，release 只有 mac/win | 中 |

---

## 0. 先读这一节

### 0.1 沿用两轮的红线（全部继续有效）

七条见 `plan-lamp-expansion.md` §0.2，两条见 `plan-hub-round2.md` §0.2。与本轮最相关：

1. **几何是两处真值**（Rust 的 `sign_width` / `sign_height` / `collapsed_window_size`
   与 `lamp.css`）。R1 重设计**不许碰灯的几何契约** —— 设置面板可以随便改，`lamp.css`
   里的 `calc(var(--size) * …)` 一个都不许动。
2. **常驻窗口的动画只动 opacity**。
3. **新能力默认关**，且 `Settings` 的四件事一起做：`Default` 给对值、`clamped()` 夹紧、
   前端 `DEFAULTS` 同步、导出/导入往返测试。加字段靠容器级 `#[serde(default)]`。
4. **出站转发宁可「少发」也不能「发成别的意思」**；一条钩子失败不许影响另一条。

### 0.2 本轮新增的五条

10. **hook 路径上不许新增无界的 I/O**（R4）。智能体的每一次工具调用都会走 hook，
    这条路径上任何落盘都必须是「有硬上限、失败即放弃」—— 写不进去宁可丢事件，
    **绝不能卡住用户的智能体**。时间与体积两个上限都要有。
11. **更新失败必须静默且可退**（R5）。自动更新是唯一一个会「自己改自己」的功能：
    不弹错误、不阻塞启动、失败退回去接着用旧版；未经真机验证的安装路径不许默认开启。
12. **Linux 产物先标 beta**（R6）。透明 + 置顶 + 无边框在 Wayland/X11/不同 WM 上行为
    不一，我们没有 Linux 真机可验 —— 发布说明里写清楚，不假装它和 macOS 一样稳。
13. **R3 的「永不过期」不做**。`idle` 早先没有 TTL 的教训（一个忘关的会话能显示半小时
    「空闲」）是本项目的一条经验，所以 TTL 只允许在区间内调，**不提供「永不消失」**。
14. **设置面板的每一行都要能一眼说清「这是什么」**（R1）。tooltip / 提示语是这一轮
    的验收标准之一，不是可选项。

### 0.3 现状速查（开工前先看这几行）

| 事实 | 位置 |
|---|---|
| 设置面板 **8 个页签**，`docs/architecture.md` §3.16 还写着「七项」（已漂） | `SettingsPanel.tsx::tabOptions` |
| 最大三页：转发 321 行 / 外观 280 行 / 接入 267 行；最小的历史只有 18 行 | 见 §1.1 的表 |
| 逐状态能被覆盖的只有三样：颜色 / 形状 / 闪烁周期 | `config.rs::StateStyle` |
| TTL 焊死在 Rust，只有上报端能用 `--ttl` 覆盖 | `state.rs::default_ttl`、`state.rs:389` |
| 灯的位置按**物理坐标**存在文件里，启动时直接用，不校验还在不在屏幕上 | `lamps.rs::create`（`positions.get`）、`lamps.rs:113 default_position` |
| hook 连不上 app 时只写一行日志就返回错误，**事件丢掉** | `bin/signalhub.rs::hook`（`expect_success` 分支）|
| 出站转发的结果落盘、`outbound.json` 环形缓冲 | `outbound.rs` |
| 仓库里没有 `window-vibrancy`，`tauri.conf.json` 的 `bundle.targets = "all"` | `src-tauri/tauri.conf.json` |
| release 只跑 mac aarch64/x86_64 + win x64 | `.github/workflows/release.yml` 的 matrix |
| 自动更新的 5 步已写在 workflow 注释里（含「插件没装时 `plugins.updater` 无效」这个顺序坑）| `.github/workflows/release.yml` 文件头 |
| 灯的右键菜单是**原生**的，分「某组」和「整盏灯」两种 | `menu.rs`；`SignalLamp.tsx::openGroupMenu/openLampMenu` |
| 设置面板的控件词汇：`section` / `section-title` / `row` / `seg` / `chip` / `switch` / `slider` / `hint` / `muted-text` | `src/styles/settings.css` |

---

## 1. R1 设置面板重排：把「页」的轴统一

### 1.1 问题（都是量出来的，不是感觉）

「乱」的根源是**同一类东西被拆到两页，而且两边各放一半**：

- **灯的行为**：「安静模式」「贴边吸附」在【外观】，而**同层概念的「点击穿透」在【系统】**
  （代码里都是 `useToggles`，都是「鼠标能不能碰到灯」）。
- **语言**在【系统】，可它是界面的事。
- 【外观】一页干三件事：①全局观感 ②逐状态表（10 行）③**各组的显示** —— 而第三件
  随会话数**无界增长**（按会话分组时每个会话一行），是整页唯一会变长的部分，
  也是唯一「数据」而不是「偏好」的部分。
- 【接入】与【指南】都在回答「怎么接上」。
- 【转发】是一列卡片（无界增长）+ 一个测试按钮。
- 所有 `hint` 都堆在页尾，和它解释的那一行隔了十几行。

### 1.2 方案：一根轴 + 分组导航

**轴 = 这一页在回答什么问题。** 左栏从「8 个平铺按钮」变成「3 组 + 1 个沉底」：

```
看          ← 只读观察
  状态         现在怎么样
  历史         这几天怎么样
灯          ← 它的样子与行为
  外观         它长什么样（预设/形态/大小/形状/配色/透明度/牌宽高/闪烁/牌上几段
               + 可展开的「逐状态覆盖」，含 R3 的停留时长）
  行为         它怎么动（安静模式/贴边吸附/点击穿透/保持在最前/托盘跟随）
  分组         每个组怎么显示（显示名/置顶/隐藏/静音）—— 无界的那一节，独立成页
接          ← 怎么接、怎么转发
  接入         检测/一键接卸/试一下/分组方式/局域网
  转发         出站钩子 + 测试
  指南         手册/接口速查/FAQ
────────
  系统         语言/开机自启/配置导入导出/关于（沉底，像常见的齿轮位）
```

- 「各组的显示」独立成【分组】：它是**唯一会变长**的一节，搬走之后【外观】回到固定高度，
  也回到「只回答灯长什么样」。
- 「逐状态」不单独成页，而是【外观】里的**可折叠段**（默认收起，折叠时显示一行摘要
  「3 个状态有自定义」）—— 它回答的是「灯的样子」的一个子问题。顺带解决「10 行表
  把整页淹掉」。
- 【接入】与【指南】**不合并**（一个是要你动手的向导，一个是查的手册），但要
  ①删掉重复的「三步接上」（真流程在【接入】）②互相留一句话指向对方。

### 1.3 三个通用件（UX 优先的实质）

1. **每行一句 tooltip**（`title`）+ 需要多说的用**行下 `hint`**，不再堆页尾。
   `Row` 组件加 `hint?: string` 与 `title?: string`。
2. **恢复默认**。`Settings` 的 `DEFAULTS` 已经是唯一真值，所以判据是现成的：
   - 每行：当前值 ≠ 默认值时，行尾出现一个 ↺；
   - 每段（`section`）：段内任一字段非默认时，标题右侧出现「恢复本段」。
   实现走一个通用 `<ResetOnChange/>`，不要每行手写。
3. **灯预览**。设置面板里放一张**真的用 `lampVars()` 渲染**的预览牌，让「折叠态 / 灯被
   别的窗口挡住 / 灯在另一块屏」时也能看见效果。
   - 前提：把 `--sign-alpha` 从 `SignalLamp.tsx` 挪进 `lampVars()`（现在它由组件单独
     设置，是第二处真值）—— 这样预览与灯共用同一份变量来源。
   - 预览卡是**纯展示**：注册表里那几段用示例值，不可点、不可改（点它不该误触发改名）。

### 1.4 坑

- **不许动灯的几何**（红线 1）。R1 只改设置窗口，`lamp.css` 的几何一行都不改。
- 预览牌复用的是 `.sign` 这套类，而 `.sign` 的颜色变量来自 `--sign-*`。预览必须自己
  带 `lampVars()` 的返回值和 `.lamp-root` 那层变量，否则会串到设置面板自己的主题上。
- 「恢复默认」别做成「重置全部」：用户会误以为会清掉整份配置。粒度就是「行」和「段」。
- 页的重划会动 `docs/architecture.md` §3.16 的表（那里现在还是「七项」）——**同一次提交里改对**。

### 1.5 验收

- 左栏是 3 组 + 1 沉底；`tabOptions` 与 §3.16 的表一致。
- 每个开关/滑杆的 `Row` 都有 `title`；页尾不再有孤立的 `hint`。
- 改任意一行，行尾出现 ↺，点了回到 `DEFAULTS`；整行恢复后 ↺ 消失。
- 预览牌在【外观】里，改大小/形状/配色/透明度/牌上几段时**即时**跟着变。
- `npm test` / `npm run build` 绿；新增的通用件有单测（`DEFAULTS` 判据）。

---

## 2. R2 灯的显示器适应

### 2.1 问题

`lamp-positions.json` 存的是**物理坐标**，`create` 时原样 `set_position`。拔掉外接屏、
改分辨率、换布局之后，那个坐标可能落在**当前不存在的屏幕上** —— 于是灯在屏幕外，
**用户再也点不到它**（只能去删配置文件）。这是本轮唯一的「真 bug」。

### 2.2 方案

1. 把「夹回可见区」抽成**纯函数**（可测、不依赖 Tauri）：
   `clamp_into(area: Rect, size: (i32, i32), pos: (i32, i32)) -> (i32, i32)` ——
   把窗口矩形夹进 `area` 的范围（边距取一个小的 `MARGIN`，别贴着屏幕边）。
2. `create` 拿到记忆位置后先夹：取「离窗口中心最近的显示器」的工作区（含菜单栏/Dock
   的 `work_area`，不是整块屏），再用 `clamp_into`。
3. 拿不到任何显示器信息时**原样使用**（和 `default_position` 现在的态度一致：不硬编坐标）。
4. 顺带：新灯的默认位置从「主屏」改成「**鼠标所在的那块屏**」（`cursor_position` 找
   最近的显示器），多屏用户第一盏灯会出现在他正在看的那块屏上。拿不到就退回主屏。

### 2.3 坑

- 坐标是**物理像素**，`work_area()` 也是物理像素，别混进逻辑坐标（Retina 上会差一倍）。
- 夹的时候要**先按 target 显示器的 scale 换算窗口尺寸**，否则高 DPI 屏上尺寸会偏。
- 别把「在屏幕外一点点的灯」当成 bug 硬拉回中间 —— 夹的是**边界**，不是中心。

### 2.4 验收

- `clamp_into` 的单测：四个方向越界、完全在屏外、完全在屏内（不动）、尺寸大于区域。
- 手工：把 `lamp-positions.json` 改成 `[9000, 9000]`，重启 → 灯出现在最近的屏幕内
  （而不是消失）。

---

## 3. R3 每个状态的停留时长（TTL）可调

### 3.1 目标与边界

用户要能说「`done` 给我停 5 秒」或「`error` 停 10 分钟」。**不做「永不消失」**（红线 13）。

### 3.2 方案

1. `config.rs::StateStyle` 加 `ttl_ms: Option<u64>`（`is_empty()` 一起算上它，
   `clamped()` 夹到 `[TTL_MIN, TTL_MAX]`，建议 5s – 24h）。
2. `state.rs`：`Signal::default_ttl(self)` 变成 `ttl_for(&self, settings: &Settings)`：
   先看 `settings.styles[state].ttl_ms`，没有再回落到现在这张硬编码表。
   **默认表一个数都不改** —— 升级不该改变任何人的灯。
3. `--ttl` 的优先级最高（上报端显式指定 > 设置 > 默认），保持现状。
4. 前端：`StateStyle` 的 TS 类型加 `ttl_ms`；写在【外观】的逐状态表里，
   一列秒数输入（占位符显示该状态的**默认值**，这样「没配」和「配成默认值」看起来一样，
   行为也一致）。

### 3.3 坑

- `off` / `offline` 的 TTL 没有意义（它们不是会话状态），**逐状态表里不给这两个行**
  留 TTL 输入，别让用户去调一个不起作用的数。
- 导出/导入往返测试要带上新字段（红线 3）。
- 前端 `DEFAULTS.styles` 是 `{}`，别为了这个字段改成带默认值的对象 —— 那会改变
  「预设是否还生效」的判断。

### 3.4 验收

- Rust：设置覆盖默认、夹取上下界、`--ttl` 仍然优先（三条单测）。
- 手工：把 `done` 调到 60s，上报一个 done → 牌留一分钟才消失。

---

## 4. R4 事件降级通道（app 没跑时不丢）

### 4.1 问题

hook 上报是「一次 HTTP，失败就记日志」。app 没运行时（重启电脑、退出 app、
刚装还没启动）**所有事件静默丢失**，而 hook 在后台跑，用户根本不会发现。
`docs/api.md` §5 自己就写着这条「还没做」。

### 4.2 方案

1. **CLI 侧落盘**：`hook` 在**连接失败/超时**时（注意：**不是** 4xx —— 那是请求本身
   不对，重放也一样错），把这一条事件追加到 `~/.signalhub/spool.jsonl`（一行一条 JSON：
   `{ at_ms, method, path, body }`）。
2. **硬上限（红线 10）**：文件超过 `SPOOL_MAX_BYTES`（建议 256 KiB）或
   `SPOOL_MAX_LINES`（建议 200）时，**从头部丢**（丢最老的）。写盘失败直接放弃，
   绝不让 hook 返回非零。
3. **app 侧重放**：应用启动、HTTP 服务起来之后，把 spool 里的记录按原 `at_ms` 顺序
   重放进状态机，然后清空 spool。重放走**和正常上报同一条 ingest 路径**，不另写一套。
4. 重放时每条都带原来的时间戳 → **TTL 自然生效**：一条 3 小时前的 `working` 会被
   立刻判定过期（对，因为它确实过期了）；`error` 这种长 TTL 的会正常显示。
5. 重放发生/失败都在 `hooks.log` 里留一行（这是用户唯一能查的地方）。

### 4.3 坑

- **别用「app 的端口文件存在」当判据**：进程崩过之后端口文件还在，那是常见的假阳性。
  判据是**这次连接本身成不成功**。
- 重放要**幂等友好**：同一会话的多条按顺序应用，最后一条赢 —— 现在的状态机已经是
  这样，别在重放里做聚合优化。
- 别把 spool 做成「队列守护进程」。它就是一个文件 + 启动时读一遍。

### 4.4 验收

- Rust：写满上限丢最老、坏行跳过、重放后文件清空（三条单测）。
- 手工：退出 app → 上报 3 个状态 → 启动 app → 三个状态按时间出现（过期的那个不出现）。

---

## 5. R5 自动更新

### 5.1 方案（照 workflow 注释里那 5 步）

1. `Cargo.toml` 加 `tauri-plugin-updater`，`lib.rs` 里 `.plugin(tauri_plugin_updater::Builder::new().build())`。
2. `capabilities/default.json` 的 permissions 加 `updater:default`。
3. `tauri.conf.json` 加 `plugins.updater`：`pubkey` 填**公钥内容本身**；`endpoints`
   指向 `https://github.com/iFishin/SignalHub/releases/latest/download/latest.json`。
   **顺序不能反**（插件没装时这个配置键会先报错）。
4. `bundle.createUpdaterArtifacts: true`（否则没有 `.sig`，也没有 `latest.json`）。
5. 生成密钥对（`npm run tauri signer generate`），私钥内容进仓库 Secrets
   （`TAURI_SIGNING_PRIVATE_KEY` / `..._PASSWORD`），公钥进配置。
6. **入口**：【系统】页一个「检查更新」按钮 + 启动后每天静默检查一次；有更新时**只显示**，
   由用户点「下载并安装」。**默认不开自动安装**（红线 11）。

### 5.2 坑

- **未签名 mac 包替换 bundle 之后，Gatekeeper 会不会拦**是唯一的未知数 —— 必须真机
  验一次（装 0.2.4 → 发布 0.2.5 → 在 0.2.4 里点更新 → 装完能启动）。验不过就
  如实标注「mac 上暂不可用，仍用手动下载」，不要假装通了。
- 更新检查的网络失败必须静默（红线 11）—— 这是常驻应用，不能因为 GitHub 抽风就报警。
- 更新完要重启应用；灯窗口的位置/设置都在 `~/.signalhub`，不受影响。

### 5.3 验收

- 装了 0.2.4 的机器能在【系统】页看到 0.2.5 并装上；启动后版本号变成 0.2.5。
- 断网时点「检查更新」→ 一句「检查失败」而不是崩。

---

## 6. R6 Linux 出包

### 6.1 方案

1. release 矩阵加 `ubuntu-22.04` / `x86_64-unknown-linux-gnu`，`bundles: deb,appimage`。
2. 矩阵里加一步 apt 装 Tauri 的 Linux 依赖（`libwebkit2gtk-4.1-dev` / `libappindicator3-dev`
   / `librsvg2-dev` / `patchelf` 等）。
3. 产物结构照 macOS 那套：**CLI 和 app 必须在同一个 bundle 里**（`externalBin` 已经把
   `signalhub` 打进去，别在 Linux 上漏掉）。
4. 发布说明加一段 Linux 使用提示：透明/置顶在 Wayland 上可能要用 `GDK_BACKEND=x11`
   启动；AppImage 在容器里要用 `--appimage-extract-and-run`。**标 beta**（红线 12）。

### 6.2 坑

- Linux 上没有 `create-dmg` 那类工具的问题 —— Linux 产物是 deb/AppImage，不依赖那些。
- `beforeBuildCommand` 会跑 `stage-cli`，那个脚本要在 Linux 上也能跑（它是 Node，应该没问题，
  但要确认 `chmod +x` 和 target 三元组命名）。
- 别为了 Linux 去改 `main` 的窗口配置（透明 + 无边框 + 置顶在 macOS 上是对的）。

### 6.3 验收

- tag 一个 rc（或直接 0.2.4）→ CI 出 Linux 产物 → 下载 AppImage，
  `--appimage-extract` 后跑 `signalhub --version` 与 `signalhub status`。

---

## 7. 实施顺序与提交

一步一提交，提交信息用中文、说清「为什么」；`git commit -- <paths>` 限定路径；
push 用 `GIT_SSH_COMMAND='ssh -i ~/.ssh/id_ed25519 -o IdentitiesOnly=yes' git push`。

| 顺序 | 提交 | 内容 | 依赖 |
|---|---|---|---|
| 1 | `R2` | 灯的显示器适应（纯函数 + 单测 + 接进 `create`） | — |
| 2 | `R1-a` | 设置面板 IA 重排：分组导航 + 页重划 + 文档 §3.16 | — |
| 3 | `R1-b` | 通用件：行内 tooltip/hint、恢复默认、灯预览（含 `--sign-alpha` 挪进 `lampVars`） | R1-a |
| 4 | `R3` | 每状态 TTL 可调（Rust 设置 + 逐状态表那一列） | R1-b |
| 5 | `R4` | 降级通道（CLI 落盘 + app 重放） | — |
| 6 | `R5` | 自动更新（插件 + 配置 + Secrets + 入口） | — |
| 7 | `R6` | Linux 出包（矩阵 + 依赖 + 说明） | — |
| 8 | 发布 | 版本号 0.2.4（5 处）+ tag + 发布 | 全部 |

每步必须过：
`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`、
`cargo test`（先 `npm run stage-cli`）、`npm test`、`npm run build`。
**不要跑 `cargo fmt`**（仓库不是 rustfmt-clean）。

R5 的真机验证与 R6 的产物验证，做完要在本文件末尾追加「实施记录」一节，
把「验过什么 / 没验出什么」写清楚 —— 尤其是 macOS 上未签名包自更新的结论。

---

## 实施记录（2026-10-16）

六个提交按计划顺序落地，外加发布：

| 提交 | 内容 |
|---|---|
| `3f6585c` | R2 灯的显示器适应 |
| `be75e3d` | R1-a 设置面板 IA 重排 |
| `2291f54` | R1-b 通用件（说明跟着行走、↺、灯预览） |
| `7028ed0` | R3 每状态停留时长可调 |
| `1013416` | R4 降级通道 |
| `932f086` | R5 自动更新 |
| `7b6c91c` | R6 Linux 出包 |
| `5d043fe` | 发布 0.2.4（tag `v0.2.4`） |

每步都过：`clippy -D warnings` 干净、`cargo test`（309 lib + 204 CLI）、
`npm test`（65 → 72）、`npm run build`。界面改动逐张看过渲染结果（headless Chrome）。

### R5 自动更新：验过什么

**端到端那条链真的跑通了**，证据三层：

1. **真端点、匿名取回。** 发布之后匿名拉
   `https://github.com/iFishin/SignalHub/releases/latest/download/latest.json`：
   HTTP 200、5.3 KB、`version: 0.2.4`；里面 `darwin-aarch64` 那个 asset URL 匿名下载
   6 025 855 字节，**sha256 与带认证下的产物逐字节一致**。这一条正是 app 运行时走的路
   （`Accept: application/json` 查清单、`application/octet-stream` 下载、跟随重定向）。
2. **下载 → 验签 → 解包 → 换 bundle → 重启。** 用 CI 产出的真 `.app.tar.gz` 与真签名，
   让一个**本机编的 0.2.3**（端点点到一份 HTTPS 清单，托管在临时 gist 里，用完已删）
   去更新。结果：日志 `[update] 有新版本 0.2.4` → 装 → **换完之后那个包里的二进制与
   CI 产物逐字节相同**（sha256 `4effbe27…`），进程在装机那一秒之后才起来
   （`13:35:22Z` 装完 / `13:35:26Z` 新进程）。
   → **未签名的包被更新替换之后能正常启动**：那条只能真机验的 Gatekeeper 疑问，
   在这个场景下答案是「不拦」（更新过程写下的文件不带 `com.apple.quarantine`，
   Gatekeeper 只对隔离过的包发作）。
3. **换完之后它自己又查了一次真端点**（新包里的端点就是 `releases/latest`）：
   没有失败行 —— 也就是静默成功、报「已是最新」。这条顺带从反面验了
   「自动检查失败是静默的」。

**验法的两处不同，必须说清**：被更新的那一份是本机编的 **0.2.3**（为了让「远端版本 >
当前版本」和签名里的 `version:0.2.4` 对得上 —— 插件会比对这两者，拿 0.2.5 去配 0.2.4
的签名会被它正确拒掉），清单的锚点是一个 gist 上的 HTTPS URL 而不是配置里那串
`releases/latest`。差别只在**配置值**和**被更新的那一份是哪个构建**；
下载、验签、解包、替换、重启这段代码路径是同一段。

**没验的**：以 `releases/latest` 为锚点，把一个已发布的 0.2.4 换成「下一版」这一整条
（要真发一版更高的版本才能测，而发一版内容相同的没有意义 —— 真想验的话，下一次
正常发版就是那次验收）。

**R5 的边界（如实说，别对外说成「三个平台都能自动更新」）**：只有 macOS。
Windows 是**上游卡死的**（插件的 `zip` feature 要 `zip = "^4"`，而 zip ≤ 9.0.2 在
RUSTSEC-2025-0168 之后被全量 yank，`^4` 无解）；Linux 是**「做得通但没验过」**
（`linux-x86_64-appimage` 指向的是没压缩的 ELF，不吃那个 feature；但 deb 那份要靠
`dpkg -i` 提权，两条路都没在真机验）。所以 `update::SUPPORTED` 只有 macOS，
其余平台界面如实说「请手动下载」+ 给一个「打开发布页」。

### R6 Linux 出包：验过什么

CI 四个 job 全绿（macOS ×2 / Windows / Linux），产物 14 个（含 `latest.json` 与各平台的
`.sig`）。**Linux 那两个包在 macOS 上跑不起来**（没有 Linux 机器，也没有容器工具），
所以验的是结构：

- `.deb` 用 `ar` + `tar` 解开看过：`/usr/bin/signalhub-app`（GUI）与 `/usr/bin/signalhub`
  （sidecar CLI）**都在、都有执行位**，`.desktop` 与图标在，依赖是
  `libayatana-appindicator3-1, libwebkit2gtk-4.1-0, libgtk-3-0`。
- `.AppImage`（84 MB —— 比 deb 大一个数量级，因为把 GTK 那套整包带上了）**本机解不开**：
  squashfs 在 macOS 上没有工具能打开（没有 `unsquashfs`，也没有容器去跑
  `--appimage-extract`）。它在 CI 里和 deb 是同一个 job、同一份 `externalBin` 打出来的，
  但计划里「解出来跑 `signalhub --version`」这一步**这轮没做**。
- 发布说明里 Linux 那段标了 **beta**，写清了 Wayland（透明/置顶可能要退回
  `GDK_BACKEND=x11`）和 FUSE（`--appimage-extract-and-run`）两条已知限制。

### 一处要说清楚的偏差

`Cargo.toml` / `docs/architecture.md` / `release.yml` 里关于「为什么只有 macOS」的
解释改过一版：早先写的是「Linux 的 AppImage 更新是 `.tar.gz`，那两个解包依赖被 `zip`
feature 门着」，**这是错的** —— 查了 v0.2.4 的实际产物，`latest.json` 里
`linux-x86_64-appimage` 指向的是**没压缩的 ELF AppImage**。现在的说法是上面那版：
Windows 归上游，Linux 归「没验过」。
