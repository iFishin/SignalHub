//! 内置采集器：把**系统自己的状态**也当一个信号源报进来。
//!
//! 和 `manual.rs` 是一对：那边是「人按出来的状态」，这边是「机器自己看出来的事」。
//! 两者都走普通上报那条路（`Store::apply` + 推送），所以聚合、今日统计、转发钩子、
//! 物理灯全都自动认它 —— 不需要第二套状态机。
//!
//! 三条纪律（见 `docs/plan-round4.md` §4 B2 的坑）：
//!
//! 1. **只读系统状态。** 不读文件内容、不写任何东西。采到的东西只用来决定「亮不亮」。
//! 2. **间隔 ≥ 5 秒，而且每条上报都带 TTL。** 采集线程万一死了，灯上那一组会自己
//!    过期消失，不会留下一条永远亮着的假状态。
//! 3. **采集器出问题不影响主进程。** 一条采集器一条线程，错自己吞掉、只写
//!    `hooks.log`；命令失败一律当成「现在没在发生」——**报一条假的「备份中」比不报
//!    更糟**。
//!
//! 每条采集器一个开关（默认关）。开关关掉时顺手把自己那个组清掉：留着一块「上次
//! 采集的残影」比没有这个功能更让人糊涂。

use crate::config::{Lang, Settings};
use crate::state::{Report, Signal};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// 上报的 agent 名。接入页按 `source` 分组（见 B1 的「信号源」），所以这个名字只
/// 回答「谁报的」——`system` 读作「这台机器自己」。
const AGENT: &str = "system";

/// 采集间隔。**红线：≥ 5 秒**（见 plan §4 B2）。一次工具调用能推几十次状态，而
/// 备份状态五分钟才变一次 —— 采太勤只是白耗电。
pub const INTERVAL: Duration = Duration::from_secs(5);

/// 一条上报的存活时间 = 3 × 间隔。比下一跳晚一点：正好等于间隔的话，牌会在两跳
/// 之间闪一下（过期清掉、下一跳又加回来）。
const TTL_MS: u32 = 15_000;

/// 一个采集器。
///
/// 枚举而不是一堆散函数：`ALL` 就是界面上从上到下的顺序，「支持 / 开关 / 组名 /
/// 牌上那行字」四件事跟着它走，加第三个采集器只需要往 `ALL` 里加一项。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Collector {
    /// Time Machine 备份中（`tmutil status`）。
    Backup,
}

impl Collector {
    /// 全部采集器。**顺序就是界面上从上到下的顺序。**
    pub const ALL: [Collector; 1] = [Collector::Backup];

    /// 稳定标识：写进 `session`、也发给前端当 key。**改了就是换了个人**（会话会
    /// 从零开始，历史记录里那一条也会对不上），所以别改。
    pub fn id(self) -> &'static str {
        match self {
            Collector::Backup => "backup",
        }
    }

    /// 组名跟界面语言。牌子上的字是给人看的，和手动状态一条规矩。
    pub fn group(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Collector::Backup, Lang::Zh) => "备份",
            (Collector::Backup, Lang::En) => "Backup",
        }
    }

    /// 这台机器上做不做得到。做不到**就不显示**（红线：绝不假装某条路能用）——
    /// Time Machine 只有 macOS 有。
    pub fn supported(self) -> bool {
        match self {
            Collector::Backup => cfg!(target_os = "macos"),
        }
    }

    /// 开关（设置里那个）。
    pub fn enabled(self, settings: &Settings) -> bool {
        match self {
            Collector::Backup => settings.collect_backup,
        }
    }

    /// 正在发生时牌上那行字。
    fn message(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Collector::Backup, Lang::Zh) => "正在备份",
            (Collector::Backup, Lang::En) => "Backing up",
        }
    }

    /// 收工那一下牌上那行字。
    fn done_message(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Collector::Backup, Lang::Zh) => "备份完成",
            (Collector::Backup, Lang::En) => "Backup finished",
        }
    }

    /// 一次采样。`Ok(true)` = 现在正在发生，`Ok(false)` = 没有，`Err` = 这次没问出来。
    ///
    /// **「没问出来」和「没有」分开**：对外表现一样（都不报状态），但日志里要能
    /// 分辨 —— 否则「采集器悄悄失效」这件事永远没人看得见。
    fn sample(self) -> Result<bool, String> {
        match self {
            Collector::Backup => Ok(parse_running(&tmutil_status()?)),
        }
    }

    /// 起一条采集线程。**一条采集器一条线程**：一条卡住不会拖住另一条。
    pub fn spawn(self, app: AppHandle) {
        if !self.supported() {
            return;
        }
        thread::spawn(move || self.run(&app));
    }

    fn run(self, app: &AppHandle) {
        let mut was_on = false;
        // 只在「开始出问题」那一次记一笔：每 5 秒记一行会把 `hooks.log` 冲掉。
        let mut complaining = false;

        loop {
            thread::sleep(INTERVAL);

            let settings = crate::current_settings(app);
            if !self.enabled(&settings) {
                // 开关关掉（或者从没开过）。上次留下的组要收掉 —— 只在真的有过的
                // 时候清，否则每 5 秒都会白扫一遍状态表。
                if was_on {
                    was_on = false;
                    clear(app, self);
                }
                complaining = false;
                continue;
            }

            let on = match self.sample() {
                Ok(on) => {
                    complaining = false;
                    on
                }
                Err(why) => {
                    if !complaining {
                        complaining = true;
                        complain(self, &why);
                    }
                    // 问不出来就当「没有」：**绝不把失败变成一条状态**。
                    false
                }
            };

            let lang = crate::current_lang(app);
            match (was_on, on) {
                // 开始了，以及持续中（每跳重报一次，把 TTL 续上）
                (_, true) => {
                    was_on = true;
                    report(app, self, Signal::Working, self.message(lang), Some(TTL_MS));
                }
                // 收工：报一次 `done`，停留时间跟着设置里那个值
                (true, false) => {
                    was_on = false;
                    report(
                        app,
                        self,
                        Signal::Done,
                        self.done_message(lang),
                        None,
                    );
                }
                // 一直没在发生：什么都不做
                (false, false) => {}
            }
        }
    }
}

