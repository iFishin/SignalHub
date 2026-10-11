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

/** 聚合可能出现的全部状态，含派生状态。灯球按这个取色。顺序大致按优先级。 */
export const SIGNALS = [
  "offline",
  "off",
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

/** 与 Rust 侧 state::Snapshot 对应。 */
export interface Snapshot {
  aggregate: SignalState;
  paused: boolean;
  since_ms: number;
  message: string | null;
  sessions: SessionView[];
}
