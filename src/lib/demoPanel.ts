/**
 * 演示里「收件箱 / 留言」两个面板的**剧本数据**。
 *
 * 和 `demo.ts` 的分工：那边写灯牌（一步 = 灯上的一个状态），这边写面板
 * （一帧 = 打开面板会看到什么）。两者都不放界面，只放数据 —— 演示最怕的不是
 * 不好看，是画出了产品里做不出来的东西，那会变成一份会骗人的说明书。
 *
 * **为什么坚持用真实结构**：`Snapshot` / `SessionView` / `Approval` /
 * `CallMeState` 全是产品里的类型，分段与排序由 `buildInbox()` 这个**真实纯函数**
 * 算（在 DemoPanel.tsx 里调用），不是演示自己排一遍。演示里那张表因此和产品
 * 永远同一套规则：一行 = 一个会话、一条审批只画一次、需要你的排最前。
 *
 * **文本不写在这里**：项目名、命令、留言内容全由 `DemoPanelTexts` 从 i18n
 * 注入。演示数据里塞中文字符串，英文界面就会露出一句中文。
 *
 * **时间固定**：收件箱的「等了多久」、留言的「多久以前」都按帧里的固定值算，
 * 不跟着墙钟走。演示是循环的，时间每次重播都变反而让人读不清那一行。
 */
import type { CallMeMessage, CallMeSource, CallMeState } from "./callme";
import type { Approval } from "./useApprovals";
import type { GroupView, SessionView, Snapshot } from "./types";
import type { AwaySummary } from "./awaySummary";

/** 面板演示文本。中英各一份，见 i18n.ts 的 `demoPanel`。 */
export interface DemoPanelTexts {
  /** 五个项目 / 会话名（收件箱里一行的名字）。 */
  project: [string, string, string, string, string];
  /** 智能体名（行里的元信息）。 */
  agent: string;
  /** 一条远程会话来自哪台机器。 */
  machine: string;
  /** 项目目录，收件箱里只显示末段。 */
  cwd: string;
  /** 待审批的工具名与要执行的命令。 */
  tool: string;
  command: string;
  /** 长到被截断的命令（看不全就不给放行）。 */
  longCommand: string;
  /** 会话自带的一句话消息。 */
  note: string;
  /** 两个 Call Me 来源的名字。 */
  source: [string, string];
  /** 三条留言：标题 / 正文 / 智能体自报的出处。 */
  letters: [DemoLetter, DemoLetter, DemoLetter];
}

export interface DemoLetter {
  title: string;
  text: string;
  context: string;
}

/** 面板帧 id。也是 `demo.ts` 里 `DemoStep.panel` 的取值。 */
export type DemoPanelFrameId =
  | "inboxNeed"
  | "inboxApproval"
  | "inboxAway"
  | "inboxEmpty"
  | "messagesInbox"
  | "messagesUnread"
  | "messagesOpen"
  | "messagesEmpty";

export type DemoPanelTab = "inbox" | "messages";

export interface DemoPanelFrame {
  /** 停在哪个页签。收件箱页也能看见留言页签上的未读徽章。 */
  tab: DemoPanelTab;
  snapshot: Snapshot;
  approvals: Approval[];
  callme: CallMeState;
  /** 「你离开的这段时间」摘要；null = 不画那一条。 */
  away: AwaySummary | null;
  /** 留言页初始展开哪一条（seq）；null = 全收起。 */
  openSeq: number | null;
}

/** 演示里的组 id。收件箱一行 = 一个会话，这五个各是一个组。 */
const GROUPS = ["web", "api", "worker", "docs", "notes"] as const;

function session(
  id: string,
  state: SessionView["state"],
  ageMs: number,
  t: DemoPanelTexts,
  extra: Partial<SessionView> = {},
): SessionView {
  return {
    id,
    state,
    agent: t.agent,
    cwd: t.cwd,
    message: t.note,
    age_ms: ageMs,
    stale_progress: null,
    ...extra,
  };
}

function group(index: number, t: DemoPanelTexts, sessions: SessionView[]): GroupView {
  return {
    name: GROUPS[index],
    default_label: t.project[index],
    aggregate: sessions[0]?.state ?? "idle",
    message: null,
    since_ms: 0,
    stale_progress: null,
    snooze_until_ms: 0,
    sessions,
  };
}

