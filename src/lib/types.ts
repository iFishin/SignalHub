/** 与 Rust 侧 state::Signal 一一对应，顺序即优先级从低到高。 */
export const SIGNALS = [
  "off",
  "idle",
  "done",
  "thinking",
  "working",
  "attention",
  "permission",
  "error",
] as const;

export type SignalState = (typeof SIGNALS)[number];

export interface SessionView {
  id: string;
  state: SignalState;
  agent: string | null;
  cwd: string | null;
  message: string | null;
  age_ms: number;
}

/** 与 Rust 侧 state::Snapshot 对应。 */
export interface Snapshot {
  aggregate: SignalState;
  paused: boolean;
  since_ms: number;
  message: string | null;
  sessions: SessionView[];
}
