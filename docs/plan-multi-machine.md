# 多机中枢（Team / Multi-machine Hub）

> 2026-11 起草。这是 `plan-round5.md` §下一步 里排第三、但被用户勾中的方向：
> **一台机器做 hub，把其他机器的看板拉过来，合并成同一盏灯、同一个收件箱、同一块看板。**

---

## 0. 先读这一节

### 0.1 为什么是它（调研结论）

两条市场信号，都指向同一件事：

1. Claude Code 从 2026-08-14 起对 Pro/Max/Team 默认开 Auto Mode —— 「手机上点允许」
   这个卖点被上游自己吃掉了一半。所以 `plan-round5.md` 里的第 2 项（Away approvals）
   不再是差异点。
2. 真正的、一直在长大的痛点是 **「多个 agent 并行时工作台太乱」**：窗口找不到、任务
   状态记不住、长任务不知道要不要守着。这一条在本项目里已经用「分组 + 收件箱」答了
   一半，**剩下的那一半是「跨机器」** —— 跑在服务器 / 另一台笔记本 / WSL 里的 agent，
   灯上完全看不见。

所以这一轮的定位不是「团队协作功能」，而是：**让灯回答「我所有机器上的 agent，现在
谁在等我」**。个人用户（笔记本 + 台式 / 家用服务器）是主要受益者，团队只是同一套机制
的延伸 —— 这个定位很重要，它决定了下面的默认值都往「一个人管几台自己的机器」靠。

### 0.2 沿用前四轮的红线

`plan-lamp-expansion.md` §0.2 的七条、`plan-hub-round2.md` §0.2 的两条继续有效。
与本轮直接相关的三条：

1. **新增外发 / 新增对外读取面，默认必须关。** 本轮新增的是**出站轮询**（hub 主动去
   拉对端），它比「让对端往我这儿推」安全一档，但仍然是新增的网络行为 —— 所以
   `machines` 默认空数组，空数组 = 一条请求都不发。
2. **不许让既有用户的灯在升级后变形。** `machines` 为空时，快照、窗口尺寸、牌数、
   通知、统计**必须和 0.4.1 逐字节一致**。这是本轮的硬约束，下面 §4 的每条规则都为它
   服务。
3. **几何是两处真值。** 本轮不碰任何尺寸，但牌上会多一个「机器名」前缀 —— 走的是
   既有的 `default_label`，**不改窗口高度算法**。

---

## 1. 一个决定性的取舍：拉，而不是推

两种做法：

| | 推（peer → hub） | **拉（hub → peer）** |
|---|---|---|
| 对端要改什么 | 要新版本、要配 hub 地址 | **不用改，0.4.x 就能当 peer** |
| 谁要开入站 | hub | peer（本来就为「局域网访问」开过） |
| 失败语义 | 要自己做心跳超时 | **TTL 过期，机制现成** |
| 新增协议 | 要一个批量上报端点 | **复用 `GET /v1/state`** |

**选拉。** 决定性理由是第一条：拉让这个功能**立刻对已发布的所有版本生效**。用户升级
一台机器就能用，另一台不用动。推的话，两台都得升到同一个新版本，而这个项目的历史
告诉我们「让用户把两台机器都升到同一个版本」是个真实的摩擦。

代价写在这里，别假装没有：

- 对端必须开「允许局域网访问」，且对端要能从 hub 路由到（不同网段 / NAT 后面不行）。
- hub 每 2 秒对每台机器发一个请求。5 台机器 = 2.5 req/s，都是本地几 KB 的响应。
- **hub 拿到的是对端的完整看板**：组名、会话名、目录、消息。这是同一份数据在对端自己
  的只读面板上本来就有的东西，但跨机器传了，所以文档里要写明。

---

## 2. 合并策略：远程会话就是普通会话

**核心决定：远程会话不另立一套数据结构，直接喂进现有的 `Store`。**

理由：下游有七个消费者（灯、收件箱、托盘、通知、统计、物理灯、只读面板），它们全都
只认 `Snapshot`。另立一套就要在这七处各写一次「本地 + 远程」的合并，而那七份合并逻辑
迟早会漂移 —— 这正是这个仓库反复在防的那类 bug。

喂进去之后，**下面这些全部零改动自动生效**：

- 灯上一块牌、收件箱一行、托盘一个颜色
- `muted_groups` / `hidden_groups`（用户可以把同事的机器静音）
- 30 天统计、时间线
- WLED 物理灯（它只取全局聚合色）
- 只读 Web 面板

代价：远程会话会进统计、会触发通知。前者是对的（hub 的统计本来就该是全局的），后者
由既有的「静音某个组」兜住 —— 不为它加第八个开关。

### 2.1 命名空间

