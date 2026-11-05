/**
 * 系统体检：把「设置 → 系统」里散在各处的诊断信号收成一次可复制的自检。
 *
 * 为什么单独一个纯函数模块：`SettingsPanel.tsx` 已经是个几千行的开关函数，
 * 判据再塞进去就没人能测了。这里**只吃普通数据、只吐普通数据** —— 不 `invoke`、
 * 不读 `Date.now()`、不认识任何一种语言。界面负责取数（`get_notify_permission`
 * 等现成命令）和把 `HealthCheck` 翻成一句话（`m.healthLine`），复制出去的报告
 * 也走同一句，界面和报告不可能说两套话。
 *
 * 它刻意不新增任何后端接口：所有判据都来自已经存在的命令和状态。这样发版前
 * 自检不需要新的状态机、新的落盘格式，也不会改变灯的任何一个状态。
 */
import type { AgentStatus, CustomAgentsInfo, Receipt } from "./useAgents";
import type { ShortcutStatus } from "./useShortcuts";

/** 一条上报超过这么久没来，就当作「上次跑过，现在不确定」。 */
export const RECEIPT_FRESH_MS = 15 * 60 * 1000;

export type HealthLevel = "ok" | "warn" | "fail" | "info";

/**
 * 稳定的检查项 id。界面按它取本地化文案，报告按它排序 —— **不是自由文本**，
 * 加一项就得在两种语言里各补一句，类型系统会盯着。
 */
export type HealthId =
  | "permission"
  | "notify"
  | "sound"
  | "alertDelay"
  | "quiet"
  | "muted"
  | "install"
  | "installBroken"
  | "installStale"
  | "receipt"
  | "approve"
  | "custom"
  | "shortcut"
  | "hookLog"
  | "sessions";

export interface HealthCheck {
  id: HealthId;
  level: HealthLevel;
  /**
   * 一句短语里的那个「值」，语言无关：权限原文、数字、`12m` 这类缩写、以及
   * 逗号连接的 id。翻译时不改它，直接嵌进句子。
   */
  value: string;
}

/**
 * 体检需要的全部输入。字段收窄成结构类型，测试里不必造一份完整的 `Settings`。
 * `nowMs` 由调用方给 —— 和 `formatAge` 同一个理由：纯函数不该偷偷读时钟。
 */
export interface HealthInput {
  /** `get_notify_permission` 的返回：granted / denied / prompt / unknown */
  permission: string;
  notify: boolean;
  sound: boolean;
  alertAfterSecs: number;
  quietHours: boolean;
  mutedGroups: number;
  approveTools: boolean;
  agents: AgentStatus[];
  receipts: Receipt[];
  custom: CustomAgentsInfo | null;
  shortcuts: ShortcutStatus[];
  hookLogLines: string[];
  sessionCount: number;
  nowMs: number;
}

/** 毫秒 → `45s` / `12m` / `3h` / `2d`。和界面上的 `formatAge` 同一套缩写。 */
export function ageLabel(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}

