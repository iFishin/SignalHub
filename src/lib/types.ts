/**
 * 可以上报的状态 —— 与 Rust 侧 `Signal::parse` 接受的集合一致。
 *
 * `stale` / `offline` 不在这里：它们是聚合派生出来的结果，不是外部程序该声称的
 * 状态。dev 调试快捷键也只该遍历这一个列表。
 */
export const REPORTABLE_SIGNALS = [
  "idle",
  "thinking",
  "working",
  "done",
  "attention",
  "permission",
  "error",
  "off",
] as const;

export type ReportableSignal = (typeof REPORTABLE_SIGNALS)[number];

/**
 * 聚合可能出现的全部状态，含派生状态。灯球按这个取色。
 *
 * 注意 `off` 只出现一次 —— 它既是一个可上报的状态（上报它等于清除该会话），
 * 也是暂停时的聚合值。早先这里把它在派生前缀和可上报列表里各写了一遍，
 * 于是颜色页多出一行重复的取色器。
 */
export const SIGNALS = [
  "offline",
  "stale",
  ...REPORTABLE_SIGNALS,
] as const;

export type SignalState = (typeof SIGNALS)[number];

export interface SessionView {
  id: string;
  state: SignalState;
  agent: string | null;
  cwd: string | null;
  message: string | null;
  age_ms: number;
  /**
   * 仅 `stale` 会话有值：距被删除还剩多少宽限期，1.0 = 刚转 stale，0.0 = 即将消失。
   * 由服务端算好，前端不必复刻 TTL 常量。
   */
  stale_progress: number | null;
}

/**
 * 一个组在灯球上的样子：一块牌。
 *
 * `groups` 由服务端按优先级排好（最紧急在最前），前端**不要再排** ——
 * 窗口高度取决于「哪几块牌可见」，两边必须算出同一批。
 */
export interface GroupView {
  name: string;
  aggregate: SignalState;
  message: string | null;
  /** 这一组进入当前状态的时间（epoch 毫秒），牌上的「已持续」由它算 */
  since_ms: number;
  sessions: SessionView[];
}

/** 与 Rust 侧 state::Snapshot 对应。 */
export interface Snapshot {
  /** 所有组里最高的那个 —— 与分组无关，通知和钩子用它 */
  aggregate: SignalState;
  paused: boolean;
  since_ms: number;
  message: string | null;
  groups: GroupView[];
}
