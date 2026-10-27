//! 灯上右键弹出的那张菜单。
//!
//! 用**原生菜单**而不是 HTML：灯球窗口是按内容精确尺寸的（见 `apply_window_size`），
//! 一个 HTML 菜单画在窗口里必然被 `overflow: hidden` 裁掉。原生菜单由系统绘制，
//! 不受窗口边界约束，位置也由系统避开屏幕边缘。
//!
//! 菜单是运行时构造的（每组的内容要跟着当前状态变），而 `Menu` 自己没有事件回调
//! —— 只能挂在应用上按 id 分派，见 `crate::run` 里的 `on_menu_event`。

use std::sync::Mutex;
use tauri::menu::{IsMenuItem, Menu, MenuItem, Submenu};
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::state::Signal;

/// 刚才右键点的是哪一组。
///
/// 组名可以是任意字符串（`ci`、`我的构建`），编进菜单项 id 再解析回来既要做转义
/// 又要防冲突。所以 id 只用固定的几个动作名，组名放在这里。
#[derive(Default)]
pub struct Target(pub Mutex<Option<String>>);

/// 菜单项的 id。**与托盘的 id 不重合** —— 两边的菜单事件都会走到
/// `AppHandle::on_menu_event`，重合了就会互相误触发。
const HIDE: &str = "group:hide";
const MUTE: &str = "group:mute";
const CLEAR: &str = "group:clear";
const TOGGLE: &str = "group:toggle";
const SETTINGS: &str = "group:settings";
const OPEN_DIR: &str = "group:open-dir";
/// 「手动状态」子菜单里那些项的 id 前缀（后面接状态名，见 [`manual_state_for`]）。
/// 和上面几个固定 id 同一个命名空间（`group:`），但不会撞：那几个没有 `manual:`
/// 这一段。
const MANUAL_PREFIX: &str = "group:manual:";

/// 手动状态子菜单里给哪几个状态。**顺序就是菜单顺序**（从「在干活」到「空闲」）。
///
/// 只给可上报的状态，且不放 `permission`：那是「智能体在等你批准」的意思，
/// 手动把一盏灯画成红色闪烁会让人以为终端里真有个确认框在等。
const MANUAL_STATES: [Signal; 6] = [
    Signal::Working,
    Signal::Thinking,
    Signal::Done,
    Signal::Attention,
    Signal::Error,
    Signal::Idle,
];

/// 手动状态子菜单的 id → 状态。`None` = 不是这一项。
fn manual_state_for(id: &str) -> Option<Signal> {
    let name = id.strip_prefix(MANUAL_PREFIX)?;
    MANUAL_STATES
        .iter()
        .copied()
        .find(|state| state.name() == name)
}

/// 灯上**通用**菜单的 id（空白处和占位牌用的那张）。同样与托盘、与组菜单都不
/// 重合：三边的菜单事件都会走到 `AppHandle::on_menu_event`。
const LAMP_SETTINGS: &str = "lamp:settings";
const LAMP_PAUSE: &str = "lamp:pause";
const LAMP_COLLAPSED: &str = "lamp:collapsed";
const LAMP_QUIT: &str = "lamp:quit";