export function buildHealth(input: HealthInput): HealthCheck[] {
  const checks: HealthCheck[] = [];
  const add = (id: HealthId, level: HealthLevel, value: string) =>
    checks.push({ id, level, value });

  // 1) 通知权限。这是上一轮「通知没弹也没响」最可能的那一处：没权限时
  //    builder.show() 是静默失败，界面和日志都不会报错。
  add(
    "permission",
    input.permission === "granted"
      ? "ok"
      : input.permission === "denied"
        ? "fail"
        : "warn",
    input.permission || "unknown",
  );
  add("notify", input.notify ? "ok" : "info", input.notify ? "on" : "off");
  add("sound", input.sound ? "ok" : "info", input.sound ? "on" : "off");
  // 延迟不等于坏事，但「为什么刚才没立刻响」大半是它，所以如实列出。
  add(
    "alertDelay",
    input.alertAfterSecs > 0 ? "info" : "ok",
    String(Math.max(0, Math.floor(input.alertAfterSecs))),
  );
  add("quiet", input.quietHours ? "warn" : "ok", input.quietHours ? "on" : "off");
  add(
    "muted",
    input.mutedGroups > 0 ? "info" : "ok",
    String(input.mutedGroups),
  );

  // 2) 接入是否真的活着。判据和接入页一致：`installed && !command_ok &&
  //    http !== true` 才是「装了却跑不起来」——HTTP 式没有命令可解析。
  const installed = input.agents.filter((agent) => agent.installed);
  const broken = installed
    .filter((agent) => !agent.command_ok && agent.http !== true)
    .map((agent) => agent.id);
  const stale = installed.filter((agent) => agent.stale).map((agent) => agent.id);
  add("install", installed.length > 0 ? "ok" : "warn", String(installed.length));
  if (broken.length > 0) add("installBroken", "fail", broken.join(", "));
  if (stale.length > 0) add("installStale", "warn", stale.join(", "));

  // 3) 有没有事件真的打过来。配置文件写着我们的命令只说明「装过」，
  //    回执才说明「在报」（见 receipts.rs）。未来时间戳当成 0 岁，不猜原因。
  const lastMs = input.receipts.reduce(
    (newest, receipt) => Math.max(newest, receipt.last_epoch_ms || 0),
    0,
  );
  const age = lastMs > 0 ? Math.max(0, input.nowMs - lastMs) : 0;
  add(
    "receipt",
    lastMs === 0 ? "warn" : age <= RECEIPT_FRESH_MS ? "ok" : "warn",
    lastMs === 0 ? "0" : ageLabel(age),
  );

  // 4) 「从灯上审批」开了，却没有 Claude Code 的 HTTP 式接入 —— 这个组合
  //    点了也不会有确认框（四处闸门都写死 claude-code，见 docs/api.md §4.17）。
  const httpClaude = installed.some(
    (agent) => agent.id === "claude-code" && agent.http === true,
  );
  add(
    "approve",
    !input.approveTools
      ? "info"
      : httpClaude
        ? "ok"
        : "warn",
    input.approveTools ? "on" : "off",
  );

  // 5) 自定义预设解析错误会被静默忽略，必须主动说出来。
  add(
    "custom",
    !input.custom
      ? "info"
      : input.custom.problems.length > 0
        ? "fail"
        : "ok",
    input.custom ? String(input.custom.problems.length) : "-",
  );
  const taken = input.shortcuts.filter((shortcut) => !shortcut.ok).length;
  add("shortcut", taken > 0 ? "warn" : "ok", String(taken));
  add(
    "hookLog",
    input.hookLogLines.length > 0 ? "ok" : "info",
    String(input.hookLogLines.length),
  );
  add("sessions", "info", String(input.sessionCount));

  return checks;
}

export function healthCounts(checks: HealthCheck[]): Record<HealthLevel, number> {
  const counts: Record<HealthLevel, number> = { ok: 0, warn: 0, fail: 0, info: 0 };
  for (const check of checks) {
    if (check.level in counts) counts[check.level] += 1;
  }
  return counts;
}

/** 一句话结论：有一个红就是红，没有红有黄就是黄，其余算通。 */
export function healthVerdict(checks: HealthCheck[]): HealthLevel {
  const { fail, warn } = healthCounts(checks);
  return fail > 0 ? "fail" : warn > 0 ? "warn" : "ok";
}

/**
 * 复制出去的报告。**同一句文案同时服务界面和报告**：调用方把 `m.healthLine`
 * 递进来，`header` 里带时间由调用方生成（纯函数不读时钟）。
 */
export function healthReport(
  checks: HealthCheck[],
  line: (check: HealthCheck) => string,
  levelWord: (level: HealthLevel) => string,
  header: string,
): string {
  return [
    header,
    ...checks.map(
      (check) => `- [${levelWord(check.level)}] ${line(check)}`,
    ),
  ].join("\n");
}
