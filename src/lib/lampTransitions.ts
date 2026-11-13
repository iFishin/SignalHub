import type { GroupView } from "./types";

/**
 * 上一帧每组的**可见签名**。键是**原始组名**（和 `GroupView.name` 同一套）。
 *
 * 签名是 `aggregate`，推迟期间再挂一个后缀（见 [`snoozeSignature`]）。存成字符串
 * 而不是 `SignalState`，是因为「同一块牌刚被推迟 / 刚解除推迟」在观感上也是一次
 * 跃迁（牌从满强度掉到淡，或者反过来）—— 那两下都该闪一下，理由和换状态一样：
 * 让用户知道刚才那一下生效了。详见 `transitions`。
 */
export type SignSignature = Record<string, string>;

/**
 * 推迟在签名里的样子。**只影响「要不要闪」，不影响任何判定。**
 *
 * 单独一个函数是为了让 `SignalLamp` 和单测拼出同一个字符串：两处各拼一次，
 * 某天改了一处就会出现「推迟时不闪、取消推迟时闪」这种半套反馈，而那只有
 * 盯着灯看的人才会发现。
 */
export function snoozeSignature(snoozed: boolean): string {
  return snoozed ? "+snooze" : "";
}

/**
 * 两帧之间，哪些组的签名变了。
 *
 * 这是「跃迁脉冲」的判据：哪块牌刚换了状态（或者刚被推迟 / 刚解除推迟）就闪一下
 * （`SignalLamp` 把它翻成 `data-pulse`）。抽成纯函数是为了能单测 —— 首帧不闪、
 * 连续同状态不闪，这两条靠肉眼看是看不出漏没漏的。
 *
 * **为什么在前端做、也不用动 Rust**：`Snapshot.groups` 里每一组的聚合是从各会话
 * 的状态派生的，所以任何会话的状态变化本来就一定会推一帧过来（见 `state.rs` 的
 * `Visible`）。前端拿一个 ref 记住上一帧、两帧一比就知道谁变了，不需要服务端再
 * 下发一条「谁跃迁了」。
 *
 * `snoozed` 是「这一组此刻是否在推迟里」的判定，由调用方给（它要 `now`，而这个
 * 函数刻意不读时钟）。**到点解除推迟那一下也算跃迁**：牌从淡的、不闪的一档回到
 * 满强度，正是用户说「过十分钟再说」之后要的那个提示 —— 不额外发通知，就靠这一闪。
 *
 * `changed` 是**组名**列表。只有「上一帧就存在、且签名不同」的组才算：
 *
 * - **首次出现不算**（不在 `previous` 里）。那更接近「灯刚亮起来」，不该闪；
 *   顺带也就保证了**首帧不闪** —— ref 初值是空表，第一帧没有任何组「变了」。
 * - 一个组从 `previous` 里消失后（本轮 `groups` 没有它），它也就不再被记住；
 *   下次再出现同样当作「首次」，不闪。
 */
export function transitions(
  previous: SignSignature,
  groups: GroupView[],
  snoozed: (group: GroupView) => boolean = () => false,
): { changed: string[]; next: SignSignature } {
  const changed: string[] = [];
  const next: SignSignature = {};
  for (const group of groups) {
    const signature = group.aggregate + snoozeSignature(snoozed(group));
    next[group.name] = signature;
    if (group.name in previous && previous[group.name] !== signature) {
      changed.push(group.name);
    }
  }
  return { changed, next };
}
