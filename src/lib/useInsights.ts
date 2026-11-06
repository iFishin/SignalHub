import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import type { InsightEvent } from "./insights";
import type { ApprovalRecord } from "./useApprovalLog";

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
  });

  const refresh = useCallback(() => {
    if (!inTauri) return;
    const nowMs = Date.now();
    const sinceMs = nowMs - days * 86_400_000;
    Promise.all([
      invoke<InsightEvent[]>("get_events", { sinceMs, limit: 5000 }),
      invoke<ApprovalRecord[]>("get_approval_history", { sinceMs, limit: 5000 }),
    ])
      .then(([events, approvals]) =>
        setData({ events, approvals, sinceMs, days, failed: false }),
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
