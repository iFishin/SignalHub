//! 多信号灯：每个可见组一盏自己的悬浮灯窗。
//!
//! 目标形态由 `Settings::lamp_layout` 决定：
//!
//! - `Single`（默认，也永远是**回落**形态）—— 现状：一盏灯画一叠牌（最多
//!   `config::MAX_SIGNS` 块）。
//! - `PerGroup` —— 每个可见组有一个自己的灯窗，可以各自拖到不同位置；主灯
//!   （label `main`）只剩一颗全局灯，各组交给各自的灯。
//!
//! 这一层只管两件事：**该有哪些灯窗**（多退少补）和**位置记忆**。事件推送仍然是
//! 广播（`notify::emit`），每个灯窗前端的自己按 `?group=` 过滤 —— 不另起推送通道，
//! 见 docs/plan-lamp-expansion.md 的 P2。

use crate::config;
use crate::Bounds;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

/// 灯窗 label 的前缀。`capabilities/default.json` 里用 `lamp-*` 放行（glob）。
pub const LABEL_PREFIX: &str = "lamp-";

/// 同时存在的灯窗数量上限。
///
/// 每个窗口都是一个真的 webview（Linux 上 WebKitGTK 的内存尤其可观），而「按会话
/// 分组」的重度用户一下就能开出几十个。超过这个数就**回落单灯模式**（各组仍画在
/// 主灯上，最多 `MAX_SIGNS` 块），设置页会说明这一点。
pub const MAX_LAMPS: usize = 8;

/// 组名 → 稳定的窗口 label。
///
/// Tauri 的窗口 label 只允许 `[a-zA-Z0-9-/:_]`，而组名可以是**中文、路径、UUID**，
/// 不能直接拿来当 label。也不能用自增序号：组消失又出现之后序号就变了，而位置记忆
/// 恰恰是按 label 存的 —— 序号一变，用户拖过的位置就丢了。
///
/// 做法：不安全的字符逐个换成 `_`，过长截断，末尾再拼一段组名的 FNV-1a 短 hash
/// 保证唯一。同组名永远得到同 label。
pub fn label_for(group: &str) -> String {
    let mut safe: String = group
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '/') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // 全 ASCII 了，所以按字节截断不会切坏一个字符
    safe.truncate(32);
    format!("{LABEL_PREFIX}{safe}-{:08x}", fnv1a(group) as u32)
}

/// FNV-1a。只要「同输入同输出、不同输入大概率不同」—— 不是密码学用途。
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 组名进 URL query 的百分号编码。
///
/// 组名要原样交给前端（`?group=`），而它能是中文 / 带 `&`、`?` 的任意串 —— 直接
/// 拼进去会把 query 切坏。这里按字节（UTF-8）编码，前端的 `URLSearchParams.get`
/// 会自动解回来。
pub fn encode_query(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 一盏「该存在」的灯：组名 + 这一帧该有的尺寸（逻辑像素，由上层按内容算好）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LampSpec {
    pub group: String,
    pub size: (u16, u16),
}

/// 建 / 改 / 关灯窗的副作用。
///
/// 抽成 trait 是为了让 `LampWindows` 的差集逻辑（增 / 删 / 保留 / 改尺寸）
/// 能在没有真实窗口的情况下单测 —— 建真窗口要一个 Tauri 应用，测试里造不出来。
pub trait LampHost {
    /// 按 `at`（记着的位置，物理像素）建这个组的灯窗，初始尺寸 `size`。
    /// 返回 `false` = 没建成（调用方会撤掉记账，下一帧再试）。
    fn open(&self, label: &str, group: &str, at: Option<(i32, i32)>, size: (u16, u16)) -> bool;
    /// 改这个窗的尺寸。
    fn resize(&self, label: &str, size: (u16, u16));
    /// 关掉这个窗，返回它最后的位置（好记下来）。
    fn close(&self, label: &str) -> Option<(i32, i32)>;
}

/// 一个「该建出来」的灯窗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpec {
    pub label: String,
    pub group: String,
    /// 记着的位置（物理像素）；`None` = 这一组第一次出现。
    pub at: Option<(i32, i32)>,
    pub size: (u16, u16),
}

