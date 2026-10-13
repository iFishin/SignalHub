# 信号灯拓展计划（写给实施的智能体）

> 2026-10 起草。三个方向：**多信号灯**（每组一盏真正独立的悬浮灯）、**长宽自定义**、**可用性提升包**。
> 本文按「可以直接开工」的标准写：每项给了目标、方案、改动锚点、坑、验收。
> 引用的行号是起草时的快照，会漂移 —— **以函数名 / 常量名为锚**，行号只当入口提示。

---

## 0. 先读这一节：背景与红线

### 0.1 项目形态

- Tauri 2（Rust edition 2024）+ React 19 / TypeScript + Vite。
- **一个 bundle 服务两个窗口**：灯窗（label `main`）与设置窗（label `settings`），
  靠 `index.html?window=light|settings` 分流（`src/lib/env.ts` 的 `windowKind()`），
  CSS 以 `html[data-window]` 定锚。`src-tauri/capabilities/default.json` 的
  `windows: ["main", "settings"]` 是窗口白名单。
- 事件推送是**广播**：`app.emit("signalhub://state", snapshot)`（`notify.rs:12` /
  `notify.rs:291`）与 `signalhub://settings`。**任何新窗口自动收到全部快照**，
  前端自己过滤要画哪个组 —— 这是多灯方案最大的顺水推舟，别另起推送通道。
- 状态机与聚合在 `state.rs`（`Store`）；HTTP 服务在 `server.rs`；设置在
  `config.rs`（`Settings`，`Arc<Mutex>` 共享、改完即生效不重启）。
- CLI（`bin/signalhub.rs`）用 `#[path]` 直接共享 `config.rs` / `state.rs`，
  **新公共 API 尽量放这三个文件里**，别把 Tauri 类型拖进共享层。

### 0.2 七条红线（违反任何一条，PR 都该被打回）

1. **几何是两处真值**。Rust（`config.rs` 的 `sign_width` = `size*7`、
   `sign_height` = `size*3/2` 或 `*5/2`、`collapsed_window_size`）与 CSS
   （`lamp.css` 的 `calc(var(--size) * …)`）**必须逐字一致**，差 1px 就会被
   `overflow: hidden` 裁掉最后一块牌。改一处必须同步另一处，同一次提交里。
2. **常驻窗口的动画只动 opacity**（`lamp.css` 文件头第 2 条，交给 GPU 合成；
   动 transform 会让它每帧重排）。现存唯一违例是 `pop`（`lamp.css:415` 用了
   `transform: scale()`）—— P3-1 顺带修掉，别新增违例。
3. **默认值不许无声改变既有行为**。新能力一律**默认关**，开关写进
   `Settings`（先例：`group_by` 默认 `none`、`lan_access` 默认 false，理由见
   `architecture.md` §3.7「默认必须是 none」那段）。
4. **给 `Settings` 加字段：靠容器级 `#[serde(default)]`，别再逐字段标。**
   `Settings` 上已经有它（`config.rs:591`）—— 缺失字段会回落到
   `Settings::default()` 的同名字段，所以**新字段不需要写任何 serde 属性**。
   真正要做的是另外三件，缺一不可：
   - **在 `Default` 里给对值**：`Default` 是**手写**的结构体字面量
     （`config.rs:805`），漏字段**编译不过**（这是好事），所以风险不在漏、
     在写错 —— 倍率的默认必须是 `1.0`，写成 `0.0` 就是牌宽归零；
   - **`clamped()` 里夹紧**（`config.rs:840`）—— 它同时兜住手改配置写进来的
     离谱值，所以范围检查不是可选的；
   - **`useSettings.ts` 的 `DEFAULTS` 同步**，且
     `settings_survive_an_export_import_round_trip` 里把新字段设成**非默认值**
     （照抄那条测试的注释：全默认的话漏字段看不出来）。
   > 顺带一个既有小瑕疵：解释 `Settings` 为什么用 `serde(default)` 的那段注释
   > （`config.rs:229–233`）被粘在了 `Shape` 的文档上（里面还写着「不派生
   > `Copy`：`webhook_url`……」—— 而 `Shape` 恰恰派生了 `Copy`）。改这块时
   > 顺手把它挪回 `Settings` 上方。
