//! 手动状态：专注倒计时 / 开会 / 离开。
//!
//! **它们就是一个普通的组**，走的也是普通上报那条路（`Store::apply` + 推送）——
//! 所以灯、今日统计、历史、只读 Web 面板、转发钩子、物理灯全都自动认它，这里没有
//! 第二套状态机。`source` 照实声明成 `custom`（见 B1 的「信号源」）—— 本地注入这
//! 条路没人读它（凭证表只在上报服务里写），但声明成 agent 会是错的。
//!
//! 为什么值得做：这是「信号中枢在**没有任何 agent** 时也有用」的最低成本证明 ——
//! 一个倒计时占一盏灯、到点提醒；开会 / 离开各占一盏灯。桌面上的那盏灯从此不只说
//! 「智能体在干什么」，也说「我在干什么」。
//!
//! 三个状态各映射到哪个信号（**不新增信号**，配色和灯效那套原样复用）：
//!
//! - 专注 → `working`（绿、会动）—— 它本来就是「正在干活」。
//! - 开会 / 离开 → `idle`（灰、安静模式下半透明）—— 别人（或者从房间另一头看过来
//!   的自己）一眼看到的是「这儿没在忙」，而牌上那行字说清是哪种不忙。
//!
//! 会议和离开**只占一盏灯，不动通知设置**：想「别打扰我」有托盘里的「暂停 / 恢复
//! 监控」，把它悄悄绑进来会让「我离开一下」顺手改掉一个用户没提过的开关。
//!
//! 入口在托盘的「手动状态」子菜单里（四个 id 见下面的常量，映射见 [`action_for`]）。

use crate::config::Lang;
use crate::lang;
use crate::state::{Report, Signal};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// 一轮专注多长。写死成常量：这是「番茄钟」这个词的全部含义，做成可配要一套输入框
/// 和一份持久化，而想专注别的时间完全可以直接再按一次（会重开一轮）。
pub const FOCUS_MINUTES: u32 = 25;

/// 开会 / 离开的停留时长：**撑得过一个工作日，但撑不过夜**。忘了关不会第二天早上
/// 还挂着一块「我离开了」，而中途不需要用户去续。
const LONG_TTL_MS: u32 = 12 * 60 * 60 * 1000;

/// 倒计时多久跳一次。一分钟一跳：牌上那行字是「还剩 N 分钟」，比这更密没有意义，
/// 而每跳一次就是一次上报（进今日统计、进历史）。
const TICK: Duration = Duration::from_secs(60);

/// 手动状态占的那个组的名字。
///
/// **跟着界面语言**（「我」/「Me」）。这里本地化是因为所有显示路径（灯、只读 Web
/// 面板、分组页）都直接用组名 —— 本地化一次就不用在那三处各加一次「这个名字要翻
/// 译」。代价是换过语言之后旧名字那个组会留到 TTL 到期（见 `GROUP_NAMES` 和 `set`）。
fn group_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Zh => GROUP_NAMES[0],
        Lang::En => GROUP_NAMES[1],
    }
}

/// 手动组**所有可能的名字**，每种语言一个。
///
/// 需要这个列表是因为「清除」和「设一个新的」都必须把**旧名字那个组**一起处理掉：
/// 换过语言之后它还在，而它的 TTL 是 12 小时 —— 不清的话牌上会同时挂着「我」和
/// 「Me」，或者「清除手动状态」清完还剩一块清不掉的牌。
const GROUP_NAMES: [&str; 2] = ["我", "Me"];

/// 会话名和上报方名字。**不跟着语言变**：它们在「状态」页和历史里是标识，不是文案。
const SESSION: &str = "me";
const AGENT: &str = "me";

// ---- 托盘菜单的四个 id -------------------------------------------------------
//
// **只有这四个字符串是菜单和动作之间的约定**，托盘那边直接用这些常量（不再手写
// 一遍字面量），映射本身由 `action_for` 一处给出。这样「id 写错了」这类问题就
// 不可能只出现在一半 —— 那种表现是菜单项点了没反应，而且不报错。
pub const MENU_FOCUS: &str = "manual_focus";
pub const MENU_MEETING: &str = "manual_meeting";
pub const MENU_AWAY: &str = "manual_away";
pub const MENU_CLEAR: &str = "manual_clear";

/// 托盘菜单里那两个手动状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manual {
    Meeting,
    Away,
}

