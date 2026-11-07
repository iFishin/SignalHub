import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import type { InsightEvent } from "./insights";
import type { ApprovalRecord } from "./useApprovalLog";

/** IPC 的硬上限。和 Rust `get_events` / `get_approval_history` 的 `min(5000)` 对齐。 */
const ENTRY_LIMIT = 5000;

/**
 * 「本周洞察」的两份数据源。都在打开「历史」页时拉一次 —— 读盘是低频动作，
 * 没必要跟着灯的状态推送一起刷新。
 */
export interface InsightData {
  events: InsightEvent[];
  approvals: ApprovalRecord[];
  sinceMs: number;
  days: number;
  /** 读盘失败要如实告诉界面：「读不到」不是「这段时间没有记录」。 */
  failed: boolean;
  /** 撞到上限：只拿到最近的一部分，下面的数字不能当全量结论。 */
  truncated: boolean;
}

export function useInsights(
  active: boolean,
  days = 7,
): { data: InsightData; refresh: () => void } {
  const [data, setData] = useState<InsightData>({
    events: [],
    approvals: [],
    sinceMs: 0,
    days,
    failed: false,
    truncated: false,
  });

  const refresh = useCallback(() => {
    if (!inTauri) return;
    const nowMs = Date.now();
    const sinceMs = nowMs - days * 86_400_000;
    Promise.all([
      invoke<InsightEvent[]>("get_events", { sinceMs, limit: ENTRY_LIMIT }),
      invoke<ApprovalRecord[]>("get_approval_history", { sinceMs, limit: ENTRY_LIMIT }),
    ])
      .then(([events, approvals]) =>
        setData({
          events,
          approvals,
          sinceMs,
          days,
          failed: false,
          // 正好等于上限就说明后面很可能还有；这时下面的数字偏小，得说出来。
          truncated: events.length >= ENTRY_LIMIT || approvals.length >= ENTRY_LIMIT,
        }),
      )
      .catch((err) => {
        console.error("读取本周洞察失败", err);
        setData((prev) => ({ ...prev, failed: true }));
      });
  }, [days]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh]);

  return { data, refresh };
}
