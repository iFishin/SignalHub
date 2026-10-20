import type { GroupView } from "./types";

/**
 * 「会议中」采集器的 session id —— 和 Rust 侧 `collect.rs::Collector::Meeting.id()`
 * 一致。组名跟着界面语言变（「会议」/「Meeting」）、用户还能给它改名，所以只认
 * id，不认名字。
 */
export const MEETING_SESSION_ID = "meeting";

/**
 * 会议组在灯上的颜色（B4 预设 3）。
 *
 * **不放进配色预设**：蓝色不表达状态（状态色里没有蓝），它表达的是「正在开会」。
 * 它不参与排序、不影响托盘、也不会发给物理灯和出站钩子 —— 那些地方认的仍是
 * 状态色。
 */
export const MEETING_BLUE = "#4c8dff";

/**
 * 这一组要不要按「会议」上色。
 *
 * 开关关着时永远返回 `undefined` —— 用户没开这个预设，蓝色就不该出现。
 * 判据是组里有没有那个固定 id 的会话：采集器每 5 秒重报一次，会议结束报 `done`
 * 之后会话还在（直到 TTL 到期），那几十秒里牌仍然算「会议」—— 蓝色淡出的时机
 * 和牌本身一致，不会出现「字还在、颜色先没了」。
 */
export function meetingTint(
  group: GroupView,
  enabled: boolean,
): string | undefined {
  if (!enabled) return undefined;
  return group.sessions.some((s) => s.id === MEETING_SESSION_ID)
    ? MEETING_BLUE
    : undefined;
}
