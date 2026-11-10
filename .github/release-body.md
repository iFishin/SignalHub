### 本次更新（0.6.3）

**0.6.2 的补丁版**：一块牌上最多能出现五段内容，牌宽是固定的（`size × 7`），再加任何
一段都只能把别的挤成省略号。这一版不写字 —— 把「表达」交给**形状和面积**：状态色可以
铺到牌的轮廓上，四个角各能挂一枚小灯。另外撤掉了牌右上角那枚悬停才出现的 `×`。
没有新增信号状态、没动状态机、没改落盘结构（只多了两个纯外观字段），按补丁发。
**升级不用改配置，已装的接入、已收到的留言都不受影响。**

> **两处可见的变化**：① 牌右上角那枚 `×` 没有了（清除改走右键菜单，见下）；
> ② 边灯和角灯**默认全关** —— 升级后灯不会自己长出新东西，想看要自己开。

**边灯：把状态色铺到轮廓上。** 四档 —— 不亮 / 顶边 / 四边 / 辉光。顶边是一道 1.5px 的
亮线加一小截余晖，四边是 1px 描边，辉光在描边外再加一圈外发光。它解决的是「离得远」：
字要凑近读，边缘的亮线在余光里就是一个颜色。

**角灯：四个角各回答一个是非题。** 每块牌的四角独立配置 —— 空 / 状态色 / 置顶 / 静音 /
将过期。**条件不成立就根本不画**，而不是画一个暗点：一排四个暗点会让人以为「这四个位置
本来就有东西、只是没亮」，于是永远不会去设置里找。

两者都**绝不参与窗口几何**：边灯全在绝对定位的伪元素上，角灯是绝对定位的子元素，牌宽高
仍只由原来那两处共享值算。所以调它们不会让灯变大变小 —— 这和 0.6.2 的质感五档是同一条
判断：不碰盒子的才能随便加。都在**外观 → 质感**里，角灯默认收起（收着时会说「挂了几个角」）。

**牌右上角那枚 `×` 撤掉了。** 它清的是「这一组的状态」，但一枚常驻窗口上的 × 读起来是
「关掉这块牌」，得靠 tooltip 纠正就说明形状本身说错了话；而那个角落正是右上角那盏角灯的
落点，一悬停就把灯盖住。**清除没有丢**：右键那块牌 →「清除该组的状态」还是原来那条，
设置 →「状态」页每行的 `×` 也照旧。首启引导那枚「不再提示」一并撤了 —— 引导本来就会在
真有组上报时自动收起，牌里也还有「看演示」。

**演示多了第五场「用光说话」。** 边灯四档逐档过一遍，最后一块牌同时满足四个条件
（有状态、被置顶、被静音、正在沉默），四枚角灯一起亮给你看；它讲的就是「不亮 = 根本不画」。

详见
[architecture.md](https://github.com/iFishin/SignalHub/blob/main/docs/architecture.md)
§3.35（边灯与角灯）/ §3.14.2（清除的入口），以及
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
