//! 展示模式的**窗口层**：选屏、铺满、显形、关窗。
//!
//! 这一层只管窗口，不管「开 / 关」这件事的状态语义（那在 `lib.rs::set_stage_impl`）。
//! 分界的理由：`UiFlags` 是瞬态真值，窗口只是它的一个投影；把两者混在一起，就会
//! 出现「窗口 hide 了但状态还开着」这类半套状态。
//!
//! **为什么是静态窗口 + show/hide，而不是运行时 `WebviewWindowBuilder`**：
//! `tauri.conf.json` 里 `visible: false` 的窗口启动时只加载 webview、不显示，设置窗
//! 和收件箱都是这个做法。展示模式要求「默认关、不自动」，指的是不自动**显示**，
//! 不是不加载。静态声明换来三件确定的事：关窗事件能在 `setup` 里一次挂好、
//! 每次进入不必重建 webview、capability 的 label 在编译期就固定。运行时建窗那条路
//! 留给组灯（`lamps.rs`），它有「组数会变」这个静态配置满足不了的理由。
//!
//! **「铺满屏幕」和「盖住系统界面」是两件事**，得分开做：
//!
//! - macOS：窗口按显示器的物理尺寸算逻辑尺寸、精确摆到那块屏上，**不用**
//!   `set_fullscreen` —— tao 在 macOS 上把它译成 `toggleFullScreen:`，会另开一个
//!   Space，退出时桌面要滑回来，而展示模式要的是「按一下立刻回到原样」。可是
//!   Dock 与菜单栏的窗口层级高于普通窗口，光铺满只会被它们压住（第一版实测：
//!   屏幕满了，Dock 还在上面）。所以另外把本 app 的
//!   `NSApplication.presentationOptions` 临时设成 `AutoHideDock | AutoHideMenuBar`。
//!   选「AutoHide」而不是「Hide」：鼠标移到屏幕边缘 Dock 仍会滑出来，想点一下
//!   别的 app 不必先退出展示模式。
//! - Windows / Linux：任务栏是 topmost，X11 面板只认 `_NET_WM_STATE_FULLSCREEN`，
//!   普通窗口同样盖不住。这两家用无边框全屏 —— 和 macOS 的 Space 不同，它**不会**
//!   新开桌面，且一并完成摆放与让位。
//!
//! 已知降级：Wayland 上的置顶 / 全屏语义由合成器决定，见 `docs/architecture.md` §7。

use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

/// 窗口 label。`tauri.conf.json` 与 `capabilities/default.json` 里必须一致。
pub const LABEL: &str = "stage";

/// 一块屏的几何。**物理像素 + 缩放比**，和 `lib.rs::work_bounds` 同一套坐标。
///
/// 单独一个结构而不是直接用 `tauri::Monitor`：选屏规则要能被单测走到，而
/// `Monitor` 在测试里造不出来。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

/// 展示窗口最终要摆到的位置与大小。位置是物理像素，尺寸是**逻辑像素**
/// （`LogicalSize` 会由 Tauri 按缩放比换成物理像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageRect {
    pub x: i32,
    pub y: i32,
    pub width: f64,
    pub height: f64,
}

fn monitor_rect(monitor: &tauri::Monitor) -> MonitorRect {
    let position = monitor.position();
    let size = monitor.size();
    MonitorRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        scale: monitor.scale_factor(),
    }
}

fn contains(rect: MonitorRect, at: (i32, i32)) -> bool {
    let x = i64::from(at.0);
    let y = i64::from(at.1);
    x >= i64::from(rect.x)
        && y >= i64::from(rect.y)
        && x < i64::from(rect.x) + i64::from(rect.width)
        && y < i64::from(rect.y) + i64::from(rect.height)
}

/// 点到矩形的距离平方，矩形内为 0。和 `lib.rs::distance_to_bounds` 同一套判据，
/// 但这里用的是整屏矩形（`work_bounds` 是工作区，会把菜单栏那条让出去）。
fn distance_sq(rect: MonitorRect, at: (i32, i32)) -> i64 {
    let left = i64::from(rect.x);
    let top = i64::from(rect.y);
    let right = left + i64::from(rect.width);
    let bottom = top + i64::from(rect.height);
    let x = i64::from(at.0);
    let y = i64::from(at.1);
    let dx = (left - x).max(0).max(x - right);
    let dy = (top - y).max(0).max(y - bottom);
    dx * dx + dy * dy
}

