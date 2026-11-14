/**
 * 演示模式的「剧本」。
 *
 * 这里只放**数据**，不放界面：每一步是「灯上该有几块牌、各是什么状态」，
 * 文案在 i18n、画面在 DemoStage。拆开是因为这段逻辑要能单测 —— 演示最怕的
 * 不是不好看，是某一步画出了和产品实际行为不一致的东西（比如把「完成」
 * 画成转沉默），那会变成一份会骗人的说明书。
 *
 * **为什么分成场景而不是一条长剧本**：这些用法彼此不相干 —— 平时干活、有事要你、
 * 它不说话了、收起来、「用光说话」（边灯 / 角灯），以及 0.5 之后长出来的收件箱、
 * Call Me 留言、灯上的手势。串成一条线性剧本，想看「折叠长什么样」得先等 40 秒；
 * 而按场景切开，每个场景自己循环，点一下就能反复看同一件事。
 * 界面文案因此**不写死场景数量**：再加一场不用回头改五处标题。
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
 *
 * 0.6.7 之后多了**三个舞台**：老五个场景画灯牌，新场景把收件箱 / 留言面板和
 * 长按呼出的圆环菜单也搬进同一段演示。舞台只决定「这一步画哪个组件」，
 * 数据仍然从这里念 —— 面板帧的 id 在 `demoPanel.ts`，文案在 i18n。
 */
import type { GroupView, SignalState } from "./types";
import type { Shape, SignLights } from "./useSettings";
import type { DemoPanelFrameId } from "./demoPanel";

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

/**
 * 「用光说话」那一场的六灯位骨架。
 *
 * 剧本只写自己要改的那几个槽，其余全是默认（不亮 / 不挂 / 常亮）。和
 * `presets.ts::lights` 是同一套写法但**故意不共用**：那边是用户能一键选的皮肤，
 * 这边是演示脚本，改一边不该让另一边跟着变。
 */
function demoLights(patch: Partial<SignLights>): SignLights {
  return {
    top: { kind: "off", trigger: "always", blink: false },
    bottom: { kind: "off", trigger: "always", blink: false },
    tl: { kind: "empty", trigger: "always", blink: false },
    tr: { kind: "empty", trigger: "always", blink: false },
    br: { kind: "empty", trigger: "always", blink: false },
    bl: { kind: "empty", trigger: "always", blink: false },
    ...patch,
  };
}

/**
 * 场景 id。文案在 i18n 的 `demoScenario`。
 *
 * 前五个讲**灯牌本身**（一天里怎么呼吸）；后三个讲 0.5 之后长出来的那半边：
 * 打开面板能做什么、Call Me 留言从哪来、灯上的手势怎么呼出菜单。
 */
export type DemoScenarioId =
  | "day"
  | "asking"
  | "silence"
  | "tucked"
  | "look"
  | "inbox"
  | "callme"
  | "menu";