5. **菜单 id 是全局分派的**（`menu.rs` 文件头注释）：三个来源（托盘、灯右键、
   组右键）都汇到 `AppHandle::on_menu_event`，id 前缀（`lamp:*` / 组菜单）必须
   不重合。多窗口后 id 还要带上下文（哪盏灯），见 P2-3。
6. **朴素审美**：不要发光 / 阴影 / 渐变；控件尺寸宁可小也不要大（用户的
   明确偏好，设置面板整份都是 11px 字、`btn-tiny` 风格）。
7. **提交规矩**：`git commit -- <paths>`（多个会话共用这个检出，路径限定防
   带进别人的暂存）；每步一个 commit；push 需
   `GIT_SSH_COMMAND='ssh -i ~/.ssh/id_ed25519 -o IdentitiesOnly=yes -o BatchMode=yes'`
   （`~/.ssh/config` 把 github.com 映射到另一把 key，**不要**去改它）；
   提交信息**不写任何署名尾注**。

CI 三平台（macOS/Windows/Ubuntu）跑 `cargo test` + `clippy -D warnings` +
`npm run build` + `npm test`，基线 0 警告。Windows/Linux 专属代码在你机器上
**编不进去**，别信本地全绿。

---

## 1. 现状盘点（你会碰到的东西）

| 主题 | 位置 | 现状 |
|---|---|---|
| 全局尺度 | `Settings.size` | 一个标量同时定字号、点径、牌宽（×7）、牌高（3/2、5/2）、折叠点径、间距；`MIN_SIZE..MAX_SIZE` 在 `Settings::clamped()`（`config.rs:840`）夹紧 |
| 窗口尺寸 | `apply_window_size`（`lib.rs:925` 附近） | 按内容算：展开 `settings.window_size(&sign_heights(...))`、折叠 `collapsed_window_size(size, dots, badges)`；只在真变了才 `set_size`，随后 `keep_on_screen`（`lib.rs:1064`） |
| 窗口位置 | `tauri-plugin-window-state` | `StateFlags::POSITION` 只记位置（`lib.rs:1216`）；**尺寸永远由内容决定**，这是刻意设计（位置记住、尺寸算出） |
| 分组 | `Settings.group_by`（none/cwd/session）+ 显式 `group` 优先 | `resolve_group`（`config.rs:890` 附近）服务端兜底；一窗多牌 `MAX_SIGNS=4`，超出 +N 角标；前端 `orderVisible`（`lib/lampSort.ts`）叠置顶 |
| 点击/拖拽 | `useDragOrClick`（`SignalLamp.tsx:122`） | 自实现（不能用 `data-tauri-drag-region`，要区分单击）；单击 = `toggle_collapsed`；右键 = `openGroupMenu` / `openLampMenu`（Rust `menu.rs`） |
| 打开目录 | `open.rs` | `reveal(path)`（选中文件）/ `open(target)`，通知按钮已在用 —— 多灯/双击直接复用 |
| 排序基线 | `last_pushed`（`state.rs`） | 快照推送用「可见结构比对」节流；`stale_progress` 分 16 档 —— 改推送频率前先读懂这段 |

两个**已知、刻意延后**的技术债（之前评估为不划算，但 P2 会把它们放大 N 倍，
届时转正为必做，见 P0）：

- `notify.rs` 每次推送深拷贝一份 `Settings`；
- `apply_window_size` 每次推送有 2 次阻塞主线程的往返（`inner_size` / `scale_factor`）。

---

## 2. P0 前置卫生（多灯的前置，不是可选项）

