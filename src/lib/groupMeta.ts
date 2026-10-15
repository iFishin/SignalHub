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
