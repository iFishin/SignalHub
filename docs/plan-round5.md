# 0.4：收件箱 + 跳转（第一块）

> 2026-10-28 起草并落地。方向不是「再加一个状态源」，而是把 SignalHub 从**被动显示器**
> 变成智能体的**注意力控制台**：发现谁需要你 → 当场处理 → 回到工作时给摘要。
> 这一版先做前两步。

## 为什么

灯解决「抬头看一眼」，但真正的成本在下一步：找到是哪个窗口、点允许、切过去。只在灯上
显示状态，价值只存在于你不在终端前的那几分钟；而 `permission` / `error` / `done` 的
处理动作全在别处。收件箱把这两步接起来，灯仍然是氛围层。

## 已落地

- **收件箱窗口**（`inbox`，420×560，深色 HUD、无边框、贴灯左侧、失焦即收起）。
  - 四段：需要你（审批 / permission / attention / error）→ 运行中 → 刚刚完成 → 空闲 / 沉默。
  - 一个会话最多一行；同时有审批时合并，行内直接允许 / 拒绝（走 `Registry::decide`，
    「看不全不给放行」仍在 Rust 里挡着）。
  - 每行可跳转、静音该组、清除该会话；`A` / `D` / `↑↓` / `Enter` / `Esc` 键盘操作。
  - 数据只来自现有快照 + 审批登记表；不轮询、不新增 IPC 通道。
- **跳转到终端**（`jump.rs`，macOS 优先）：
  - pid → `ps -o tty=` → tmux pane（`list-panes` 按 pane tty 匹配，选中后按 client tty
    找到外层终端标签页）或 Terminal.app / iTerm2 的标签页（AppleScript 按 tty 精确匹配）。
  - 认不出精确标签页时按父进程链激活终端 / IDE；再不行才在 Finder 打开项目目录。
  - 每层都在返回值里说清做到了哪一步；不猜、不假装。argv 传参、不过 shell；tty 只放行
    安全字符。
- **入口与交互**：灯左键单击打开收件箱，双击折叠 / 展开（单击等 280ms 的双击窗口，
  拖拽优先并取消待执行单击）；`⌃⌥I`（`Ctrl+Alt+I`）；托盘菜单第一项；第二次启动应用。
- **托盘**：菜单加「收件箱…」，与设置、折叠、暂停并列。

## 验证

- 单测：`inboxRows.ts`（分段 / 排序 / 去重 / 时间格式，99 个前端测试）、`inbox.rs`
  （窗口定位，含负坐标工作区 / 超大窗口）、`jump.rs`（tty 校验 / tmux 解析 / 已知应用名）。
- 实机：dev 版第二实例打开收件箱，`CGWindowListCopyWindowInfo` 看到 420×560 的窗口贴在
  灯旁边；用 headless Chrome 跑了空态和假数据两版 UI（分段、审批详情、按钮、键盘提示）。
- 门禁：`cargo clippy --all-targets -- -D warnings` / `cargo test` / `npm test` / `npm run build`。

## 限制 / 没验证到的

- 精确跳转只有 macOS；Windows / Linux 先打开项目目录。IDE 内置终端没有公开的标签页定位
  接口，只能激活应用。
- 第一次跳转 Terminal / iTerm2 会请求「控制终端」的自动化权限；ad-hoc 签名更新后可能要
  重给。若被拒绝，界面会明说，不会退回「假装跳了」。
- 没有在真实的多 tab / tmux / VS Code 场景逐一点过（需要人工）；解析层有单测，端到端要
  真机确认。
- 没有做「离开回来后的摘要」和手机审批 —— 这是下一块（见下）。

## 下一步（按价值排序）

1. **While you were away**：离开 >10 分钟回来给结构化摘要（完成 / 失败 / 等审批 / git diff
   摘要 / 等了多久）；数据已在 history / timeline / stats 里。
2. **Away approvals**：把只读 Web 面板升级成带 token 的 PWA，配合 outbound 的手机推送，
   能看命令并允许 / 拒绝。
3. **团队 / 多机 hub**：一台机器做 hub，团队所有 agent 的状态汇到一块看板 / 办公室物理灯。
4. 再往后才是更多 agent 适配器和主题 —— 那些不解决需求性。

## 发布记录

- **v0.4.0**（2026-10-28）：tag `v0.4.0` → `21ad2d5`；Release workflow `40712336996`
  四平台全绿（macOS aarch64 / x64、Windows、Linux），产物含 dmg、Windows setup +
  portable、AppImage / deb 和 `latest.json`。**没有签名**，macOS 首次打开仍需清隔离标记。
- 没有单独发 0.3.4：本来留给下一个补丁的两处修复（长任务保持工作态、低透明度描边）
  直接进了 0.4.0。
- 仍留在 issue 里：Windows 探活 #20、真 app 视觉 / 采集器验收 #21、CSC 真机 #22、
  无进程号接入的长任务 #23。
