import type { InboxModel, InboxRow } from "./inboxRows";

/**
 * 「离开多久才算离开」。
 *
 * 10 分钟：短于这个数回来，收件箱里那些行本来就是刚刚看到的，再顶一条摘要只是
 * 噪音。这个阈值只在**打开收件箱**时判一次，不会随时间自己冒出来。
 */
export const AWAY_MIN_MS = 10 * 60 * 1000;

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
}

/** 收件箱的全部行，顺序与界面一致。 */
export function allRows(model: InboxModel): InboxRow[] {
  return [...model.need, ...model.running, ...model.done, ...model.quiet];
}

/**
 * 按「上次看过的时间」切一刀，统计之后发生的变化。
 *
 * 判据是**每条会话当前状态的起点**（`now - ageMs`）晚于 `lastSeenMs` —— 也就是
 * 「这条在我离开之后才变成现在这样」。这样不需要任何事件日志：`ageMs` 已经由
 * 服务端算好，而收件箱本来就要它来显示「等了 3 分钟」。
 *
 * 三件事它**做不到**，写在这里免得以后被当成 bug：
 *
 * 1. 离开期间**出现过又消失**的会话看不到 —— 它们已经从快照里删掉了。要覆盖
 *    这一段得有一份事件流水，那是另一件事（见 plan-round5 的后续）。
 * 2. 离开期间状态变过好几次、最后又回到原样的，只按「最后一次变化」算一次。
 * 3. 用户从没打开过收件箱（`lastSeenMs` 为 `null`）时不猜 —— 直接返回 `null`，
 *    没有基准就没有「离开期间」。
 *
 * 返回 `null` 的两种情况：没有基准、以及离开不足 `AWAY_MIN_MS`。
 */
export function buildAwaySummary(
  model: InboxModel,
  nowMs: number,
  lastSeenMs: number | null,
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
  };

  for (const row of allRows(model)) {
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

  return summary;
}

/** 摘要里一条都不用说（离开期间什么都没变）。 */
export function isQuiet(summary: AwaySummary): boolean {
  return (
    summary.done === 0 &&
    summary.failed === 0 &&
    summary.need === 0 &&
    summary.running === 0
  );
}
