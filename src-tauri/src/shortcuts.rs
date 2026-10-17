//! 全局快捷键。
//!
//! 两个来源合在一起：**点击穿透的键盘出路**（唯一一个非有不可的 —— 穿透之后鼠标
//! 点不到灯了，没有它就是把自己锁在外面）和三个对着「当前最差那一组」的动作。
//!
//! **快捷键是固定的，不做可配。** 可配需要一套按键录制器、一套校验（重复、保留键、
//! 平台差异）、一份持久化和每次改动重新注册；而它解决的问题（「想换成别的组合」）
//! 在真实故障里几乎不出现 —— 真实故障是**被别的程序占了**，那件事可配也解决不了
//! （用户得先去关掉那个程序）。所以这里把力气花在**把状态说出来**：注册失败会被
//! 记下来，设置 →「系统」页明说哪一个没抢到，而不是静默失效。
//!
//! 三个动作都作用在**最差的那一组**上（紧急度最高的那一组，和灯上最上面那块牌
//! 同一个判据）。理由是这些动作的语义本来就是「处理最要紧的那件事」：全局静音
//! 已经有一个更好的入口（托盘的「暂停 / 恢复监控」），而「静音一切」在开会时是
//! 对的动作、平时是错的动作。

use crate::lang;
use crate::state::Snapshot;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

// ---- 四个组合 ---------------------------------------------------------------
//
// 常量是**唯一真值**：文案（`shortcut_label`）和注册（`register`）都从它派生。
// 这里曾经有过一份独立的 label 常量，结果和真正注册的组合漂移了 —— 而且漂移了
// 也没人会发现，因为文案只出现在提示和日志里。派生出来就不存在「两份真值」。
//
// 两条挑选规则（`no_shortcut_steals_a_plain_keystroke` 钉着）：
//
// 1. **至少两个修饰键，而且带一个主修饰键**（⌘ / Ctrl）。裸的 ⌘S 是全世界的
//    「保存」—— 全局注册等于把它从所有其它应用手里抢走，而这里曾经真的这么注册过。
// 2. **不要「⌥⇧+字母 / 数字」这种能打出字符的组合。** 在 macOS 上 ⌥⇧L 打出来的是
//    Ò、⌥⇧0 是 º；全局注册之后这些字符**在所有应用里都打不出来了** —— 而这是
//    看不见的那种坏（不像「撞车了」会在这里被明说）。计划里写的是 ⌥⇧，这里是
//    **有意的改动**：换成 ⌃⌥ 就没有这个问题，而且它同样不是任何应用的常用组合。
//
// Windows / Linux 上走 Ctrl+Alt（和已有的穿透快捷键同一套），代价是欧洲键盘布局
// 的 AltGr = Ctrl+Alt，会抢掉几个 AltGr 字符 —— 这条取舍穿透那个键已经付过一次了。

/// 点击穿透的出路。**唯一一个非有不可的**：穿透之后鼠标点不到灯了。
#[cfg(target_os = "macos")]
pub const PASS_THROUGH: (Modifiers, Code) =
    (Modifiers::SUPER.union(Modifiers::ALT), Code::KeyS);
#[cfg(not(target_os = "macos"))]
pub const PASS_THROUGH: (Modifiers, Code) =
    (Modifiers::CONTROL.union(Modifiers::ALT), Code::KeyS);

// 这三个在**两个平台上都是 Ctrl+Alt**（macOS 上是 ⌃⌥），所以没有 `cfg` 分叉 ——
// 上面那两条规则算下来，⌃⌥ 在两边的名字恰好一样，多一个分叉只会让人以为它们不同。
// 只有穿透那个要分（macOS 上用 ⌥⌘S，因为那是这个平台的习惯写法）。

/// 显示 / 隐藏灯。和托盘的「显示 / 隐藏灯球」是同一个动作。
const TOGGLE_LAMP: (Modifiers, Code) = (Modifiers::CONTROL.union(Modifiers::ALT), Code::KeyL);

/// 静音 / 取消静音最差的那一组。
const MUTE_WORST: (Modifiers, Code) = (Modifiers::CONTROL.union(Modifiers::ALT), Code::KeyM);

