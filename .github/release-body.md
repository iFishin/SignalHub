### 本次更新（0.4.3）

**审批从「能点」变成「敢点」。** 这一版的重心是审批这条路上剩下的几件事 ——
不只是「有个按钮可以按」，而是按之前看得清、按之后查得到、按错了说得回去。

- **审批历史 / 审计**（A1）。每次允许 / 拒绝 / 超时都落一行到
  `~/.signalhub/approvals/YYYY-MM-DD.jsonl`（保留 30 天，文件 `0600`、目录 `0700`，
  和 token 同档），设置 →「历史」页能看到「昨天那条 `rm -rf` 是谁放行的、等了多久」。
  落盘前会把详情再截一次（盘上 200 字符，屏幕上看 240）—— `API_KEY=…` 不该永久
  躺在硬盘上。
- **拒绝可以带理由**（A2）。拒绝旁边一个小箭头，三个预设（太危险 / 换更安全的做法 /
  先解释再试）外加自定义。理由会变成一句英文进模型上下文，模型下一步不再只能收到
  一句「用户拒绝了」。**默认路径逐字节不变** —— 不选理由仍然是一键拒绝。
- **危险命令要按住 800ms**（A3）。`rm -rf`、`git push --force`、`git reset --hard`、
  `DROP TABLE`、`curl | sh`、`mkfs`、`dd if=`、`chmod 777`、`sudo` 命中时，气泡顶部
  一条红横幅 + 一句人话，「允许」要按住才生效。**只强调、不拦截、不自动拒绝**，
  而且**拒绝永远一键** —— 危险的东西，让人更容易说「不」才对。
- **写文件时看得见要写什么**（A7）。`Edit` 显示 `路径 · +12 −3` 加三行预览，
  `Write` 显示路径与内容，`Bash` 显示命令全文 + 工作目录。几百行时最后一行是
  `… +497`，**说出底下还有**。装饰装不下就不加 —— **绝不**把一条本来能放行的调用
  变成只能拒绝。
- **手机审批**（A4）。只读 Web 面板现在能点允许 / 拒绝（危险命令同样要按住），
  在设置 →「接入」的「Web 面板」下面开「面板审批」，**默认关**。这是一个**显式
  扩大暴露面**的开关：开了之后命令原文会进到浏览器里。关着时面板仍然只读、连详情
  都不拉。同源 POST 因此放行（跨源仍然 401），token 那一关照旧。

**Windows 上点「每组分灯」整个应用卡死 —— 修了。**

根因是 Windows 独有的重入，两个环叠在一起：Windows 的 IPC 跑在 WebView2 的消息回调里
（**就在主线程**），而 `WebviewWindowBuilder::build()` 在 Windows 上会开一层**嵌套消息泵**
去等 WebView2 自己的创建回调 —— 在回调内部再泵一层去等它自己，互等。macOS 上怎么都
复现不出来，因为那边的 IPC 不在主线程。

修法：建 / 关灯窗不再就地做，而是投给一条专门的线程按计划顺序执行（`lamp_worker`），
设置面板的 IPC 立刻返回。**只有改尺寸仍就地做** —— 折叠态悬停展开是热路径。
同步段从 159ms 降到 40–90ms。**建第一盏灯就卡，所以和几盏灯无关**，和报告里的
「一个组、多个组都一样」完全对得上。

**收件箱多了「你离开的这段时间」。** 离开 >10 分钟再打开收件箱，顶部一句话：

```
你离开的 47m 里   2 个在等你 · 1 个失败 · 3 个完成 · 1 个还在跑
```

判据是每条会话当前状态的起点晚于「上次看过收件箱的时刻」，所以**不需要任何事件流水**。
离开期间什么都没变就说「没有新动静」。做不到的三件事（离开期间出现过又消失的会话、
变了又变回原样的、`idle`/`stale` 不计）写在 `docs/architecture.md` §3.25.1 里。

**两处位置修正。** 「分组方式」和「灯的数量」从「接入」页搬到「分组」页 —— 导航里本来
就有这一页，而这两项正是它的主题（一个定「有哪些组」，一个定「它们摆成几盏灯」）。
另外切换布局前后各记一行到 `~/.signalhub/hooks.log`，以后「卡住了」能查：**有头无尾
就是卡在中间**。

**详情见 [README](https://github.com/iFishin/SignalHub#readme) 与各文档：**
`docs/plan-approval.md`（A1–A7）、`docs/architecture.md` §3.25 / §3.7。

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
