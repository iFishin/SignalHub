//! 状态时长统计。
//!
//! 按「前端给的本地日期」分桶 —— **本地日期只有前端知道**（Rust 这边没有
//! 时区数据，`SystemTime` 只给 UTC），所以由前端推过来；日期一变就结算。
//! 这样不必引入时区依赖，也不用猜用户在哪一天。
//!
//! **每日合计会落盘**（`~/.signalhub/stats.json`）。灯球是常驻应用，但「常驻」
//! 不等于「永不重启」：改了设置、升级、崩溃重来，一天里那些「等授权等了多久」
//! 是用户会想跨重启看的东西。
//!
//! 内存里的当前这段用的是 `Instant`（单调钟，不受系统时钟调整影响），而落盘
//! 必须用**墙上时钟的日期** —— `Instant` 存不下来，跨重启也没有意义。所以文件
//! 里只有「某天某状态多少秒」，当前这一段是**存的时候现折算**的。

use crate::state::Signal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

/// 时间线最多记住多少次状态变化。超过就丢最旧的 —— 常驻应用跑一周也不该
/// 无限增长。
const TIMELINE_CAPACITY: usize = 512;

/// 每日合计保留多少天。够了（能回答「这周和上周比」），又不会让文件无限长。
const KEEP_DAYS: usize = 30;

/// 落盘的样子。
///
/// 键是**状态名的字符串**，不是 `Signal` 本身。这不是图省事：`Signal` 的
/// `Deserialize` 是给外部上报用的，**刻意拒绝 `stale` / `offline`**（派生状态不该
/// 被外部声称）—— 而统计里那两个恰恰要有，灯灰着的那段时间也是时间。用字符串键
/// 走 `Signal::from_name`（它接受全部名字）就绕开了那个属于**上报**的约束。
#[derive(Serialize, Deserialize, Default)]
struct Stored {
    days: HashMap<String, HashMap<String, u64>>,
}

impl Stored {
    fn from_stats(stats: &Stats, now: Instant) -> Self {
        let mut days: HashMap<String, HashMap<String, u64>> = stats
            .history
            .iter()
            .map(|(day, totals)| (day.clone(), by_name(totals)))
            .collect();

        let today = by_name(&stats.snapshot_today(now));
        if !today.is_empty() {
            days.insert(stats.day.clone(), today);
        }
        Self { days }
    }

    fn into_stats(self) -> Stats {
        let history: HashMap<String, HashMap<Signal, u64>> = self
            .days
            .into_iter()
            .map(|(day, totals)| (day, to_states(&totals)))
            .collect();
        let mut stats = Stats {
            history,
            ..Default::default()
        };
        stats.trim_history();
        stats
    }
}

/// `Signal` → 名字。走 `name()`，所以文件和线上用的是同一套拼写。
fn by_name(totals: &HashMap<Signal, u64>) -> HashMap<String, u64> {
    totals
        .iter()
        .filter(|(_, secs)| **secs > 0)
        .map(|(state, secs)| (state.name().to_string(), *secs))
        .collect()
}

/// 名字 → `Signal`。认不出来的名字**直接丢掉**而不是报错：文件可能是旧版本写的，
/// 里面有一个已经不存在的状态名，为此把整份统计判死不值得。
fn to_states(totals: &HashMap<String, u64>) -> HashMap<Signal, u64> {
    totals
        .iter()
        .filter_map(|(name, secs)| Signal::from_name(name).map(|state| (state, *secs)))
        .collect()
}

/// 按时长从多到少；时长相同再按优先级。
///
/// 今日和历史共用这一条：**顺序不能取决于 `HashMap` 的迭代顺序**，否则界面每刷新
/// 一次就可能换个次序；而两份各写一遍的话，迟早只改到一处。
fn by_duration_desc(rows: &mut [(Signal, Duration)]) {
    rows.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| b.0.priority().cmp(&a.0.priority()))
    });
}

