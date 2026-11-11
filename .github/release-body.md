### 本次更新（0.6.6）

**托盘图标左键现在弹出面板，菜单挪到右键。** 以前左键点菜单栏那个图标，跳出来的是一列
原生菜单 —— 想看一眼「谁在等我」得先展开子菜单、再点「收件箱」、再等窗口出现。现在
左键直接长出一个面板，贴在图标正下方，顶部两个页签：**收件箱**（待处理数用红徽章）和
**留言**（未读数用浅灰徽章）。从灯上长按 480ms 呼出的圆环菜单打开时，面板仍然贴在灯
旁边 —— 两个入口各从自己那个锚点展开，不互相借用位置。

面板里的留言是设置里那页的紧凑版：来源名、多久以前、标题、两行正文（点开看全文并标为
已读），展开后有「打开来源」和「复制」。未读是蓝点，读过压成灰点 —— 红色在这个窗口里
专指「有事在等你」，两种点不该同色。这一页**没有做来源筛选**：420px 宽里筛选是「整理」
的动作，而这个面板回答的只有一句「刚才有人找我吗」。

> **一个必须说清楚的时序**：面板开着的时候点托盘图标，鼠标按下会先让面板失焦收起，
> 等托盘那边的抬起事件到的时候窗口已经不可见 —— 天真的写法会把它又打开，表现就是
> 「点图标永远关不掉面板」。所以失焦收起会记一笔时刻，400ms 内那次点击按「收起」处理、
> 读一次即清空；`Esc` 和 `×` 不计这一笔，否则「刚关掉又想打开」会被误吞。

**边灯和角灯现在可以各自闪烁。** 上一版把边灯、角灯从「一条轨 + 两行触发」拆成六个
独立灯位，但闪烁是写死的：边灯能呼吸、角灯永远常亮。理由是怕四枚角灯一起硬闪太吵 ——
可这个判断不该由我们替所有人做。现在外观页那六行各多一枚开关，默认**全关**，观感
和升级前完全一样；打开之后角灯复用的是同一个状态节奏（`working` 1600ms 呼吸、`error`
340ms 硬闪），系统开了「减少动态效果」时仍然一律静止。

顺带修掉一个更隐蔽的问题：**选了中性色，状态角灯会跟着变灰。** 中性档本来只说「边框
线上的中性描边压暗一点」，不该牵连靠状态取色的角灯 —— 上一版把它们一并压了，于是角灯
看着「不亮也不闪」，像是开关坏了。现在中性档只压边线与记号类角灯，状态角灯照旧取
`--state`（`#16a34a` / `#d0342c`）。

**外观页的「基础 / 质感」改成分区条，边框光独立成页。** 原来那两个切换是 12.5px 的
透明文字按钮，而下面常驻的内容有一大半不跟着它变 —— 用户很自然地把它们当成普通按钮
点一下就忘。现在是一整条三等份、等宽的格子，压在实时预览正下方、滚动时吸顶，每格下面
有一行说明字；第三格「边框光」把六行灯位单独拎出来，不再和三套外观预设、五套皮肤挤在
一页。

升级**不用改配置**：没有新增信号状态、没动状态机，旧配置读盘时照旧自动迁移；六个灯位
默认仍然全关。已装的接入、已收到的留言都不受影响。

详见
[architecture.md](https://github.com/iFishin/SignalHub/blob/main/docs/architecture.md)
§3.25（托盘面板与留言页）、§3.35.3（灯位闪烁与取色），以及
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
