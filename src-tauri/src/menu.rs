//! 灯上右键弹出的那张菜单。
//!
//! 用**原生菜单**而不是 HTML：灯球窗口是按内容精确尺寸的（见 `apply_window_size`），
//! 一个 HTML 菜单画在窗口里必然被 `overflow: hidden` 裁掉。原生菜单由系统绘制，
//! 不受窗口边界约束，位置也由系统避开屏幕边缘。
//!
//! 菜单是运行时构造的（每组的内容要跟着当前状态变），而 `Menu` 自己没有事件回调
//! —— 只能挂在应用上按 id 分派，见 `crate::run` 里的 `on_menu_event`。

use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Manager};

/// 刚才右键点的是哪一组。
///
/// 组名可以是任意字符串（`ci`、`我的构建`），编进菜单项 id 再解析回来既要做转义
/// 又要防冲突。所以 id 只用固定的几个动作名，组名放在这里。
#[derive(Default)]
pub struct Target(pub Mutex<Option<String>>);

/// 菜单项的 id。**与托盘的 id 不重合** —— 两边的菜单事件都会走到
/// `AppHandle::on_menu_event`，重合了就会互相误触发。
const HIDE: &str = "group:hide";
const CLEAR: &str = "group:clear";
const TOGGLE: &str = "group:toggle";
const SETTINGS: &str = "group:settings";

/// 在灯球窗口上弹出菜单。`group` 是右键点中的那一组的名字。
pub fn show(app: &AppHandle, group: &str) -> Result<(), String> {
    *app.state::<Target>().0.lock().expect("menu target poisoned") = Some(group.to_string());

    let collapsed = crate::current_settings(app).collapsed;
    let hide = MenuItem::with_id(app, HIDE, "隐藏该组", true, None::<&str>).map_err(menu_err)?;
    let clear =
        MenuItem::with_id(app, CLEAR, "清除该组的状态", true, None::<&str>).map_err(menu_err)?;
    // 措辞跟着当前形态走：写「切换」的话得先看一眼灯才知道会切成什么
    let toggle = MenuItem::with_id(
        app,
        TOGGLE,
        if collapsed { "展开牌面" } else { "折叠成一颗灯" },
        true,
        None::<&str>,
    )
    .map_err(menu_err)?;
    let settings =
        MenuItem::with_id(app, SETTINGS, "打开设置…", true, None::<&str>).map_err(menu_err)?;

    let menu = Menu::with_items(app, &[&hide, &clear, &toggle, &settings]).map_err(menu_err)?;

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到灯球窗口".to_string())?;
    window
        .popup_menu(&menu)
        .map_err(|err| format!("弹出右键菜单失败: {err}"))
}

/// 分派一次菜单事件。返回「是不是我们这张菜单的」，好让调用方知道要不要管。
pub fn handle(app: &AppHandle, id: &str) -> bool {
    if !matches!(id, HIDE | CLEAR | TOGGLE | SETTINGS) {
        // 托盘菜单那些 id（quit / toggle / …）也走这条路，不是我们的就放行
        return false;
    }

    // 打开设置不需要知道是哪一组
    if id == SETTINGS {
        crate::reveal_settings(app);
        return true;
    }

    let target = app
        .state::<Target>()
        .0
        .lock()
        .expect("menu target poisoned")
        .clone();
    let Some(group) = target else {
        // 没记下组名说明这张菜单不是我们弹的，什么都不做比猜一个组安全
        return true;
    };

    let result = match id {
        HIDE => hide_group(app, &group),
        CLEAR => {
            clear_group(app, &group);
            Ok(())
        }
        TOGGLE => {
            let collapsed = !crate::current_settings(app).collapsed;
            crate::set_collapsed(app, collapsed).map(|_| ())
        }
        _ => unreachable!("上面已经过滤过 id 了"),
    };

    if let Err(err) = result {
        eprintln!("[signalhub] 右键菜单操作失败: {err}");
    }
    true
}

/// 把一组藏起来。藏起来的组不再占灯上的位置，但**仍然参与聚合、通知和钩子**
/// —— 藏起来是「别占我地方」，不是「当它不存在」。
fn hide_group(app: &AppHandle, group: &str) -> Result<(), String> {
    let settings = crate::current_settings(app);
    if !settings.shows_group(group) {
        return Ok(());
    }
    let mut hidden_groups = settings.hidden_groups.clone();
    hidden_groups.push(group.to_string());

    crate::persist_settings(
        app,
        crate::config::Settings { hidden_groups, ..settings },
    )
    .map(|_| ())
}

/// 清掉某一组的全部会话。
///
/// 这是「这个组的状态卡住了」的解药：上报方崩了或者没发收尾事件时，灯会一直
/// 停在最后一个状态上，等 TTL 到期要好几分钟。清除是立刻的。
fn clear_group(app: &AppHandle, group: &str) -> usize {
    let store = app.state::<crate::SharedStore>();
    let (removed, changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        let (removed, changed) = guard.clear_group(group);
        (removed, changed, guard.snapshot())
    };

    // 一个会话都没清掉就不推前端 —— 和 Store 各处「没变化不推」的规矩一致
    if changed {
        crate::notify::push(app, &snapshot);
    }
    removed
}

fn menu_err(err: tauri::Error) -> String {
    format!("构造右键菜单失败: {err}")
}