**为什么排在最前**：下面两项在单灯下是微秒级的浪费，在 N 盏灯下是 ×N 的
主线程阻塞与堆分配。先修，P2 就不用背这个乘数。

**P0-1 `apply_window_size` 去阻塞。** `window.inner_size()` / `scale_factor()`
每次推送都同步调；改为缓存 scale factor（窗口创建时取一次，
`tauri::WindowEvent::ScaleFactorChanged` 事件更新 —— 该变体确实存在，
`tauri/src/app.rs:136`，且 `lib.rs:1151` 已有 `on_window_event` 的先例可抄）
+ 记住上次 `set_size` 的逻辑值（自己算出的 w/h 存起来比对，不再问窗口）。
**验收**：推送热路径上没有任何对 window 的同步查询（代码审）；
`keep_on_screen` 行为不变（单测：`lib.rs` tests 里那组「窗口越界拉回」的用例
保持全绿）。

**P0-2 `notify.rs` 去深拷贝。** 推送只需要聚合与静音列表，不需要整份
`Settings`；传 `Arc<Settings>` 或抽一个只读小结构。**验收**：`cargo test` 全绿，
热路径没有 `Settings` 的 `clone()`（代码审 + 之前的注释照旧成立）。

---

## 3. P1 长宽自定义

### 3.1 先想清楚「长宽自定义」是什么

**不要做成「窗口可自由拖角 resize」。** 窗口内容是 CSS 画的：拖大了只会多出
空白，拖小了裁内容 —— 字不会跟着变大。而且「位置记住、尺寸由内容决定」是
这个项目的刻意架构（`architecture.md` 反复写）。正确做法是**尺寸语义化**：
给牌宽 / 牌高一个**倍率**，默认 1.0 = 现状，窗口照常按内容算。

（如果最终确实想要拖拽的手感，见 3.3 的「反推」方案 —— 效果等同，实现仍然
语义化。）

### 3.2 方案：两个倍率字段

```rust
// config.rs —— Settings 新增（记得 Default = 1.0，clamped() 里夹范围）
pub sign_w_mult: f32,   // 0.8 ..= 2.5
pub sign_h_mult: f32,   // 0.8 ..= 2.5
```

- **Rust 侧**：`sign_width(size)` → `sign_width(size, w)` =
  `(size as f32 * 7.0 * w).round()`；`sign_height(size, has_message, h)`
  同理（两档都乘）。所有调用点（`sign_heights`、`window_size`）把倍率传下去 ——
  `Settings` 就在场，顺手的事。
- **CSS 侧**：`lampVars()`（`lib/lampStyle.ts`）多推两个变量
  `--sign-w-mult` / `--sign-h-mult`；`.sign` 的
  `width: calc(var(--size) * 7 * var(--sign-w-mult, 1))`、
  `height: calc(var(--size) * 3 / 2 * var(--sign-h-mult, 1))`。
  **两处真值同步，红线 1。**
- **UI**：设置面板「外观」页，两个小号滑杆（0.8–2.5，步进 0.1，显示 ×1.2 这样）
  —— 样式抄现有 `blink_speed` / `sign_opacity` 那两行，别发明新控件。
- **作用范围**：只作用于**展开态**的牌。折叠态是一排正方形点，长宽倍率对它
  没有意义（写了也只会让点变椭圆）。

### 3.3 可选进阶：拖了就记住（语义化的「自由 resize」）

`main` 窗 `resizable: false` 保持不动；设置页提供「拖出一张预览牌」不现实，
但可以在牌上做：**按住 Option/Alt 拖牌边缘** → 松手时 Rust 把最终窗口宽度
反推 `sign_w_mult = (width - LAMP_MARGIN) / (size * 7)`，clamp 后存设置。
之后 `apply_window_size` 照常算，算出来正好等于用户拖的宽度。高度同理
（注意要先知道这块牌是单行还是双行档）。**只在用户真要这个手感时做**，
先交付滑杆。

