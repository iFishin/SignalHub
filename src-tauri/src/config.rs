use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 圆点直径的允许范围。config.json 是用户可手改的文件，属于系统边界，
/// 所以加载时要夹紧——否则一个手滑的 2000 会在屏幕上摊出一个巨型窗口。
const MIN_SIZE: u16 = 12;
const MAX_SIZE: u16 = 64;
pub const DEFAULT_SIZE: u16 = 22;

/// 牌宽 / 牌高的倍率范围。1.0 = 现状。
///
/// 下限 0.8：再小牌上那几段字就压成一团了（牌宽已经是「文字被截断」的前提，
/// 缩太多等于把信息全砍掉）。上限 2.5：已经大到能占掉小半块屏，再多就不该靠
/// 这一盏灯回答了。
pub const MIN_SIGN_MULT: f32 = 0.8;
pub const MAX_SIGN_MULT: f32 = 2.5;

/// 倍率的夹紧。**NaN 要单独挡掉**：浮点的 `f32::clamp` 遇到 NaN 会把 NaN 原样
/// 返回（它是自查式比较，和一切都不相等），而 NaN 一路传到 `ceil` 会变成 0、
/// 把牌宽归零 —— 那正是这份夹紧要防的事。非有限值也一并退回 1.0。
fn clamp_mult(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_SIGN_MULT, MAX_SIGN_MULT)
    } else {
        1.0
    }
}

/// 圆点四周留白的**总量**（不是每边）。留白是为了好抓取圆点来拖动。
///
/// **28（每边 14）不是随手取的**：窗口右上角那枚「暂停」角标就住在这圈留白里
/// （`lamp.css` 的 `.lamp-flag`：12px 的方块 + 1px 边距 = 14）。它不参与布局，所以
/// 留白是它唯一的容身之处。早先总量 20（每边 10）时角标只能做成 9px，小到像渲染残留。
pub const LAMP_MARGIN: u16 = 28;

/// 灯球上最多同时显示几块牌。超出的截断 —— 牌按优先级排，所以留下的是最紧急的。
pub const MAX_SIGNS: usize = 4;

/// 待审批气泡占的高度（**每一条**）。见 `lib.rs` 的 `approval_area`。
///
/// **必须和 `src/styles/lamp.css` 里 `.approval` 的高度一致**：Rust 用它算窗口高度、
/// 前端用它排版，两处不一致就是「按钮被裁掉一半」—— 而那正好是这个功能最不能出的错
/// （看不见的按钮等于没有按钮）。和牌那套几何一样，这是刻意的两处真值 + 同一次提交
/// 同步。
pub const APPROVAL_BUBBLE_H: u16 = 132;

/// 待审批气泡占的宽度。窗口本来只装得下一块牌（约 `size` 宽），气泡比它宽 ——
/// 因为里面要放一整条命令。
pub const APPROVAL_BUBBLE_W: u16 = 360;

/// 牌的宽度。
///
/// 由 `size` 派生，不另加一个设置 —— `size` 现在表示整块牌的尺度，「大小」
/// 一个旋钮就够。文字长度不定，所以宽度**固定**、超出用省略号截断：让宽度
/// 跟着文字走会让窗口每次消息变化都跳一下。
///
/// `w` 是用户设的**宽度倍率**（见 `Settings::sign_w_mult`），1.0 = 原样。
/// 两步都重要：
/// - **先算整数 `size * 7`，再乘倍率**：这样 `w = 1.0` 时逐字等于旧公式 ——
///   默认值不许无声改变既有行为（红线 3）。
/// - **向上取整**，不是四舍五入：CSS 那边的宽度是 `size*7*w` 这个小数（浏览器
///   不会帮我们取整），取整方向反了（`round` 可能向下）会让窗口比牌窄一线，
///   `overflow: hidden` 就把右边缘裁掉了。向上只会多留不到 1px。
pub fn sign_width(size: u16, w: f32) -> u16 {
    ((size * 7) as f32 * w).ceil() as u16
}

/// 一块牌的高度。有消息就多一行 —— 那块牌会变高，正好是个「有东西可读」的提示。
///
/// `h` 是高度倍率，弄法和 `sign_width` 一样：先整数、后乘、向上取整。
///
/// **CSS 里的 `.sign` / `.sign[data-has-message="true"]` 必须与这两个值严格对齐**，
/// 否则窗口会裁掉最后一块牌、或者底下留一条缝。
pub fn sign_height(size: u16, has_message: bool, h: f32) -> u16 {
    let base = if has_message { size * 5 / 2 } else { size * 3 / 2 };
    (base as f32 * h).ceil() as u16
}

/// 折叠形态的圆点个数上限。折叠成一个单纯的灯时，一个组一个点。
///
/// 刻意和 `MAX_SIGNS` 取同一个值：折叠只是换了个外形，不该顺带少显示几个组 ——
/// 那样会让人以为一折叠就把状态弄丢了。
pub const MAX_DOTS: usize = MAX_SIGNS;

/// 折叠态最多几枚额外角标：现在只有「还有几组」的 `+N` 一枚。
///
/// 暂停 / 穿透那枚曾经也占一个点位，开关一拨窗口就要跟着加宽，整排灯珠被重新
/// 居中、看着像被挤出去；它现在是灯体内部最上沿的绝对定位徽标（`lamp.css`
/// 的 `.lamp-flag`），不参与几何。这个上限只用来兜住一个算错的调用点。
const MAX_BADGES: usize = 1;

/// 折叠后那几颗灯往哪个方向排。
///
/// **默认竖排**（用户明确要的默认改动）：折叠的诉求是让出横向桌面，一串竖着的
/// 点占的是一条窄边；原来横着排反而把窗口摊成一条长条，挡住的是整行。
/// 老配置里没有这个字段，`Settings` 的容器级 `serde(default)` 会把它们一并
/// 落到 `Vertical` —— 这是有意的行为变化，不是迁移事故。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FoldAxis {
    #[default]
    Vertical,
    Horizontal,
}

/// 折叠后每颗灯旁边带哪一段文字。
///
/// 四档**单选**而不是可多选：窗口宽度必须能从设置直接算出来（文字长度不定，
/// 一律固定宽度 + 省略号截断），多选会让「哪几段开着」变成一个组合数，Rust
/// 与 CSS 就得各自维护一张加法表。四档覆盖「是哪一盏 / 排第几 / 现在怎么样」
/// 三种最常被问到的事，再加一个「什么都不要」。
///
/// `None` 是默认：升级上来的人看到的是原来的裸点，只是方向变成竖排。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FoldLabel {
    #[default]
    None,
    Name,
    Index,
    State,
}

impl FoldLabel {
    /// 一格的宽度相对 `size` 的倍率。`None` 时不画板，格子就是一颗 `size` 见方的点。
    ///
    /// **这条曲线的用途是「一眼认出是哪一组」，不是「把组名排完」**。折叠的全部
    /// 意义是让出桌面：展开牌宽 `7×size`（22px 档 154px），一格能写的字只要够
    /// 认出一组就行。0.6.7 之前给的倍率是 3 / 4 / **6**，组名档 132px 宽、
    /// 窗口 160px —— 比展开牌只窄 22px，用户的原话是「那么折叠没有意义啊」。
    ///
    /// 现在按「文字区还能放下几个汉字」反推（一格里的固定开销 ≈ 1.48×size：
    /// 左右内边距 0.5 + 灯珠 0.8 + 珠字间距 0.22；字号 0.5×size，
    /// 所以能写的汉字数 ≈ 2×倍率 − 2.95）：
    ///
    /// | 档位 | 倍率 | 22px 档格宽 | 文字区 | 能写 |
    /// |------|------|-------------|--------|------|
    /// | 序号 | 2.4 | 53px | 21px | 3 位数字（最多 4 组，1 位就够） |
    /// | 状态 | 3.5 | 77px | 45px | 「需要授权」正好 4 字 |
    /// | 组名 | 4.0 | 88px | 56px | 5 个汉字，超出省略号 |
    ///
    /// 窗口宽 116px（组名档）对展开态 182px：窄 36%，折叠终于看得出来。
    ///
    /// 倍数写成小数没问题 —— 跟 `size` 相乘后再 `ceil`，窗口宽仍**只由设置决定**，
    /// 组名再长也不会让窗口跳一下（和 `sign_width` 同一个取整方向）。
    pub fn width_mult(self) -> f32 {
        match self {
            FoldLabel::None => 1.0,
            FoldLabel::Index => 2.4,
            FoldLabel::State => 3.5,
            FoldLabel::Name => 4.0,
        }
    }

    /// 带文字的档共用一块窄板；只有灯的档是几颗互不相连的裸点。
    pub fn has_text(self) -> bool {
        self != FoldLabel::None
    }
}

/// 折叠态一格的宽 / 高（逻辑像素，不含窗口留白）。
///
/// **带文字时行高直接取一块单行牌的高度**（`sign_height`），于是折叠窄板和
/// 展开的行是同一把尺子 —— 不用再维护第三套高度。宽度是 `size × 倍率 × 宽倍率`，
/// 向上取整的方向与 `sign_width` 一致：窗口只许多留，不许裁。
pub fn fold_cell_size(size: u16, label: FoldLabel, w: f32, h: f32) -> (u16, u16) {
    if !label.has_text() {
        return (size, size);
    }
    (
        (size as f32 * label.width_mult() * w).ceil() as u16,
        sign_height(size, false, h),
    )
}

/// 折叠态要不要挂那枚 `+N` 角标：可见组数超过 [`MAX_DOTS`] 时，多出来的那些
/// 在灯上**一点痕迹都没有** —— 而「有几个智能体在跑」恰恰是这盏灯要回答的问题。
///
/// 展开态同样的提示直接挂在最后一块牌上（见 `SignalLamp.tsx` 的 `sign-more`），
/// 折叠态没有牌面可寄住，只能自己占一个点位。
///
/// **两边必须是同一个判断**：前端画几枚，`collapsed_window_size` 的 `badges`
/// 就得算几枚 —— 少了那枚角标会被窗口右边裁掉，多了会白留一块。
pub fn more_badge(visible: usize) -> usize {
    usize::from(visible > MAX_DOTS)
}

/// 折叠形态的窗口尺寸：N 格按 `axis` 排成一行或一列，每格大小由 `label` 决定。
///
/// `label = None` + `axis = Horizontal` **逐像素等于 0.6.6 的那排裸点**：
/// `N × size + (N−1) × size/3 + LAMP_MARGIN` 宽、`size + LAMP_MARGIN` 高。
/// 竖排只是把这两维对调，所以「默认竖排」不需要动别的几何约定。
///
/// 带文字时每格变成一段「灯珠 + 文字」，N 格共享一块窄板（`.fold-board`）：
/// 板内靠 1px 细线分格，**不再留点距** —— 否则连续的一块板会被切成几条。
/// `w` / `h` 是用户设的牌宽高倍率，和展开态同一把尺子。
///
/// `badges` 是**占一格位置的额外角标数**，现在只有「还有几组没画出来」的 `+N`
/// 那一枚（见 [`more_badge`]）。暂停角标是绝对定位在灯体内部最上沿的
/// 徽标，**不占格位**，所以切换穿透 / 暂停都不该让窗口尺寸变（那会把灯珠推偏）。
///
/// ⚠️ 点径、点距、格宽高、留白和 CSS 的 `.lamp-dot` / `.lamp-dot-more` /
/// `.fold-row` / `.lamp-root` 是两处真值（见本文件头），改一处必须同步另一处。
/// 前端把这四个数算成 px 变量挂在根上（`lampStyle.ts::lampVars`），CSS 不再
/// 自己用小数 `calc` 重算一遍 —— 那正是旧折叠态差半像素的来源。
pub fn collapsed_window_size(
    size: u16,
    dots: usize,
    badges: usize,
    axis: FoldAxis,
    label: FoldLabel,
    w: f32,
    h: f32,
) -> (u16, u16) {
    // 分别封顶再相加，而不是封顶它们的和：封住和的话「多枚角标」会被当成
    // 「少几个组」，于是牌少了一块 —— 那正是截断要避免的那种谎。
    let dots = dots.clamp(0, MAX_DOTS);
    let badges = badges.min(MAX_BADGES);
    let count = (dots + badges).max(1) as u16;
    let gap = if label.has_text() { 0 } else { size / 3 };
    let (cell_w, cell_h) = fold_cell_size(size, label, w, h);
    let (width, height) = match axis {
        FoldAxis::Horizontal => (count * cell_w + (count - 1) * gap, cell_h),
        FoldAxis::Vertical => (cell_w, count * cell_h + (count - 1) * gap),
    };
    (width + LAMP_MARGIN, height + LAMP_MARGIN)
}

/// 圆环菜单的窗口边长（逻辑像素，正方形）。
///
/// **为什么不跟 `size` 派生**：牌和点跟着 `size` 缩放是对的（它们画的是内容），
/// 但菜单项是**可点击目标** —— 缩小到 16px 的灯上给一颗 8px 的按钮等于点不中。
/// 所以这里是一组固定的绝对尺寸，用户调灯的「大小」不会影响它。
///
/// 数值的来由：圆环半径 66 + 菜单项半径 17 + 留白 15 = 98，取整成 196 的边长。
/// 半径 66 是四项均分 90° 时相邻项之间还留得出 2×66×sin(45°) ≈ 93px 的空档
/// （项直径才 34px），不会挤在一起；六项时降到 2×66×sin(30°) = 66px，仍然够。
pub const MENU_WINDOW: u16 = 196;

/// 圆环菜单的半径（逻辑像素）。CSS 的 `--menu-radius` 必须与它一致 ——
/// 那是两处真值，和牌那套几何同一个道理。
pub const MENU_RADIUS: u16 = 66;

/// 圆环菜单打开时窗口该多大：正方形，**不看当前形态**。
///
/// 菜单一打开，灯牌就让位给圆环（前端只画圆环 + 中心那颗灯珠），所以窗口
/// 不必是「牌尺寸」和「圆环尺寸」的并集 —— 直接换成正方形，圆环居中。
/// 这样做的好处是展开态 / 折叠态共用一套几何，没有第三种情况要维护。
pub fn menu_window_size() -> (u16, u16) {
    (MENU_WINDOW, MENU_WINDOW)
}

/// `~/.signalhub`
///
/// 这里不用 Tauri 的 `PathResolver`：CLI 是独立二进制、不链接 Tauri，
/// 而两边必须解析出完全相同的路径，所以只能靠一个共享的实现。
pub fn data_dir() -> io::Result<PathBuf> {
    let dir = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "找不到用户主目录"))?
        .join(".signalhub");
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
        set_mode(&dir, 0o700)?;
    }
    Ok(dir)
}

/// 只认环境变量，不引入 `dirs` 之类的依赖。
/// 代价是 HOME / USERPROFILE 都缺失的极端情况会失败（此时会明确报错，不会
/// 静默写错地方）；换来的是 CLI 的依赖树里少一整棵。app 由终端或 launchd
/// 启动时两者必有其一，所以这个取舍是安全的。
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";

    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

pub fn port_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("port"))
}

fn token_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("token"))
}

/// 读取 app 写入的端口。CLI 用它自动发现服务；读不到即视为 app 未运行。
pub fn read_port() -> io::Result<u16> {
    let raw = fs::read_to_string(port_file()?)?;
    raw.trim().parse().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, format!("端口文件内容不是数字: {raw:?}"))
    })
}

/// 读取已有 token。
///
/// CLI 只该调用这个，绝不该走 `load_or_create_token` —— 自己生成会造出一个
/// app 并不认识的值，表现为「服务在跑但所有上报都 401」，很难排查。
pub fn read_token() -> io::Result<String> {
    let raw = fs::read_to_string(token_file()?)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token 文件为空"));
    }
    Ok(trimmed.to_string())
}

/// 首次启动生成 token，之后复用。
/// 复用而非每次重生成，是为了重启 app 后已配置好的 CLI / hook 不用改。
pub fn load_or_create_token() -> io::Result<String> {
    // 读失败（含文件不存在、内容为空）就当作需要新建。真要是权限问题，
    // 下面那次写入会再报一次错，错误信息不会丢。
    if let Ok(existing) = read_token() {
        return Ok(existing);
    }

    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|err| io::Error::other(format!("生成随机 token 失败: {err}")))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();

    write_private(&token_file()?, &token)?;
    Ok(token)
}

/// 把实际监听端口写进文件，供 CLI 自动发现。
///
/// 也走原子写：端口文件被读到的时机是「CLI 刚启动、要连上去」，一个残缺的数字
/// 会让它连到错的端口或者干脆解析失败 —— 而这个文件每次启动只写一次，代价可以忽略。
pub fn write_port(port: u16) -> io::Result<()> {
    atomic_write(&port_file()?, &port.to_string(), None)
}

/// hook 调用日志的路径。
///
/// 存在的原因很实际：出问题时（「灯为什么显示成这样」）需要知道**哪些事件
/// 真的触发了**。事件本身看不到的话，这类问题只能靠猜——而「一轮结束时
/// Stop 有没有触发」正是这样一个问题。
pub fn hook_log_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("hooks.log"))
}

/// 每日状态时长的落盘位置。
///
/// 单独一个文件而不是塞进 `config.json`：那份是**用户手改的偏好**（改了要能一眼
/// 看懂、甚至能直接编辑），而这个是机器攒的运行数据 —— 混在一起会让配置文件里
/// 冒出一大坨没人看得懂、又不敢删的东西。
pub fn stats_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("stats.json"))
}

/// 每个信号源「最近一次上报」的落盘位置（见 `receipts.rs`）。
///
/// 和 `stats.json` 一样是**机器攒的运行数据**，不塞进 `config.json`（那份是用户手改
/// 的偏好，混进来会让配置文件里冒出一坨没人看得懂、又不敢删的东西）。
pub fn receipts_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("receipts.json"))
}

/// 日志时间戳。用 UTC 并在值上标 `Z`：不引入时区数据依赖，且不会含糊。
pub fn utc_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!("{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// 往排查日志追加一行。
///
/// **四个出口共用这一个。** 入站（智能体调 CLI）、出站（SignalHub 发钩子）、
/// 崩溃记录（`install_panic_hook`）都写它 —— 各写一遍轮转逻辑的话，迟早有一处
/// 忘了加，那个文件就会无界增长。
///
/// 单代轮转：超过 512KB 就改名为 `.log.1`（覆盖上一次的）。日志是排查用的，不是
/// 档案 —— 保留一代够回答「刚才到底发生了什么」。
///
/// app 和 CLI 是两个进程，可能同时写。`O_APPEND` 下单行的写入是原子的，所以不会
/// 互相截断；但**行与行的先后顺序不保证**。
pub fn append_hook_log(line: &str) {
    let Ok(path) = hook_log_path() else {
        return;
    };

    const MAX_BYTES: u64 = 512 * 1024;
    if fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }

    use std::io::Write;
    // **0600**，和 token 一个待遇：这份日志里记着出站钩子的完整 URL（带推送服务的
    // 密钥），不该让同一台机器上别的用户读得到（见 issue #2）。只在**建文件**时生效；
    // 已经存在的老文件保持原样（不偷偷改用户的文件权限）。
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

// ---- 用户设置 --------------------------------------------------------------

/// 圆点形状。
///
/// 默认是圆；此时 `permission` / `error` 会额外画成**圆角方**，那是给色盲用的
/// 形状区分（红绿是红绿色盲最难分的一对）。用户若把形状改成别的，这个区分就
/// 失效了 —— 设置面板里会说明这一点。
///
/// 0.6.5 从三档扩到六档（多了方 / 菱 / 胶囊）。**旧值仍然是合法的**：枚举只往
/// 后加，不改已有拼写，所以老配置不会因为多出几档就整份解析失败。新形状全部
/// 只用 `border-radius` / `clip-path` 画在 `.dot` 内部，不改盒模型、不改窗口几何。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    #[default]
    Circle,
    Rounded,
    /// 空心圆环。桌面上几乎没有别的东西长这样，最好认。
    Ring,
    /// 正方。
    Square,
    /// 菱形（转 45° 的方）。
    Diamond,
    /// 竖胶囊。
    Capsule,
}

/// 多灯布局：一盏灯画一叠牌，还是每个可见组一盏自己的灯。
///
/// **默认单灯**，也是永远可用的回落形态（可见组超过 `lamps::MAX_LAMPS` 时自动回到
/// 它）—— 新能力默认关（红线 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LampLayout {
    /// 一盏灯画一叠牌（现状）。
    #[default]
    Single,
    /// 每个可见组一盏自己的灯窗，可以各自拖到不同位置；主灯只剩一颗全局灯。
    PerGroup,
}

/// 灯上**单击**的主操作。
///
/// v0.5.0 的手势重做把折叠挪到了单击，收件箱被挤进「长按 → 圆环 → 点图标」三步。
/// 这里让用户把最便宜的手势还给自己最常做的事，而不是替所有人规定一个「主操作」。
///
/// **默认 `Collapse`**：升级不该让任何人的单击突然变成打开一个窗口（红线 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClickAction {
    /// 折叠 / 展开（v0.5.0 至今的行为）。
    #[default]
    Collapse,
    /// 打开收件箱。
    Inbox,
    /// 什么都不做；拖拽与长按仍然照常。
    None,
}

/// 配色方案。
///
/// 给的是**预设而不是任意取色**：红 = 危险、黄 = 注意是有语义的，
/// 让用户随便改会把这层含义弄没。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    /// 默认：绿 / 黄 / 红，颜色压暗一档以便在浅色桌面上站得住
    #[default]
    Default,
    /// 色盲友好：Okabe-Ito 的蓝 / 橙 / 朱红
    Colorblind,
    /// 高饱和：在花哨或深色的桌面上更容易看清
    Vivid,
}

/// 牌面圆角档位。
///
/// **只改 `border-radius`，不改任何几何。** 牌宽 / 牌高由 `sign_width` /
/// `sign_height` 算，和这里的圆角无关 —— 所以调它不会让窗口裁掉最后一块牌。
/// 圆角大到接近牌高一半（`Round` = 0.72×size ≈ 15.8px，牌高 33px）也不会
/// 啃到文字：`overflow: hidden` 只裁圆角之外的角落，而文字的起点落在圆内。
///
/// 取值是**相对 size 的倍数**，写在 `lamp.css` 的 `[data-sign-radius]` 那几条
/// 规则里。多一档就多一条规则，别把倍数搬进 TS —— 那样像素值就有两处真值了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignRadius {
    /// 直角。牌变成一块纯粹的色块，最硬朗。
    Square,
    /// 轻微圆角，几乎像直角但边缘不割手。
    Soft,
    /// 现状（v0.4 至今）。**默认** —— 升级不该让任何人的灯换个样子。
    #[default]
    Standard,
    /// 大圆角，接近药丸。
    Round,
}

/// 牌面描边强度。
///
/// 现状是 1px 白色 14% —— 在深色牌面和浅色桌面之间画一道分界。关掉之后牌就是
/// 一块靠不透明度立住的色块（浅色桌面上仍然分得清：0.86 不透明度的深色本来就
/// 够，描边只是让边缘更利），这也是为什么「无描边」值得作为一档存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignBorder {
    /// 不画描边。牌与桌面之间只剩底色差。
    None,
    /// 现状：1px，白色 14%。
    #[default]
    Hairline,
    /// 更明显的一道边（白色 32%）。牌读起来像卡片而不是色块。
    Strong,
}

/// 牌面底色。
///
/// 只影响 `.sign` 的深色基色（`--sign-rgb`），**不改 `--sign-alpha`** —— 后者是
/// 用户单独调的不透明度，两条轴互不干扰。灯珠 / 状态色 / 托盘图标一律不受影响：
/// 那是语义色，不是"材质"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignTint {
    /// 现状：略偏冷的深灰（#1c1c20）。和应用图标是同一套语言。
    #[default]
    Graphite,
    /// 纯黑。在彩色桌面上更沉得住，代价是在深色桌面上边缘更依赖描边。
    Black,
    /// 暖黑，带一点棕。和米色 / 木色桌面更协调。
    Warm,
    /// 蓝黑。科技感更强，和蓝色系桌面或壁纸更合。
    Slate,
}

/// 牌内字号档位。
///
/// 是一个**整体的乘数**（0.9 / 1 / 1.12），不是某个字号 —— 组名、状态词、
/// 消息、时长、`+N` 一起缩放，否则调完只会得到「组名比状态词大得离谱」。
///
/// 上限刻意压在 1.12：牌宽固定 `size × 7`，字再大只会让省略号更早出现，
/// 换不来更多可读信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignText {
    /// 0.9×：信息更密，一行能多塞两三个字。
    Compact,
    /// 现状。
    #[default]
    Standard,
    /// 1.12×：隔着一米也读得清，代价是组名省略得更早。
    Large,
}

/// 「谁在叫我」的分层强度。
///
/// 多块牌同时存在时，`lampStyle.ts::toneOf` 把授权 / 关注 / 出错划成 `need`、
/// 其余划成 `calm`，`lamp.css` 据此把 calm 的牌底压到 0.72、组名降到 400 字重、
/// 灯珠缩到 0.55×。效果是「等你动手的那块」自然抢到注意力。
///
/// 但这条规则**界面上原本没有任何解释**：用户看到某块牌比别的淡，第一反应是
/// 「是不是坏了」。所以给它一个开关 —— 既是个性化（有人就想要所有牌一样重），
/// 也是一种说明（关掉它能看出差异来自这里）。
///
/// **默认 `Auto`**，即保留分层（红线 3：升级不改观感）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignEmphasis {
    /// 现状：need / calm 两档分明。
    #[default]
    Auto,
    /// 所有牌同等分量。灯珠一样大、牌底一样实、组名一样粗。
    Flat,
}

/// 牌内字体。
///
/// 牌的宽高是固定公式算出来的，字体的意义全在「读起来像什么」：
///
/// - `System`（默认）：系统无衬线 —— 现状。
/// - `Mono`：整块牌换成等宽（组名、状态词、消息、时长一起换），读起来像一段
///   终端日志，和「看智能体在做什么」这件事更合。
///
/// **不影响牌宽高**（固定 `size × 7`），只影响字形和省略号出现的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignFont {
    /// 系统无衬线（现状）。
    #[default]
    System,
    /// 整块牌都用等宽。
    Mono,
}

/// 牌边的**边框光轨**：把状态色从一颗 8px 的点扩展到牌的轮廓上。
///
/// 它不承载新的信息 —— 颜色仍然只表示状态，和灯珠、和细环说的是同一件事。它
/// 买的是**面积**：一块牌在余光里最先被读到的不是文字，而是一块有颜色的面。
/// 灯珠再大也只占 `0.55 × size`（22px 档上不到 13px），而一条顶边有整块牌宽。
/// 这就是「不增加文字也能让灯表达得更多」的第一条路。
///
/// **四档是同一条轨的覆盖范围，不是四种画法**（0.6.3 返工）：第一版 `Top` 画渐变
/// 绿雾、`Frame` 画 1px 细线、`Glow` 又叠外发光，切档像换了一块牌，而且和
/// `SignBorder` 的中性描边叠成两层线。现在三者都是 `.sign::before` 上同一条
/// `border`，只是 `border-top-width` / 四边 / 再加 `box-shadow` 的区别。
/// 开边灯时中性描边让位，牌边永远只有一层线。
///
/// 默认 `Off`（红线 3：升级不改观感）。**没有一档参与窗口几何**（`::before` 是
/// 绝对定位的伪元素，`box-shadow` 也不改盒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignEdge {
    /// 现状：牌面只有那圈中性描边。
    #[default]
    Off,
    /// 轨只走上边 —— 最克制的一档，远看只是一条更亮的边。
    Top,
    /// 轨走四边：整块牌被状态色框住，像一台仪表的边框。
    Frame,
    /// 四边 + 牌外一圈同色柔光。隔着一米也看得出这块牌是什么颜色，
    /// 代价是在深色桌面上它比别的东西都亮。
    Glow,
}

