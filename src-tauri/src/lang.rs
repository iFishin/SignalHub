//! 界面语言，以及**原生界面**上的文案。
//!
//! 前端那一半的文案在 `src/lib/i18n.ts`；这里只管 Rust 自己画出来的那些 ——
//! 托盘菜单、灯上的右键菜单、系统通知。它们不经过 webview，前端换不了。
//!
//! **Rust 不知道系统语言**（就像它不知道时区一样），所以语言由前端解析好推过来：
//! `Settings::locale` 里存的是用户的**偏好**（可能是 `"auto"`），前端把它解析成
//! 具体语言再调 `set_lang`。和 `LocalHour` 一个路子 —— 前端提供事实，Rust 用它
//! 挑文案。
//!
//! `Lang` 本身定义在 `config.rs`（CLI 也要用它），这里只用不定义。

use crate::config::Lang;
use crate::state::Signal;
use std::sync::Mutex;

/// 前端推过来的当前界面语言。
#[derive(Default)]
pub struct LocalLang(Mutex<Lang>);

impl LocalLang {
    pub fn set(&self, lang: Lang) {
        *self.0.lock().expect("lang poisoned") = lang;
    }

    pub fn get(&self) -> Lang {
        *self.0.lock().expect("lang poisoned")
    }
}

/// 状态名。**只用于系统通知的标题** —— 界面上的文案在前端的 `i18n.ts`。
///
/// 放在这里而不是 `state.rs`：那个文件被 CLI 通过 `#[path]` 共用，而 CLI 不需要
/// 文案。两个 `match` 都是穷尽的，所以新增状态时编译器会强制两种语言都补上。
pub fn signal_label(lang: Lang, state: Signal) -> &'static str {
    match lang {
        Lang::Zh => match state {
            Signal::Offline => "没有智能体在上报",
            Signal::Off => "已暂停监控",
            Signal::Stale => "会话已沉默",
            Signal::Idle => "空闲",
            Signal::Done => "已完成",
            Signal::Thinking => "推理中",
            Signal::Working => "执行中",
            Signal::Attention => "需要关注",
            Signal::Permission => "需要授权",
            Signal::Error => "出错 / 阻塞",
        },
        Lang::En => match state {
            Signal::Offline => "No agents reporting",
            Signal::Off => "Monitoring paused",
            Signal::Stale => "Session went quiet",
            Signal::Idle => "Idle",
            Signal::Done => "Done",
            Signal::Thinking => "Thinking",
            Signal::Working => "Working",
            Signal::Attention => "Needs attention",
            Signal::Permission => "Needs permission",
            Signal::Error => "Error / blocked",
        },
    }
}

/// 「盯住这个任务」到点那条通知。标题跟着结果走，正文点名是哪个会话。
///
/// 返回 `(title, body)` 而不是一个拼好的字符串：通知插件要两个字段，而在调用点
/// 再拆一次字符串只会让「哪一半是标题」变成第二处真值。
pub fn watch_notice(lang: Lang, state: Option<Signal>, label: &str) -> (String, String) {
    match lang {
        Lang::Zh => match state {
            Some(Signal::Done) => ("任务完成".to_string(), format!("「{label}」跑完了")),
            Some(Signal::Error) => ("任务失败".to_string(), format!("「{label}」出错了")),
            _ => ("任务结束".to_string(), format!("「{label}」已经结束")),
        },
        Lang::En => match state {
            Some(Signal::Done) => ("Task finished".to_string(), format!("\"{label}\" has finished")),
            Some(Signal::Error) => ("Task failed".to_string(), format!("\"{label}\" hit an error")),
            _ => ("Task ended".to_string(), format!("\"{label}\" has ended")),
        },
    }
}

/// 工具动词的分类 → 这个词（HTTP 式 hook 生成「正在做什么」那一行要用）。
///
/// **和 CLI 侧那张表（`cli_text.rs` 的 `tool_v_*`）必须是同一组词。** 同一个工具在
/// 命令式接入下由 CLI 生成这句话、在 HTTP 式接入下由应用生成，两边的说法不一致
/// 会让人以为它们在做不同的事。
pub fn tool_verb(lang: Lang, verb: crate::hookmap::Verb) -> &'static str {
    use crate::hookmap::Verb;
    match (lang, verb) {
        (Lang::Zh, Verb::Exec) => "执行",
        (Lang::Zh, Verb::Edit) => "改",
        (Lang::Zh, Verb::Read) => "读",
        (Lang::Zh, Verb::Search) => "找",
        (Lang::Zh, Verb::Net) => "访问",
        (Lang::En, Verb::Exec) => "Running",
        (Lang::En, Verb::Edit) => "Editing",
        (Lang::En, Verb::Read) => "Reading",
        (Lang::En, Verb::Search) => "Searching",
        (Lang::En, Verb::Net) => "Fetching",
    }
}