#[derive(Debug, Default)]
pub struct Stats {
    /// 前端上次给的本地日期，用来判断是否跨天
    day: String,
    /// **今天**已结算的各状态累计时长
    totals: HashMap<Signal, Duration>,
    /// 过去几天（键是本地日期，值是各状态的**秒数**）。落盘的就是这个 + 今天。
    ///
    /// 存秒数而不是 `Duration`：文件是给人看也给人改的，`{"working": 3600}` 比
    /// 一个带纳秒的结构好读得多，而且秒是够用的精度。
    history: HashMap<String, HashMap<Signal, u64>>,
    /// 当前这一段的起点
    segment_start: Option<Instant>,
    /// 当前正在累计的状态
    current: Option<Signal>,
    /// 最近的状态变化，按时间先后排列，供 `timeline` 画「近 N 分钟」的条带。
    /// 只在状态**真的变了**时追加，所以相邻两项的状态必定不同。
    timeline: Vec<(Instant, Signal)>,
}

/// 时间线上的一段：某个状态持续了多久。
///
/// 直接给时长而不是给时间戳：`Instant` 换成墙上时钟要处理时区和时钟回拨，
/// 而前端只需要「这一段占多宽」。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Segment {
    pub state: Signal,
    /// 这一段的时长（毫秒）
    pub ms: u64,
}

impl Stats {
    /// 记一次聚合变化：结算上一段，然后开始新的一段。
    pub fn record(&mut self, now: Instant, aggregate: Signal) {
        self.close_segment(now);
        self.current = Some(aggregate);
        self.segment_start = Some(now);

        // 只在状态真的变了时记一笔 —— 同一个状态反复上报不该刷屏时间线
        if self.timeline.last().map(|(_, s)| *s) != Some(aggregate) {
            self.timeline.push((now, aggregate));
            // 每次只多一个，所以最多丢一个；O(n) 的 remove(0) 在 512 的规模上无妨
            if self.timeline.len() > TIMELINE_CAPACITY {
                self.timeline.remove(0);
            }
        }
    }

    /// 跨天就结算：把昨天归档、清零，新的一天重新起算。返回是否真的跨了天。
    ///
    /// **从 `report` 里摘出来，是为了让「不查询」的那条路径也能调它。** 原先跨天只在
    /// `report` 里判，而那只在设置面板开着时发生 —— 灯球是常驻应用，整天不开面板是
    /// 常态，于是「今日时长」会横跨多天一路累加。日期现在由前端每分钟推一次
    /// （见 `lib.rs` 的 `set_local_clock`），跨天因此能在分钟级被发现。
    pub fn rollover(&mut self, now: Instant, today: &str) -> bool {
        if self.day == today {
            return false;
        }
        let first = self.day.is_empty();

        if !first {
            // **先把当前这段结算掉再归档。** 不做这一步的话，一天里最后那一整段
            // ——从上次状态变化到跨天，可能几小时——会整个丢掉：它在 `segment_start`
            // 里，还没进 `totals`。
            self.close_segment(now);

            // 空的一天不记 —— 应用没开的日子在文件里留一条全零的记录，只会让
            // 「哪几天真的在用」这件事看不出来。
            let archived: HashMap<Signal, u64> = self
                .totals
                .iter()
                .filter(|(_, total)| !total.is_zero())
                .map(|(state, total)| (*state, total.as_secs()))
                .collect();
            if !archived.is_empty() {
                self.history.insert(self.day.clone(), archived);
            }
            self.totals.clear();
            // 跨天时当前这段也重新起算，否则会把昨天那段算进今天
            if self.current.is_some() {
                self.segment_start = Some(now);
            }
        }

        self.day = today.to_string();

        // 这一天盘上可能已经有记录 —— 重启之后接着算，或者应用关了一夜再开。
        // **取出来就用、并从 history 里删掉**，这样「今天」只在一处：`totals`。
        // 留着的话，落盘时 `history + 今天的 totals` 会把今天算两遍。
        if let Some(restored) = self.history.remove(today) {
            self.totals = restored
                .into_iter()
                .map(|(state, secs)| (state, Duration::from_secs(secs)))
                .collect();
        }

        self.trim_history();
        !first
    }

    /// 只留最近 `KEEP_DAYS` 天。按日期字符串排序即可 —— 本地日期是 `YYYY-MM-DD`，
    /// 字典序就是时间序（这也是让前端用 `toLocaleDateString("sv")` 的原因之一）。
    fn trim_history(&mut self) {
        while self.history.len() > KEEP_DAYS {
            let Some(oldest) = self.history.keys().min().cloned() else {
                break;
            };
            self.history.remove(&oldest);
        }
    }

