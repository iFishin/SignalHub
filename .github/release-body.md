### 本次更新（0.4.0）

**收件箱：从「抬头看一眼」到「当场处理」。** 以前灯只告诉你「有事」，真正要花时间的是
下一步 —— 找到是哪个窗口、点允许、切过去。这一版给了一个贴着灯出现的小窗口：

- 左键点灯、`⌃⌥I`、托盘菜单第一项，或再启动一次应用，都会打开收件箱。
- 按「需要你 → 运行中 → 刚刚完成 → 空闲 / 沉默」列出所有会话；一个会话最多一行。
- 待审批的条目可以**当场允许 / 拒绝**（「看不全不给放行」的规则不变）；每行都能
  **跳转**到对应终端、静音该组、清除该会话。
- 键盘：`↑↓` 选择、`Enter` 跳转、`A` 允许、`D` 拒绝、`Esc` 收起。
- 审批的命令详情只进这个本机窗口，不进 `/v1/state`、通知和出站钩子。

**跳到会话所在的终端（macOS 优先）。** 会话的进程号 → tty → tmux pane，或
Terminal.app / iTerm2 的标签页，直接切到那个窗口。认不出精确标签页时按父进程链
激活终端 / IDE；再不行才在 Finder 里打开项目目录 —— 每一层都会在界面上说清做到了
哪一步，不会假装跳成功。第一次跳 Terminal / iTerm2 会请求「控制终端」的自动化权限。

**长任务不会再「跑几分钟就沉默」。** 带进程号的会话（Claude Code 命令式 hook、
opencode / Pi / Hermes 插件、`signalhub run`）在 TTL 到期时只续期、保持最后的状态：
20 分钟的构建、十分钟的推理期间一个事件都没有，灯仍然显示「执行中 / 思考中」。
结束由智能体自己的结束事件决定，进程没了立刻收掉。没有进程号的接入仍按 TTL 走
（见下面的已知限制）。

**低透明度下的字改成「浅字 + 描边」。** 牌面淡过一半（包括安静模式压暗）时，字保持
浅色、四周补一圈深色描边：深色窗口上靠浅字读，浅色桌面上靠描边勾出字形。描边跟着
字号缩放，紧凑预设也不会糊。悬停只恢复被安静压暗的牌，不再把字色和描边一起清掉。

**交互变化**：左键点灯现在是打开收件箱，**双击**才在「一叠牌」和「一颗灯」之间切换；
单击会等 280ms 的双击窗口，拖拽优先。设置 →「系统」页现在列着五组全局快捷键。

**升级提示**：如果「接入」页提示某个接入是旧版本写的，点一次「重新接入」即可。旧
opencode 插件没有格式版本行，会显示「有更新」。

**已知限制**：

- 精确跳转终端目前只有 macOS；Windows / Linux 先做到「打开项目目录」。IDE 内置终端
  没有公开的标签页定位接口，只能激活编辑器。
- app 是 ad-hoc 签名，更新后「控制终端」的自动化授权可能需要重给 —— 和没有 Apple
  开发者证书是同一个根因。
- Codex / Cursor / Gemini / Goose / CSC 的 hook 还不带进程号，长任务仍会在 TTL 后转
  `stale`（issue #23）；Windows 探活（#20）、真 app 视觉 / 采集器验收（#21）、CSC 真机
  验证（#22）也仍未完成。

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