/// 这一帧要对窗口做的事。**在锁里算出来，拿去锁外执行**。
///
/// 为什么要分两步：`open` 最终会调 `WebviewWindowBuilder::build()`，而从非主线程
/// 建窗是「投递给主事件循环 + 阻塞等回执」。如果算计划时还握着 `Lamps` 锁，主线程
/// 上的 `set_peek` / `set_settings` 也会来要同一把锁 —— 两边互等，整个应用冻住
/// （见 issue #4）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub close: Vec<String>,
    pub resize: Vec<(String, (u16, u16))>,
    pub open: Vec<OpenSpec>,
}

/// 真机上唯一用到的 `LampHost`：用 `WebviewWindowBuilder` 建真窗口。
pub struct TauriHost<'a>(pub &'a AppHandle);

impl TauriHost<'_> {
    /// 鼠标所在（或最近）的那块屏的工作区 + 缩放比。
    fn cursor_area(&self) -> Option<(Bounds, f64)> {
        let cursor = self.0.cursor_position().ok()?;
        crate::nearest_screen(self.0, (cursor.x.round() as i32, cursor.y.round() as i32))
    }

    /// 主屏的工作区 + 缩放比。拿不到就是 `None`。
    fn primary_area(&self) -> Option<(Bounds, f64)> {
        let monitor = self.0.primary_monitor().ok().flatten()?;
        Some((crate::work_bounds(&monitor), monitor.scale_factor()))
    }

    /// 把记忆中的位置夹回可见区。
    ///
    /// **为什么必须夹**：位置是按物理坐标存进 `lamp-positions.json` 的，而那个坐标
    /// 可能落在**一块已经不在了的屏幕上** —— 拔掉外接屏、改分辨率、换显示器布局都会
    /// 这样。灯于是出现在屏幕外，用户**再也点不到它**（它没有 Dock 图标、托盘也只能
    /// 打开设置，最后只能去删配置文件）。拔掉外接屏的人不会想到「灯被留在了那块屏上」。
    ///
    /// 判据与主灯的 `keep_on_screen` 完全一致（`crate::clamp_to_area` + 就近挑屏，
    /// 用工作区而不是整块屏）。所以**完全在屏内的位置一个像素都不动** —— 这里只处理
    /// 「屏没了」。
    fn clamp_to_visible(&self, at: (i32, i32), size: (u16, u16)) -> (i32, i32) {
        let Some((bounds, scale)) = crate::nearest_screen(self.0, at) else {
            // 拿不到显示器信息就原样用 —— 和 `default_position` 一个态度：不硬编坐标
            return at;
        };
        // `size` 是逻辑像素，存下来的位置是物理像素 —— 夹之前得换算（Retina 上差一倍）
        let physical = (
            (f64::from(size.0) * scale).round() as i32,
            (f64::from(size.1) * scale).round() as i32,
        );
        crate::clamp_to_area(at, physical, bounds)
    }

    /// 一盏灯**第一次**出现、又没记着位置时的默认摆放：贴屏幕**右缘**从上往下
    /// 堆叠，间距取半个点径。第几盏按「此刻已经开着几盏灯」算 —— 已记着位置的灯
    /// 回原位、不重排，新灯自然接在现有灯下面。
    ///
    /// 挑哪块屏：**鼠标所在的那块**（多屏用户的第一盏灯该出现在他正看着的地方，
    /// 而不是「永远主屏」）。拿不到鼠标位置就退回主屏。
    ///
    /// 拿不到显示器信息就返回 `None`（交回系统默认摆放），不硬编一个坐标：多屏 /
    /// 远程桌面上编出来的点可能落在根本不存在的屏幕上。
    fn default_position(&self, size: (u16, u16)) -> Option<(i32, i32)> {
        let (bounds, scale) = self.cursor_area().or_else(|| self.primary_area())?;
        let ((_left, top), (right, bottom)) = bounds;
        let width = (f64::from(size.0) * scale).round() as i32;
        let height = (f64::from(size.1) * scale).round() as i32;
        let gap = (f64::from(size.0) * scale / 2.0).round() as i32;
        let index = self
            .0
            .webview_windows()
            .keys()
            .filter(|label| label.starts_with(LABEL_PREFIX))
            .count() as i32;
        let x = right - width - gap;
        let y = top + gap + index * (height + gap);
        // 堆到屏幕底下就停在最下面（keep_on_screen 的兜底）
        let y = y.min(bottom - height - gap);
        Some((x, y))
    }
}

