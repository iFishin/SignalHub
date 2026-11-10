/**
 * 演示模式的「剧本」。
 *
 * 这里只放**数据**，不放界面：每一步是「灯上该有几块牌、各是什么状态」，
 * 文案在 i18n、画面在 DemoStage。拆开是因为这段逻辑要能单测 —— 演示最怕的
 * 不是不好看，是某一步画出了和产品实际行为不一致的东西（比如把「完成」
 * 画成转沉默），那会变成一份会骗人的说明书。
 *
 * **为什么分成场景而不是一条长剧本**：一盏灯的真实用法有五种互不相干的样子
 * —— 平时干活、有事要你、它不说话了、收起来、以及「用光说话」（边灯 / 角灯）。
 * 串成一条 20 步的线性剧本，
 * 想看「折叠长什么样」得先等 40 秒；而按场景切开，每个场景自己循环，点一下
 * 就能反复看同一件事。
 *
 * 四条硬约束：
 * - 每一步的 `aggregate` 必须等于各组里优先级最高的那个（红 > 黄 > 绿），
 *   和 Rust 的聚合规则一致；`demoAggregate()` 就是用来在测试里盯住它的。
 * - 组按紧急度排好（最急的在前），前端不会再排一次。
 * - 「刚完成」那一步 `rows` 必须是空的：真实产品里 `done` 到期是**删除**，
 *   不是转沉默，空状态那句话正是靠 `last_done_ms` 记住的。
 * - **每一步都要能被真实产品做出来**。`collapsed` / `quiet` 都是设置里真有的
 *   开关，`stale_progress` 用的是 `state.rs` 那套真实比例，没有一个字段是
 *   只为演示编出来的。
 */
import type { GroupView, SignalState } from "./types";
import type { Shape, SignCorners, SignEdge, LightTrigger } from "./useSettings";

/** 演示里出现的组。名字是稳定的 id，显示名走 i18n（中英各一份）。 */
export type DemoGroupId = "build" | "api" | "review";

/**
 * 「用光说话」那一场里同时被置顶 + 静音的那块牌。
 *
 * 角灯的 `pinned` / `muted` 两枚靠 `group_meta` 与 `muted_groups` 判定，而这两个键
 * 用的是**组 id**（`build`），不是显示名。DemoStage 要按同一条 id 把这两个事实喂给
 * 灯，所以 id 只能有一处出处 —— 从剧本里念一遍字符串，两边迟早对不上。
 */
export const DEMO_CORNER_GROUP: DemoGroupId = "build";

/** 场景 id。文案在 i18n 的 `demoScenario`。 */
export type DemoScenarioId = "day" | "asking" | "silence" | "tucked" | "look";


export interface DemoRow {
  id: DemoGroupId;
  state: SignalState;
  message?: string;
  /**
   * 静默进度（0..1，1 = 刚到静默，0 = 马上消失）。
   *
   * 只有 `stale` 组才有 —— 和 `state.rs::stale_progress` 是同一个含义。
   * **只有圆环形状画得出来**（`ring` 的弧长就是它，见 lamp.css）；默认的实心点
   * 没有地方承载进度，给了也不显示。
   */
  staleProgress?: number;
}

