import type { Settings } from "./useSettings";
import type { GroupView } from "./types";

/**
 * 同时存在的组灯窗数量上限。
 *
 * **必须和 Rust 侧 `lamps::MAX_LAMPS` 一致**：超过这个数两边的回落决定会不一致 ——
 * Rust 让各组仍画在主灯上，前端却以为主灯只画全局（或反过来），窗口尺寸立刻对不上。
 */
export const MAX_LAMPS = 8;

/**
 * 这一帧是不是「每组分灯」。
 *
 * 和 Rust 侧 `per_group_active` 是**同一条规则的两个实现**：布局选的是 per-group，
 * 且可见组数没超过硬上限。超过就回落单灯。
 *
 * 可见 = 没被用户隐藏（`hidden_groups`），和 Rust 的 `shows_group` 逐字一致。
 */
export function perGroupActive(settings: Settings, groups: GroupView[]): boolean {
  if (settings.lamp_layout !== "per-group") return false;
  const visible = groups.filter((group) => !settings.hidden_groups.includes(group.name));
  return visible.length <= MAX_LAMPS;
}