impl Manual {
    /// 牌上那行字，**同时就是托盘菜单里那一项** —— 两处是同一句话，所以只有一个来源。
    fn message(self, lang: Lang) -> &'static str {
        let t = lang::text(lang);
        match self {
            Manual::Meeting => t.manual_meeting,
            Manual::Away => t.manual_away,
        }
    }
}

/// 菜单 id → 动作。`None` = 不是手动状态那一项。
pub fn action_for(id: &str) -> Option<fn(&AppHandle)> {
    Some(match id {
        MENU_FOCUS => start_focus,
        MENU_MEETING => |app| set_manual(app, Manual::Meeting),
        MENU_AWAY => |app| set_manual(app, Manual::Away),
        MENU_CLEAR => clear,
        _ => return None,
    })
}

/// 专注倒计时的取消令牌。
///
/// 存一个计数器而不是 `JoinHandle`：线程大部分时间睡在 `sleep` 里，`join` 会卡住
/// 调用方，而「醒来发现该走了」比「叫醒它」简单得多。
#[derive(Default)]
pub struct Token {
    generation: u64,
}

impl Token {
    /// 让上一个线程收工，返回新的代号。
    fn next(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

pub type SharedManual = Arc<Mutex<Token>>;

fn bump(app: &AppHandle) -> u64 {
    let shared = app.state::<SharedManual>();
    let mut guard = shared.lock().expect("manual poisoned");
    guard.next()
}

fn is_current(app: &AppHandle, generation: u64) -> bool {
    let shared = app.state::<SharedManual>();
    shared.lock().expect("manual poisoned").is_current(generation)
}

/// 手动状态对应的那条上报。
///
/// **抽成纯函数**：这条上报的字段就是这个功能对外的全部表现（组名、状态、牌上那行
/// 字、存活多久），所以它们值得被直接断言，而不用起一个真的 app 才能看一眼。
fn request(lang: Lang, state: Signal, message: String, ttl_ms: Option<u32>) -> Report {
    Report {
        state,
        session: Some(SESSION.to_string()),
        group: Some(group_name(lang).to_string()),
        agent: Some(AGENT.to_string()),
        // 「自定义」那一类：这不是智能体报的（见 B1）。**本地注入这条路没人读它**
        // （凭证表只在上报服务里写，见 server.rs），但声明成 agent 会是错的，
        // 所以照实写。
        source: Some("custom".to_string()),
        cwd: None,
        message: Some(message),
        ttl_ms: ttl_ms.map(u64::from),
        pid: None,
    }
}

/// 报一个手动状态。
///
/// 走 `crate::mutate`（和 HTTP 上报、测试上报同一条路），所以推送、聚合、今日统计
/// 都自动跟上；`ttl_ms` 显式给，因此设置里那个「静默多久算过期」压不过它
/// （见 `state::state_ttl` 的第一级）。
fn report(app: &AppHandle, state: Signal, message: String, ttl_ms: Option<u32>) {
    let lang = crate::current_lang(app);
    let req = request(lang, state, message, ttl_ms);
    let ttl = crate::report_ttl(app, &req);
    let store = app.state::<crate::SharedStore>();
    crate::mutate(app, &store, |guard, now| guard.apply(now, req, ttl));
}

/// 把旧名字那个组清掉（换过语言之后它还在），再报当前语言的名字。
fn set(app: &AppHandle, state: Signal, message: String, ttl_ms: Option<u32>) {
    let lang = crate::current_lang(app);
    for name in GROUP_NAMES {
        if name != group_name(lang) {
            crate::clear_group(app, name);
        }
    }
    report(app, state, message, ttl_ms);
}

/// 开会 / 离开。
///
/// **设，不是切**：再按一次同一个是「重新开始计时」，想收掉就按「清除手动状态」——
/// 三个动作都是幂等的，托盘那几项的文案因此不用跟着状态变（那正是菜单状态最容易
/// 和真实状态漂移的地方）。
pub fn set_manual(app: &AppHandle, which: Manual) {
    // 专注可能正在跑：它和手动状态占的是同一盏灯，换掉它就得让旧线程收工
    bump(app);
    let lang = crate::current_lang(app);
    set(
        app,
        Signal::Idle,
        which.message(lang).to_string(),
        Some(LONG_TTL_MS),
    );
}

/// 开始一轮专注：牌上立刻出现倒计时，每分钟更新一次，到点提醒并转 `done`。
pub fn start_focus(app: &AppHandle) {
    let generation = bump(app);
    let lang = crate::current_lang(app);
    let total = FOCUS_MINUTES;
    set(
        app,
        Signal::Working,
        lang::manual_focus_left(lang, total),
        Some(tick_ttl_ms(total)),
    );

    let app = app.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(u64::from(total) * 60);
        loop {
            // 睡到下一跳或到点。**用 deadline 算剩余**而不是固定睡 60 秒：笔记本
            // 睡一觉醒来之后，固定睡法会把过期的分钟数报出去（然后牌上那行字再
            // 挂 24 分钟）。
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            thread::sleep(left.min(TICK));

            // 被新的专注 / 手动状态 / 清除顶掉了：什么都不做地收工。
            // **在报状态之前查**，否则被顶掉的那一轮会往灯上写一笔过期的东西。
            if !is_current(&app, generation) {
                return;
            }
            match minutes_left(deadline, Instant::now()) {
                Some(minutes) => {
                    let lang = crate::current_lang(&app);
                    report(
                        &app,
                        Signal::Working,
                        lang::manual_focus_left(lang, minutes),
                        Some(tick_ttl_ms(minutes)),
                    );
                }
                None => break,
            }
        }

        if !is_current(&app, generation) {
            return;
        }
        // 到点。`done` 的停留时间**不显式给** —— 跟着设置里那个值（默认 30 秒）
        let lang = crate::current_lang(&app);
        let done = lang::text(lang).manual_focus_done;
        report(&app, Signal::Done, done.to_string(), None);

        // 到点要**出声**（跟着「提示音」那个开关）：一个自己设的倒计时到点却悄无声息，
        // 等于没提醒。不走告警那条路 —— 那条要一个快照、会记「这条通知说的是哪一组」、
        // 带动作按钮，而定时器没有「哪一组」。
        let sound = crate::current_settings(&app).sound;
        crate::alerts::send_timer(&app, done, &lang::manual_focus_done_body(lang), sound);
    });
}