impl LampHost for TauriHost<'_> {
    fn open(&self, label: &str, group: &str, at: Option<(i32, i32)>, size: (u16, u16)) -> bool {
        let app = self.0;
        // 已经有一盏就不重建（布局在 single / per-group 之间来回切时会出现这种情况）
        if app.get_webview_window(label).is_some() {
            return true;
        }
        let url = format!("index.html?window=light&group={}", encode_query(group));
        // 记着位置就回原位 —— **但先夹回可见区**（那块屏可能已经不在了，见
        // `clamp_to_visible`）；第一次出现就贴右缘堆叠（鼠标所在的那块屏）。
        // 两种情况都拿不到屏幕信息时交回系统摆放。
        let position = at
            .map(|saved| self.clamp_to_visible(saved, size))
            .or_else(|| self.default_position(size));
        // 跟着全局的「总在最前」—— 组灯不该和主灯设置不一致。建窗是低频操作，
        // 这里读一次设置无所谓。
        let always_on_top = app
            .state::<crate::SharedSettings>()
            .lock()
            .expect("settings poisoned")
            .always_on_top;
        let built = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title("SignalHub")
            .inner_size(f64::from(size.0), f64::from(size.1))
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .always_on_top(always_on_top)
            .skip_taskbar(true)
            .shadow(false)
            .focused(false)
            // 和主灯一样：用户不该能把它关掉（关了之后 `reconcile` 不会自己发现，
            // 那盏灯就再也不回来了）。收起来只能通过「隐藏这一组」或换回单灯。
            .closable(false)
            .maximizable(false)
            .minimizable(false)
            .no_redirection_bitmap(true)
            .visible(true)
            .build();
        match built {
            Ok(window) => {
                let _ = window.set_visible_on_all_workspaces(true);
                if let Some((x, y)) = position {
                    let _ = window.set_position(PhysicalPosition::new(x, y));
                }
                // 组灯也要能贴边吸附（和主灯同一套，见 lib.rs 的 `wire_snap`）
                crate::wire_snap(app, &window);
                true
            }
            Err(err) => {
                eprintln!("[signalhub] 建灯窗 {label} 失败: {err}");
                false
            }
        }
    }

    fn resize(&self, label: &str, size: (u16, u16)) {
        if let Some(window) = self.0.get_webview_window(label) {
            let _ = window.set_size(tauri::LogicalSize::new(size.0, size.1));
        }
    }

    fn close(&self, label: &str) -> Option<(i32, i32)> {
        let window = self.0.get_webview_window(label)?;
        let at = window.outer_position().ok().map(|p| (p.x, p.y));
        let _ = window.destroy();
        at
    }
}

/// 灯窗集合：组名 → label，加各自的尺寸与位置记忆。
///
/// 位置按 **label** 存（组名会变长、变短，label 是稳定键）。文件是
/// `~/.signalhub/lamp-positions.json`。
#[derive(Debug, Default)]
pub struct LampWindows {
    /// 当前开着的灯：组名 → label。
    open: BTreeMap<String, String>,
    /// label → 上次设下去的尺寸，用来跳过没必要的 `set_size`（热路径）。
    sizes: BTreeMap<String, (u16, u16)>,
    /// label → (x, y)，物理像素。拖过之后记下来，下次建窗放回原处。
    positions: BTreeMap<String, (i32, i32)>,
}

impl LampWindows {
    /// 从盘上读回位置记忆。读不到（首次运行 / 文件坏了）就用空表。
    pub fn load() -> Self {
        let positions = read_positions().unwrap_or_default();
        Self { positions, ..Self::default() }
    }