/// 边框四个转角上「灯位」里的一枚小灯表达什么。
///
/// **灯位落在边框上，不是牌内**：落点按圆角半径算（`0.293 × r`），四档圆角里
/// 都紧贴边灯那条轨（见 `lamp.css` 的 `--corner-arc`）。0.6.3 之前写死内缩
/// `0.24 × size`，在默认圆角下离边框 5px，读起来是「牌内四个散点」，和边灯
/// 不成一套 —— 这是「灯看起来不协调」的一条主因。
///
/// 四个角是**独立**的（见 [`SignCorners`]），每一枚只回答一个是 / 否 —— 它是
/// 「不写字的第二个信息通道」，用来把那些「说得出但塞不进 154px」的事挂上去：
/// 这一组被置顶了没有、被静音了没有、还剩多少时间要沉默。
///
/// 都用**常亮**的小点，不跟状态闪：节奏是灯珠的活，四角再闪就是四处都在动。
///
/// 除 `State` 外都是**条件色**：条件不成立时那枚角灯根本不画（不是画暗）。
/// 「有一枚暗点」和「没有这枚点」是两种意思，混起来等于说谎。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignCorner {
    /// 不画。
    #[default]
    Empty,
    /// 状态色的实心点。**关掉牌上那颗圆点（`show.dot`）之后，这是唯一的颜色来源** ——
    /// 「不想要圆点、但还想一眼看出颜色」的人用它。
    State,
    /// 置顶（`group_meta[组].order` 有值）。近白实心点。
    Pinned,
    /// 静音（`muted_groups` 里有它）。**空心环** —— 和实心区分开，不必读字。
    Muted,
    /// 沉默进度：`stale` 时那圈剩余时间，越接近过期越淡。不沉默就不画。
    Stale,
    /// 点击穿透开着（`useToggles` 的瞬态状态）。**空心方** —— 圆的都是「组的属性」，
    /// 方的留给「窗口本身的状态」，形状和语义一起分家。
    ///
    /// 0.6.4 之前它是窗口右上角一枚 12px 的 `ban` 角标。角标是**常驻**的：穿透时
    /// 鼠标点不到灯，一次性通知早就忘光了，连 tooltip 都不弹。把它搬进角灯之后，
    /// 用户自己决定要不要挂、挂哪个角；没挂的人靠托盘菜单和 tooltip 那行字就够了。
    Passthrough,
}

/// 四角上的四枚小灯。
///
/// 每个角独立选（[`SignCorner`]），默认全空 —— 也就是「加这个功能之前的样子」。
///
/// **为什么不预设一套固定语义**（左上=置顶、右上=静音）：固定布局要求用户记住
/// 一张对照表，而这张表不会出现在灯上。让用户自己挂，他挂上去的每一枚都是他
/// 想要的那个意思；四个角的顺序（`tl` / `tr` / `br` / `bl`）只是屏幕上的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SignCorners {
    pub tl: SignCorner,
    pub tr: SignCorner,
    pub br: SignCorner,
    pub bl: SignCorner,
}

impl SignCorners {
    /// 挂了几枚。设置页那一行摘要用（收起时只说「挂了 2 枚」）。
    pub fn count(self) -> usize {
        [self.tl, self.tr, self.br, self.bl]
            .iter()
            .filter(|corner| **corner != SignCorner::Empty)
            .count()
    }
}

/// 边框光**什么时候亮**。
///
/// 0.6.3 之前边灯和角灯都是「配了就一直亮」，于是「要不要亮」和「长什么样」
/// 被绑死在外观页里。但这两件事回答的是不同问题：**外观**管的是「亮起来长什么样」，
/// **行为**管的是「凭什么现在亮」。拆开之后，「平时不吵、只有要我动手时才把整块牌
/// 框成红的」这类用法才有地方放。
///
/// 边灯和角灯**共用同一张条件表**、各存一份选择：边灯面积大，适合只在「要你处理」
/// 时亮；角灯是余光级的小记号，多数人愿意让它一直挂着。0.6.4 之前只有四档，
/// 而角灯还能按「置顶 / 静音」这类事实逐枚决定挂不挂 —— 边灯没有对应的入口，
/// 看起来就像「只有角灯有触发条件」。现在两行控件一模一样，九档全都能选。
///
/// 判定在前端 `lampStyle.ts::lightOn`（纯函数、有单测）—— Rust 不参与逐帧渲染，
/// 这里只负责把用户的偏好存下来。默认 `Always` = 加这些字段之前的样子（红线 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightTrigger {
    /// 条件成立就一直亮（角灯的「条件」指它自己那枚的语义，见 [`SignCorner`]）。
    #[default]
    Always,
    /// 只在「需要你动手」的三种状态亮：授权 / 关注 / 出错（`toneOf` 的 `need`）。
    Attention,
    /// 只在还有活动的时候亮；空闲 / 完成 / 沉默 / 离线 / 暂停时整条轨熄掉。
    Active,
    /// 跟随「安静模式」：安静时熄，鼠标一碰灯就回来。
    Quiet,
    /// 达到**指定状态**才亮。挑哪个状态存 `sign_trigger_state`。
    State,
    /// 只在**指定时段**内亮。起止存 `sign_trigger_from` / `sign_trigger_to`
    /// （本地时间的分钟数，0–1439；跨夜也没问题，起 > 止就是「晚上到早上」）。
    Schedule,
    /// 沉默宽限期过半（`stale_progress <= 0.5`）才亮 —— 快被忘掉之前再提醒一次。
    Stale,
    /// 牌上带着消息文字才亮。空消息的行不该被框起来。
    Message,
    /// 只给置顶的组亮（`group_meta[组].order` 有值）。
    Pinned,
}

/// 「达到指定状态」那一档能挑的状态。和 `notify_when` 共用同一套规整（见
/// [`normalize_notify_states`]），只是它多选一，所以这里存一个 `String`。
pub const DEFAULT_LIGHT_TRIGGER_STATE: &str = "permission";
/// 可挑的是**全部**十个聚合状态（含 `offline` / `off`），和通知那张表不同：
/// 通知不该提醒「你暂停了」，但灯完全可以在「离线时把边亮起来」这件事上有意义。
pub const LIGHT_TRIGGER_STATES: [&str; 10] = [
    "offline",
    "off",
    "stale",
    "idle",
    "done",
    "thinking",
    "working",
    "attention",
    "permission",
    "error",
];
/// 指定时段的默认起止：工作时段 09:00–18:00。存的是**分钟数**而不是 `"09:00"`
/// 字符串，因为前端判定的输入就是 `Date` 的小时 × 60 + 分钟，字符串还要再解析一次，
/// 手改过的配置文件里也就不会出现 `"9:0"` 这种半合法值。
pub const DEFAULT_LIGHT_TRIGGER_FROM: u16 = 9 * 60;
pub const DEFAULT_LIGHT_TRIGGER_TO: u16 = 18 * 60;

/// 会话组在牌上叫什么。
///
/// 0.6.3 及以前，服务端把「目录末段 · 智能体」写进 `default_label` 直接画出来。
/// 牌宽固定 `size × 7`，那一行还要和状态词分，于是 `SignalHub · workbuddy` 里的
/// `workbuddy` 几乎每次都被省略号吃掉 —— **留下来的是噪音，被切掉的才是区分度**。
/// 默认改成只显示会话 / 项目名；想看智能体、或想自己写一套标题，都交给用户。
///
/// 只影响**灯上的标题**（前端 `lampSort.ts::labelOf`）。转发摘要 / Web 面板仍用
/// `default_label` 那个全名：它们不受牌宽限制，多一个智能体名是有用的信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignLabel {
    /// 只显示会话名（项目目录末段）。默认。
    #[default]
    Session,
    /// 「会话 · 智能体」—— 0.6.3 及以前的样子。
    WithAgent,
    /// 用 `sign_label_template` 自己拼。
    Custom,
}

pub const DEFAULT_SIGN_LABEL_TEMPLATE: &str = "{session} · {agent}";
pub const MAX_SIGN_LABEL_TEMPLATE: usize = 80;

/// 边框光的粗细。**边灯的线宽和角灯的直径共用这一档** —— 两样分开调，用户
/// 十有八九会调出「一条粗线配四颗小芝麻」，而它们本来就是同一条轨上的东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightScale {
    Thin,
    #[default]
    Normal,
    Thick,
}

/// 边框光的明暗。和 [`LightScale`] 分开，是因为「细而亮」和「粗而淡」是两种
/// 正当的口味：前者适合浅色桌面，后者适合深色桌面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightAlpha {
    Soft,
    #[default]
    Normal,
    Strong,
}

/// 边框光取哪个颜色。`State` = 每块牌自己的状态色（和灯珠同源）；`Neutral` =
/// 近白，让边框只负责「形状」、不抢灯珠那点颜色。
///
/// **不能把它做成 CSS 变量从窗口根上往下传**：状态色是**逐块牌**的（`--state`
/// 挂在 `.sign` 上），自定义属性里的 `var()` 在声明它的那个元素上就求值完了，
/// 根上写 `--light-c: var(--state)` 拿到的是根上的空值。所以这里存的是「取哪一路」，
/// 真正的取值在 `.sign` 那一层完成（见 lamp.css）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightColor {
    #[default]
    State,
    Neutral,
}

// ---- 六个独立灯位（0.6.5） --------------------------------------------------

/// 一条边怎么亮。
///
/// 0.6.5 之前全局只有一个 [`SignEdge`]，四档描述的是**一条轨铺多大范围**，于是
/// 上边和下边永远一模一样，也没法各自挑「什么时候亮」。现在每条边是一个独立
/// 灯位（[`EdgeSlot`]），同时存「画成什么样」和「凭什么亮」。
///
/// 四档都是同一条 `border` 的变体，不引入第二层线；全部画在绝对定位的元素上，
/// **不参与窗口几何**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeLight {
    /// 现状：这条边只有那圈中性描边。
    #[default]
    Off,
    /// 一条实线。
    Line,
    /// 实线 + 牌外一圈同色柔光。
    Glow,
    /// 两端淡出的实线（`mask-image` 做的，不是真把线截短）。
    Fade,
}

/// 一条边的灯位：画法 + 触发 + 闪不闪。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeSlot {
    pub kind: EdgeLight,
    pub trigger: LightTrigger,
    /// 这条边跟着这块牌的**状态节奏**闪（见 [`crate::config::SignLights`] 的注释）。
    ///
    /// 默认 `false` = 常亮。升级上来的旧配置没有这个字段，`serde(default)` 把它
    /// 读成 `false`，观感一个像素都不变。
    pub blink: bool,
}

/// 一个角的灯位：挂什么 + 触发。
///
/// 0.6.5 之前四个角共用一个 `sign_corner_trigger`，一个 gate 让四枚一起亮灭 ——
/// 连「状态色」这种无条件挂的角灯也跟着消失。现在每角各判各的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CornerSlot {
    pub kind: SignCorner,
    pub trigger: LightTrigger,
    /// 这枚角灯闪不闪。语义与 [`EdgeSlot::blink`] 相同。
    pub blink: bool,
}

/// 六个灯位：上边 / 下边 + 左上 / 右上 / 右下 / 左下。
///
/// **这是边框光的唯一真值。** 旧的 `sign_edge` / `sign_corners` /
/// `sign_edge_trigger` / `sign_corner_trigger` 只在读旧 config.json 时存在，
/// `migrate_legacy_light_slots` 会把它们搬进来，之后永不写回。
///
/// **为什么只有上下两条边**：牌是横向的一条，`size * 7` 宽、`size * 1.5` 高，
/// 左右那两截竖边不到 30px，单独给它们各配一套触发只会让设置页多两行看不出
/// 区别的选项。旧模型里 `Frame` / `Glow` 顺带画出的左右两截，迁移时**不保留** ——
/// 那两截本来也不是用户主动选的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SignLights {
    pub top: EdgeSlot,
    pub bottom: EdgeSlot,
    pub tl: CornerSlot,
    pub tr: CornerSlot,
    pub br: CornerSlot,
    pub bl: CornerSlot,
}

impl SignLights {
    /// 挂了几枚角灯（设置页收起时的那行摘要用）。
    pub fn corner_count(self) -> usize {
        [self.tl.kind, self.tr.kind, self.br.kind, self.bl.kind]
            .iter()
            .filter(|kind| **kind != SignCorner::Empty)
            .count()
    }
}

/// 整块牌的轮廓。和 [`SignRadius`]（圆角倍数）是两件事：圆角调的是「四个角
/// 多圆」，轮廓换的是「这块牌整体什么形状」。
///
/// **只能用 `border-radius` 实现，不能用 `clip-path`**：角灯和边框光是绝对定位在
/// 牌的 bounding box 四角 / 四边上的，clip-path 会把它们一起裁掉。每档另外给
/// `--corner-arc` 一个覆盖值，让角灯继续贴着边角走（见 `lamp.css`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignOutline {
    /// 遵守 `sign_radius`。默认 —— 升级不改观感。
    #[default]
    Rect,
    /// 胶囊：两端全圆。
    Capsule,
    /// 对角圆：左上 / 右下圆，另两角近直。
    Leaf,
    /// 拱形：上方两角圆，下方两角近直。
    Arch,
}

/// 动效风格。
///
/// 0.6.5 之前只有一个 `lively: bool`：要么「柔光呼吸 + 弹簧」全开，要么全关。
/// 现在是多选一；[`Effect::Breathe`] 就是原来那个开关开着的样子，所以升级不改
/// 观感。系统开了「减少动态效果」时，五档一律降级成静止（见 `lamp.css` 末尾）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// 完全静止，逐像素回到 0.4.4 的样子。
    Off,
    /// 柔光呼吸 + 状态跃迁涟漪 + 弹簧转场（原「灵动效果」）。
    #[default]
    Breathe,
    /// 灯珠随状态节奏轻微缩放，牌边一次脉冲。
    Pulse,
    /// 一道高光周期性扫过牌面。
    Sweep,
    /// 整块牌随状态节奏淡入淡出。
    Fade,
}

/// 多块牌之间的间距。
///
/// 这是外观里**唯一会影响窗口几何**的一档：牌竖着叠，间距变大窗口就变高，
/// 所以只能给三个离散档 —— `window_size` 必须把同一个倍率算进总高，CSS 的
/// 行距与这里的 `gaps` 要逐像素一致（见 [`SignGap::px`]）。
///
/// **0.6.7 起 CSS 那边的形态变了，公式没变**：多行共享一块 `.sign-board`，
/// 间距从「窗口里两张卡之间露出桌面的透明空隙」变成「板内每行自己的底部
/// 留白」（`--row-gap` 由 `.sign-board > .sign:not(:last-child)` 消化）。
/// `stacked + gap×(N−1) + LAMP_MARGIN` 一个字都不用改 —— 这也是拼接板能
/// 不动 Rust 的前提。
///
/// 默认 `Standard` = 现状 `size / 2`（红线 3：升级不改观感）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignGap {
    /// 0.5×：多组时更紧凑，一屏能多摆两块牌。
    Tight,
    /// 现状：`size / 2`。
    #[default]
    Standard,
    /// 2×：行与行之间留出呼吸，一眼分得清。
    Loose,
}

impl SignGap {
    /// 相对 `size / 2` 的倍率。`Standard` 是 1.0 —— 恒等，不能是别的值。
    pub fn mult(self) -> f32 {
        match self {
            SignGap::Tight => 0.5,
            SignGap::Standard => 1.0,
            SignGap::Loose => 2.0,
        }
    }

    /// 这个档位实际的间距（px）。
    ///
    /// **先取整再乘、乘完向上取整**：`Standard` 必须逐字等于旧公式 `size / 2`
    /// 的整数结果，否则默认设置下窗口就比牌多一像素。向上取整的方向和
    /// `sign_width` 一致 —— 窗口只许多留，不许裁。
    ///
    /// CSS 那边是 `.sign` 的 `--row-gap: calc(size / 2 * var(--sign-gap))`（小数）。
    /// `Tight` 在偶数尺寸下会差半个像素（11 × 0.5 = 5.5 vs 这里 6）—— 窗口比内容高 0.5px，
    /// 那是留白，不会裁到牌。
    pub fn px(self, size: u16) -> u16 {
        ((size / 2) as f32 * self.mult()).ceil() as u16
    }
}

/// 一条出站钩子：聚合变成 `when` 里的某个状态时，执行 `action`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hook {
    /// 哪些状态变化时触发。**空 = 任何变化都触发。**
    ///
    /// 值是状态名，含 `stale` / `offline` 这类派生状态 —— 和 `colors` 一样
    /// 单独用 `Signal::ALL_NAMES` 校验，因为 `Signal::parse` 不接受派生状态。
    #[serde(default)]
    pub when: Vec<String>,
    /// 限定只看哪几个组的跃迁。**空 = 看全局聚合。**
    ///
    /// 两种语义是有意的，不是二选一：
    ///
    /// - **空**：关心「整体现在出事了没有」。一组在等授权、另一组在跑，整体是
    ///   `permission` —— 这时不该因为「某个组变成 error 了」就发。
    /// - **非空**：关心「这一组怎么了」。于是每个组**各自的跃迁**都算数，不必等
    ///   它成为全局最高的那个。想「ci 组失败发到 A、deploy 组失败发到 B」就得用它。
    ///
    /// 之所以不去掉第一种：现有的钩子都是按全局聚合配的，改掉会让用户已经配好的
    /// 自动化静默变行为 —— 那是这类改动里最不该发生的事。
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(flatten)]
    pub action: HookAction,
}

/// 钩子能做的事。
///
/// `Command` 会执行本地命令。信任模型和 git hooks / npm scripts 一样：配置文件
/// 是用户自己的，能改它就能执行任何东西。所以不额外加开关 —— 多一个开关只会
/// 给人一种「关掉就安全」的错觉，而配置文件本来就在用户手里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    Http {
        url: String,
        /// 额外请求头。**鉴权靠它** —— `Authorization: Bearer …`、ntfy 的
        /// `X-Token`。没有它的话，任何要鉴权的地址都配不了。
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// 服务预设：决定 content-type 和 body 的形状（见 [`HttpPreset`]）。
        ///
        /// `#[serde(default)]` 是「老配置照旧」那条契约的落点：
        /// `{"type":"http","url":"…"}` 解析出来就是 `raw`，一个字节都不变。
        #[serde(default)]
        preset: HttpPreset,
        /// **自定义请求体模板**（只对 `preset: "custom"` 有意义）。
        ///
        /// 占位符写成 `{state}` 这样，完整列表见 [`TEMPLATE_PLACEHOLDERS`]。
        /// content-type 是 JSON 时，替换进去的值会做 JSON 字符串转义 —— 于是
        /// `{"text":"{message}"}` 里出现引号也不会把整条 body 弄坏。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// 自定义 content-type（只对 `custom` 有意义）。空 = `application/json`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    Command { command: String },
}
/// 自定义请求体里能用的占位符（不带花括号）。
///
/// **这就是全部**：多一个占位符意味着多一处要向用户解释、多一处要在别的语言里
/// 同步。缺什么再加，别提前铺。
pub const TEMPLATE_PLACEHOLDERS: [&str; 8] = [
    "state",
    "aggregate",
    "group",
    "message",
    "duration",
    "since_ms",
    "at_ms",
    "summary",
];

/// 自定义模板的上限。一条通知的 body 到 4 KB 已经非常宽裕；再长多半是把整段
/// 文档粘进来了。
pub const TEMPLATE_MAX_CHARS: usize = 4096;

/// 自定义 content-type 的上限。它就是一个 MIME，128 字符足够。
pub const CONTENT_TYPE_MAX_CHARS: usize = 128;

/// `preset: "custom"` 没填 content-type 时用什么。
pub const DEFAULT_CUSTOM_CONTENT_TYPE: &str = "application/json";

/// HTTP 转发的**服务预设**：决定 content-type 和 body 形状。
///
/// 起因是一条很具体的反馈：照着占位符填了 ntfy，收到的是一大坨 JSON。默认发的
/// `raw` 是 SignalHub 自己的快照 schema，而现实里做通知的那些服务（Slack /
/// Discord / Telegram / 飞书 / 企业微信 / 钉钉 / Bark / ntfy）要的都是
/// **一个 URL + 一段文字**，服务端自己决定怎么排版。
///
/// **默认 `raw`**：新能力默认关（红线 3），也保证老的 `{"type":"http","url":…}`
/// 行为一个字都不变。每个预设的形状都在 `outbound::compose` 里，那是唯一一处实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpPreset {
    /// 原样发 SignalHub 自己的 JSON 快照（默认）。给「自己写了个接收端」的人用。
    #[default]
    Raw,
    /// 一行纯文本。任何「贴一行字」的地址都吃（ntfy / Bark / 自己的脚本）。
    Text,
    /// 同文本，另加 `Title` 头 —— ntfy 用它当通知的标题。
    Ntfy,
    /// Bark（iOS 推送）
    Bark,
    /// Slack incoming webhook：`{"text": …}`
    Slack,
    /// Discord webhook：`{"content": …}`
    Discord,
    /// 飞书自定义机器人：`{"msg_type":"text","content":{"text": …}}`
    Lark,
    /// 企业微信群机器人：`{"msgtype":"text","text":{"content": …}}`
    Wecom,
    /// 钉钉自定义机器人：形状同企业微信
    Dingtalk,
    /// Telegram Bot API。`chat_id` 写在 URL 的查询串上
    /// （`…/bot<token>/sendMessage?chat_id=<id>`），发送时搬进表单体。
    Telegram,
    /// PushDeer（官方服务器或自架）。`pushkey` 写在 URL 的查询串上
    /// （`https://api2.pushdeer.com/message/push?pushkey=<key>`），正文按表单体发。
    Pushdeer,
    /// **自定义请求体**：用户自己写模板，占位符见 [`TEMPLATE_PLACEHOLDERS`]。
    ///
    /// 存在的理由很直接：预设覆盖的是「现成服务」，而接自建接口的总要一个
    /// 自己的形状（某个内部通知服务、某个只认 `{"text": …}` 的网关）。
    /// 模板和 content-type 在 `HookAction::Http` 上，见那里。
    Custom,
}

impl Hook {
    /// 这次聚合变化该不该触发它（只看 `when` 那部分）。
    pub fn matches(&self, aggregate: &str) -> bool {
        self.when.is_empty() || self.when.iter().any(|name| name == aggregate)
    }

    /// 这次该不该触发它。
    ///
    /// `aggregate` 是全局聚合，`transitioned` 是全局聚合**本次有没有跃迁**，
    /// `changes` 是本次发生跃迁的组（组名 + 新状态）。两条路径的区别见 `groups`
    /// 字段的说明。
    pub fn fires(
        &self,
        aggregate: &str,
        transitioned: bool,
        changes: &[(String, crate::state::Signal)],
    ) -> bool {
        if self.groups.is_empty() {
            // 不限定组：只在**全局聚合跃迁**时触发。这个语义是从一开始就有的，
            // 去掉它会让用户已经配好的钩子变成「任何组一变就发」—— 静默改行为
            // 是这类改动里最不该发生的事。
            return transitioned && self.matches(aggregate);
        }
        changes.iter().any(|(name, state)| {
            self.groups.iter().any(|group| group == name) && self.matches(state.name())
        })
    }
}

/// **宽松解析**：认识的值照收（大小写不敏感），**不认识的落到 `raw`**。
///
/// 不是随手加的宽容。`Settings` 是一个**整体**：任何一个字段解析失败都会让它整份
/// 退回默认值（`load_settings` 那个兜底）。而手写配置里把 `"slack"` 写成 `"Slack"`、
/// 或者照着别的工具写成 `"webhook"`，都是很正常的事 —— 代价却是用户丢掉**全部**设置
/// （组显示名、配色、所有钩子）。退回一个预设，用户看到的是「发出去的是 JSON」，
/// 而设置面板上那个下拉就明写着「原样 JSON」，改一下就好了。
///
/// 序列化永远是那几个小写名字（`#[serde(rename_all = "lowercase")]`）。
impl<'de> Deserialize<'de> for HttpPreset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "text" => HttpPreset::Text,
            "ntfy" => HttpPreset::Ntfy,
            "bark" => HttpPreset::Bark,
            "slack" => HttpPreset::Slack,
            "discord" => HttpPreset::Discord,
            "lark" => HttpPreset::Lark,
            "wecom" => HttpPreset::Wecom,
            "dingtalk" => HttpPreset::Dingtalk,
            "telegram" => HttpPreset::Telegram,
            "pushdeer" => HttpPreset::Pushdeer,
            "custom" => HttpPreset::Custom,
            _ => HttpPreset::Raw,
        })
    }
}

impl HookAction {
    /// 目标填了吗。没填的钩子不触发、也不报错 —— 编辑时必然要经过
    /// 「刚添加、还没填目标」这个中间态，那时候报错就没法编辑了。
    pub fn is_configured(&self) -> bool {
        match self {
            HookAction::Http {
                url,
                preset,
                body,
                ..
            } => {
                if url.trim().is_empty() {
                    return false;
                }
                // 自定义模板空着时**不算配好**：否则用户选了「自定义」还没写 body，
                // 我们就会往他的地址发一个空请求 —— 对方收到什么、算不算错都不确定。
                // 和「刚添加还没填地址」同一种中间态，静默跳过才对。
                *preset != HttpPreset::Custom
                    || body.as_deref().is_some_and(|text| !text.trim().is_empty())
            }
            HookAction::Command { command } => !command.trim().is_empty(),
        }
    }

    /// 目标本身（网址或命令）。
    ///
    /// 日志和界面都**按它去认一条钩子**，而不是按下标 —— 钩子列表可以增删排序，
    /// 下标会错位，于是会显示成「另一条钩子的结果」，比不显示更糟。
    pub fn target(&self) -> String {
        match self {
            HookAction::Http { url, .. } => url.clone(),
            HookAction::Command { command } => command.clone(),
        }
    }

    /// 一条只要网址的 HTTP 钩子（预设 `raw`、不带额外头）。
    ///
    /// 给「就要默认行为」的调用方和测试一个短写法：多出来两个字段之后，每处都写全
    /// `Http { url, headers: BTreeMap::new(), preset: HttpPreset::Raw }` 会把真正的
    /// 意图（「默认那条路」）埋在一片噪音里。
    pub fn http(url: impl Into<String>) -> Self {
        HookAction::Http {
            url: url.into(),
            headers: BTreeMap::new(),
            preset: HttpPreset::default(),
            body: None,
            content_type: None,
        }
    }
}

/// 一台**物理灯**（目前只支持 WLED）。
///
/// 挂在「转发」页里而不是新开一个页签：它和出站钩子是同一件事 ——
/// **SignalHub 主动往外发**，区别只是发的东西不一样（那边发一段文字，这边发一个
/// 颜色）。为一个功能多开一个页签的成本比这个功能本身还大。
///
/// **只有地址。** 没有开关：填了地址就发、地址空着就不发，和钩子一个脾气
/// （见 [`HookAction::is_configured`]）。多一个开关只会多出一种「配置看起来在、
/// 实际不发」的哑谜；要停掉一盏灯就把它删了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// 灯的地址。**只填主机**（含端口），如 `192.168.1.50`、`wled.local:8080`、
    /// `http://192.168.1.50`。路径是我们拼的（`/json/state`）。
    ///
    /// 带不带 `http://` 都认；顺手把 `/json/state` 粘进来也认（见
    /// [`crate::devices::url`]）。
    #[serde(default)]
    pub host: String,
}

/// 一台灯配置上的问题。空地址不算错（「刚添加、还没填」是编辑时必然经过的中间态）。
///
/// 只挡**明显的粘贴事故**（地址里带了空格 —— 多半是把「192.168.1.50 的灯」整句
/// 粘进来了）。刻意不校验 IP 形状：`wled.local` 这类 mDNS 名字是正经用法，而
/// 「地址对不对」只有真发一次才知道 —— 那是日志那行 `ok 200` 要回答的。
pub fn device_problem(device: &Device) -> Option<String> {
    let host = device.host.trim();
    if host.is_empty() {
        return None;
    }
    if host.chars().any(char::is_whitespace) {
        return Some(format!("灯的地址里不能有空格：{:?}", device.host));
    }
    None
}

