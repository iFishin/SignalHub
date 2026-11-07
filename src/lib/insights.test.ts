import { describe, expect, it } from "vitest";
import { buildInsights, insightsMarkdown, type InsightEvent } from "./insights";
import type { ApprovalRecord } from "./useApprovalLog";

const NOW = 1_704_240_000_000;
const DAY = 86_400_000;
const SINCE = NOW - 7 * DAY;
const dayKey = (ts: number) => new Date(ts).toISOString().slice(0, 10);

function ev(over: Partial<InsightEvent> = {}): InsightEvent {
  return {
    ts_ms: NOW - 60_000,
    kind: "start",
    session: "s1",
    group: "g",
    agent: "claude-code",
    machine: "",
    state: "working",
    cwd: "/work/a",
    message: "",
    ...over,
  };
}

function approval(over: Partial<ApprovalRecord> = {}): ApprovalRecord {
  return {
    ts_ms: NOW - 60_000,
    session: "s1",
    agent: "claude-code",
    tool: "Bash",
    target: "",
    detail: "",
    truncated: false,
    decision: "deny",
    reason: "unsafe",
    source: "lamp",
    waited_ms: 1000,
    ...over,
  };
}

function input(events: InsightEvent[], approvals: ApprovalRecord[] = []) {
  return { events, approvals, sinceMs: SINCE, nowMs: NOW, days: 7, dayKey };
}

describe("buildInsights", () => {
  it("一条记录都没有就只说一句，不叠一串 0", () => {
    expect(buildInsights(input([]))).toEqual([{ kind: "empty" }]);
  });

  it("按会话去重统计活跃 / 完成 / 失败", () => {
    const facts = buildInsights(
      input([
        ev({ session: "s1", ts_ms: NOW - 6 * DAY }),
        ev({ session: "s1", kind: "change", state: "done", ts_ms: NOW - 5 * DAY }),
        ev({ session: "s2", state: "error", ts_ms: NOW - 2 * DAY }),
        ev({ session: "s2", kind: "end", state: "error", ts_ms: NOW - DAY }),
        ev({ session: "s3", ts_ms: NOW - DAY }),
      ]),
    );
    expect(facts[0]).toMatchObject({ kind: "volume", sessions: 3, finished: 1, failed: 1 });
  });

  it("窗口外的事件和未来的时间戳都不算", () => {
    const facts = buildInsights(
      input([
        ev({ session: "old", ts_ms: SINCE - DAY }),
        ev({ session: "future", ts_ms: NOW + DAY }),
      ]),
    );
    expect(facts).toEqual([{ kind: "empty" }]);
  });

  it("项目取目录末段，Windows 路径也认；平局按名字排", () => {
    const facts = buildInsights(
      input([
        ev({ session: "s1", cwd: "/work/a", agent: "codex", ts_ms: NOW - 6 * DAY }),
        ev({ session: "s2", cwd: "C:\\work\\b", agent: "codex", ts_ms: NOW - 2 * DAY }),
        ev({ session: "s3", cwd: "/work/a", agent: "claude-code", ts_ms: NOW - DAY }),
      ]),
    );
    expect(facts).toContainEqual({ kind: "topProject", name: "a", sessions: 2 });
    expect(facts).toContainEqual({ kind: "topAgent", name: "codex", sessions: 2 });
  });

  it("没有 cwd 的会话不算项目，不能凭空造一个", () => {
    const facts = buildInsights(input([ev({ cwd: "" })]));
    expect(facts.some((fact) => fact.kind === "topProject")).toBe(false);
  });

  it("最常被拒绝的工具 + 等待总和与中位数", () => {
    const facts = buildInsights(
      input(
        [],
        [
          approval({ tool: "Bash", waited_ms: 1000 }),
          approval({ tool: "Bash", waited_ms: 3000 }),
          approval({ tool: "Edit", waited_ms: 5000 }),
          approval({ tool: "Read", decision: "allow", waited_ms: 9000 }),
        ],
      ),
    );
    expect(facts).toContainEqual({ kind: "deniedTool", tool: "Bash", count: 2, total: 3 });
    expect(facts).toContainEqual({ kind: "wait", totalMs: 18_000, medianMs: 4000, count: 4 });
  });

  it("只有一天时不说「最忙的是今天」这种废话", () => {
    const facts = buildInsights(
      input([ev({ session: "s1" }), ev({ session: "s2" })]),
    );
    expect(facts.some((fact) => fact.kind === "peakDay")).toBe(false);
  });

  it("跨两天的 start 会挑出会话更多的那天", () => {
    const facts = buildInsights(
      input([
        ev({ session: "s1", ts_ms: NOW - 3 * DAY }),
        ev({ session: "s2", ts_ms: NOW - 2 * DAY }),
        ev({ session: "s3", ts_ms: NOW - 2 * DAY }),
      ]),
    );
    const peak = facts.find((fact) => fact.kind === "peakDay");
    expect(peak).toMatchObject({ kind: "peakDay", sessions: 2 });
  });

  it("活跃日按所有事件算，跨天的长会话也计入（不只数 start）", () => {
    const facts = buildInsights(
      input([
        ev({ session: "s1", ts_ms: NOW - 3 * DAY }),
        ev({ session: "s1", kind: "change", state: "done", ts_ms: NOW - 2 * DAY }),
        ev({ session: "s2", kind: "change", state: "done", ts_ms: NOW - 2 * DAY }),
        ev({ session: "s3", kind: "end", state: "done", ts_ms: NOW - 2 * DAY }),
      ]),
    );
    // 只数 start 的话第二天一条都没有；现在第二天有 3 个会话，应当胜出。
    expect(facts.find((fact) => fact.kind === "peakDay")).toMatchObject({
      kind: "peakDay",
      sessions: 3,
    });
  });
});

describe("insightsMarkdown", () => {
  it("标题加一行一条事实", () => {
    const text = insightsMarkdown(
      [{ kind: "empty" }, { kind: "volume", sessions: 2, finished: 1, failed: 0, days: 7 }],
      (fact) => fact.kind,
      "HEADER",
    );
    expect(text).toBe("HEADER\n- empty\n- volume");
  });
});