export interface DemoStep {
  /** i18n 里 `demoStep.<id>` 的键；同时用作 React key。 */
  id: string;
  durationMs: number;
  aggregate: SignalState;
  rows: DemoRow[];
  /** 空状态演示「刚完成」时距现在多久；不填 = 不记住刚完成。 */
  lastDoneAgoMs?: number;
  /** 这一步整叠牌收起成一颗灯（就是单击折叠之后的样子）。 */
  collapsed?: boolean;
  /** 这一步开着安静模式（空闲 / 静默会压到几乎透明）。 */
  quiet?: boolean;
  /**
   * 这一步用哪种点形状。**只为「圆环 + 静默进度」那一步存在**：默认的实心点
   * 没有地方画进度，细环是 `ring` 形状才有的表达（见 lamp.css 的
   * `.dot[data-shape="ring"]`）。不填 = 跟随用户设置。
   */
  shape?: Shape;
  /**
   * 这一步把边灯换成哪一档。不填 = 跟随用户设置。
   *
   * 只有「用光说话」那个场景会填 —— 那一场讲的就是这四个档位，跟着用户的设置走
   * 就等于什么都没讲。
   */
  edge?: SignEdge;
  /** 这一步四角各挂什么。不填 = 跟随用户设置。 */
  corners?: SignCorners;
  /** 这一步把那块牌当成「被置顶」（角灯的 `pinned` 判据）。 */
  pinned?: boolean;
  /** 这一步把那块牌当成「被静音」（角灯的 `muted` 判据）。 */
  muted?: boolean;
  /**
   * 这一步把边灯的触发方式换成哪一档。不填 = 常亮。
   *
   * 只有「用光说话」最后那一步会填：让两块状态不同的牌并排，只有需要你动手的
   * 那块亮边 —— 触发方式这种「什么时候亮」的事，光靠一行设置说明讲不清楚。
   */
  edgeTrigger?: LightTrigger;
  /** 这一步角灯的触发方式。不填 = 常亮（和边灯各判各的，见「用光说话」倒数第二步）。 */
  cornerTrigger?: LightTrigger;
  /** 「达到指定状态」那一档挑哪个状态。不填 = DemoStage 给 `permission`。 */
  triggerState?: SignalState;
  /** 这一步假装在点击穿透，让 `passthrough` 角灯有条件亮。 */
  passThrough?: boolean;
}

export interface DemoScenario {
  id: DemoScenarioId;
  steps: DemoStep[];
}

/** 每一步停多久。场景最长 7 步 ≈ 22 秒，最短 2 步 ≈ 7 秒。 */
export const DEMO_STEP_MS = 3200;