/// 另一台机器 —— 多机中枢（见 `docs/plan-multi-machine.md`）。
///
/// 方向和其他所有配置项都相反：钩子 / 物理灯是**我们往外发**，这个是**我们往外读**。
/// 拉而不是推的决定性理由是「对端不用升级」：`GET /v1/state` 从 0.1 就有，
/// 所以一台 0.4.1 的机器立刻能当对端（见方案 §1）。
///
/// **只有三样东西，没有「协议版本」**：对端就是 SignalHub 自己的 HTTP 接口，
/// 给它加一个版本号只会多出一处要维护的兼容矩阵。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// 显示名，同时也是**命名空间**（远程会话的 id 和组名都挂在它下面）。
    ///
    /// 所以它必须非空、唯一，而且**改了等于换了一台机器** —— 旧会话会以新名字重新
    /// 进来，旧的那些在 TTL 到期后自然消失。这条写进文档，别让人以为改名是「重命名」。
    ///
    /// 允许中文 / 空格：它只出现在界面上和组名里，从不进 URL。
    #[serde(default)]
    pub id: String,
    /// 对端地址。`192.168.1.20:41320`、`http://host:port`、`https://…` 都认；
    /// 顺手把 `/v1/state` 粘进来也认（见 `remote::endpoint`）。
    ///
    /// **空着保留**，和 [`Device::host`] 一个脾气：那是「刚点了添加、还没填」这个
    /// 编辑时必然经过的中间态，存下去被丢掉的话，界面上就是「刚点出来的那一行自己
    /// 没了」。真正轮询时按空地址过滤。
    #[serde(default)]
    pub url: String,
    /// 对端的 token（对端机器上 `~/.signalhub/token` 的内容）。
    ///
    /// 明文存在 `config.json` 里，和本机 token 同一个待遇 —— 它本来就在同一个目录、
    /// 同样的权限下。**不额外加密**：为此引一个密钥链依赖，收益只是「拿到 config.json
    /// 的人多花五分钟」，而那个前提已经是「他已经能读你的家目录」了。
    #[serde(default)]
    pub token: String,
    /// 关掉但留着配置。默认 `true` —— 一条被加进来的机器就是「我要看它」，
    /// 加完还要再点一次开关才生效是多余的一步。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// `serde` 的 `default` 需要一个返回 `true` 的函数（`bool::default()` 是 `false`，
/// 而 [`Machine::enabled`] 的缺省必须是 `true`）。
fn default_true() -> bool {
    true
}

/// 能配几台机器。
///
/// 上界不是防性能（20 台 × 2 秒一次也就 10 req/s），而是防**误操作**：把一整段
/// 主机名列表粘进来、或者脚本反复往配置里追加时，界面会变成一屏滚不到头的输入框，
/// 而每一台都在真的发请求。
pub const MAX_MACHINES: usize = 20;

/// Call Me —— 用户自部署的收信服务器（见 `docs/callme.md`）。
///
/// 和 [`Machine`] 一样是**往外读**：桌面端在 NAT 后面，公网服务器推不进来，
/// 只能由我们定时去拉。区别是拉回来的东西**不进 `Store`** —— 留言不是状态信号，
/// 没有 TTL、不参与聚合、不该闪灯；它在设置面板的「Call Me」页里单独阅读。
///
/// **默认空 = 一条请求都不发**（红线：新增外发默认关）。旧版 `config.json` 里
/// 没有这个字段时，容器级 `serde(default)` 会落到 [`CallMeConfig::default`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CallMeConfig {
    /// 服务器地址，例如 `https://callme.example.com`。
    ///
    /// 不写 scheme 时按 **https** 补 —— 这是公网服务，http 只应该由用户显式写出。
    /// 空着 = 功能关闭（不发任何请求）。
    #[serde(default)]
    pub url: String,
    /// 服务器上的 `CALLME_ADMIN_TOKEN`。
    ///
    /// 明文存在 `config.json` 里，和多机中枢的 `Machine::token` 同一个待遇：
    /// 它们本来就在同一个目录、同样的权限下。
    #[serde(default)]
    pub token: String,
    /// 轮询间隔（毫秒）。留言不是实时信号，默认 15 秒。
    #[serde(default = "default_callme_poll_ms")]
    pub poll_ms: u64,
}

impl Default for CallMeConfig {
    fn default() -> Self {
        CallMeConfig {
            url: String::new(),
            token: String::new(),
            poll_ms: default_callme_poll_ms(),
        }
    }
}

impl CallMeConfig {
    /// 是否配置完整。**两个都填了才算开**：只有地址没有 token 时请求必然 401，
    /// 每 15 秒打一次只会给服务器添乱，也在客户端制造一条永远消不掉的错误。
    pub fn enabled(&self) -> bool {
        !self.url.trim().is_empty() && !self.token.trim().is_empty()
    }
}

fn default_callme_poll_ms() -> u64 {
    15_000
}

/// 轮询间隔下限。比多机中枢（1 秒）宽松得多：那边是状态，掉线要在半分钟内显形；
/// 这边是留言，15 秒一次已经算及时，再密只是白耗电。
pub const MIN_CALLME_POLL_MS: u64 = 5_000;
pub const MAX_CALLME_POLL_MS: u64 = 300_000;


/// 轮询间隔的可调范围（毫秒）。
///
/// 下界 1 秒：再快就只是给对端增加无意义的负载，而人眼分辨不出 1 秒以内的差别。
/// 上界 10 秒：TTL 是它的 3 倍（见 `remote`），10 秒意味着掉线要 30 秒才在灯上
/// 显形 —— 再慢就失去「及时发现」的意义了。
pub const MIN_MACHINE_POLL_MS: u64 = 1000;
pub const MAX_MACHINE_POLL_MS: u64 = 10_000;

/// 一台机器配置上的问题。空地址不算错（理由见 [`Machine::url`]）。
///
/// 只挡**明显的粘贴事故**：地址里有空格。不校验形状 —— `wled.local` 那样的
/// mDNS 名字、`https://` 的反代都是正经用法，「通不通」只有真拉一次才知道，
/// 那是「测试连接」要回答的。
pub fn machine_problem(machine: &Machine) -> Option<String> {
    let url = machine.url.trim();
    if url.is_empty() {
        return None;
    }
    if url.chars().any(char::is_whitespace) {
        return Some(format!("地址里不能有空格：{:?}", machine.url));
    }
    if machine.id.trim().is_empty() {
        return Some("还没有填名称 —— 名称是这台机器在灯上的命名空间".to_string());
    }
    None
}

/// 额外请求头的条数上限。
///
/// 转发目标（尤其是别人给你的一个 webhook 地址）算**不可信输入**：一个几千条的
/// map 会让每次转发都多花一大截时间在拼头上，而正常的鉴权只需要一两条。
pub const MAX_HOOK_HEADERS: usize = 16;

/// 规整额外请求头：去空白、丢掉空键、按上限截断。
///
/// **不做「只差大小写」的去重**：HTTP 头名是大小写不敏感的，但要判断
/// `Authorization` 和 `authorization` 哪个是「后写的那条」，得知道用户写下的
/// **顺序** —— 而 `BTreeMap` 已经把顺序排掉了，任何胜者都是任意挑的。宁可两条都发
/// （服务器会用一个状态码告诉我们它不接受），也不要悄悄丢掉一条用户明确写下的鉴权头。
/// 编辑界面在输入那一层挡重复的键，那里顺序还在。
///
/// 截断按**键的字典序**（`BTreeMap` 的顺序），所以丢哪几条是确定的，不是随机。
fn normalize_headers(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some((name, value.trim().to_string()))
        })
        .take(MAX_HOOK_HEADERS)
        .collect()
}

/// 展开的牌上显示哪几段内容。五段都可以单独关掉 —— 有人只要一眼颜色，
/// 有人要看清「卡在哪一步」，也有人只想要一块纯文字卡。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SignFields {
    /// 组名
    pub name: bool,
    /// 状态名（如「需授权」）
    pub state: bool,
    /// 已持续多久
    pub age: bool,
    /// 消息 —— 关掉之后牌永远是单行高
    pub message: bool,
    /// 状态圆点。关掉之后牌就是一块纯文字卡，状态只剩文字（和牌底的粗细分层）。
    ///
    /// **不影响几何**：圆点本来就在牌的 flex 行里，关掉它只是让文字往左挪 ——
    /// 牌宽高仍由 `sign_width` / `sign_height` 算。默认开（红线 3）。
    pub dot: bool,
}

impl Default for SignFields {
    fn default() -> Self {
        // 默认全开：牌的信息量就是它存在的理由
        Self { name: true, state: true, age: true, message: true, dot: true }
    }
}

/// 某一个状态的个性化外观。三样都可以单独覆盖，**没写的用全局默认**。
///
/// 为什么是「可选的字段」而不是「填满的表格」：默认值会随版本演进（新增状态、
/// 调整默认节奏、改配色），如果用户配置文件里存着一份填满的表，那些改进就永远
/// 到不了他那儿。空着 = 跟随，是唯一能让默认值继续演进的表达。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StateStyle {
    /// 颜色 `#rrggbb`。没写 = 用配色方案里那个状态的默认色。
    pub color: Option<String>,
    /// 灯珠形状。没写 = 用全局形状（全局是「圆」时 permission / error 仍会
    /// 画成圆角方，那是给色盲用的形状区分）。
    pub shape: Option<Shape>,
    /// 闪烁周期（毫秒）。**0 = 常亮**。没写 = 按全局节奏倍率算默认周期。
    pub blink_ms: Option<u16>,
    /// 停留时长（毫秒）：这个状态**静默多久算过期**。没写 = 用内置默认
    /// （`state::Signal::default_ttl`，如工作态 5 分钟、空闲态 10 分钟）。
    ///
    /// 和上面三样不同，它**不是外观**：它决定「智能体不出声之后，它的牌还在灯上
    /// 待多久」。放在同一张表里是因为调节它的场景就是「我盯着某一个状态觉得它
    /// 消失得太早 / 太晚」—— 那时人已经在看这张表了。
    ///
    /// 它只在**上报进来的那一刻**算一次，不追溯已存在的会话（见 `state::state_ttl`）。
    pub ttl_ms: Option<u32>,
}

impl StateStyle {
    /// 一样都没写。这种条目没有任何作用，`clamped()` 会把它们丢掉 ——
    /// 留着会让「预设是否还生效」的判断无谓地失败。
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.shape.is_none() && self.blink_ms.is_none() && self.ttl_ms.is_none()
    }
}

/// 勿扰时段。这段时间里**不弹通知、不出声**，但灯照常显示 —— 勿扰是「别吵我」，
/// 不是「别告诉我」。
///
/// **只有小时，没有分钟**：分钟级精度只会让配置变麻烦，而「晚上十点到早上八点
/// 别吵我」这种需求本来就不需要分钟。
///
/// 起止都按**用户本地时间**。本地时区只有前端知道（Rust 这边没有时区数据，
/// `SystemTime` 只给 UTC），所以前端每分钟把当前小时推给 Rust，**由 Rust 判断在
/// 不在窗口内** —— 策略留在 Rust 是为了能单测，前端只提供「现在是几点」这个事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHours {
    pub enabled: bool,
    /// 起始小时（0–23）
    pub start: u8,
    /// 结束小时（0–23），**不含**这一小时
    pub end: u8,
}

impl Default for QuietHours {
    fn default() -> Self {
        // 22:00–08:00。enabled 默认 false，但这个窗口是最常见的那个 ——
        // 用户一打开开关就是个合理的值，不用先调时间。
        Self { enabled: false, start: 22, end: 8 }
    }
}

impl QuietHours {
    /// 这个小时算不算「勿扰」。
    ///
    /// 跨午夜（`start > end`）是常态而不是特例，所以必须分两段判断：
    /// 22–8 表示 `[22, 24) ∪ [0, 8)`。
    pub fn covers(&self, hour: u8) -> bool {
        if !self.enabled {
            return false;
        }
        // start == end 读作「空窗口」而不是「全天」：一个手滑把两个值调成一样，
        // 后果应该是「没生效」，而不是「从此再也不提醒」。
        if self.start == self.end {
            return false;
        }
        if self.start < self.end {
            hour >= self.start && hour < self.end
        } else {
            hour >= self.start || hour < self.end
        }
    }
}

/// 界面语言的**偏好**。注意和 `Lang` 的区别：那个是**解析后**的语言，只有中文和
/// 英文；这里的 `Auto` 要由前端（CLI 那边则由环境变量）解析成具体语言 ——
/// Rust 不知道系统语言。
///
/// 定义在这个文件而不是 `lang.rs`：`Settings` 要被 CLI 通过 `#[path]` 共用，
/// 而 `lang.rs` 是 app 侧的模块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    /// 跟随系统
    #[default]
    Auto,
    Zh,
    En,
}

/// 设置窗口的亮 / 深色偏好。**和 `Locale` 同构**：`Auto` 由**前端**解析成实际外观
/// —— Rust 这边不知道系统现在用的是亮色还是深色，只有前端的
/// `matchMedia("(prefers-color-scheme: dark)")` 说得准。
///
/// 只作用于设置窗口和演示页：灯球 / 收件箱是透明窗口的固定深色 HUD，刻意不读
/// 这套主题变量（理由写在 `src/styles/theme.css` 文件头）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// 跟随系统外观
    #[default]
    Auto,
    Light,
    Dark,
}

/// 解析之后的界面语言。
///
/// **和 `Locale` 放在一起**而不是 `lang.rs`：CLI 也要用它挑自己的文案（`--help`
/// 和报错），而 `lang.rs` 里那套 `NativeText` 是 app 侧的（托盘菜单、系统通知），
/// CLI 一条都用不上 —— 把整个 `lang.rs` 拖进去只会换来一堆 dead_code 警告，而
/// CI 现在是 `-D warnings`。理由和上面 `Locale` 那段是同一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// 默认中文：这是这个项目的母语，也是「谁都没告诉我们」时唯一有依据的猜测。
    #[default]
    Zh,
    En,
}

impl Lang {
    /// 认不出就返回 `None`，由调用方决定怎么办 —— 不在这里猜一个默认值，
    /// 因为「拿到一个不认识的语言」是需要能被发现的事。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh-hans" | "zh-hant" => Some(Lang::Zh),
            "en" | "en-us" | "en-gb" => Some(Lang::En),
            _ => None,
        }
    }
}

/// app 解析出来的**当前界面语言**，供 CLI 读。
///
/// 为什么要有这个文件：CLI 是独立进程，它要挑自己的 `--help` / 报错文案，而
/// `locale: "auto"` 的解析**只有前端做得了**（它读 `navigator.language`）。让 CLI
/// 自己去猜环境变量在 Windows 上会直接落空，也会和界面上实际显示的语言不一致 ——
/// 而这两个**必须一致**：用户在英文界面上敲 `signalhub --help`、看到的却是中文，
/// 会以为装错了版本。
///
/// 和 `port` 是同一类东西：app 写、CLI 读的运行时事实。
pub fn lang_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("lang"))
}

/// app 在 `set_lang` 时写一次（见 `lib.rs`）。
pub fn write_lang(lang: Lang) -> io::Result<()> {
    let text = match lang {
        Lang::Zh => "zh",
        Lang::En => "en",
    };
    atomic_write(&lang_file()?, text, None)
}

/// CLI 读它。**读不到就返回 `None`**（app 从没跑过），由调用方决定退到哪儿。
pub fn read_lang() -> Option<Lang> {
    Lang::parse(&fs::read_to_string(lang_file().ok()?).ok()?)
}

/// 把「偏好 + app 写下的实际语言 + 环境变量」折成一个具体语言。
///
/// 顺序是有讲究的：
///   1. `Settings::locale` 是**显式**的 zh / en → 用它。用户明确选过，最权威。
///   2. 是 `auto` → 用 app 写下的 `lang` 文件。那是界面**实际**在显示的语言。
///   3. 还没有（app 从没跑过）→ 猜环境变量。
///   4. 连环境变量都没有 → `Lang::default()`。
///
/// **`lang` 文件只对 `auto` 生效**：显式偏好排在它前面，所以用户在应用没跑的时候
/// 手改 `config.json` 也立刻有效，不会被上一次运行留下的文件压住。
pub fn resolve_lang(settings: &Settings) -> Lang {
    match settings.locale {
        Locale::Zh => Lang::Zh,
        Locale::En => Lang::En,
        Locale::Auto => read_lang().or_else(lang_from_env).unwrap_or_default(),
    }
}

/// 从环境变量猜系统语言。
///
/// 按 POSIX 的优先级取：`LC_ALL` > `LC_MESSAGES` > `LANG`。**`LANG` 在 Windows 上
/// 通常不存在**，所以这条路只对 Unix 可靠 —— Windows 靠的是上面那个 `lang` 文件，
/// 而 app 只要跑过一次就会把它写下来。
///
/// 取不到就继续往下试，**不因为 `LC_ALL=C` 或空串而放弃**：那两个都表示「没有
/// 指定 locale」，而不是「指定成某种不认识的语言」。
fn lang_from_env() -> Option<Lang> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key)
            && let Some(lang) = Lang::parse(&normalize_posix_locale(&value))
        {
            return Some(lang);
        }
    }
    None
}

/// POSIX 的 locale 写法 → `Lang::parse` 认得的那种。
///
/// `en_US.UTF-8` → `en-US`：去掉 `.` 之后的编码，下划线换成连字符。`Lang::parse`
/// 认的是 `navigator.language` 那一套（`en-US`），两边只差这一层。
pub fn normalize_posix_locale(raw: &str) -> String {
    // `@` 是修饰符（`sr_RS@latin`），和编码一样丢掉
    let base = raw.split(['.', '@']).next().unwrap_or("");
    base.replace('_', "-")
}

/// 某一组的显示偏好。
///
/// **只影响灯上怎么显示**，不影响聚合、通知和钩子 —— 组名是上报方给的标识
/// （`build-1`、`ci`），这里改的是「怎么称呼它、排在第几位」，不是「它是什么」。
/// 隐藏 / 静音的匹配也仍然按**原始组名**，不按显示名。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupMeta {
    /// 显示名。没写就用上报方给的原始组名。
    pub name: Option<String>,
    /// 置顶权重。`Some(n)` = 置顶，越小越靠前；`None` = 不置顶，按紧急程度排。
    ///
    /// 用 `Option<i16>` 而不是 `pinned: bool`：置顶的组之间也要有确定的顺序，
    /// 而「先置顶的排前面」正是用户按下开关的顺序 —— 那就不必再做一个拖拽界面。
    pub order: Option<i16>,
}

impl GroupMeta {
    /// 两样都没写。这种条目没有任何作用，`clamped()` 会把它们丢掉。
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.order.is_none()
    }
}

/// 用户设置。**这份文件是用户可手改、也会跨版本留存的**，所以带 `serde(default)`：
/// 缺字段时退回该字段的默认值，而不是整份解析失败 —— 否则每次新增设置项，都会把
/// 用户已经调好的其他设置一起重置。
///
/// 注意它是**容器级**的：加新字段**不需要**逐字段写 serde 属性，只要在下面那个
/// 手写的 `Default` 里给出想要的默认值。
///
/// 不派生 `Copy`：`webhook_url` 是 `Option<String>`。相关方法因此都取 `&self`。
///
/// 也**不派生 `Eq`**：牌宽 / 牌高是 `f32` 倍率，而 `f32` 不是 `Eq`（NaN ≠ NaN）。
/// 需要比较的地方（导出 / 导入的往返测试）用 `PartialEq` 就够了。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 圆点直径（px）
    pub size: u16,
    /// 牌宽倍率。**1.0 = 现状**（`size * 7`）。
    ///
    /// 只作用于**展开态**的牌 —— 折叠态是一排正方形点，长宽倍率对它没有意义
    /// （写了也只会让点变椭圆）。窗口尺寸照旧按内容算（见 `apply_window_size`），
    /// 这个倍率只是把「内容」本身放大或缩小。
    pub sign_w_mult: f32,
    /// 牌高倍率。**1.0 = 现状**（一行 `size * 3/2`，带消息两行 `size * 5/2`）。
    ///
    /// 两档都乘 —— 漏一档窗口就会裁掉带消息的那块牌。
    pub sign_h_mult: f32,
    /// 多灯布局。默认 `Single`（一盏灯画一叠牌），`PerGroup` 时每个可见组一盏灯。
    pub lamp_layout: LampLayout,
    pub always_on_top: bool,
    /// 拖完之后若窗口离屏幕**左 / 右**边缘 12px 以内，就吸附过去贴齐。
    ///
    /// **默认关**（红线 3）：吸附会改变「拖到哪就是哪」的既有手感，得由用户明确
    /// 打开。只在左右吸 —— 上下是任务栏 / Dock 的地盘，吸过去会和 `keep_on_screen`
    /// 的底边夹取抢地方。
    pub snap_to_edge: bool,
    /// 「有事才显形」：空闲 / 沉默 / 离线时把圆点压到几乎透明，
    /// 只有真正有活动或需要你注意时才完全显形。
    pub quiet: bool,
    /// 需要你注意（授权 / 关注 / 出错）时弹系统通知。
    pub notify: bool,
    /// 哪些状态变化时弹系统通知 —— `notify` 总开关之下的**一层过滤**。
    ///
    /// 默认就是加这个字段之前的行为（`DEFAULT_NOTIFY_STATES`），所以从旧版本
    /// 升级上来的人不会被 `working` / `done` 突然吵到。
    ///
    /// **空 = 一条都不弹。** 和 `hooks[].when` 的「空 = 任何变化」刻意不同：
    /// 那里空是「不过滤」，这里要是照搬，等于给每个状态都弹通知。
    #[serde(default = "default_notify_when")]
    pub notify_when: Vec<String>,
    /// **已废弃**，只为读得懂旧配置。启动时会搬成一条 `hooks` 条目
    /// （见 `migrate_legacy_webhook`），并且**永不写回** —— 出口只有一条路，
    /// 不让「单条 webhook」和「钩子列表」两种表达并存。
    #[serde(skip_serializing)]
    pub webhook_url: Option<String>,
    /// 出站钩子。聚合变化时按 `when` 过滤后依次触发。
    pub hooks: Vec<Hook>,
    /// 物理灯（WLED）。聚合变化时把**状态色**发给它们。
    ///
    /// **默认空 = 不发**（红线 3：新增外发默认关）。里面只有地址，理由见 [`Device`]。
    ///
    /// 发出去的东西只有颜色：**没有组名、没有会话名、没有路径、没有消息内容**
    /// （红线 3）。灯可能挂在别人也看得见的地方，而 WLED 自己的界面会把收到的
    /// 东西显示出来。
    pub devices: Vec<Device>,
    /// **从灯上审批工具调用**（C3）。默认关。
    ///
    /// 这是全项目唯一会**改变智能体行为**的开关：打开之后，Claude Code 每次要问你
    /// 批准某个工具调用时，那个确认框会被搬到灯上（`PermissionRequest` hook →
    /// `POST /v1/approve/claude-code`），你在灯上点允许 / 拒绝，或者不管它 ——
    /// 不管的时候交回 Claude Code 自己问。
    ///
    /// 默认关的理由不是「没做完」，而是它**会挂住智能体**（最多
    /// `approvals::WAIT`）：这是一条只有用户明确想要时才该生效的路。关着的时候
    /// 端点照常回话但什么都不说，所以装了 hook 也不会改变任何行为。
    pub approve_tools: bool,
    /// Time Machine 备份中 → 「备份」组（B2 的内置采集器）。
    ///
    /// 默认关：采集器读的是**这台机器的状态**，而「谁在备份」这件事跟大多数人
    /// 无关 —— 要它的人去接入页打开。
    pub collect_backup: bool,
    /// 会议中（麦克风 / 摄像头被某个进程使用）→ 「会议」组（B2 的内置采集器）。
    ///
    /// 默认关，理由同上。它**只在 macOS 上有**（CoreAudio / CoreMediaIO 的
    /// `DeviceIsRunningSomewhere`）—— 别的平台上这条采集器根本不出现在接入页。
    pub collect_meeting: bool,
    /// 圆点形状
    pub shape: Shape,
    /// 配色方案（用户没单独指定颜色时的默认值）
    pub palette: Palette,
    /// 牌面圆角 / 描边 / 底色 / 字号 / 分层强度 —— 「质感」那五档。
    ///
    /// 全部**只改 CSS**：牌宽高仍由 `sign_width` / `sign_height` 算，
    /// 灯珠颜色仍归 `palette` 与 `styles[].color`。所以这五项可以随便调，
    /// 不会出现「窗口裁掉最后一块牌」。
    pub sign_radius: SignRadius,
    pub sign_border: SignBorder,
    pub sign_tint: SignTint,
    pub sign_text: SignText,
    pub sign_emphasis: SignEmphasis,
    /// 牌内字体：系统无衬线 / 整块等宽。只改字形，不改几何。
    pub sign_font: SignFont,
    /// 多块牌之间的间距档位。**唯一会影响窗口高度的外观项**（见 [`SignGap`]）。
    pub sign_gap: SignGap,
    /// **已废弃**：0.6.5 之前的一整条边灯。只为读得懂旧配置，
    /// `migrate_legacy_light_slots` 会搬进 `sign_lights`，并且**永不写回**。
    #[serde(skip_serializing)]
    pub sign_edge: SignEdge,
    /// **已废弃**：0.6.5 之前四角只存「挂什么」。同上。
    #[serde(skip_serializing)]
    pub sign_corners: SignCorners,
    /// **已废弃**：0.6.5 之前边灯 / 四角各一个触发条件。同上。
    #[serde(skip_serializing)]
    pub sign_edge_trigger: LightTrigger,
    #[serde(skip_serializing)]
    pub sign_corner_trigger: LightTrigger,
    /// 六个独立灯位：上边 / 下边 + 四个角，每个各自有画法与触发条件（见
    /// [`SignLights`]）。**这是边框光的唯一真值**，纯 CSS，不参与窗口几何。
    /// 外观页管「画成什么样」，行为页管「凭什么现在亮」，但都存在同一个槽里 ——
    /// 拆成两张表的话，换一档画法会把触发条件悄悄重置。
    pub sign_lights: SignLights,
    /// 触发条件配套的三样参数，**六个灯位共用一份**：挑哪个状态、时段的起止。
    /// 分开存会出现「两行都选了指定状态，却只有一个状态输入框」这种没法解释的
    /// 界面；六个灯位各自存一份更是六倍的输入框。六个槽里只要有一个用到带参的
    /// 档位，就把参数行显示出来。
    pub sign_trigger_state: String,
    pub sign_trigger_from: u16,
    pub sign_trigger_to: u16,
    /// 会话组的标题怎么拼（见 [`SignLabel`]）。放在「外观 → 基础」页。
    pub sign_label: SignLabel,
    pub sign_label_template: String,
    /// 边框光的三把尺子：粗细、明暗、取哪一路颜色。六个灯位共用一份 ——
    /// 它们本来就是同一条轨上的东西，分开调会调出「一条粗线配四颗小芝麻」。
    /// 三者都**不是**窗口几何：只画在绝对定位的光轨与角灯上，牌宽高仍由
    /// `sign_width` / `sign_height` 算。
    pub sign_light_scale: LightScale,
    pub sign_light_alpha: LightAlpha,
    pub sign_light_color: LightColor,
    /// 整块牌的轮廓（见 [`SignOutline`]）。只改 CSS，不改几何。
    pub sign_outline: SignOutline,
    /// 界面语言偏好。`Auto` 由**前端**解析成具体语言 —— Rust 不知道系统语言。
    pub locale: Locale,
    /// 设置窗口的亮 / 深色偏好。`Auto` = 跟随系统，解析见前端 `lib/theme.ts`。
    pub theme: Theme,
    /// 逐组的显示偏好（显示名 / 置顶）。键是**上报方给的原始组名**。
    ///
    /// 与 `hidden_groups` / `muted_groups` 是两回事：那两个管「显不显示 / 要不要
    /// 吵我」，这个管「怎么称呼它、排第几位」。
    pub group_meta: BTreeMap<String, GroupMeta>,
    /// 逐个状态的外观覆盖（颜色 / 形状 / 节奏）。没写的项跟随全局。
    ///
    /// 键是状态名，**含 `stale` / `offline` 这类派生状态** —— `Signal::parse`
    /// 刻意不接受派生状态（它们不能上报），但用户要能改它们长什么样。
    /// 所以这里存字符串键、单独校验。
    pub styles: BTreeMap<String, StateStyle>,
    /// **已废弃**，只为读得懂旧配置。启动时会搬进 `styles` 的 `color` 字段
    /// （见 `migrate_legacy_colors`），并且**永不写回** —— 一个状态的外观只有
    /// 一处表达，不让「单独的颜色表」和「外观表」并存。
    #[serde(skip_serializing)]
    pub colors: BTreeMap<String, String>,

    /// 折叠成一个单纯的灯（只剩灯，没有牌面）。**默认 false：展开成牌。**
    ///
    /// 存进设置而不是当作瞬态开关：用户折叠是有意的选择，重启后该保持。
    /// 托盘里有「展开 / 折叠外形」可以随时切回来，不会卡在折叠态。
    pub collapsed: bool,
    /// 折叠后 N 颗灯往哪个方向排。默认竖排，用户可改回横排。
    /// 这两个字段**只影响折叠态**，展开态一律是竖着叠的牌。
    pub fold_axis: FoldAxis,
    /// 折叠后每颗灯旁边带哪一段文字。`None` = 只有灯（0.6.6 的样子）。
    /// 带文字时窗口宽度由档位固定，不随组名长短变化 —— 见 [`fold_cell_size`]。
    pub fold_label: FoldLabel,
    /// 展开的牌上显示哪几段内容
    pub show: SignFields,
    /// 牌面背景的不透明度（`MIN_SIGN_OPACITY`–100）。调低能看见桌面。
    pub sign_opacity: u8,
    /// 用户从灯上右键「隐藏」掉的组。
    ///
    /// **只影响灯上显示哪几块牌**，不影响聚合、通知和钩子 —— 聚合是全局概念，
    /// 把一块牌藏起来不等于当这一组不存在。想彻底忽略一组应该让它的上报方
    /// 别再报，而不是指望这里。
    pub hidden_groups: Vec<String>,
    /// 闪烁节奏倍率（50–200，100 = 默认）。闪得太快会分心，太慢又看不出在动。
    pub blink_speed: u16,
    /// **已废弃**：0.6.5 之前的「灵动效果」布尔开关。只为读得懂旧配置，
    /// `migrate_legacy_effect` 会把它搬进 `effect`，并且**永不写回**。
    /// 默认是 `true`（= `Effect::Breathe`），所以只有旧配置显式写了 `false`
    /// 才需要迁移，不会把新配置里的效果档误改成默认值。
    #[serde(skip_serializing)]
    pub lively: bool,
    /// 动效风格（见 [`Effect`]）。默认 `Breathe` == 旧开关开着的样子（红线 3）。
    /// 五档都只影响前端绘制，不进任何状态判断、不影响窗口几何。
    pub effect: Effect,
    /// 需要你注意时，系统通知是否带声音。
    ///
    /// 声音挂在系统通知上（`NotificationBuilder::sound`），所以关掉通知就没有
    /// 声音 —— 这是刻意的：不为一个提示音再加一条独立的音频通路和它的跨平台麻烦。
    pub sound: bool,
    /// 勿扰时段：这段时间里不弹通知、不出声，灯照常显示。
    pub quiet_hours: QuietHours,
    /// 被静音的组：**只影响通知和提示音**。
    ///
    /// 与 `hidden_groups` 是两件事，别混：那个管「灯上显不显示」，这个管
    /// 「要不要吵我」。一个组可以只隐藏不静音（看得见但不打扰），也可以只静音
    /// 不隐藏（灯上照常红着，只是不弹通知）。
    pub muted_groups: Vec<String>,
    /// 「需要你注意」的状态要**持续**这么多秒才弹通知。0 = 立刻弹。
    ///
    /// 存在的理由：智能体经常在一秒内进出 permission（先问再自己答，或者
    /// 弹一下就被自动批准）。立刻弹的话，用户被叫过去看到的是一个已经解决的
    /// 问题。等一会儿能过滤掉这类「一闪而过」。
    pub alert_after_secs: u16,
    /// **预设 1**：某个组卡在「需要授权」超过 60 秒时，用「转发」里配好的钩子
    /// **再推一次**到手机（Bark / PushDeer / ntfy 那些）。
    ///
    /// 默认关。它自己不是一条推送通道 —— 没配钩子就没有东西可发，界面上写清楚。
    /// 一动就批准掉的那种权限请求不值得吵你；卡住的才值得。
    pub rule_stuck_push: bool,
    /// **预设 2**：有会话正在「思考 / 执行」时，暂停系统通知与重复提醒。
    ///
    /// 默认关。灯照常显示、上报照常进来；停的只是「弹出来打扰你」这一层。
    /// 和勿扰时段同一个语义：**这一轮不弹，也不在忙完之后补发**。
    pub rule_busy_silence: bool,
    /// **预设 3**：会议组存在时，灯上那一组画成蓝色，并且暂停通知（和预设 2
    /// 共用同一套暂停逻辑）。需要配合「会议中」采集器使用。
    ///
    /// 默认关。蓝色不参与状态语义（状态色里没有蓝），它表达的是「正在开会」，
    /// 不是「更紧急」—— 所以只在灯上画，不影响排序、托盘和钩子。
    pub rule_meeting: bool,
    /// 托盘图标是否跟随当前聚合状态变色。关掉就回到单色模板图（跟着菜单栏明暗反色）。
    pub tray_follows_state: bool,
    /// 上报没带 `group` 时，怎么给它兜一个。
    ///
    /// **默认 `None`，也就是不分组**，这条比看着重要：一旦默认按目录分组，
    /// 所有现有用户的灯会从一块牌变成若干块、窗口尺寸跟着变，而他们没配过的
    /// 每一组的显示名都是原始路径 —— 属于无声改变既有行为。
    pub group_by: GroupBy,
    /// 把 HTTP 服务暴露到局域网，让别的机器 / 容器 / WSL 里的智能体也能上报。
    ///
    /// **默认必须是关的**，这条比看着重要：开了之后网内任何人只要拿到 token 就能
    /// 上报，而「带 Origin 头一律拒绝」那道防线对 curl 这类**非浏览器**客户端无效
    /// —— 它挡的是「你正在浏览的网页偷偷打本地端口」，挡不住同一网段里的另一台机器。
    /// 换句话说开了之后唯一的防线就是 token 本身。所以这是个显式开关，不悄悄放宽。
    ///
    /// **改完要重启应用才生效**：监听地址在启动时绑定一次，运行中重绑会让端口变化
    /// （CLI 的自动发现跟着断一下），出问题时表现为「灯突然收不到上报」这种很难
    /// 归因的现象。与其在运行中做这件事，不如写明「下次启动生效」。
    pub lan_access: bool,
    /// **只读 Web 面板**（`GET /`）：手机 / 平板上打开一个链接就能看灯。
    ///
    /// **默认关**（红线）：它是一个**新的对外读取面**，而灯本身是私人的东西
    /// （哪个项目在跑、跑到哪一步、哪一步卡住了）。要看得见，得由用户明确打开。
    ///
    /// 它和上报服务**共用监听地址**：绑回环时只有本机能看；开了「允许局域网访问」
    /// 才轮到同一 WiFi 下的设备 —— 那时设置页会明说这一点。
    ///
    /// **改完立刻生效**（不像 `lan_access` 要重启）：它只是一个路由的开关，
    /// 读的是这份设置的当前值，不涉及重新绑定。
    pub web_panel: bool,
    /// **在 Web 面板上审批**（`docs/plan-approval.md` A4）：手机 / 平板点「允许 / 拒绝」。
    ///
    /// **默认关，而且它是「从灯上审批」下面的第二道开关**：`approve_tools` 关着时
    /// 根本没有待审批的东西，这个开关什么都不做。
    ///
    /// 它为什么需要单独一个开关（而不是跟着面板一起开）：面板本来是**只读**的，
    /// 而且它刻意连审批详情都不拉 —— 详情里有命令原文、文件路径。打开这个开关
    /// 就是**明确同意把那些东西送到这个页面上**，而那个页面可能正被投在客厅的
    /// 平板上、或者一条转发出去的链接里。
    ///
    /// 说清楚它**不是**一道安全边界：拿到 token 的人本来就能直接 curl
    /// `/v1/approvals`。这道开关管的是「我们自己的页面会不会把详情画出来」，
    /// 不是「别人能不能拿」。真正的门仍然是 token。
    pub web_approve: bool,
    /// 其他机器 —— **多机中枢**（见 `docs/plan-multi-machine.md`）。
    ///
    /// 我们是**拉**的一方：每 `machine_poll_ms` 去每台机器的 `GET /v1/state` 取一次
    /// 快照，把它们的会话按 `m:{机器}:{会话}` 的名字喂进本机同一个 `Store`。于是灯、
    /// 收件箱、托盘、通知、统计、物理灯、只读面板**全部零改动**就看到了多机内容。
    ///
    /// **默认空 = 一条请求都不发**（红线：新增外发默认关）。空数组时快照与没有这个
    /// 功能时逐字节一致，这是升级不改变既有行为的硬约束。
    ///
    /// 和 `lan_access` 是**两个方向**，别混：那个管「别人能不能上报给我」，这个管
    /// 「我要不要去看别人」。两个都开是正常用法（互为 hub）。
    #[serde(default)]
    pub machines: Vec<Machine>,
    /// 轮询间隔（毫秒）。远程会话的 TTL 是它的 3 倍 —— 所以这个值同时决定了
    /// 「对端掉线后多久在灯上显形」。
    ///
    /// 默认 2 秒：TTL 6 秒，掉线半分钟内必然转 `stale`，而 2 秒一次本地请求的开销
    /// 可以忽略。
    #[serde(default = "default_machine_poll_ms")]
    pub machine_poll_ms: u64,
    /// Call Me（自部署收信服务器）。**默认空 = 一条请求都不发**（红线：新增外发
    /// 默认关）；不配置时桌面端的行为与没有这个功能时一致。
    #[serde(default)]
    pub callme: CallMeConfig,
    /// 灯上单击做什么，见 [`ClickAction`]。容器级 `serde(default)` 会让旧配置
    /// 自动落到 `Collapse`，升级不改变既有手感。
    pub click_action: ClickAction,
}