/// 「正在执行 npm」那一句。模板跟着 CLI 的 `tool_doing`（`{} {}`）。
pub fn tool_doing(lang: Lang, verb: crate::hookmap::Verb, target: &str) -> String {
    format!("{} {}", tool_verb(lang, verb), target)
}

/// 原生界面上的全部文案。
///
/// 收在一个结构里而不是散成一堆函数：改语言时要重建的东西**正好就是这些**，
/// 有一个地方能一次拿全，就不会漏掉某一条（漏掉的表现是界面上混着两种语言，
/// 而那是最难自查的一类问题）。
pub struct NativeText {
    // ---- 托盘菜单 ----
    pub tray_inbox: &'static str,
    pub tray_show_hide: &'static str,
    pub tray_settings: &'static str,
    pub tray_pause: &'static str,
    pub tray_pass_through: &'static str,
    /// 穿透开着时托盘那一项的文案。带上快捷键后由 `tray.rs` 拼成
    /// 「退出点击穿透 · ⌃⌥S」—— 通知只弹一次，菜单是随时能回去查的那一处。
    pub tray_pass_through_exit: &'static str,
    pub tray_collapsed: &'static str,
    pub tray_quit: &'static str,

    // ---- 灯上的右键菜单 ----
    pub menu_hide: &'static str,
    /// 「重命名…」—— 它不开输入框，只是把设置窗翻到「分组」页（见 `menu.rs`）。
    pub menu_rename: &'static str,
    pub menu_mute: &'static str,
    pub menu_unmute: &'static str,
    pub menu_clear: &'static str,
    /// 「推迟 15 分钟」：牌上不再催，状态一点不改（见 `Store::snooze`）。
    /// 时长必须和 `SNOOZE_MS` 一致 —— 菜单是唯一能说明到底多久的地方。
    pub menu_snooze: &'static str,
    /// 已经在推迟里时同一项变成这个。**同一个菜单 id**，动作跟着翻。
    pub menu_unsnooze: &'static str,
    pub menu_collapse: &'static str,
    pub menu_expand: &'static str,
    pub menu_settings: &'static str,
    /// 「打开项目目录」。点通知本体做的是同一件事，但通知上不再有这枚按钮 ——
    /// 见 `alerts.rs::wire_actions`。文案只留这一份，两处不会各写各的。
    pub menu_open_dir: &'static str,
    /// 「全部归位」：把散落的组灯摆回默认堆叠。措辞说「全部」是为了和设置页
    /// 那颗按钮对得上，用户不会以为是只归位当前右键的这一盏。
    pub menu_reset_lamps: &'static str,

    // ---- 系统通知上的动作按钮 ----
    /// 「已知晓」：**只收起这条通知**，不改灯、不静音、也不碰重复提醒链。
    /// 它在动作列表里排第一个是刻意的（macOS 点「选项」会上报第一个动作），
    /// 详见 `alerts.rs::ACTION_ACK`。
    pub action_ack: &'static str,
    pub action_mute: &'static str,
    pub pass_through_title: &'static str,
    /// 快捷键和托盘菜单两条逃生通道都不在时，拒绝开启穿透的报错文案。
    pub pass_through_no_escape: &'static str,

    // ---- 手动状态（托盘子菜单 + 牌上那行字）----
    /// 托盘里那个子菜单的标题。
    pub manual_title: &'static str,
    /// 专注那一项。牌上的倒计时文案见 `manual_focus_left`。
    pub manual_focus: &'static str,
    /// 开会 / 离开：**托盘那一项和牌上那行字是同一个字符串**（见 `manual::Manual`），
    /// 所以只有一份。
    pub manual_meeting: &'static str,
    pub manual_away: &'static str,
    pub manual_clear: &'static str,
    /// 专注到点时牌上那行字，同时也是那条通知的标题。
    pub manual_focus_done: &'static str,
    /// 从灯上右键给**某一组**手动设状态时，牌上那行字（组名和状态已经由灯画出来了，
    /// 这一行只说明「是手动设的」）。
    pub manual_set: &'static str,

