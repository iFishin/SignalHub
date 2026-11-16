### 本次更新（0.6.9）

**灯牌上限现在可以自己定，默认从 4 抬到 6。** 「外观 → 基础 → 灯牌上限」给 4/5/6/7/8 五档，
同时开五六路会话时不必再折起来看；屏幕小想省地方就调回 4，想一眼看全就到 8。上限不再是写死
在代码里的常量，窗口高度、折叠点数、`+N` 的出现时机都跟着它走，拨完不会有牌被裁掉、也不会
在底下空一条。

**先说清一件事：把牌面顶满的不是子 agent。** 查了真实轨迹 —— 子 agent 用的是父会话的 id，
事件本来就归进父组，不会单独占一块牌。真正占满的是「五六个终端窗口 / 项目各开一路」这种独立
会话。所以这版没有去加一套按 agent id 归并的逻辑（那是无用功，还会误伤真想按会话分组的人），
而是把上限交到你手里。

**`+N` 汇总牌不再是一枚灰数字。** 折叠把「那堆里有人在等你」也一起藏了。现在角标带**被折起来
那堆里最紧急的状态色**：展开态染牌尾那颗小圆点，折叠态染圆点的 1px 描边。红黄绿一眼分得出，
牌宽高和窗口尺寸一处没动。

**折叠板里，名字空了的那行不再只剩一颗灯珠。** 有些上报方在会话沉默后不再带项目名和智能体名，
那一行会变成「左边一颗灯、右边一大片空」。现在它回退显示状态词 —— 沉默的会话正好显示
「沉默」，每行又是一个完整的「灯 + 词」单元。没有改布局、没有居中那颗灯（居中会让中间行和
上下行错开，更显眼），几何、窗口尺寸一处没动。

**升级须知：没有破坏性变化。** 没有新增信号状态、没动状态机，接入、留言、审批记录、历史统计
全都不受影响。旧配置缺 `max_signs` 这个键会自动读默认值 6。两处**观感**变化写在上面：默认
多显示两块牌（仍可调回 4），以及折叠板空名字行会多出一个状态词。

详见
[architecture.md](https://github.com/iFishin/SignalHub/blob/main/docs/architecture.md)
§3.35.19（灯牌上限可设置、`+N` 汇总牌）与 §3.35.16（空名字回退），以及
[README](https://github.com/iFishin/SignalHub#readme)。

### 安装

**macOS** —— 下载 `.dmg`，把 SignalHub 拖进「应用程序」。
Apple Silicon 选 `aarch64` 那个，Intel 选 `x64`。

这一版**没有签名**，所以第一次打开会被 Gatekeeper 拦住。**推荐**在终端里跑：

```
xattr -dr com.apple.quarantine /Applications/SignalHub.app
```

它递归清掉整个包的隔离标记 —— 应用和打包在里面的 `signalhub` 命令都覆盖到。
（右键点图标 →「打开」→ 再点一次也可以，只是不保证覆盖到里面那个命令。）

**macOS 版从 0.2.4 起支持应用内更新**：设置 →「系统 → 更新与诊断」页里会显示有没有新版本，
「下载并安装」**点了才会装**（默认**不**自动装），装完自动重启。验的是我们自己那对更新
密钥（minisign），和 Apple 证书是两回事 —— 所以没签名也能用。万一它没成功，
手动下载照旧可用，不会把应用弄坏。

**Linux 和 Windows 这一版还只能手动下载**：Windows 那边要用的解包库被上游全量撤下了
（安全和依赖解析都过不去），Linux 侧是还没在真机上验收过。

**Windows** —— 两种拿法：

- `-setup.exe`（NSIS 安装器）—— 装进 Program Files，带开始菜单和卸载项。
- `_x64_portable.zip` —— **免安装**：解压到任意目录，双击里面的
  `signalhub-app.exe` 就跑。不需要安装器、不需要管理员权限；**0.6.1 起**首次启动会为了
  Windows 原生通知在自己名下写一个 HKCU 键（`AppUserModelId` 下）和一个当前用户的
  开始菜单项 —— Windows 要求 Toast 必须有带应用身份的快捷方式，安装版由 NSIS 建，
  免安装版只能由应用补上（见 issue #25）。不做这一步，通知会被系统静默丢弃。
  **从 0.6.0 的 portable 包升级**：直接换用这一版的 zip 即可，旧 zip 里没有这个修复；
  也可以改用这一版的 `-setup.exe`。装完若通知仍不弹，先看「设置 → 通知 → 提醒」
  顶部的状态是不是「未注册」，再点「重新注册」，诊断报告末尾会带上具体错误。

两种都**没有签名**，第一次运行会被 SmartScreen 拦一道：点「更多信息」→「仍要运行」。

portable 版解压出来是**两个文件**，两个都要留着：`signalhub-app.exe` 是界面，
`signalhub.exe` 是命令行，应用靠它做「一键接入」。（界面那个不能叫
`SignalHub.exe` —— Windows 文件名不区分大小写，会和命令行那个撞成同一个文件。）

设置和状态仍然存在 `%USERPROFILE%\.signalhub`，所以它「免安装」但不「随身带配置」。

**Linux（beta）** —— 两种拿法：

- `.AppImage` —— 加个执行位直接跑：`chmod +x SignalHub_*_amd64.AppImage`。
- `.deb` —— `sudo dpkg -i SignalHub_*_amd64.deb`（缺依赖就 `sudo apt-get -f install`）。
  WebView 本体和托盘库都在依赖里，不用自己找。

**这一版请当成 beta 用**：没在真机上长期跑过，已知三点：

- 透明窗口和「始终置顶」在 **Wayland** 下不一定生效 —— 那是合成器的限制，不是能改
  的开关。真碰上就用 `GDK_BACKEND=x11` 启动（走 XWayland）：
  `GDK_BACKEND=x11 SignalHub_*_amd64.AppImage`。
- **展示模式在 Wayland 下没验过**：它靠无边框不透明窗 + 无边框全屏占屏，理论上没问题，
  但合成器可能忽略窗口位置。真机上没验过之前，别拿它当会议投屏的唯一方案。
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