/// [`Settings::machine_poll_ms`] 的缺省值。见那里的说明。
fn default_machine_poll_ms() -> u64 {
    2000
}

/// 兜底分组的依据。
///
/// 只有两个取值，而且**默认那个必须是不分组**（理由见 `Settings::group_by`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    /// 不分组：所有不带 `group` 的上报归 `default`，和没有这个开关时完全一样。
    #[default]
    None,
    /// 用上报里的 `cwd` 当组名。一个工作目录一块牌。
    Cwd,
    /// 用上报里的 `session` 当组名。**一个会话一块牌。**
    ///
    /// 同一个智能体并行开好几个会话时（每个窗口 / 每个 worktree 一个），只按目录
    /// 分组还是会把它们挤在同一块牌上 —— 而它们的进度往往是各不相同的。按会话分
    /// 就把每个会话摊成自己的一块，多个智能体自然也分开了（它们的会话必定不同）。
    ///
    /// 代价是组名是上报方给的会话 id，多半是个 UUID 那样不好读的串；牌上会自动
    /// 截断，想去「外观 → 各组的显示」改名也行。
    Session,
}

/// 牌面背景的默认不透明度。够看清内容，又能透出一点桌面。
pub const DEFAULT_SIGN_OPACITY: u8 = 86;

/// 牌面不透明度的**下限**。
///
/// 牌里的字是浅色的（组名近白、次要文字中灰），对比**全部**来自牌面那层深色。α 低于
/// 这条线时从牌面透出来的桌面比牌面本身还显眼，字其实是画在桌面上 —— 近白的程序
/// 窗口、浅色壁纸上就淡得看不见。
///
/// 以前的做法是渲染时补救：α < 50 就给牌里的字加一圈深色 `text-shadow` 描边。
/// 那圈边会把笔画吃掉、小字号糊成一团，读起来像空心字，观感比「淡一点但干净」
/// 差得多 —— 用户第一眼就看得出「这块牌被救过」。所以改成在下限上回答掉这个问题。
///
/// **65 是量出来的，不是算出来的**：同一块牌按 50/60/65/70/78/86 排一列铺在近白
/// 窗口上比，50% 时次要文字对比只有约 1.1:1（那不是淡，是消失），60% 余量太小，
/// 65% 才稳。同时 `lamp.css` 的 `--sign-muted` 从 `#9b9ba3` 提到 `#c2c2cb` ——
/// 只抬下限不动字色，等于让灰字配一块更深的灰底。
///
/// 想要更隐形的灯走**安静模式**（没事时整盏压到 `opacity: 0.15`）：那条路上本来
/// 就不指望读字，不存在「字读不清」。两条路一条管「看得见」，一条管「别打扰」。
///
/// **和前端是同一个数字**：`src/lib/lampStyle.ts::SIGN_OPACITY_MIN`。前端用它做
/// 滑杆的 `min`，这里用它夹手改的 config.json 和旧版本存下的更小值（老配置里有
/// 0–49 的，加载后会被抬到 65 —— 这是修复，不是数据损坏）。
pub const MIN_SIGN_OPACITY: u8 = 65;

/// 闪烁节奏的可调范围。上下界都留得住意义：再慢看不出在动，再快只剩烦。
pub const MIN_BLINK_SPEED: u16 = 50;
pub const MAX_BLINK_SPEED: u16 = 200;

/// 单个状态的闪烁周期范围（毫秒）。**0 单独表示常亮**，不落在这个区间里。
///
/// 下限取 120ms：比这更快就只剩一片模糊，读不出「在动」。上限取 4 秒：
/// 再慢用户会以为它卡住了。
pub const MIN_BLINK_MS: u16 = 120;
pub const MAX_BLINK_MS: u16 = 4000;

/// 单个状态的**停留时长**（毫秒）的可调范围：这个状态静默多久后算过期。
///
/// **刻意没有「永不过期」这一档。** 一盏不会自己熄灭的灯等于一条永远不撤的
/// 告警：智能体被强杀（`SessionEnd` 没触发）之后它会一直亮着，用户拿它没办法。
/// 早先 `idle` 没有 TTL，一个忘关的会话能在灯上挂半小时 —— 那还只是半小时。
///
/// 下界 1 秒：比一次上报还短的 TTL 等于「报完立刻消失」，那不是设置，是自相矛盾。
/// 上界 24 小时：有理由要很久（跨夜跑的任务），但不能没有边 —— 灯最终必须能回到
/// 「没有智能体上报」。上界同时受 `Signal::default_ttl` 那条注释约束：放宽的代价
/// 是「崩溃的智能体」更晚被发现。
pub const MIN_STATE_TTL_MS: u32 = 1_000;
pub const MAX_STATE_TTL_MS: u32 = 24 * 60 * 60 * 1_000;

/// 「持续多久才弹通知」的上限（秒）。一小时已经远超任何有意义的等待。
pub const MAX_ALERT_DELAY_SECS: u16 = 3600;

/// 默认的「持续多久才弹通知」。5 秒足够滤掉「问一下又自己答了」这类一闪而过，
/// 又不至于让真正的等待等太久。
pub const DEFAULT_ALERT_DELAY_SECS: u16 = 5;

/// 默认会在哪些状态下弹系统通知。
///
/// **这必须等于加 `notify_when` 之前的行为**：老配置里没有这个字段，落到的就是
/// 这几个值。要是这里写成「空 = 全部状态」，升级上来的用户会突然在每一次
/// `working` / `done` 时被弹一下 —— 这是这类改动里最不该发生的事。
pub const DEFAULT_NOTIFY_STATES: [&str; 3] = ["permission", "attention", "error"];

/// 能给通知挑的状态：`Signal::ALL_NAMES` 去掉两个。
///
/// 去掉的是 `offline` 和 `off`：它们不代表「发生了什么」。`offline` 是「一个组
/// 都没有 / 全被静音」时的兜底值，`off` 是「用户自己按了暂停」—— 勾上只会得到
/// 「你暂停时提醒你暂停了」这种噪音。`stale` 留着：一个会话沉默到快消失，确实
/// 是值得知道的事。
///
/// 前端那份是 `src/lib/types.ts` 的 `NOTIFY_STATES`（由 `SIGNALS` 过滤出来），
/// 两边集合必须一致 —— 界面能勾的项在这里被丢掉的话，用户点了半天没反应。
pub const NOTIFY_STATES: [&str; 8] = [
    "stale", "idle", "thinking", "working", "done", "attention", "permission", "error",
];

fn default_notify_when() -> Vec<String> {
    DEFAULT_NOTIFY_STATES.iter().map(|name| name.to_string()).collect()
}

/// 通知状态列表的规整：去空白、统一小写、丢掉不认识 / 不能选的、去重。
///
/// **保持用户给的顺序**：判定是 `contains`，顺序没有语义，但按原样留着更容易
/// 看出「用户当时点了什么」。
fn normalize_notify_states(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim().to_ascii_lowercase();
        if NOTIFY_STATES.contains(&name.as_str()) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// 通知状态列表里有没有不能用的名字。`validate_settings` 用它给出明确报错 ——
/// 界面只会发合法值，所以这条只在「导入一份手改过的配置」时才会触发。
pub fn notify_when_problem(names: &[String]) -> Option<String> {
    for name in names {
        if !NOTIFY_STATES.contains(&name.as_str()) {
            return Some(format!(
                "状态 {name:?} 不能配通知；可用: {}",
                NOTIFY_STATES.join(" ")
            ));
        }
    }
    None
}

/// 组名列表的规整：去空白、去空串、去重、排序。
///
/// 隐藏和静音两个列表都要它。脏数据（`" ci"` 和 `"ci"`）会让「这一组藏了吗」
/// 时灵时不灵，而那是最难查的一类问题。
fn normalize_groups(names: Vec<String>) -> Vec<String> {
    let mut names: Vec<String> = names
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// 由工作目录得出的组名：**完整路径**，去掉末尾的分隔符。
///
/// 用完整路径而不是最后一段目录名：同名目录（`~/a/api` 和 `~/b/api`）太常见，
/// 撞名会把两个项目的会话混进同一块牌 —— 那正是这个开关最不该出的错。代价是
/// 组名很长，所以灯上会自动截断（见 `settings.css` 的 `.group-name`），
/// 设置里也能逐组改名。
fn group_from_cwd(cwd: &str) -> Option<String> {
    let name = cwd.trim().trim_end_matches(['/', '\\']);
    // 根目录 `/` 去掉分隔符就成了空串 —— 那不是个能用的组名，退回 `default`。
    // 一个会话的工作目录就是根目录，本来也少见。
    (!name.is_empty()).then(|| name.to_string())
}

/// 取路径最后一段。两种分隔符都认，免得 Windows 上写的路径在别处解析错。
///
/// 通知正文（`alerts.rs`）和会话分组的默认显示名（`state.rs`）都用它 ——
/// 两处各写一遍「最后一段怎么切」迟早会分叉，而它们要的显然是同一个东西。
pub fn project_name(cwd: &str) -> String {
    cwd.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

/// `#rrggbb` 或 `#rgb`。只做形状检查，不判断这个色值好不好看。
pub fn is_hex_color(value: &str) -> bool {
    let hex = value.strip_prefix('#').unwrap_or("");
    matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// 只做最基本的判断（协议 + 有主机名），不试图猜目标是 ntfy 还是 Slack。
pub fn is_http_url(url: &str) -> bool {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or("");
    !rest.is_empty() && !rest.starts_with(['.', '/'])
}

/// 一条钩子是否可用。校验和 `clamped()` 的清理都用它，免得两处规则漂移。
pub fn hook_problem(hook: &Hook) -> Option<String> {
    for name in &hook.when {
        if !crate::state::Signal::is_known_name(name) {
            return Some(format!(
                "未知的状态名 {name:?}；可用: {}",
                crate::state::Signal::ALL_NAMES.join(" ")
            ));
        }
    }
    // 空目标不算错（见 is_configured）；填了但填错才算
    if let HookAction::Http {
        url,
        preset,
        body,
        content_type,
        ..
    } = &hook.action
    {
        if !url.trim().is_empty() && !is_http_url(url) {
            return Some(format!(
                "钩子地址不合法：{url:?}（需要以 http:// 或 https:// 开头）"
            ));
        }
        if *preset == HttpPreset::Custom {
            if let Some(name) = body.as_deref().and_then(unknown_placeholder) {
                let known: Vec<String> = TEMPLATE_PLACEHOLDERS
                    .iter()
                    .map(|name| format!("{{{name}}}"))
                    .collect();
                return Some(format!(
                    "自定义请求体里有不认识的占位符 {{{name}}}；可用: {}",
                    known.join(" ")
                ));
            }
            if let Some(value) = content_type
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                && value.chars().any(char::is_control)
            {
                return Some("content-type 里不能有换行或控制字符".to_string());
            }
        }
    }
    None
}

/// 模板里出现的、不在 [`TEMPLATE_PLACEHOLDERS`] 里的那个占位符。
///
/// 只认 `{snake_case}` 这种形状：JSON 里的 `{"a":1}`、`{ "a": 1 }` 都不算 ——
/// 否则一条完全正常的 JSON body 会遍地「未知占位符」，校验比不校验更烦人。
pub fn unknown_placeholder(template: &str) -> Option<String> {
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start && bytes.get(end) == Some(&b'}') {
                let name = &template[start..end];
                if !TEMPLATE_PLACEHOLDERS.contains(&name) {
                    return Some(name.to_string());
                }
                index = end;
            }
        }
        index += 1;
    }
    None
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            // 「原样」。**必须是 1.0**：写成 0.0 就是牌宽归零（红线 3 的经典翻车）。
            sign_w_mult: 1.0,
            sign_h_mult: 1.0,
            // 默认单灯 —— 新能力默认关（红线 3）
            lamp_layout: LampLayout::Single,
            always_on_top: true,
            // 吸附会改变拖动的手感，默认关（红线 3）
            snap_to_edge: false,
            quiet: true,
            notify: true,
            notify_when: default_notify_when(),
            webhook_url: None,
            hooks: Vec::new(),
            // 空 = 一台灯都不发（新增外发默认关，红线 3）
            devices: Vec::new(),
            // 会改变智能体行为的开关一律默认关（红线 3）
            approve_tools: false,
            collect_backup: false,
            collect_meeting: false,
            shape: Shape::default(),
            palette: Palette::default(),
            // 五项默认全等于「加这些字段之前」的样子：升级上来观感不变（红线 3）
            sign_radius: SignRadius::default(),
            sign_border: SignBorder::default(),
            sign_tint: SignTint::default(),
            sign_text: SignText::default(),
            sign_emphasis: SignEmphasis::default(),
            // 字体和间距同理：默认 == 加这两个字段之前的样子
            sign_font: SignFont::default(),
            sign_gap: SignGap::default(),
            // 六个灯位默认全关 / 全空、触发全常亮：加这些字段之前，牌的四面和
            // 四角本来就是空的。旧字段同样取默认 —— 它们只是迁移输入。全部等价于
            // 「没加过」，所以旧配置读进来观感一点不变（红线 3）
            sign_edge: SignEdge::default(),
            sign_corners: SignCorners::default(),
            sign_edge_trigger: LightTrigger::default(),
            sign_corner_trigger: LightTrigger::default(),
            sign_lights: SignLights::default(),
            // 触发参数 + 标题档：全部等于「没加过」。注意标题的**默认值变了**
            // （`Session` 不再拼智能体名），这是用户明确要的观感改动，不是兼容项。
            sign_trigger_state: DEFAULT_LIGHT_TRIGGER_STATE.to_string(),
            sign_trigger_from: DEFAULT_LIGHT_TRIGGER_FROM,
            sign_trigger_to: DEFAULT_LIGHT_TRIGGER_TO,
            sign_label: SignLabel::default(),
            sign_label_template: DEFAULT_SIGN_LABEL_TEMPLATE.to_string(),
            sign_light_scale: LightScale::default(),
            sign_light_alpha: LightAlpha::default(),
            sign_light_color: LightColor::default(),
            // 轮廓默认 Rect（= 遵守 sign_radius），升级不改观感
            sign_outline: SignOutline::default(),
            locale: Locale::default(),
            // 跟随系统：升级上来的旧配置没有这个字段，行为必须和加开关之前一致
            theme: Theme::default(),
            group_meta: BTreeMap::new(),
            styles: BTreeMap::new(),
            colors: BTreeMap::new(),
            collapsed: false,
            // 折叠默认竖排、不带文字：用户明确要的「默认竖排」，而「只有灯」
            // 保证升级上来的人不会突然多出一块板。
            fold_axis: FoldAxis::default(),
            fold_label: FoldLabel::default(),
            show: SignFields::default(),
            sign_opacity: DEFAULT_SIGN_OPACITY,
            blink_speed: 100,
            // 门面默认带一层呼吸感。旧开关默认 true 与 Breathe 是同一副样子，
            // 所以 migrate_legacy_effect 只在旧配置写了 false 时才动手
            lively: true,
            effect: Effect::default(),
            hidden_groups: Vec::new(),
            sound: true,
            quiet_hours: QuietHours::default(),
            muted_groups: Vec::new(),
            alert_after_secs: DEFAULT_ALERT_DELAY_SECS,
            // 预设规则默认关：它们会改变「什么时候打扰你」，属于用户明确要了才生效的行为
            rule_stuck_push: false,
            rule_busy_silence: false,
            rule_meeting: false,
            tray_follows_state: true,
            group_by: GroupBy::default(),
            // 默认只监听本机。见字段注释：开了之后唯一的防线就是 token
            lan_access: false,
            web_panel: false,
            // 会改变智能体行为的开关，缺字段必须是关的（同 `approve_tools`）
            web_approve: false,
            // 多机中枢默认一台都不配 —— 空数组 = 一条请求都不发
            machines: Vec::new(),
            machine_poll_ms: default_machine_poll_ms(),
            // Call Me 默认关闭：地址为空 → 轮询线程一个请求都不发
            callme: CallMeConfig::default(),
            // 旧版配置里没有这个字段 —— 默认折叠，升级不让单击改行为
            click_action: ClickAction::default(),
        }
    }
}

impl Settings {
    /// 这个状态该不该弹系统通知。
    ///
    /// 用名字比字符串，而不是先把列表解析成 `Vec<Signal>`：这条判断在每一次状态
    /// 推送里都会跑（一秒可能好几次），为它每次都分配一个小 Vec 不值当。列表在
    /// `clamped` 里已经规整过，所以这里只做精确比较。
    pub fn notify_wants(&self, state: crate::state::Signal) -> bool {
        let name = state.name();
        self.notify_when.iter().any(|wanted| wanted == name)
    }

