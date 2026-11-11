//! 收件箱窗口：把「谁需要你」从灯上搬到一个能点的列表里。
//!
//! 窗口本身是纯前端（`src/components/InboxWindow.tsx`）；这里只管三件窗口层面的事：
//!
//! 1. **开 / 关 / 定位** —— 有两个锚点，各自跟着触发它的那个东西出现：
//!    从灯上打开时贴在灯旁边（灯是用户放好的锚点），从托盘图标左键打开时垂在
//!    图标正下方（点哪长哪，像菜单栏应用的弹出面板）。两个位置都从用户点的地方
//!    长出来，视线不用重新找。
//! 2. **关掉时回到 Accessory** —— 灯球应用平时不占 Dock；收件箱和设置窗一样，
//!    打开时临时变成 Regular，关掉时再退回去（设置窗还开着就别退）。
//! 3. **关窗只隐藏、不销毁** —— 和设置窗同一个理由：重建 webview 比 show 慢，
//!    而且会丢掉滚动位置。**代价**：前端组件整个应用生命周期只挂载一次，所以
//!    「每次打开都要做的事」不能写在 mount 里，得由这里的事件广播驱动。
//! 4. **显示 / 隐藏各广播一次事件** —— `signalhub://inbox-shown` 让前端重算
//!    「你离开的这段时间」，`-hidden` 把「上次看过收件箱」推进到当下。
//!
//! 窗口初始尺寸在 `tauri.conf.json` 里（420×560，可由用户拉伸），这里只做定位；
//! 不参与「尺寸由内容决定」那套灯球几何 —— 收件箱是普通的可滚动窗口，不是像素级
//! 拼出来的牌。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

/// 窗口 label。`tauri.conf.json` 与 `capabilities/default.json` 里必须一致。
pub const LABEL: &str = "inbox";

/// 收件箱与灯之间的间距（物理像素）。
const GAP: i32 = 12;

/// 托盘图标与面板之间的间距（物理像素）。
///
/// 比贴灯的 12px 小：macOS 的 work area 已经把菜单栏让出来了，图标底边正好等于
/// 工作区上沿，再空 12px 会看着「飘」在半空。
const TRAY_GAP: i32 = 6;

/// 每次 show / hide 都发。名字和 `InboxWindow.tsx` 里监听的字符串是两处真值，
/// 改一处必须同步另一处。
const SHOWN_EVENT: &str = "signalhub://inbox-shown";
const HIDDEN_EVENT: &str = "signalhub://inbox-hidden";

/// 打开 / 关闭。灯上圆环菜单、全局快捷键都走这一个入口 —— 「点一下开着就关掉」
/// 是弹层的直觉，不需要第二个命令。从托盘图标来的走 `toggle_from_tray`。
pub fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide(app);
    } else {
        show(app);
    }
}

/// 托盘图标左键：显示 / 收起，位置贴着托盘图标。
///
/// **为什么不能直接复用 `toggle`**：面板可见时点托盘图标，鼠标按下的那一刻窗口
/// 先失焦 → `wire` 把它收起；紧接着托盘的 Click(Up) 才到，`toggle` 会看到窗口
/// 已不可见、又把它打开 —— 于是「点图标关不掉面板」。这里先吃掉那次
/// 「刚刚因失焦收起」的点击（见 `take_recent_blur_hide`）。
pub fn toggle_from_tray(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide(app);
        return;
    }
    if take_recent_blur_hide() {
        return;
    }
    show_at_tray(app);
}

pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    present(app, &window, lamp_position(app, &window));
}

/// 从托盘图标弹出：优先图标正下方，拿不到图标位置（Linux 不支持 `tray.rect()`）
/// 就退回贴灯 —— 总比出现在系统给的默认位置好。
///
/// 公开给托盘**菜单**那一项用（右键菜单第一项「收件箱」）。它**不能**走
/// `toggle_from_tray`：菜单弹出来的时候面板已经因失焦收起了，那笔 blur 记录还在，
/// 走那条路会把这次点击吞掉，变成「点了没反应」。
pub fn show_at_tray(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let position = tray_position(app, &window).or_else(|| lamp_position(app, &window));
    present(app, &window, position);
}

