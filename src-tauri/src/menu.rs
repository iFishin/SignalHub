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
/// 组菜单里那一份「全部归位」。**id 与通用菜单的 `LAMP_RESET` 分开**：muda 按 id
/// 维护一张全局表，两张菜单同时建同 id 的项会互相覆盖。行为不分叉靠的是两条
/// 分支都调同一个 `reset_lamp_positions_impl`，不是靠复用 id。
const RESET: &str = "group:reset-positions";
/// 「重命名…」：**不开输入框**，只是把设置窗翻到「分组」页（那一页有显示名的输入框）。
///
/// 牌上的就地改名在 0.6.2 之后被撤掉了：灯是常驻桌面的，点一下名字就进入编辑状态，
/// 误触的代价（改到别人的名字、或者干脆没发现自己在编辑）比便利大。改名本身没
/// 丢，只是搬到了设置里 —— 于是这里要留一条「知道它搬哪儿去了」的路，否则用户
/// 会以为功能没了。
const RENAME: &str = "group:rename";
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

/// 灯上**通用**菜单的 id（空白处和占位牌用的那张）。同样与托盘不重合：三边的
/// 菜单事件都会走到 `AppHandle::on_menu_event`。
const LAMP_SETTINGS: &str = "lamp:settings";
const LAMP_PAUSE: &str = "lamp:pause";
const LAMP_COLLAPSED: &str = "lamp:collapsed";
/// 通用菜单里那份「全部归位」（组菜单那份是 `RESET`，见它的注释）。
///
/// **为什么两张菜单都放**：归位不是「对这一组」的操作，可组菜单本来就有
/// `TOGGLE`（折叠是全局的）这类全局项 —— 更要紧的是多灯模式下用户右键点的
/// **就是灯珠 / 牌面**，通用菜单只在四周留白才弹得出来，只放通用菜单等于把
/// 最顺手的入口藏了。两处都调 `reset_lamp_positions_impl`，行为不分叉。
///
/// 两处都只在**真的开着组灯**时插入。单灯模式没有可归位的灯，摆一个点了没反应
/// 的项只会让人以为坏了；判据取当前窗口而不是 `lamp_layout` 设置 —— 超过
/// `MAX_LAMPS` 时多灯会回落单灯，那时设置里仍写着「每组分灯」，窗口才是真值。
const LAMP_RESET: &str = "lamp:reset-positions";
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
        t.menu_open_dir,
        project_dir.is_some(),
        None::<&str>,
    )
    .map_err(menu_err)?;
    let rename =
        MenuItem::with_id(app, RENAME, t.menu_rename, true, None::<&str>).map_err(menu_err)?;
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
    // 只是那条会话会在**本组牌面**上压过真实智能体会话（否则组里有个 working /
    // permission 的会话时点了看不出变化）。收掉用下面那个「清除该组的状态」。
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
    let reset = MenuItem::with_id(app, RESET, t.menu_reset_lamps, true, None::<&str>)
        .map_err(menu_err)?;

    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &open_dir,
        &rename,
        &manual,
        &hide,
        &mute,
        &clear,
        &toggle,
        &open_settings,
    ];
    // 和通用菜单同一个判据（`has_group_lamps`）：只问「现在有没有组灯」，不问
    // 设置里写的是什么布局（超过 MAX_LAMPS 会回落单灯，那时设置仍是「每组分灯」
    // 却无灯可归）。id 用 `RESET` 而不是 `LAMP_RESET`，理由见那个常量的注释。
    if has_group_lamps(app) {
        // 挨着「折叠」放：两项都在管「灯怎么摆」
        items.insert(6, &reset);
    }
    let menu = Menu::with_items(app, &items).map_err(menu_err)?;

    popup(window, &menu)
}

