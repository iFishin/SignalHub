/**
 * 「系统 → 开发者」的模拟剧本：状态、会话、灯位三组数据。
 *
 * 这里的每条数据都必须能被真实产品做出来 —— 状态取 `REPORTABLE_SIGNALS`，
 * 灯位取 `SignLights` 的真字段，会话名一律加 `DEV_SESSION_PREFIX`。开发者面板
 * 的价值是「看到真的产品行为」，一旦剧本里有产品自己造不出的状态，它就变成
 * 一份会骗人的说明书（和 `demo.ts` 的约束同源）。
 *
 * 文案不在这里：界面说明和留言正文走 i18n，本文件只放结构与测试数据。
 */

import { REPORTABLE_SIGNALS, type SignalState } from "./types";
import type { SignLights } from "./useSettings";

/**
 * 与 Rust `lib.rs::DEV_SESSION_PREFIX` 是两处真值：Rust 按它整批清会话，
 * 前端按它拼名字。改一处必须改另一处。
 */
export const DEV_SESSION_PREFIX = "dev:";

/** 单条模拟会话的规格。`key` 会被拼到 `DEV_SESSION_PREFIX` 后面。 */
export interface DevSessionSpec {
  key: string;
  group: string;
  state: SignalState;
  message: string;
}

/** 一键铺开的场景。文案键在 i18n 的 `dev.scenario`。 */
export interface DevScenario {
  id: string;
  sessions: readonly DevSessionSpec[];
}

/** 把剧本里的会话名变成真正上报用的 session id。 */
export function devSessionId(spec: DevSessionSpec): string {
  return `${DEV_SESSION_PREFIX}${spec.key}`;
}

/**
 * 可上报且适合「看灯效」的状态。`off` 被去掉 —— 上报它等于清掉会话，
 * 那不是「灯变成某个颜色」，而是「牌没了」。
 */
export const DEV_STATES: readonly SignalState[] = REPORTABLE_SIGNALS.filter(
  (state) => state !== "off",
);

/** 三档 TTL。够看清灯的变化，又不至于关掉面板后假牌赖着不走。 */
export const DEV_TTL_MS = {
  short: 20_000,
  medium: 120_000,
  long: 600_000,
} as const;
export type DevTtl = keyof typeof DEV_TTL_MS;
export const DEV_TTLS: readonly DevTtl[] = ["short", "medium", "long"];

/** 四种多会话场景：一个、日常三个、七种状态全铺、专门喂收件箱。 */
export const DEV_SCENARIOS: readonly DevScenario[] = [
  {
    id: "solo",
    sessions: [{ key: "solo", group: "demo", state: "working", message: "Working on the release" }],
  },
  {
    id: "daily",
    sessions: [
      { key: "build", group: "build", state: "working", message: "Compiling packages" },
      { key: "review", group: "review", state: "attention", message: "Two comments to answer" },
      { key: "docs", group: "docs", state: "idle", message: "Waiting for the next task" },
    ],
  },
  {
    id: "every",
    sessions: [
      { key: "error", group: "sync", state: "error", message: "Connection refused" },
      { key: "permission", group: "design", state: "permission", message: "May I edit config.rs?" },
      { key: "attention", group: "review", state: "attention", message: "Review requested" },
      { key: "working", group: "build", state: "working", message: "Running the test suite" },
      { key: "thinking", group: "plan", state: "thinking", message: "Weighing two approaches" },
      { key: "done", group: "deploy", state: "done", message: "Staging deploy finished" },
      { key: "idle", group: "docs", state: "idle", message: "Standing by" },
    ],
  },
  {
    id: "needsYou",
    sessions: [
      { key: "ask", group: "design", state: "permission", message: "Approve the new palette?" },
      { key: "blocked", group: "api", state: "error", message: "Token expired" },
      { key: "ping", group: "review", state: "attention", message: "You were mentioned" },
    ],
  },
];

/** 六灯位全关的骨架。和 `presets.ts::lights` / `demo.ts::demoLights` 同构。 */
function baseLights(): SignLights {
  return {
    top: { kind: "off", trigger: "always", blink: false },
    bottom: { kind: "off", trigger: "always", blink: false },
    tl: { kind: "empty", trigger: "always", blink: false },
    tr: { kind: "empty", trigger: "always", blink: false },
    br: { kind: "empty", trigger: "always", blink: false },
    bl: { kind: "empty", trigger: "always", blink: false },
  };
}

/**
 * 灯位预设。这里改的是**设置**，不是临时画一层假光 —— 走 `patch` 写回
 * `settings.sign_lights`，所以退出面板后用户看到的就是产品真实的灯位行为。
 * 文案键在 i18n 的 `dev.lights`。
 */
export const DEV_LIGHT_PRESETS: Readonly<Record<string, SignLights>> = {
  none: baseLights(),
  edges: {
    ...baseLights(),
    top: { kind: "line", trigger: "always", blink: false },
    bottom: { kind: "line", trigger: "always", blink: false },
    tl: { kind: "state", trigger: "always", blink: false },
    tr: { kind: "state", trigger: "always", blink: false },
    br: { kind: "state", trigger: "always", blink: false },
    bl: { kind: "state", trigger: "always", blink: false },
  },
  attention: {
    ...baseLights(),
    top: { kind: "glow", trigger: "attention", blink: true },
    tl: { kind: "state", trigger: "active", blink: false },
    br: { kind: "pinned", trigger: "always", blink: false },
  },
  everything: {
    top: { kind: "glow", trigger: "always", blink: true },
    bottom: { kind: "line", trigger: "state", blink: false },
    tl: { kind: "state", trigger: "always", blink: false },
    tr: { kind: "pinned", trigger: "always", blink: false },
    br: { kind: "stale", trigger: "stale", blink: false },
    bl: { kind: "muted", trigger: "always", blink: false },
  },
};

export const DEV_LIGHT_IDS = ["none", "edges", "attention", "everything"] as const;
