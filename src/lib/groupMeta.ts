import type { GroupMeta } from "./useSettings";

/**
 * 改某一组的**显示名**，返回新的 `group_meta`。
 *
 * 两处入口共用它：设置 →「外观」→「各组的显示」里的输入框，和展开的灯上那块牌
 * 就地改名（`SignalLamp.tsx`）。同一件事两条入口，各写一份算法迟早分叉 ——
 * 表现出来就是「灯上改完，设置里看着还是旧名字」。
 *
 * 三条规则：
 *
 * 1. **空字符串 = 清掉自定义名**（回到「跟随上报方给的原始组名」）。所以传空串
 *    不是「取一个空名字」，是删除。
 * 2. **`name` 为空的条目会被整条删掉**（如果它也没有 `order`）—— 一条什么都不
 *    写的条目对渲染没有任何影响，留在配置里只会让 `group_meta` 越来越长。
 * 3. **只动一个键**：其它组的偏好、以及这一组自己的 `order`（置顶）原样保留。
 *    删掉一个组名不该顺手把它从置顶里摘出来。
 *
 * 这里的 `group` 必须是**原始组名**，不是显示名 —— `group_meta` 的键是原始组名
 * （会话分组出来的组，键是那个会话 id）。拿显示名当键会给「build-1」建一条叫
 * 「build-1」的显示名，而牌上还是原来那个名字。
 */
export function withGroupName(
  meta: Record<string, GroupMeta>,
  group: string,
  name: string,
): Record<string, GroupMeta> {
  const next = { ...meta };
  const trimmed = name.trim();
  const current = next[group];
  const merged: GroupMeta = { ...current, name: trimmed === "" ? null : trimmed };

  if ((merged.name ?? null) === null && (merged.order ?? null) === null) {
    delete next[group];
  } else {
    next[group] = merged;
  }
  return next;
}

/**
 * 提交改名时判「用户到底动没动过」，动过才写设置。
 *
 * 编辑框被打开时填进框里的是**那一刻牌上显示的名字**（`labelOf` 的三级：用户起的
 * → 探测出来的 → 原始组名）。所以「点进来又点回去」提交上来的就是那个名字，必须
 * 当成没改过 —— 不判这一下的话，一次误点就把一个**探测出来的**名字钉成用户自定义
 * 名：之后上报方的项目 / 智能体变了，牌上还挂着旧的那个。
 *
 * `next` 为空（用户清空）同样要判：
 *
 * - 本来就跟随自动（`current` 也是空）时没什么可写；
 * - 而改过名之后又清掉，是想**回到自动**，那一笔要写（由 `withGroupName` 删掉它）。
 *
 * 两处入口（灯上就地改名、设置里的输入框）共用 —— 各自判一次迟早分叉，表现出来
 * 就是「灯上点一下不会写配置，设置里点一下却写了」。
 */
export function nameChanged(seed: string, next: string, current: string): boolean {
  return next !== seed && next !== current;
}
