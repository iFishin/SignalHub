//! 收件箱窗口：把「谁需要你」从灯上搬到一个能点的列表里。
//!
//! 窗口本身是纯前端（`src/components/InboxWindow.tsx`）；这里只管三件窗口层面的事：
//!
//! 1. **开 / 关 / 定位** —— 贴在灯旁边，而不是屏幕中央。灯是用户放好的锚点，
//!    收件箱跟着它出现，视线不用重新找。
//! 2. **关掉时回到 Accessory** —— 灯球应用平时不占 Dock；收件箱和设置窗一样，
//!    打开时临时变成 Regular，关掉时再退回去（设置窗还开着就别退）。
//! 3. **关窗只隐藏、不销毁** —— 和设置窗同一个理由：重建 webview 比 show 慢，
//!    而且会丢掉滚动位置。
//!
//! 窗口尺寸在 `tauri.conf.json` 里固定（420×560），这里只做定位；不参与「尺寸由
//! 内容决定」那套灯球几何 —— 收件箱是普通的可滚动窗口，不是像素级拼出来的牌。

use tauri::{AppHandle, Manager, WebviewWindow};

/// 窗口 label。`tauri.conf.json` 与 `capabilities/default.json` 里必须一致。
pub const LABEL: &str = "inbox";

/// 收件箱与灯之间的间距（物理像素）。
const GAP: i32 = 12;

/// 打开 / 关闭。灯、托盘菜单、全局快捷键都走这一个入口 —— 「点一下开着就关掉」
/// 是弹层的直觉，不需要第二个命令。
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

pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };

    // 灯球是 Accessory：不切 Regular 的话窗口不会到前台、也拿不到键盘焦点。
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

    // 标题跟着界面语言；窗口无边框时它主要出现在 Mission Control / 窗口列表里。
    let _ = window.set_title(crate::lang::text(crate::current_lang(app)).inbox_window_title);
    place_near_lamp(app, &window);
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
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
                restore_activation(app);
            }
            // 像弹层一样：点到别的应用就收起。没有这一条的话，收件箱会一直
            // 置顶挡在终端前面 —— 而它本来只是「看一眼、处理一条」的地方。
            tauri::WindowEvent::Focused(false) => {
                let _ = handle.hide();
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

fn place_near_lamp(app: &AppHandle, window: &WebviewWindow) {
    let Some(lamp) = app.get_webview_window("main") else {
        return;
    };
    let (Ok(position), Ok(size), Ok(inbox), Some(monitor)) = (
        lamp.outer_position(),
        lamp.outer_size(),
        window.outer_size(),
        lamp.current_monitor().ok().flatten(),
    ) else {
        // 拿不到几何就算了：窗口会出现在系统给它的默认位置。**不要**为了「有个位置」
        // 硬猜一个 —— 那可能把收件箱放到另一块屏上。
        return;
    };

    let area = monitor.work_area();
    let (x, y) = place(
        (position.x, position.y, size.width as i32, size.height as i32),
        (inbox.width as i32, inbox.height as i32),
        (
            area.position.x,
            area.position.y,
            area.size.width as i32,
            area.size.height as i32,
        ),
    );
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
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
}
