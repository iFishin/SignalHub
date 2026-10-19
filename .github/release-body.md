### 本次更新（0.3.1）

**转发可以自定义请求体了。** 「转发」页里每条钩子除了那些现成预设，多了一个
「自定义请求体」：自己写 JSON，用 `{state}` / `{group}` / `{message}` /
`{duration}` 这类占位符填值，content-type 也能改。content-type 是 JSON 时，替换
进去的值会**自动转义引号和换行** —— 消息里带个 `"` 不会把整条 body 写坏。

占位符一共八个：`{state}` `{aggregate}` `{group}` `{message}` `{duration}`
`{since_ms}` `{at_ms}` `{summary}`。保存时会校验，写错了会告诉你能用的是哪几个；
认不出的占位符运行时原样保留。格式见 [docs/api.md §4.9](docs/api.md)。

**修掉的一批问题**（多数来自上一轮代码审查）：

- Windows 上「试一下」必然失败（直接 spawn 了 `sh`）；
- 「试一下」的输出过长时可能让应用直接退出（按字节截断踩到 UTF-8 中间）；
- 多灯模式下可能整个应用卡死（建灯窗时一直握着锁）；
- 折叠态 5 组以上时 `+N` 角标被窗口裁掉；
- 点一下灯效预设会静默删掉每个状态的颜色和静默超时；
- 待审批 4 条以上时灯上的布局被撑坏；
- 设置窗的接入页 / 系统页数据在启动后不再刷新；
- 刚静音的组，5 / 15 / 30 分钟的重复提醒照旧会来；
- 免鉴权的 hook 端点挡不住浏览器发起的跨站请求（现在带 `Origin` 一律 403）；
- 「系统」页的日志尾巴未脱敏，复制进 issue 会把推送密钥一起带走；
- Windows 上并发写配置文件可能丢一轮记录；
- 设置面板的开关和滑杆没有可访问名称（读屏只会念「开关」）。

文档与实现不一致的地方一并改了（README 中英、api.md）。

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
