import type { GroupView } from "./types";

/**
 * 「会议中」采集器的 session id —— 和 Rust 侧 `collect.rs::Collector::Meeting.id()`
 * 一致。组名跟着界面语言变（「会议」/「Meeting」）、用户还能给它改名，所以只认
 * id，不认名字。
 */
export const MEETING_SESSION_ID = "meeting";

/**
 * 采集器的 agent 名 —— 和 Rust 侧 `collect::AGENT` 一致。
 *
 * 光认 session id 不够：用户完全可能手写一条 `--session meeting` 的上报。
 * 采集器报的是 `system`，智能体会话报的是 `claude-code` 之类。
 */
export const SYSTEM_AGENT = "system";

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
 *
 * 判据是「那个固定 id + `system` 信号源的会话还在 `working`」：采集器散会时会报
 * 一条 `done`（「会议结束」），那条要按 done 的 TTL 在灯上留一会儿 —— 如果继续
 * 画蓝，看起来就像会还没散。Rust 侧的静音判据（`rules.rs::meeting_running`）也用
 * 同一个条件，两边必须一致：蓝色和静音是同一件事的两种表现。
 */
export function meetingTint(
  group: GroupView,
  enabled: boolean,
): string | undefined {
  if (!enabled) return undefined;
  return group.sessions.some(
    (s) =>
      s.id === MEETING_SESSION_ID &&
      s.agent === SYSTEM_AGENT &&
      s.state === "working",
  )
    ? MEETING_BLUE
    : undefined;
}