/** 一步画在哪个舞台上。不填 = 灯牌（老五个场景全是这个）。 */
export type DemoStageKind = "lamp" | "panel" | "menu";


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
   * 这一步六个灯位换成什么。不填 = 跟随用户设置。
   *
   * 只有「用光说话」那个场景会填 —— 那一场讲的就是上下两条边和四个角，跟着
   * 用户的设置走等于什么都没讲。每个槽自带画法 + 触发，所以一步就能演示
   * 「上边只在出错时亮、下边常亮、四个角各挂各的」。
   */
  lights?: SignLights;
  /** 这一步换成哪套整套皮肤（id 见 `presets.ts::skins`）。不填 = 逐项手调。 */
  skin?: string;
  /** 这一步把那块牌当成「被置顶」（角灯的 `pinned` 判据）。 */
  pinned?: boolean;
  /** 这一步把那块牌当成「被静音」（角灯的 `muted` 判据）。 */
  muted?: boolean;
  /** 「达到指定状态」那一档挑哪个状态。不填 = DemoStage 给 `permission`。 */
  triggerState?: SignalState;
  /** 这一步假装在点击穿透，让 `passthrough` 角灯有条件亮。 */
  passThrough?: boolean;

  /**
   * 这一步画在哪个舞台上。不填 = 灯牌（老五个场景全是这个）。
   *
   * `panel` 用只读面板（`DemoPanel`），`menu` 用长按呼出的圆环（`LampMenu`）。
   * 两者都是**产品里那个组件的本体**，不是另画一张示意图。
   */
  stage?: DemoStageKind;
  /** `stage === "panel"` 时取哪一帧。frame id 与 step.id 同名，单测会盯住。 */
  panel?: DemoPanelFrameId;
  /** `stage === "menu"` 时，「暂停 / 继续」那一项摆成暂停后的样子。 */
  menuPaused?: boolean;
  /**
   * 这一步把哪一组标成「推迟 15 分钟」（0.6.7 的右键菜单动作）。
   *
   * 只改展示：牌退成灰、停闪、显示倒计时，底层状态一点不动 —— 所以演示里
   * 也只是给 `GroupView.snooze_until_ms` 一个未来的时间，没有别的假字段。
   */
  snooze?: DemoGroupId;
}

export interface DemoScenario {
  id: DemoScenarioId;
  steps: DemoStep[];
}