    // ---- 窗口 ----
    pub settings_window_title: &'static str,
    pub inbox_window_title: &'static str,

    // ---- 转发摘要 ----
    /// 转发出去那行字的方括号前缀。收件人一眼知道是谁发的。
    pub summary_prefix: &'static str,
    /// 测试消息的前缀。**必须看起来就像测试** —— 用户点一下「发送测试」收到一条
    /// 和真告警长得一样的消息，会以为真出事了。
    pub summary_prefix_test: &'static str,
}

pub fn text(lang: Lang) -> &'static NativeText {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

/// 穿透模式的说明。要插快捷键文案，所以是函数不是常量。
///
/// `None` = 快捷键没抢到（只剩托盘）。这时候**不能**含糊地说「按快捷键退出」：
/// 通知是一次性的、穿透下又点不到窗口去查，写错等于给用户一条假出路。
pub fn pass_through_body(lang: Lang, shortcut: Option<&str>) -> String {
    match (lang, shortcut) {
        (Lang::Zh, Some(key)) => format!("圆点不再响应鼠标。按 {key} 或从托盘菜单退出。"),
        (Lang::Zh, None) =>
            "圆点不再响应鼠标。全局快捷键没抢到，请从托盘菜单的「退出点击穿透」退出。".to_string(),
        (Lang::En, Some(key)) => {
            format!("The lamp no longer responds to the mouse. Press {key} or use the tray menu to exit.")
        }
        (Lang::En, None) =>
            "The lamp no longer responds to the mouse. The global shortcut is unavailable; use the tray menu's \"Exit click-through\" item to exit.".to_string(),
    }
}

/// 「已静音 / 已取消静音」那两条通告的标题和正文。
///
/// 需要它是因为**静音在屏幕上是看不见的**：静音只影响通知和提示音，灯照常显示
/// 那一组。一个按下去什么都没发生的全局快捷键，用户只会再按几次。见
/// `shortcuts::mute_worst`。
///
/// 返回 `(标题, 正文)` 而不是两个函数：这两句是同一件事的两半，分开写迟早会有
/// 人只改一半。
pub fn mute_message(lang: Lang, group: &str, muted: bool) -> (String, String) {
    match (lang, muted) {
        (Lang::Zh, true) => (
            format!("已静音「{group}」"),
            "这一组不再弹通知和提示音（灯照常显示）。再按一次快捷键恢复。".to_string(),
        ),
        (Lang::Zh, false) => (
            format!("已取消静音「{group}」"),
            "这一组的通知恢复了。".to_string(),
        ),
        (Lang::En, true) => (
            format!("Muted \"{group}\""),
            "This group no longer notifies or plays a sound (the lamp still shows it). Press the shortcut again to undo.".to_string(),
        ),
        (Lang::En, false) => (
            format!("Unmuted \"{group}\""),
            "This group notifies again.".to_string(),
        ),
    }
}

/// 专注倒计时牌上那行字（`manual::report` 每分钟更新一次）。
///
/// 向上取整之后的值从 `manual.rs` 传进来 —— 还剩 30 秒时写「1 分钟」比写「0 分钟」
/// 诚实，而那个取整和「什么时候算到点」是同一件事，不该分两处。
pub fn manual_focus_left(lang: Lang, minutes: u32) -> String {
    match lang {
        Lang::Zh => format!("专注 · 还剩 {minutes} 分钟"),
        Lang::En => format!("Focus · {minutes}m left"),
    }
}

/// 专注到点那条通知的正文。**刻意不提具体分钟数** —— 那个数在 `manual.rs`，
/// 而这里写一句「起来动一动」不需要和它同步。
pub fn manual_focus_done_body(lang: Lang) -> String {
    match lang {
        Lang::Zh => "这一轮结束了，起来动一动。".to_string(),
        Lang::En => "That round is done — time to move.".to_string(),
    }
}