    /// 算这一帧要做的窗口操作，**只动自己的记账，不碰任何窗口**。
    ///
    /// 记账是**乐观**的：新组在计划里就已经算作「开着」了。这样并发进来的下一帧
    /// （主线程和后台线程都会调 `sync_lamps`）看到的是同一份判断，不会重复建窗。
    /// 建失败由 [`Self::commit`] 撤掉。
    pub fn plan(&mut self, specs: &[LampSpec]) -> Plan {
        let desired: BTreeSet<&str> = specs.iter().map(|spec| spec.group.as_str()).collect();
        let mut plan = Plan::default();

        // 关：开着的、但这一帧不该有的
        let gone: Vec<String> = self
            .open
            .keys()
            .filter(|group| !desired.contains(group.as_str()))
            .cloned()
            .collect();
        for group in gone {
            let Some(label) = self.open.remove(&group) else {
                continue;
            };
            self.sizes.remove(&label);
            plan.close.push(label);
        }

        // 建 / 改尺寸
        for spec in specs {
            if let Some(label) = self.open.get(&spec.group).cloned() {
                if self.sizes.get(&label) == Some(&spec.size) {
                    continue;
                }
                plan.resize.push((label.clone(), spec.size));
                self.sizes.insert(label, spec.size);
                continue;
            }
            let label = label_for(&spec.group);
            plan.open.push(OpenSpec {
                label: label.clone(),
                group: spec.group.clone(),
                at: self.positions.get(&label).copied(),
                size: spec.size,
            });
            self.sizes.insert(label.clone(), spec.size);
            self.open.insert(spec.group.clone(), label);
        }

        plan
    }

    /// 把执行结果记回来。返回 `true` = 位置记忆变了，调用方该落盘。
    ///
    /// **在锁里调，但只做内存操作** —— 落盘在锁外（`write_positions`），否则一次
    /// 磁盘写就把这把锁占住了。
    pub fn commit(&mut self, closed: &[(String, Option<(i32, i32)>)], failed: &[String]) -> bool {
        let mut moved = false;
        for (label, at) in closed {
            if let Some(at) = at {
                self.positions.insert(label.clone(), *at);
                moved = true;
            }
        }
        // 建窗失败：把乐观记账撤掉，下一帧再试。不退的话这一组永远没有灯，而界面上
        // 看不出任何区别 —— 只有一条 stderr。
        for label in failed {
            self.open.retain(|_, open_label| open_label != label);
            self.sizes.remove(label);
        }
        moved
    }

    /// 位置记忆的快照（落盘用；在锁外写，见 [`Self::commit`]）。
    pub fn positions_snapshot(&self) -> BTreeMap<String, (i32, i32)> {
        self.positions.clone()
    }

    /// `plan` + 同步执行 + `commit` 的便利形态。
    ///
    /// **只给单测用**：真实路径（`lib.rs` 的 `sync_lamps`）必须把三步分开，让窗口
    /// 操作落在锁外。这里把三步串起来正是因为测试里没有锁、也没有真窗口。
    #[cfg(test)]
    pub fn reconcile(&mut self, host: &dyn LampHost, specs: &[LampSpec]) {
        let plan = self.plan(specs);
        let closed: Vec<(String, Option<(i32, i32)>)> = plan
            .close
            .iter()
            .map(|label| (label.clone(), host.close(label)))
            .collect();
        for (label, size) in &plan.resize {
            host.resize(label, *size);
        }
        let failed: Vec<String> = plan
            .open
            .iter()
            .filter(|open| !host.open(&open.label, &open.group, open.at, open.size))
            .map(|open| open.label.clone())
            .collect();
        self.commit(&closed, &failed);
    }
}

/// 退出前把每个还开着的灯窗的位置落盘。
///
/// 平时改尺寸 / 关窗时已经顺手存过，但用户拖完最后一个动作就退出（没等下一帧）时，
/// 那点位移只在窗口里 —— 不在这儿补一次就丢了。
pub fn save_open_positions(app: &AppHandle) {
    let mut positions = read_positions().unwrap_or_default();
    for (label, window) in app.webview_windows() {
        if !label.starts_with(LABEL_PREFIX) {
            continue;
        }
        if let Ok(at) = window.outer_position() {
            positions.insert(label, (at.x, at.y));
        }
    }
    if let Err(err) = write_positions(&positions) {
        eprintln!("[signalhub] 记灯窗位置失败: {err}");
    }
}

fn positions_file() -> io::Result<PathBuf> {
    Ok(config::data_dir()?.join("lamp-positions.json"))
}

fn read_positions() -> io::Result<BTreeMap<String, (i32, i32)>> {
    let text = fs::read_to_string(positions_file()?)?;
    Ok(serde_json::from_str(&text)?)
}