/** 每一步停多久。场景最长 9 步 ≈ 30 秒，最短 3 步 ≈ 10 秒。 */
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
    // 用光说话：上下两条边 + 四个角 = **六个独立灯位**，最后一步一键换整套皮肤。
    // 这一场存在的理由是**可发现性** —— 六槽默认全关，不进演示，用户根本不会
    // 知道设置里有它们。每一步都由剧本给 `lights`，不跟着用户当前设置走。
    id: "look",
    steps: [
      {
        id: "edgeLine",
        durationMs: DEMO_STEP_MS,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        // 只亮上边 —— 下边仍是独立的「不亮」。这正是 0.6.4 那一条轨做不到的。
        lights: demoLights({ top: { kind: "line", trigger: "always", blink: false } }),
      },
      {
        id: "edgeBoth",
        durationMs: DEMO_STEP_MS,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        lights: demoLights({
          top: { kind: "line", trigger: "always", blink: false },
          bottom: { kind: "line", trigger: "always", blink: false },
        }),
      },
      {
        id: "edgeGlow",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        lights: demoLights({
          top: { kind: "glow", trigger: "always", blink: false },
          bottom: { kind: "glow", trigger: "always", blink: false },
        }),
      },
      {
        id: "cornersState",
        durationMs: DEMO_STEP_MS,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        lights: demoLights({
          tl: { kind: "state", trigger: "always", blink: false },
          tr: { kind: "state", trigger: "always", blink: false },
          br: { kind: "state", trigger: "always", blink: false },
          bl: { kind: "state", trigger: "always", blink: false },
        }),
      },
      {
        // 四条判据同时成立，四枚才一起亮 —— 正是「不亮 = 根本不画」的反面。
        id: "cornersMeaning",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "stale",
        rows: [{ id: DEMO_CORNER_GROUP, state: "stale", staleProgress: 0.78 }],
        lights: demoLights({
          tl: { kind: "state", trigger: "always", blink: false },
          tr: { kind: "pinned", trigger: "always", blink: false },
          br: { kind: "muted", trigger: "always", blink: false },
          bl: { kind: "stale", trigger: "always", blink: false },
        }),
        pinned: true,
        muted: true,
      },
      {
        // 两行状态不同，只有「等你批准」那行上沿亮 —— 行为页的触发方式用一张图
        // 就能说清，光靠一行说明讲不明白。这一步不挂四格记号，免得两件事抢注意力。
        id: "lightTrigger",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        lights: demoLights({
          top: { kind: "line", trigger: "attention", blink: false },
        }),
      },
      {
        // 穿透不再是窗口右上角一枚常驻角标，而是行尾四格里可选的一枚记号。
        // 这一步把「穿透开着」当成事实喂进去，那枚竖条才会出现（不亮 = 根本不画）。
        id: "cornerPassthrough",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "thinking" },
        ],
        lights: demoLights({
          top: { kind: "line", trigger: "always", blink: false },
          tl: { kind: "passthrough", trigger: "always", blink: false },
          tr: { kind: "state", trigger: "always", blink: false },
        }),
        passThrough: true,
      },
      {
        // 六个槽**各判各的**：上沿挑「达到指定状态 → 出错」，下沿常亮，四格常亮。
        // 于是出错那行上下都亮、正在跑那行只剩一道下沿刻线，而四枚状态色记号两行
        // 都挂着 —— 一张图同时说清「九档条件」和「六行互不牵连」。
        id: "lightTriggerState",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "error",
        rows: [
          { id: "review", state: "error" },
          { id: "build", state: "working" },
        ],
        lights: demoLights({
          top: { kind: "line", trigger: "state", blink: false },
          bottom: { kind: "line", trigger: "always", blink: false },
          tl: { kind: "state", trigger: "always", blink: false },
          tr: { kind: "state", trigger: "always", blink: false },
          br: { kind: "state", trigger: "always", blink: false },
          bl: { kind: "state", trigger: "always", blink: false },
        }),
        triggerState: "error",
      },
      {
        // 一键换整套：配色、牌底、圆角、描边、灯珠形状、牌的轮廓、动效，以及
        // 六个灯位的画法与触发。由 DemoStage 从 `skins()` 里按 id 取，剧本只存 id
        // —— 皮肤改了，演示跟着变，不会变成一份过时的说明书。
        id: "lookSkin",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        skin: "neon",
      },
    ],
  },
  {
    // 0.6.6 之后「灯红了，然后呢」的完整链路。灯只告诉你有事，细节在面板里：
    // 打开 → 看分段与审批 → 离开摘要 → 不想现在处理就推迟 → 处理完的空态。
    id: "inbox",
    steps: [
      {
        id: "inboxLampRed",
        durationMs: DEMO_STEP_MS,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
      },
      {
        id: "inboxNeed",
        stage: "panel",
        panel: "inboxNeed",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "permission",
        // 面板帧自己的四个组：permission / attention / working / done。
        // 这里用三个演示组 id 复述同一组优先级，让 demoSnapshot 仍然自洽。
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "attention" },
          { id: "api", state: "working" },
        ],
      },
      {
        id: "inboxApproval",
        stage: "panel",
        panel: "inboxApproval",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "permission" },
        ],
      },
      {
        id: "inboxAway",
        stage: "panel",
        panel: "inboxAway",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "attention" },
          { id: "api", state: "working" },
        ],
      },
      {
        id: "inboxSnoozed",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        // 右键那块牌 →「推迟 15 分钟」。牌退成灰、停闪、显示倒计时。
        snooze: "review",
      },
      {
        id: "inboxEmpty",
        stage: "panel",
        panel: "inboxEmpty",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "offline",
        rows: [],
      },
    ],
  },
  {
    // Call Me：外面智能体发来的留言走的是**另一条通道** —— 不点红灯、不塞待办。
    // 这一场从「灯没红」开始，讲的是同一个面板的第二个页签。
    id: "callme",
    steps: [
      {
        id: "callmeLampQuiet",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "done" },
        ],
      },
      {
        id: "messagesInbox",
        stage: "panel",
        panel: "messagesInbox",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "done" },
        ],
      },
      {
        id: "messagesUnread",
        stage: "panel",
        panel: "messagesUnread",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "done" },
        ],
      },
      {
        id: "messagesOpen",
        stage: "panel",
        panel: "messagesOpen",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "done" },
        ],
      },
      {
        id: "messagesEmpty",
        stage: "panel",
        panel: "messagesEmpty",
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "working",
        rows: [
          { id: "build", state: "working" },
          { id: "api", state: "done" },
        ],
      },
    ],
  },
  {
    // 灯上的手势：单击折叠、长按 480ms 呼出圆环、拖动移动窗口。
    // 圆环用 LampMenu 本体（preview 开关关掉抢焦点 / 键盘 / 真动作）。
    id: "menu",
    steps: [
      {
        id: "menuLamp",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
      },
      {
        id: "menuRing",
        stage: "menu",
        durationMs: DEMO_STEP_MS + 1800,
        aggregate: "permission",
        rows: [{ id: "review", state: "permission" }],
      },
      {
        id: "menuPaused",
        stage: "menu",
        menuPaused: true,
        durationMs: DEMO_STEP_MS + 1200,
        aggregate: "working",
        rows: [{ id: "build", state: "working" }],
      },
      {
        id: "menuBack",
        durationMs: DEMO_STEP_MS + 800,
        aggregate: "permission",
        rows: [
          { id: "review", state: "permission" },
          { id: "build", state: "working" },
        ],
        collapsed: true,
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

/**
 * 独立演示页（`?window=demo`）的深链：`?demo=inbox&step=2` 直接停到那一帧。
 *
 * 给 README 动图和视觉核对用 —— 不用先等前面几个场景播完，也不用往产品代码里
 * 塞一个只为截图存在的开关。返回的是 `DEMO_STEPS` 的全局下标；认不出来的场景
 * 返回 null，落回默认的第一场。`step` 是**场景内的 1 起序号**，越界就夹到首尾。
 */
export function demoDeepLink(search: string): number | null {
  const params = new URLSearchParams(search);
  const id = params.get("demo");
  if (!id || !DEMO_SCENARIOS.some((item) => item.id === id)) return null;
  const scenario = id as DemoScenarioId;
  const count = scenarioSteps(scenario).length;
  const raw = Number(params.get("step") ?? "1");
  const wanted = Number.isInteger(raw) && raw >= 1 ? raw : 1;
  return scenarioStart(scenario) + Math.min(wanted, count) - 1;
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
  /** 六灯位覆盖；`undefined` = 跟随用户设置。 */
  lights: SignLights | undefined;
  /** 整套皮肤 id；`undefined` = 不换肤。 */
  skin: string | undefined;
  /** 这一步是否把 [`DEMO_CORNER_GROUP`] 当作置顶 / 静音。 */
  pinned: boolean;
  muted: boolean;
  /** 「达到指定状态」那一步挑的状态。 */
  triggerState: SignalState | undefined;
  /** 这一步是否把「点击穿透」当成已开启（给 `passthrough` 角灯当事实）。 */
  passThrough: boolean;
  /** 这一步画哪个舞台。 */
  stage: DemoStageKind;
  /** `stage === "panel"` 时取哪一帧。 */
  panel: DemoPanelFrameId | undefined;
  /** `stage === "menu"` 时「暂停 / 继续」那一项摆成暂停后的样子。 */
  menuPaused: boolean;
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
    // 只有「推迟 15 分钟」那一步给一个未来的时间，其余一律 0。
    // 用的是产品里那个真实字段：灯自己会据此退成灰、停闪、显示倒计时。
    snooze_until_ms: step.snooze === row.id ? now + 15 * 60_000 : 0,
    sessions: [],
  }));
  return {
    groups,
    aggregate: step.aggregate,
    lastDoneMs: step.lastDoneAgoMs == null ? 0 : now - step.lastDoneAgoMs,
    collapsed: step.collapsed === true,
    quiet: step.quiet === true,
    shape: step.shape,
    lights: step.lights,
    skin: step.skin,
    pinned: step.pinned === true,
    muted: step.muted === true,
    triggerState: step.triggerState,
    passThrough: step.passThrough === true,
    stage: step.stage ?? "lamp",
    panel: step.panel,
    menuPaused: step.menuPaused === true,
  };
}