### 3.4 坑与验收

- **新字段照红线 4 走**（`Default` 里给对值 + `clamped()` 夹紧 + `DEFAULTS` +
  往返测试；**不需要**逐字段写 serde 属性）。这条对本节的倍率和 P2-4 的
  `lamp_layout` 同样适用 —— 倍率的默认值必须是 `1.0`（「原样」），
  而不是 `0.0`（那是牌宽归零）。
- 倍率只乘在**牌**上；点径、字号、牌间距仍是 `size` 派生 —— 别把倍率泄进
  `.dot`，否则折叠态也变形。
- `MAX_SIGNS` 内 `.sign` 宽了之后，窗口宽 = `sign_width + margin`，一行式，
  不影响高度累加逻辑；但**牌高变了**，`sign_heights` 的两档都要乘，
  漏一档窗口就裁牌。
- **验收**：`cargo test`：倍率 clamp、`sign_width/height` 算式、导出/导入
  往返含新字段（`settings_survive_an_export_import_round_trip` 把新字段设成
  非默认值）**，外加一条「老配置（无新字段）能解析出默认倍率」**；前端：
  `npm test` + 浏览器实测牌宽像素 = `size*7*mult`（±2px，做法抄
  `lampStyle.test.ts` 两份测试的思路）；三平台 CI 绿。

---

## 4. P2 多信号灯（真·每组一盏独立悬浮灯）

目标形态：`lamp_layout` = `"single"`（现状，默认）| `"per-group"`。
后者下每个**可见组**一盏自己的灯窗，可各自拖到不同位置；`main` 窗保留为
**全局灯**（显示全局聚合、暂停/穿透角标、点击开设置）—— 不拿掉它：通知 /
托盘的语义映射都以全局聚合为锚，留一盏总灯让「全局怎么样」永远有处可看。

**多灯模式下主灯不再画各组牌**（这条容易漏，漏了就每盏灯的信息在主灯上再重
复一遍）：布局是 `per-group` 时，主灯退化成**一颗全局灯珠**（等价于现在的
折叠形态几何 —— `collapsed_window_size(size, 0, badges)` 正好是它的尺寸），
各组交给各自的灯。渲染分支据此切，`SignalLamp` 共用同一组件、传不同的
`groups`（主灯传空 = 走现有的占位/全局分支）。

### 4.1 分四步，每步一个 commit

**P2-1 窗口管理器（纯 Rust，可单测）。** 新文件 `src-tauri/src/lamps.rs`：

```rust
pub struct LampWindows { /* label 映射 + 已知窗口集合 */ }
impl LampWindows {
    /// 快照推送后调：diff「该有的组」与「已有的窗」，多退少补。
    pub fn reconcile(&mut self, app: &AppHandle, visible: &[String]);
    /// 组名 → 稳定 label（见下面的坑）
    fn label_for(group: &str) -> String;
}
```

- **label 的坑（最容易埋雷的地方）**：Tauri 窗口 label 只允许
  `[a-zA-Z0-9-/:_]`，而组名可以是**中文、路径**（按 cwd 分组就是路径）、
  UUID。不能直接拿组名当 label。也不能用自增序号（组消失重建后序号变了，
  位置记忆就丢）。做法：**确定性转义** —— 组名 percent-encode 后过长截断，
  再拼一个组名的短 hash（比如 FNV-1a 的 hex）保证唯一与稳定：
  `lamp-<encoded>-<hash8>`。同组名永远同 label。
- **窗口生命周期**：组出现 → `WebviewWindowBuilder` 建窗（属性照抄
  `tauri.conf.json` 里 `main` 的那一套：transparent / decorations:false /
  alwaysOnTop / skipTaskbar / shadow:false / resizable:false，
  `url: index.html?window=light&group=<encoded>`）；组隐藏
  （`hidden_groups`）或 TTL 彻底消失 → 关窗。**stale 宽限期内不关**（stale
  本身是信息）。关窗前把位置手动 save（见下面的坑）。