/// 真正把窗口亮出来。位置由调用方算好（`None` = 保持系统上次给的位置）。
fn present(app: &AppHandle, window: &WebviewWindow, position: Option<(i32, i32)>) {
    // 灯球是 Accessory：不切 Regular 的话窗口不会到前台、也拿不到键盘焦点。
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

    // 标题跟着界面语言；窗口无边框时它主要出现在 Mission Control / 窗口列表里。
    let _ = window.set_title(crate::lang::text(crate::current_lang(app)).inbox_window_title);
    if let Some((x, y)) = position {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_focus();
    // 窗口只隐藏不销毁，所以「每次打开」这件事只能靠事件通知前端，不能靠组件挂载。
    let _ = app.emit(SHOWN_EVENT, ());
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    let _ = app.emit(HIDDEN_EVENT, ());
    restore_activation(app);
}

/// 收件箱 / 设置窗关掉之后：另一个还开着就留在 Regular，两个都关了就回到
/// Accessory。**不能无条件退回** —— 那会让同时开着的另一个窗口突然失去焦点行为。
#[cfg(target_os = "macos")]
pub fn restore_activation(app: &AppHandle) {
    let any_open = ["settings", LABEL].iter().any(|label| {
        app.get_webview_window(label)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false)
    });
    if !any_open {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn restore_activation(_app: &AppHandle) {}

/// 关窗只隐藏；`CloseRequested` 在 macOS 上也会由 Cmd+W 触发。
/// 窗口配置里已经 `closable: false`，这里是第二道闸（不同平台对无边框窗口的处理
/// 不完全一样），顺带把激活策略收回来。
pub fn wire(app: &tauri::App) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let handle = window.clone();
    window.on_window_event(move |event| {
        let app = handle.app_handle();
        match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = handle.hide();
                // 只要是「隐藏」就必须广播：前端只认这条事件推进「上次看过」的基准，
                // 三条隐藏路径（命令 / 失焦 / 关闭）漏一条，摘要的基准就会偏。
                let _ = app.emit(HIDDEN_EVENT, ());
                restore_activation(app);
            }
            // 像弹层一样：点到别的应用就收起。没有这一条的话，收件箱会一直
            // 置顶挡在终端前面 —— 而它本来只是「看一眼、处理一条」的地方。
            tauri::WindowEvent::Focused(false) => {
                mark_blur_hide();
                let _ = handle.hide();
                // 失焦隐藏不经过 `hide(app)`，基准得在这里补写一次 —— 否则「上次
                // 看过收件箱」会一路停到上回主动关窗那一刻。
                let _ = app.emit(HIDDEN_EVENT, ());
                restore_activation(app);
            }
            _ => {}
        }
    });
}

/// 把收件箱放到灯旁边：优先左侧，左边放不下放右侧，两边都不够就居中；竖直方向
/// 让它的底边贴住灯的底边，并夹进工作区。
///
/// 纯函数，输入输出都是物理像素；单测覆盖了「灯在右缘 / 左缘 / 收件箱比屏幕还高」
/// 这些在真机上很难摆出来的边界。
fn place(
    lamp: (i32, i32, i32, i32),
    inbox_size: (i32, i32),
    area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (lx, ly, lw, lh) = lamp;
    let (iw, ih) = inbox_size;
    let (ax, ay, aw, ah) = area;

    let max_x = ax + (aw - iw).max(0);
    let left = lx - iw - GAP;
    let right = lx + lw + GAP;
    let x = if left >= ax {
        left
    } else if right <= max_x {
        right
    } else {
        ax + (aw - iw) / 2
    };
    let x = x.clamp(ax, max_x);

    let max_y = ay + (ah - ih).max(0);
    // 底边对齐：收件箱的脚和灯的脚在同一条线上，比顶对齐更像「从这里长出来」。
    let y = (ly + lh - ih).clamp(ay, max_y);

    (x, y)
}

fn lamp_position(app: &AppHandle, window: &WebviewWindow) -> Option<(i32, i32)> {
    let lamp = app.get_webview_window("main")?;
    let (Ok(position), Ok(size), Ok(inbox), Some(monitor)) = (
        lamp.outer_position(),
        lamp.outer_size(),
        window.outer_size(),
        lamp.current_monitor().ok().flatten(),
    ) else {
        // 拿不到几何就算了：窗口会出现在系统给它的默认位置。**不要**为了「有个位置」
        // 硬猜一个 —— 那可能把收件箱放到另一块屏上。
        return None;
    };

    let area = monitor.work_area();
    Some(place(
        (position.x, position.y, size.width as i32, size.height as i32),
        (inbox.width as i32, inbox.height as i32),
        (
            area.position.x,
            area.position.y,
            area.size.width as i32,
            area.size.height as i32,
        ),
    ))
}

