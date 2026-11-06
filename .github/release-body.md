### 本次更新（0.5.1）

**补丁版**：把 0.5.0 之后攒下的几处「用起来别扭」收掉，顺便把门口的路牌立好。
没有新增信号状态、没有动状态机、没有改任何落盘格式（`agents.json` 只是多了一条安全的
写入路径），所以按语义化版本走 patch。**升级不需要改配置，也不用重装接入。**

**列表外的智能体，现在有话可以照抄了。** 以前「接入」页只列得出我们写死的那几种预设，
碰到没预设的智能体，用户得自己翻 `docs/api.md` 才知道要往 `agents.json` 里写什么。
现在设置 →「接入」→「自定义智能体」和「指南」页都有「**复制接入提示词**」：把那段话
粘给正在用的智能体，它会自己探测本机配置、问清接入方式，然后登记自己。提示词里的预设
清单、上报地址、端口都是运行时从应用里取的真值，不是一份会过期的硬编码；也不经手 token。

**让智能体写文件，但不让它直接写文件。** 外部终端进程够不到 webview 的 IPC，所以
`signalhub add-agent` 同时给了两条入口：CLI 给智能体，IPC 命令加设置面板里那张默认收起的
表单给人。两条都走 Rust 侧同一个 `agents::add`：先校验、再原子写落盘；`agents.json`
已经是坏 JSON 就拒绝覆盖，重复登记幂等，撞了同 id 要显式 `--replace`，config 指向数据目录
直接拒绝，文件里其它未知键和 `_example` 原样保留。比让脚本自己去改一个 JSON 稳得多。

**折叠角标不再挤走灯珠。** 开了「点击穿透」之后，那个 ⇥ 箭头会占掉展开态的一段内容宽度，
折叠时甚至把灯珠顶出窗口。现在角标统一绝对定位、彻底不参与布局，窗口几何也不再替它预留
位置；随后又把它从窗口角外边挪进**顶部留白的正中**，尺寸仍是原来的 9px。
`src-tauri/src/config.rs` 与 `src/styles/lamp.css` 那两处几何真值同步改过，注释也跟上了。

**设置左栏的选中态去掉了那道圆角竖条。** 现在选中就是浅灰底 + 正文色 + 500 字重、方角，
和面板里其它单选控件说同一种话；「扁平、无阴影无渐变、只靠 1px 细线分隔」这套原则也写进了
`src/styles/settings.css` 文件头，免得下次又长出一个新样式。

**顺手补齐的几件：**

- 新增 [docs/backlog.md](https://github.com/iFishin/SignalHub/blob/main/docs/backlog.md)，
  把散在各份计划文档里的待办收成「待实现 / 待验收 / 平台限制 / 刻意不做」四类，顺手修掉
  几处互相矛盾的旧描述。README 中英两版的「待做」都指向它。
- README 顶部换成图标 + 一句话定位，下面是一张操作示例动图（`idle → thinking → working →
  permission → working → done`，授权那段含按住 800ms）和信号流示意图。动图是程序化绘制的
  高保真示意，**不是屏幕录制**。
- Windows 上「命令式 hook 拿不到进程号」这半个老问题补上了：CLI 借 ToolHelp 快照沿父进程
  链找可信祖先，跳过 `cmd.exe` / PowerShell 这类短命 shell，找不到就回落原来的 30 分钟
  兜底。**已装的 hook 命令文本没动**，不会触发 `command_stale` 要求重装；这一条仍待
  Windows 真机验收。

**详情见 [README](https://github.com/iFishin/SignalHub#readme)、
[docs/backlog.md](https://github.com/iFishin/SignalHub/blob/main/docs/backlog.md)
与 `docs/plan-round5.md` 的发布记录。**

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