- **位置记忆的坑**：`tauri-plugin-window-state` 在退出时保存，但**被 close
  掉的窗口的最后位置可能不落盘**。两个选：① 组窗 close 前手动调插件的
  save；② 干脆组窗的位置**自己记**（`~/.signalhub/lamp-positions.json`，
  label → (x,y)，`config.rs` 的 `atomic_write` 现成）。推荐 ②：动态 label 的
  生命周期和插件假设的不完全合拍，自记一份更可控，也就一个 map 的读写。
- **单测**：`label_for` 稳定性（同名同 label、不同名不同 label、中文/路径/
  超长名）；`reconcile` 的增 / 删 / 保留 diff（不需要真窗口，抽 trait 或把
  建/关动作用闭包注入 —— 项目里 `TempDir` 的用法是先例）。

**P2-2 capabilities 与前端分流。** `capabilities/default.json` 的
`windows` 加 `"lamp-*"` —— **glob 是官方支持的**：schema 里写明
「Can be a glob pattern」，出处 `src-tauri/gen/schemas/desktop-schema.json`
里 `Capability.windows` 的 description（仍要在真机上跑一次多窗确认）。
前端 `windowKind()` 边上加 `windowGroup()`（读 `?group=`，
percent-decode）；`SignalLamp` 收到的 `groups` 先
`filter(g => g.name === mine)` 再走现有渲染 —— **一行过滤，别改渲染本身**，
单灯和组灯共用同一组件。`MAX_SIGNS` 对组灯无意义（它只有一块牌），
`+N` / `+N` 角标逻辑对组窗天然不触发（hidden 恒空）。

**P2-3 菜单上下文（多窗下最容易错的地方）。** 组菜单的 id 现在是全局分派的，
多盏灯各自右键时，分派处必须知道「这张菜单是哪个组的」。做法：`menu.rs`
建一个注册表 `Mutex<HashMap<u32, String>>`（短 id → 组名），组菜单 id 用
`group:<短id>:<动作>`；`on_menu_event` 分派时查表、用完即清（窗口关了菜单
也不会再点）。**短 id 用递增计数，不用组名本身**（id 有字符集限制，
同 P2-1 的坑）。托盘菜单 / `main` 灯菜单不带上下文，维持现状。
**验收**：两盏灯同时右键，各自菜单作用于各自组（真机手测清单里必有一条）。

**P2-4 per-window 几何与设置页开关。** `apply_window_size` 拆两半：
`main` 窗照旧（全局灯）；组窗每个只有一块牌 →
`sign_w + margin` × `sign_height(单块)` —— 比 `main` 还简单，因为不用管
`MAX_SIGNS` 与 +N。设置页「接入」页分组那节旁加 `lamp_layout` 的
`seg` 三态改两态（样式抄 `group_by` 那组按钮）；**切换即 `reconcile`**，
文案里写明「每盏灯第一次出现的位置是默认摆放，拖到想要的地方后**各自**记住」
（首次没有历史位置的排布：屏幕右缘纵向堆叠，间距 `size/2`，从上往下，
`keep_on_screen` 兜底）。

### 4.2 明确不做的（写下来防止实施时顺手做了）

- **不做跨组拖拽编组**（把一盏灯拖到另一盏上合并）—— 交互成本高，
  而分组本来就该由上报侧决定。
- **不做每盏灯独立的外观设置** —— `Settings` 是全局的，按组存样式会把
  配置面翻几倍；真要差异化，先在 `group_meta` 里加（已有 per-group 的
  `name` / `order` 先例），外观（颜色/形状/节奏）**暂不**按组。
- **通知 / 托盘 / 出站钩子不感知多灯** —— 它们的语义本来就是全局聚合，
  多灯只是显示层分拆。别在这条链上加「来源灯」概念。

