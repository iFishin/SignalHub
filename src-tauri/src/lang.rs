//! 界面语言，以及**原生界面**上的文案。
//!
//! 前端那一半的文案在 `src/lib/i18n.ts`；这里只管 Rust 自己画出来的那些 ——
//! 托盘菜单、灯上的右键菜单、系统通知。它们不经过 webview，前端换不了。
//!
//! **Rust 不知道系统语言**（就像它不知道时区一样），所以语言由前端解析好推过来：
//! `Settings::locale` 里存的是用户的**偏好**（可能是 `"auto"`），前端把它解析成
//! 具体语言再调 `set_lang`。和 `LocalHour` 一个路子 —— 前端提供事实，Rust 用它
//! 挑文案。

use crate::state::Signal;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// 默认中文：这是这个项目的母语，也是前端还没推第一帧时唯一有依据的猜测。
    #[default]
    Zh,
    En,
}

impl Lang {
    /// 认不出就返回 `None`，由调用方决定怎么办 —— 不在这里猜一个默认值，
    /// 因为「前端推了个不认识的语言」是需要能被发现的事。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh-hans" | "zh-hant" => Some(Lang::Zh),
            "en" | "en-us" | "en-gb" => Some(Lang::En),
            _ => None,
        }
    }
}

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

/// 原生界面上的全部文案。
///
/// 收在一个结构里而不是散成一堆函数：改语言时要重建的东西**正好就是这些**，
/// 有一个地方能一次拿全，就不会漏掉某一条（漏掉的表现是界面上混着两种语言，
/// 而那是最难自查的一类问题）。
pub struct NativeText {
    // ---- 托盘菜单 ----
    pub tray_show_hide: &'static str,
    pub tray_settings: &'static str,
    pub tray_pause: &'static str,
    pub tray_pass_through: &'static str,
    pub tray_collapsed: &'static str,
    pub tray_quit: &'static str,

    // ---- 灯上的右键菜单 ----
    pub menu_hide: &'static str,
    pub menu_mute: &'static str,
    pub menu_unmute: &'static str,
    pub menu_clear: &'static str,
    pub menu_collapse: &'static str,
    pub menu_expand: &'static str,
    pub menu_settings: &'static str,

    // ---- 系统通知 ----
    pub action_reveal: &'static str,
    pub action_mute: &'static str,
    pub pass_through_title: &'static str,

    // ---- 窗口 ----
    pub settings_window_title: &'static str,
}

pub fn text(lang: Lang) -> &'static NativeText {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

/// 穿透模式的说明。要插快捷键文案，所以是函数不是常量。
pub fn pass_through_body(lang: Lang, shortcut: &str) -> String {
    match lang {
        Lang::Zh => format!("圆点不再响应鼠标。按 {shortcut} 或从托盘菜单退出。"),
        Lang::En => {
            format!("The lamp no longer responds to the mouse. Press {shortcut} or use the tray menu to exit.")
        }
    }
}

static ZH: NativeText = NativeText {
    tray_show_hide: "显示 / 隐藏灯球",
    tray_settings: "设置…",
    tray_pause: "暂停 / 恢复监控",
    tray_pass_through: "切换点击穿透",
    tray_collapsed: "展开 / 折叠外形",
    tray_quit: "退出 SignalHub",

    menu_hide: "隐藏该组",
    menu_mute: "静音该组",
    menu_unmute: "取消静音该组",
    menu_clear: "清除该组的状态",
    menu_collapse: "折叠成一颗灯",
    menu_expand: "展开牌面",
    menu_settings: "打开设置…",

    action_reveal: "打开项目目录",
    action_mute: "静音该组",
    pass_through_title: "已进入点击穿透模式",

    settings_window_title: "SignalHub 设置",
};

static EN: NativeText = NativeText {
    tray_show_hide: "Show / hide lamp",
    tray_settings: "Settings…",
    tray_pause: "Pause / resume monitoring",
    tray_pass_through: "Toggle click-through",
    tray_collapsed: "Expand / collapse shape",
    tray_quit: "Quit SignalHub",

    menu_hide: "Hide this group",
    menu_mute: "Mute this group",
    menu_unmute: "Unmute this group",
    menu_clear: "Clear this group's state",
    menu_collapse: "Collapse to a light",
    menu_expand: "Expand signs",
    menu_settings: "Open settings…",

    action_reveal: "Open project folder",
    action_mute: "Mute this group",
    pass_through_title: "Click-through mode is on",

    settings_window_title: "SignalHub Settings",
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
            en.tray_show_hide,
            en.tray_settings,
            en.tray_pause,
            en.tray_pass_through,
            en.tray_collapsed,
            en.tray_quit,
            en.menu_hide,
            en.menu_mute,
            en.menu_unmute,
            en.menu_clear,
            en.menu_collapse,
            en.menu_expand,
            en.menu_settings,
            en.action_reveal,
            en.action_mute,
            en.pass_through_title,
            en.settings_window_title,
        ];
        for line in all {
            assert!(
                !line.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "{line:?} 里还有中文"
            );
        }
        assert!(
            !pass_through_body(Lang::En, "Ctrl+Alt+S")
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "穿透说明里还有中文"
        );
    }
}