/// 「已经等了多久」。重复提醒的正文用它 —— 见 `alerts::send_reminder`。
///
/// 超过一小时就换成小时制：卡了三个小时时「已等待 180 分钟」还得让人自己换算。
pub fn stuck_for(lang: Lang, minutes: u64) -> String {
    let hours = minutes / 60;
    let rest = minutes % 60;

    match (lang, hours, rest) {
        (Lang::Zh, 0, _) => format!("已等待 {minutes} 分钟"),
        (Lang::Zh, h, 0) => format!("已等待 {h} 小时"),
        (Lang::Zh, h, m) => format!("已等待 {h} 小时 {m} 分钟"),
        (Lang::En, 0, _) => format!("waiting {minutes}m"),
        (Lang::En, h, 0) => format!("waiting {h}h"),
        (Lang::En, h, m) => format!("waiting {h}h {m}m"),
    }
}

static ZH: NativeText = NativeText {
    tray_inbox: "收件箱…",
    tray_show_hide: "显示 / 隐藏灯球",
    tray_settings: "设置…",
    tray_pause: "暂停 / 恢复监控",
    tray_pass_through: "切换点击穿透",
    tray_pass_through_exit: "退出点击穿透",
    tray_collapsed: "展开 / 折叠外形",
    tray_quit: "退出 SignalHub",

    menu_hide: "隐藏该组",
    menu_rename: "重命名…",
    menu_mute: "静音该组",
    menu_unmute: "取消静音该组",
    menu_clear: "清除该组的状态",
    menu_snooze: "推迟 15 分钟",
    menu_unsnooze: "取消推迟",
    menu_collapse: "折叠成灯",
    menu_expand: "展开牌面",
    menu_settings: "打开设置…",
    menu_open_dir: "打开项目目录",
    menu_reset_lamps: "灯全部归位",

    action_ack: "已知晓",
    action_mute: "静音该组",
    pass_through_title: "已进入点击穿透模式",
    pass_through_no_escape: "没有可用的退出通道（全局快捷键没抢到、托盘菜单也不在），拒绝开启点击穿透",

    manual_title: "手动状态",
    manual_focus: "专注 25 分钟",
    manual_meeting: "开会中",
    manual_away: "离开",
    manual_clear: "清除手动状态",
    manual_focus_done: "专注结束",
    manual_set: "手动设置",

    settings_window_title: "SignalHub 设置",
    inbox_window_title: "SignalHub 收件箱",

    summary_prefix: "SignalHub",
    summary_prefix_test: "SignalHub 测试",
};