export const DEMO_SCENARIOS: DemoScenario[] = [
  {
    // 原来那条线性剧本，一步没改：它讲的是「一天里这盏灯怎么呼吸」。
    id: "day",
    steps: [
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
    ],
  },
  {
    // 这一组只讲「黄和红各是什么意思、怎么一眼分开」。
    id: "asking",
    steps: [
      {
        id: "attention",
        durationMs: DEMO_STEP_MS,
        aggregate: "attention",
        rows: [
          { id: "review", state: "attention" },
          { id: "build", state: "working" },
          { id: "api", state: "working" },
        ],
      },
      {
        id: "both",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "permission",
        // 红 > 黄 > 绿，排序就是该处理的顺序。
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "attention" },
          { id: "api", state: "working" },
        ],
      },
      {
        id: "error",
        durationMs: DEMO_STEP_MS,
        aggregate: "error",
        rows: [
          { id: "build", state: "error" },
          { id: "api", state: "working" },
        ],
      },
    ],
  },
  {
    // 「没消息」和「空闲」不是一回事 —— 这条是产品最容易被误解的地方。
    id: "silence",
    steps: [
      {
        id: "stale",
        durationMs: DEMO_STEP_MS + 600,
        aggregate: "stale",
        // 只剩一组，聚合才会是 stale —— 混一组 working 的话整灯仍是绿的，
        // 那块灰牌要靠眼睛在牌里找，讲不清「沉默」这件事。
        rows: [{ id: "build", state: "stale" }],
      },
      {
        id: "staleRing",
        durationMs: DEMO_STEP_MS + 1000,
        aggregate: "stale",
        // 进度只有圆环形状画得出来（默认的实心点没有地方承载它），
        // 所以这一步**只在演示里**换成 ring，并明说这是外观页可选项。
        rows: [{ id: "build", state: "stale", staleProgress: 0.72 }],
        shape: "ring",
      },
      {
        id: "offline",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "offline",
        rows: [],
      },
    ],
  },
  {
    // 折叠与安静：这两个都不是设置页里的抽象开关，是桌面上看得见的样子。
    id: "tucked",
    steps: [
      {
        id: "quiet",
        durationMs: DEMO_STEP_MS,
        aggregate: "idle",
        rows: [{ id: "build", state: "idle" }],
        quiet: true,
      },
      {
        id: "collapsed",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
          { id: "api", state: "working" },
        ],
        collapsed: true,
      },
      {
        id: "collapsedDone",
        durationMs: DEMO_STEP_MS,
        aggregate: "done",
        rows: [
          { id: "build", state: "done" },
          { id: "api", state: "done" },
          { id: "review", state: "done" },
        ],
        collapsed: true,
      },
    ],
  },
  {
    // 用光说话：边灯四档 + 四角角灯。这一场存在的理由是**可发现性** ——
    // 两项默认全关，不进演示，用户根本不会知道设置里有它们。
    // 每一步都换成「这一档长什么样」，而不是跟着用户当前的设置走。
    id: "look",
    steps: [
      {
        id: "edgeTop",
        durationMs: DEMO_STEP_MS,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        edge: "top",
      },
      {
        id: "edgeFrame",
        durationMs: DEMO_STEP_MS,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        edge: "frame",
      },
      {
        id: "edgeGlow",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        edge: "glow",
      },
      {
        id: "cornersState",
        durationMs: DEMO_STEP_MS,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        edge: "off",
        corners: { tl: "state", tr: "state", br: "state", bl: "state" },
      },
      {
        // 四条判据同时成立，四枚才一起亮 —— 正是「不亮 = 根本不画」的反面。
        id: "cornersMeaning",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "stale",
        rows: [{ id: DEMO_CORNER_GROUP, state: "stale", staleProgress: 0.78 }],
        edge: "off",
        corners: { tl: "state", tr: "pinned", br: "muted", bl: "stale" },
        pinned: true,
        muted: true,
      },
      {
        // 两块牌状态不同，只有「等你批准」那块亮边 —— 行为页的触发方式用一张图
        // 就能说清，光靠一行说明讲不明白。这一步不挂角灯，免得两件事抢注意力。
        id: "lightTrigger",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        edge: "frame",
        corners: { tl: "empty", tr: "empty", br: "empty", bl: "empty" },
        edgeTrigger: "attention",
      },
      {
        // 0.6.4：穿透不再是窗口右上角一枚常驻角标，而是四角里可选的一枚灯。
        // 这一步把「穿透开着」当成事实喂进去，空心方才会出现（不亮 = 根本不画）。
        id: "cornerPassthrough",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        edge: "frame",
        corners: { tl: "passthrough", tr: "state", br: "empty", bl: "empty" },
        passThrough: true,
      },
      {
        // 两行触发**各判各的**：边灯挑「达到指定状态 → 出错」，角灯仍常亮。
        // 于是出错那块亮边、正在跑那块不亮，而四枚状态角灯两块牌都挂着 ——
        // 一张图同时说清「九档条件」和「边灯/角灯互不牵连」。
        id: "lightTriggerState",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "error",
        rows: [
          { id: "review", state: "error" },
          { id: "build", state: "working" },
        ],
        edge: "frame",
        corners: { tl: "state", tr: "state", br: "state", bl: "state" },
        edgeTrigger: "state",
        triggerState: "error",
        cornerTrigger: "always",
      },
    ],
  },
];

/**
 * 所有步骤按场景顺序摊平。`demoSnapshot()` 和 DemoStage 都用这一份下标，
 * 场景只决定「播哪一段」。
 */
export const DEMO_STEPS: DemoStep[] = DEMO_SCENARIOS.flatMap((item) => item.steps);

/** 每个场景在 `DEMO_STEPS` 里的起始下标。模块加载时算一次。 */
const SCENARIO_START: Record<string, number> = (() => {
  const map: Record<string, number> = {};
  let cursor = 0;
  for (const scenario of DEMO_SCENARIOS) {
    map[scenario.id] = cursor;
    cursor += scenario.steps.length;
  }
  return map;
})();