    /// 把越界值夹回允许范围。设置来自两处不可信输入：手改过的 config.json
    /// 和前端传来的值。
    pub fn clamped(mut self) -> Self {
        self.size = self.size.clamp(MIN_SIZE, MAX_SIZE);
        // 倍率先夹紧再往下走：NaN / 无穷 / 手改的离谱值都在这儿被挡掉
        self.sign_w_mult = clamp_mult(self.sign_w_mult);
        self.sign_h_mult = clamp_mult(self.sign_h_mult);
        // 上下界都要夹：下限是「牌面还托得住浅字」的可读线（见 `MIN_SIGN_OPACITY`）。
        // 旧配置里存过 0–64 的人，读进来会被抬到 65 —— 字变清楚了，牌面也没那么透。
        self.sign_opacity = self.sign_opacity.clamp(MIN_SIGN_OPACITY, 100);
        self.blink_speed = self.blink_speed.clamp(MIN_BLINK_SPEED, MAX_BLINK_SPEED);
        self.alert_after_secs = self.alert_after_secs.min(MAX_ALERT_DELAY_SECS);
        // 触发参数：状态名不认识就退回默认（否则那一档永远不亮，界面上没有任何
        // 线索）；时段按分钟取模绕回一天内；模板按字符数截断，免得一行配置把
        // config.json 撑成几 MB，牌上也只会显示前几个字。
        let wanted = self.sign_trigger_state.trim().to_ascii_lowercase();
        self.sign_trigger_state = if LIGHT_TRIGGER_STATES.contains(&wanted.as_str()) {
            wanted
        } else {
            DEFAULT_LIGHT_TRIGGER_STATE.to_string()
        };
        self.sign_trigger_from %= 1440;
        self.sign_trigger_to %= 1440;
        // `String::truncate` 按字节切，切在 UTF-8 中间会 panic —— 按字符数收。
        if self.sign_label_template.chars().count() > MAX_SIGN_LABEL_TEMPLATE {
            self.sign_label_template = self
                .sign_label_template
                .chars()
                .take(MAX_SIGN_LABEL_TEMPLATE)
                .collect();
        }
        // 通知状态列表：写坏的项直接丢掉。**全丢光时退回默认** —— 一个都认不出来
        // 还留成空表的话，表现是「通知开着、权限也给了，但永远不弹」，而界面上
        // 没有任何线索指向这里。
        let had_states = !self.notify_when.is_empty();
        let states = normalize_notify_states(std::mem::take(&mut self.notify_when));
        self.notify_when = if states.is_empty() && had_states {
            default_notify_when()
        } else {
            states
        };
        self.quiet_hours.start = self.quiet_hours.start.min(23);
        self.quiet_hours.end = self.quiet_hours.end.min(23);
        // 组名去空白、去空串、去重并排序。这两个列表都会被反复比较（每块牌都要问
        // 一次「我该不该显示 / 我该不该吵」），脏数据留在里面就每次都要重新规整。
        self.hidden_groups = normalize_groups(self.hidden_groups);
        self.muted_groups = normalize_groups(self.muted_groups);
        // 空白串等于「不转发」，归一成 None，免得每次转发都白试一次
        self.webhook_url = self
            .webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string);
        // 丢掉形状不对的颜色，并把十六进制统一成小写（便于前端比较）。
        // **不认识的键名刻意留着**：clamped 是手改配置文件的兜底，静默删掉用户
        // 写的东西是数据丢失；键名合法性由 set_settings 校验（会明确报错），
        // 这里只保证值本身可用。
        for style in self.styles.values_mut() {
            if let Some(color) = style.color.take() {
                let color = color.trim().to_ascii_lowercase();
                if is_hex_color(&color) {
                    style.color = Some(color);
                }
            }
            // 0 是「常亮」，单独放行；其余夹回有意义的区间
            if let Some(ms) = style.blink_ms {
                style.blink_ms = Some(if ms == 0 { 0 } else { ms.clamp(MIN_BLINK_MS, MAX_BLINK_MS) });
            }
            // 停留时长没有「常驻」这一档，所以直接夹（见 MIN_STATE_TTL_MS 的注释）
            if let Some(ms) = style.ttl_ms {
                style.ttl_ms = Some(ms.clamp(MIN_STATE_TTL_MS, MAX_STATE_TTL_MS));
            }
        }
        // 一样都没写的条目没有任何作用，留着只会让「预设是否还生效」的判断无谓失败
        self.styles.retain(|_, style| !style.is_empty());
        // 显示名去空白、空串归一成 None；两样都没写的条目同样丢掉
        for meta in self.group_meta.values_mut() {
            if let Some(name) = meta.name.take() {
                let name = name.trim().to_string();
                meta.name = (!name.is_empty()).then_some(name);
            }
        }
        self.group_meta.retain(|_, meta| !meta.is_empty());
        // 同样地，写坏的钩子只丢掉那一条，不让整份设置失效
        self.hooks.retain(|hook| hook_problem(hook).is_none());        // 钩子里的组名也要规整，理由同 hidden_groups
        for hook in &mut self.hooks {
            hook.groups = normalize_groups(std::mem::take(&mut hook.groups));
            if let HookAction::Http {
                headers,
                body,
                content_type,
                ..
            } = &mut hook.action
            {
                *headers = normalize_headers(std::mem::take(headers));
                // 模板和 content-type 按字符夹紧，手改配置写进来的离谱值也兜住。
                // **切换预设时不清空**：用户来回切一下回来，辛苦写的模板还得在。
                *body = body
                    .as_deref()
                    .map(|text| text.chars().take(TEMPLATE_MAX_CHARS).collect::<String>());
                *content_type = content_type.as_deref().map(|text| {
                    text.trim().chars().take(CONTENT_TYPE_MAX_CHARS).collect::<String>()
                });
            }
        }
        // 地址去空白。**空地址留着**，和钩子一个脾气：那是「刚点了添加、还没填」
        // 这个必然经过的中间态，存下去会被丢掉的话，界面上就是「刚点出来的那一行
        // 自己没了」。真正发的时候按空地址过滤（见 `devices::sync`）。
        for device in &mut self.devices {
            device.host = device.host.trim().to_string();
        }
        // 多机中枢。规则比 devices 多两条，因为**机器名是命名空间**：
        //
        // - 名称空 = 没法做命名空间，整条丢掉（不是「先留着待填」—— 一个没有名字的
        //   机器连报错都指不出是哪台）。
        // - 名称重复 = 两台机器的会话会挤进同一个命名空间，而组名 / 会话 id 会互相
        //   覆盖。**先出现的赢**，因为那是用户先写的那个。
        // - 地址空仍保留（理由同 devices）。
        //
        // 顺序不能反：先去空白再判空，否则一个只打了空格的名称会被当成有名字。
        for machine in &mut self.machines {
            machine.id = machine.id.trim().to_string();
            machine.url = machine.url.trim().to_string();
            machine.token = machine.token.trim().to_string();
        }
        self.machines.retain(|machine| !machine.id.is_empty());
        {
            // 用一个 set 记见过的名字。`HashSet` 而不是排序去重：要的是**保序**去重
            // （先出现的赢），排序会把用户排的顺序打乱。
            let mut seen = std::collections::HashSet::new();
            self.machines.retain(|machine| seen.insert(machine.id.clone()));
        }
        self.machines.truncate(MAX_MACHINES);
        self.machine_poll_ms = self
            .machine_poll_ms
            .clamp(MIN_MACHINE_POLL_MS, MAX_MACHINE_POLL_MS);
        // Call Me：地址和 token 都去空白（粘贴时经常带首尾空格）；空值保留 ——
        // 那是「刚打开这一页、还没填」的中间态，轮询线程按 `enabled()` 判断。
        self.callme.url = self.callme.url.trim().to_string();
        self.callme.token = self.callme.token.trim().to_string();
        self.callme.poll_ms = self
            .callme
            .poll_ms
            .clamp(MIN_CALLME_POLL_MS, MAX_CALLME_POLL_MS);
        self
    }

    /// 某个状态最终用哪个颜色（`#rrggbb`）。
    ///
    /// 优先级和灯上一致：**用户逐个状态指定的 > 配色预设**（见 `palettes.ts` 的
    /// `resolveColors`）。`state` 是状态名，含 `stale` / `offline` 这类派生状态。
    ///
    /// 这是 Rust 侧唯一的「状态 → 颜色」入口：只读 Web 面板和物理灯都从这里取，
    /// 免得两处各写一份投影。**配色表本身仍是两处真值**（`palettes.ts` 里十个状态
    /// 逐行写着，这里是四个语义档）—— 改配色时两边要一起改，那边也有一句指回来的
    /// 注释。
    ///
    /// 认不出来的状态名落到「没动静」那一档的灰，而不是空串：颜色最终要塞进
    /// JSON / 发给灯，空串会让调用方各自兜底，而那正是「同一个状态在三个地方
    /// 三个颜色」的来源。
    pub fn state_color(&self, state: &str) -> &str {
        // 用户改过的那个状态用他自己的颜色
        if let Some(custom) = self
            .styles
            .get(state)
            .and_then(|style| style.color.as_deref())
            .map(str::trim)
            .filter(|color| !color.is_empty())
        {
            return custom;
        }
        let (ok, warn, danger, muted) = match self.palette {
            Palette::Default => ("#16a34a", "#d99a06", "#d0342c", "#9ca3af"),
            Palette::Colorblind => ("#0072b2", "#e69f00", "#d55e00", "#8b8f96"),
            Palette::Vivid => ("#00b34a", "#ffab00", "#ff2d2d", "#9aa1ab"),
        };
        match state {
            "idle" | "thinking" | "working" | "done" => ok,
            "attention" => warn,
            "permission" | "error" => danger,
            _ => muted, // stale / offline / off
        }
    }

    /// 这个组该不该出现在灯上。Rust 算窗口高度和前端渲染都必须问同一句话 ——
    /// 两边算出不同的组，窗口就会裁掉最后一块牌，或者底下空一条。
    pub fn shows_group(&self, name: &str) -> bool {
        !self.hidden_groups.iter().any(|hidden| hidden == name)
    }

    /// 这次上报最终归哪个组。
    ///
    /// 上报**自己带的 `group` 优先**，`group_by` 只是兜底，不是覆盖。这条顺序
    /// 不能反：反了就等于「开了按目录分组之后，手写 `group` 的用法全失效」——
    /// 而多分组最早、也最稳的用法就是手写 `group`（见 docs/api.md §2.7）。
    /// 想只给某一路单独分组的人也不必关掉全局开关。
    ///
    /// `session` 是**必定有值**的（`Report::session_id()` 兜到 `default`），所以
    /// `GroupBy::Session` 不需要 `cwd` 那样的 `Option` 处理。
    pub fn resolve_group(
        &self,
        group: Option<&str>,
        cwd: Option<&str>,
        session: &str,
    ) -> Option<String> {
        let explicit = group.map(str::trim).filter(|name| !name.is_empty());
        if let Some(name) = explicit {
            return Some(name.to_string());
        }
        match self.group_by {
            GroupBy::None => None,
            GroupBy::Cwd => cwd.and_then(group_from_cwd),
            // 会话 id 本身就是组名。空串理论上到不了这儿（`session_id()` 兜了
            // `default`），但真到了也没有意义 —— 退回不分组比造一个空组名好。
            GroupBy::Session => {
                let id = session.trim();
                (!id.is_empty()).then(|| id.to_string())
            }
        }
    }

    /// 这一组在灯上叫什么。没设显示名就用上报方给的原始组名。
    ///
    /// 注意**只是显示**：隐藏 / 静音的匹配、`/v1/state` 里的组名都仍然是原始名。
    pub fn group_label<'a>(&'a self, name: &'a str) -> &'a str {
        self.group_meta
            .get(name)
            .and_then(|meta| meta.name.as_deref())
            .unwrap_or(name)
    }

    /// 灯上那块牌**显示**的组名：用户改的名字 > 服务端给的默认名 > 原始组名。
    ///
    /// 和前端 `lampSort.ts::labelOf` 是同一条三级规则。转发摘要用它，因为转发出去的
    /// 那行字必须是**同一个人看到的那个名字**：用户在灯上把 `build-1` 改成了「构建」，
    /// 转发里却还是 `build-1`，那就是两处真值打架。
    ///
    /// 匹配（隐藏 / 静音 / 通知动作 / 钩子的 `groups`）**永远用原始 `name`** ——
    /// 和前端一样，显示名不参与任何匹配。
    pub fn display_label<'a>(&'a self, group: &'a crate::state::GroupView) -> &'a str {
        if let Some(custom) = self
            .group_meta
            .get(&group.name)
            .and_then(|meta| meta.name.as_deref())
        {
            return custom;
        }
        group.default_label.as_deref().unwrap_or(&group.name)
    }

    /// 置顶权重。`None` = 不置顶，按紧急程度排。
    pub fn group_order(&self, name: &str) -> Option<i16> {
        self.group_meta.get(name).and_then(|meta| meta.order)
    }

    /// 窗口尺寸。`signs` 是可见牌的高度列表（Rust 侧算好，见 `sign_height`）。
    ///
    /// 一块牌时窗口就是「牌 + 留白」；多块竖着叠，高度逐块累加。
    pub fn window_size(&self, signs: &[u16]) -> (u16, u16) {
        // 间距档位（`SignGap`）是外观里唯一影响几何的一项 —— 行竖着叠，
        // 它直接进窗口总高。0.6.7 起 CSS 把它消化成板内每行的底部留白，
        // 总高公式不变；默认档必须逐字等于原来的 `size / 2`。
        let gap = self.sign_gap.px(self.size);
        let count = signs.len().clamp(1, MAX_SIGNS) as u16;
        let stacked: u16 = signs.iter().take(count as usize).sum();
        let gaps = gap * count.saturating_sub(1);
        (sign_width(self.size, self.sign_w_mult) + LAMP_MARGIN, stacked + gaps + LAMP_MARGIN)
    }

    /// 折叠态窗口尺寸。**主灯和每组分灯都走这里**，免得两处各自拼七个参数、
    /// 将来加一档折叠内容时漏掉一个调用点。几何见 [`collapsed_window_size`]。
    pub fn folded_size(&self, dots: usize, badges: usize) -> (u16, u16) {
        collapsed_window_size(
            self.size,
            dots,
            badges,
            self.fold_axis,
            self.fold_label,
            self.sign_w_mult,
            self.sign_h_mult,
        )
    }
}

fn settings_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("config.json"))
}

/// 读设置。任何问题（文件不存在、内容坏掉）都退回默认值并写一行 stderr ——
/// 为了一个配置文件解析失败而拒绝启动，代价远大于收益。
pub fn load_settings() -> Settings {
    let Ok(path) = settings_file() else {
        return Settings::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    match serde_json::from_str::<Settings>(&raw) {
        Ok(settings) => {
            // 顺序无所谓：三件事各搬各的字段，互不读取对方的产物。
            let settings = migrate_legacy_light_slots(settings);
            let settings = migrate_legacy_effect(settings);
            migrate_legacy_colors(migrate_legacy_webhook(settings)).clamped()
        }
        Err(err) => {
            eprintln!("[signalhub] {} 无法解析（{err}），使用默认设置", path.display());
            Settings::default()
        }
    }
}

/// 把 0.6.4 及以前的一整条边灯 / 四角共用触发搬进六个独立灯位。
///
/// 旧字段标了 `skip_serializing`，配置写回一次之后它们就再也不出现，所以迁移
/// 只会发生一次 —— 不会在用户后来把某个灯位改回默认时又被旧值盖掉。
///
/// **「新结构完全没写过」是迁移的唯一条件**：新配置里四个旧字段一律是默认值，
/// 这个判断既把它们挡在外面，也保证用户手工同时写了新旧两套时以新的为准。
fn migrate_legacy_light_slots(mut settings: Settings) -> Settings {
    if settings.sign_lights != SignLights::default() {
        return settings;
    }
    let edge = settings.sign_edge;
    let edge_trigger = settings.sign_edge_trigger;
    let corners = settings.sign_corners;
    let corner_trigger = settings.sign_corner_trigger;
    if edge == SignEdge::Off
        && corners.count() == 0
        && edge_trigger == LightTrigger::Always
        && corner_trigger == LightTrigger::Always
    {
        return settings;
    }

    // 旧的「覆盖范围」四档映射到上下两条边：Top 只走上边，Frame / Glow 上下都走。
    // 左右两截不保留，理由见 SignLights 的注释。
    let top = match edge {
        SignEdge::Off => EdgeLight::Off,
        SignEdge::Top | SignEdge::Frame => EdgeLight::Line,
        SignEdge::Glow => EdgeLight::Glow,
    };
    let bottom = match edge {
        SignEdge::Off | SignEdge::Top => EdgeLight::Off,
        SignEdge::Frame => EdgeLight::Line,
        SignEdge::Glow => EdgeLight::Glow,
    };
    settings.sign_lights.top = EdgeSlot {
        kind: top,
        trigger: edge_trigger,
        // 旧模型没有「闪」这一档，迁移出来的灯一律常亮 —— 升级不改观感。
        blink: false,
    };
    settings.sign_lights.bottom = EdgeSlot {
        kind: bottom,
        trigger: edge_trigger,
        blink: false,
    };

    let slot = |kind: SignCorner| CornerSlot {
        kind,
        trigger: corner_trigger,
        blink: false,
    };
    settings.sign_lights.tl = slot(corners.tl);
    settings.sign_lights.tr = slot(corners.tr);
    settings.sign_lights.br = slot(corners.br);
    settings.sign_lights.bl = slot(corners.bl);

    // 内存里也清掉，免得这份对象被别处直接序列化时又把旧值带出去。
    settings.sign_edge = SignEdge::Off;
    settings.sign_corners = SignCorners::default();
    settings.sign_edge_trigger = LightTrigger::Always;
    settings.sign_corner_trigger = LightTrigger::Always;
    eprintln!("[signalhub] 把旧的一条边框光搬进了六个独立灯位");
    settings
}

/// 把旧的「灵动效果：关」搬成 `effect = off`。
///
/// `lively` 默认 `true`，而新配置里根本没有这个键（`skip_serializing`），所以
/// 只有旧配置**显式写了 false** 才会走到这里；用户在新版里选了别的效果档不会被改。
fn migrate_legacy_effect(mut settings: Settings) -> Settings {
    if !settings.lively {
        settings.effect = Effect::Off;
        settings.lively = true;
        eprintln!("[signalhub] 把旧的「灵动效果：关」搬成了 effect=off");
    }
    settings
}

/// 把旧版的逐状态颜色表搬进 `styles`。
///
/// 旧版本用一张扁平的 `colors` 表存颜色；现在一个状态的外观是「颜色 + 形状 +
/// 节奏」三样，所以改成了 `styles`。不搬的话，用户调过的颜色会在升级后静默
/// 变回默认 —— 而那正是最难发现的一类回归。
///
/// 如果某个状态在 `styles` 里已经有颜色（用户同时写了两处），以 `styles` 为准。
fn migrate_legacy_colors(mut settings: Settings) -> Settings {
    let legacy = std::mem::take(&mut settings.colors);
    if legacy.is_empty() {
        return settings;
    }

    let mut moved = 0usize;
    for (state, color) in legacy {
        let style = settings.styles.entry(state).or_default();
        if style.color.is_none() {
            style.color = Some(color);
            moved += 1;
        }
    }
    if moved > 0 {
        eprintln!("[signalhub] 把旧的逐状态颜色表搬进了 styles（{moved} 项）");
    }
    settings
}

/// 把旧版的单条 `webhook_url` 搬成一条钩子。
///
/// 旧版本只支持「一条 webhook，只在需要注意时转发」，正好等价于一条
/// `when = [permission, attention, error]` 的 HTTP 钩子。不搬的话用户已经配好的
/// 转发会在升级后静默失效 —— 而那正是最难发现的一类回归。
fn migrate_legacy_webhook(mut settings: Settings) -> Settings {
    if let Some(url) = settings.webhook_url.take() {
        let url = url.trim().to_string();
        if !url.is_empty() {
            eprintln!("[signalhub] 把旧的 webhook 地址搬成了一条钩子：{url}");
            settings.hooks.push(Hook {
                when: vec!["permission".into(), "attention".into(), "error".into()],
                // 旧的单条 webhook 只看全局聚合，这正是 groups 为空的语义
                groups: Vec::new(),
                action: HookAction::http(url),
            });
        }
    }
    settings
}

pub fn save_settings(settings: &Settings) -> io::Result<()> {
    let settings = settings.clone().clamped();
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|err| io::Error::other(format!("序列化设置失败: {err}")))?;
    atomic_write(&settings_file()?, &json, None)
}

/// 原子地写一个文件：先写同目录的临时文件，再 `rename` 过去。
///
/// **为什么必须原子**：`load_settings` 的失败兜底是**静默退回全部默认值**。非原子写
/// 写到一半崩溃（断电、强杀、磁盘满），留下的就是残缺的 JSON —— 用户调了一小时的
/// 颜色、钩子、分组会一声不响地全没，而且没有任何地方会告诉他。`rename` 在同一个
/// 文件系统内是原子的，所以读到的要么是旧的完整内容、要么是新的完整内容。
///
/// 三处要点，少一处都会出问题：
///
/// - 临时文件**必须和目标同目录**。`rename` 跨文件系统会失败（`EXDEV`），而放
///   `/tmp` 正好就跨了 —— 配置在 `~/.signalhub`，`/tmp` 常常是另一个挂载点。
/// - 权限在建临时文件时就设好，不是 `rename` 之后再 `chmod`。后者会留下一个
///   「已经是正式文件、但还没收紧权限」的窗口期 —— 那正是 `write_private` 注释里
///   说的那个窗口。
/// - 目标是**符号链接**时写到它指向的地方。用 dotfiles 管配置的人常把
///   `~/.claude/settings.json` 做成链接，而 `rename` 换的是目录项，会把链接替换成
///   一个普通文件 —— 用户的仓库就此断掉，而且不会报错。
pub(crate) fn atomic_write(path: &Path, contents: &str, mode: Option<u32>) -> io::Result<()> {
    // symlink 要顺着写。canonicalize 对「存在但不是链接」也返回同一个路径（只是
    // 顺带解了目录上的链接），对「还不存在」失败 —— 那时用原路径正是要的。
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("目标路径没有父目录"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("目标路径没有文件名"))?;
    // 带上进程 id **和一个进程内计数器**：单实例插件保证只有一个进程，真正并发
    // 的是同一进程里的两个线程（出站钩子的「发送测试」会并发跑）。只用 pid 的话
    // 两边会抢同一个临时文件 —— 一个 rename 成功、另一个失败，或者互相覆盖掉
    // 一轮记录（issue #17）。
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed),
    ));

    // **进程内的写串行化。** 临时文件互不相同只解决了一半；Windows 上
    // `MoveFileEx(REPLACE_EXISTING)` 替换一个**正被另一处替换**的目标时会返回
    // `ERROR_ACCESS_DENIED`（code 5）—— 同一进程里的两个线程并发写同一个文件
    // 会稳定撞上（出站钩子的「发送测试」就是并发跑的）。
    //
    // 串行化是最小、最确定的修法：这些写本来就低频（配置、日志、位置），
    // 排个队完全无所谓。锁只包住「写临时文件 + rename」，不带任何用户回调。
    let _serial = WRITE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    match write_then_rename(&tmp, &path, contents, mode) {
        Ok(()) => Ok(()),
        Err(err) => {
            // 半截的临时文件留在配置目录里只会让下次困惑
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// 写文件用的进程内锁。见 `atomic_write` 里的说明。
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn write_then_rename(tmp: &Path, path: &Path, contents: &str, mode: Option<u32>) -> io::Result<()> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    // Windows 上文件默认继承用户目录的 ACL，`mode` 没有对应概念
    #[cfg(not(unix))]
    let _ = mode;

    let mut file = options.open(tmp)?;
    file.write_all(contents.as_bytes())?;
    // 先把数据刷下去再改名。不 sync 的话，崩溃后可能出现「名字已经换了、内容还是空的」
    // —— 那正是这个函数要防的那种残缺文件。
    file.sync_all()?;
    drop(file);

    // Windows 上把临时文件 rename 到**已存在的目标**时，如果那个文件正被别处
    // 打开着（杀毒软件扫描、另一个进程读配置），会拿到 `ERROR_ACCESS_DENIED`
    // （code 5）。进程内的并发已经由 `atomic_write` 里的 `WRITE_LOCK` 排掉了，
    // 这里兜的是**进程外**的短暂占用：重试几次，最后一次仍失败就原样返回，
    // 不假装成功。
    let mut delay = std::time::Duration::from_millis(2);
    let mut last = None;
    for _ in 0..RENAME_ATTEMPTS {
        match fs::rename(tmp, path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                last = Some(err);
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("rename 失败")))
}

/// `rename` 最多试几次（2 + 4 + 8 + 16 ms）。见 `write_then_rename` 里的说明。
const RENAME_ATTEMPTS: usize = 5;

/// 只有它能读的东西（目前是 token）才用这个。分开命名是为了让调用点自己说清
/// 「这个文件是私密的」，而不是靠传一个 `Some(0o600)` 让人猜。
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    atomic_write(path, contents, Some(0o600))
}