/// 当前是否真有组灯窗口开着。归位是对**这些窗口**的操作，所以窗口本身才是真值，
/// 而不是 `Settings::lamp_layout` 或「这一帧有没有超过上限」的推算。
fn has_group_lamps(app: &AppHandle) -> bool {
    app.webview_windows()
        .keys()
        .any(|label| label.starts_with(crate::lamps::LABEL_PREFIX))
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
    // 和「折叠」挨着放：两项都是「灯怎么摆」，一件事在菜单上不该被别的操作隔开。
    let reset = MenuItem::with_id(app, LAMP_RESET, t.menu_reset_lamps, true, None::<&str>)
        .map_err(menu_err)?;
    // 菜单项是运行时构造的，所以这里能按真值决定插不插。数组长度随形态变，
    // 用 Vec 而不是写死长度的字面量 —— 写死就得为「有归位 / 没归位」维护两份。
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> =
        vec![&open_settings, &pause, &collapse];
    if has_group_lamps(app) {
        items.push(&reset);
    }
    items.push(&quit);
    let menu = Menu::with_items(app, &items).map_err(menu_err)?;

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
    // 这一组动作都不需要知道是哪一组，所以先判、先返回，全部来自灯上通用菜单
    // （空白处 / 占位牌）。组菜单那份归位用 `RESET`，在下面第二段里同样先判。
    if matches!(
        id,
        LAMP_SETTINGS | LAMP_PAUSE | LAMP_COLLAPSED | LAMP_RESET | LAMP_QUIT
    ) {
        // 「全部归位」没有失败路径：拿不到显示器信息就一盏不动，返回 0。
        // 单独摘出来，免得为了塞进 `Result<(), String>` 去编一个错误文案。
        if id == LAMP_RESET {
            crate::reset_lamp_positions_impl(app);
            return true;
        }
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

    if !matches!(id, OPEN_DIR | RENAME | HIDE | MUTE | CLEAR | TOGGLE | SETTINGS | RESET)
        && manual_state_for(id).is_none()
    {
        // 托盘菜单那些 id（quit / toggle / …）也走这条路，不是我们的就放行
        return false;
    }

    // 这几项都不需要知道是哪一组。**归位尤其要先判**：组菜单是在记录了 target
    // 之后弹的，但归位动的是所有组灯，和右键点中哪一组无关。
    if id == SETTINGS {
        crate::reveal_settings(app);
        return true;
    }
    if id == RESET {
        crate::reset_lamp_positions_impl(app);
        return true;
    }
    // 「重命名…」直接翻到「分组」页 —— 那一页的「各组的显示」里就是显示名输入框。
    // 不带上组名：翻到那一页、列表按名字排着，比自动滚到某一行更容易理解
    // （而且 Tauri 的原生菜单没法可靠地滚到某一行，做到一半反而是骗人）。
    if id == RENAME {
        crate::reveal_settings_tab(app, "groups");
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

    /// 三张菜单的事件都汇到同一个 `on_menu_event`，id 撞了就会互相误触发。
    /// 「全部归位」有两份（通用菜单 + 组菜单），**必须各用各的 id**：muda 按 id
    /// 维护全局表，同 id 会互相覆盖；而两处都得能被自己的分支认领。
    #[test]
    fn lamp_menu_ids_do_not_collide_with_each_other_or_other_menus() {
        let lamp = [LAMP_SETTINGS, LAMP_PAUSE, LAMP_COLLAPSED, LAMP_RESET, LAMP_QUIT];
        let mut seen = Vec::new();
        for id in lamp {
            assert!(!seen.contains(&id), "{id} 和灯上另一项撞了");
            assert!(!id.starts_with("group:"), "{id} 落进了组菜单的命名空间");
            assert!(manual_state_for(id).is_none(), "{id} 被当成手动状态了");
            seen.push(id);
        }
        assert_eq!(RESET, "group:reset-positions");
        assert_ne!(RESET, LAMP_RESET, "两张菜单的归位项不能共用一个 id");
        assert!(!seen.contains(&RESET), "组菜单的归位项和通用菜单撞了");
        assert!(manual_state_for(RESET).is_none());
        // 组菜单其它固定 id 也不能和它撞
        for other in [HIDE, MUTE, CLEAR, TOGGLE, SETTINGS, OPEN_DIR, RENAME] {
            assert_ne!(RESET, other, "归位项和组菜单的 {other} 撞了");
        }
        // 托盘那几个 id 都是不带前缀的裸词
        for tray in ["inbox", "toggle", "settings", "pause", "pass_through", "collapsed", "quit"] {
            assert!(!seen.contains(&tray), "托盘的 {tray} 和灯上菜单撞了");
        }
    }
}
