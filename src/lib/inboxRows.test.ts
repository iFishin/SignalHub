import { describe, expect, it } from "vitest";
import { buildInbox, formatAge } from "./inboxRows";
import type { Approval } from "./useApprovals";
import type { GroupMeta } from "./useSettings";
import type { GroupView, SessionView, SignalState, Snapshot } from "./types";

function session(
  id: string,
  state: SignalState,
  extra: Partial<SessionView> = {},
): SessionView {
  return {
    id,
    state,
    agent: "claude-code",
    cwd: "/tmp/project",
    message: null,
    age_ms: 10_000,
    stale_progress: null,
    ...extra,
  };
}

function group(name: string, sessions: SessionView[], label?: string): GroupView {
  return {
    name,
    default_label: label ?? null,
    aggregate: sessions[0]?.state ?? "offline",
    message: sessions[0]?.message ?? null,
    since_ms: 0,
    stale_progress: null,
    sessions,
  };
}

function snapshot(groups: GroupView[]): Snapshot {
  return { aggregate: groups[0]?.aggregate ?? "offline", paused: false, since_ms: 0, message: null, groups };
}

function approval(sessionId: string, age = 5_000): Approval {
  return {
    id: `id-${sessionId}`,
    session: sessionId,
    tool: "Bash",
    target: "npm test",
    detail: "npm test",
    truncated: false,
    allowable: true,
    age_ms: age,
  };
}

const META: Record<string, GroupMeta> = { work: { name: "工作" } };

describe("buildInbox", () => {
  it("把会话按「需要你 / 在跑 / 完成 / 安静」分开", () => {
    const model = buildInbox(
      snapshot([
        group("work", [
          session("err", "error"),
          session("perm", "permission"),
          session("run", "working"),
          session("think", "thinking"),
          session("done", "done"),
          session("idle", "idle"),
          session("quiet", "stale"),
        ]),
      ]),
      [],
      META,
    );

    expect(model.need.map((row) => row.session)).toEqual(["err", "perm"]);
    expect(model.running.map((row) => row.session)).toEqual(["run", "think"]);
    expect(model.done.map((row) => row.session)).toEqual(["done"]);
    expect(model.quiet.map((row) => row.session)).toEqual(["idle", "quiet"]);
    expect(model.total).toBe(2);
  });

  it("同一个会话的审批只画一行，并带上审批", () => {
    const model = buildInbox(
      snapshot([group("work", [session("perm", "permission")])]),
      [approval("perm")],
      META,
    );

    expect(model.need).toHaveLength(1);
    const row = model.need[0];
    expect(row.session).toBe("perm");
    expect(row.approval?.id).toBe("id-perm");
    // 审批的等待时间比会话状态更准
    expect(row.ageMs).toBe(5_000);
  });

  it("审批登记表里多出来的会话（快照里没有）也要显示", () => {
    const model = buildInbox(snapshot([]), [approval("ghost")], META);
    expect(model.need).toHaveLength(1);
    expect(model.need[0].session).toBe("ghost");
    expect(model.need[0].group).toBe("");
  });

  it("需要你的排序：出错 > 授权 > 关注，同类里等得最久的在前", () => {
    const model = buildInbox(
      snapshot([
        group("work", [
          session("attention-old", "attention", { age_ms: 90_000 }),
          session("error", "error", { age_ms: 1_000 }),
          session("perm-new", "permission", { age_ms: 1_000 }),
          session("perm-old", "permission", { age_ms: 60_000 }),
        ]),
      ]),
      [],
      META,
    );

    expect(model.need.map((row) => row.session)).toEqual([
      "error",
      "perm-old",
      "perm-new",
      "attention-old",
    ]);
  });

  it("组名优先用用户改的显示名", () => {
    const model = buildInbox(
      snapshot([group("work", [session("run", "working")], "默认名")]),
      [],
      META,
    );
    expect(model.running[0].label).toBe("工作");
  });

  it("没有默认名时退回原始组名", () => {
    const model = buildInbox(
      snapshot([group("ci", [session("run", "working")])]),
      [],
      {},
    );
    expect(model.running[0].label).toBe("ci");
  });
});

describe("formatAge", () => {
  it("按秒 / 分 / 时 / 天给一个单位", () => {
    expect(formatAge(0)).toBe("0s");
    expect(formatAge(59_999)).toBe("59s");
    expect(formatAge(60_000)).toBe("1m");
    expect(formatAge(3_600_000)).toBe("1h");
    expect(formatAge(86_400_000)).toBe("1d");
  });

  it("负数和 NaN 不变成怪字符串", () => {
    expect(formatAge(-1)).toBe("0s");
    expect(formatAge(Number.NaN)).toBe("0s");
  });
});
