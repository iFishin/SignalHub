### 本次更新（0.6.0）

**功能版**：新增 **Call Me** —— 让读到你的文章的外部 AI 智能体，通过你自己部署的服务器
给你留言；同时把设置面板按「看内容 / 配功能 / 找帮助」重排。没有新增信号状态、没动状态机、
也没改已有的落盘格式（只多出 `~/.signalhub/callme-*.json` 几份新文件），所以按语义化版本
走 minor。**升级不用改配置，已装的接入不受影响。**

**Call Me 是另一个方向的通道。** 灯回答的是「我的智能体在干什么」，Call Me 回答的是
「别人读了你的东西想说话，怎么送到你这儿」。你在文章里贴一条链接，读到的智能体往你自己的
服务器 POST 一条留言；桌面端每 15 秒增量拉一次，在「设置 → 工作台 → 留言」里读，旁边有
「打开出处」，方便回去改。它不进状态机、不进收件箱 —— 留言是要读的文字，没有「允许 / 拒绝」
这类即时动作，混进收件箱只会把「需要你」那一段淹掉。

**服务器是你的，不是我们的。** `server/` 是一个独立 crate（不链接 Tauri），裸 VPS 上装个
Rust 就能编，也可以 `docker build`；只监听明文 HTTP，TLS 交给 Caddy 两行配置。启动只要一个
`CALLME_ADMIN_TOKEN`（少于 16 字符直接拒绝启动）。每处一条链接、各自 128 位令牌，能单独
配额、单独吊销；防滥用三道闸门：每 IP 每小时 10 条、每链接 24 小时 50 条、正文 4000 字符。

**不想先买服务器，就用本机那条。** 「连接 → Call Me → 服务器」点「启动」，应用会在
`127.0.0.1:43877` 起同一个服务，地址和 token 自动填回设置，退出应用即停。它**只监听回环**，
所以那条链接发给外网是打不开的 —— 它是用来先把流程跑通、自己用 curl 发一条看看的。真要
公开，还是得部署到 VPS（或你自己的隧道），再把文章里的链接换成公网地址。

**设置面板重排。** 十几个页签压在一层，三类任务混着：看内容（状态 / 历史 / 留言）、配功能
（灯 / 通知 / 连接 / 系统）、找帮助（指南 / 演示）。现在是五组 + 一个沉底的「系统」：

```text
工作台  状态 / 历史 / 留言
灯      外观 / 行为 / 分组
通知    提醒 / 转发
连接    接入 / 多机 / Call Me
帮助    指南 / 演示
系统    通用 / 更新与诊断 / 数据
```

**只给三个最长的页面加页内子页**（接入、Call Me、系统），其余保持一页到底，也不再加第三层
导航。左栏只有 140px，多一页就要从正文拿一像素；页签超过一屏，「翻页」就变成「先搜索」。

**留言独立成页。** 以前服务器地址、链接管理、留言阅读纵向堆在 Call Me 一页，想读留言得先
经过配置；未读点也只能挂在那页上，提醒指向了用户并不想读的那一页。现在红点挂在「留言」，
Call Me 只剩「链接 / 服务器」两个子页。两页共用同一份轮询状态，未读数不会因为分页而不同步。

**每页一个页头，今日时长搬去「历史」。** 顶部原来在每一页重复显示聚合状态和今日各状态
时长；后者是回顾数据，不该占每一页的一行。现在全局顶部只留聚合，页头负责说清「我在哪儿、
这页能做什么」，主操作（全部已读、管理链接）放在右侧。

**旧书签不会走错页。** 页签结构搬进了 `lib/settingsNavigation.ts`，单测守着「历史上出现过的
页签 id 仍然认得」—— 旧版本把当前页存进 localStorage，删一个 id 等于把回来的人送到空白页。

**详情见 [README](https://github.com/iFishin/SignalHub#readme)、
[docs/callme.md](https://github.com/iFishin/SignalHub/blob/main/docs/callme.md) 与
[architecture.md](https://github.com/iFishin/SignalHub/blob/main/docs/architecture.md)
§3.27 / §3.28。**

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