    /// 落盘。`now` 用来把**正在进行的那一段**也折算进去 —— 不折算的话，崩掉时
    /// 正在跑的那一段（可能已经几十分钟）会整个丢掉。
    ///
    /// 写的是「历史 + 今天」的完整快照，不是增量：文件小、逻辑简单，而且坏掉时
    /// 最多丢一次刷新的量。
    pub fn save(&self, now: Instant) -> io::Result<()> {
        let Ok(path) = crate::config::stats_file() else {
            return Ok(());
        };
        self.save_to(&path, now)
    }

    /// 指定路径的版本 —— 单测用它写进临时目录，不必碰用户真实的 `~/.signalhub`。
    pub fn save_to(&self, path: &std::path::Path, now: Instant) -> io::Result<()> {
        if self.day.is_empty() {
            // 还不知道「今天」是哪天（前端还没推过时钟），这时候写下去只会把
            // 当前这段记到一个空日期上
            return Ok(());
        }

        let json = serde_json::to_string_pretty(&Stored::from_stats(self, now))
            .map_err(|err| io::Error::other(format!("序列化统计失败: {err}")))?;
        crate::config::atomic_write(path, &json, None)
    }

    /// 今天的合计，含**正在进行的那一段**（按 `now` 现折算，不改动自身）。
    fn snapshot_today(&self, now: Instant) -> HashMap<Signal, u64> {
        let mut totals: HashMap<Signal, u64> = self
            .totals
            .iter()
            .filter(|(_, total)| !total.is_zero())
            .map(|(state, total)| (*state, total.as_secs()))
            .collect();

        // 正在跑的那一段要按 `now` 现算进来，而不是只算已结算的部分
        if let (Some(state), Some(start)) = (self.current, self.segment_start)
            && let Some(elapsed) = now.checked_duration_since(start)
        {
            *totals.entry(state).or_default() += elapsed.as_secs();
        }
        totals
    }