/// 贴在锚点正下方时的位置：水平居中于锚点，竖直在它下面 `TRAY_GAP` 处，
/// 再整体夹进工作区。
///
/// 锚点通常比面板窄得多（菜单栏图标十几点宽、面板 420 宽），所以「居中」是唯一
/// 看着对的对齐方式 —— 左对齐会让面板从图标右边甩出去，右对齐则反过来。
fn place_below(
    anchor: (i32, i32, i32, i32),
    panel: (i32, i32),
    area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (ax, ay, aw, ah) = anchor;
    let (pw, ph) = panel;
    let (wx, wy, ww, wh) = area;

    let max_x = wx + (ww - pw).max(0);
    let x = (ax + aw / 2 - pw / 2).clamp(wx, max_x);

    // 贴着屏幕顶部时工作区已经把菜单栏让出来了，所以这里通常正好落在工作区上沿；
    // 图标贴着底部（任务栏在下方）时会被夹回去，至少标题栏还在。
    let max_y = wy + (wh - ph).max(0);
    let y = (ay + ah + TRAY_GAP).clamp(wy, max_y);

    (x, y)
}

/// 托盘图标所在的那块矩形，换算成物理像素。
///
/// 三个平台的 `tray.rect()` 给的单位不一样（macOS 直接返回物理像素，Windows 走
/// 逻辑坐标），所以这里统一按 `Rect` 的枚举分支处理。逻辑坐标只在拿不到物理值时
/// 才出现，用一个缩放比足够 —— 托盘图标在菜单栏上，缩放比跟着主屏走。
fn tray_rect_physical(app: &AppHandle, rect: &tauri::Rect) -> (i32, i32, i32, i32) {
    use tauri::{Position, Size};

    let scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0);

    let (x, y) = match rect.position {
        Position::Physical(p) => (p.x, p.y),
        Position::Logical(p) => (
            (p.x * scale).round() as i32,
            (p.y * scale).round() as i32,
        ),
    };
    let (w, h) = match rect.size {
        Size::Physical(s) => (s.width as i32, s.height as i32),
        Size::Logical(s) => (
            (s.width * scale).round() as i32,
            (s.height * scale).round() as i32,
        ),
    };
    (x, y, w, h)
}

fn tray_position(app: &AppHandle, window: &WebviewWindow) -> Option<(i32, i32)> {
    let tray = app.tray_by_id("signalhub")?;
    let rect = tray.rect().ok().flatten()?;
    let (ax, ay, aw, ah) = tray_rect_physical(app, &rect);
    let inbox = window.outer_size().ok()?;

    // 面板要出现在图标所在的那块屏上：先按图标中心点找屏，找不到再退回主屏。
    // 直接拿主屏的工作区会在副屏的菜单栏上把面板摆到另一块屏去。
    let monitor = app
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                ax + aw / 2 >= position.x
                    && ax + aw / 2 < position.x + size.width as i32
                    && ay + ah / 2 >= position.y
                    && ay + ah / 2 < position.y + size.height as i32
            })
        })
        .or_else(|| app.primary_monitor().ok().flatten())?;

    let area = monitor.work_area();
    Some(place_below(
        (ax, ay, aw, ah),
        (inbox.width as i32, inbox.height as i32),
        (
            area.position.x,
            area.position.y,
            area.size.width as i32,
            area.size.height as i32,
        ),
    ))
}

// ---- 「刚因失焦收起」的那次托盘点击 -----------------------------------------
//
// 时序问题（macOS 上每次都发生）：面板可见时点托盘图标 —— 鼠标按下让面板失焦
// → `wire` 收起面板 → 托盘 Click(Up) 到达，`toggle_from_tray` 看到窗口不可见，
// 会把它重新打开。用户看到的是「点图标关不掉面板」。
//
// 所以收起时记一笔时刻，400ms 内到达的那次托盘点击视为「收起」而不是「打开」。
// **只记失焦这一条路径**：Esc / × 是明确的关闭动作，用户随后点图标就是要重新
// 打开，不该被吞掉。

/// 上一次「因为失焦而收起」的时刻。
static BLUR_HIDDEN_AT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

/// 点托盘图标到面板失焦收起之间，这点时间足够；再长就会把「刚关掉又想打开」
/// 的正常操作也吞掉。
const TRAY_CLICK_GRACE: Duration = Duration::from_millis(400);

fn blur_marks() -> &'static Mutex<Option<Instant>> {
    BLUR_HIDDEN_AT.get_or_init(|| Mutex::new(None))
}

