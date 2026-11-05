### 本次更新（0.4.4）

**第一次打开，先看见它在干什么。** 这一版没有新增状态、没有动状态机，做的是两件
更基础的事：让新来的人在 30 秒内看懂这盏灯，让用了一阵的人回来知道今天发生过什么。

- **演示模式。** 设置 →「演示」页里有一块预览，用的是**和真实灯牌同一个组件**，
  跑一份纯前端的剧本：开工 → 等审批 → 失败 → 收工，一轮一轮走。它不连后端、
  不写状态、不碰你的真实会话 —— 所以还没接任何智能体时，也能先看清这盏灯长什么样。
- **灯窗上的第一句话。** 第一次启动时灯下面有一行「第一次用？看演示 ×」，点它直接
  开设置里的演示页；看过一次就不再出现。
- **设置面板好找了一点。** 左栏加了页签搜索（输 `通知` / `hook` / `审批` 直接跳），
  每页右侧多一个小节目录，记住上次停在哪个页签，切页有过渡动画，键盘 Tab 有焦点环。
  十几个页签的面板，找东西不该靠翻。

**通知到底通没通，按一下就知道。** 设置 →「提醒」页多了「发一条测试通知」：它走
**真实的通知出口**（和「任务完成」那条完全同一条路，提示音按你的设置来），发完把
权限状态回给你 —— 已允许 / 被拒绝 / 还没问过。以前「没弹」只能靠猜：是没权限、
是勿扰、还是根本没触发？现在点一下，答案就在按钮下面。它不改任何状态、不写统计、
不触发告警闸门。

**发版前自检不用翻五个页签了。** 设置 →「系统」页顶部的「系统体检」一次跑完：
通知权限、通知 / 提示音 / 延迟 / 勿扰 / 静音组、接入命令还有效吗、装完之后
`signalhub` 命令被删了没有、上报回执是不是 15 分钟内的、`approve_tools` 和
Claude Code 的 HTTP 接入配没配上、自定义预设有没有写错、快捷键注册成功没有、
hook 日志有没有异常。结论分「通过 / 注意 / 有问题」，能一键复制成 Markdown 贴进 issue。

**回来之后知道今天发生过什么。** 设置 →「历史」页顶部一张「今日复盘」：今天总共
亮了多久、哪一组最久、真正在跑多久、等你审批多久、审批几次（允许 / 拒绝 / 超时，
最长等了多久）、和之前几天的日均比是多了还是少了。数据全部来自已有的统计和审批
历史，**不新增任何落盘格式**。它也说清自己看不到什么：没有事件流水，所以「出现过
又消失的会话」不在里面；空闲和离线时间计入总时长。

**「从灯上审批」只支持 Claude Code（HTTP 接入）—— 这条现在写在界面上了。**
WorkBuddy / Codex / Cursor 等可以正常上报状态、正常亮灯，但审批气泡和「允许 / 拒绝」
只对 Claude Code 的 HTTP 接入生效。WorkBuddy 目前不发 `PermissionRequest`，它报的是
`Notification` 的 `permission_prompt` —— 我们只能据此把灯点红，没有可以回传决定的
通道。**这不是开关没打开，是那条路还不存在**，所以不做成看起来能用、按下去没反应。

**详情见 [README](https://github.com/iFishin/SignalHub#readme) 与各文档：**
`docs/plan-round5.md`（本轮清单）、`docs/api.md` §4.17（审批适用范围）。

### 安装

**macOS** —— 下载 `.dmg`，把 SignalHub 拖进「应用程序」。
Apple Silicon 选 `aarch64` 那个，Intel 选 `x64`。

这一版**没有签名**，所以第一次打开会被 Gatekeeper 拦住。**推荐**在终端里跑：

```
xattr -dr com.apple.quarantine /Applications/SignalHub.app
```

它递归清掉整个包的隔离标记 —— 应用和打包在里面的 `signalhub` 命令都覆盖到。
（右键点图标 →「打开」→ 再点一次也可以，只是不保证覆盖到里面那个命令。）

**macOS 版从 0.2.4 起支持应用内更新**：设置 →「系统」页里会显示有没有新版本，
「下载并安装」**点了才会装**（默认**不**自动装），装完自动重启。验的是我们自己那对更新
密钥（minisign），和 Apple 证书是两回事 —— 所以没签名也能用。万一它没成功，
手动下载照旧可用，不会把应用弄坏。

**Linux 和 Windows 这一版还只能手动下载**：Windows 那边要用的解包库被上游全量撤下了
（安全和依赖解析都过不去），Linux 侧是还没在真机上验收过。

**Windows** —— 两种拿法：

- `-setup.exe`（NSIS 安装器）—— 装进 Program Files，带开始菜单和卸载项。
- `_x64_portable.zip` —— **免安装**：解压到任意目录，双击里面的
  `signalhub-app.exe` 就跑。不写注册表、不建快捷方式。

两种都**没有签名**，第一次运行会被 SmartScreen 拦一道：点「更多信息」→「仍要运行」。

portable 版解压出来是**两个文件**，两个都要留着：`signalhub-app.exe` 是界面，
`signalhub.exe` 是命令行，应用靠它做「一键接入」。（界面那个不能叫
`SignalHub.exe` —— Windows 文件名不区分大小写，会和命令行那个撞成同一个文件。）

设置和状态仍然存在 `%USERPROFILE%\.signalhub`，所以它「免安装」但不「随身带配置」。

**Linux（beta）** —— 两种拿法：

- `.AppImage` —— 加个执行位直接跑：`chmod +x SignalHub_*_amd64.AppImage`。
- `.deb` —— `sudo dpkg -i SignalHub_*_amd64.deb`（缺依赖就 `sudo apt-get -f install`）。
  WebView 本体和托盘库都在依赖里，不用自己找。

**这一版请当成 beta 用**：没在真机上长期跑过，已知两点：

- 透明窗口和「始终置顶」在 **Wayland** 下不一定生效 —— 那是合成器的限制，不是能改
  的开关。真碰上就用 `GDK_BACKEND=x11` 启动（走 XWayland）：
  `GDK_BACKEND=x11 SignalHub_*_amd64.AppImage`。
- 在没有 FUSE 的环境里（容器、部分精简发行版），AppImage 要这样跑：
  `./SignalHub_*_amd64.AppImage --appimage-extract-and-run`。

`signalhub` 命令同样**已经打包在里面**（「设置 → 接入」页的一键接入会自己用它），
所以不用另外装：

- deb 装完就在 `/usr/bin/signalhub`。
- AppImage 是解开的目录里那个（`./SignalHub_*_amd64.AppImage --appimage-extract`，
  和 deb 同一个布局：`usr/bin/signalhub`）。

想直接在终端里用，把它链到 PATH 上：

```bash
sudo ln -sfn /usr/bin/signalhub /usr/local/bin/signalhub   # deb
```

### 命令行

`signalhub` 这个命令**已经打包在应用里了**，不用另外装 —— 设置里的「接入」页
一键接入时会自动用它。想在终端里直接用，把它加到 PATH：

macOS：

```
ln -s /Applications/SignalHub.app/Contents/MacOS/signalhub /usr/local/bin/signalhub
```

Windows（管理员 PowerShell，路径按实际安装位置改）：

```
New-Item -ItemType SymbolicLink -Path "$env:LOCALAPPDATA\Microsoft\WindowsApps\signalhub.exe" -Target "C:\Program Files\SignalHub\signalhub.exe"
```