/// 清除最差那一组的状态（和牌上的右键「清除该组的状态」同一个动作）。
const CLEAR_WORST: (Modifiers, Code) = (Modifiers::CONTROL.union(Modifiers::ALT), Code::Digit0);

// ---- 人话写法 ---------------------------------------------------------------

/// 快捷键写法的两种平台习惯。
///
/// **做成显式参数、而不是在函数体里直接 `cfg!`**，是为了让两套写法在**任何平台
/// 上都能被单测走到**。起因是一次真实的漏网：`efc8f16` 把 macOS 的修饰键顺序照抄
/// 进了 Windows / Linux 那一支，写出来的标签是 `Alt+Super+S`（哪个平台都不这么写），
/// 而本地是 macOS、`cfg!` 永远走不到那一支 —— CI 上 Linux 连红三次才被发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyStyle {
    /// macOS：按 Apple HIG 的 ⌃⌥⇧⌘，Command 紧贴键名。
    Mac,
    /// Windows / Linux：`Ctrl+Alt+S` / `Super+Alt+S`。
    Other,
}

fn key_style() -> KeyStyle {
    if cfg!(target_os = "macos") {
        KeyStyle::Mac
    } else {
        KeyStyle::Other
    }
}

/// 快捷键的人话写法：macOS 上是「⌥⌘S」，别处是「Super+Alt+S」。
fn shortcut_label(mods: Modifiers, code: Code) -> String {
    // `Code` 的 `Display` 给的是 W3C 名字（`KeyS`、`Digit1`），键盘上印的不是这个。
    let w3c = code.to_string();
    let key = w3c
        .strip_prefix("Key")
        .or_else(|| w3c.strip_prefix("Digit"))
        .or_else(|| w3c.strip_prefix("Numpad"))
        .unwrap_or(&w3c);

    shortcut_label_for(mods, key, key_style())
}

/// 指定写法的版本。单测用它把**两套**都跑一遍，不受当前平台限制。
fn shortcut_label_for(mods: Modifiers, key: &str, style: KeyStyle) -> String {
    match style {
        KeyStyle::Mac => {
            let mut out = String::new();
            for (flag, glyph) in [
                (Modifiers::CONTROL, '⌃'),
                (Modifiers::ALT, '⌥'),
                (Modifiers::SHIFT, '⇧'),
                (Modifiers::SUPER, '⌘'),
            ] {
                if mods.contains(flag) {
                    out.push(glyph);
                }
            }
            out.push_str(key);
            out
        }
        KeyStyle::Other => {
            let mut parts: Vec<&str> = Vec::new();
            for (flag, name) in [
                (Modifiers::SUPER, "Super"),
                (Modifiers::CONTROL, "Ctrl"),
                (Modifiers::ALT, "Alt"),
                (Modifiers::SHIFT, "Shift"),
            ] {
                if mods.contains(flag) {
                    parts.push(name);
                }
            }
            parts.push(key);
            parts.join("+")
        }
    }
}

/// 穿透快捷键的写法。点击穿透的说明要用它（「按 ⌥⌘S 退出」）。
pub fn pass_through_label() -> String {
    shortcut_label(PASS_THROUGH.0, PASS_THROUGH.1)
}

// ---- 注册 -------------------------------------------------------------------

/// 一条快捷键现在的处境。
///
/// **要送到界面上**：抢不到一个组合时，用户看到的必须是「这一个没生效」，而不是
/// 一个按了没反应的键。这是这一节存在的全部理由。
#[derive(Debug, Clone, Serialize)]
pub struct ShortcutStatus {
    /// 哪个动作（`lamp` / `mute` / `clear` / `pass_through`）。界面按它取本地化的名字。
    pub action: &'static str,
    /// 人话写法（`⌥⇧L`）—— 从注册的那份派生，不另立常量。
    pub label: String,
    /// 抢到了没有。false 时界面会明说（多半是被别的程序占了）。
    pub ok: bool,
}

/// 四个动作的处境，注册时算一次，之后不再变。
pub type SharedShortcuts = Arc<Mutex<Vec<ShortcutStatus>>>;

