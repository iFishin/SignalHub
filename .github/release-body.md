### 本次更新（0.3.0）

从「AI 智能体信号灯」变成**桌面信号中枢**。这一版加了七样东西，最值得说的是第一样。

**从灯上审批（默认关）** —— 设置 →「接入」→ 从灯上审批。打开之后，Claude Code 每次
要问你批准某个工具调用时，那个确认框会被搬到灯上：灯上弹一条气泡，点「允许」或
「拒绝」，或者不管它（45 秒后交回终端里那个确认框）。

这是**全项目唯一会改变智能体行为的功能**，所以形状刻意收窄：只在「它本来就要问你」的
那一刻生效，不拦别的工具调用；**永远不替你说「允许」**（超时、被顶掉、开关关掉、绑着
局域网，一律什么都不说，回到终端里问）；**看不全就不给放行**（命令超过 240 字符算截断，
那条只能拒绝或者回终端自己看）；命令全文只出现在本机那个小窗口里，状态上报仍然只有
「执行 npm」那个短词。

**手动状态 / 番茄钟** —— 托盘菜单 →「手动状态」：「专注 25 分钟」（灯上出现倒计时，
每分钟走一格，到点提醒并转「已完成」）、「开会中」、「离开」、「清除手动状态」。
它们就是普通的一组分，所以统计、历史、只读面板、转发钩子、物理灯全都自动认它。

**全局快捷键** —— `⌃⌥L` 显示 / 隐藏灯、`⌃⌥M` 静音最差那一组、`⌃⌥0` 清除最差那一组
（Windows / Linux 上是 `Ctrl+Alt+L / M / 0`）。抢不到的那一个会在设置里标出来。

**物理灯** —— 「转发」页最下面填一台 WLED 的地址，状态色就直接发到它上面。发出去的
**只有颜色**：没有组名、没有会话名、没有消息内容。默认一台都不配。

**只读 Web 面板** —— 手机上打开一条带 token 的链接就能看灯（状态、每组在做什么、已
持续多久），不用装任何东西。默认关，要配合「允许局域网访问」才看得到。

**接入这件事本身** —— 「接入」页现在会告诉你每个信号源**最近一次上报是多久前**、可以
「试一下」真的调一次、有一键诊断可复制文本；Claude Code 还可以改用 **HTTP 式接入**
（不需要 `signalhub` 命令在 PATH 上）；信号源按 **AI 智能体 / 系统 / 自定义** 分组。

其余是内部整理（事件合帧，同样的状态不重复推）。

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
