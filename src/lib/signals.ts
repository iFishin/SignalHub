import { MESSAGES, type Lang } from "./i18n";
import type { SignalState } from "./types";

/** 状态名。灯球和设置面板共用，避免两处措辞不一致。 */
export function signalLabel(lang: Lang, state: SignalState): string {
  return MESSAGES[lang].signal[state];
}

/**
 * 「刚完成」还能报多久（毫秒）。
 *
 * 和任何 TTL 都无关 —— TTL 管的是「牌还在不在」，这个管的是「牌没了之后，那块
 * 位置还能说多久『刚完成』」。十分钟是「离开一会儿回来还看得到」的量级，再长就
 * 变成陈年旧事了（那时「没有智能体在上报」才是实话）。
 */
export const JUST_FINISHED_MS = 10 * 60_000;

/**
 * 空状态的那句话：刚干完活就说「刚完成 · N 分钟前」，否则退回「没有智能体在上报」。
 *
 * **只在「一个会话都没有」时调用。** 有会话时灯上每块牌各说各的，用不到这句。
 *
 * 为什么需要它：`done` 到期是被**整个删掉**的（不是转 `stale`），于是干完活三分钟
 * 之后灯上只剩一句「没有智能体在上报」—— 而「刚才那个任务完成了吗」恰恰是这个
 * 时刻最想问的问题。这份记忆由 Rust 侧 `Snapshot::last_done_ms` 提供，它比会话活得久。
 *
 * 返回 `null` 表示「没有刚完成这回事」，调用方自己退回 `signalLabel`。
 */
export function justFinishedLabel(
  lang: Lang,
  lastDoneMs: number,
  now: number,
): string | null {
  // 0 = 这个进程还没见过任何一次「完成」
  if (!lastDoneMs) return null;
  const elapsed = now - lastDoneMs;
  // 时钟回拨时不硬凑一个「-3 分钟前」，直接当作没有这回事
  if (elapsed < 0 || elapsed > JUST_FINISHED_MS) return null;
  return MESSAGES[lang].justFinished(Math.floor(elapsed / 60_000));
}