    /// 从盘上读回来。**任何问题都退回空统计**，和 `load_settings` 的兜底一致 ——
    /// 为一份统计文件读不出来而拒绝启动，代价远大于收益。
    pub fn load() -> Self {
        let Ok(path) = crate::config::stats_file() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Stored>(&raw) {
            Ok(stored) => stored.into_stats(),
            Err(err) => {
                eprintln!(
                    "[signalhub] {} 无法解析（{err}），统计从零开始",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// 取统计，按时长从多到少。
    ///
    /// `today` 是前端给的本地日期；和上次不同就重置（跨天了）。
    /// 会把**正在进行的那一段**也算进去，所以不必等状态变化就能看到当前进度。
    pub fn report(&mut self, now: Instant, today: &str) -> Vec<(Signal, Duration)> {
        self.rollover(now, today);

        self.close_segment(now);
        // 结算完立刻把当前这段重新打开，否则下一次查询会漏掉这段时间
        if self.current.is_some() {
            self.segment_start = Some(now);
        }

        let mut rows: Vec<(Signal, Duration)> = self
            .totals
            .iter()
            .filter(|(_, total)| !total.is_zero())
            .map(|(state, total)| (*state, *total))
            .collect();
        by_duration_desc(&mut rows);
        rows
    }

    /// 过去 `days` 天（**含今天**）的每日分状态时长，**由近及远**。
    ///
    /// 今天走 `report()`，更早的从 `history` 字段拿 —— 两边本来就是同一份账的两半
    /// （今天在 `totals` 里、过去在 `history` 里，见 `rollover`），这里只是把它们
    /// 摆成「一天一行」，不重算任何东西。
    ///
    /// **没有记录的那几天不补零**：`history` 里只有真的记过的日期，中间断掉的日子
    /// 本来就不存在。补一排 0 会读成「那天灯开着但什么都没干」，而真相是应用没跑。
    ///
    /// 叫 `recent_days` 而不是 `history`：那样会和 `history` 字段撞名，
    /// `stats.history(...)` 和 `stats.history` 摆在一起谁也读不出哪个是哪个。
    pub fn recent_days(
        &mut self,
        now: Instant,
        today: &str,
        days: usize,
    ) -> Vec<(String, Vec<(Signal, Duration)>)> {
        let mut out = vec![(today.to_string(), self.report(now, today))];

        // 日期是 `YYYY-MM-DD`，所以字符串倒序就是时间倒序（`trim_history` 同理）
        let mut past: Vec<(&String, &HashMap<Signal, u64>)> = self.history.iter().collect();
        past.sort_by(|a, b| b.0.cmp(a.0));

        for (day, seconds_by_state) in past.into_iter().take(days.saturating_sub(1)) {
            let mut rows: Vec<(Signal, Duration)> = seconds_by_state
                .iter()
                .filter(|(_, seconds)| **seconds > 0)
                .map(|(state, seconds)| (*state, Duration::from_secs(*seconds)))
                .collect();
            by_duration_desc(&mut rows);
            out.push((day.clone(), rows));
        }
        out
    }

    /// 近 `window` 内的状态分段，**从旧到新**，覆盖 `[now - window, now]`。
    ///
    /// 最后一段是「现在还在持续的那一段」，会截到 `now`。第一个样本之前没有已知
    /// 状态，所以时间线从 `max(now - window, 第一个样本的时刻)` 开始 —— 宁可短一点，
    /// 也不假装知道应用启动前是什么状态。
    pub fn timeline(&self, now: Instant, window: Duration) -> Vec<Segment> {
        if self.timeline.is_empty() {
            return Vec::new();
        }

        // 窗口左边界；减不动（早于 Instant 能表示的最早点）就不截窗口
        let start = now.checked_sub(window).unwrap_or(now);
        // 第一个样本之前状态未知，所以覆盖区间从两者中较晚的那个起算
        let begin = start.max(self.timeline[0].0);
        if begin >= now {
            // 已知的状态都发生在 now 之后，这段时间里没什么可画的
            return Vec::new();
        }

        // 先找出 begin 时刻生效的状态：最后一个不晚于 begin 的样本
        let mut state = self.timeline[0].1;
        let mut cursor = begin;
        let mut segments: Vec<Segment> = Vec::new();

        for &(at, next) in &self.timeline {
            if at <= begin {
                state = next;
                continue;
            }
            if at >= now {
                // 窗口右边界之外的变化不画；它属于「未来」
                break;
            }
            // 上一段 [cursor, at) 到此为止，随后切换到 next
            segments.push(Segment {
                state,
                ms: at.saturating_duration_since(cursor).as_millis() as u64,
            });
            state = next;
            cursor = at;
        }

        // 收尾：当前状态从 cursor 一直持续到 now
        segments.push(Segment {
            state,
            ms: now.saturating_duration_since(cursor).as_millis() as u64,
        });

        segments
    }

    fn close_segment(&mut self, now: Instant) {
        if let (Some(start), Some(state)) = (self.segment_start.take(), self.current) {
            *self.totals.entry(state).or_default() +=
                now.saturating_duration_since(start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例一个独立目录，避免相互干扰，也不必碰用户真实的 `~/.signalhub`
    fn temp_path(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "signalhub-stats-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// 存了能读回来：今天已累计的部分，重启之后接着算。
    #[test]
    fn today_survives_a_restart() {
        let path = temp_path("stats.json");
        let (mut stats, t) = (Stats::default(), Instant::now());

        stats.record(t, Signal::Working);
        stats.rollover(t, "2026-10-12");
        // 跑 40 秒
        stats.record(t + Duration::from_secs(40), Signal::Idle);
        stats.save_to(&path, t + Duration::from_secs(40)).unwrap();

        // 「重启」：读回来，再推一次同一个日期
        let mut restored = Stats::load_from(&path);
        let t2 = Instant::now();
        restored.record(t2, Signal::Idle);
        restored.rollover(t2, "2026-10-12");

        let rows = restored.report(t2 + Duration::from_secs(5), "2026-10-12");
        assert!(
            rows.contains(&(Signal::Working, Duration::from_secs(40))),
            "昨天（其实是今天）那 40 秒要还在：{rows:?}"
        );
        // 重启后新攒的 5 秒要加在原来那 40 秒上，而不是另起一段
        assert!(
            rows.contains(&(Signal::Idle, Duration::from_secs(5))),
            "{rows:?}"
        );
    }

    /// 过完的一天要归档，而且**下一个新日子从零开始**。
    #[test]
    fn a_finished_day_is_archived_and_the_next_one_starts_clean() {
        let path = temp_path("stats.json");
        let (mut stats, t) = (Stats::default(), Instant::now());

        stats.record(t, Signal::Working);
        stats.rollover(t, "2026-10-12");
        stats.record(t + Duration::from_secs(40), Signal::Idle);
        // 跨天：40 秒的 working 归档到 10-12
        stats.rollover(t + Duration::from_secs(40), "2026-10-13");
        stats.save_to(&path, t + Duration::from_secs(40)).unwrap();

        let mut restored = Stats::load_from(&path);
        let t2 = Instant::now();
        restored.rollover(t2, "2026-10-13");
        let rows = restored.report(t2, "2026-10-13");
        assert!(rows.is_empty(), "新的一天不该带着昨天的时长：{rows:?}");

        // 但昨天那份还在（落盘时写进去了）—— 这是「历史」的意义
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("2026-10-12"), "{raw}");
        assert!(raw.contains("\"working\": 40"), "{raw}");
    }

    /// **正在跑的那一段也要存进去。** 不折算的话，崩掉时正在跑的那一段
    /// （可能已经几十分钟）会整个丢掉。
    #[test]
    fn the_open_segment_is_folded_in_at_save_time() {
        let path = temp_path("stats.json");
        let (mut stats, t) = (Stats::default(), Instant::now());

        stats.record(t, Signal::Working);
        stats.rollover(t, "2026-10-12");
        // 刻意**不**再 record —— 那一段还在跑
        stats.save_to(&path, t + Duration::from_secs(90)).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"working\": 90"), "在跑的那 90 秒要算进去：{raw}");
    }

    /// 还不知道今天是哪天时**不写盘** —— 否则会把当前这段记到一个空日期上。
    #[test]
    fn nothing_is_written_before_the_date_is_known() {
        let path = temp_path("stats.json");
        let (stats, t) = (Stats::default(), Instant::now());
        stats.save_to(&path, t).unwrap();
        assert!(!path.exists(), "没日期就不该落盘");
    }

    /// 认不出来的状态名直接丢掉，不把整份统计判死（文件可能是旧版本写的）。
    #[test]
    fn an_unknown_state_name_is_dropped_not_fatal() {
        let path = temp_path("stats.json");
        std::fs::write(
            &path,
            r#"{"days":{"2026-10-12":{"working":60,"teleporting":99}}}"#,
        )
        .unwrap();

        let mut stats = Stats::load_from(&path);
        let t = Instant::now();
        stats.rollover(t, "2026-10-12");
        let rows = stats.report(t, "2026-10-12");
        assert_eq!(rows, vec![(Signal::Working, Duration::from_secs(60))], "{rows:?}");
    }

    /// 坏文件退回空统计，而不是让应用起不来。
    #[test]
    fn a_corrupt_file_falls_back_to_empty() {
        let path = temp_path("stats.json");
        std::fs::write(&path, "{ 这不是 json").unwrap();
        let stats = Stats::load_from(&path);
        assert!(stats.history.is_empty());
    }

    /// 只留最近 `KEEP_DAYS` 天。
    #[test]
    fn old_days_are_trimmed() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        // 40 个连续日期（跨两个月），比 KEEP_DAYS 多
        let dates: Vec<String> = (0..40)
            .map(|i| {
                let (month, day) = if i < 31 { (1, i + 1) } else { (2, i - 30) };
                format!("2026-{month:02}-{day:02}")
            })
            .collect();

        let mut clock = t;
        stats.rollover(clock, &dates[0]);
        for date in &dates[1..] {
            stats.record(clock, Signal::Working);
            clock += Duration::from_secs(10);
            stats.rollover(clock, date);
        }

        assert_eq!(stats.history.len(), KEEP_DAYS, "只留最近 {KEEP_DAYS} 天");
        assert!(!stats.history.contains_key(&dates[0]), "最老的那天要被挤掉");
        assert!(
            stats.history.contains_key(&dates[38]),
            "最新归档的那天要在"
        );
        // 最后那个是**今天**，它在 `totals` 里而不是 history —— 这条也顺手锁住
        // 「今天只存一处」这个不变量，免得落盘时把今天算两遍
        assert!(
            !stats.history.contains_key(&dates[39]),
            "今天不该出现在历史里"
        );
    }

    /// 历史是「一天一行、由近及远」，**今天永远在最前**（它不在 `history` 字段里，
    /// 而在 `totals` 里 —— 这条也顺手锁住「今天只存一处」这个不变量）。
    #[test]
    fn recent_days_put_today_first_and_skip_days_with_no_record() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);
        stats
            .history
            .insert("2026-10-10".into(), [(Signal::Working, 600u64)].into_iter().collect());
        stats
            .history
            .insert("2026-10-09".into(), [(Signal::Error, 120u64)].into_iter().collect());

        let days = stats.recent_days(t, "2026-10-12", 7);
        let names: Vec<&str> = days.iter().map(|(day, _)| day.as_str()).collect();
        // 10-11 那天没有任何记录 —— **不补一行零**：补了会读成「那天灯开着但什么都没干」
        assert_eq!(names, ["2026-10-12", "2026-10-10", "2026-10-09"]);
    }

    /// 每天内部**按时长从多到少**，零秒的状态不出现。
    ///
    /// 排序不能交给 `HashMap` 的迭代顺序：那样界面每刷新一次就可能换个次序。
    #[test]
    fn a_day_lists_its_states_longest_first() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.history.insert(
            "2026-10-11".into(),
            [
                (Signal::Idle, 10u64),
                (Signal::Working, 900),
                (Signal::Done, 0),
                (Signal::Error, 60),
            ]
            .into_iter()
            .collect(),
        );

        let days = stats.recent_days(t, "2026-10-12", 7);
        let states: Vec<Signal> = days[1].1.iter().map(|(state, _)| *state).collect();
        assert_eq!(
            states,
            [Signal::Working, Signal::Error, Signal::Idle],
            "多的在前，零秒的不出现"
        );
    }

    /// `days` 是**总天数**（含今天），不是「再往前几天」。
    #[test]
    fn the_day_count_includes_today() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        for day in ["2026-10-08", "2026-10-09", "2026-10-10", "2026-10-11"] {
            stats
                .history
                .insert(day.into(), [(Signal::Working, 60u64)].into_iter().collect());
        }

        // 要 3 天 = 今天 + 昨天 + 前天
        let names: Vec<String> = stats
            .recent_days(t, "2026-10-12", 3)
            .into_iter()
            .map(|(day, _)| day)
            .collect();
        assert_eq!(names, ["2026-10-12", "2026-10-11", "2026-10-10"]);

        // 要 0 天也至少有今天那一行 —— 否则界面上会连「今天」都不见了
        assert_eq!(stats.recent_days(t, "2026-10-12", 0).len(), 1);
    }

