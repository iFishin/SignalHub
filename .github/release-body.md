### 本次更新（0.6.1）

**0.6.0 的跟进版**：把刚发布的 Call Me 和设置面板按实际使用反馈收了一遍。三件事 ——
Call Me 从「要填一堆参数」改成「点一次启动」；链接管理与设置左栏补齐常用操作；
Windows 免安装版的系统通知修好。没有新增信号状态、没动状态机、没改已有落盘格式
（只多了来源上的 `auto_label` 一个字段和 `~/.signalhub/toast-shortcut.txt` 一个状态文件），
所以按 0.6.0 的补丁发。**升级不用改配置，已装的接入、已收到的留言都不受影响。**

> 这一版补的端点和交互不少，但都是围着 0.6.0 刚加的两块做打磨，没有开新方向，
> 因此没有另起一个 minor。

**Call Me：用户只点一次「启动」。** 0.6.0 建链接还要填名字和出处，可这个功能本来就是给
外部智能体用的 —— 名字和出处该由它自己带过来。现在「连接 → Call Me → 服务器」点
「启动」，应用会起本机服务、自动填回地址与 token、顺手建好第一条链接，没有任何要填的
表单。链接暂时叫 `link-xxxx`，智能体发第一条留言时用 `title` / `context` 把名称与出处
回填；你手动改过名之后就不会再被覆盖。

**链接自带用法，而且仍然足够短。** 文章里贴的还是一条 `https://…/c/<token>`。智能体
直接 GET 这条链接就能拿到纯文本用法（请求方式、请求体长什么样、`Content-Type` 要求），
对着**同一个 URL** POST 即可留言 —— 不用翻文档、不用另附提示词。链接长度不变，POST
收信的行为也不变，已经发出去的链接照旧可用。

**链接管理补齐了。** 有效链接占主列表，可复制、打开用法、行内改名、吊销、恢复、彻底
删除（删除有二次确认）；吊销的收进默认折叠的「已吊销 (n)」，不再一直占位。50 条上限
只数有效链接，删除会释放位置，恢复受同一上限约束；删掉来源后**已经收到的留言仍然
保留**。服务器侧对应新增 `/rename` / `/delete` / `/restore` 三个 admin 端点（统一用
POST：桌面端只有一套 POST 封装，也避开部分反代对 `DELETE` 的不一致策略）。

**设置窗放大，左栏默认折叠。** 窗口从 720×690 改成 1000×720（最小 820×560）；五组
左栏默认只展开当前页那一组，切页自动归位，搜索时强制全展开。导航、折叠组标题和次要
动作统一换上 16×16 线性图标（`lib/icons.ts` + `components/Icon.tsx`，单色、不参与
选中态）。折叠组的未读点上提到组标题，免得收起「工作台」后看不到留言提醒。

**Windows 免安装版的通知修好了（issue #25）。** 症状是：portable zip 的用户按「测试
通知」毫无反应，界面却显示「已允许 / 已发出」；macOS dmg 正常。根因是 Windows 的
Toast 要求应用有带 `AppUserModelID` 的**开始菜单快捷方式** —— NSIS 安装器会建，解压
即用的 zip 没有，系统就静默丢弃通知；再叠上插件在 Windows 恒返回 `Granted`、注册动作
后走的 `show_tracked` 又吞掉底层 `show()` 错误，就成了「界面说发了，系统没显示」。

现在应用启动时（仅当前用户，不需要管理员）补齐这套身份：
`SetCurrentProcessExplicitAppUserModelID`、`HKCU` 下 `AppUserModelId` 的一个键，以及
用 COM 写进当前用户开始菜单 `.lnk` 的 `System.AppUserModel.ID`。快捷方式这一步是
决定性的 —— 只写注册表不够。同时把「静默」改成可见：注册失败时设置页显示「未注册」
+ 具体错误 +「重新注册」按钮，一键诊断报告追加后端状态；测试通知的文案改成「已交给
系统，应用看不到系统是否真的显示了它」。

> **口径变化**：portable 版不再「完全不写注册表、不建快捷方式」。首次启动只写一个
> HKCU 键和一个当前用户开始菜单项，不写 Program Files、不需要提权。
> **这一版还没有 Windows 真机 Toast 验收**（CI 只保证 Windows 分支真的编过），
> [issue #25](https://github.com/iFishin/SignalHub/issues/25) 保持 OPEN 等实测。

**详情见 [README](https://github.com/iFishin/SignalHub#readme)、
[docs/callme.md](https://github.com/iFishin/SignalHub/blob/main/docs/callme.md)、
[docs/api.md](https://github.com/iFishin/SignalHub/blob/main/docs/api.md) 与
[architecture.md](https://github.com/iFishin/SignalHub/blob/main/docs/architecture.md)
§3.29 / §3.30。**

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