fn mark_blur_hide() {
    *blur_marks().lock().expect("blur mark poisoned") = Some(Instant::now());
}

/// 读一次就清掉 —— 一次收起只该吞掉一次点击。
fn take_recent_blur_hide() -> bool {
    let mut guard = blur_marks().lock().expect("blur mark poisoned");
    let recent = guard.is_some_and(|at| at.elapsed() < TRAY_CLICK_GRACE);
    *guard = None;
    recent
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
    const INBOX: (i32, i32) = (420, 560);

    /// 灯在右下角（最常见的位置）：收件箱放在它左边、底边对齐。
    #[test]
    fn it_sits_left_of_a_right_edge_lamp() {
        let (x, y) = place((1800, 900, 42, 42), INBOX, AREA);
        assert_eq!(x, 1800 - 420 - GAP);
        assert_eq!(y, 900 + 42 - 560);
    }

    /// 灯贴左缘时左边放不下，改放右边；竖直方向拉回工作区。
    #[test]
    fn it_falls_back_to_the_right_side_and_clamps_vertically() {
        let (x, y) = place((10, 10, 42, 42), INBOX, AREA);
        assert_eq!(x, 10 + 42 + GAP);
        assert_eq!(y, 0, "顶到屏幕外时要夹回工作区");
    }

    /// 灯在另一块屏的负坐标上同样成立（工作区原点不一定是 0,0）。
    #[test]
    fn it_honours_a_negative_work_area_origin() {
        let area = (-1440, -200, 1440, 900);
        let (x, y) = place((-1460, 100, 42, 42), INBOX, area);
        assert!(x >= area.0, "{x}");
        assert!(x + INBOX.0 <= area.0 + area.2, "{x}");
        assert!(y >= area.1, "{y}");
        assert!(y + INBOX.1 <= area.1 + area.3, "{y}");
    }

    /// 收件箱比工作区还高时不能算出负数 —— 先夹到上边缘，至少标题栏还在。
    #[test]
    fn an_oversized_inbox_stays_at_the_top() {
        let area = (0, 0, 800, 300);
        let (_, y) = place((700, 250, 42, 42), (420, 560), area);
        assert_eq!(y, 0);
    }

    /// 托盘图标在菜单栏中央：面板水平居中于图标，垂在它下面。
    #[test]
    fn it_hangs_below_a_centred_tray_icon() {
        let (x, y) = place_below((900, 0, 24, 24), INBOX, AREA);
        assert_eq!(x, 900 + 12 - 210);
        assert_eq!(y, 24 + TRAY_GAP);
    }

    /// 图标贴着左缘时面板会被推出屏幕 —— 必须夹回来，而不是留半块在屏幕外。
    #[test]
    fn it_clamps_a_left_edge_tray_icon() {
        let (x, _) = place_below((0, 0, 24, 24), INBOX, AREA);
        assert_eq!(x, 0);

        let (x, _) = place_below((1896, 0, 24, 24), INBOX, AREA);
        assert_eq!(x, 1920 - INBOX.0);
    }

    /// 任务栏在下方、面板比剩下的高度还高时，不能算出屏幕外的 y。
    #[test]
    fn a_short_work_area_pulls_the_panel_back_in() {
        let area = (0, 0, 1920, 400);
        let (_, y) = place_below((900, 0, 24, 24), INBOX, area);
        assert_eq!(y, 0, "面板比工作区高时应贴在顶边");
    }

    /// 负坐标的副屏（左侧 / 上方那块）同样成立。
    #[test]
    fn place_below_honours_a_negative_work_area_origin() {
        let area = (-1440, -200, 1440, 900);
        let (x, y) = place_below((-1130, -200, 24, 24), INBOX, area);
        assert!(x >= area.0, "{x}");
        assert!(x + INBOX.0 <= area.0 + area.2, "{x}");
        assert!(y >= area.1, "{y}");
        assert!(y + INBOX.1 <= area.1 + area.3, "{y}");
    }

    /// 失焦记的那一笔只该吞掉「紧接着」的那次点击，而且只能吞一次。
    #[test]
    fn a_blur_mark_only_swallows_one_click() {
        // 先清干净：其它用例可能留下记录（同一个进程里 static 是共享的）
        let _ = take_recent_blur_hide();

        assert!(!take_recent_blur_hide(), "没有记录时不该吞点击");

        mark_blur_hide();
        assert!(take_recent_blur_hide(), "刚收起后的第一次点击应被吞掉");
        assert!(!take_recent_blur_hide(), "第二次点击必须正常打开面板");
    }
}
