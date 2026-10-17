import { describe, expect, it } from "vitest";
import { groupReceipts, type Receipt } from "./useAgents";

function receipt(agent: string, source: string): Receipt {
  return { agent, source, last_epoch_ms: 1, state: "working", count: 1 };
}

const KNOWN = new Set(["claude-code", "opencode"]);

describe("groupReceipts", () => {
  it("按声明把非智能体的信号源分成系统和自定义", () => {
    const { system, custom } = groupReceipts(
      [
        receipt("build-watch", "system"),
        receipt("my-script", "custom"),
        receipt("aider", "agent"),
        receipt("legacy", ""),
      ],
      KNOWN,
    );

    expect(system.map((r) => r.agent)).toEqual(["build-watch"]);
    // 声明成 agent 但名字不在清单里的（`aider`）也落在自定义 —— 那一组的话是
    // 「不在上面清单里的都在这儿」，而不是「用户说了 custom 的」
    expect(custom.map((r) => r.agent)).toEqual(["my-script", "aider", "legacy"]);
  });

  it("清单里的智能体不进这两组", () => {
    const { system, custom } = groupReceipts(
      [receipt("claude-code", "system"), receipt("opencode", "custom")],
      KNOWN,
    );

    expect(system).toEqual([]);
    expect(custom).toEqual([]);
  });
});