fn write_positions(positions: &BTreeMap<String, (i32, i32)>) -> io::Result<()> {
    let json = serde_json::to_string_pretty(positions)?;
    config::atomic_write(&positions_file()?, &json, None)
}

/// 落盘一份位置快照。**在 `Lamps` 锁外调**（见 [`LampWindows::commit`]）。
pub fn save_positions(positions: &BTreeMap<String, (i32, i32)>) {
    if let Err(err) = write_positions(positions) {
        eprintln!("[signalhub] 记灯窗位置失败: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 只记「建了 / 关了 / 改成多大了」，不碰真窗口。
    #[derive(Default)]
    struct FakeHost {
        open: RefCell<BTreeMap<String, String>>,
        resized: RefCell<BTreeMap<String, (u16, u16)>>,
        closed: RefCell<Vec<String>>,
        /// 已知的位置，`close` 时返回它（模拟「窗口还记得自己拖到哪」）。
        known: BTreeMap<String, (i32, i32)>,
        /// 让建窗失败（按 label），验证「失败会被撤账、下一帧重试」。
        failing: std::cell::RefCell<BTreeSet<String>>,
    }

    impl FakeHost {
        fn open_groups(&self) -> Vec<String> {
            let mut groups: Vec<String> = self.open.borrow().values().cloned().collect();
            groups.sort();
            groups
        }
        fn size_of(&self, group: &str) -> Option<(u16, u16)> {
            let label = label_for(group);
            self.resized.borrow().get(&label).copied()
        }
    }

    impl LampHost for FakeHost {
        fn open(&self, label: &str, group: &str, _at: Option<(i32, i32)>, size: (u16, u16)) -> bool {
            if self.failing.borrow().contains(label) {
                return false;
            }
            self.open.borrow_mut().insert(label.to_string(), group.to_string());
            self.resized.borrow_mut().insert(label.to_string(), size);
            true
        }
        fn resize(&self, label: &str, size: (u16, u16)) {
            self.resized.borrow_mut().insert(label.to_string(), size);
        }
        fn close(&self, label: &str) -> Option<(i32, i32)> {
            self.closed.borrow_mut().push(label.to_string());
            self.open.borrow_mut().remove(label);
            self.known.get(label).copied()
        }
    }

    fn spec(group: &str, w: u16, h: u16) -> LampSpec {
        LampSpec { group: group.to_string(), size: (w, h) }
    }

    #[test]
    fn labels_are_stable_and_window_safe() {
        let label = label_for("build-1");
        assert!(label.starts_with(LABEL_PREFIX), "{label}");
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '/')),
            "label 只能是 Tauri 允许的字符: {label}"
        );
        assert_eq!(label, label_for("build-1"), "同组名永远同 label");

        // 非 ASCII（中文 / 路径 / UUID）也要能区分开 —— 不能全部塌成同一个
        assert_ne!(label_for("我的构建"), label_for("你的构建"));
        assert_eq!(label_for("我的构建"), label_for("我的构建"));
    }

    #[test]
    fn reconcile_opens_keeps_and_closes() {
        let host = FakeHost::default();
        let mut lamps = LampWindows::default();

        lamps.reconcile(&host, &[spec("a", 10, 10), spec("b", 12, 12)]);
        assert_eq!(host.open_groups(), vec!["a", "b"]);

        // 同一批再来一次：一个都不重建、尺寸也没变，所以不 resize
        lamps.reconcile(&host, &[spec("a", 10, 10), spec("b", 12, 12)]);
        assert_eq!(host.open.borrow().len(), 2);

        // c 出现、a 消失
        lamps.reconcile(&host, &[spec("b", 12, 12), spec("c", 14, 14)]);
        assert_eq!(host.open_groups(), vec!["b", "c"]);
        assert_eq!(host.closed.borrow().len(), 1, "只有 a 该被关掉");
    }

    #[test]
    fn reconcile_resizes_only_when_the_size_changed() {
        let host = FakeHost::default();
        let mut lamps = LampWindows::default();

        lamps.reconcile(&host, &[spec("a", 10, 10)]);
        assert_eq!(host.size_of("a"), Some((10, 10)));

        // 尺寸没变：不 resize
        host.resized.borrow_mut().clear();
        lamps.reconcile(&host, &[spec("a", 10, 10)]);
        assert!(host.resized.borrow().is_empty(), "尺寸没变就不该动窗口");

        // 尺寸变了：resize 到新值
        lamps.reconcile(&host, &[spec("a", 20, 30)]);
        assert_eq!(host.size_of("a"), Some((20, 30)));
    }

    #[test]
    fn reconcile_with_no_specs_closes_everything() {
        let host = FakeHost::default();
        let mut lamps = LampWindows::default();
        lamps.reconcile(&host, &[spec("a", 10, 10), spec("b", 12, 12)]);

        lamps.reconcile(&host, &[]);
        assert!(host.open.borrow().is_empty());
        assert_eq!(host.closed.borrow().len(), 2);
    }

    /// 组消失后回来，要能想起它拖到哪儿了（位置按 label 存）。
    #[test]
    fn a_returning_group_recalls_its_position() {
        let mut host = FakeHost::default();
        let label = label_for("a");
        host.known.insert(label.clone(), (340, 512));

        let mut lamps = LampWindows::default();
        lamps.reconcile(&host, &[spec("a", 10, 10)]);
        // 关掉（模拟这一组暂时没有了）
        lamps.reconcile(&host, &[]);
        assert!(lamps.positions.contains_key(&label), "关窗时该把位置记下来");

        // 回来时带着位置
        lamps.reconcile(&host, &[spec("a", 10, 10)]);
        assert_eq!(host.open_groups(), vec!["a"]);
    }

    /// **建窗失败不能被记成「已经开着」** —— 记错了这一组就永远没有灯，而界面上
    /// 和「组还没出现」一模一样。
    #[test]
    fn a_failed_open_is_retried_on_the_next_frame() {
        let host = FakeHost::default();
        host.failing.borrow_mut().insert(label_for("a"));
        let mut lamps = LampWindows::default();

        lamps.reconcile(&host, &[spec("a", 10, 10)]);
        assert!(host.open.borrow().is_empty(), "这一帧确实没建成");
        assert!(lamps.open.is_empty(), "失败不该被记账");

        host.failing.borrow_mut().clear();
        lamps.reconcile(&host, &[spec("a", 10, 10)]);
        assert_eq!(host.open_groups(), vec!["a"], "下一帧该再试一次");
    }

    /// 算计划和动窗口是两步 —— 锁只包住前一步，所以 `plan` 绝不能碰窗口。
    #[test]
    fn plan_computes_without_touching_any_window() {
        let host = FakeHost::default();
        let mut lamps = LampWindows::default();

        let plan = lamps.plan(&[spec("a", 10, 10)]);
        assert_eq!(plan.open.len(), 1);
        assert!(host.open.borrow().is_empty(), "plan 阶段不能建窗");

        // 乐观记账：同一批再来一次，计划里不该重复建
        assert!(lamps.plan(&[spec("a", 10, 10)]).open.is_empty());

        // 尺寸变了才算 resize
        assert_eq!(lamps.plan(&[spec("a", 20, 20)]).resize.len(), 1);

        // 组没了只出 close，仍然不碰窗口
        assert_eq!(lamps.plan(&[]).close, vec![label_for("a")]);
        assert!(host.closed.borrow().is_empty());
    }

    /// 关窗时拿到的位置要在 `commit` 里记下来，并且告诉调用方「该落盘了」。
    #[test]
    fn commit_records_positions_and_reports_a_change() {
        let mut lamps = LampWindows::default();
        lamps.plan(&[spec("a", 10, 10)]);
        lamps.commit(&[], &[]);

        let label = label_for("a");
        assert_eq!(lamps.plan(&[]).close, vec![label.clone()]);
        assert!(!lamps.commit(&[(label.clone(), None)], &[]), "没位置就不用落盘");
        assert!(lamps.commit(&[(label.clone(), Some((7, 9)))], &[]));
        assert_eq!(lamps.positions_snapshot().get(&label), Some(&(7, 9)));
    }

    #[test]
    fn encode_query_keeps_url_safe_chars_and_escapes_the_rest() {
        assert_eq!(encode_query("build-1"), "build-1");
        assert_eq!(encode_query("a b"), "a%20b");
        assert_eq!(encode_query("x&y=z"), "x%26y%3Dz");
        // 中文按 UTF-8 字节编码
        assert_eq!(encode_query("构"), "%E6%9E%84");
    }
}