/// 挑一块屏：鼠标落在哪块就用哪块，落在屏外（多屏拔插的瞬间）取最近的一块，
/// 拿不到鼠标位置就取第一块。空列表返回 `None`，由调用方退回主屏。
///
/// 返回的是**下标**而不是 `MonitorRect`：非 macOS 上无边框全屏要一个
/// `tauri::Monitor` 句柄，而那个类型造不出来、也不便比相等，只能靠下标回原表取。
pub fn pick_monitor_index(monitors: &[MonitorRect], cursor: Option<(i32, i32)>) -> Option<usize> {
    if monitors.is_empty() {
        return None;
    }
    let Some(at) = cursor else { return Some(0) };
    if let Some(index) = monitors.iter().position(|rect| contains(*rect, at)) {
        return Some(index);
    }
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, rect)| distance_sq(**rect, at))
        .map(|(index, _)| index)
}

/// 物理整屏 → 逻辑窗口尺寸。缩放比拿不到 / 不合法时按 1 处理（明示降级，
/// 不假装自己知道）；尺寸为 0 时返回 `None`，交回系统摆放。
pub fn stage_rect(position: (i32, i32), size: (u32, u32), scale: f64) -> Option<StageRect> {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let width = f64::from(size.0) / scale;
    let height = f64::from(size.1) / scale;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(StageRect {
        x: position.0,
        y: position.1,
        width,
        height,
    })
}

/// 进入展示模式前的那份瞬态状态。退出时按它恢复，不写回 Settings。
///
/// `peek` / `menu` 存的是 label 而不是 bool —— 多灯下只恢复被收掉的那一盏，
/// 否则「退出展示模式」会把所有灯都展开一遍。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StagePrevious {
    pub pass_through: bool,
    pub peek: Option<String>,
    pub menu: Option<String>,
}

/// 「这次调用到底该做什么」——抽成纯函数，状态机的边界才有单测兜住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagePlan {
    /// 已经是目标状态。再按一次「进入」也不该把 since_ms 重置掉。
    Idempotent,
    Enter,
    Exit,
    /// 穿透开着时不进入：那盏灯正在替用户挡误点，不该被一个展示动作悄悄改掉。
    RefusePassThrough,
}

pub fn stage_plan(active: bool, pass_through: bool, enabled: bool) -> StagePlan {
    if enabled == active {
        return StagePlan::Idempotent;
    }
    if enabled && pass_through {
        return StagePlan::RefusePassThrough;
    }
    if enabled {
        StagePlan::Enter
    } else {
        StagePlan::Exit
    }
}

/// 这次该摆到哪块屏：返回「几何 + 显示器句柄」。
///
/// **优先鼠标所在屏**（会议投屏时鼠标多半刚点在副屏上），拿不到显示器清单才退回
/// 主屏；两者都拿不到返回 `(None, None)`，让调用方只 show、沿用配置里的默认尺寸
/// —— 明示降级而不是假装三平台一致。
///
/// 几何与句柄成对返回是刻意的：macOS 只要几何（自己摆窗口），Windows / Linux 要
/// 句柄（交给 `Fullscreen::Borderless`），两边都不必再查一次显示器列表。
pub fn stage_target(app: &AppHandle) -> (Option<StageRect>, Option<tauri::Monitor>) {
    let monitors = app.available_monitors().ok().unwrap_or_default();
    let rects: Vec<MonitorRect> = monitors.iter().map(monitor_rect).collect();
    let cursor = app
        .cursor_position()
        .ok()
        .map(|point| (point.x.round() as i32, point.y.round() as i32));
    let picked = pick_monitor_index(&rects, cursor).and_then(|index| monitors.get(index).cloned());
    let monitor = picked.or_else(|| app.primary_monitor().ok().flatten());
    let target = monitor
        .as_ref()
        .map(monitor_rect)
        .and_then(|rect| stage_rect((rect.x, rect.y), (rect.width, rect.height), rect.scale));
    (target, monitor)
}