/// 注册全部四个。**不阻断启动**：抢不到一个快捷键不该让 app 起不来（穿透那个
/// 抢不到也还有托盘菜单能退出）。
pub fn register_all(app: &AppHandle) -> Vec<ShortcutStatus> {
    vec![
        register(app, "lamp", TOGGLE_LAMP, |app| {
            crate::tray::toggle_lamp(app)
        }),
        register(app, "mute", MUTE_WORST, mute_worst),
        register(app, "clear", CLEAR_WORST, clear_worst),
        register(app, "pass_through", PASS_THROUGH, toggle_pass_through),
    ]
}

/// 注册一条并记下结果。
fn register(
    app: &AppHandle,
    action: &'static str,
    (mods, code): (Modifiers, Code),
    run: impl Fn(&AppHandle) + Send + Sync + 'static,
) -> ShortcutStatus {
    let shortcut = Shortcut::new(Some(mods), code);
    let label = shortcut_label(mods, code);

    let ok = match app
        .global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            // 只认按下，否则一次按键会来回切换两次（按下 + 抬起都会派发）
            if event.state != ShortcutState::Pressed {
                return;
            }
            run(app);
        }) {
        Ok(()) => true,
        Err(err) => {
            // 失败只记日志 + 记进状态，不弹东西：这多半是「别的程序占着这个组合」，
            // 而用户此刻可能正在启动过程中。
            eprintln!("[signalhub] 注册全局快捷键 {label} 失败（{err}）");
            false
        }
    };

    ShortcutStatus { action, label, ok }
}

// ---- 三个动作 ---------------------------------------------------------------

/// 「最差的那一组」= 紧急度最高的那一组。
///
/// `snapshot.groups` 本身已经按（紧急度降序、组名升序）排好（见 `Store::snapshot_at`），
/// 所以第一个就是它。**置顶只改显示顺序，不改这里的判据**：这些动作要处理的是
/// 「最要紧的那件事」，而不是「我摆在最上面的那块牌」。
///
/// 抽成纯函数是为了能被测到 —— 不然这条规则只能靠按一次快捷键、看看哪块牌没了来
/// 验证（而选错组的表现是「清掉了一个本来没事的组」，很难注意到）。
fn worst_of(snapshot: &Snapshot) -> Option<String> {
    snapshot.groups.first().map(|group| group.name.clone())
}

fn worst_group(app: &AppHandle) -> Option<String> {
    worst_of(&crate::current_snapshot(app))
}

/// 静音 / 取消静音最差的那一组。
///
/// 和右键菜单、通知上的「静音该组」改的是同一份设置（`muted_groups`），所以三个
/// 入口的表现必然一致。**发一条通告**：静音只影响通知和提示音，灯照常显示那一组
/// —— 也就是说这个动作在屏幕上是**看不见**的，而一个按下去什么都没发生的快捷键
/// 用户只会再按几次。
fn mute_worst(app: &AppHandle) {
    let Some(group) = worst_group(app) else {
        return;
    };
    let settings = crate::current_settings(app);
    let (next_muted, muted) = toggled_muted(&settings.muted_groups, &group);

    let mut next = (*settings).clone();
    next.muted_groups = next_muted;
    if let Err(err) = crate::persist_settings(app, next) {
        eprintln!("[signalhub] 快捷键静音 {group} 失败: {err}");
        return;
    }

    let lang = crate::current_lang(app);
    let (title, body) = lang::mute_message(lang, &group, muted);
    crate::alerts::send_message(app, &title, &body);
}

/// 按一次之后的 `muted_groups`，以及「现在是静音还是取消静音」。
///
/// 纯函数，好让「再按一次恢复」这条语义能被测到 —— 它是这个动作里唯一不是
/// 「搬运」的一步（其余几步都是既有的 `persist_settings` / `send_message`）。
/// 去重的责任在 `Settings::clamped`（`normalize_groups`），这里只保证不重复加。
fn toggled_muted(current: &[String], group: &str) -> (Vec<String>, bool) {
    if current.iter().any(|name| name == group) {
        (
            current
                .iter()
                .filter(|name| name.as_str() != group)
                .cloned()
                .collect(),
            false,
        )
    } else {
        let mut next = current.to_vec();
        next.push(group.to_string());
        (next, true)
    }
}

