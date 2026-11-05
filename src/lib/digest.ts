/**
 * 「今日复盘」——回来之后知道今天发生过什么，不用去翻三张表。
 *
 * 数据全部来自已经存在的命令：`get_stats`（今日分状态时长）、`get_history`
 * （近 7 天）、`get_approval_history`（审批记录）。这里**只做加法和挑最大值**，
 * 不造事件流水，也不猜「出现过又消失的会话」——那些在快照里已经没了，硬编
 * 一段出来就是撒谎（同样的取舍见 `awaySummary.ts`）。
 *
 * 输出是一组 `DigestFact`，而不是拼好的中文：界面和「复制复盘」共用
 * `m.digestFact(fact)` 这一句，两种语言、两个出口都不会说两套话。
 */
import type { StatRow } from "./useStats";
import type { DayRow } from "./useHistory";
import type { ApprovalRecord } from "./useApprovalLog";

export type DigestFact =
  /** 今天还没有任何数据 —— 单独一条，不叠一句「0s」。 */
  | { kind: "empty" }
  /** 灯亮着的总时长（各状态之和，Rust 保证区间互不重叠）。 */
  | { kind: "total"; seconds: number }
  /** 占得最多的那个状态。label 已经由 Rust 按当前语言翻好。 */
  | { kind: "top"; label: string; seconds: number }
  /** 真正在干活：thinking + working。 */
  | { kind: "busy"; seconds: number }
  /** 在等你：permission + attention。 */
  | { kind: "wait"; seconds: number }
  | {
      kind: "approvals";
      total: number;
      allow: number;
      deny: number;
      expired: number;
      longestWaitMs: number;
    }
  /** 和之前那些天的日均比。只看有数据的天，没有可比对象就不说。 */
  | { kind: "compare"; deltaSeconds: number; days: number };

export interface DigestInput {
  /** `get_stats` 的返回 */
  stats: StatRow[];
  /** `get_history` 的返回，今天可能在也可能不在 */
  days: DayRow[];
  /** `get_approval_history` 的返回（上限 100 条，如实说明见界面 hint） */
  approvals: ApprovalRecord[];
  /** 本地日期 `YYYY-MM-DD`，和传给 Rust 的 `today` 是同一个值 */
  today: string;
  /** 本地零点的时间戳。由调用方算 —— 纯函数不碰时区。 */
  todayStartMs: number;
  nowMs: number;
}

function sumSeconds(rows: StatRow[]): number {
  return rows.reduce(
    (total, row) =>
      total + (Number.isFinite(row.seconds) && row.seconds > 0 ? row.seconds : 0),
    0,
  );
}

/** 今日审批。`ts_ms` 是墙上时钟，未来时间戳不算 —— 宁可少一条也不编。 */
export function todayApprovals(
  records: ApprovalRecord[],
  todayStartMs: number,
  nowMs: number,
): ApprovalRecord[] {
  return records.filter(
    (record) =>
      Number.isFinite(record.ts_ms) &&
      record.ts_ms >= todayStartMs &&
      record.ts_ms <= nowMs,
  );
}

export function buildDigest(input: DigestInput): DigestFact[] {
  const total = sumSeconds(input.stats);
  const approvals = todayApprovals(
    input.approvals,
    input.todayStartMs,
    input.nowMs,
  );

  // 没有时长也没有审批，就是「今天还没开始」。别输出一串 0。
  if (total === 0 && approvals.length === 0) return [{ kind: "empty" }];

  const facts: DigestFact[] = [];
  if (total > 0) facts.push({ kind: "total", seconds: total });

  const rows = input.stats
    .filter((row) => Number.isFinite(row.seconds) && row.seconds > 0)
    .sort((a, b) => b.seconds - a.seconds);
  if (rows.length > 0) {
    facts.push({ kind: "top", label: rows[0].label, seconds: rows[0].seconds });
  }

  const busy = sumSeconds(
    input.stats.filter(
      (row) => row.state === "thinking" || row.state === "working",
    ),
  );
  if (busy > 0) facts.push({ kind: "busy", seconds: busy });

  const wait = sumSeconds(
    input.stats.filter(
      (row) => row.state === "permission" || row.state === "attention",
    ),
  );
  if (wait > 0) facts.push({ kind: "wait", seconds: wait });

  if (approvals.length > 0) {
    const count = (decision: string) =>
      approvals.filter((record) => record.decision === decision).length;
    const longestWaitMs = approvals.reduce((longest, record) => {
      const waited = Number.isFinite(record.waited_ms)
        ? Math.max(0, record.waited_ms)
        : 0;
      return Math.max(longest, waited);
    }, 0);
    facts.push({
      kind: "approvals",
      total: approvals.length,
      allow: count("allow"),
      deny: count("deny"),
      expired: count("expired"),
      longestWaitMs,
    });
  }

  // 和「之前那些有数据的天」的日均比。今天自己不算进去，否则今天忙会同时
  // 抬高基准，差值被稀释 —— 那句「比平时多 / 少」就没意义了。
  const prior = input.days.filter(
    (day) => day.day !== input.today && day.total_seconds > 0,
  );
  if (total > 0 && prior.length > 0) {
    const average =
      prior.reduce((sum, day) => sum + day.total_seconds, 0) / prior.length;
    facts.push({
      kind: "compare",
      deltaSeconds: Math.round(total - average),
      days: prior.length,
    });
  }

  return facts;
}

/** 复制出去的 Markdown。和卡片上同一句文案，只多一个标题。 */
export function digestMarkdown(
  facts: DigestFact[],
  line: (fact: DigestFact) => string,
  header: string,
): string {
  return [header, ...facts.map((fact) => `- ${line(fact)}`)].join("\n");
}
