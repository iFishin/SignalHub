/**
 * 「本周洞察」—— 把已经躺在盘上的事件流水和审批日志，按项目 / 智能体 / 工具
 * 重新组织成几句话。
 *
 * 这是 `digest.ts` 的另一半：**今日复盘**回答「今天这盏灯亮了多久」，按聚合状态
 * 计时，看不见会话；**本周洞察**回答「这几天谁在忙、哪个项目最闹、什么最常被我
 * 拒绝」，按会话和审批记录数数。两份都不造事件、不猜「本来可能发生了什么」。
 *
 * 输出是一组 `InsightFact`，不是拼好的中文：界面和「复制洞察」共用
 * `m.insightFact(fact)` 这一句，两种语言、两个出口不会说两套话（和复盘同一个
 * 取舍）。纯函数不碰时区 —— 日期分桶由调用方给 `dayKey`。
 */
import type { ApprovalRecord } from "./useApprovalLog";

/** Rust `events::Event` 的序列化契约，字段名逐字对应。 */
export interface InsightEvent {
  ts_ms: number;
  /** `start` / `change` / `end` */
  kind: string;
  session: string;
  group: string;
  agent: string;
  machine: string;
  state: string;
  cwd: string;
  message: string;
}

export type InsightFact =
  /** 这段时间一条记录都没有 —— 单独一条，不叠一串 0。 */
  | { kind: "empty" }
  /** 活跃过的会话数，以及其中完成 / 失败过的。 */
  | { kind: "volume"; sessions: number; finished: number; failed: number; days: number }
  /** 按会话去重后，出现在最多会话里的那一天。 */
  | { kind: "peakDay"; day: string; sessions: number }
  /** 目录末段（Windows 的 `\` 也认）；没有 cwd 的会话不参与。 */
  | { kind: "topProject"; name: string; sessions: number }
  | { kind: "topAgent"; name: string; sessions: number }
  /** 最常被拒绝的工具，以及这段时间一共拒了多少次。 */
  | { kind: "deniedTool"; tool: string; count: number; total: number }
  /** 等审批的时间：总和与中位数。 */
  | { kind: "wait"; totalMs: number; medianMs: number; count: number };

export interface InsightInput {
  events: InsightEvent[];
  approvals: ApprovalRecord[];
  sinceMs: number;
  nowMs: number;
  days: number;
  /** 本地日期 `YYYY-MM-DD`；由调用方算，纯函数不碰时区。 */
  dayKey: (tsMs: number) => string;
}

function inWindow(ts: number, sinceMs: number, nowMs: number): boolean {
  return Number.isFinite(ts) && ts >= sinceMs && ts <= nowMs;
}

/** 目录末段。两种分隔符都认：远程 / Windows 会话的路径不一定长什么样。 */
function projectOf(cwd: string): string {
  const trimmed = cwd.trim().replace(/[\\/]+$/, "");
  if (!trimmed) return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}

function median(values: number[]): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 === 1
    ? sorted[mid]
    : Math.round((sorted[mid - 1] + sorted[mid]) / 2);
}

/** 计数表里最大的那个；平局时按名字排，保证界面不会每次刷新换个说法。 */
function topOf(counts: Map<string, number>): [string, number] | null {
  const entries = [...counts.entries()].filter(([name, count]) => name !== "" && count > 0);
  if (entries.length === 0) return null;
  entries.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  return entries[0];
}

export function buildInsights(input: InsightInput): InsightFact[] {
  const { sinceMs, nowMs, days, dayKey } = input;
  const events = input.events.filter((event) => inWindow(event.ts_ms, sinceMs, nowMs));
  const approvals = input.approvals.filter((record) =>
    inWindow(record.ts_ms, sinceMs, nowMs),
  );
  if (events.length === 0 && approvals.length === 0) return [{ kind: "empty" }];

  // 一个会话算一次。事件是倒序给的（新的在前），所以第一次遇到的就是最新的
  // 那条元数据；`cwd` / `agent` 在同一个会话里本来也不该变。
  const meta = new Map<string, { cwd: string; agent: string }>();
  const finished = new Set<string>();
  const failed = new Set<string>();
  const daysCount = new Map<string, Set<string>>();
  for (const event of events) {
    if (!event.session) continue;
    if (!meta.has(event.session)) {
      meta.set(event.session, { cwd: event.cwd ?? "", agent: event.agent ?? "" });
    }
    if (event.state === "done") finished.add(event.session);
    if (event.state === "error") failed.add(event.session);
    // 活跃日按「这一天出现过事件的会话」去重，而不是只数 start：跨天的长会话
    // 在第二天没有 start，但它那天确实还在跑，漏掉会让「最忙的一天」失真。
    const day = dayKey(event.ts_ms);
    if (!daysCount.has(day)) daysCount.set(day, new Set());
    daysCount.get(day)!.add(event.session);
  }

  const facts: InsightFact[] = [
    {
      kind: "volume",
      sessions: meta.size,
      finished: finished.size,
      failed: failed.size,
      days,
    },
  ];

  const dayCounts = new Map(
    [...daysCount.entries()].map(([day, ids]) => [day, ids.size]),
  );
  const peak = topOf(dayCounts);
  // 只有一天时「最忙的是今天」是废话，不说。
  if (peak && dayCounts.size > 1 && peak[1] > 0) {
    facts.push({ kind: "peakDay", day: peak[0], sessions: peak[1] });
  }

  const projects = new Map<string, number>();
  const agents = new Map<string, number>();
  for (const info of meta.values()) {
    const project = projectOf(info.cwd);
    if (project) projects.set(project, (projects.get(project) ?? 0) + 1);
    if (info.agent) agents.set(info.agent, (agents.get(info.agent) ?? 0) + 1);
  }
  const topProject = topOf(projects);
  if (topProject) facts.push({ kind: "topProject", name: topProject[0], sessions: topProject[1] });
  const topAgent = topOf(agents);
  if (topAgent) facts.push({ kind: "topAgent", name: topAgent[0], sessions: topAgent[1] });

  const denied = approvals.filter((record) => record.decision === "deny");
  const tools = new Map<string, number>();
  for (const record of denied) {
    const tool = record.tool || "?";
    tools.set(tool, (tools.get(tool) ?? 0) + 1);
  }
  const topTool = topOf(tools);
  if (topTool) {
    facts.push({ kind: "deniedTool", tool: topTool[0], count: topTool[1], total: denied.length });
  }

  const waits = approvals
    .map((record) => record.waited_ms)
    .filter((ms) => Number.isFinite(ms) && ms >= 0);
  if (waits.length > 0) {
    facts.push({
      kind: "wait",
      totalMs: waits.reduce((sum, ms) => sum + ms, 0),
      medianMs: median(waits),
      count: waits.length,
    });
  }

  return facts;
}

/** 复制出去的 Markdown。和卡片上同一句文案，只多一个标题。 */
export function insightsMarkdown(
  facts: InsightFact[],
  line: (fact: InsightFact) => string,
  header: string,
): string {
  return [header, ...facts.map((fact) => `- ${line(fact)}`)].join("\n");
}
