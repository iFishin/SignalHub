import { useMemo } from "react";
import type { Approval } from "../lib/useApprovals";
import { useMessages } from "../lib/i18n";
import { useNow } from "../lib/useNow";

/**
 * 灯上一叠「等着你点」的气泡（从灯上审批，见 `approvals.rs`）。
 *
 * 几件事是刻意的：
 *
 * - **最多画 `MAX_APPROVAL_BUBBLES` 条**，和 Rust 的 `approval_area` 是同一个数。
 *   多出来的不画也不占地方 —— 它们不会丢（那几条的牌在灯上是黄的），答完一条
 *   下一条自己冒出来。
 * - **「允许」在看不全的时候不画**。长命令只能拒绝、或者回终端自己看：在灯上
 *   放行一条自己没看全的命令，正是这个功能最不该做的事。
 * - 命令全文**只在这里出现**。状态上报用的是那个短词，通知和出站钩子都不带它。
 */
export const MAX_APPROVAL_BUBBLES = 3;

interface Props {
  approvals: Approval[];
  onDecide: (id: string, decision: "allow" | "deny") => void;
}

export function ApprovalBubble({ approvals, onDecide }: Props) {
  const m = useMessages();
  // `age_ms` 是 Rust 在**拉列表那一刻**算的快照，之后不会自己变（列表只在登记 /
  // 决定 / 过期时重推）。于是「已等待 12 秒」会一直停在收到它的那一刻 —— 而这条
  // 数字唯一的用处就是「还要等多久」。用 `useNow` 每秒自己往前推。
  const now = useNow(true);
  // 收到一条时反推出它的开始时刻，之后按 now 算年龄；列表一刷新就跟着重算。
  const startedAt = useMemo(
    () => new Map(approvals.map((approval) => [approval.id, Date.now() - approval.age_ms])),
    [approvals],
  );

  if (approvals.length === 0) return null;

  const shown = approvals.slice(0, MAX_APPROVAL_BUBBLES);
  return (
    <div className="approvals">
      {shown.map((approval) => (
        <div className="approval" key={approval.id}>
          <div className="approval-head">
            <span className="approval-tool">{approval.tool}</span>
            <span className="approval-session" title={approval.session}>
              {approval.session}
            </span>
            <span>
              {m.approvalWaiting(
                Math.round(Math.max(0, now - (startedAt.get(approval.id) ?? now)) / 1000),
              )}
            </span>
          </div>
          {/* 命令全文 / 文件路径 / 输入 JSON。`pre-wrap` + 滚动（高度是固定的，
              见 lamp.css —— 不能让它把窗口撑开）。 */}
          <div className="approval-detail">
            {approval.detail}
          </div>
          {!approval.allowable && <div className="approval-warn">{m.approvalTooLong}</div>}
          <div className="approval-buttons">
            {/* 「看不全」时这个按钮**不画**：规则在 Rust 里挡着，这里只是不给它
                一个点了没反应的地方。 */}
            {approval.allowable && (
              <button
                type="button"
                className="approval-btn"
                data-decision="allow"
                data-state="working"
                onClick={() => onDecide(approval.id, "allow")}
              >
                {m.approvalAllow}
              </button>
            )}
            <button
              type="button"
              className="approval-btn"
              onClick={() => onDecide(approval.id, "deny")}
            >
              {m.approvalDeny}
            </button>
          </div>
        </div>
      ))}
      {approvals.length > shown.length && (
        <div className="approval-warn">
          {m.approvalMore(approvals.length - shown.length)}
        </div>
      )}
    </div>
  );
}