/// 进 / 出全屏（Windows、Linux 用）。
///
/// **在 macOS 上也参与编译**（只是没人调用，`allow(dead_code)` 就是为这个加的）：
/// 三平台里只有 macOS 是本地开发环境，这段代码若跟着 `#[cfg]` 一起被排除，
/// 「Windows 上类型对不对」就只能等 CI 变红才知道。上次 `shortcut_label` 的
/// `Alt+Super` 就是这么漏掉的（见 `docs/architecture.md` §7.1）。
///
/// 为什么用 `set_fullscreen_on_monitor` 而不是 `set_fullscreen`：展示模式明确要在
/// 光标那块屏上铺开，而带 `on_monitor` 的这个会用显示器位置去定位，不依赖「窗口
/// 当前在哪块屏」。Tauri 2 对外只有 bool，没有 tao 那套 `Fullscreen::Borderless`
/// 枚举 —— 别照着 tao 的 API 写。
///
/// 这两家的全屏**不会**新开桌面（和 macOS 的 Space 不同），而且会把任务栏 / 面板
/// 一并挤走，所以「铺满 + 让位」是一步。
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn fullscreen_on(
    window: &WebviewWindow,
    monitor: Option<&tauri::Monitor>,
    enabled: bool,
) -> Result<(), String> {
    if !enabled {
        return window
            .set_fullscreen(false)
            .map_err(|err| format!("退出展示窗口全屏失败: {err}"));
    }
    match monitor {
        Some(monitor) => {
            let position = monitor.position();
            window
                .set_fullscreen_on_monitor(PhysicalPosition::new(
                    f64::from(position.x),
                    f64::from(position.y),
                ))
                .map_err(|err| format!("切换展示窗口全屏失败: {err}"))
        }
        // 拿不到显示器清单时退回「当前屏」；`None` 在 Tauri 里就是没有目标屏可指定。
        None => window
            .set_fullscreen(true)
            .map_err(|err| format!("切换展示窗口全屏失败: {err}")),
    }
}

/// 占住那块屏：摆好窗口，并让系统的 Dock / 任务栏 / 面板让位。
///
/// 平台差异全部收在这一个函数里 —— `present` 只管「显形 + 聚焦」。
fn occupy_screen(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let (target, _monitor) = stage_target(app);
        match target {
            Some(rect) => {
                window
                    .set_size(LogicalSize::new(rect.width, rect.height))
                    .map_err(|err| format!("设置展示窗口尺寸失败: {err}"))?;
                window
                    .set_position(PhysicalPosition::new(rect.x, rect.y))
                    .map_err(|err| format!("摆放展示窗口失败: {err}"))?;
            }
            None => eprintln!("[signalhub] 拿不到显示器信息，展示窗口沿用默认尺寸（明示降级）"),
        }
        // 只铺满不够：Dock 与菜单栏在更高的窗口层级上，会把展示窗压住。
        on_main(app, system_bars::hide);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let (_target, monitor) = stage_target(app);
        fullscreen_on(window, monitor.as_ref(), true)?;
    }

    Ok(())
}

/// 把窗口亮出来。顺序是刻意的：**先摆好，再显示** —— 反过来会先看到一帧
/// 配置里的 1280×800 小窗闪在屏幕中间，然后才跳到目标屏。
pub fn present(app: &AppHandle, lang: crate::config::Lang) -> Result<(), String> {
    let window: WebviewWindow = app
        .get_webview_window(LABEL)
        .ok_or_else(|| "找不到展示窗口".to_string())?;
    // 灯球是 Accessory：不切 Regular 的话窗口不会到前台、Esc 也落不到这个 webview。
    // 要在摆窗口之前切 —— 反过来可能先按 Accessory 的规则激活一次，
    // 系统随后再校正窗口位置。
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    occupy_screen(app, &window)?;
    let _ = window.set_title(crate::lang::text(lang).stage_window_title);
    window.show().map_err(|err| format!("打开展示窗口失败: {err}"))?;
    // 静态窗口配的是 focus:false（启动时不抢焦点），键盘逃生要靠这里主动聚焦。
    let _ = window.set_focus();
    Ok(())
}

