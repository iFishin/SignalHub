import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import { useVisibleRefresh } from "./useVisibleRefresh";
import type { StatRow } from "./useStats";

/** 与 Rust 侧 lib.rs 的 DayRow 对应。 */
export interface DayRow {
  /** 本地日期 `YYYY-MM-DD`，原样从 Rust 来 */
  day: string;
  /** 这一天的分状态时长，按时长从多到少 */
  rows: StatRow[];
  /**
   * 这一天各状态之和的秒数。
   *
   * 可以和是因为统计记的是**聚合状态占用**的时间：同一时刻只有一个聚合值，
   * 所以各区间的互不重叠，和就是「这一天灯亮着多久」。由 Rust 算好给过来，
   * 前端不自己加 —— 两份求和迟早会有一处忘了过滤零值。
   */
  total_seconds: number;
}

/** 一次看几天。落盘只留 30 天（`stats::KEEP_DAYS`），再多要也没有。 */
export const HISTORY_DAYS = 7;

/**
 * 最近若干天的每日合计，**由近及远**（今天在最前）。
 *
 * 只在挂载 / 手动 `refresh()` 时取：历史是按天变的，没有「看着它实时涨」的意义 ——
 * 定时轮询只是白费调用。今天的数会在下一次打开面板时刷新（`useVisibleRefresh`）。
 *
 * `today` 由**前端**给：Rust 那边只有 UTC（没有时区数据），而「今天」得按用户
 * 所在时区算。`toLocaleDateString("sv")` 给的是本地时区的 `YYYY-MM-DD`。
 */
export function useHistory(days = HISTORY_DAYS) {
  const [dayRows, setDayRows] = useState<DayRow[]>([]);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    const today = new Date().toLocaleDateString("sv");
    invoke<DayRow[]>("get_history", { today, days })
      .then(setDayRows)
      .catch((err) => console.error("get_history 失败", err));
  }, [days]);

  useEffect(refresh, [refresh]);
  useVisibleRefresh(refresh);

  return { days: dayRows, refresh };
}