/// `tmutil status` 的输出里 `Running` 是不是 1。
///
/// 抽成纯函数：这行解析是「备份中」这件事的全部判断依据，值得被直接断言，而不用
/// 真的跑一次备份（跑一次要几十分钟，还得有台 Time Machine 的目标盘）。
///
/// 输出的形状（`tmutil status` 会打一段 plist 风格的文本，缩进是真制表符）：
///
/// ```text
/// Backup session status:
/// {
///     "Running" = 1;
///     ...
/// }
/// ```
///
/// 解析写得松一点：`Running = 1;` / `Running=1` / 前后有空格都认。**只认 `Running`
/// 这一行**，不去猜别的字段 —— 别的字段在版本之间变过。
fn parse_running(text: &str) -> bool {
    text.lines().any(|line| {
        let mut parts = line.trim().splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim();
        key == "Running" && value.trim_end_matches(';').trim() == "1"
    })
}

/// 跑一次 `tmutil status`。
fn tmutil_status() -> Result<String, String> {
    let out = std::process::Command::new("tmutil")
        .arg("status")
        .output()
        .map_err(|err| format!("跑不起 tmutil：{err}"))?;
    if !out.status.success() {
        return Err(format!("tmutil status 退出码 {:?}", out.status.code()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 采集器自己出问题时记一行。**测试里不写**：那是开发者真实的
/// `~/.signalhub/hooks.log`（同 `outbound.rs` 里那条注释的理由）。
fn complain(collector: Collector, why: &str) {
    if cfg!(test) {
        return;
    }
    crate::config::append_hook_log(&format!(
        "{}  {AGENT:<12} {:<20} session={:<38} 采集失败：{why}\n",
        crate::config::utc_hms(),
        "collect",
        collector.id(),
    ));
}

/// 报一条采集结果。走 `crate::mutate`（和 HTTP 上报、手动状态同一条路）。
fn report(
    app: &AppHandle,
    collector: Collector,
    state: Signal,
    message: &str,
    ttl_ms: Option<u32>,
) {
    let req = request(crate::current_lang(app), collector, state, message, ttl_ms);
    let ttl = crate::report_ttl(app, &req);
    let store = app.state::<crate::SharedStore>();
    crate::mutate(app, &store, |guard, now| guard.apply(now, req, ttl));
}

/// 这条上报的字段就是这个功能对外的全部表现（组名、状态、牌上那行字、存活多久、
/// 信号源），所以抽成纯函数直接断言 —— 和 `manual.rs::request` 同一个理由。
///
/// 语言**当参数传**而不是在这里问 `AppHandle`：这样整条上报都能在单测里断言，
/// 而不用起一个真的 app。
fn request(
    lang: Lang,
    collector: Collector,
    state: Signal,
    message: &str,
    ttl_ms: Option<u32>,
) -> Report {
    Report {
        state,
        session: Some(collector.id().to_string()),
        group: Some(collector.group(lang).to_string()),
        agent: Some(AGENT.to_string()),
        // 「系统」那一类（见 B1）。
        source: Some("system".to_string()),
        cwd: None,
        message: Some(message.to_string()),
        ttl_ms: ttl_ms.map(u64::from),
    }
}

/// 清掉这条采集器的组。**两个语言的名字都清** —— 换过界面语言之后，旧名字那个组
/// 还在（和 `manual.rs::set` 同一个坑）。
fn clear(app: &AppHandle, collector: Collector) {
    for lang in [Lang::Zh, Lang::En] {
        crate::clear_group(app, collector.group(lang));
    }
}

/// 发给「接入」页那一节的一行。
///
/// **只发这台机器上做得到的**（`supported`）—— 做不到的采集器不显示，理由见
/// `Collector::supported`。所以结构体里没有再带一个 `supported` 字段：能出现在
/// 这张表里就是能做到。
#[derive(serde::Serialize)]
pub struct CollectorView {
    pub id: &'static str,
    /// 它会把状态报到哪个组（跟着界面语言）
    pub group: String,
    pub enabled: bool,
    /// 多久采一次，界面上写成「每 5 秒看一次」
    pub interval_secs: u64,
}

/// 界面上要显示的那几行。
pub fn views(lang: Lang, settings: &Settings) -> Vec<CollectorView> {
    Collector::ALL
        .iter()
        .filter(|collector| collector.supported())
        .map(|collector| CollectorView {
            id: collector.id(),
            group: collector.group(lang).to_string(),
            enabled: collector.enabled(settings),
            interval_secs: INTERVAL.as_secs(),
        })
        .collect()
}

/// 按 id 找一个采集器（前端只发 id）。
pub fn find(id: &str) -> Option<Collector> {
    Collector::ALL.into_iter().find(|c| c.id() == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tmutil status` 真的在备份时那一段（抄自真机输出）。
    const RUNNING: &str = "Backup session status:\n{\n\tClientID = \"com.apple.backupd\";\n\tRunning = 1;\n\tPercent = \"12.3\";\n}\n";
    /// 没在备份时的形状：`Running = 0`。
    const IDLE: &str = "Backup session status:\n{\n\tRunning = 0;\n}\n";

    #[test]
    fn running_is_read_out_of_the_plist_text() {
        assert!(parse_running(RUNNING));
        assert!(!parse_running(IDLE));
        // 从没备份过 / 命令换了输出格式：一律当成「没在备份」
        assert!(!parse_running(""));
        assert!(!parse_running("No backups.\n"));
    }

    /// 写法松一点没关系，但**别把别的字段认成 Running** —— `Percent = 1` 这种
    /// 行在真实输出里就有。
    #[test]
    fn only_the_running_line_counts() {
        assert!(parse_running("Running=1"));
        assert!(parse_running("  Running  =  1 ;  "));
        assert!(!parse_running("Percent = 1;"));
        assert!(!parse_running("Running = 0;"));
        // 值里带 1 的别的字段也不行
        assert!(!parse_running("Running = 10;"));
    }

    /// 每个语言都有组名，而且**两个名字互不相同** —— 相同的话「换语言之后清旧名字」
    /// 会把新的那个一起清掉。
    #[test]
    fn every_language_has_its_own_group_name() {
        for collector in Collector::ALL {
            assert_ne!(
                collector.group(Lang::Zh),
                collector.group(Lang::En),
                "{} 的中英组名撞了",
                collector.id()
            );
            assert!(!collector.group(Lang::Zh).trim().is_empty());
        }
    }

    /// 每个采集器的 id 唯一 —— 它同时是 `session` 和前端那张表的 key。
    #[test]
    fn every_collector_has_its_own_id() {
        let mut ids: Vec<&str> = Collector::ALL.iter().map(|c| c.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    /// 牌上那两行字（进行中 / 收工）在每个语言里都有，而且不一样 —— 一样的话
    /// 「备份完成」会看起来像还在备份。
    #[test]
    fn the_two_messages_never_collide() {
        for collector in Collector::ALL {
            for lang in [Lang::Zh, Lang::En] {
                let on = collector.message(lang);
                let done = collector.done_message(lang);
                assert!(!on.trim().is_empty());
                assert!(!done.trim().is_empty());
                assert_ne!(on, done, "{} 的进行中和收工文案撞了", collector.id());
            }
        }
    }

    /// 这条上报的字段：`session` / 组名 / 信号源 / 状态 / TTL 都是这个功能对外的
    /// 表现。**TTL 那条是重点**：不带 TTL 的话，采集线程一死，那一组就永远亮着。
    #[test]
    fn the_report_declares_itself_as_a_system_source() {
        for collector in Collector::ALL {
            for lang in [Lang::Zh, Lang::En] {
                let req = request(
                    lang,
                    collector,
                    Signal::Working,
                    collector.message(lang),
                    Some(TTL_MS),
                );

                assert_eq!(req.state, Signal::Working);
                assert_eq!(req.source.as_deref(), Some("system"));
                assert_eq!(req.session.as_deref(), Some(collector.id()));
                assert_eq!(req.group.as_deref(), Some(collector.group(lang)));
                assert_eq!(req.agent.as_deref(), Some(AGENT));
                assert_eq!(req.message.as_deref(), Some(collector.message(lang)));
                assert_eq!(req.ttl_ms, Some(u64::from(TTL_MS)));
                // 采集器不认路径：它不是从某个目录里报上来的
                assert_eq!(req.cwd, None);
            }
        }
    }

    /// 间隔是红线的一部分：**≥ 5 秒**。这条钉住它，免得以后有人为了「更灵」调小。
    #[test]
    fn the_interval_respects_the_red_line() {
        assert!(INTERVAL >= Duration::from_secs(5), "{INTERVAL:?}");
        // TTL 要比一跳长，否则牌会在两跳之间闪一下
        assert!(u64::from(TTL_MS) > INTERVAL.as_millis() as u64);
    }
}
