### 本次更新（0.3.3）

**接入页现在能看出「装的是旧版本」。** 以前升级之后，界面上「已接入」和「装的是老版本
写的 hook / 插件」长得一模一样 —— 功能还在，行为却停在上一版（典型：进程号没跟着传，
灯过一会儿又回到「沉默」）。这一版给两种接入都补上了判据：

- **命令式**（Claude Code / CSC / Codex / Cursor / Gemini / Goose）：配置里那条命令
  和当前版本会写的不一致 → 那一行提示「接入是旧版本写的」，并给一个「重新接入」按钮
  （就地改写，不用先卸载）。
- **插件式**（opencode / Pi / Hermes）：源码头部带 `signalhub-plugin-format: N`，
  旧文件没有这一行 → 同样提示。这个版本号只在模板内容变化时抬，不跟 app 版本走。
- 比较只对程序名做 canonicalize（裸名、绝对路径、软链指向同一个文件都算同一条），
  所以自己调过 PATH 不会被误报。只提示、**不自动改**用户配置。
- `signalhub diagnose` 也会打印「接入版本 旧」。

**新加 CSC（CoStrict CLI）接入。** `signalhub install hooks csc`（`costrict` 也认）
会写 `~/.costrict/settings.json`，条目形状、事件表和 Claude Code 同构；上报的 agent
名是 `csc`，灯上能和 Claude Code 分开归因。和 Claude Code 一样需要重启一次 CSC。

**灯上右键能给每一组手动设状态**：执行中 / 推理中 / 已完成 / 需要关注 / 出错 / 空闲。
点哪盏灯就设哪一组，停留 12 小时（撑得过一天、撑不过夜）；收掉用菜单里现有的
「清除该组的状态」。它走的是普通上报路径，组里要是有更紧急的智能体会话（比如正在等
授权），仍然按原有优先级显示。「隐藏该组」「清除该组的状态」照旧。

**安静模式下字也会反色。** 安静模式把没事的牌压到几乎透明，但以前只有把「牌面透明度」
滑杆拖到一半以下才会翻字 —— 现在被安静压暗时同样翻成深色，浅色桌面上也读得出来；
把鼠标移上去、牌面恢复不透明时，字也跟着收回来。

**修掉的问题**：

- 安静模式压暗后字色不反转，浅色桌面上几乎不可读。
- （内部）程序名比较没有解开软链，会把「绝对路径」和「PATH 裸名」误判成两条不同的
  命令 —— 接入页会误报「有更新」。

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
