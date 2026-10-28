import type { Approval } from "./useApprovals";
import type { GroupMeta } from "./useSettings";
import type { GroupView, SignalState, Snapshot } from "./types";
import { labelOf } from "./lampSort";

/**
 * 收件箱的一行。
 *
 * **一行 = 一个会话**，不是一个组。组是灯上的画法；收件箱要回答的是「哪个会话在
 * 等我」，而一个组里可能同时跑着三个会话。
 */
export interface InboxRow {
  /** React key；同一会话在重排后不该丢选中态。 */
  key: string;
  session: string;
  /** 原始组名（静音 / 清除都按它匹配）。 */
  group: string;
  /** 给人看的组名（用户改的 > 服务端默认 > 原始名）。 */
  label: string;
  agent: string | null;
  state: SignalState;
  message: string | null;
  ageMs: number;
  cwd: string | null;
  /** 有这条时，这一行同时是一条待审批；行内会画允许 / 拒绝。 */
  approval?: Approval;
}

export interface InboxModel {
  /** 需要你：待审批 + permission / attention / error。 */
  need: InboxRow[];
  /** 在跑：thinking / working。 */
  running: InboxRow[];
  /** 刚刚完成：done（30 秒后自己消失）。 */
  done: InboxRow[];
  /** 空闲 / 沉默 / 离线 —— 收件箱折叠在最后，不抢注意力。 */
  quiet: InboxRow[];
  /** 需要你的条数（角标 / 标题用）。 */
  total: number;
}

/** 需要你处理的那几档。`stale` 不算：它只是「不说话」，不是「等你」。 */
const NEED_STATES: readonly SignalState[] = ["permission", "attention", "error"];

/** 需求内部的先后：出错 > 等你授权 > 需要关注。和灯的优先级同一套顺序。 */
const NEED_ORDER: Partial<Record<SignalState, number>> = {
  error: 3,
  permission: 2,
  attention: 1,
};

function isNeed(row: InboxRow): boolean {
  return row.approval !== undefined || NEED_STATES.includes(row.state);
}

function byAgeDesc(a: InboxRow, b: InboxRow): number {
  return b.ageMs - a.ageMs;
}

function labelFor(group: GroupView | undefined, meta: Record<string, GroupMeta>, fallback: string): string {
  return group ? labelOf(group, meta) : fallback;
}

/**
 * 把快照 + 待审批列表摊成收件箱的几段。
 *
 * **纯函数**：不订阅、不 invoke、不读 `Date.now()`。排序 / 去重 / 「一条审批只画
 * 一次」这些规则都在这里，单测能直接钉住；组件只负责画和按键。
 *
 * 去重规则：**同一个会话最多一行**。它既在快照里又有一条待审批时，合并成一行、
 * 带上审批 —— 否则用户会看到「同一个窗口」出现两次，一次能点允许、一次不能。
 */
export function buildInbox(
  snapshot: Snapshot,
  approvals: Approval[],
  meta: Record<string, GroupMeta>,
): InboxModel {
  const rows: InboxRow[] = [];
  const seen = new Set<string>();

  for (const group of snapshot.groups) {
    for (const session of group.sessions) {
      const approval = approvals.find((item) => item.session === session.id);
      rows.push({
        key: `s:${session.id}`,
        session: session.id,
        group: group.name,
        label: labelFor(group, meta, session.id),
        agent: session.agent,
        state: approval ? "permission" : session.state,
        message: session.message,
        ageMs: approval?.age_ms ?? session.age_ms,
        cwd: session.cwd,
        approval,
      });
      seen.add(session.id);
    }
  }

  // 审批登记表可能比会话多活一小会儿（会话刚被清掉、或者快照还没到）。
  // 这种条目仍然要显示 —— 它正是「有人在等你」的那一条。
  for (const approval of approvals) {
    if (seen.has(approval.session)) continue;
    rows.push({
      key: `a:${approval.id}`,
      session: approval.session,
      group: "",
      label: approval.session,
      agent: null,
      state: "permission",
      message: approval.target,
      ageMs: approval.age_ms,
      cwd: null,
      approval,
    });
  }

  const need = rows.filter(isNeed).sort((a, b) => {
    const left = NEED_ORDER[a.state] ?? 0;
    const right = NEED_ORDER[b.state] ?? 0;
    if (left !== right) return right - left;
    // 同类里等得最久的排最前 —— 那一条最可能已经被忘了。
    return byAgeDesc(a, b);
  });
  const running = rows
    .filter((row) => !isNeed(row) && (row.state === "working" || row.state === "thinking"))
    .sort(byAgeDesc);
  const done = rows
    .filter((row) => !isNeed(row) && row.state === "done")
    .sort((a, b) => a.ageMs - b.ageMs);
  const quiet = rows
    .filter(
      (row) =>
        !isNeed(row) &&
        row.state !== "working" &&
        row.state !== "thinking" &&
        row.state !== "done",
    )
    .sort(byAgeDesc);

  return { need, running, done, quiet, total: need.length };
}

/** 「等了多久」。收件箱里最多显示到天，不写秒 —— 它不是秒表。 */
export function formatAge(ageMs: number): string {
  const safe = Number.isFinite(ageMs) ? ageMs : 0;
  const seconds = Math.max(0, Math.floor(safe / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}
