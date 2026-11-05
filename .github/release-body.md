### 本次更新（0.5.0）

**这盏灯第一次被当成「门面」来做。** 之前几版都在补能力：审批、多机、统计、演示。
这一版反过来，只做一件事 —— 让那个每天挂在桌面角落的小东西，看起来配得上它承担的角色。

> **升级前请先看一眼这条：左键的行为变了。**
> 以前是「单击打开收件箱、双击折叠」，现在是「**单击折叠 / 展开、长按呼出圆环菜单**」。
> 收件箱挪进了圆环菜单里（托盘和快捷键那两条入口照旧，没有少）。

**它开始会呼吸了。** 每颗灯珠外面多了一圈同色柔光，跟着那个状态**自己的节奏**呼吸
（推理时快、执行时慢，时长是灯珠闪烁的两倍 —— 慢一拍才像呼吸，同步就只是闪）。
换状态时从灯珠扩出一圈涟漪，牌面多了一道极细的内高光，深色牌从「一块深色牌」变成
「一片有厚度的玻璃」。悬停时那道高光会微微亮起，像手指碰了一下玻璃，而不是开了一盏灯。

这些全是**纯观感**：不动牌宽、不动点径、不碰窗口几何、不改状态机。所以它也是**可关的** ——
设置 →「外观」→「灵动效果」，关掉后逐像素回到 0.4.4 的样子。系统开了「减少动态效果」时
会自动降级成静态柔光。

**长按灯，它会展开成一个圆环。** 四个动作绕一圈：上=收件箱、右=设置、下=暂停 / 继续、
左=退出，中心那颗灯珠就是它现在的状态。弹簧依次弹出，Esc 或点空白处收起。
为什么做成圆环而不是又一个下拉菜单：右键那张管的是「**这一组**的操作」（改名、静音、
隐藏、手动状态，项多且要分父子），而圆环管的是「**这盏灯**本身的动作」—— 只有四件、
彼此平级，绕一圈正好。两套各管一件事，不打架。

**顺带变快的一点。** 以前为了让「开收件箱」和「折叠」不撞车，单击必须等 280ms
判断是不是双击。双击取消之后，那 280ms 就没有存在理由了 —— 现在点一下立刻有反应。

**设置面板那个重叠修掉了。** 0.4.4 发出去之后有反馈：左栏页签一多，底部「暂停监控」
和退出按钮就被压住。根因是左栏的滚动区没有收缩下限，内容顶破了容器。现在顶部搜索框和
底部按钮都留在滚动区外面，中间那段自己滚；同时把散在八档的字号收成四档、正文行距放宽，
小标题不再比正文还小。

**这一版没有新增状态、没有动状态机、没有新增落盘格式。** 唯一的「破坏性」变化是左键手势，
所以按语义化版本走了 minor。

**详情见 [README](https://github.com/iFishin/SignalHub#readme) 与各文档：**
`docs/plan-round5.md`（本轮清单）、`src/styles/lamp.css` 文件头（灯窗几何的两处真值）。

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