function approval(
  id: string,
  sessionId: string,
  t: DemoPanelTexts,
  extra: Partial<Approval> = {},
): Approval {
  return {
    id,
    session: sessionId,
    tool: t.tool,
    target: t.command,
    detail: t.command,
    truncated: false,
    danger: null,
    allowable: true,
    age_ms: 3 * 60_000,
    ...extra,
  };
}

/**
 * 四段都在的一屏：需要你 / 运行中 / 刚刚完成 / 空闲沉默。
 *
 * 这一帧**故意不带审批**：审批详情会把一行撑到三倍高，四段就挤不进同一屏了。
 * 下一条 `inboxApproval` 帧专门讲那块内容，两帧各讲一件事。
 */
function needSnapshot(t: DemoPanelTexts): Snapshot {
  return {
    aggregate: "permission",
    paused: false,
    since_ms: 0,
    message: null,
    last_done_ms: 0,
    groups: [
      group(0, t, [session("s-web", "permission", 3 * 60_000, t)]),
      // 黄色 attention 也进「需要你」段，但优先级低于红色 permission ——
      // 于是整盏灯的聚合仍是「等你批准」，和这一场要讲的红灯一致。
      group(1, t, [session("s-api", "attention", 5 * 60_000, t)]),
      group(2, t, [session("s-worker", "working", 12 * 60_000, t)]),
      // 远程会话：行上会多一枚机器徽章，跳转按钮也因此不再可用。
      group(3, t, [
        session("s-docs", "done", 40_000, t, { machine: t.machine }),
      ]),
      // 第五组是 idle —— 少了它，「空闲沉默」那一段在这帧里根本不会出现，
      // 文案说「四段」就成了假的。
      group(4, t, [session("s-notes", "idle", 25 * 60_000, t, { message: null })]),
    ],
  };
}

/** 只留两条待审批，一条能放行、一条命令太长只能拒绝。 */
function approvalSnapshot(t: DemoPanelTexts): Snapshot {
  return {
    ...needSnapshot(t),
    groups: [
      group(0, t, [session("s-web", "permission", 3 * 60_000, t)]),
      group(1, t, [session("s-api", "permission", 8 * 60_000, t)]),
    ],
  };
}

function approvalApprovals(t: DemoPanelTexts): Approval[] {
  return [
    approval("a-web", "s-web", t),
    approval("a-api", "s-api", t, {
      tool: t.tool,
      target: t.longCommand,
      detail: t.longCommand,
      truncated: true,
      allowable: false,
      danger: "delete",
      age_ms: 8 * 60_000,
    }),
  ];
}

function awaySummary(): AwaySummary {
  return {
    awayMs: 24 * 60_000,
    done: 2,
    failed: 1,
    need: 1,
    running: 1,
    longestWaitMs: 6 * 60_000,
    transientDone: 1,
    transientFailed: 0,
  };
}

function emptySnapshot(): Snapshot {
  return {
    aggregate: "offline",
    paused: false,
    since_ms: 0,
    message: null,
    last_done_ms: 0,
    groups: [],
  };
}

/**
 * 灯安静、收件箱里没有「需要你」的一屏。
 *
 * 这是 Call Me 那一场的**关键事实**：智能体从别处发来留言，走的是另一条通道，
 * 它不会把灯点红，也不会往收件箱塞一条待办。演示若把这一帧画成红灯，等于
 * 骗人说「留言会像授权一样打断你」，而产品恰恰不是这么设计的。
 */
function quietSnapshot(t: DemoPanelTexts): Snapshot {
  return {
    ...needSnapshot(t),
    aggregate: "working",
    groups: [
      group(2, t, [session("s-worker", "working", 12 * 60_000, t)]),
      group(3, t, [session("s-docs", "done", 40_000, t)]),
      group(4, t, [session("s-notes", "idle", 25 * 60_000, t, { message: null })]),
    ],
  };
}