/// 清除最差那一组的状态。
///
/// 走的是和右键菜单、设置面板那个 `×` 同一个 `clear_group`。**不发通告**：这一个
/// 是**看得见**的（那块牌立刻没了 / 退回「没动静」），再弹一条通知就是多此一举。
fn clear_worst(app: &AppHandle) {
    if let Some(group) = worst_group(app) {
        crate::clear_group(app, &group);
    }
}

/// 切换点击穿透。和托盘那一项、设置里的开关走同一条路（`set_pass_through_impl`
/// 里会发一条「怎么退出」的通告）。
fn toggle_pass_through(app: &AppHandle) {
    let enabled = app
        .state::<crate::SharedUi>()
        .lock()
        .expect("ui poisoned")
        .pass_through;
    if let Err(err) = crate::set_pass_through_impl(app, !enabled) {
        eprintln!("[signalhub] 快捷键切换点击穿透失败: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Report, Signal, Store};
    use std::time::{Duration, Instant};

    /// 每一组都必须**带一个主修饰键**（⌘ / Ctrl）而且**至少两个修饰键**。
    ///
    /// 两条规则各挡一类事故：
    ///
    /// - 缺主修饰键（⌥⇧L）→ macOS 上那是「打一个字符」（Ò），全局注册之后那个字符
    ///   在所有应用里都打不出来了。而这是**看不见**的坏。
    /// - 只有一个修饰键（⌘S）→ 那是全世界的「保存」，全局注册等于把它抢走。这里
    ///   曾经真的这么注册过穿透。
    #[test]
    fn no_shortcut_steals_a_plain_keystroke() {
        for (name, (mods, _)) in [
            ("pass_through", PASS_THROUGH),
            ("lamp", TOGGLE_LAMP),
            ("mute", MUTE_WORST),
            ("clear", CLEAR_WORST),
        ] {
            assert!(
                mods.contains(Modifiers::CONTROL) || mods.contains(Modifiers::SUPER),
                "{name} 缺一个主修饰键（⌥⇧+字母在 macOS 上是打字符）"
            );
            let count = [
                Modifiers::CONTROL,
                Modifiers::ALT,
                Modifiers::SHIFT,
                Modifiers::SUPER,
            ]
            .iter()
            .filter(|flag| mods.contains(**flag))
            .count();
            assert!(count >= 2, "{name} 只有一个修饰键，会和常用快捷键撞车");
        }
    }

    /// 四个组合互不相同。相同的话第二个注册会失败 —— 那会表现成「有一个动作永远
    /// 不生效」，而这是最容易在改快捷键时犯的错。
    #[test]
    fn the_four_shortcuts_are_distinct() {
        let mut seen: Vec<(Modifiers, Code)> = Vec::new();
        for spec in [PASS_THROUGH, TOGGLE_LAMP, MUTE_WORST, CLEAR_WORST] {
            assert!(!seen.contains(&spec), "{spec:?} 和另一个重复了");
            seen.push(spec);
        }
    }

    /// 文案必须真的描述那个注册的组合 —— 这条测试就是那个「会发现的东西」。
    #[test]
    fn every_label_matches_its_registered_shortcut() {
        for (mods, code) in [PASS_THROUGH, TOGGLE_LAMP, MUTE_WORST, CLEAR_WORST] {
            let label = shortcut_label(mods, code);
            assert_eq!(
                mods.contains(Modifiers::ALT),
                label.contains('⌥') || label.contains("Alt"),
                "{label} 与注册的修饰键对不上"
            );
            assert_eq!(
                mods.contains(Modifiers::SHIFT),
                label.contains('⇧') || label.contains("Shift"),
                "{label} 与注册的修饰键对不上"
            );
            assert!(
                !label.contains("Key") && !label.contains("Digit"),
                "{label} 里还留着 W3C 前缀"
            );
        }
    }

    /// 格式化函数本身：W3C 名字要去掉前缀，修饰键要按平台习惯写。
    ///
    /// **两套写法都断言，不管跑在哪个平台。** 用 `shortcut_label_for` 显式指定
    /// 写法，而不是靠 `cfg!` 只走到当前平台那一支 —— 这个测试原来正是那么写的，
    /// 于是「Windows / Linux 那一支照抄了 macOS 的顺序」在 macOS 开发机上永远
    /// 测不出来，CI 上 Linux 连红三次。
    #[test]
    fn shortcut_labels_read_like_keyboards() {
        let word = Modifiers::CONTROL.union(Modifiers::ALT);
        let command = Modifiers::SUPER.union(Modifiers::ALT);

        // macOS：Apple HIG 的 ⌃⌥⇧⌘，Command 紧贴键名
        assert_eq!(shortcut_label_for(word, "S", KeyStyle::Mac), "⌃⌥S");
        assert_eq!(shortcut_label_for(command, "S", KeyStyle::Mac), "⌥⌘S");

        // Windows / Linux：Super 打头，别把上面那套照抄下来
        assert_eq!(shortcut_label_for(word, "S", KeyStyle::Other), "Ctrl+Alt+S");
        assert_eq!(
            shortcut_label_for(command, "S", KeyStyle::Other),
            "Super+Alt+S"
        );

        // 数字键同样要去掉 `Digit` 前缀，否则会写出「Ctrl+Digit1」这种东西
        let digit = shortcut_label_for(Modifiers::CONTROL, "0", KeyStyle::Other);
        assert_eq!(digit, "Ctrl+0");
    }

    /// 穿透那个的写法就是界面提示里那句（「按 ⌥⌘S 退出」）。
    #[test]
    fn the_pass_through_label_is_derived_from_the_registration() {
        let label = pass_through_label();
        assert!(label.ends_with('S'), "{label}");
    }

    /// 按一次静音、再按一次恢复。**顺序也要保住** —— `muted_groups` 会被反复比较
    /// （每次要不要弹通知都问一次），乱序会让它每次都当成「变了」。
    #[test]
    fn muting_toggles_and_keeps_the_order() {
        let current = vec!["deploy".to_string()];

        // 没静音的组：加到最后，不打乱已有的
        let (muted, is_muted) = toggled_muted(&current, "ci");
        assert!(is_muted);
        assert_eq!(muted, vec!["deploy".to_string(), "ci".to_string()]);

        // 再按一次：拿掉它，别的原样
        let (back, is_muted) = toggled_muted(&muted, "ci");
        assert!(!is_muted);
        assert_eq!(back, vec!["deploy".to_string()]);

        // 已经在静音列表里的组：不会加第二遍
        let (again, is_muted) = toggled_muted(&back, "deploy");
        assert!(!is_muted);
        assert!(again.is_empty());
    }

    /// 界面读的是这三个字段名。改名字会让设置页那一行变成空白 —— 而这里没有别的
    /// 东西会提醒（TS 那侧的接口是手写的镜像，不做代码生成）。
    #[test]
    fn the_status_serialises_with_the_field_names_the_ui_reads() {
        let json = serde_json::to_value(ShortcutStatus {
            action: "lamp",
            label: "⌃⌥L".to_string(),
            ok: true,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "action": "lamp", "label": "⌃⌥L", "ok": true })
        );
    }

    /// 「最差的那一组」按**紧急度**选，不是按字母序。
    ///
    /// 这条规则的全部依据是 `Store::snapshot_at` 那个排序。要是有人把那边改成按
    /// 组名排，快捷键会开始清掉一个本来没事的组 —— 而那种错几乎不会被发现。
    #[test]
    fn the_worst_group_is_the_most_urgent_one() {
        let mut store = Store::default();
        let now = Instant::now();
        let mut idle = Report::new(Signal::Idle);
        idle.group = Some("aaa".to_string());
        store.apply(now, idle, Duration::from_secs(600));
        let mut bad = Report::new(Signal::Error);
        bad.group = Some("zzz".to_string());
        store.apply(now, bad, Duration::from_secs(600));

        let snapshot = store.snapshot_at(now);
        assert_eq!(worst_of(&snapshot).as_deref(), Some("zzz"));
    }

    /// 一组都没有时什么都不做 —— 快捷键不是错误入口。
    #[test]
    fn nothing_to_act_on_is_not_an_error() {
        assert_eq!(worst_of(&Store::default().snapshot_at(Instant::now())), None);
    }
}