/// 在灯球窗口上弹出菜单。`group` 是右键点中的那一组的名字。
///
/// `window` 是**发起这次右键的那个灯窗** —— 多灯模式下每个组灯都会调这里，菜单必须
/// 弹在用户点的那盏灯上，而不是永远弹在 `main` 上（那样点组灯会弹出别处的菜单）。
pub fn show(app: &AppHandle, window: &WebviewWindow, group: &str) -> Result<(), String> {
    *app.state::<Target>().0.lock().expect("menu target poisoned") = Some(group.to_string());

    let settings = crate::current_settings(app);
    let muted = settings.muted_groups.iter().any(|name| name == group);
    let t = crate::lang::text(crate::current_lang(app));

    // 「打开项目目录」——目录取该组 winner 会话的 `cwd`。没有（或已经不是目录）就
    // 置灰而不是藏起来：藏起来的话用户不知道这个能力存在，也不知道为什么没有。
    let project_dir = crate::group_project_dir(app, group);
    let open_dir = MenuItem::with_id(
        app,
        OPEN_DIR,
        t.action_reveal,
        project_dir.is_some(),
        None::<&str>,
    )
    .map_err(menu_err)?;
    let hide = MenuItem::with_id(app, HIDE, t.menu_hide, true, None::<&str>).map_err(menu_err)?;
    // 措辞跟着当前状态走：写「切换静音」的话得先看一眼设置才知道会切成什么
    let mute = MenuItem::with_id(
        app,
        MUTE,
        if muted { t.menu_unmute } else { t.menu_mute },
        true,
        None::<&str>,
    )
    .map_err(menu_err)?;
    let clear =
        MenuItem::with_id(app, CLEAR, t.menu_clear, true, None::<&str>).map_err(menu_err)?;
    // 「手动状态」子菜单（issue #1）：灯不只是展示 —— 点哪盏灯，就能把那盏灯设成
    // 自己要的状态。它不是第二套状态机：走 `manual::set_group` 的普通上报路径，
    // 收掉用下面那个「清除该组的状态」。
    let lang = crate::current_lang(app);
    let manual_items = MANUAL_STATES
        .iter()
        .map(|state| {
            MenuItem::with_id(
                app,
                format!("{MANUAL_PREFIX}{}", state.name()),
                crate::lang::signal_label(lang, *state),
                true,
                None::<&str>,
            )
            .map_err(menu_err)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let manual_refs: Vec<&dyn IsMenuItem<tauri::Wry>> = manual_items
        .iter()
        .map(|item| item as &dyn IsMenuItem<tauri::Wry>)
        .collect();
    let manual = Submenu::with_items(app, t.manual_title, true, &manual_refs).map_err(menu_err)?;
    // 措辞跟着当前形态走：写「切换」的话得先看一眼灯才知道会切成什么
    let toggle = MenuItem::with_id(
        app,
        TOGGLE,
        if settings.collapsed { t.menu_expand } else { t.menu_collapse },
        true,
        None::<&str>,
    )
    .map_err(menu_err)?;
    let open_settings =
        MenuItem::with_id(app, SETTINGS, t.menu_settings, true, None::<&str>).map_err(menu_err)?;

    let items: [&dyn IsMenuItem<tauri::Wry>; 7] = [
        &open_dir,
        &manual,
        &hide,
        &mute,
        &clear,
        &toggle,
        &open_settings,
    ];
    let menu = Menu::with_items(app, &items).map_err(menu_err)?;

    popup(window, &menu)
}

/// 灯上空白处（以及还没接任何上报方时那块占位牌）的右键菜单。
///
/// 这一张不是「某一组的操作」，而是**灯自己的入口**。存在的理由很具体：第一次
/// 装好、还没接任何上报方时，灯上那块牌的名字是空的，它对不上任何真实的组 ——
/// 于是 `open_group_menu` 那条路走不通，灯上完全没有进入设置的地方，用户只能
/// 去托盘里找。
pub fn show_general(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let t = crate::lang::text(crate::current_lang(app));
    let collapsed = crate::current_settings(app).collapsed;

    let open_settings =
        MenuItem::with_id(app, LAMP_SETTINGS, t.menu_settings, true, None::<&str>).map_err(menu_err)?;
    // 措辞跟托盘一致、**不显示当前开没开**：快捷键也能切换暂停，菜单里那份状态
    // 会漂移，不如从一开始就不声称它知道
    let pause = MenuItem::with_id(app, LAMP_PAUSE, t.tray_pause, true, None::<&str>).map_err(menu_err)?;
    let collapse = MenuItem::with_id(
        app,
        LAMP_COLLAPSED,
        if collapsed { t.menu_expand } else { t.menu_collapse },
        true,
        None::<&str>,
    )
    .map_err(menu_err)?;
    let quit = MenuItem::with_id(app, LAMP_QUIT, t.tray_quit, true, None::<&str>).map_err(menu_err)?;

    let menu = Menu::with_items(app, &[&open_settings, &pause, &collapse, &quit]).map_err(menu_err)?;

    popup(window, &menu)
}

/// 把菜单弹在**发起的那个灯窗**上。两张菜单共用，免得错误处理写两遍。
fn popup(window: &WebviewWindow, menu: &Menu<tauri::Wry>) -> Result<(), String> {
    window
        .popup_menu(menu)
        .map_err(|err| format!("弹出右键菜单失败: {err}"))
}

/// 分派一次菜单事件。返回「是不是我们这张菜单的」，好让调用方知道要不要管。
pub fn handle(app: &AppHandle, id: &str) -> bool {
    // 灯自己那张菜单（空白处 / 占位牌）。它不与任何组绑定，所以先判、先返回。
    if matches!(id, LAMP_SETTINGS | LAMP_PAUSE | LAMP_COLLAPSED | LAMP_QUIT) {
        let result = match id {
            LAMP_SETTINGS => {
                crate::reveal_settings(app);
                Ok(())
            }
            LAMP_PAUSE => {
                crate::tray::toggle_monitor(app);
                Ok(())
            }
            LAMP_COLLAPSED => {
                let collapsed = !crate::current_settings(app).collapsed;
                crate::set_collapsed(app, collapsed).map(|_| ())
            }
            LAMP_QUIT => {
                app.exit(0);
                Ok(())
            }
            _ => unreachable!("上面已经过滤过 id 了"),
        };
        if let Err(err) = result {
            eprintln!("[signalhub] 灯上菜单操作失败: {err}");
        }
        return true;
    }

    if !matches!(id, OPEN_DIR | HIDE | MUTE | CLEAR | TOGGLE | SETTINGS)
        && manual_state_for(id).is_none()
    {
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

    // 手动设状态：用户点的是哪一盏，就设哪一组（见 `manual::set_group`）
    if let Some(state) = manual_state_for(id) {
        crate::manual::set_group(app, &group, state);
        return true;
    }

    let result = match id {
        OPEN_DIR => open_project_dir(app, &group),
        HIDE => hide_group(app, &group),
        MUTE => {
            let muted = crate::current_settings(app)
                .muted_groups
                .iter()
                .any(|name| name == &group);
            // 通知上的「静音该组」按钮走的是同一个函数 —— 两条入口各写一遍
            // 行为迟早分叉
            crate::set_group_muted(app, &group, !muted).map(|_| ())
        }
        CLEAR => {
            crate::clear_group(app, &group);
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

/// 在文件管理器里打开这一组的项目目录。
///
/// 目录**在点下去这一刻**再取一次（不把弹菜单时的值传过来）：菜单弹出来到点下去
/// 之间状态可能已经变了。目录没了时 `reveal_dir` 自己会静默跳过。
fn open_project_dir(app: &AppHandle, group: &str) -> Result<(), String> {
    match crate::group_project_dir(app, group) {
        Some(dir) => {
            crate::open::reveal_dir(&dir).map_err(|err| format!("打开项目目录失败: {err}"))
        }
        None => Ok(()),
    }
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
        crate::config::Settings { hidden_groups, ..(*settings).clone() },
    )
    .map(|_| ())
}

fn menu_err(err: tauri::Error) -> String {
    format!("构造右键菜单失败: {err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 子菜单的每一项都必须映射回它自己的状态 —— 一个漏掉的 id 表现为「点了没反应」
    /// 而且不报错（和托盘手动状态那边同一个坑，见 `manual::action_for`）。
    #[test]
    fn every_manual_item_maps_to_its_own_state() {
        let mut seen = Vec::new();
        for state in MANUAL_STATES {
            let id = format!("{MANUAL_PREFIX}{}", state.name());
            assert_eq!(manual_state_for(&id), Some(state), "{id} 映射错了");
            assert!(!seen.contains(&id), "{id} 和另一项撞了");
            seen.push(id);
        }
        // 六个状态互不相同（不然菜单里会出现两个点了同样的项）
        let mut names: Vec<&str> = MANUAL_STATES.iter().map(|state| state.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }

    /// 别的菜单 id（组菜单的、托盘的）不能被这里认领 —— 认错了就会在点「清除该组
    /// 的状态」时顺手把状态改掉。
    #[test]
    fn other_menu_ids_are_not_manual_items() {
        for id in ["group:clear", "group:mute", "manual_clear", "group:manual:"] {
            assert_eq!(manual_state_for(id), None, "{id} 被当成手动状态了");
        }
    }

    /// `permission` 刻意不在子菜单里：手动画成「等你批准」会让人以为终端里真有个
    /// 确认框在等。
    #[test]
    fn permission_is_not_offered_manually() {
        let id = format!("{MANUAL_PREFIX}permission");
        assert_eq!(manual_state_for(&id), None);
    }
}