| 项 | 规则 | 例 |
|---|---|---|
| 机器名 | 用户填的 `id`，**必须唯一、非空** | `mac-mini` |
| 会话 id | `m:{机器}:{对端会话 id}` | `m:mac-mini:a3f9…` |
| 组名 | `{机器}/{对端组名}` | `mac-mini/ci` |
| 健康会话 | `m:{机器}:__hub__`，组名 `{机器}` | 见 §2.4 |

`m:` 前缀同时干三件事，这也是选它的原因：

1. **循环防护**：轮询时**跳过**对端快照里 id 以 `m:` 开头的会话。hub A 拉 hub B、B 又
   拉了 A 时，不会无限套娃（B 里那份「A 的远程会话」被 A 丢掉）。
2. **UI 判据**：收件箱据此把「跳转到终端」关掉 —— 那个路径在本机上不存在，跳过去只会
   打开一个不存在的目录。
3. 不占组名空间：组名里不会出现 `m:`，用户自己的组叫什么都不受影响。

### 2.2 状态映射

对端快照里的 `state` 是**已经聚合过的**，可能带派生状态。规则：

| 对端状态 | 处理 |
|---|---|
| `offline` | **跳过**（对端没有会话，不该在我这儿造一个） |
| `stale` | 映射成 `idle` |
| 其余 | 原样 |

`stale` → `idle` 的理由：`stale` 的含义是「它沉默超过自己的 TTL 了」，而这件事在本机
由**本机自己的 TTL** 重新判定（见 §2.3）。原样转发会让远程会话一进来就是灰的，而且
它的 `stale_progress` 在本机没有意义（那是用对端的时钟算的）。

### 2.3 TTL：让对端失联自动降级

每个远程会话按 `ttl = max(3 × 轮询间隔, 6 秒)` 上报。于是：

- 对端正常 → 每 2 秒刷新一次，永远不过期。
- 对端掉线 → 最多 6 秒后过期，转 `stale`，再过 `STALE_GRACE` 才消失。
- 对端某台 agent 关掉 → 对端快照里没有它了，本机同样在 6 秒后转 `stale`。

**不主动清理**掉线的机器：一次丢包就清空会让牌闪一下。让 TTL 接管，行为单调、可预测。

### 2.4 机器掉线要看得见

连续 3 次失败（约 6 秒）后，注入一个健康会话：

```
id:    m:{机器}:__hub__
group: {机器}
state: error
message: 连不上 · {短原因}
```

恢复后立刻用 `state: off` 清掉它。

**为什么值得单独造一条**：机器挂了和机器上没有 agent，在灯上是**同一个样子**（没有
牌）。而「我的服务器连不上了」恰恰是多机场景里最需要知道的一条。用 `error` 而不是
`attention`：它是真的出错了，且用户明确开了这台机器。

### 2.5 牌上的显示名

`GroupView.default_label` 目前只给「按会话分组」的组用。本轮扩一条：

> 组内**所有**会话都带 `machine` 时，`default_label = "{机器} · {组名去掉机器前缀后的部分}"`，
> 前缀为空时就是机器名。

这样灯上读作 `mac-mini · ci`，而不是 `mac-mini/ci`。**仍然只是默认值**：用户在
「外观 → 各组的显示」里改的名字照旧优先（`lampSort.ts` 的 `labelOf`）。

---

## 3. 数据模型

```rust
// config.rs
pub struct Machine {
    /// 显示名 + 命名空间。**改了等于换了一台机器**（旧会话按新名字重新进来）。
    pub id: String,
    /// 对端地址。`192.168.1.20:41320` / `http://host:port` 都认。
    pub url: String,
    /// 对端的 token（`~/.signalhub/token`）。
    pub token: String,
    pub enabled: bool,
}
```

`Settings` 新增：

- `machines: Vec<Machine>` —— 默认空。
- `machine_poll_ms: u64` —— 默认 2000，夹在 1000–10000。

`Report` 新增 `#[serde(skip)] machine: Option<String>`：

> **用 `serde(skip)` 而不是普通字段**：这样 HTTP 上报**无法**声称自己来自某台机器
> （字段被忽略），而 Rust 侧（轮询器）可以设置它。接口契约一个字都不用改，`docs/api.md`
> 也不用为它加一节。

`Session` / `SessionView` 新增 `machine: Option<String>`，给 UI 判据和牌面标签用。

---

## 4. 改动清单

### 4.1 `src-tauri/src/config.rs`

- `Machine` 结构 + `Settings.machines` / `machine_poll_ms`。
- `clamped()`：
  - `id` / `url` / `token` 去空白；`url` **空着保留**（「刚点了添加还没填」这个中间态，
    和 `devices` 同一个脾气），`id` 空则整条丢掉（没有名字就没法做命名空间）。
  - 按 `id` 去重，先出现的赢。
  - 上限 20 台（`MAX_MACHINES`）。
  - `machine_poll_ms` 夹到 1000–10000。
- 单测：默认空 → 快照与 0.4.1 一致（红线 2）；去重；空 id 丢弃；轮询间隔夹紧。

