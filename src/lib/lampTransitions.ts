import type { GroupView, SignalState } from "./types";

/** 上一帧每组的聚合值。键是**原始组名**（和 `GroupView.name` 同一套）。 */
export type AggregateMap = Record<string, SignalState>;

/**
 * 两帧之间，哪些组的聚合状态**跃迁**了。
 *
 * 这是「跃迁脉冲」的判据：哪块牌刚换了状态就闪一下（`SignalLamp` 把它翻成
 * `data-pulse`）。抽成纯函数是为了能单测 —— 首帧不闪、连续同状态不闪，这两条
 * 靠肉眼看是看不出漏没漏的。
 *
 * **为什么在前端做、也不用动 Rust**：`Snapshot.groups` 里每一组的聚合是从各会话
 * 的状态派生的，所以任何会话的状态变化本来就一定会推一帧过来（见 `state.rs` 的
 * `Visible`）。前端拿一个 ref 记住上一帧、两帧一比就知道谁变了，不需要服务端再
 * 下发一条「谁跃迁了」。
 *
 * `changed` 是**组名**列表。只有「上一帧就存在、且聚合值不同」的组才算：
 *
 * - **首次出现不算**（不在 `previous` 里）。那更接近「灯刚亮起来」，不该闪；
 *   顺带也就保证了**首帧不闪** —— ref 初值是空表，第一帧没有任何组「变了」。
 * - 一个组从 `previous` 里消失后（本轮 `groups` 没有它），它也就不再被记住；
 *   下次再出现同样当作「首次」，不闪。
 */
export function transitions(
  previous: AggregateMap,
  groups: GroupView[],
): { changed: string[]; next: AggregateMap } {
  const changed: string[] = [];
  const next: AggregateMap = {};
  for (const group of groups) {
    next[group.name] = group.aggregate;
    if (group.name in previous && previous[group.name] !== group.aggregate) {
      changed.push(group.name);
    }
  }
  return { changed, next };
}
