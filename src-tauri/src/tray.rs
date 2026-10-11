use crate::notify;
use crate::state::Store;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

/// 托盘是这台没有 Dock 图标、窗口又不可关闭的应用**唯一**的退出入口，
/// 所以它在 Phase 1 就必须存在，不属于后续打磨项。
pub fn init(app: AppHandle) -> tauri::Result<()> {
    // 这些构造器都要求传 &M（M: Manager），所以统一用 &app
    let toggle = MenuItem::with_id(&app, "toggle", "显示 / 隐藏灯球", true, None::<&str>)?;
    let settings = MenuItem::with_id(&app, "settings", "设置…", true, None::<&str>)?;
    let pause = MenuItem::with_id(&app, "pause", "暂停 / 恢复监控", true, None::<&str>)?;
    // 故意不显示「开 / 关」状态：快捷键也能切换它，菜单里的状态会漂移。
    // 准确状态在设置面板里（get_toggles）。
    let pass_through = MenuItem::with_id(&app, "pass_through", "切换点击穿透", true, None::<&str>)?;
    let quit = MenuItem::with_id(&app, "quit", "退出 SignalHub", true, None::<&str>)?;
    let menu = Menu::with_items(
        &app,
        &[&toggle, &settings, &pause, &pass_through, &quit],
    )?;

    TrayIconBuilder::with_id("signalhub")
        .icon(app.default_window_icon().expect("缺少应用图标").clone())
        .tooltip("SignalHub")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "toggle" => toggle_lamp(app),
            "settings" => crate::reveal_settings(app),
            "pause" => toggle_monitor(app),
            "pass_through" => {
                let enabled = app
                    .state::<crate::SharedUi>()
                    .lock()
                    .expect("ui poisoned")
                    .pass_through;
                if let Err(err) = crate::set_pass_through_impl(app, !enabled) {
                    eprintln!("[signalhub] 托盘切换点击穿透失败: {err}");
                }
            }
            _ => {}
        })
        .build(&app)?;

    Ok(())
}

fn toggle_lamp(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn toggle_monitor(app: &AppHandle) {
    let store = app.state::<Arc<Mutex<Store>>>();
    let (changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        // 暂停状态不在 Store 的公开读接口里，从快照取当前值再取反
        let next = !guard.snapshot().paused;
        let changed = guard.set_paused(next);
        (changed, guard.snapshot())
    };
    if changed {
        notify::push(app, &snapshot);
    }
}
