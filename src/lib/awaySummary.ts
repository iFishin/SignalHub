import type { InboxModel, InboxRow } from "./inboxRows";

/**
 * 「离开多久才算离开」。
 *
 * 10 分钟：短于这个数回来，收件箱里那些行本来就是刚刚看到的，再顶一条摘要只是
 * 噪音。这个阈值只在**打开收件箱**时判一次，不会随时间自己冒出来。
 */
export const AWAY_MIN_MS = 10 * 60 * 1000;

/**
 * Rust `events::Event` 里摘要用得到的那几个字段（其余字段这里不关心，也不该
 * 因为后端加了个字段就改这一份类型）。
 *
 * `end` 的 `state` 是**最后已知状态**，不是「结束」这个状态 —— 见 `events.rs`。
 */
export interface AwayEvent {
  ts_ms: number;
  /** `start` / `change` / `end` */
  kind: string;
  session: string;
  state: string;
}

/**
 * 「你不在的时候发生了什么」。
 *
 * 只有**计数**和「等得最久的那一条」，没有逐条列表 —— 列表就是收件箱本身，
 * 再列一遍等于把同一份内容画两次。
 */
export interface AwaySummary {
  /** 离开了多久（毫秒）。 */
  awayMs: number;
  /** 这段时间里变成 `done` 的。 */
  done: number;
  /** 这段时间里变成 `error` 的。 */
  failed: number;
  /** 此刻在等你（授权 / attention）的。 */
  need: number;
  /** 此刻还在跑的。 */
  running: number;
  /** 等你的事里等得最久的那个（毫秒）；没有就是 0。 */
  longestWaitMs: number;
  /** 离开期间跑完、现在已经从快照里消失的会话数。 */
  transientDone: number;
  /** 离开期间失败、现在已经从快照里消失的会话数。 */
  transientFailed: number;
}

/** 收件箱的全部行，顺序与界面一致。 */
export function allRows(model: InboxModel): InboxRow[] {
  return [...model.need, ...model.running, ...model.done, ...model.quiet];
}

/**
 * 按「上次看过的时间」切一刀，统计之后发生的变化。
 *
 * **两份数据各管一半**，合起来才是「离开期间发生过什么」：
 *
 * - 快照管「现在还在的」：判据是每条会话当前状态的起点（`now - ageMs`）晚于
 *   `lastSeenMs`。`ageMs` 由服务端算好，收件箱本来就要它显示「等了 3 分钟」。
 * - 事件流水管「已经不在的」：`end` 事件里那些时间戳落在窗口内、且此刻快照里
 *   找不到同 id 的会话。第一版只有前一半，所以「跑完就被删掉的会话」完全看不见。
 *
 * 两份不重复计数：快照里还看得见的 session 一律不进 transient，哪怕流水里也有
 * 它的 `end`（同 id 被复用时会出现这种情况）。事件按 session 去重、取最新一条
 * `end` —— 摘要按「几个会话」说话，不按「几次状态变化」说话。
 *
 * 剩下两件它**做不到**，写在这里免得以后被当成 bug：
 *
 * 1. 离开期间状态变过好几次、最后又回到原样的，只按「最后一次变化」算一次。
 * 2. 用户从没打开过收件箱（`lastSeenMs` 为 `null`）时不猜 —— 直接返回 `null`，
 *    没有基准就没有「离开期间」。
 *
 * `events` 读不到（磁盘坏、旧版本 Rust）时传空数组即可：摘要安静地退回第一版
 * 的行为，不是把整块摘要藏起来。
 *
 * 返回 `null` 的两种情况：没有基准、以及离开不足 `AWAY_MIN_MS`。
 */
export function buildAwaySummary(
  model: InboxModel,
  nowMs: number,
  lastSeenMs: number | null,
  events: AwayEvent[] = [],
): AwaySummary | null {
  if (lastSeenMs === null || !Number.isFinite(lastSeenMs)) return null;
  const awayMs = nowMs - lastSeenMs;
  if (!(awayMs >= AWAY_MIN_MS)) return null;

  const summary: AwaySummary = {
    awayMs,
    done: 0,
    failed: 0,
    need: 0,
    running: 0,
    longestWaitMs: 0,
    transientDone: 0,
    transientFailed: 0,
  };

  const rows = allRows(model);
  for (const row of rows) {
    // 状态起点早于上次看过 → 这条在离开前就是这个样子，不算「离开期间发生的事」。
    if (nowMs - row.ageMs <= lastSeenMs) continue;
    switch (row.state) {
      case "done":
        summary.done += 1;
        break;
      case "error":
        summary.failed += 1;
        break;
      case "permission":
      case "attention":
        summary.need += 1;
        summary.longestWaitMs = Math.max(summary.longestWaitMs, row.ageMs);
        break;
      case "thinking":
      case "working":
        summary.running += 1;
        break;
      default:
        // idle / stale / offline 不计数：它们说的是「没在干活」，不是「发生了什么」。
        break;
    }
  }

  // 已经结束、从收件箱里消失的那些。`events` 由 Rust 按时间倒序给回来，
  // 所以同一个 session 第一次遇到的 `end` 就是它最新的一次。
  const visible = new Set(rows.map((row) => row.session));
  const seen = new Set<string>();
  for (const event of events) {
    if (event.kind !== "end" || seen.has(event.session)) continue;
    if (!(event.ts_ms > lastSeenMs && event.ts_ms <= nowMs)) continue;
    if (visible.has(event.session)) continue;
    seen.add(event.session);
    if (event.state === "done") summary.transientDone += 1;
    else if (event.state === "error") summary.transientFailed += 1;
    // 其余状态（idle / stale / working …）不算「发生过什么」：和上面快照那一半
    // 用同一个判据，别让两份数据说出两套话。
  }

  return summary;
}

/** 摘要里一条都不用说（离开期间什么都没变）。 */
export function isQuiet(summary: AwaySummary): boolean {
  return (
    summary.done === 0 &&
    summary.failed === 0 &&
    summary.need === 0 &&
    summary.running === 0 &&
    summary.transientDone === 0 &&
    summary.transientFailed === 0
  );
}