/** 两个来源，名字由 i18n 给；`revoked` 全 false，链接都是有效链接。 */
function sources(t: DemoPanelTexts): CallMeSource[] {
  const now = 1_700_000_000_000;
  return t.source.map((label, index) => ({
    id: `src-${index}`,
    label,
    note: null,
    token: "demo-token",
    link: `/c/demo-${index}`,
    created_ms: now - 3 * 24 * 60 * 60_000,
    revoked: false,
    total: 12 + index * 5,
    today: index === 0 ? 2 : 1,
    last_received_ms: now - (index === 0 ? 2 : 25) * 60_000,
  }));
}

/** 三条留言，时间从旧到新（Rust 侧就是这个顺序）。 */
function letters(t: DemoPanelTexts, now: number): CallMeMessage[] {
  return t.letters.map((letter, index) => ({
    seq: index + 1,
    id: `msg-${index + 1}`,
    source_id: `src-${index === 2 ? 1 : 0}`,
    // 2 分钟 / 8 分钟 / 25 分钟前
    received_ms: now - [25, 8, 2][index] * 60_000,
    title: letter.title,
    text: letter.text,
    agent: t.agent,
    context: letter.context,
  }));
}

function callmeState(t: DemoPanelTexts, now: number, read: Record<string, number>): CallMeState {
  return {
    enabled: true,
    ok: true,
    last_ok_ms: now - 1_000,
    latency_ms: 42,
    error: null,
    sources: sources(t),
    messages: letters(t, now),
    read,
  };
}

/** 三条里两条未读：最新的一条和来源二那条。 */
const SOME_READ: Record<string, number> = { "src-0": 1, "src-1": 0 };

function emptyCallMe(t: DemoPanelTexts, now: number): CallMeState {
  return {
    ...callmeState(t, now, SOME_READ),
    // 「没配服务器」和「真没有留言」是两种空态，说错原因会让人去修错的东西。
    // 这一帧特意演示前者：空态里出现的是「去配置」，不是「没有留言」。
    enabled: false,
    ok: false,
    last_ok_ms: 0,
    error: null,
    messages: [],
    sources: [],
    read: {},
  };
}

/**
 * 取一帧。`now` 用来把留言的「多久以前」摆到合理的数字上 —— 传进来的是
 * DemoStage 挂载时刻，整轮演示里固定不变。
 */
export function demoPanelFrame(
  id: DemoPanelFrameId,
  now: number,
  t: DemoPanelTexts,
): DemoPanelFrame {
  const base = {
    tab: "inbox" as DemoPanelTab,
    snapshot: needSnapshot(t),
    approvals: [],
    callme: callmeState(t, now, SOME_READ),
    away: null,
    openSeq: null,
  } satisfies DemoPanelFrame;

  // 留言那一场的底板：收件箱没有「需要你」，但留言页签挂着两条未读。
  const messagesBase: DemoPanelFrame = {
    ...base,
    snapshot: quietSnapshot(t),
    approvals: [],
  };

  switch (id) {
    case "inboxNeed":
      return base;
    case "inboxApproval":
      return { ...base, snapshot: approvalSnapshot(t), approvals: approvalApprovals(t) };
    case "inboxAway":
      return { ...base, away: awaySummary() };
    case "inboxEmpty":
      return { ...base, snapshot: emptySnapshot(), approvals: [] };
    case "messagesInbox":
      // 停在收件箱页，但留言页签上已经挂着未读徽章 —— 讲「同一个面板两个问题」。
      // tab 必须是 inbox：这一帧讲的就是「人还在收件箱，先看见旁边有个红点」。
      return messagesBase;
    case "messagesUnread":
      return { ...messagesBase, tab: "messages" };
    case "messagesOpen":
      return { ...messagesBase, tab: "messages", openSeq: 3 };
    case "messagesEmpty":
      return { ...messagesBase, tab: "messages", callme: emptyCallMe(t, now) };
  }
}

/** 帧 id 全集，单测用它盯住「每个 id 都有数据、都能被真实纯函数处理」。 */
export const DEMO_PANEL_FRAMES: DemoPanelFrameId[] = [
  "inboxNeed",
  "inboxApproval",
  "inboxAway",
  "inboxEmpty",
  "messagesInbox",
  "messagesUnread",
  "messagesOpen",
  "messagesEmpty",
];