/// 清除手动状态（顺手停掉正在跑的专注）。
pub fn clear(app: &AppHandle) {
    bump(app);
    for name in GROUP_NAMES {
        crate::clear_group(app, name);
    }
}

/// 到点还差多少分钟（向上取整）。`None` = 已经到点了。
///
/// 向上取整：还剩 30 秒时牌上写「1 分钟」比写「0 分钟」诚实。抽成纯函数是因为
/// **时间逻辑最该被这样测** —— 不然验证它要真的坐着等 25 分钟。
fn minutes_left(deadline: Instant, now: Instant) -> Option<u32> {
    let left = deadline.saturating_duration_since(now);
    if left.is_zero() {
        None
    } else {
        Some(left.as_secs().div_ceil(60).max(1) as u32)
    }
}

/// 一跳的存活时间：**比下一跳晚一点**。
///
/// 给的是「还剩 N 分钟」（向上取整）再加一跳的余量。正好等于剩余时间的话，牌会在
/// 两跳之间过期一下 —— 扫除先把它清掉、下一跳又加回来，表现为倒计时闪一下。
fn tick_ttl_ms(minutes: u32) -> u32 {
    minutes.max(1) * 60_000 + 60_000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个语言都有名字，而且**两个名字互不相同** —— 相同的话「换语言之后清旧名字」
    /// 会连新的那个一起清掉（表现是切完语言按什么都不亮）。
    #[test]
    fn every_language_has_its_own_group_name() {
        let zh = group_name(Lang::Zh);
        let en = group_name(Lang::En);
        assert_ne!(zh, en);
        assert!(GROUP_NAMES.contains(&zh), "{zh} 不在 GROUP_NAMES 里");
        assert!(GROUP_NAMES.contains(&en), "{en} 不在 GROUP_NAMES 里");
    }

    /// 开会和离开在牌上必须**看得出是两件事**：同一个组、同一个 `idle` 状态，区别
    /// 全在那行字上 —— 两个标签撞了就等于没区别。
    #[test]
    fn meeting_and_away_say_different_things() {
        for lang in [Lang::Zh, Lang::En] {
            assert_ne!(Manual::Meeting.message(lang), Manual::Away.message(lang));
            assert!(!Manual::Meeting.message(lang).is_empty());
        }
    }

    /// 四个菜单 id 必须互不相同（相同的话后注册的那一项会顶掉前一项的动作），
    /// 而且每一个都能映射到动作 —— 一个漏掉的 id 表现为「点了没反应」且不报错。
    #[test]
    fn every_menu_id_maps_to_an_action() {
        let ids = [MENU_FOCUS, MENU_MEETING, MENU_AWAY, MENU_CLEAR];
        for (i, id) in ids.iter().enumerate() {
            assert!(
                !ids[..i].contains(id),
                "{id} 和另一个菜单 id 撞了"
            );
            assert!(action_for(id).is_some(), "{id} 没有对应的动作");
        }
        assert!(action_for("toggle").is_none(), "别人的 id 不该被这里认领");
    }

    /// 整个倒计时的走法：25 分钟里报 24 次「还剩 N 分钟」（24 → 1），第 25 分钟
    /// 到点。**这条覆盖的是最容易错的地方** —— 少跳一次、最后一分钟写 0、或者到点
    /// 那一下没来。
    #[test]
    fn the_countdown_walks_down_to_one_and_then_ends() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(u64::from(FOCUS_MINUTES) * 60);

        // 刚开始：立刻就是「还剩 25 分钟」（`start_focus` 自己报的那一次）
        assert_eq!(minutes_left(deadline, start), Some(FOCUS_MINUTES));

        let mut walked = Vec::new();
        for minute in 1..=FOCUS_MINUTES {
            let now = start + Duration::from_secs(u64::from(minute) * 60);
            walked.push(minutes_left(deadline, now));
        }

        let expected: Vec<Option<u32>> = (1..FOCUS_MINUTES)
            .rev()
            .map(Some)
            .chain(std::iter::once(None))
            .collect();
        assert_eq!(walked, expected);
    }

    /// 一跳必须活过下一跳，否则倒计时会闪（见 `tick_ttl_ms`）。
    #[test]
    fn a_tick_outlives_the_next_one() {
        for minutes in 1..=FOCUS_MINUTES {
            let ttl = tick_ttl_ms(minutes);
            assert!(
                ttl > minutes * 60_000,
                "还剩 {minutes} 分钟时给了 {ttl} 毫秒，撑不到下一跳"
            );
            assert!(ttl <= LONG_TTL_MS, "一跳的存活时间不该超过手动状态的上限");
        }
    }

    /// 这条上报的字段就是这个功能的对外表现：**组名跟着语言**、状态、牌上那行字、
    /// 以及「活多久」。ttl 显式给，所以设置里那个「静默多久算过期」压不过它 ——
    /// 开会 / 离开能挂一整天靠的就是这个。
    #[test]
    fn the_report_carries_the_manual_state() {
        for (lang, group) in [(Lang::Zh, "我"), (Lang::En, "Me")] {
            let meeting = request(
                lang,
                Signal::Idle,
                Manual::Meeting.message(lang).to_string(),
                Some(LONG_TTL_MS),
            );
            assert_eq!(meeting.group.as_deref(), Some(group));
            assert_eq!(meeting.state, Signal::Idle);
            assert_eq!(meeting.message.as_deref(), Some(Manual::Meeting.message(lang)));
            assert_eq!(meeting.ttl_ms, Some(u64::from(LONG_TTL_MS)));
            // 不是智能体报的：归到「自定义」那一类（见 B1）
            assert_eq!(meeting.source.as_deref(), Some("custom"));
            // 会话名和上报方名字**不跟着语言变**（它们在状态页和历史里是标识）
            assert_eq!(meeting.session.as_deref(), Some(SESSION));
            assert_eq!(meeting.agent.as_deref(), Some(AGENT));

            // 专注报的是 working（绿、会动），而且那行字里有分钟数
            let focus = request(
                lang,
                Signal::Working,
                lang::manual_focus_left(lang, FOCUS_MINUTES),
                Some(tick_ttl_ms(FOCUS_MINUTES)),
            );
            assert_eq!(focus.state, Signal::Working);
            assert!(
                focus.message.as_deref().unwrap_or_default().contains(&FOCUS_MINUTES.to_string()),
                "专注那行字里该有分钟数"
            );
        }
    }

    /// 取消令牌：新的那一轮开始之后，旧线程必须认得出自己已经过期了 ——
    /// 否则它会继续往灯上写（表现是倒计时跳回上一个数）。
    #[test]
    fn a_new_generation_cancels_the_old_one() {
        let mut token = Token::default();
        let first = token.next();
        assert!(token.is_current(first));

        let second = token.next();
        assert!(!token.is_current(first), "旧线程还当自己是当前那个");
        assert!(token.is_current(second));
    }
}