    /// **跨天时要先把正在跑的那一段结算掉再归档。**
    /// 不结算的话，一天里最后那一段（从上次状态变化到跨天，可能几小时）整个丢掉
    /// —— 它在 `segment_start` 里，还没进 `totals`。
    #[test]
    fn the_open_segment_is_archived_on_rollover() {
        let path = temp_path("stats.json");
        let (mut stats, t) = (Stats::default(), Instant::now());

        stats.record(t, Signal::Working);
        stats.rollover(t, "2026-10-12");
        // 整整跑了 3 小时没换过状态 —— 这段全程都在 segment_start 里
        let three_hours = Duration::from_secs(3 * 3600);
        stats.rollover(t + three_hours, "2026-10-13");
        stats.save_to(&path, t + three_hours).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains(&format!("\"working\": {}", 3 * 3600)),
            "在跑的那 3 小时必须归档，不能丢：{raw}"
        );
    }

    /// **不查询也能跨天。** 原先跨天只在 `report` 里判，而那只在有人查统计
    /// （设置面板开着）时发生 —— 灯球是常驻应用，整天不开面板是常态，于是
    /// 「今日时长」会横跨多天一路累加。现在 `rollover` 可以被单独调用，
    /// 跟着前端每分钟一次的时钟心跳走。
    #[test]
    fn rollover_clears_the_totals_without_being_asked_for_a_report() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);
        stats.rollover(t, "2026-10-11");

        // 攒了 10 秒
        assert_eq!(
            stats.report(t + Duration::from_secs(10), "2026-10-11"),
            vec![(Signal::Working, Duration::from_secs(10))]
        );

        // 跨天：不查统计，只推一次新日期
        assert!(stats.rollover(t + Duration::from_secs(20), "2026-10-12"));

        // 昨天的 10 秒不该还在。新的一天只从「发现跨天的那一刻」（t+20）起算 ——
        // 前端每分钟推一次，所以午夜真正发生在 (t+10, t+20] 之间某一刻，两边都得
        // 猜；猜「从现在开始」宁可少算，也好过把昨天的时段算进今天。
        let rows = stats.report(t + Duration::from_secs(30), "2026-10-12");
        assert_eq!(
            rows,
            vec![(Signal::Working, Duration::from_secs(10))],
            "跨天要清零，只算发现跨天之后这一段：{rows:?}"
        );
    }

    /// 第一次拿到日期**不清空** —— 从启动到前端第一次推时钟之间累计的时间
    /// 不能被白扔。
    #[test]
    fn the_first_date_is_only_remembered() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);

        assert!(
            !stats.rollover(t + Duration::from_secs(5), "2026-10-11"),
            "第一次不算跨天"
        );
        assert_eq!(
            stats.report(t + Duration::from_secs(5), "2026-10-11"),
            vec![(Signal::Working, Duration::from_secs(5))],
            "那 5 秒要留着"
        );
    }

    /// 同一个日期反复推不该有任何副作用（前端每分钟推一次）。
    #[test]
    fn the_same_date_is_a_no_op() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);
        stats.rollover(t, "2026-10-11");

        for _ in 0..5 {
            assert!(!stats.rollover(t + Duration::from_secs(3), "2026-10-11"));
        }
        assert_eq!(
            stats.report(t + Duration::from_secs(3), "2026-10-11"),
            vec![(Signal::Working, Duration::from_secs(3))],
            "反复推同一个日期不该把累计清掉"
        );
    }

    #[test]
    fn time_accumulates_into_the_state_that_was_active() {
        let (mut stats, t) = (Stats::default(), Instant::now());

        stats.record(t, Signal::Working);
        stats.record(t + Duration::from_secs(10), Signal::Idle);
        // 刻意用不重复的时长：平局顺序另有规则，不该由这个用例来断言
        stats.record(t + Duration::from_secs(16), Signal::Permission);

        let rows = stats.report(t + Duration::from_secs(20), "2026-10-11");
        assert_eq!(
            rows,
            vec![
                (Signal::Working, Duration::from_secs(10)),
                (Signal::Idle, Duration::from_secs(6)),
                (Signal::Permission, Duration::from_secs(4)),
            ]
        );
    }

    /// 时长平局时按优先级排，否则顺序取决于 HashMap 迭代顺序、界面会跳
    #[test]
    fn equal_durations_are_ordered_by_priority() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Idle);
        stats.record(t + Duration::from_secs(5), Signal::Permission);
        stats.record(t + Duration::from_secs(10), Signal::Working);

        // 查到 t+15 才让三个状态各占满 5 秒（否则最后一段是 0 秒，会被过滤掉）
        let rows = stats.report(t + Duration::from_secs(15), "2026-10-11");
        assert_eq!(
            rows,
            vec![
                (Signal::Permission, Duration::from_secs(5)),
                (Signal::Working, Duration::from_secs(5)),
                (Signal::Idle, Duration::from_secs(5)),
            ],
            "同为 5 秒时按优先级降序"
        );
    }

    /// 查询不该把正在跑的那一段吃掉 —— 否则反复查询会漏时间
    #[test]
    fn querying_repeatedly_does_not_lose_time() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);

        // 连查三次，时间点不同
        stats.report(t + Duration::from_secs(4), "2026-10-11");
        stats.report(t + Duration::from_secs(7), "2026-10-11");
        let rows = stats.report(t + Duration::from_secs(10), "2026-10-11");

        assert_eq!(rows, vec![(Signal::Working, Duration::from_secs(10))]);
    }

    #[test]
    fn a_new_day_resets_the_totals() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);
        stats.report(t + Duration::from_secs(60), "2026-10-11");

        // 跨天：前一天的累计清零，且当前这段重新起算（不把昨天的尾巴算进今天）
        let rows = stats.report(t + Duration::from_secs(61), "2026-10-12");
        assert_eq!(rows, vec![], "换天应重置");

        // 新的一天继续正常累计
        let rows = stats.report(t + Duration::from_secs(90), "2026-10-12");
        assert_eq!(rows, vec![(Signal::Working, Duration::from_secs(29))]);
    }

    /// 第一次查询只该记下日期，不该把启动后累计的时间扔掉
    #[test]
    fn the_first_query_keeps_what_already_accumulated() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);

        // 启动后过了 5 分钟用户才第一次打开设置窗
        let rows = stats.report(t + Duration::from_secs(300), "2026-10-11");
        assert_eq!(rows, vec![(Signal::Working, Duration::from_secs(300))]);
    }

    #[test]
    fn zero_duration_states_are_not_reported() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        // 状态切了但没停留
        stats.record(t, Signal::Working);
        stats.record(t, Signal::Idle);
        stats.record(t, Signal::Permission);

        let rows = stats.report(t, "2026-10-11");
        assert!(rows.is_empty(), "零时长的状态不该出现在汇总里: {rows:?}");
    }

    /// 一个样本都没有时返回空 —— 别假装知道应用启动前是什么状态
    #[test]
    fn an_empty_timeline_has_no_segments() {
        let stats = Stats::default();
        let now = Instant::now();
        assert!(stats.timeline(now, Duration::from_secs(1800)).is_empty());
    }

    /// 窗口内的转换按时间顺序各自成段，首尾相接，总长正好等于覆盖区间
    #[test]
    fn transitions_inside_the_window_become_contiguous_segments() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);
        stats.record(t + Duration::from_secs(10), Signal::Idle);
        stats.record(t + Duration::from_secs(20), Signal::Error);

        let segs = stats.timeline(t + Duration::from_secs(30), Duration::from_secs(30));
        assert_eq!(
            segs,
            vec![
                Segment { state: Signal::Working, ms: 10_000 },
                Segment { state: Signal::Idle, ms: 10_000 },
                Segment { state: Signal::Error, ms: 10_000 },
            ]
        );
        assert_eq!(segs.last().unwrap().state, Signal::Error, "末段是最近记录的状态");
        // 无缝隙、无重叠：各段之和等于覆盖区间
        assert_eq!(segs.iter().map(|s| s.ms).sum::<u64>(), 30_000);
    }

    /// 比窗口还老的一段：整窗都是它，既不会被截成 0 也不会变成空
    #[test]
    fn a_state_older_than_the_window_fills_the_whole_window() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);

        let segs = stats.timeline(t + Duration::from_secs(600), Duration::from_secs(30));
        assert_eq!(segs, vec![Segment { state: Signal::Working, ms: 30_000 }]);
    }

    /// 起点早于窗口边界的状态：第一段在边界处被截断，总长仍是整整一个窗口
    #[test]
    fn the_first_segment_is_truncated_at_the_window_boundary() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working); // 这一段开始于窗口之外
        stats.record(t + Duration::from_secs(100), Signal::Idle);

        // 窗口 [t+90, t+120]：Working 的后 10 秒 + Idle 的前 20 秒
        let segs = stats.timeline(t + Duration::from_secs(120), Duration::from_secs(30));
        assert_eq!(
            segs,
            vec![
                Segment { state: Signal::Working, ms: 10_000 },
                Segment { state: Signal::Idle, ms: 20_000 },
            ]
        );
        assert_eq!(segs.iter().map(|s| s.ms).sum::<u64>(), 30_000);
    }

    /// 窗口把旧转换切掉时，接替它的那一段仍从边界开始，不留下空隙
    #[test]
    fn old_transitions_cut_off_by_the_window_leave_their_successor_at_the_boundary() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        stats.record(t, Signal::Working);
        stats.record(t + Duration::from_secs(10), Signal::Idle);
        stats.record(t + Duration::from_secs(20), Signal::Error);

        // 窗口 [t+15, t+30]：Working→Idle 的转换落在窗口外，Idle 从边界续上
        let segs = stats.timeline(t + Duration::from_secs(30), Duration::from_secs(15));
        assert_eq!(
            segs,
            vec![
                Segment { state: Signal::Idle, ms: 5_000 },
                Segment { state: Signal::Error, ms: 10_000 },
            ]
        );
    }

    /// 同一个状态反复上报只记一笔 —— 否则时间线会被无意义的重复刷满
    #[test]
    fn repeated_identical_states_add_only_one_sample() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        for i in 0..10 {
            stats.record(t + Duration::from_secs(i), Signal::Working);
        }
        assert_eq!(stats.timeline.len(), 1);
    }

    /// 时间线封顶：超出的最旧样本被丢掉，永远不超过容量，也不会 panic
    #[test]
    fn the_timeline_drops_the_oldest_entries_when_it_overflows() {
        let (mut stats, t) = (Stats::default(), Instant::now());
        // 两个状态交替，确保每一笔都真的被记下来
        for i in 0..(TIMELINE_CAPACITY + 50) {
            let state = if i % 2 == 0 { Signal::Working } else { Signal::Idle };
            stats.record(t + Duration::from_secs(i as u64), state);
        }
        assert_eq!(stats.timeline.len(), TIMELINE_CAPACITY);
        assert!(stats.timeline[0].0 > t, "最旧的样本应已被丢掉");
    }
}