/// 只隐藏、不销毁：下次进入不用重建 webview，组件也只挂载一次。
pub fn dismiss(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        // 退出全屏要显式做：窗口会被 hide，但全屏状态若留在 WM 那边，
        // 下次进入时「设全屏」会被当成无变化而跳过。
        #[cfg(not(target_os = "macos"))]
        let _ = fullscreen_on(&window, None, false);
        let _ = window.hide();
    }
    // 系统界面必须还原，否则退出展示模式后整台机器都看不到 Dock ——
    // 这是「进入时要记录，退出时必须还」的那类状态，不能只做一半。
    #[cfg(target_os = "macos")]
    on_main(app, system_bars::show);
    crate::inbox::restore_activation(app);
}

/// 把闭包丢到主线程执行。**AppKit 的 `presentationOptions` 只能在主线程改**，
/// 而命令跑在 IPC 线程上。
///
/// 不阻塞等待：投递之后立刻 `show()` 最多让 Dock 多留一帧，换来的是
/// 「`present` / `dismiss` 永远不会因为等主线程而死锁」。已经在主线程时走同步，
/// 这样 `tauri dev` 里从主线程调用也能立刻生效。
#[cfg(target_os = "macos")]
fn on_main(app: &AppHandle, f: impl FnOnce() + Send + 'static) {
    if objc2::MainThreadMarker::new().is_some() {
        f();
        return;
    }
    if let Err(err) = app.run_on_main_thread(f) {
        eprintln!("[signalhub] 投递到主线程失败: {err}");
    }
}

/// macOS：把 Dock 与菜单栏**自动藏起来**（不是禁掉）。
///
/// 用 `AutoHide` 而不是 `Hide`：鼠标移到屏幕边缘它们仍会滑出来。展示模式是
/// 「排满一屏给人看」，不是 Kiosk —— 用户想临时点一下别的 app，不该被迫先退出。
#[cfg(target_os = "macos")]
mod system_bars {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationPresentationOptions};
    use std::sync::Mutex;

    /// 进入前系统给这个 app 的 presentationOptions。`0` 就是
    /// `NSApplicationPresentationDefault`；退出时原样还回去，不假设「一定是 0」。
    ///
    /// 存 `usize` 而不是那个 bitflags 类型：静态量要求 `Sync`，存整数没有语义损失。
    static PREVIOUS: Mutex<Option<usize>> = Mutex::new(None);

    pub(super) fn hide() {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("[signalhub] 不在主线程，跳过隐藏 Dock / 菜单栏");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let current = app.presentationOptions().bits();
        *PREVIOUS.lock().expect("poisoned") = Some(current);
        app.setPresentationOptions(
            NSApplicationPresentationOptions::AutoHideDock
                | NSApplicationPresentationOptions::AutoHideMenuBar,
        );
    }

    pub(super) fn show() {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("[signalhub] 不在主线程，跳过还原 Dock / 菜单栏");
            return;
        };
        // `take` 而不是读：没进过展示模式时这里是 `None`，那就什么都不做 ——
        // 不能凭空把别人的 presentationOptions 改成 0。
        let Some(previous) = PREVIOUS.lock().expect("poisoned").take() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setPresentationOptions(NSApplicationPresentationOptions::from_bits_truncate(previous));
    }
}

