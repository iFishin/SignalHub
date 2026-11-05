import { describe, expect, it } from "vitest";
import {
  RECEIPT_FRESH_MS,
  ageLabel,
  buildHealth,
  healthCounts,
  healthReport,
  healthVerdict,
  type HealthCheck,
  type HealthInput,
} from "./health";
import type { AgentStatus, Receipt } from "./useAgents";

const NOW = 1_700_000_000_000;

function agent(id: string, over: Partial<AgentStatus> = {}): AgentStatus {
  return {
    id,
    name: id,
    detected: true,
    installed: true,
    config: `/tmp/${id}.json`,
    command: "signalhub hook claude-code",
    command_ok: true,
    stale: false,
    http: false,
    custom: false,
    ...over,
  };
}

function receipt(last_epoch_ms: number): Receipt {
  return { agent: "claude-code", last_epoch_ms, state: "working", source: "agent", count: 3 };
}

function base(over: Partial<HealthInput> = {}): HealthInput {
  return {
    permission: "granted",
    notify: true,
    sound: true,
    alertAfterSecs: 0,
    quietHours: false,
    mutedGroups: 0,
    approveTools: false,
    agents: [agent("claude-code")],
    receipts: [receipt(NOW - 5_000)],
    custom: null,
    shortcuts: [{ action: "lamp", label: "⌘⇧L", ok: true }],
    hookLogLines: ["12:00:00 working"],
    sessionCount: 2,
    nowMs: NOW,
    ...over,
  };
}

const byId = (checks: HealthCheck[], id: string) =>
  checks.find((check) => check.id === id);

describe("ageLabel", () => {
  it("和界面上的相对时间同一套缩写", () => {
    expect(ageLabel(0)).toBe("0s");
    expect(ageLabel(45_000)).toBe("45s");
    expect(ageLabel(12 * 60_000)).toBe("12m");
    expect(ageLabel(3 * 3_600_000)).toBe("3h");
    expect(ageLabel(2 * 86_400_000)).toBe("2d");
  });

  it("未来的时间戳不写成负数", () => {
    expect(ageLabel(-10_000)).toBe("0s");
  });
});

describe("buildHealth", () => {
  it("全绿的时候是 ok，且没有「问题专用」的检查项", () => {
    const checks = buildHealth(base());
    expect(healthVerdict(checks)).toBe("ok");
    expect(healthCounts(checks).fail).toBe(0);
    expect(byId(checks, "installBroken")).toBeUndefined();
    expect(byId(checks, "installStale")).toBeUndefined();
  });

  it("权限被拒绝是 fail，unknown / prompt 是 warn", () => {
    expect(byId(buildHealth(base({ permission: "denied" })), "permission")?.level).toBe("fail");
    expect(byId(buildHealth(base({ permission: "prompt" })), "permission")?.level).toBe("warn");
    expect(byId(buildHealth(base({ permission: "" })), "permission")?.value).toBe("unknown");
  });

  it("只有命令式装坏才算 fail；HTTP 式没有命令可解析", () => {
    const broken = buildHealth(base({ agents: [agent("cursor", { command_ok: false })] }));
    expect(byId(broken, "installBroken")?.level).toBe("fail");
    expect(byId(broken, "installBroken")?.value).toBe("cursor");

    const http = buildHealth(
      base({ agents: [agent("claude-code", { http: true, command_ok: false })] }),
    );
    expect(byId(http, "installBroken")).toBeUndefined();
  });

  it("旧版本接入是 warn，不升级也能用但行为停在旧版", () => {
    const checks = buildHealth(base({ agents: [agent("claude-code", { stale: true })] }));
    expect(byId(checks, "installStale")?.level).toBe("warn");
  });

  it("一个都没接、一次都没报过都是 warn", () => {
    const checks = buildHealth(base({ agents: [], receipts: [], hookLogLines: [] }));
    expect(byId(checks, "install")?.level).toBe("warn");
    expect(byId(checks, "receipt")?.level).toBe("warn");
    expect(byId(checks, "receipt")?.value).toBe("0");
  });

  it("回执新鲜度按 15 分钟切，取所有信号源里最新的那条", () => {
    const fresh = buildHealth(
      base({
        receipts: [receipt(NOW - 3_600_000), receipt(NOW - 1_000)],
      }),
    );
    expect(byId(fresh, "receipt")?.level).toBe("ok");
    expect(byId(fresh, "receipt")?.value).toBe("1s");

    const old = buildHealth(base({ receipts: [receipt(NOW - RECEIPT_FRESH_MS - 1)] }));
    expect(byId(old, "receipt")?.level).toBe("warn");
  });

  it("开了从灯上审批却没有 Claude Code HTTP 接入，主动提醒", () => {
    const checks = buildHealth(base({ approveTools: true }));
    expect(byId(checks, "approve")?.level).toBe("warn");

    const ok = buildHealth(
      base({
        approveTools: true,
        agents: [agent("claude-code", { http: true })],
      }),
    );
    expect(byId(ok, "approve")?.level).toBe("ok");
  });

  it("关着的开关是 info 不是 fail —— 那是用户自己的选择", () => {
    const checks = buildHealth(base({ notify: false, sound: false, approveTools: false }));
    expect(byId(checks, "notify")?.level).toBe("info");
    expect(byId(checks, "sound")?.level).toBe("info");
    expect(byId(checks, "approve")?.level).toBe("info");
  });

  it("自定义预设读不懂、快捷键抢不到都要出声", () => {
    const checks = buildHealth(
      base({
        custom: { path: "/tmp/agents.json", problems: ["第 2 行不认识"], families: [] },
        shortcuts: [{ action: "lamp", label: "⌘⇧L", ok: false }],
      }),
    );
    expect(byId(checks, "custom")?.level).toBe("fail");
    expect(byId(checks, "shortcut")?.level).toBe("warn");
    expect(healthVerdict(checks)).toBe("fail");
  });

  it("有黄没红时结论是 warn", () => {
    expect(healthVerdict(buildHealth(base({ quietHours: true })))).toBe("warn");
  });
});

describe("healthReport", () => {
  it("界面和报告用同一句文案，逐项带上等级", () => {
    const checks = buildHealth(base({ permission: "denied" }));
    const text = healthReport(
      checks,
      (check) => `${check.id}=${check.value}`,
      (level) => level.toUpperCase(),
      "HEADER",
    );
    const lines = text.split("\n");
    expect(lines[0]).toBe("HEADER");
    expect(lines).toHaveLength(checks.length + 1);
    expect(lines[1]).toBe("- [FAIL] permission=denied");
  });
});
