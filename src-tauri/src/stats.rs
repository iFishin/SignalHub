//! 状态时长统计。
//!
//! 按「前端给的本地日期」分桶 —— **本地日期只有前端知道**（Rust 这边没有
//! 时区数据，`SystemTime` 只给 UTC），所以由前端在查询时带过来；日期一变
//! 就重置。这样不必引入时区依赖，也不用猜用户在哪一天。

use crate::state::Signal;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
}

impl Stats {
    /// 记一次聚合变化：结算上一段，然后开始新的一段。
    pub fn record(&mut self, now: Instant, aggregate: Signal) {
        self.close_segment(now);
        self.current = Some(aggregate);
        self.segment_start = Some(now);
    }

    /// 取统计，按时长从多到少。
    ///
    /// `today` 是前端给的本地日期；和上次不同就重置（跨天了）。
    /// 会把**正在进行的那一段**也算进去，所以不必等状态变化就能看到当前进度。
    pub fn report(&mut self, now: Instant, today: &str) -> Vec<(Signal, Duration)> {
        if self.day.is_empty() {
            // 第一次拿到日期只记下来，**不清空** —— 否则从启动到第一次查询之间
            // 累计的时间会被白扔（而第一次查询往往发生在用户打开设置窗时）
            self.day = today.to_string();
        } else if self.day != today {
            self.day = today.to_string();
            self.totals.clear();
            // 跨天时当前这段也重新起算，否则会把昨天那段算进今天
            if self.current.is_some() {
                self.segment_start = Some(now);
            }
        }

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
}