### 4.3 坑与验收

- **N 个窗口 = N 个 webview**：WebKitGTK（Linux）上内存可观。加硬上限
  （建议 8 盏，超出回落单灯模式并在设置页说明），别让「按会话分组」的
  重度用户一开 20 盏。
- 前面说过：`app.emit` 是广播，**别改成 `emit_to` 按窗点对点** —— 广播 +
  前端过滤的改动量小一个数量级，快照本来就不大。
- `main` 窗的 `visible: false` + 启动后 `show()` 的时序（`lib.rs:1208`）别动；
  组窗**建即显示**（`WindowBuilder` 的 `visible` 默认就是 `true`，
  `tao/src/window.rs:308` —— 但建议仍显式写 `.visible(true)`，意图更清楚）。
- **验收**：单测（4.1 列的）+ 三平台 CI 绿 + **真机手测清单**（macOS 与
  Windows 至少各一轮）：① 同一 agent 双会话 → 两盏灯位置各自独立记忆；
  ② 组 TTL 消失 → 灯关、回来 → 原位复活； ③ `hidden_groups` → 关窗，
  取消隐藏 → 复活； ④ 两盏灯各自右键菜单作用于各自组； ⑤ 单灯 ↔ 多灯来回切，
  窗口不残留； ⑥ Linux 上（若手头有）多透明窗的合成器行为。

---

## 5. P3 可用性包（每项独立小步，可穿插在 P 之间）

| # | 项 | 做法 | 锚点 |
|---|---|---|---|
| 1 | **跃迁脉冲**：哪块牌刚变了就闪一下 | 见下面「5.1」—— **纯前端，不用动 Rust** | `SignalLamp.tsx`；`lamp.css` |
| 2 | **打开项目目录** | 见下面「5.2」—— 推荐**放进组右键菜单**，双击有冲突 | `menu.rs`；`open.rs`；`alerts.rs:298` |
| 3 | **贴边吸附**：拖到屏幕边缘 12px 内松手 → 吸到边 | 在 `useDragOrClick` 的 mouseup 处判（前端知道窗口内坐标，屏幕边缘位置从 `getCurrent().outerPosition` 拿或 Rust 侧做 —— 推荐 Rust：`set_position` 一次 + 记忆由 window-state 插件照旧）。只吸附**左右边缘**，上下不吸（任务栏/Dock 在那儿） | `SignalLamp.tsx`；`lib.rs` |
| 4 | **牌宽自适应默认值** | 不单独做：P1 落地后「按会话分组」的牌因为 `default_label` 变长而自然受益；P1 被砍时此项改为「会话牌默认 ×1.3」 | 与 P1 合并 |
| 5 | **折叠态悬停出牌面** | 折叠点 `title` 已有信息（tooltip）；升级为 hover 时**临时展开**（挂 3s 的 `data-collapsed=false` 又不改设置）—— 注意窗口尺寸要跟着临时变大，`apply_window_size` 需要一个「override 高度」的口子。复杂度中等，**排最后** | `SignalLamp.tsx`；`lib.rs` |

### 5.1 跃迁脉冲：不用动 Rust（订正）

初稿写的是「把组级跃迁加进 `state.rs` 的 `Visible` 比对」—— 核对后**不需要**：
`Visible.sessions` 已经是 `Vec<(会话id, 组名, 状态)>`（`state.rs:344`），
也就是说**任何会话的状态变化本来就一定会推一帧前端**，组级聚合是从它派生的。
所以前端拿一个 `useRef` 记住「上一帧每组的 aggregate」，两帧一比就知道谁跃迁了。

- **首帧必须不闪**（ref 初次为 `undefined`，全部当基线记下、不触发脉冲），
  否则应用一启动满屏闪。
