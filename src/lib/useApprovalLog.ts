import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";

/**
 * 一条审批记录（Rust `approval_log::Record`，见 `docs/plan-approval.md` A1）。
 *
 * 字段名和 Rust 那份逐字对应 —— 这是序列化契约，改一边就要改另一边。
 */
export interface ApprovalRecord {
  ts_ms: number;
  session: string;
  agent: string;
  tool: string;
  target: string;
  detail: string;
  truncated: boolean;
  /** `allow` / `deny` / `expired` */
  decision: string;
  /** 拒绝理由的键；允许与超时为空 */
  reason: string;
  /** `lamp` / `api` / `web` / `system`（`system` = 超时或被顶掉，没经过任何界面） */
  source: string;
  waited_ms: number;
}

/**
 * 审批历史。**只在那一页打开时拉**（`active`）—— 它是一份读盘的东西，
 * 没必要在别的页签里跟着刷新。
 */
export function useApprovalLog(active: boolean): {
  records: ApprovalRecord[];
  refresh: () => void;
} {
  const [records, setRecords] = useState<ApprovalRecord[]>([]);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    invoke<ApprovalRecord[]>("get_approval_history", { limit: 100 })
      .then(setRecords)
      // 历史读不出来不该挡住别的：那一页显示空列表就行
      .catch((err) => console.error("get_approval_history 失败", err));
  }, []);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh]);

  return { records, refresh };
}
