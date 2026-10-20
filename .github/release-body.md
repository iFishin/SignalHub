### 本次更新（0.3.2）

**灯能分清「没上报」和「没了」。** 以前智能体窗口还开着、只是没有新事件，过一会儿
牌上会写「会话已沉默」，半小时后干脆变成「没有智能体在上报」—— 把「还开着」说成
了「没了」。这一版让内置接入带上自己的进程号：

- 进程还活着、只是没上报 → 牌上转「空闲」，不再是「沉默」；
- 进程被关掉 / 杀掉 → 牌子**立刻**消失，不用等 TTL；
- 只有 thinking / working / idle 会这样转；授权、出错这类「有人在等」仍然照旧，
  进程活着不代表你已经批了。

**需要重装一次接入**：老版本写进配置的 hook / 插件没有进程号。升级后到「接入」页
把 Claude Code（命令式）、opencode / Pi / Hermes 重新点一次接入，再重启对应的智能体。
进程号走环境变量 `SIGNALHUB_PID`，所以 PATH 上即使是旧版 CLI 也不会因此报错、上报
不会断。Windows 上暂时没有探活，行为和以前一样。

**会议中（新采集器，默认关）** —— 麦克风或摄像头被某个进程占用时，自动报一个
「会议」组。只看设备在不在用，不打开流、不采集内容，也不会弹隐私授权。只有 macOS
有，和 Time Machine「备份中」那条一样每 5 秒看一次。

**三个写死的预设（默认全关）**，在「提醒」页各一个开关：

- 授权卡住 60 秒 → 用「转发」里已经配好的钩子再推一次手机（没配钩子就没有东西可发）；
- 有智能体在思考 / 执行 → 暂停系统通知和重复提醒（灯照常显示，忙完也不补发）；
- 会议中 → 「会议」那一组变蓝，并暂停通知（需要上面的采集器）。

**修掉的问题**：

- 0.3.1 的自定义请求体里，替换进去的值可能被后面的占位符**二次替换**（消息里恰好
  有 `{group}` 就会被换掉）—— 改成单遍扫描。
- 采集器一次查询失败会被报成「备份完成 / 会议结束」——「不知道」不等于「结束了」，
  现在什么都不报，让上一条自己过期。
- 会议判据补上信号源：用户自己恰好叫 `meeting` 的会话不会再把灯变蓝、把通知静音。
- `pid: 0` 不再被当成进程号（它在 `kill` 里指的是当前进程组）。

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
