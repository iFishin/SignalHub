import type { GroupMeta } from "./useSettings";
import type { GroupView } from "./types";

/**
 * 置顶排序的比较器。
 *
 * **必须是稳定的**（JS 的 `Array.prototype.sort` 从 ES2019 起保证稳定）：没置顶的组
 * 要原样保持服务端给的**紧急度**顺序，置顶组之间按权重、同权重时也保持原顺序 ——
 * 一条规则说清两件事。
 *
 * **与 Rust 的 `visible_groups` 里那个 `sort_by_key` 是同一条规则的两份实现。**
 * 两边必须算出同一批可见的牌，否则窗口高度会对不上：多算一块牌 → 底下空一条，
 * 少算一块 → 最后一块被 `overflow: hidden` 切掉。这也是它被从组件里摘出来、
 * 单独放一个文件并配单测的原因 —— 埋在 `SignalLamp.tsx` 里就没人会去测它。
 */
export function comparePinned(
  a: GroupView,
  b: GroupView,
  meta: Record<string, GroupMeta>,
): number {
  const left = meta[a.name]?.order ?? null;
  const right = meta[b.name]?.order ?? null;
  // 两边都没置顶：返回 0 让**稳定排序**把它们留在服务端给的紧急度顺序上
  if (left === right) return 0;
  if (left === null) return 1; // 没置顶的排到所有置顶组之后
  if (right === null) return -1;
  return left - right;
}

/**
 * 这一组在灯上叫什么。没设显示名就用上报方给的原始组名。
 *
 * 注意**只是显示**：隐藏 / 静音的匹配、排序、`/v1/state` 里的组名都仍然是原始名。
 */
export function labelOf(group: GroupView, meta: Record<string, GroupMeta>): string {
  return meta[group.name]?.name ?? group.name;
}

/**
 * 灯上真正会画出来的组：滤掉隐藏的，再叠一层用户的置顶偏好。
 *
 * 顺序和截断都要和 Rust 的 `visible_groups` 一致（见 `comparePinned` 的说明）。
 * 截断留给调用方 —— 这里只回答「按什么顺序排」。
 */
export function orderVisible(
  groups: GroupView[],
  hidden: string[],
  meta: Record<string, GroupMeta>,
): GroupView[] {
  return groups
    .filter((group) => !hidden.includes(group.name))
    .sort((a, b) => comparePinned(a, b, meta));
}