- 动画用 **opacity**（0.35→1，约 420ms，一次），**顺带把 `pop` 的 transform
  违例修掉**（`lamp.css:415` 的 `@keyframes pop` + `lampStyle.ts` 里 `done` 的
  配方）—— 那条配方是红线 2 唯一现存违例，改成 opacity 后 `done` 的「落定」
  观感由脉冲本身承担，不再需要单独的 `pop`。
- **验收**：连续两帧同状态不闪（单测钉在比对函数上，抽成纯函数就能测）；
  浏览器里真实跑一遍，确认「A 组变状态时只有 A 那块闪」。

### 5.2 打开项目目录：别做双击（订正）

初稿说「双击牌打开目录，冲突值得权衡」—— 核对后冲突比想的硬：

- 灯的**任何位置单击**都走 `useDragOrClick`（`SignalLamp.tsx:122`）→
  `toggle_collapsed`。双击 = 折叠再展开（净效果为零但**会闪一下**），
  然后才轮到双击处理器。
- 而且**没有可替代的键盘通路**：全局快捷键只注册了穿透那一个
  `PASS_THROUGH_SHORTCUT`（⌥⌘S / Ctrl+Alt+S，`lib.rs:78`），折叠没有快捷键
  —— 所以「把折叠挪走、腾出双击」会直接拿走一个现存能力，违反红线 3。

**推荐做法**：把「打开项目目录」放进**组的右键菜单**（`openGroupMenu` 已经
存在、已经在按组分派），零冲突、和「隐藏 / 静音 / 置顶」同一处，用户找得到。
路径取该组 winner 会话的 `cwd`，**argv 传路径绝不过 shell**（`alerts.rs` 的
`AlertTarget` 就是这个安全模式，照抄）；没有 `cwd` 时菜单项置灰。

**真要双击也可以**，但那是一次明确的交互变更，得先问用户：把「单击整灯 =
折叠」收窄成「单击圆点/空白 = 折叠，单击牌身 = 无操作 / 双击牌身 = 打开目录」。
这是**改变既有行为**，不能由实施方自行决定。

**3 / 4 / 5 的验收**：3 —— 吸附后和 `keep_on_screen` 不打架（吸到边再被
拉回原位那种来回跳必须避免；单测：`clamp_to_area` 那组用例的邻居）；
4 —— 并入 P1 的验收；5 —— 临时展开期间**不改设置**（退出重开仍是折叠态），
且松开/超时后窗口尺寸回到折叠尺寸（否则会留一个变大的空窗）。

---

## 6. 推荐顺序与总验收

```
P0-1 → P0-2          （卫生，半天级）
P1                   （长宽倍率，一天级；本身立刻解决「名字显示不全」的现痛点）
P3-1 跃迁脉冲         （纯前端、与 P1 无交集，可并行）
P2-1 → P2-4          （多灯，主体工程；每步独立 commit、独立可回滚）
P3-2 / P3-3 / P3-5   （收尾小项）
```

**依赖关系**（别弄反）：P3-1 只碰前端渲染，和谁都不冲突；P3-4 并入 P1；
**P2 依赖 P0**（否则主线程阻塞 ×N）；P2-4 的组窗几何依赖 P1 的
`sign_width(size, w)` 已经带上倍率参数 —— 先做 P1，P2 就不用回头改签名。

**每步的总验收**（不满足就不算完）：`cargo test` + `cargo clippy -D warnings` +
`npm run build` + `npm test` 全绿；三平台 CI 绿；动了几何的，浏览器里**量过
像素**（不是「看着对」）；动了窗口生命周期的，真机手测 4.3 那张清单；
每个 commit 只含该步的文件（`git commit -- <paths>`），提交信息把「为什么」
写清楚 —— 这个仓库的惯例是给未来的自己留设计理由，见任一现有提交。

**总原则再念一遍**：这盏灯的价值在「瞥一眼」。任何把它变得需要「看一会儿」
的提议（图表、趋势线、面板化）都在往另一个应用走 —— 拓展功能时先问一句：
这能不能在 0.5 秒内被看懂？