### 4.2 `src-tauri/src/state.rs`

- `Report.machine`（`serde(skip)`）、`Session.machine`、`SessionView.machine`。
- `apply` 拆成 `apply_inner(now, req, ttl, machine)`，`apply` 转调（`None`）。
- `Store::clear_machine(&mut self, machine: &str, now: Instant) -> bool` —— 机器被删 /
  关掉时清干净，避免「配置里没了，灯上还挂着」。
- `Store::remote_machines(&self) -> Vec<String>` —— 给轮询器算差集。
- `session_label` 前面加一层 `remote_label`（§2.5）。
- 单测：命名空间不串组；`clear_machine` 只清那台；远程组拿到 `default_label`。

### 4.3 `src-tauri/src/remote.rs`（新）

```rust
pub struct Health { id, ok, last_ok_ms, latency_ms, error, sessions }
pub struct Registry { /* Arc<Mutex<BTreeMap<String, Health>>> */ }

pub fn spawn(store: SharedStore, settings: SharedSettings,
             machines: SharedMachines, notify: Notifier);
```

- 一条 `std::thread`，每 `machine_poll_ms` 醒一次（和 `sweep` / `collect` 同一个模式，
  不起 async 任务：`ureq` 是阻塞的，扔进 tokio 只会占着一个 worker）。
- 每台机器一次 `GET {url}/v1/state`，`Authorization: Bearer {token}`，
  `timeout_connect 2s` / 每调用 3s（比轮询间隔略大，别让一台慢机器拖住整轮）。
- 解析用一个**全字段可选**的 `PeerSnapshot`（`#[serde(default)]`）：对端版本比我们新 /
  旧都不能解析失败。
- 收完之后**一次性** `notify(snapshot)`，不是每个会话推一次。
- 纯函数（可单测）：`endpoint()`（补 scheme、去尾斜杠、去掉误粘的 `/v1/state`）、
  `session_id()`、`group_name()`、`state_of()`（§2.2）。

### 4.4 `src-tauri/src/lib.rs`

- `SharedMachines` 注册到 Builder 链。
- `setup` 里 `remote::spawn(...)`，紧挨着 `server::spawn`。
- 命令：`list_machines`（健康快照）、`probe_machine { url, token }`（点「测试」用，
  单次拉取，不落盘）。

### 4.5 前端

- `useSettings.ts`：`Machine` 类型、`Settings.machines` / `machine_poll_ms`、`DEFAULTS`。
- `types.ts`：`SessionView.machine?: string | null`。
- `SettingsPanel.tsx`：新增页签 **多机**（`railConnect` 组里，排在「接入」之后）。
  - 每台一行：名称 / 地址 / token / 开关 / 状态（延迟、最近成功、错误原因）。
  - 「测试连接」就地显示结果 —— 和 `try_agent` 同一个交互。
  - 底部一句：对端要开「允许局域网访问」，token 在设置 → 接入里。
- `InboxWindow.tsx`：远程行带机器徽章；**禁用跳转**（`m:` 前缀）。
- `i18n.ts`：新增页签那一组文案（中英各一套）。

### 4.6 文档

- `docs/api.md` 新增 §「多机聚合」：拉取协议（就是 `GET /v1/state`）、命名空间规则、
  循环防护、隐私边界。
- `docs/architecture.md` 新增一节：为什么选拉、为什么喂进 Store。
- `README.md` 功能列表加一行。

---

## 5. 验收

必须全绿才算完：

1. `machines: []` 时，`cargo test` 里既有的快照测试**一条都不用改**（红线 2）。
2. `cargo clippy --all-targets -- -D warnings` 在 macOS 上干净 —— 且 `remote.rs` 里
   没有平台专用项（这一轮不涉及平台差异，CI 三平台应该同绿）。
3. `npm test` 全绿。
4. 手工：本机开两个实例不行（单实例锁），所以用 CLI 模拟对端 —— 起一个
   `signalhub report --group ci --state permission`，另一台机器（或本机改端口）当 hub，
   灯上出现 `本机名 · ci`，收件箱里那一行**不能跳转**。
5. 把对端进程杀掉 → 6 秒后转 `stale`，再 3 次失败后出现 `连不上` 那一块牌。
6. 把机器从设置里删掉 → 牌立刻消失（`clear_machine`）。

---

## 6. 不做的事（写清楚，免得以为漏了）

- **不做远程跳转**（SSH 过去开终端）：那是另一个量级的产品，且需要知道对端的窗口系统。
- **不做远程审批**：`approvals` 登记表是进程内状态，跨机器转发它要先解决「谁有权点」，
  这一轮不碰。收件箱里远程行只读。
- **不做中继 / 公网穿透**：只在能直连的局域网 / VPN 内工作。文档里明说。
- **不做对端身份认证**：只有 token。对端信任边界没变 —— 它本来就只认 token。