export function scenarioStart(id: DemoScenarioId): number {
  return SCENARIO_START[id] ?? 0;
}

export function scenarioSteps(id: DemoScenarioId): DemoStep[] {
  return DEMO_SCENARIOS.find((item) => item.id === id)?.steps ?? [];
}

/** 这个全局下标属于哪个场景。 */
export function stepScenario(index: number): DemoScenarioId {
  let cursor = 0;
  for (const scenario of DEMO_SCENARIOS) {
    cursor += scenario.steps.length;
    if (index < cursor) return scenario.id;
  }
  // 越界就落到最后一个场景 —— 调用方拿到的一定是个能播的场景。
  return DEMO_SCENARIOS[DEMO_SCENARIOS.length - 1].id;
}

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

/**
 * 下一步的下标，**在场景内循环** —— 走到本场景末尾回到本场景第一步。
 *
 * 早先是一条长剧本的全局循环，现在分场景了：从「折叠」那一步往回绕到「空闲」
 * 会让画面从一颗灯突然变回一张牌，看着像跳帧。
 */
export function nextDemoStep(index: number): number {
  const scenario = stepScenario(index);
  const start = scenarioStart(scenario);
  const count = scenarioSteps(scenario).length;
  if (count === 0) return index;
  return start + ((index - start + 1) % count);
}

/** 上一步，同样在场景内循环。 */
export function prevDemoStep(index: number): number {
  const scenario = stepScenario(index);
  const start = scenarioStart(scenario);
  const count = scenarioSteps(scenario).length;
  if (count === 0) return index;
  return start + ((index - start - 1 + count) % count);
}

/** 把一步变成灯牌能直接吃的形状。 */
export function demoSnapshot(
  index: number,
  now: number,
  labels: Record<DemoGroupId, string>,
): {
  groups: GroupView[];
  aggregate: SignalState;
  lastDoneMs: number;
  collapsed: boolean;
  quiet: boolean;
  /** `undefined` = 跟随用户设置。 */
  shape: Shape | undefined;
  /** `undefined` = 跟随用户设置。 */
  edge: SignEdge | undefined;
  /** `undefined` = 跟随用户设置。 */
  corners: SignCorners | undefined;
  /** 这一步是否把 [`DEMO_CORNER_GROUP`] 当作置顶 / 静音。 */
  pinned: boolean;
  muted: boolean;
  /** 边灯 / 角灯这一刻的触发方式；`undefined` = 常亮。 */
  edgeTrigger: LightTrigger | undefined;
  cornerTrigger: LightTrigger | undefined;
  /** 「达到指定状态」那一步挑的状态。 */
  triggerState: SignalState | undefined;
  /** 这一步是否把「点击穿透」当成已开启（给 `passthrough` 角灯当事实）。 */
  passThrough: boolean;
} {
  const step = DEMO_STEPS[index];
  const groups: GroupView[] = step.rows.map((row) => ({
    name: row.id,
    default_label: labels[row.id],
    aggregate: row.state,
    message: row.message ?? null,
    // 固定「6 秒前进入当前状态」，牌上的「已持续」会自己往上走
    since_ms: now - 6_000,
    stale_progress: row.staleProgress ?? null,
    sessions: [],
  }));
  return {
    groups,
    aggregate: step.aggregate,
    lastDoneMs: step.lastDoneAgoMs == null ? 0 : now - step.lastDoneAgoMs,
    collapsed: step.collapsed === true,
    quiet: step.quiet === true,
    shape: step.shape,
    edge: step.edge,
    corners: step.corners,
    pinned: step.pinned === true,
    muted: step.muted === true,
    edgeTrigger: step.edgeTrigger,
    cornerTrigger: step.cornerTrigger,
    triggerState: step.triggerState,
    passThrough: step.passThrough === true,
  };
}
