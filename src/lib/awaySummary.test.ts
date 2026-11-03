import { describe, expect, it } from "vitest";
import { AWAY_MIN_MS, buildAwaySummary, isQuiet } from "./awaySummary";
import type { InboxModel, InboxRow } from "./inboxRows";
import type { SignalState } from "./types";

function row(id: string, state: SignalState, ageMs: number): InboxRow {
  return {
    key: `s:${id}`,
    session: id,
    group: "g",
    label: id,
    agent: "claude-code",
    state,
    message: null,
    ageMs,
    cwd: "/tmp/p",
    machine: null,
  };
}

/** 把行塞进它们该在的段里 —— 摘要只看得到分段之后的结果。 */
function modelOf(rows: InboxRow[]): InboxModel {
  const out: InboxModel = { need: [], running: [], done: [], quiet: [], total: 0 };
  for (const item of rows) {
    if (item.state === "permission" || item.state === "attention" || item.state === "error") {
      out.need.push(item);
    } else if (item.state === "thinking" || item.state === "working") {
      out.running.push(item);
    } else if (item.state === "done") {
      out.done.push(item);
    } else {
      out.quiet.push(item);
    }
  }
  out.total = out.need.length;
  return out;
}

const NOW = 1_000_000_000;
const MIN = 60_000;

describe("buildAwaySummary", () => {
  it("没有基准就不猜", () => {
    expect(buildAwaySummary(modelOf([row("a", "done", 1000)]), NOW, null)).toBeNull();
  });

  it("离开不足 10 分钟不打扰", () => {
    const lastSeen = NOW - 9 * MIN;
    expect(buildAwaySummary(modelOf([row("a", "done", MIN)]), NOW, lastSeen)).toBeNull();
  });

  it("刚好 10 分钟就算离开过", () => {
    const lastSeen = NOW - AWAY_MIN_MS;
    const summary = buildAwaySummary(modelOf([row("a", "done", MIN)]), NOW, lastSeen);
    expect(summary?.awayMs).toBe(AWAY_MIN_MS);
    expect(summary?.done).toBe(1);
  });

  it("只统计离开之后才变成现在这样的会话", () => {
    const lastSeen = NOW - 20 * MIN;
    const summary = buildAwaySummary(
      modelOf([
        // 30 分钟前就完成了 → 离开前就在，不算
        row("old", "done", 30 * MIN),
        // 5 分钟前完成 → 离开期间完成
        row("new", "done", 5 * MIN),
      ]),
      NOW,
      lastSeen,
    );
    expect(summary?.done).toBe(1);
  });

  it("四档分别计数，并记下等得最久的那一条", () => {
    const lastSeen = NOW - 60 * MIN;
    const summary = buildAwaySummary(
      modelOf([
        row("d1", "done", 10 * MIN),
        row("d2", "done", 12 * MIN),
        row("e1", "error", 15 * MIN),
        row("p1", "permission", 20 * MIN),
        row("p2", "attention", 40 * MIN),
        row("w1", "working", 2 * MIN),
      ]),
      NOW,
      lastSeen,
    );
    expect(summary).toMatchObject({ done: 2, failed: 1, need: 2, running: 1 });
    expect(summary?.longestWaitMs).toBe(40 * MIN);
  });

  it("idle / stale 不计入「发生了什么」", () => {
    const lastSeen = NOW - 30 * MIN;
    const summary = buildAwaySummary(
      modelOf([row("i", "idle", 5 * MIN), row("s", "stale", 6 * MIN)]),
      NOW,
      lastSeen,
    );
    expect(summary).not.toBeNull();
    expect(isQuiet(summary!)).toBe(true);
  });
});

describe("isQuiet", () => {
  it("四档全零才算安静", () => {
    const base = { awayMs: 20 * MIN, done: 0, failed: 0, need: 0, running: 0, longestWaitMs: 0 };
    expect(isQuiet(base)).toBe(true);
    expect(isQuiet({ ...base, running: 1 })).toBe(false);
    expect(isQuiet({ ...base, need: 1 })).toBe(false);
  });
});