/// 关窗 = 退出展示模式。不能只 `hide()`：那样 `UiFlags.stage` 会一直开着，
/// 托盘写着「退出展示模式」、穿透也被一直挡着，而屏幕上什么都没有。
pub fn wire(app: &tauri::App) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Err(err) = crate::set_stage_impl(handle.app_handle(), false) {
                eprintln!("[signalhub] 关闭展示窗口时退出失败: {err}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(x: i32, y: i32, width: u32, height: u32, scale: f64) -> MonitorRect {
        MonitorRect {
            x,
            y,
            width,
            height,
            scale,
        }
    }

    /// 断言辅助：按下标回原表取。真实代码里这一步由 `stage_target` 做。
    fn pick(monitors: &[MonitorRect], at: Option<(i32, i32)>) -> Option<MonitorRect> {
        pick_monitor_index(monitors, at).map(|index| monitors[index])
    }

    #[test]
    fn picks_the_screen_under_the_cursor() {
        let primary = screen(0, 0, 1920, 1080, 2.0);
        // 副屏在左侧且带负坐标：这里最容易写成「只比 x」而挑错屏
        let external = screen(-2560, -200, 2560, 1440, 1.0);
        let monitors = [primary, external];
        assert_eq!(pick(&monitors, Some((-100, 100))), Some(external));
        assert_eq!(pick(&monitors, Some((100, 100))), Some(primary));
        // 光标落在两块屏之外时按「最近」兜住，而不是返回 None
        assert_eq!(pick(&monitors, Some((2000, 300))), Some(primary));
    }

    #[test]
    fn cursor_outside_every_screen_uses_the_nearest_one() {
        let left = screen(-2560, 0, 2560, 1440, 1.0);
        let right = screen(0, 0, 1920, 1080, 2.0);
        assert_eq!(pick(&[left, right], Some((-3000, 100))), Some(left));
        assert_eq!(pick(&[left, right], Some((3000, 100))), Some(right));
    }

    #[test]
    fn missing_cursor_or_monitors_degrades_instead_of_panicking() {
        let only = screen(0, 0, 1920, 1080, 1.0);
        assert_eq!(pick(&[only], None), Some(only));
        assert_eq!(pick(&[], Some((10, 10))), None);
        assert_eq!(pick(&[], None), None);
    }

    #[test]
    fn index_points_back_at_the_same_screen() {
        // 句柄只能靠下标回原表取，下标一旦错位就会把窗口摆到另一块屏上 ——
        // 这块屏是负坐标的副屏，错位会表现为「窗口凭空消失」。
        let primary = screen(0, 0, 1920, 1080, 2.0);
        let external = screen(-2560, -200, 2560, 1440, 1.0);
        let monitors = [primary, external];
        assert_eq!(pick_monitor_index(&monitors, Some((10, 10))), Some(0));
        assert_eq!(pick_monitor_index(&monitors, Some((-10, 10))), Some(1));
        assert_eq!(pick_monitor_index(&monitors, None), Some(0));
    }

    #[test]
    fn retina_screen_is_converted_to_logical_size() {
        // 3456×2234 @2 → 1728×1117 逻辑像素；位置保持物理坐标
        assert_eq!(
            stage_rect((0, 0), (3456, 2234), 2.0),
            Some(StageRect {
                x: 0,
                y: 0,
                width: 1728.0,
                height: 1117.0,
            })
        );
        assert_eq!(
            stage_rect((1920, -200), (1920, 1080), 1.0),
            Some(StageRect {
                x: 1920,
                y: -200,
                width: 1920.0,
                height: 1080.0,
            })
        );
    }

    #[test]
    fn bad_scale_or_zero_size_degrades() {
        // scale 为 0 / NaN 时按 1 处理，不是把窗口除成无穷大
        assert_eq!(
            stage_rect((0, 0), (800, 600), 0.0),
            Some(StageRect {
                x: 0,
                y: 0,
                width: 800.0,
                height: 600.0,
            })
        );
        assert_eq!(
            stage_rect((0, 0), (800, 600), f64::NAN),
            Some(StageRect {
                x: 0,
                y: 0,
                width: 800.0,
                height: 600.0,
            })
        );
        assert_eq!(stage_rect((0, 0), (0, 0), 2.0), None);
    }

    #[test]
    fn stage_plan_refuses_to_disrupt_active_click_through() {
        assert_eq!(stage_plan(false, true, true), StagePlan::RefusePassThrough);
        assert_eq!(stage_plan(false, false, true), StagePlan::Enter);
        assert_eq!(stage_plan(true, false, false), StagePlan::Exit);
        // 幂等：重复进入不该把「离开这段时间」的起点刷新掉
        assert_eq!(stage_plan(true, false, true), StagePlan::Idempotent);
        assert_eq!(stage_plan(false, false, false), StagePlan::Idempotent);
        // 退出穿透后仍能进入；退出展示模式从不因穿透被拒
        assert_eq!(stage_plan(false, false, true), StagePlan::Enter);
        assert_eq!(stage_plan(true, true, false), StagePlan::Exit);
    }

    #[test]
    fn previous_keeps_the_exact_lamp_label() {
        let previous = StagePrevious {
            pass_through: false,
            peek: Some("main".to_string()),
            menu: Some("lamp-build-a1b2".to_string()),
        };
        assert_eq!(previous.peek.as_deref(), Some("main"));
        assert_eq!(previous.menu.as_deref(), Some("lamp-build-a1b2"));
        assert_eq!(StagePrevious::default(), StagePrevious {
            pass_through: false,
            peek: None,
            menu: None,
        });
    }
}