/// 收窄一个文件的权限。**只在 unix 上有意义**（Windows 的 ACL 不走这条路，
/// 那里靠的是 `%USERPROFILE%` 本身的可见性）。
///
/// `pub(crate)`：审批历史（`approval_log.rs`）也要用 —— 它落盘的是命令原文，
/// 和 token 是同一档东西。
#[cfg(unix)]
pub(crate) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(crate) fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- 原子写 ---------------------------------------------------------------

    /// 每个用例一个独立目录，避免相互干扰（同 hooks.rs 的测试）
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = format!(
                "signalhub-config-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let dir = std::env::temp_dir().join(unique);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = TempDir::new();
        let path = dir.file("config.json");

        atomic_write(&path, "first", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");

        atomic_write(&path, "second", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second", "覆盖要生效");
    }

    /// 同一进程里的两个线程同时写同一个文件 —— 出站钩子的「发送测试」就是并发
    /// 跑所有勾选的地址，每个都可能顺手落盘一次。临时文件名只带 pid 时两边会
    /// 抢同一个名字：一个 rename 成功、另一个 ENOENT，或者互相覆盖（issue #17）。
    #[test]
    fn concurrent_writes_in_one_process_do_not_collide() {
        let dir = TempDir::new();
        let path = dir.file("config.json");
        let writers: Vec<_> = (0..4)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for round in 0..50 {
                        atomic_write(&path, &format!("{{\"w\":{writer},\"r\":{round}}}"), None)
                            .expect("同一个进程里的并发写不该失败");
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }

        // 最后留下的必须是**某一次完整的**写入，不能是两半拼起来的
        let text = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("文件被写坏了");
        assert!(parsed.get("w").is_some() && parsed.get("r").is_some(), "{text}");

        let leftovers = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "临时文件都不该留下");
    }

    /// **不留临时文件。** 半截的 `.config.json.<pid>.<seq>.tmp` 落在用户的
    /// `~/.signalhub` 里，下次只会让人困惑「这是什么、能不能删」。
    #[test]
    fn atomic_write_leaves_no_temp_file_behind() {
        let dir = TempDir::new();
        atomic_write(&dir.file("config.json"), "x", None).unwrap();

        let mut names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["config.json"], "配置目录里只该有目标文件");
    }

    /// 目标是符号链接时写到**它指向的地方**，不能把链接本身替换掉。
    ///
    /// 用 dotfiles 管配置的人常把 `~/.claude/settings.json` 做成链接 —— 就地
    /// `rename` 会把链接变成一个普通文件，用户的仓库就此断掉，而且不会报错。
    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_a_symlink_instead_of_replacing_it() {
        let dir = TempDir::new();
        let real = dir.file("real.json");
        let link = dir.file("link.json");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        atomic_write(&link, "new", None).unwrap();

        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "new",
            "内容要落到链接指向的那个文件上"
        );
        assert!(
            fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "链接本身必须还在，否则 dotfiles 仓库就断了"
        );
    }

    /// 私密文件建出来就该是 0600 —— 不能有「先 world-readable 再收紧」的窗口期。
    #[cfg(unix)]
    #[test]
    fn atomic_write_applies_the_private_mode_up_front() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let path = dir.file("token");
        atomic_write(&path, "s3cret", Some(0o600)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "实际 {mode:o}");
    }

    /// 目标目录不存在时给出错误，而不是 panic —— 调用点要把错误报给用户。
    #[test]
    fn atomic_write_reports_a_missing_directory() {
        let dir = TempDir::new();
        let missing = dir.file("nope").join("config.json");
        assert!(atomic_write(&missing, "x", None).is_err());
    }

    // ---- 导出 / 导入 ----------------------------------------------------------

    /// 导出 → 导入的往返：序列化出来的必须能**一模一样**地读回来。
    ///
    /// 这是「换机器 / 重装后导入回去」的全部承诺。漏一个字段、类型对不上，用户的
    /// 颜色 / 钩子 / 分组就白导了 —— 而且导入那边只会「成功」，不会有任何提示。
    ///
    /// **每一项都设成非默认值**：全用默认的话，漏掉哪个字段都看不出来。
    /// 两个已在迁移里搬走的旧字段（`webhook_url` / `colors`）保持默认：它们标了
    /// `skip_serializing`，本来就不落盘，设了反而会失败。
    #[test]
    fn settings_survive_an_export_import_round_trip() {
        let settings = Settings {
            size: 30,
            // 新字段设成**非默认值**：全默认的话「漏了字段」这种错看不出来
            sign_w_mult: 1.4,
            sign_h_mult: 0.9,
            lamp_layout: LampLayout::PerGroup,
            always_on_top: false,
            snap_to_edge: true,
            approve_tools: true,
            collect_backup: true,
            collect_meeting: true,
            quiet: false,
            notify: false,
            hooks: vec![
                Hook {
                    when: vec!["permission".into(), "error".into()],
                    groups: vec!["builds".into()],
                    // preset / headers 都设成**非默认值**：全默认的话「漏了这个字段」
                    // 这种错在往返里看不出来，而这两个字段正好是新加的
                    action: HookAction::Http {
                        url: "https://example.test/hook".into(),
                        preset: HttpPreset::Slack,
                        headers: [("X-Token".to_string(), "abc".to_string())]
                            .into_iter()
                            .collect(),
                        body: None,
                        content_type: None,
                    },
                },
                Hook {
                    when: vec![],
                    groups: vec![],
                    action: HookAction::Command {
                        command: "notify-send hi".into(),
                    },
                },
            ],
            shape: Shape::Ring,
            palette: Palette::Vivid,
            // 质感五项同样取非默认值，理由同上
            sign_radius: SignRadius::Round,
            sign_border: SignBorder::None,
            sign_tint: SignTint::Warm,
            sign_text: SignText::Large,
            sign_emphasis: SignEmphasis::Flat,
            // 字体与间距同理：都取非默认值，漏字段才验得出来
            sign_font: SignFont::Mono,
            sign_gap: SignGap::Loose,
            // 旧的那四个字段标了 skip_serializing，读回来必然是默认值 —— 夹具里
            // 只能留默认，否则这条 assert_eq 会自己失败
            sign_edge: SignEdge::Off,
            sign_corners: SignCorners::default(),
            sign_edge_trigger: LightTrigger::Always,
            sign_corner_trigger: LightTrigger::Always,
            // 六个灯位全部取非默认：漏一个槽、漏一个 trigger，往返里就该现形。
            // 闪烁故意有开有关 —— 全开或全关都验不出「写回时被统一成同一个值」。
            sign_lights: SignLights {
                top: EdgeSlot {
                    kind: EdgeLight::Glow,
                    trigger: LightTrigger::Attention,
                    blink: true,
                },
                bottom: EdgeSlot {
                    kind: EdgeLight::Fade,
                    trigger: LightTrigger::Schedule,
                    blink: false,
                },
                tl: CornerSlot {
                    kind: SignCorner::State,
                    trigger: LightTrigger::Always,
                    blink: true,
                },
                tr: CornerSlot {
                    kind: SignCorner::Pinned,
                    trigger: LightTrigger::Pinned,
                    blink: false,
                },
                br: CornerSlot {
                    kind: SignCorner::Stale,
                    trigger: LightTrigger::Stale,
                    blink: true,
                },
                bl: CornerSlot {
                    kind: SignCorner::Muted,
                    trigger: LightTrigger::Message,
                    blink: false,
                },
            },
            // 三个触发参数 + 两个标题字段也全部非默认
            sign_trigger_state: "error".into(),
            sign_trigger_from: 8 * 60 + 30,
            sign_trigger_to: 20 * 60 + 15,
            sign_label: SignLabel::Custom,
            sign_label_template: "{session} / {agent}".into(),
            sign_light_scale: LightScale::Thick,
            sign_light_alpha: LightAlpha::Soft,
            sign_light_color: LightColor::Neutral,
            // 轮廓和效果同样取非默认（默认 Rect / Breathe）
            sign_outline: SignOutline::Leaf,
            effect: Effect::Sweep,
            locale: Locale::En,
            // 同理：`Theme` 默认是 `Auto`，这里用 `Dark` 才验得出「漏了这个字段」
            theme: Theme::Dark,
            group_meta: [(
                "builds".to_string(),
                GroupMeta {
                    name: Some("构建".into()),
                    order: Some(2),
                },
            )]
            .into_iter()
            .collect(),
            styles: [(
                "error".to_string(),
                StateStyle {
                    color: Some("#ff0000".into()),
                    shape: Some(Shape::Rounded),
                    blink_ms: Some(0),
                    ttl_ms: None,
                },
            )]
            .into_iter()
            .collect(),
            collapsed: true,
            // 折叠两轴同样取非默认：默认竖排 + 只有灯，用默认值的话
            // 「导出时漏了这两个字段」在往返里看不出来
            fold_axis: FoldAxis::Horizontal,
            fold_label: FoldLabel::State,
            // 新字段设成非默认值：`ClickAction` 默认是 `Collapse`，用默认值的话
            // 「漏了这个字段」在往返里看不出来
            click_action: ClickAction::Inbox,
            show: SignFields {
                name: false,
                state: true,
                age: false,
                message: true,
                // 非默认（默认是 true）：漏了这个字段，往返里看得出来
                dot: false,
            },
            // 非默认、且在合法区间内（50 以下已经低于下限了 —— 样本必须是「真可能出现的」）
            sign_opacity: 72,
            hidden_groups: vec!["noisy".into()],
            blink_speed: 150,
            // `lively` 是迁移输入，读回来必然是默认 true —— 效果档由上面的
            // `effect: Effect::Sweep` 负责取非默认值
            lively: true,
            sound: false,
            quiet_hours: QuietHours {
                enabled: true,
                start: 23,
                end: 7,
            },
            muted_groups: vec!["quiet".into()],
            alert_after_secs: 30,
            // 新字段设成非默认值：默认是那三个「需要你处理」的状态，全默认的话
            // 「漏了这个字段」在往返里看不出来
            notify_when: vec!["done".into(), "stale".into()],
            rule_stuck_push: true,
            rule_busy_silence: true,
            rule_meeting: true,
            tray_follows_state: false,
            group_by: GroupBy::Cwd,
            lan_access: true,
            // 面板也要设成非默认值 —— 它是这条往返里最容易漏的一个（`GET /` 的
            // 那个开关只在服务端读，前端不碰）
            web_panel: true,
            // 面板审批同理：它只被 `panel.rs` 读，前端设置页改的是同一份 JSON
            web_approve: true,
            // 物理灯同理：列表为空时「漏了 devices 字段」这种错看不出来
            devices: vec![Device { host: "192.168.1.50".into() }],
            // 多机同理 —— 而且它比 devices 更容易漏（两个新字段，其中一个还是标量）
            machines: vec![Machine {
                id: "mac-mini".into(),
                url: "http://192.168.1.20:41320".into(),
                token: "t0ken".into(),
                enabled: false,
            }],
            machine_poll_ms: 3000,
            // Call Me 同样设成非默认值（默认是「全空 = 关闭」）
            callme: CallMeConfig {
                url: "https://callme.example.test".into(),
                token: "callme-token".into(),
                poll_ms: 30_000,
            },
            webhook_url: None,
            colors: BTreeMap::new(),
        };

        let json = serde_json::to_string_pretty(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings, "导出再导入必须一模一样：\n{json}");
    }

    /// **老配置照旧**：`{"type":"http","url":"…"}` 里没有 `preset` / `headers`，
    /// 靠两条 `#[serde(default)]` 落到 `raw` + 空表。这是整个 R4 里最该盯住的一条
    /// —— 它要是坏了，所有已经配好转发的人在升级后都会静默变成「发一坨 JSON」。
    #[test]
    fn an_old_http_hook_without_preset_reads_as_raw() {
        let legacy: Hook =
            serde_json::from_str(r#"{"type":"http","url":"https://x/y","when":["error"]}"#)
                .unwrap();
        assert_eq!(
            legacy.action,
            HookAction::Http {
                url: "https://x/y".into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Raw,
                body: None,
                content_type: None,
            }
        );

        // 反过来：`raw` 写出去之后也还能读回来
        let wire = serde_json::to_string(&legacy.action).unwrap();
        assert!(wire.contains(r#""preset":"raw""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<HookAction>(&wire).unwrap(),
            legacy.action
        );
    }

    /// 预设写错一个字母**不该毁掉整份设置**。
    ///
    /// `Settings` 是一个整体，任何一个字段解析失败都会让它整份退回默认值 ——
    /// 组显示名、配色、所有钩子一起没。手写配置时把 `"slack"` 写成 `"Slack"` 是
    /// 很正常的事，所以预设的解析是宽松的：认不出来就落到 `raw`（发 JSON），
    /// 而设置面板上那个下拉本身就写着「原样 JSON」。
    #[test]
    fn a_mistyped_preset_falls_back_to_raw_instead_of_losing_the_config() {
        let raw = r#"{
            "sign_w_mult": 1.4,
            "group_meta": { "builds": { "name": "构建" } },
            "hooks": [
                { "type": "http", "url": "https://x/y", "preset": "Slack" },
                { "type": "http", "url": "https://x/z", "preset": "webhook" },
                { "type": "http", "url": "https://api2.pushdeer.com/message/push?pushkey=K", "preset": "pushdeer" }
            ]
        }"#;
        let settings = serde_json::from_str::<Settings>(raw)
            .expect("不认识的预设不该让整份设置解析失败");
        assert_eq!(settings.sign_w_mult, 1.4, "别的设置一个都不该丢");
        assert_eq!(settings.group_meta["builds"].name.as_deref(), Some("构建"));
        assert_eq!(
            settings.hooks.len(),
            3,
            "钩子也不该被丢掉 —— 它们只是多发一坨 JSON"
        );
        // 大小写写错的那个能认出来，完全没见过的落到 raw
        assert_eq!(
            settings.hooks[0].action,
            HookAction::Http {
                url: "https://x/y".into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Slack,
                body: None,
                content_type: None,
            }
        );
        assert_eq!(settings.hooks[1].action.target(), "https://x/z");
        // **每个预设的名字都得认得出来**：认不出来的会静默落到 `raw`，于是
        // 「配了 PushDeer」变成「发了一坨 JSON 过去」—— 手机一声不响，而面板上
        // 那个下拉显示的却是「原样 JSON」，用户得自己发现。
        assert_eq!(
            settings.hooks[2].action,
            HookAction::Http {
                url: "https://api2.pushdeer.com/message/push?pushkey=K".into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Pushdeer,
                body: None,
                content_type: None,
            }
        );
    }

    /// 请求头的规整：去空白、丢掉空键、按上限截断（**确定的**截断）。
    #[test]
    fn extra_headers_are_trimmed_and_capped() {
        let mut headers = BTreeMap::new();
        headers.insert("  X-Token  ".to_string(), "  abc  ".to_string());
        headers.insert("   ".to_string(), "没人要的值".to_string());
        headers.insert("".to_string(), "也没人要".to_string());
        let clean = normalize_headers(headers);
        assert_eq!(
            clean,
            [("X-Token".to_string(), "abc".to_string())]
                .into_iter()
                .collect(),
            "键值都该去空白，空键那条该被丢掉"
        );

        // 条数上限：多出来的按键的字典序丢，丢哪几条是确定的
        let many: BTreeMap<String, String> = (0..MAX_HOOK_HEADERS + 5)
            .map(|index| (format!("X-H{index:03}"), "v".to_string()))
            .collect();
        let capped = normalize_headers(many);
        assert_eq!(capped.len(), MAX_HOOK_HEADERS);
        assert!(capped.contains_key("X-H000"), "前几个该留着");
        assert!(!capped.contains_key("X-H020"), "超出的该丢掉");

        // 经过 clamped() 也一样（手改的配置文件走的是这条路）
        let raw = r#"{"hooks":[{"type":"http","url":"https://x/y",
            "headers":{" A ":" 1 ","  ":""}}]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        let HookAction::Http { headers, .. } = &settings.hooks[0].action else {
            panic!("应是一条 http 钩子");
        };
        assert_eq!(
            headers,
            &[("A".to_string(), "1".to_string())].into_iter().collect(),
            "clamped() 也要规整请求头"
        );
    }

    /// 转发摘要里的组名是**用户看到的那个**：用户改的名字 > 服务端默认名 > 原始组名。
    /// 和前端 `lampSort.ts::labelOf` 是同一条三级规则 —— 两处算出不同的名字，就是
    /// 「转发里写着 build-1、灯上写着构建」这种两处真值打架。
    #[test]
    fn display_label_follows_the_same_three_levels_as_the_lamp() {
        let group = |name: &str, default_label: Option<&str>| crate::state::GroupView {
            name: name.to_string(),
            default_label: default_label.map(str::to_string),
            aggregate: crate::state::Signal::Idle,
            message: None,
            since_ms: 0,
            stale_progress: None,
            sessions: Vec::new(),
        };

        let mut settings = Settings::default();
        // 三级里最低的一级：什么都没有就是原始组名
        assert_eq!(settings.display_label(&group("build-1", None)), "build-1");
        // 中间那一级：会话分组出来的组有 default_label
        assert_eq!(
            settings.display_label(&group("0191f0aa", Some("proj · claude"))),
            "proj · claude"
        );
        // 最高一级：用户起的名字压过 default_label
        settings.group_meta.insert(
            "0191f0aa".to_string(),
            GroupMeta {
                name: Some("构建".into()),
                order: None,
            },
        );
        assert_eq!(
            settings.display_label(&group("0191f0aa", Some("proj · claude"))),
            "构建"
        );
    }

    /// 导入一个坏文件要**报错**（而不是静默套用默认值）。
    ///
    /// 静默退回默认值在「启动时读配置」那条路上是对的（总得起得来），但在
    /// 「用户主动点导入」这条路上是错的 —— 用户以为导进去了，其实什么都没变。
    #[test]
    fn importing_a_broken_file_is_an_error_not_a_silent_default() {
        let parsed = serde_json::from_str::<Settings>("{ 这不是 json");
        assert!(parsed.is_err(), "坏文件必须报错");
    }

    /// 真实踩到的坑：加了 `notify` 字段之后，旧版本写的 config.json 里没有它。
    /// 若不加 `serde(default)`，整份解析会失败、退回全默认 —— 用户已经调好的
    /// 圆点尺寸会被静默重置。这条测试锁住「缺字段只退回该字段的默认值」。
    #[test]
    fn settings_tolerate_missing_fields_from_older_versions() {
        let old = r#"{"size": 32, "always_on_top": false, "quiet": false}"#;
        let settings: Settings = serde_json::from_str(old).unwrap();

        assert_eq!(settings.size, 32, "用户调过的值必须保留");
        assert!(!settings.always_on_top);
        assert!(!settings.quiet);
        assert!(settings.notify, "缺的字段退回该字段的默认值");
        assert!(!settings.approve_tools, "会改智能体行为的开关，缺字段必须是关的");
        assert!(
            !settings.web_approve,
            "会让审批详情进浏览器页面的开关，缺字段必须是关的"
        );
        assert!(!settings.collect_backup, "读系统状态的采集器，缺字段必须是关的");
        assert!(!settings.rule_stuck_push, "会改变打扰时机的预设，缺字段必须是关的");
        assert!(!settings.rule_busy_silence, "会改变打扰时机的预设，缺字段必须是关的");
        assert!(!settings.rule_meeting, "会改变打扰时机的预设，缺字段必须是关的");
        assert!(!settings.collect_meeting, "读系统状态的采集器，缺字段必须是关的");
        assert!(settings.colors.is_empty(), "缺的颜色表是空的，不是解析失败");
    }

    #[test]
    fn hex_colors_are_checked_by_shape_only() {
        for good in ["#fff", "#FFFFFF", "#16a34a", "#000000"] {
            assert!(is_hex_color(good), "{good} 应被接受");
        }
        for bad in ["16a34a", "#12345", "#gggggg", "", "#", "red"] {
            assert!(!is_hex_color(bad), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn hook_urls_are_checked_at_the_boundary() {
        for good in [
            "https://ntfy.sh/my-topic",
            "http://127.0.0.1:8080/hook",
            "  https://hooks.slack.com/services/x  ",
        ] {
            assert!(is_http_url(good), "{good} 应被接受");
        }
        for bad in ["", "ntfy.sh/topic", "ftp://x/y", "https://", "https:///path", "  "] {
            assert!(!is_http_url(bad), "{bad:?} 应被拒绝");
        }
    }

    fn http_hook(when: &[&str], url: &str) -> Hook {
        Hook {
            when: when.iter().map(|s| s.to_string()).collect(),
            groups: Vec::new(),
            action: HookAction::http(url),
        }
    }

    fn custom_hook(url: &str, body: &str) -> Hook {
        Hook {
            when: Vec::new(),
            groups: Vec::new(),
            action: HookAction::Http {
                url: url.into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Custom,
                body: Some(body.into()),
                content_type: None,
            },
        }
    }

    /// 自定义模板的校验：只认 `{snake_case}` 形状的占位符，普通 JSON 的花括号
    /// 不算；写错了要说清能用的是哪几个。
    #[test]
    fn custom_templates_are_validated_against_the_known_placeholders() {
        assert!(
            hook_problem(&custom_hook("https://x/y", r#"{"text":"{state}","since":{since_ms}}"#))
                .is_none()
        );
        // 普通 JSON 里的花括号不是占位符
        assert!(hook_problem(&custom_hook("https://x/y", r#"{"a":{"b":1},"c":{}}"#)).is_none());

        let typo = hook_problem(&custom_hook("https://x/y", r#"{"text":"{stste}"}"#)).unwrap();
        assert!(typo.contains("{stste}"), "{typo}");
        assert!(typo.contains("{state}"), "要列出可用的占位符：{typo}");
    }

    /// 选了「自定义」却还没写 body：**不触发，但也不算配置错误** —— 和「刚添加、
    /// 还没填地址」同一种中间态。否则新加一条钩子会立刻顶上一条红线。
    #[test]
    fn an_empty_custom_template_is_not_configured_but_not_an_error() {
        let hook = custom_hook("https://x/y", "   ");
        assert!(hook_problem(&hook).is_none());
        assert!(!hook.action.is_configured(), "空模板不该发出去");

        assert!(custom_hook("https://x/y", r#"{"s":"{state}"}"#).action.is_configured());
    }

    #[test]
    fn hook_problems_are_named_precisely() {
        assert!(hook_problem(&http_hook(&["permission"], "https://x/y")).is_none());
        // 空 when 是合法的：任何变化都触发
        assert!(hook_problem(&http_hook(&[], "https://x/y")).is_none());

        let bad_state = http_hook(&["nonsense"], "https://x/y");
        assert!(hook_problem(&bad_state).unwrap().contains("未知的状态名"));

        let bad_url = http_hook(&["idle"], "not-a-url");
        assert!(hook_problem(&bad_url).unwrap().contains("不合法"));

        // 空目标是「还没配好」，不算错 —— 否则编辑器里刚点「添加」就报错
        assert!(hook_problem(&http_hook(&[], "")).is_none());
        let empty_command = Hook {
            when: vec![],
            groups: Vec::new(),
            action: HookAction::Command { command: "   ".into() },
        };
        assert!(hook_problem(&empty_command).is_none());
        assert!(!empty_command.action.is_configured(), "空命令不触发");
    }

    #[test]
    fn an_empty_hook_target_is_not_configured() {
        assert!(!http_hook(&[], "").action.is_configured());
        assert!(http_hook(&[], "https://x/y").action.is_configured());
        assert!(!HookAction::Command { command: "  ".into() }.is_configured());
        assert!(HookAction::Command { command: "say hi".into() }.is_configured());
    }

    /// 派生状态也能作为过滤条件 —— 用户可能只想知道「会话沉默了」
    #[test]
    fn hooks_may_filter_on_derived_states() {
        assert!(hook_problem(&http_hook(&["stale", "offline"], "https://x/y")).is_none());
    }

    /// 旧版本只有一条 webhook 地址，升级后不能让它静默失效
    #[test]
    fn a_legacy_webhook_becomes_a_hook() {
        let legacy = r##"{"webhook_url": "https://ntfy.sh/x"}"##;
        let settings = migrate_legacy_webhook(serde_json::from_str::<Settings>(legacy).unwrap());

        assert_eq!(settings.hooks.len(), 1, "应搬成一条钩子");
        assert_eq!(
            settings.hooks[0].when,
            vec!["permission", "attention", "error"],
            "触发条件要和旧行为一致（只在需要注意时转发）"
        );
        assert_eq!(
            settings.hooks[0].action,
            HookAction::http("https://ntfy.sh/x")
        );

        // 空白的旧值不该凭空造出一条钩子
        let blank = migrate_legacy_webhook(
            serde_json::from_str::<Settings>(r##"{"webhook_url": "  "}"##).unwrap(),
        );
        assert!(blank.hooks.is_empty());
    }

    /// 废弃字段永不写回 —— 出口只有一条路
    #[test]
    fn the_legacy_webhook_field_is_never_written_back() {
        let settings = Settings {
            webhook_url: Some("https://x/y".into()),
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("webhook_url"), "不该写回废弃字段: {json}");
    }

    /// 手改配置文件时写坏的钩子只丢掉那一条
    #[test]
    fn clamping_drops_bad_hooks_but_keeps_the_rest() {
        let raw = r##"{"size": 24, "hooks": [
            {"when": ["error"], "type": "http", "url": "https://x/y"},
            {"when": ["nope"], "type": "http", "url": "https://x/y"},
            {"when": [], "type": "http", "url": "not-a-url"},
            {"when": [], "type": "command", "command": ""}
        ]}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.size, 24, "其余设置不受影响");
        assert_eq!(settings.hooks.len(), 2, "未知状态名和坏地址应被丢掉，空命令留下");
        assert_eq!(settings.hooks[0].when, vec!["error"]);
        assert!(!settings.hooks[1].action.is_configured(), "空命令那条还在，但不触发");
    }

    /// 手改的配置文件里颜色写坏了不该让整份设置失效 —— 只丢掉那一条
    #[test]
    fn clamping_drops_bad_styles_but_keeps_the_rest() {
        // 用 r##"..."## ：内容里有 "# ，单个 # 会提前结束原始字符串
        let raw = r##"{"size": 26, "styles": {
            "error": {"color": "#FF0000"},
            "idle":  {"color": "not-a-color", "blink_ms": 900},
            "nope":  {"color": "#fff"}
        }}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.size, 26, "其余设置不受影响");
        assert_eq!(
            settings.styles["error"].color.as_deref(),
            Some("#ff0000"),
            "合法的颜色要保留，并统一成小写"
        );
        assert_eq!(
            settings.styles["idle"].blink_ms,
            Some(900),
            "颜色写坏了只丢颜色，同一状态上别的覆盖要留着"
        );
        assert_eq!(settings.styles["idle"].color, None, "坏色值应被丢掉");
        // 键名合法性由 set_settings 校验（会报错），clamped 只保证值可用
        assert!(settings.styles.contains_key("nope"));
    }

    /// 旧版本用一张扁平的 `colors` 表存颜色，升级后不能让它静默失效
    #[test]
    fn a_legacy_color_table_moves_into_styles() {
        let raw = r##"{"colors": {"error": "#ff0000", "idle": "#00ff00"}}"##;
        let settings =
            migrate_legacy_colors(serde_json::from_str::<Settings>(raw).unwrap()).clamped();

        assert_eq!(settings.styles["error"].color.as_deref(), Some("#ff0000"));
        assert_eq!(settings.styles["idle"].color.as_deref(), Some("#00ff00"));
        assert!(settings.colors.is_empty(), "搬完就该空了");

        // 两处都写了同一个状态时，新的 styles 说了算
        let both = r##"{"colors": {"error": "#111111"}, "styles": {"error": {"color": "#222222"}}}"##;
        let settings =
            migrate_legacy_colors(serde_json::from_str::<Settings>(both).unwrap()).clamped();
        assert_eq!(settings.styles["error"].color.as_deref(), Some("#222222"));
    }

    /// 废弃字段永不写回 —— 一个状态的外观只有一处表达
    #[test]
    fn the_legacy_color_field_is_never_written_back() {
        let mut settings = Settings::default();
        settings.colors.insert("error".into(), "#ff0000".into());
        settings.styles.insert("idle".into(), StateStyle::default());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("\"colors\""), "不该写回废弃字段: {json}");
    }

    /// 三样都没写的条目没有作用，留着会让「预设是否还生效」的判断无谓地失败
    #[test]
    fn empty_style_entries_are_dropped() {
        let raw = r##"{"styles": {"idle": {}, "error": {"shape": "ring"}}}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert!(!settings.styles.contains_key("idle"), "空条目应被丢掉");
        assert_eq!(settings.styles["error"].shape, Some(Shape::Ring));
    }

    /// 0 是「常亮」，是合法值；其余夹回有意义的区间 —— 太快只剩模糊，太慢像卡住
    #[test]
    fn per_state_blink_allows_zero_but_clamps_everything_else() {
        let raw = r##"{"styles": {
            "idle": {"blink_ms": 0},
            "thinking": {"blink_ms": 1},
            "working": {"blink_ms": 60000},
            "done": {"blink_ms": 700}
        }}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.styles["idle"].blink_ms, Some(0), "0 表示常亮，要放行");
        assert_eq!(settings.styles["thinking"].blink_ms, Some(MIN_BLINK_MS));
        assert_eq!(settings.styles["working"].blink_ms, Some(MAX_BLINK_MS));
        assert_eq!(settings.styles["done"].blink_ms, Some(700), "区间内原样保留");
    }

    /// 勿扰时段跨午夜是常态，不是特例
    #[test]
    fn quiet_hours_cover_the_midnight_wrap() {
        // 22:00 – 08:00
        let night = QuietHours { enabled: true, start: 22, end: 8 };
        for hour in [22, 23, 0, 1, 7] {
            assert!(night.covers(hour), "{hour} 点应在勿扰时段内");
        }
        for hour in [8, 12, 21] {
            assert!(!night.covers(hour), "{hour} 点不该在勿扰时段内");
        }

        // 不跨午夜的窗口（比如午休 12:00–14:00）
        let noon = QuietHours { enabled: true, start: 12, end: 14 };
        assert!(noon.covers(12));
        assert!(noon.covers(13));
        assert!(!noon.covers(14), "结束那一小时不含在内");
        assert!(!noon.covers(11));
    }

    /// 关掉开关就完全不管时间；起止相同读作「空窗口」而不是「全天」
    #[test]
    fn quiet_hours_degrade_safely() {
        let off = QuietHours { enabled: false, start: 0, end: 23 };
        for hour in 0..24 {
            assert!(!off.covers(hour), "没启用就不该有任何小时被算进去");
        }

        let equal = QuietHours { enabled: true, start: 9, end: 9 };
        for hour in 0..24 {
            assert!(
                !equal.covers(hour),
                "起止相同应读作空窗口 —— 手滑调成一样不该变成全天静音"
            );
        }

        assert!(!QuietHours::default().enabled, "默认不启用");
    }

    /// 手改的配置里小时可以是 200，夹回 0–23
    #[test]
    fn quiet_hours_are_clamped_to_real_hours() {
        let raw = r#"{"quiet_hours": {"enabled": true, "start": 200, "end": 250}}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.quiet_hours.start, 23);
        assert_eq!(settings.quiet_hours.end, 23);
    }

    /// 静音列表和隐藏列表用同一套规整规则 —— 两处各写一遍迟早分叉
    #[test]
    fn muted_groups_are_normalized_like_hidden_ones() {
        let raw = r#"{"hidden_groups": [" ci ", "ci", ""], "muted_groups": ["work", " work ", "zeta", "alpha"]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.hidden_groups, vec!["ci"]);
        assert_eq!(settings.muted_groups, vec!["alpha", "work", "zeta"]);
    }

    /// 阈值告警的秒数也要夹紧：手改成一个巨大的值等于永久不提醒
    #[test]
    fn the_alert_delay_is_clamped() {
        let raw = r#"{"alert_after_secs": 60000}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.alert_after_secs, MAX_ALERT_DELAY_SECS);
    }

    /// 牌面不透明度有**下限**，不只是上限：α 压到一半以下时牌里的浅字就落在桌面上，
    /// 而描边那套补救已经被删掉了（见 `MIN_SIGN_OPACITY`）。
    ///
    /// 这条测试钉的是「老配置能被救回来」：0.6.2 之前的版本允许 0–100，升级上来的人
    /// 配置文件里可能就是 10 这种值 —— 读一次设置就该被抬到 65，而不是让灯一直糊着。
    #[test]
    fn the_sign_opacity_has_a_floor() {
        for raw in [r#"{"sign_opacity": 0}"#, r#"{"sign_opacity": 49}"#] {
            let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
            assert_eq!(settings.sign_opacity, MIN_SIGN_OPACITY, "{raw}");
        }
        // 合法值原样保留（别把下限写成了「夹到 65」）
        let settings = serde_json::from_str::<Settings>(r#"{"sign_opacity": 72}"#)
            .unwrap()
            .clamped();
        assert_eq!(settings.sign_opacity, 72);
        // 上限照旧
        let settings = serde_json::from_str::<Settings>(r#"{"sign_opacity": 255}"#)
            .unwrap()
            .clamped();
        assert_eq!(settings.sign_opacity, 100);
    }

    // ---- 通知的状态过滤 -------------------------------------------------------

    /// 老配置里没有这个字段 —— 落到的必须是**加开关之前**的那三个状态，
    /// 否则升级上来的用户会在每一次 `working` / `done` 时被弹一下。
    #[test]
    fn an_old_config_notifies_on_the_three_urgent_states() {
        let settings = serde_json::from_str::<Settings>("{}").unwrap().clamped();
        assert_eq!(settings.notify_when, vec!["permission", "attention", "error"]);

        let wants = |state| settings.notify_wants(state);
        assert!(wants(crate::state::Signal::Permission));
        assert!(wants(crate::state::Signal::Attention));
        assert!(wants(crate::state::Signal::Error));
        for state in [
            crate::state::Signal::Working,
            crate::state::Signal::Done,
            crate::state::Signal::Idle,
            crate::state::Signal::Stale,
        ] {
            assert!(!wants(state), "{state:?} 不该在默认集合里");
        }
    }

    /// 默认集合必须是**能勾选**的集合的子集 —— 否则界面上一开始就有三项是死的
    #[test]
    fn the_default_states_are_all_selectable() {
        for name in DEFAULT_NOTIFY_STATES {
            assert!(
                NOTIFY_STATES.contains(&name),
                "{name} 在默认集合里却选不了"
            );
        }
        // 派生状态里只有 stale 给了通知，offline / off 不给（见 NOTIFY_STATES 注释）
        assert!(!NOTIFY_STATES.contains(&"offline"));
        assert!(!NOTIFY_STATES.contains(&"off"));
        assert!(NOTIFY_STATES.contains(&"stale"));
    }

    /// 手改配置里写坏的值要被丢掉，而不是让整份设置退回默认
    #[test]
    fn notify_states_are_normalized() {
        let raw = r#"{"notify_when": [" Done ", "DONE", "nonsense", "offline"]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(
            settings.notify_when,
            vec!["done"],
            "去空白、统一小写、去重，认不出来的（含不能选的 offline）丢掉"
        );
    }

    /// **一个都认不出来时退回默认**：留成空表的话，表现是「通知开着却永远不弹」，
    /// 而界面上没有任何线索指向这里。
    #[test]
    fn an_unreadable_notify_list_falls_back_to_the_default() {
        let raw = r#"{"notify_when": ["nonsense", "offline"]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.notify_when, vec!["permission", "attention", "error"]);
    }

    /// 但**显式清空**是用户的真实意图（一个都不弹），不能被兜底顶掉
    #[test]
    fn an_explicitly_empty_notify_list_stays_empty() {
        let raw = r#"{"notify_when": []}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert!(settings.notify_when.is_empty());
        assert!(!settings.notify_wants(crate::state::Signal::Permission));
    }

    /// 报错要指出是哪个名字不能用 —— 这条只在导入一份手改过的配置时才会走到
    #[test]
    fn a_bad_notify_state_is_named_precisely() {
        assert!(notify_when_problem(&["done".into(), "stale".into()]).is_none());
        let problem = notify_when_problem(&["offline".into()]).unwrap();
        assert!(problem.contains("offline"), "{problem}");
        assert!(problem.contains("permission"), "报错里要列出可用的名字：{problem}");
    }

    // ---- 逐组显示偏好 ---------------------------------------------------------

    #[test]
    fn group_names_are_trimmed_and_empty_entries_are_dropped() {
        let raw = r#"{"group_meta": {
            "ci":   {"name": "  构建  ", "order": 0},
            "work": {"name": "   "},
            "docs": {},
            "x":    {"order": 2}
        }}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.group_label("ci"), "构建", "两头空白要去掉");
        assert!(!settings.group_meta.contains_key("work"), "全空白的名字等于没设");
        assert!(!settings.group_meta.contains_key("docs"), "空条目没有作用，丢掉");
        assert_eq!(settings.group_label("x"), "x", "只有置顶、没设名字的条目要留着");
        assert_eq!(settings.group_order("x"), Some(2));
    }

    /// 显示名只是显示：没设就回落到上报方给的原始组名
    #[test]
    fn the_group_label_falls_back_to_the_raw_id() {
        let mut settings = Settings::default();
        assert_eq!(settings.group_label("build-1"), "build-1", "没设就该原样显示");

        settings
            .group_meta
            .insert("build-1".into(), GroupMeta { name: Some("构建".into()), order: None });
        assert_eq!(settings.group_label("build-1"), "构建");
    }

    #[test]
    fn a_group_is_pinned_only_when_it_has_an_order() {
        let settings = Settings::default();
        assert_eq!(settings.group_order("ci"), None, "默认不置顶");

        let raw = r#"{"group_meta": {"ci": {"order": 0}, "work": {"name": "工作"}}}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.group_order("ci"), Some(0), "0 也是有效的置顶权重");
        assert_eq!(settings.group_order("work"), None, "只有显示名不算置顶");
    }

    #[test]
    fn an_empty_object_yields_all_defaults() {
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }

    /// 默认必须是「展开」：灯的本职是让人一眼看到状态，折叠是用户主动要的例外
    #[test]
    fn the_default_shape_is_expanded_and_the_sign_shows_everything() {
        let settings = Settings::default();
        assert!(!settings.collapsed, "默认应是展开的牌面");
        assert_eq!(
            settings.show,
            SignFields { name: true, state: true, age: true, message: true, dot: true },
            "默认每段都显示"
        );
        assert!(settings.hidden_groups.is_empty(), "默认不藏任何组");
        assert_eq!(settings.sign_opacity, DEFAULT_SIGN_OPACITY);
        assert_eq!(settings.blink_speed, 100, "100 = 原始节奏");
    }

    /// 质感五项（圆角 / 描边 / 底色 / 字号 / 分层）的默认值必须**等于加这些字段
    /// 之前的样子** —— 否则升级上来的用户会莫名其妙觉得灯换了张脸。
    ///
    /// 这条测试钉的是「默认值」而不是具体像素：像素在 `lamp.css` 的
    /// `[data-sign-*]` 规则里，这里只保证选中的是那一档。
    #[test]
    fn the_sign_texture_defaults_to_the_old_look() {
        let settings = Settings::default();
        assert_eq!(settings.sign_radius, SignRadius::Standard, "默认是原来的 0.32×size");
        assert_eq!(settings.sign_border, SignBorder::Hairline, "默认保留那道 1px 边");
        assert_eq!(settings.sign_tint, SignTint::Graphite, "默认还是偏冷的深灰");
        assert_eq!(settings.sign_text, SignText::Standard, "默认字号不动");
        assert_eq!(settings.sign_emphasis, SignEmphasis::Auto, "默认保留 need / calm 分层");
        assert_eq!(settings.sign_font, SignFont::System, "默认还是系统无衬线 + 等宽消息");
        assert_eq!(settings.sign_gap, SignGap::Standard, "默认间距还是 size / 2");

        // 旧配置里一个字段都没有，也不能因此让整份设置失效
        let old: Settings = serde_json::from_str(r#"{"size": 22}"#).unwrap();
        assert_eq!(old.sign_radius, SignRadius::Standard);
        assert_eq!(old.sign_emphasis, SignEmphasis::Auto);
        assert_eq!(old.sign_font, SignFont::System);
        assert_eq!(old.sign_gap, SignGap::Standard);
        assert!(old.show.dot, "旧配置缺 dot：牌上的圆点必须还在（红线 3）");
    }

    /// 两项都存字符串枚举，改一个值要能原样存回去 —— 序列化往返丢值会让设置
    /// 「点了没反应」，而那种 bug 在界面上看不出任何线索。
    #[test]
    fn the_sign_texture_roundtrips_through_json() {
        let raw = r#"{
            "sign_radius": "round",
            "sign_border": "none",
            "sign_tint": "slate",
            "sign_text": "compact",
            "sign_emphasis": "flat",
            "sign_font": "mono",
            "sign_gap": "loose"
        }"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap();
        assert_eq!(settings.sign_radius, SignRadius::Round);
        assert_eq!(settings.sign_border, SignBorder::None);
        assert_eq!(settings.sign_tint, SignTint::Slate);
        assert_eq!(settings.sign_text, SignText::Compact);
        assert_eq!(settings.sign_emphasis, SignEmphasis::Flat);
        assert_eq!(settings.sign_font, SignFont::Mono);
        assert_eq!(settings.sign_gap, SignGap::Loose);

        let back = serde_json::to_string(&settings).unwrap();
        assert!(back.contains(r#""sign_radius":"round""#));
        assert!(back.contains(r#""sign_tint":"slate""#));
        assert!(back.contains(r#""sign_emphasis":"flat""#));
        assert!(back.contains(r#""sign_font":"mono""#));
        assert!(back.contains(r#""sign_gap":"loose""#));
    }

    /// 六个灯位默认全关 / 全空：加这些字段之前，牌的四面和四角本来就是空的
    /// （红线 3）。轮廓和效果也取「等于没加过」的那一档。
    ///
    /// 这些是**观感**的默认值而不是行为的：改错了不会有任何测试失败，只会让
    /// 升级上来的用户看到一盏忽然发光、四角冒点的灯。
    #[test]
    fn the_six_light_slots_default_to_off() {
        let settings = Settings::default();
        assert_eq!(settings.sign_lights, SignLights::default());
        assert_eq!(settings.sign_lights.corner_count(), 0);
        assert_eq!(settings.sign_lights.top.kind, EdgeLight::Off);
        assert_eq!(settings.sign_lights.top.trigger, LightTrigger::Always);
        assert_eq!(settings.sign_light_scale, LightScale::Normal);
        assert_eq!(settings.sign_light_alpha, LightAlpha::Normal);
        assert_eq!(settings.sign_light_color, LightColor::State);
        assert_eq!(settings.sign_outline, SignOutline::Rect);
        assert_eq!(settings.effect, Effect::Breathe, "默认 == 旧 lively=true");
        assert_eq!(settings.sign_label, SignLabel::Session);
        assert_eq!(settings.sign_label_template, DEFAULT_SIGN_LABEL_TEMPLATE);
        assert_eq!(settings.sign_trigger_state, DEFAULT_LIGHT_TRIGGER_STATE);
        assert_eq!(settings.sign_trigger_from, DEFAULT_LIGHT_TRIGGER_FROM);
        assert_eq!(settings.sign_trigger_to, DEFAULT_LIGHT_TRIGGER_TO);

        // 0.6.2 写出来的 config.json 里这些键一个都没有，不能因此整份失效
        let old: Settings = serde_json::from_str(r#"{"size": 22}"#).unwrap();
        assert_eq!(old.sign_lights, SignLights::default());
        assert_eq!(old.sign_light_scale, LightScale::Normal);
        assert_eq!(old.effect, Effect::Breathe);
        assert_eq!(old.sign_outline, SignOutline::Rect);
        assert_eq!(old.sign_label, SignLabel::Session);
    }

    /// 六个槽可以只写一部分 —— `SignLights` / `EdgeSlot` / `CornerSlot` 各自有
    /// `serde(default)`。**这条不是洁癖**：手改 config.json 的人是按需写的，只写
    /// `{"br": {"kind": "muted"}}` 很自然；没有字段级默认，那份文件会让整份设置
    /// 解析失败，然后被当成损坏静默重置 —— 一句话丢掉他所有配置。
    #[test]
    fn a_half_written_slot_set_still_parses() {
        let settings: Settings = serde_json::from_str(
            r#"{"sign_lights": {"top": {"kind": "glow"}, "br": {"kind": "muted", "trigger": "pinned"}}}"#,
        )
        .unwrap();
        assert_eq!(settings.sign_lights.top.kind, EdgeLight::Glow);
        assert_eq!(
            settings.sign_lights.top.trigger,
            LightTrigger::Always,
            "没写的触发是常亮"
        );
        assert_eq!(
            settings.sign_lights.bottom.kind,
            EdgeLight::Off,
            "没写的边是关"
        );
        assert!(
            !settings.sign_lights.top.blink,
            "没写的闪烁是常亮 —— 0.6.6 之前写下的配置读进来不能突然开始闪"
        );
        assert_eq!(settings.sign_lights.br.kind, SignCorner::Muted);
        assert_eq!(settings.sign_lights.br.trigger, LightTrigger::Pinned);
        assert_eq!(
            settings.sign_lights.tl.kind,
            SignCorner::Empty,
            "没写的角是空"
        );
        assert_eq!(settings.sign_lights.corner_count(), 1);
    }

    /// 六个灯位各自的画法 / 触发都要能原样存回去 —— 存不回去的表现是
    /// 「设置点了没反应」，而那种 bug 在界面上没有任何线索。
    #[test]
    fn the_six_light_slots_roundtrip_through_json() {
        let raw = r#"{
            "sign_lights": {
                "top": {"kind": "fade", "trigger": "attention", "blink": true},
                "bottom": {"kind": "glow", "trigger": "schedule"},
                "tl": {"kind": "state", "trigger": "always"},
                "tr": {"kind": "pinned", "trigger": "pinned", "blink": true},
                "br": {"kind": "stale", "trigger": "stale"},
                "bl": {"kind": "passthrough", "trigger": "active"}
            }
        }"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap();
        assert_eq!(settings.sign_lights.top.kind, EdgeLight::Fade);
        assert_eq!(settings.sign_lights.bottom.trigger, LightTrigger::Schedule);
        assert_eq!(settings.sign_lights.bl.kind, SignCorner::Passthrough);
        assert_eq!(settings.sign_lights.corner_count(), 4);
        // 闪烁是逐槽的开关：写了的要读回来，没写的保持常亮。两边都对，
        // 「给一角打开闪烁，四个角一起闪」才算真的不存在。
        assert!(settings.sign_lights.top.blink);
        assert!(settings.sign_lights.tr.blink);
        assert!(!settings.sign_lights.bottom.blink);
        assert!(!settings.sign_lights.br.blink);

        let back = serde_json::to_string(&settings).unwrap();
        for needle in [
            r#""sign_lights":{"#,
            r#""top":{"kind":"fade","trigger":"attention","blink":true}"#,
            r#""bottom":{"kind":"glow","trigger":"schedule","blink":false}"#,
            r#""bl":{"kind":"passthrough","trigger":"active","blink":false}"#,
        ] {
            assert!(back.contains(needle), "序列化漏了 {needle}: {back}");
        }
        // 旧字段是迁移输入，永不写回 —— 否则每次启动都会把新设置盖回旧样子
        for gone in [
            "sign_edge",
            "sign_corners",
            "sign_edge_trigger",
            "sign_corner_trigger",
            "lively",
        ] {
            assert!(
                !back.contains(&format!("\"{gone}\"")),
                "旧字段 {gone} 不该再落盘: {back}"
            );
        }
    }

    /// 触发参数、三把尺子、标题、轮廓、效果、形状都要能原样存回去。
    ///
    /// 它们散落在外观 / 行为两页，而 `set_settings` 是**整份替换**：任何一处只发
    /// 自己那页的字段，另一页设过的东西就会悄悄回到默认值 —— 表现是「行为页调完，
    /// 回外观页看一眼，触发方式自己变了」。前端那侧由整份 patch 保证。
    #[test]
    fn the_light_params_outline_effect_and_shape_roundtrip() {
        let raw = r#"{
            "sign_trigger_state": "error",
            "sign_trigger_from": 510,
            "sign_trigger_to": 1215,
            "sign_light_scale": "thick",
            "sign_light_alpha": "soft",
            "sign_light_color": "neutral",
            "sign_label": "custom",
            "sign_label_template": "{agent} / {session}",
            "sign_outline": "leaf",
            "effect": "sweep",
            "shape": "diamond",
            "sign_lights": {"tl": {"kind": "passthrough", "trigger": "state"}}
        }"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap();
        assert_eq!(settings.sign_trigger_state, "error");
        assert_eq!(settings.sign_trigger_from, 510);
        assert_eq!(settings.sign_trigger_to, 1215);
        assert_eq!(settings.sign_light_scale, LightScale::Thick);
        assert_eq!(settings.sign_light_alpha, LightAlpha::Soft);
        assert_eq!(settings.sign_light_color, LightColor::Neutral);
        assert_eq!(settings.sign_label, SignLabel::Custom);
        assert_eq!(settings.sign_outline, SignOutline::Leaf);
        assert_eq!(settings.effect, Effect::Sweep);
        assert_eq!(settings.shape, Shape::Diamond);
        assert_eq!(settings.sign_lights.tl.kind, SignCorner::Passthrough);

        let back = serde_json::to_string(&settings).unwrap();
        for needle in [
            r#""sign_trigger_state":"error""#,
            r#""sign_light_scale":"thick""#,
            r#""sign_light_color":"neutral""#,
            r#""sign_label":"custom""#,
            r#""sign_outline":"leaf""#,
            r#""effect":"sweep""#,
            r#""shape":"diamond""#,
            r#""tl":{"kind":"passthrough","trigger":"state","blink":false}"#,
        ] {
            assert!(back.contains(needle), "序列化漏了 {needle}: {back}");
        }
    }

    /// 0.6.4 的配置（一条整轨 + 四角共用一个触发）要能升级成六个独立灯位。
    /// 不搬的话，用户已经挂好的角灯和边灯会在升级后一夜之间全没了。
    #[test]
    fn legacy_light_slots_are_migrated_into_six_slots() {
        let raw = r#"{
            "sign_edge": "glow",
            "sign_edge_trigger": "attention",
            "sign_corners": {"tl": "state", "tr": "pinned", "br": "stale", "bl": "muted"},
            "sign_corner_trigger": "quiet"
        }"#;
        let settings = migrate_legacy_light_slots(serde_json::from_str(raw).unwrap());
        assert_eq!(settings.sign_lights.top.kind, EdgeLight::Glow);
        assert_eq!(settings.sign_lights.bottom.kind, EdgeLight::Glow);
        assert_eq!(settings.sign_lights.top.trigger, LightTrigger::Attention);
        assert_eq!(settings.sign_lights.bottom.trigger, LightTrigger::Attention);
        assert_eq!(settings.sign_lights.tl.kind, SignCorner::State);
        assert_eq!(settings.sign_lights.bl.kind, SignCorner::Muted);
        assert_eq!(settings.sign_lights.br.trigger, LightTrigger::Quiet);
        assert_eq!(settings.sign_lights.corner_count(), 4);
        // 旧字段在内存里也清掉了，免得被直接序列化带出去
        assert_eq!(settings.sign_edge, SignEdge::Off);
        assert_eq!(settings.sign_corners.count(), 0);

        // Top 只走上边 —— 这一档最容易漏
        let top_only =
            migrate_legacy_light_slots(serde_json::from_str(r#"{"sign_edge": "top"}"#).unwrap());
        assert_eq!(top_only.sign_lights.top.kind, EdgeLight::Line);
        assert_eq!(top_only.sign_lights.bottom.kind, EdgeLight::Off);
    }

    /// 迁移只能发生一次：新配置里旧字段全是默认值，不能再把它们盖到用户
    /// 后来调好的新灯位上。
    #[test]
    fn migration_never_overwrites_a_written_new_slot_set() {
        let raw = r#"{
            "sign_edge": "glow",
            "sign_lights": {"top": {"kind": "fade", "trigger": "message"}}
        }"#;
        let settings = migrate_legacy_light_slots(serde_json::from_str(raw).unwrap());
        assert_eq!(settings.sign_lights.top.kind, EdgeLight::Fade);
        assert_eq!(settings.sign_lights.top.trigger, LightTrigger::Message);
        assert_eq!(settings.sign_lights.bottom.kind, EdgeLight::Off);
    }

    /// 旧的「灵动效果：关」要变成 `effect=off`；没写这个键的新配置不能被动到。
    #[test]
    fn legacy_lively_off_migrates_to_effect_off() {
        let off = migrate_legacy_effect(
            serde_json::from_str(r#"{"lively": false, "effect": "pulse"}"#).unwrap(),
        );
        assert_eq!(off.effect, Effect::Off, "显式关过就该是静止");

        let fresh = migrate_legacy_effect(serde_json::from_str(r#"{"effect": "fade"}"#).unwrap());
        assert_eq!(
            fresh.effect,
            Effect::Fade,
            "新配置里 lively 默认 true，不该覆盖效果档"
        );
    }

    /// 手改过的配置里那三个自由值要规整，不能让灯「配了却永远不亮」。
    #[test]
    fn free_form_light_trigger_values_are_clamped() {
        // 用多字节字符拼一个超长模板：`String::truncate` 按字节切的话，这里会
        // 直接 panic（切在 UTF-8 中间），而不只是「长度不对」。
        let long = "构建".repeat(60);
        let raw = format!(
            r#"{{
                "sign_trigger_state": "NOPE",
                "sign_trigger_from": 2000,
                "sign_trigger_to": 50000,
                "sign_label_template": "{long}"
            }}"#
        );
        let settings = serde_json::from_str::<Settings>(&raw).unwrap().clamped();
        assert_eq!(settings.sign_trigger_state, DEFAULT_LIGHT_TRIGGER_STATE);
        assert_eq!(settings.sign_trigger_from, 2000 % 1440);
        assert_eq!(settings.sign_trigger_to, 50000 % 1440);
        assert_eq!(
            settings.sign_label_template.chars().count(),
            MAX_SIGN_LABEL_TEMPLATE
        );
    }

    /// 隐藏的组名来自右键菜单、也会被用户手改进配置文件，所以要规整：
    /// 否则同一个组写成 " ci" 和 "ci" 会被当成两个，"已隐藏" 就时灵时不灵。
    #[test]
    fn hidden_groups_are_trimmed_deduped_and_sorted() {
        let raw = r#"{"hidden_groups": [" ci ", "ci", "", "  ", "work", "zeta", "alpha"]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(
            settings.hidden_groups,
            vec!["alpha", "ci", "work", "zeta"],
            "去空白、去空串、去重、排序"
        );
        assert!(!settings.shows_group("ci"), "藏起来的组不该显示");
        assert!(settings.shows_group("other"), "没被藏起来的照常显示");
    }

    /// 旧版本的 config.json 里没有这些新字段，不能因此让整份设置失效
    #[test]
    fn missing_appearance_fields_fall_back_to_expanded() {
        let settings: Settings = serde_json::from_str(r#"{"size": 30}"#).unwrap();
        assert_eq!(settings.size, 30, "用户调过的值必须保留");
        assert!(!settings.collapsed, "缺字段应退回「展开」，那是默认形态");
        assert_eq!(settings.sign_opacity, DEFAULT_SIGN_OPACITY);
        assert!(settings.hidden_groups.is_empty());
    }

    /// Call Me 的默认值必须是「关」：旧配置里没有这个字段时，升级后一条请求
    /// 都不该发出去（红线：新增外发默认关）。
    #[test]
    fn callme_defaults_to_off() {
        let settings: Settings = serde_json::from_str(r#"{"size": 30}"#).unwrap();
        assert!(!settings.callme.enabled(), "没有地址和 token 时必须是关闭的");
        assert_eq!(settings.callme.url, "");
        assert_eq!(settings.callme.token, "");
        assert_eq!(settings.callme.poll_ms, 15_000);
    }

    /// 只填一半（只有地址 / 只有 token）同样算关：那种状态下的请求必然失败，
    /// 每 15 秒打一次只是白耗电。
    #[test]
    fn callme_needs_both_url_and_token() {
        let settings: Settings =
            serde_json::from_str(r#"{"callme": {"url": "https://callme.example.com"}}"#).unwrap();
        assert!(!settings.callme.enabled());
        let settings: Settings =
            serde_json::from_str(r#"{"callme": {"token": "abcdef0123456789"}}"#).unwrap();
        assert!(!settings.callme.enabled());
        let settings: Settings = serde_json::from_str(
            r#"{"callme": {"url": "https://callme.example.com", "token": "abcdef0123456789"}}"#,
        )
        .unwrap();
        assert!(settings.callme.enabled());
    }

    /// 粘贴地址经常带首尾空格；轮询间隔可能被手改成 0 或者一大串数字。
    #[test]
    fn callme_is_trimmed_and_clamped() {
        let raw = r#"{"callme": {"url": "  https://callme.example.com  ", "token": " abc ", "poll_ms": 1}}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.callme.url, "https://callme.example.com");
        assert_eq!(settings.callme.token, "abc");
        assert_eq!(settings.callme.poll_ms, MIN_CALLME_POLL_MS);

        let raw = r#"{"callme": {"poll_ms": 999999999}}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.callme.poll_ms, MAX_CALLME_POLL_MS);
    }

    /// Rust 侧的「状态 → 颜色」只有这一个入口：只读 Web 面板和物理灯都问它。
    ///
    /// 所以它必须和灯上一致（`palettes.ts` 的 `resolveColors`）—— 这条用例钉的是
    /// 三件容易漂的事：派生状态也有颜色、认不出来的名字有兜底、逐个覆盖压过预设。
    #[test]
    fn a_state_colour_is_the_override_then_the_palette() {
        let mut settings = Settings::default();
        assert_eq!(settings.state_color("working"), "#16a34a");
        assert_eq!(settings.state_color("permission"), "#d0342c");
        // 聚合出来的派生状态也要有颜色（面板要画它们，灯要显示它们）
        assert_eq!(settings.state_color("offline"), "#9ca3af");
        // 认不出来的名字落到「没动静」那档，**不是空串** —— 空串会让每个调用方
        // 各自兜底，那正是「同一个状态三处三个颜色」的来源
        assert_eq!(settings.state_color("nonsense"), "#9ca3af");

        settings.styles.insert(
            "error".to_string(),
            StateStyle {
                color: Some("#123456".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(settings.state_color("error"), "#123456");

        settings.palette = Palette::Colorblind;
        assert_eq!(settings.state_color("working"), "#0072b2");
        assert_eq!(settings.state_color("error"), "#123456", "逐个覆盖仍然压过配色");
    }

    /// 地址去空白、空地址留着 —— 手改配置里留一个 `{"host": ""}` 不该让界面上的
    /// 那一行消失（见 `clamped` 里的说明），而真正要发的时候会按空地址过滤。
    #[test]
    fn devices_are_trimmed_but_an_empty_host_stays() {
        let settings = Settings {
            devices: vec![
                Device { host: " 192.168.1.50 ".into() },
                Device { host: "   ".into() },
            ],
            ..Settings::default()
        }
        .clamped();
        assert_eq!(settings.devices.len(), 2);
        assert_eq!(settings.devices[0].host, "192.168.1.50");
        assert_eq!(settings.devices[1].host, "");
    }

    #[test]
    fn clamped_pulls_out_of_range_sizes_back() {        let wild = Settings { size: 9999, ..Settings::default() };
        assert_eq!(wild.clamped().size, MAX_SIZE);
        let tiny = Settings { size: 1, ..Settings::default() };
        assert_eq!(tiny.clamped().size, MIN_SIZE);
    }

    // ---- 多机中枢 -------------------------------------------------------------

    /// **默认一台都不配。** 这条比看着重要：空数组 = 一条请求都不发，也是
    /// 「升级后灯的牌数和 0.4.1 完全一致」这条硬约束的前提（见方案 §0.2 红线 2）。
    #[test]
    fn no_machines_are_configured_by_default() {
        let settings = Settings::default();
        assert!(settings.machines.is_empty());
        assert_eq!(settings.machine_poll_ms, 2000);
    }

    /// 机器名是**命名空间**，所以：空的丢掉、重复的留第一个、地址空着保留。
    ///
    /// 地址那一条和 `devices` 同一个脾气 —— 「刚点了添加、还没填」是编辑时必然
    /// 经过的中间态，存下去被丢掉的话界面上就是「刚点出来的那一行自己没了」。
    #[test]
    fn machine_ids_are_trimmed_deduped_and_never_empty() {
        let machine = |id: &str, url: &str| Machine {
            id: id.into(),
            url: url.into(),
            token: " t0ken ".into(),
            enabled: true,
        };
        let settings = Settings {
            machines: vec![
                machine("  mac-mini  ", " 192.168.1.20:41320 "),
                // 只打了空格的名称 = 没有名称
                machine("   ", "192.168.1.21:41320"),
                // 重复：命名空间会撞，留先出现的那个
                machine("mac-mini", "192.168.1.99:41320"),
                // 地址空着保留
                machine("server", ""),
            ],
            ..Settings::default()
        }
        .clamped();

        let ids: Vec<&str> = settings.machines.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["mac-mini", "server"]);
        assert_eq!(settings.machines[0].url, "192.168.1.20:41320");
        assert_eq!(settings.machines[0].token, "t0ken", "token 也要去空白");
        assert_eq!(settings.machines[1].url, "");
    }

    /// 轮询间隔夹在 1–10 秒。下界是「再快只是给对端加负载」，上界是「再慢掉线就
    /// 要半分钟以上才在灯上显形」。
    #[test]
    fn the_poll_interval_is_clamped() {
        let fast = Settings { machine_poll_ms: 10, ..Settings::default() }.clamped();
        assert_eq!(fast.machine_poll_ms, MIN_MACHINE_POLL_MS);
        let slow = Settings { machine_poll_ms: 600_000, ..Settings::default() }.clamped();
        assert_eq!(slow.machine_poll_ms, MAX_MACHINE_POLL_MS);
    }

    /// 地址里有空格 = 把「192.168.1.50 那台」整句粘进来了。只挡这一种明显的粘贴
    /// 事故，不校验形状（mDNS 名字、https 反代都是正经用法）。
    #[test]
    fn a_pasted_sentence_is_flagged_but_a_hostname_is_not() {
        let mut machine = Machine {
            id: "mini".into(),
            url: "192.168.1.50 那台".into(),
            token: String::new(),
            enabled: true,
        };
        assert!(machine_problem(&machine).is_some());
        machine.url = "mini.local:41320".into();
        assert!(machine_problem(&machine).is_none());
        machine.url = String::new();
        assert!(machine_problem(&machine).is_none(), "空地址是编辑中的正常态");
    }

    // ---- 兜底分组 ----

    /// 默认必须是**不分组**。这条比看着重要：一旦默认按目录分组，所有现有
    /// 用户的灯会从一块牌变成若干块、窗口尺寸跟着变，属于无声改变既有行为。
    #[test]
    fn grouping_is_off_by_default() {
        let settings = Settings::default();
        assert_eq!(settings.group_by, GroupBy::None);
        assert_eq!(settings.resolve_group(None, Some("/proj/api"), "s-1"), None);
    }

    /// 上报自己带的 `group` **优先**，`group_by` 只是兜底而不是覆盖。
    /// 顺序反了就等于「开了按目录分组之后手写 group 全失效」。
    #[test]
    fn an_explicit_group_beats_the_cwd_fallback() {
        let settings = Settings { group_by: GroupBy::Cwd, ..Settings::default() };
        assert_eq!(
            settings.resolve_group(Some("work"), Some("/proj/api"), "s-1").as_deref(),
            Some("work")
        );
    }

    #[test]
    fn cwd_mode_falls_back_to_the_full_path() {
        let settings = Settings { group_by: GroupBy::Cwd, ..Settings::default() };

        // **完整路径**，不是最后一段目录名：`~/a/api` 和 `~/b/api` 撞名会让
        // 两个项目的会话混进同一块牌。
        assert_eq!(
            settings.resolve_group(None, Some("/Users/me/code/api"), "s-1").as_deref(),
            Some("/Users/me/code/api")
        );
        // 末尾分隔符要去掉，否则 `/proj` 和 `/proj/` 会变成两个组
        assert_eq!(
            settings.resolve_group(None, Some("/Users/me/code/api/"), "s-1").as_deref(),
            Some("/Users/me/code/api")
        );
        assert_eq!(settings.resolve_group(None, Some("  /proj  "), "s-1").as_deref(), Some("/proj"));

        // 没给 cwd 就退回 default，而不是造一个空名字的组
        assert_eq!(settings.resolve_group(None, None, "s-1"), None);
        // 空串和空白串都不算「给了一个组」
        assert_eq!(settings.resolve_group(Some("   "), Some("/proj"), "s-1").as_deref(), Some("/proj"));
        // 根目录去掉分隔符只剩空串 —— 那不是个能用的组名
        assert_eq!(settings.resolve_group(None, Some("/"), "s-1"), None);
    }

    /// 取路径最后一段。两种分隔符都认，免得 Windows 上写的路径在别处解析错。
    ///
    /// 通知正文和会话分组的默认显示名（「目录末段 · 智能体」）共用它，
    /// 所以这条同时挡住「两处各写一遍、然后分叉」。
    #[test]
    fn project_name_is_the_last_path_segment() {
        assert_eq!(project_name("/Users/me/proj"), "proj");
        assert_eq!(project_name("/Users/me/proj/"), "proj");
        assert_eq!(project_name(r"C:\work\api"), "api");
        assert_eq!(project_name("proj"), "proj");
    }

    /// 按会话分组：**一个会话一块牌**。
    ///
    /// 这是「同一个智能体并行开了几个会话、进度各不相同」那个场景的解：只按目录
    /// 分组还是会把它们挤在同一块牌上，只能看到最紧急的那一个。
    #[test]
    fn session_mode_gives_every_session_its_own_sign() {
        let settings = Settings {
            group_by: GroupBy::Session,
            ..Settings::default()
        };

        assert_eq!(
            settings.resolve_group(None, Some("/proj"), "abc-123").as_deref(),
            Some("abc-123")
        );

        // **同一个目录里的两个会话必须分开** —— 这正是这个模式存在的理由，
        // 所以用 `assert_ne!` 直接钉这一点，而不是只断言各自等于自己的 id
        assert_ne!(
            settings.resolve_group(None, Some("/proj"), "abc-123"),
            settings.resolve_group(None, Some("/proj"), "def-456"),
            "同目录的两个会话被分到同一块牌了"
        );

        // 显式 `group` 仍然优先 —— 「上报自己带的优先」这条对所有模式一视同仁
        assert_eq!(
            settings.resolve_group(Some("work"), None, "abc-123").as_deref(),
            Some("work")
        );

        // 没有 cwd 也照样能分（会话 id 总是有的，不像 cwd）
        assert_eq!(
            settings.resolve_group(None, None, "abc-123").as_deref(),
            Some("abc-123")
        );
    }

    // ---- 界面语言（CLI 也用它） ------------------------------------------------

    /// POSIX 的 locale 写法要能翻译成 `Lang::parse` 认得的那种。
    ///
    /// 这一条是**实测踩到的**：`LANG=en_US.UTF-8` 一开始换不出英文，因为
    /// `Lang::parse` 认的是 `navigator.language` 那一套（`en-US`），而环境变量给的是
    /// 带编码、带下划线的 POSIX 写法。少了这层归一，CLI 在几乎所有 Unix 上都会
    /// 退回中文 —— 而那正是「看起来做了、其实没生效」。
    #[test]
    fn posix_locale_spellings_are_normalized() {
        for (raw, want) in [
            ("en_US.UTF-8", "en-US"),
            ("zh_CN.UTF-8", "zh-CN"),
            ("en_US", "en-US"),
            ("en", "en"),
            ("C", "C"),
            ("", ""),
            // `@` 是修饰符，和编码一样该丢掉
            ("sr_RS@latin", "sr-RS"),
        ] {
            assert_eq!(normalize_posix_locale(raw), want, "{raw:?}");
        }

        // 归一之后要真的能被认出来
        assert_eq!(Lang::parse(&normalize_posix_locale("en_US.UTF-8")), Some(Lang::En));
        assert_eq!(Lang::parse(&normalize_posix_locale("zh_CN.UTF-8")), Some(Lang::Zh));
        // `C` / `POSIX` 表示「没有指定 locale」，认不出来是对的 —— 调用方会继续往下试
        assert_eq!(Lang::parse(&normalize_posix_locale("C")), None);
    }

    /// **显式偏好最权威**，排在 `lang` 文件和环境变量前面。
    ///
    /// 这条不走文件系统：`Locale::Zh` / `Locale::En` 两个分支直接返回，只有 `Auto`
    /// 才会去读文件。所以这里能确定性地断言 —— 而「用户明确选过」优先于「上次 app
    /// 解析出来的」，正是为了让手改 `config.json` 立刻生效。
    #[test]
    fn an_explicit_locale_beats_everything_else() {
        let zh = Settings { locale: Locale::Zh, ..Settings::default() };
        let en = Settings { locale: Locale::En, ..Settings::default() };
        assert_eq!(resolve_lang(&zh), Lang::Zh);
        assert_eq!(resolve_lang(&en), Lang::En);
    }

    /// `theme` 和 `group_by` 一样是设置里的一个值：线上名字写错会让整份配置
    /// 反序列化失败（不是「开关点了没反应」这么轻），所以三种值都锁一遍。
    #[test]
    fn theme_round_trips_through_json() {
        for (mode, wire) in [
            (Theme::Auto, "auto"),
            (Theme::Light, "light"),
            (Theme::Dark, "dark"),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{wire}\""), "{mode:?} 的线上名字变了");
            assert_eq!(serde_json::from_str::<Theme>(&json).unwrap(), mode);
        }
    }

    /// 旧配置里没有 `theme` —— 必须读成 Auto，升级不能悄悄把人切进深色。
    #[test]
    fn settings_without_theme_read_as_auto() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""theme":"auto""#), "{json}");

        let legacy = json.replace(r#""theme":"auto","#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.theme, Theme::Auto);
    }

    /// `group_by` 是设置里的一个值，所以它必须能**原样存取** —— 值名字写错了
    /// （比如 serde 那边写成 `by_session`）会让开关打开后静默退回不分组。
    #[test]
    fn group_by_round_trips_through_json() {        for (mode, wire) in [
            (GroupBy::None, "none"),
            (GroupBy::Cwd, "cwd"),
            (GroupBy::Session, "session"),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{wire}\""), "{mode:?} 的线上名字变了");
            assert_eq!(serde_json::from_str::<GroupBy>(&json).unwrap(), mode);
        }
    }

    /// `group_by` 是设置里的一个值，所以旧配置（没有这个键）必须能读成 `None`。
    #[test]
    fn settings_without_group_by_read_as_none() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""group_by":"none""#), "{json}");

        let legacy = json.replace(r#""group_by":"none","#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.group_by, GroupBy::None);
    }

    #[test]
    fn window_grows_downward_as_signs_stack() {
        let settings = Settings::default();
        let one_line = sign_height(settings.size, false, 1.0);
        let one = settings.window_size(&[one_line]);

        // 宽度不随牌数变 —— 牌是竖着叠的，不是横着排
        let two = settings.window_size(&[one_line, one_line]);
        assert_eq!(two.0, one.0, "宽度固定");
        assert!(two.1 > one.1, "多一块牌就多占一份高度");

        // 有消息的那块多一行，于是窗口更高 —— 那是刻意的提示
        let with_message = settings.window_size(&[sign_height(settings.size, true, 1.0)]);
        assert!(with_message.1 > one.1, "有消息的牌多占一行");

        // 超出上限按上限算，不会无限往下长
        let many = vec![one_line; MAX_SIGNS + 3];
        let capped = vec![one_line; MAX_SIGNS];
        assert_eq!(settings.window_size(&many), settings.window_size(&capped));
    }

    /// 间距档位是外观里**唯一影响几何**的一项：默认档必须逐字等于旧公式
    /// `size / 2`，另外两档要真的改变间距、并且把窗口一起推高。
    ///
    /// CSS 那边是另一处真值（`lamp.css` 里 `.sign` 的 `--row-gap` 用
    /// `calc(size / 2 * var(--sign-gap))`，由共享板非末行的留白消化；
    /// `--sign-gap` 由 `lampStyle.ts` 从同一个
    /// 枚举推过来）。这里钉的是 Rust 这一侧的口径：先取整再乘、乘完向上取整。
    #[test]
    fn the_sign_gap_is_identity_at_standard_and_moves_the_window() {
        for size in [16u16, 22, 32] {
            assert_eq!(
                SignGap::Standard.px(size),
                size / 2,
                "默认档必须逐字等于旧公式：{size}"
            );
        }
        assert!(SignGap::Tight.px(22) < SignGap::Standard.px(22), "紧凑档更窄");
        assert!(SignGap::Loose.px(22) > SignGap::Standard.px(22), "宽松档更宽");

        let one_line = sign_height(22, false, 1.0);
        let base = Settings::default().window_size(&[one_line, one_line]);
        let loose = Settings { sign_gap: SignGap::Loose, ..Settings::default() }
            .window_size(&[one_line, one_line]);
        let tight = Settings { sign_gap: SignGap::Tight, ..Settings::default() }
            .window_size(&[one_line, one_line]);
        assert!(loose.1 > base.1, "宽松档的窗口更高");
        assert!(tight.1 < base.1, "紧凑档的窗口更矮");
        // 牌是竖着叠的：间距只影响高度，宽度一分都不该变
        assert_eq!(loose.0, base.0);
        assert_eq!(tight.0, base.0);
    }

    #[test]
    fn a_sign_is_wider_than_tall_and_scales_with_size() {
        for size in [16, 22, 32] {
            let width = sign_width(size, 1.0);
            let height = sign_height(size, false, 1.0);
            assert!(width > height, "牌是横条：{size} → {width}×{height}");
            assert_eq!(sign_height(size, true, 1.0), height * 5 / 3, "有消息时约 1.67 倍高");
        }
    }

    /// 倍率在 1.0 时必须**逐字**等于旧公式 —— 默认值不许无声改变既有行为（红线 3）。
    /// 它一旦不是 1.0，就该真的把牌放大 / 缩小。
    #[test]
    fn sign_multipliers_scale_the_sign_and_are_identity_at_one() {
        for size in [12u16, 22, 33, 64] {
            assert_eq!(sign_width(size, 1.0), size * 7, "宽 {size}");
            assert_eq!(sign_height(size, false, 1.0), size * 3 / 2, "单行高 {size}");
            assert_eq!(sign_height(size, true, 1.0), size * 5 / 2, "双行高 {size}");
        }

        let size = 22;
        // 放大：确实更大，且随倍率单调
        assert!(sign_width(size, 1.5) > sign_width(size, 1.0));
        assert!(sign_width(size, 2.0) > sign_width(size, 1.5));
        assert!(sign_height(size, false, 1.2) > sign_height(size, false, 1.0));
        assert!(sign_height(size, true, 1.2) > sign_height(size, true, 1.0));
        // 缩小也认
        assert!(sign_width(size, 0.8) < sign_width(size, 1.0));

        // **向上取整**：22*7*1.3 = 200.2 → 201（不是 200）。差这一下，CSS 那边
        // 200.2px 的牌就会被 `overflow: hidden` 裁掉右边缘一线。
        assert_eq!(sign_width(22, 1.3), 201);
    }

    /// 手改配置 / 前端都可能塞进越界的倍率，`clamped()` 必须夹住它们。
    #[test]
    fn clamped_pulls_the_sign_multipliers_into_range() {
        let wild = Settings {
            sign_w_mult: 99.0,
            sign_h_mult: 0.01,
            ..Settings::default()
        };
        let clamped = wild.clamped();
        assert_eq!(clamped.sign_w_mult, MAX_SIGN_MULT);
        assert_eq!(clamped.sign_h_mult, MIN_SIGN_MULT);

        // NaN / 无穷必须被挡掉：NaN 一路传到 `ceil` 会变成 0，把牌宽归零。
        let broken = Settings {
            sign_w_mult: f32::NAN,
            sign_h_mult: f32::INFINITY,
            ..Settings::default()
        };
        let clamped = broken.clamped();
        assert_eq!(clamped.sign_w_mult, 1.0, "NaN 要退回 1.0，不能令牌宽归零");
        assert_eq!(clamped.sign_h_mult, 1.0, "无穷一律退回 1.0");
        assert!(sign_width(22, clamped.sign_w_mult) > 0);
    }

    /// 旧配置（没有这两个键）必须读成倍率 1.0 —— 也就是完全不变的行为。
    /// 这是 `serde(default)` 那条契约的落点：升级不该让任何人的灯变形。
    #[test]
    fn settings_without_the_sign_multipliers_read_as_one() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""sign_w_mult":1.0,"#), "{json}");

        // 把两个键整个删掉，模拟旧版本写出来的文件
        let legacy = json
            .replace(r#""sign_w_mult":1.0,"#, "")
            .replace(r#""sign_h_mult":1.0,"#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.sign_w_mult, 1.0);
        assert_eq!(parsed.sign_h_mult, 1.0);
    }

    /// 旧配置（没有 `lamp_layout`）必须读成单灯 —— 新能力默认关（红线 3）。
    #[test]
    fn settings_without_lamp_layout_read_as_single() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""lamp_layout":"single""#), "{json}");

        let legacy = json.replace(r#""lamp_layout":"single","#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.lamp_layout, LampLayout::Single);
    }

    /// 旧配置（没有 `snap_to_edge`）必须读成「不吸附」—— 新能力默认关（红线 3）。
    #[test]
    fn settings_without_snap_to_edge_read_as_off() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""snap_to_edge":false"#), "{json}");

        let legacy = json.replace(r#""snap_to_edge":false,"#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert!(!parsed.snap_to_edge);
    }

    /// 0.6.6 的样子：横排裸点。老单测全部走这个包装，语义一个字没改。
    fn horizontal_dots(size: u16, dots: usize, badges: usize) -> (u16, u16) {
        collapsed_window_size(size, dots, badges, FoldAxis::Horizontal, FoldLabel::None, 1.0, 1.0)
    }

    /// 折叠态：一个组一个点，窗口比展开态小得多
    #[test]
    fn collapsed_window_holds_one_dot_per_group() {
        let size = Settings::default().size;

        let (w1, h1) = horizontal_dots(size, 1, 0);
        assert_eq!(w1, size + LAMP_MARGIN, "一个点：方窗");
        assert_eq!(h1, size + LAMP_MARGIN);

        let (w3, h3) = horizontal_dots(size, 3, 0);
        assert!(w3 > w1, "多点横着排，所以变宽");
        assert_eq!(h3, h1, "高度只和点的大小有关");

        // 一个组都没有时也要有一个点（表示 offline）
        assert_eq!(horizontal_dots(size, 0, 0), horizontal_dots(size, 1, 0));
        // 超出上限按上限算
        assert_eq!(
            horizontal_dots(size, MAX_DOTS + 5, 0),
            horizontal_dots(size, MAX_DOTS, 0)
        );
    }

    /// 折叠态比点还多的组要靠一枚 `+N` 角标表达。调用点传进来的必须是**未截断**
    /// 的可见组数 —— 传 `visible_groups` 那种已经截到 `MAX_SIGNS` 的列表长度，
    /// `more_badge` 恒得 0，那枚角标就被窗口裁掉了（issue #7）。
    #[test]
    fn groups_beyond_the_dots_get_a_badge_slot() {
        let size = Settings::default().size;
        assert_eq!(more_badge(MAX_DOTS), 0, "正好装得下时没有角标");
        assert_eq!(more_badge(MAX_DOTS + 1), 1);

        let visible = MAX_DOTS + 1;
        assert!(
            horizontal_dots(size, visible, more_badge(visible))
                > horizontal_dots(size, visible, 0),
            "多出来的那组要占一个点位，否则角标落在窗口外面"
        );
    }

    /// 折叠态只有 `+N` 还占点位。暂停 / 穿透那枚是灯体内部最上沿的绝对定位徽标，
    /// **不占点位** —— 把它算进来的话，开关一拨窗口就加宽一个点距，整排灯珠
    /// 被重新居中，看着像被那枚箭头挤了出去。
    #[test]
    fn only_the_more_badge_takes_a_dot_of_its_own() {
        let size = Settings::default().size;

        let (bare, _) = horizontal_dots(size, 2, 0);
        let (flagged, _) = horizontal_dots(size, 2, 1);
        assert_eq!(flagged - bare, size + size / 3, "多一个点加一个点距");

        // 上限仍然要封住，否则一个算错的调用点会让窗口无限变宽。
        // 现在只剩一枚角标，多传也只按 1 算。
        assert_eq!(
            horizontal_dots(size, MAX_DOTS, 9),
            horizontal_dots(size, MAX_DOTS, MAX_BADGES),
            "角标再多也只按 MAX_BADGES 算"
        );

        // 角标上限是**单独**封的，不侵占组的份额：组数按 MAX_DOTS 截断，
        // 角标再占位，两者相加才是最终的点数。
        let (full, _) = horizontal_dots(size, MAX_DOTS, 0);
        let (full_badged, _) = horizontal_dots(size, MAX_DOTS, MAX_BADGES);
        assert_eq!(
            full_badged - full,
            MAX_BADGES as u16 * (size + size / 3),
            "满屏也不该挤掉角标"
        );
    }

    /// `+N` 角标出现的时机。**前端按同一条件画**（`SignalLamp.tsx` 里的
    /// `hidden.length > 0`），所以这条同时也是两边契约的锚。
    #[test]
    fn the_more_badge_appears_only_past_the_dot_limit() {
        assert_eq!(more_badge(0), 0, "一个组都没有时没有可藏的");
        assert_eq!(more_badge(MAX_DOTS), 0, "刚好放得下，一枚都不藏");
        assert_eq!(more_badge(MAX_DOTS + 1), 1, "藏了一个就得说");
        assert_eq!(more_badge(MAX_DOTS + 10), 1, "藏得再多也只有一枚角标");
    }

    /// 折叠态必须比展开态小 —— 「折叠」的意义就是让出地方
    #[test]
    fn collapsed_is_much_smaller_than_expanded() {
        let settings = Settings::default();
        let (cw, ch) = horizontal_dots(settings.size, 1, 0);
        let (ew, eh) = settings.window_size(&[sign_height(settings.size, false, 1.0)]);
        assert!(cw < ew, "折叠态更窄: {cw} vs {ew}");
        assert!(ch < eh, "折叠态更矮: {ch} vs {eh}");
    }

    /// 折叠默认竖排、不带文字：用户明确要的默认改动。老配置缺这两个字段时
    /// 也必须落到同一组值（容器级 `serde(default)`），不能退回横排。
    #[test]
    fn fold_defaults_to_a_vertical_column_of_bare_dots() {
        let settings = Settings::default();
        assert_eq!(settings.fold_axis, FoldAxis::Vertical);
        assert_eq!(settings.fold_label, FoldLabel::None);

        let old = r#"{"size": 30, "collapsed": true}"#;
        let legacy: Settings = serde_json::from_str(old).unwrap();
        assert!(legacy.collapsed, "用户折过就还是折着");
        assert_eq!(legacy.fold_axis, FoldAxis::Vertical);
        assert_eq!(legacy.fold_label, FoldLabel::None);

        let size = settings.size;
        let (w, h) = settings.folded_size(3, 0);
        assert_eq!(w, size + LAMP_MARGIN, "竖排只占一条窄边");
        assert_eq!(h, 3 * size + 2 * (size / 3) + LAMP_MARGIN, "三个点竖着叠");
    }

    /// 换轴只换「哪一维累加格数」，不改格子本身。裸点的格子是正方形，所以
    /// 竖排正好是横排的转置；带文字的格子是长方形，两维各自按格宽 / 格高累加。
    #[test]
    fn the_fold_axis_only_decides_which_dimension_stacks_cells() {
        let size = Settings::default().size;
        for label in [FoldLabel::None, FoldLabel::Index, FoldLabel::State, FoldLabel::Name] {
            let (cell_w, cell_h) = fold_cell_size(size, label, 1.0, 1.0);
            let gap = if label.has_text() { 0 } else { size / 3 };
            for dots in [1, 3, MAX_DOTS] {
                let n = dots as u16;
                let (hw, hh) = collapsed_window_size(
                    size, dots, 0, FoldAxis::Horizontal, label, 1.0, 1.0,
                );
                let (vw, vh) = collapsed_window_size(
                    size, dots, 0, FoldAxis::Vertical, label, 1.0, 1.0,
                );
                assert_eq!(
                    (hw, hh),
                    (n * cell_w + (n - 1) * gap + LAMP_MARGIN, cell_h + LAMP_MARGIN),
                    "{label:?} × {dots} 格：横排"
                );
                assert_eq!(
                    (vw, vh),
                    (cell_w + LAMP_MARGIN, n * cell_h + (n - 1) * gap + LAMP_MARGIN),
                    "{label:?} × {dots} 格：竖排"
                );
                if label == FoldLabel::None {
                    assert_eq!((vw, vh), (hh, hw), "正方形格子：竖排就是转置");
                }
            }
        }
    }

    /// 带文字时宽度只由档位决定，与组数无关；板内不留点距，高度是 N 个单行牌。
    #[test]
    fn labeled_folds_have_a_fixed_width_and_no_dot_gap() {
        let size = Settings::default().size;
        let row_h = sign_height(size, false, 1.0);
        for (label, mult) in [
            (FoldLabel::Index, 2.4),
            (FoldLabel::State, 3.5),
            (FoldLabel::Name, 4.0),
        ] {
            let one = collapsed_window_size(
                size, 1, 0, FoldAxis::Vertical, label, 1.0, 1.0,
            );
            let three = collapsed_window_size(
                size, 3, 0, FoldAxis::Vertical, label, 1.0, 1.0,
            );
            assert_eq!(one.0, (size as f32 * mult).ceil() as u16 + LAMP_MARGIN);
            assert_eq!(three.0, one.0, "{label:?}: 组名再长也不该让窗口变宽");
            assert_eq!(three.1 - one.1, 2 * row_h, "{label:?}: 板内不该再有点距");
        }
        assert!(
            FoldLabel::Index.width_mult() < FoldLabel::State.width_mult()
                && FoldLabel::State.width_mult() < FoldLabel::Name.width_mult(),
            "序号最窄、组名最宽"
        );
        // **折叠的全部意义是让出桌面**：组名档要是和展开牌差不多宽，折叠就白做了。
        // 0.6.7 之前组名档是 6.0×size（22px 档 132px、窗口 160px），只比展开牌的
        // 154px / 182px 窄 22px —— 用户的原话是「那么折叠没有意义啊」。
        let name_w =
            collapsed_window_size(size, 1, 0, FoldAxis::Vertical, FoldLabel::Name, 1.0, 1.0).0;
        let sign_w = sign_width(size, 1.0) + LAMP_MARGIN;
        assert!(
            name_w * 100 < sign_w * 70,
            "组名档 {name_w}px 相对展开牌 {sign_w}px 没窄下 30%，折叠会显得没有意义"
        );
    }

    /// 折叠内容跟着牌宽高倍率走：用户把整盏灯调大时，窄板不该仍用旧尺寸。
    #[test]
    fn labeled_folds_follow_the_sign_multipliers() {
        // 一次性写进初始化式而不是先 default 再逐个赋值：clippy 的
        // `field_reassign_with_default` 在 CI 里是 `-D warnings` 的一部分。
        let settings = Settings {
            fold_label: FoldLabel::Name,
            sign_w_mult: 1.5,
            sign_h_mult: 1.2,
            ..Settings::default()
        };
        let (w, h) = settings.folded_size(2, 0);
        let (cw, ch) = fold_cell_size(settings.size, FoldLabel::Name, 1.5, 1.2);
        assert_eq!(w, cw + LAMP_MARGIN);
        assert_eq!(h, 2 * ch + LAMP_MARGIN);
        // 这里引用 `width_mult()` 而不是再写一遍 4.0：具体数值由上一个测试钉住，
        // 这条只验「倍率有没有乘进格宽」。
        assert_eq!(
            cw,
            (settings.size as f32 * FoldLabel::Name.width_mult() * 1.5).ceil() as u16
        );
        assert_eq!(ch, sign_height(settings.size, false, 1.2));
    }

    /// 两个新字段要落盘、也要读得回来；手改的非法值不能让整份设置失效到
    /// 连用户调过的 size 都丢掉（那是 Settings 的 serde 契约，这里一起钉住）。
    #[test]
    fn fold_settings_round_trip_through_config_json() {
        // 同上：字段一次性写进初始化式，别让 clippy 的
        // `field_reassign_with_default` 把 CI 拦下来。
        let settings = Settings {
            fold_axis: FoldAxis::Horizontal,
            fold_label: FoldLabel::Index,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""fold_axis":"horizontal""#), "{json}");
        assert!(json.contains(r#""fold_label":"index""#), "{json}");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.fold_axis, FoldAxis::Horizontal);
        assert_eq!(back.fold_label, FoldLabel::Index);

        let legacy: Settings = serde_json::from_str(r#"{"size": 30}"#).unwrap();
        assert_eq!(legacy.size, 30);
        assert_eq!(legacy.fold_axis, FoldAxis::Vertical);
        assert_eq!(legacy.fold_label, FoldLabel::None);
    }

    // ---- 圆环菜单的窗口 -------------------------------------------------------

    /// 菜单窗口是**正方形**，且边长就是 `MENU_WINDOW`。
    ///
    /// 为什么必须是正方形：圆环按角度均分摆项，宽高不等就会摆成椭圆，
    /// 相邻项的距离一会儿大一会儿小 —— 看起来像没对齐。
    #[test]
    fn the_menu_window_is_a_square() {
        let (w, h) = menu_window_size();
        assert_eq!(w, h, "圆环菜单必须正方形: {w}x{h}");
        assert_eq!(w, MENU_WINDOW);
    }

    /// 圆环装得下：半径 + 一项的半个高度 + 一点留白，不能超过窗口半边长。
    ///
    /// 项宽 56、高约 40（见 lamp.css 的 `.lamp-menu-item`），所以外沿是
    /// `MENU_RADIUS + 20`。超了的话最外侧那几个项会被窗口裁掉一半 ——
    /// 而裁掉的按钮就是点不到的按钮。
    #[test]
    fn the_ring_fits_inside_the_window() {
        let half = MENU_WINDOW / 2;
        // 20 = 菜单项半高（40/2）；再留 4px 给描边和悬停时的视觉余量
        assert!(
            MENU_RADIUS + 20 + 4 <= half,
            "圆环外沿 {} 超出半窗 {half}",
            MENU_RADIUS + 20 + 4
        );
    }

    /// 菜单窗口**不跟 `size` 走**：菜单项是可点击目标，缩到 16px 的灯上
    /// 给一颗 8px 的按钮等于点不中。这条钉住「无论用户把灯调多大调多小，
    /// 菜单都是同一个尺寸」。
    #[test]
    fn the_menu_window_ignores_the_lamp_size() {
        assert_eq!(menu_window_size(), (MENU_WINDOW, MENU_WINDOW));
        // 它是个无参函数 —— 这个断言本身就是契约：将来有人想给它加个 size 参数，
        // 得先想清楚上面那条。
    }
}