static EN: NativeText = NativeText {
    tray_inbox: "Inbox…",
    tray_show_hide: "Show / hide lamp",
    tray_settings: "Settings…",
    tray_pause: "Pause / resume monitoring",
    tray_pass_through: "Toggle click-through",
    tray_pass_through_exit: "Exit click-through",
    tray_collapsed: "Expand / collapse shape",
    tray_quit: "Quit SignalHub",

    menu_hide: "Hide this group",
    menu_rename: "Rename…",
    menu_mute: "Mute this group",
    menu_unmute: "Unmute this group",
    menu_clear: "Clear this group's state",
    menu_snooze: "Snooze for 15 min",
    menu_unsnooze: "Cancel snooze",
    menu_collapse: "Collapse to lights",
    menu_expand: "Expand signs",
    menu_settings: "Open settings…",
    menu_open_dir: "Open project folder",
    menu_reset_lamps: "Reset lamp positions",

    action_ack: "Got it",
    action_mute: "Mute this group",
    pass_through_title: "Click-through mode is on",
    pass_through_no_escape: "No exit route is available (shortcut not registered, tray menu missing); click-through was not enabled",

    manual_title: "Manual state",
    manual_focus: "Focus for 25 minutes",
    manual_meeting: "In a meeting",
    manual_away: "Away",
    manual_clear: "Clear manual state",
    manual_focus_done: "Focus finished",
    manual_set: "Set manually",

    settings_window_title: "SignalHub Settings",
    inbox_window_title: "SignalHub Inbox",

    summary_prefix: "SignalHub",
    summary_prefix_test: "SignalHub test",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_parses_common_spellings() {
        for zh in ["zh", "ZH", "zh-CN", " zh-hans "] {
            assert_eq!(Lang::parse(zh), Some(Lang::Zh), "{zh}");
        }
        for en in ["en", "EN", "en-US", "en-GB"] {
            assert_eq!(Lang::parse(en), Some(Lang::En), "{en}");
        }
        // 认不出就返回 None，让调用方决定 —— 不在这里猜
        for other in ["fr", "auto", "", "  "] {
            assert_eq!(Lang::parse(other), None, "{other:?} 不该被认成某种语言");
        }
    }

    /// 前端还没推过第一帧时是中文 —— 前端那边 `detectLang` 才是解析 `"auto"` 的人
    #[test]
    fn the_default_language_is_chinese_and_remembers_what_is_pushed() {
        let lang = LocalLang::default();
        assert_eq!(lang.get(), Lang::Zh);
        lang.set(Lang::En);
        assert_eq!(lang.get(), Lang::En);
    }

    /// 两种语言都必须覆盖全部状态 —— 漏一个的表现是通知标题里冒出一个空字符串
    #[test]
    fn every_state_has_a_label_in_both_languages() {
        let states = [
            Signal::Offline,
            Signal::Off,
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
            Signal::Attention,
            Signal::Permission,
            Signal::Error,
        ];
        for lang in [Lang::Zh, Lang::En] {
            for state in states {
                assert!(
                    !signal_label(lang, state).trim().is_empty(),
                    "{lang:?} 少了 {state:?} 的文案"
                );
            }
        }
    }

    /// 英文那一份不能漏译 —— 漏了就是界面上混着中文，而这只有肉眼能发现
    #[test]
    fn no_english_string_is_left_in_chinese() {
        let en = text(Lang::En);
        let all = [
            en.tray_inbox,
            en.tray_show_hide,
            en.tray_settings,
            en.tray_pause,
            en.tray_pass_through,
            en.tray_pass_through_exit,
            en.tray_collapsed,
            en.tray_quit,
            en.menu_hide,
            en.menu_rename,
            en.menu_mute,
            en.menu_unmute,
            en.menu_clear,
            en.menu_snooze,
            en.menu_unsnooze,
            en.menu_collapse,
            en.menu_expand,
            en.menu_settings,
            en.menu_open_dir,
            en.menu_reset_lamps,
            en.action_ack,
            en.action_mute,
            en.pass_through_title,
            en.pass_through_no_escape,
            en.manual_title,
            en.manual_focus,
            en.manual_meeting,
            en.manual_away,
            en.manual_clear,
            en.manual_focus_done,
            en.settings_window_title,
            en.inbox_window_title,
            en.summary_prefix,
            en.summary_prefix_test,
        ];
        for line in all {
            assert!(
                !line.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "{line:?} 里还有中文"
            );
        }
        assert!(
            !pass_through_body(Lang::En, Some("Ctrl+Alt+S"))
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "穿透说明里还有中文"
        );
        assert!(
            !pass_through_body(Lang::En, None)
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "「只剩托盘」那份穿透说明里还有中文"
        );
        // 两条说明都得给出托盘这条唯一的常驻出路，且没抢到时不能出现按键文案
        let with_key = pass_through_body(Lang::En, Some("Ctrl+Alt+S"));
        let tray_only = pass_through_body(Lang::En, None);
        assert!(with_key.contains("Ctrl+Alt+S") && with_key.contains("tray menu"));
        assert!(!tray_only.contains("Ctrl+Alt+S") && tray_only.contains("tray menu"));
        assert!(
            !stuck_for(Lang::En, 95)
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "「已等待多久」的英文里还有中文"
        );
        for (title, body) in [
            mute_message(Lang::En, "ci", true),
            mute_message(Lang::En, "ci", false),
            (
                manual_focus_left(Lang::En, 25),
                manual_focus_done_body(Lang::En),
            ),
        ] {
            assert!(
                !title
                    .chars()
                    .chain(body.chars())
                    .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "静音通告的英文里还有中文：{title} / {body}"
            );
        }
    }

    /// 「已经等了多久」。一小时以上换成小时制 —— 卡了三小时还写「已等待 180 分钟」
    /// 得让人自己换算，而这行字是重复提醒里唯一的新信息。
    #[test]
    fn stuck_for_switches_to_hours_past_sixty_minutes() {
        assert_eq!(stuck_for(Lang::Zh, 5), "已等待 5 分钟");
        assert_eq!(stuck_for(Lang::Zh, 59), "已等待 59 分钟");
        assert_eq!(stuck_for(Lang::Zh, 60), "已等待 1 小时");
        assert_eq!(stuck_for(Lang::Zh, 95), "已等待 1 小时 35 分钟");

        assert_eq!(stuck_for(Lang::En, 5), "waiting 5m");
        assert_eq!(stuck_for(Lang::En, 60), "waiting 1h");
        assert_eq!(stuck_for(Lang::En, 95), "waiting 1h 35m");
    }
}
