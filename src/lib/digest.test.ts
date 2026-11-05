import { describe, expect, it } from "vitest";
import {
  buildDigest,
  digestMarkdown,
  todayApprovals,
  type DigestFact,
  type DigestInput,
} from "./digest";
import type { StatRow } from "./useStats";
import type { DayRow } from "./useHistory";
import type { ApprovalRecord } from "./useApprovalLog";

const TODAY = "2026-11-05";
const START = new Date("2026-11-05T00:00:00").getTime();
const NOW = START + 12 * 3_600_000;

const row = (state: StatRow["state"], seconds: number): StatRow => ({
  state,
  label: state,
  seconds,
});

const day = (date: string, total_seconds: number): DayRow => ({
  day: date,
  total_seconds,
  rows: [row("working", total_seconds)],
});

function approval(over: Partial<ApprovalRecord> = {}): ApprovalRecord {
  return {
    ts_ms: START + 3_600_000,
    session: "s1",
    agent: "claude-code",
    tool: "Bash",
    target: "ls",
    detail: "",
    truncated: false,
    decision: "allow",
    reason: "",
    source: "lamp",
    waited_ms: 4_000,
    ...over,
  };
}

function base(over: Partial<DigestInput> = {}): DigestInput {
  return {
    stats: [row("working", 3600), row("permission", 300), row("thinking", 600)],
    days: [day(TODAY, 4500), day("2026-11-04", 3000), day("2026-11-03", 5000)],
    approvals: [],
    today: TODAY,
    todayStartMs: START,
    nowMs: NOW,
    ...over,
  };
}

const kinds = (facts: DigestFact[]) => facts.map((fact) => fact.kind);
const fact = (facts: DigestFact[], kind: DigestFact["kind"]) =>
  facts.find((entry) => entry.kind === kind);

describe("todayApprovals", () => {
  it("只认本地今天，昨天和未来的都不算", () => {
    const records = [
      approval({ ts_ms: START - 1 }),
      approval({ ts_ms: START }),
      approval({ ts_ms: NOW }),
      approval({ ts_ms: NOW + 1 }),
      approval({ ts_ms: Number.NaN }),
    ];
    const picked = todayApprovals(records, START, NOW);
    expect(picked).toHaveLength(2);
  });
});

describe("buildDigest", () => {
  it("今天什么都没有时只说一句，不叠一串 0", () => {
    expect(buildDigest(base({ stats: [], days: [], approvals: [] }))).toEqual([
      { kind: "empty" },
    ]);
  });

  it("总时长是各状态之和，取占比最大的那个状态", () => {
    const facts = buildDigest(base());
    expect(fact(facts, "total")).toEqual({ kind: "total", seconds: 4500 });
    expect(fact(facts, "top")).toEqual({
      kind: "top",
      label: "working",
      seconds: 3600,
    });
  });

  it("干活 = thinking + working，等你 = permission + attention", () => {
    const facts = buildDigest(
      base({ stats: [row("working", 100), row("thinking", 50), row("attention", 20)] }),
    );
    expect(fact(facts, "busy")).toEqual({ kind: "busy", seconds: 150 });
    expect(fact(facts, "wait")).toEqual({ kind: "wait", seconds: 20 });
  });

  it("零值 / 非法值不进统计，也不产生 0 秒的句子", () => {
    const facts = buildDigest(
      base({
        stats: [row("working", 10), row("done", 0), { ...row("idle", 1), seconds: Number.NaN }],
      }),
    );
    expect(fact(facts, "total")).toEqual({ kind: "total", seconds: 10 });
    // working 那 10s 该算进「真正在跑」；零值和 NaN 不产生额外句子。
    expect(fact(facts, "busy")).toEqual({ kind: "busy", seconds: 10 });
    expect(kinds(facts)).not.toContain("wait");
  });

  it("审批只统计今天，允许 / 拒绝 / 超时和最长等待都算得出来", () => {
    const facts = buildDigest(
      base({
        approvals: [
          approval({ decision: "allow", waited_ms: 3_000 }),
          approval({ decision: "deny", waited_ms: 9_000 }),
          approval({ decision: "expired", waited_ms: 60_000 }),
          approval({ ts_ms: START - 1, decision: "deny" }),
          approval({ decision: "something-else", waited_ms: -5 }),
        ],
      }),
    );
    const a = fact(facts, "approvals");
    expect(a).toEqual({
      kind: "approvals",
      total: 4,
      allow: 1,
      deny: 1,
      expired: 1,
      longestWaitMs: 60_000,
    });
  });

  it("没有时长但有审批时，复盘照样说得出来", () => {
    const facts = buildDigest(base({ stats: [], days: [], approvals: [approval()] }));
    expect(kinds(facts)).toEqual(["approvals"]);
  });

  it("同比排除今天自己，按有数据的那几天取日均", () => {
    // 之前两天 3000 / 5000 → 日均 4000；今天 4500 → 多 500
    expect(fact(buildDigest(base()), "compare")).toEqual({
      kind: "compare",
      deltaSeconds: 500,
      days: 2,
    });
  });

  it("没有可比的日子就不编一句「比平时多」", () => {
    const facts = buildDigest(base({ days: [day(TODAY, 4500)] }));
    expect(kinds(facts)).not.toContain("compare");
  });

  it("days 里没有今天也照样能排除自己", () => {
    const facts = buildDigest(
      base({ days: [day("2026-11-04", 3000), day("2026-11-03", 5000)] }),
    );
    expect(fact(facts, "compare")).toEqual({
      kind: "compare",
      deltaSeconds: 500,
      days: 2,
    });
  });
});

describe("digestMarkdown", () => {
  it("标题 + 每条一句，和卡片共用同一个 line 函数", () => {
    const facts = buildDigest(base());
    const text = digestMarkdown(facts, (f) => `${f.kind}`, "HEADER");
    expect(text.split("\n")).toEqual(["HEADER", ...kinds(facts).map((k) => `- ${k}`)]);
  });
});
