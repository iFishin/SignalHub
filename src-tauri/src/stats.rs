//! 状态时长统计。
//!
//! 按「前端给的本地日期」分桶 —— **本地日期只有前端知道**（Rust 这边没有
//! 时区数据，`SystemTime` 只给 UTC），所以由前端在查询时带过来；日期一变
//! 就重置。这样不必引入时区依赖，也不用猜用户在哪一天。

use crate::state::Signal;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 时间线最多记住多少次状态变化。超过就丢最旧的 —— 常驻应用跑一周也不该
/// 无限增长。
const TIMELINE_CAPACITY: usize = 512;

#[derive(Debug, Default)]
pub struct Stats {
    /// 前端上次给的本地日期，用来判断是否跨天
    day: String,
    /// 已结算的各状态累计时长
    totals: HashMap<Signal, Duration>,
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

    /// 跨天就结清：累计清零，当前这一段重新起算。返回是否真的跨了天。
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
        self.day = today.to_string();

        // 第一次拿到日期只记下来，**不清空** —— 否则从启动到第一次拿到日期之间累计的
        // 时间会被白扔，而那段可能相当长（启动到前端第一次推时钟之间）
        if first {
            return false;
        }

        self.totals.clear();
        // 跨天时当前这段也重新起算，否则会把昨天那段算进今天
        if self.current.is_some() {
            self.segment_start = Some(now);
        }
        true
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
        // 时长相同（例如都是 5 秒）时再按优先级排 —— 否则顺序取决于 HashMap
        // 的迭代顺序，界面每次刷新都可能换个次序
        rows.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| b.0.priority().cmp(&a.0.priority()))
        });
        rows
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
