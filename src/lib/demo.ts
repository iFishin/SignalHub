/**
 * 演示模式的「剧本」。
 *
 * 这里只放**数据**，不放界面：每一步是「灯上该有几块牌、各是什么状态」，
 * 文案在 i18n、画面在 DemoStage。拆开是因为这段逻辑要能单测 —— 演示最怕的
 * 不是不好看，是某一步画出了和产品实际行为不一致的东西（比如把「完成」
 * 画成转沉默），那会变成一份会骗人的说明书。
 *
 * 三条硬约束：
 * - 每一步的 `aggregate` 必须等于各组里优先级最高的那个（红 > 黄 > 绿），
 *   和 Rust 的聚合规则一致；`demoAggregate()` 就是用来在测试里盯住它的。
 * - 组按紧急度排好（最急的在前），前端不会再排一次。
 * - 「刚完成」那一步 `rows` 必须是空的：真实产品里 `done` 到期是**删除**，
 *   不是转沉默，空状态那句话正是靠 `last_done_ms` 记住的。
 */
import type { GroupView, SignalState } from "./types";

/** 演示里出现的组。名字是稳定的 id，显示名走 i18n（中英各一份）。 */
export type DemoGroupId = "build" | "api" | "review";

export interface DemoRow {
  id: DemoGroupId;
  state: SignalState;
  message?: string;
}

export interface DemoStep {
  /** i18n 里 `demoStep.<id>` 的键；同时用作 React key。 */
  id: string;
  durationMs: number;
  aggregate: SignalState;
  rows: DemoRow[];
  /** 空状态演示「刚完成」时距现在多久；不填 = 不记住刚完成。 */
  lastDoneAgoMs?: number;
}

/** 每一步停多久。整轮约 7 × 3.2s ≈ 22 秒，够看完又不烦。 */
export const DEMO_STEP_MS = 3200;

export const DEMO_STEPS: DemoStep[] = [
  {
    id: "idle",
    durationMs: DEMO_STEP_MS,
    aggregate: "idle",
    rows: [{ id: "build", state: "idle" }],
  },
  {
    id: "thinking",
    durationMs: DEMO_STEP_MS,
    aggregate: "thinking",
    rows: [{ id: "build", state: "thinking" }],
  },
  {
    id: "working",
    durationMs: DEMO_STEP_MS,
    aggregate: "working",
    rows: [
      { id: "build", state: "working" },
      { id: "api", state: "working" },
      { id: "review", state: "thinking" },
    ],
  },
  {
    id: "permission",
    durationMs: DEMO_STEP_MS + 800,
    aggregate: "permission",
    // 最急的排最前：红色那块不会被后面两颗绿灯顶掉。
    rows: [
      { id: "review", state: "permission" },
      { id: "build", state: "working" },
      { id: "api", state: "working" },
    ],
  },
  {
    id: "approved",
    durationMs: DEMO_STEP_MS,
    aggregate: "working",
    rows: [
      { id: "build", state: "working" },
      { id: "api", state: "working" },
      { id: "review", state: "working" },
    ],
  },
  {
    id: "done",
    durationMs: DEMO_STEP_MS,
    aggregate: "done",
    rows: [
      { id: "build", state: "done" },
      { id: "api", state: "done" },
      { id: "review", state: "done" },
    ],
  },
  {
    id: "empty",
    durationMs: DEMO_STEP_MS + 1200,
    aggregate: "offline",
    rows: [],
    lastDoneAgoMs: 60_000,
  },
];

/** 优先级顺序，和 Rust 侧聚合用的是同一条：红 > 黄 > 绿 > 灰。 */
const PRIORITY: Record<SignalState, number> = {
  error: 6,
  permission: 5,
  attention: 4,
  working: 3,
  thinking: 2,
  done: 1,
  idle: 1,
  stale: 0,
  offline: 0,
  off: 0,
};

/** 一步里各组的最高优先级状态。测试用它校验 `aggregate` 没写错。 */
export function demoAggregate(step: DemoStep): SignalState {
  if (step.rows.length === 0) return "offline";
  return step.rows.reduce<SignalState>(
    (worst, row) => (PRIORITY[row.state] > PRIORITY[worst] ? row.state : worst),
    step.rows[0].state,
  );
}

/** 下一步的下标，走到末尾回到开头 —— 演示是循环的，不需要「结束页」。 */
export function nextDemoStep(index: number): number {
  return (index + 1) % DEMO_STEPS.length;
}

/** 把一步变成灯牌能直接吃的形状。 */
export function demoSnapshot(
  index: number,
  now: number,
  labels: Record<DemoGroupId, string>,
): { groups: GroupView[]; aggregate: SignalState; lastDoneMs: number } {
  const step = DEMO_STEPS[index];
  const groups: GroupView[] = step.rows.map((row) => ({
    name: row.id,
    default_label: labels[row.id],
    aggregate: row.state,
    message: row.message ?? null,
    // 固定「6 秒前进入当前状态」，牌上的「已持续」会自己往上走
    since_ms: now - 6_000,
    stale_progress: null,
    sessions: [],
  }));
  return {
    groups,
    aggregate: step.aggregate,
    lastDoneMs: step.lastDoneAgoMs == null ? 0 : now - step.lastDoneAgoMs,
  };
}
