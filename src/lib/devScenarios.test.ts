import { describe, expect, it } from "vitest";
import {
  DEV_LIGHT_IDS,
  DEV_LIGHT_PRESETS,
  DEV_SCENARIOS,
  DEV_SESSION_PREFIX,
  DEV_STATES,
  DEV_TTL_MS,
  devSessionId,
} from "./devScenarios";
import { REPORTABLE_SIGNALS } from "./types";
import { SLOT_IDS } from "./useSettings";

describe("开发者模拟剧本", () => {
  it("只上报真实产品收得到的状态", () => {
    for (const state of DEV_STATES) {
      expect(REPORTABLE_SIGNALS).toContain(state);
      expect(state).not.toBe("off");
    }
  });

  it("每个场景的会话名都唯一，且都带 dev 前缀", () => {
    for (const scenario of DEV_SCENARIOS) {
      const ids = scenario.sessions.map(devSessionId);
      expect(ids.length).toBe(new Set(ids).size);
      for (const id of ids) expect(id.startsWith(DEV_SESSION_PREFIX)).toBe(true);
    }
  });

  it("场景里没有产品造不出来的状态", () => {
    for (const scenario of DEV_SCENARIOS) {
      for (const session of scenario.sessions) {
        expect(REPORTABLE_SIGNALS).toContain(session.state);
        expect(session.message.length).toBeGreaterThan(0);
      }
    }
  });

  it("每个灯位预设都写全六个槽", () => {
    for (const id of DEV_LIGHT_IDS) {
      const lights = DEV_LIGHT_PRESETS[id];
      expect(lights).toBeDefined();
      for (const slot of SLOT_IDS) expect(lights[slot]).toBeDefined();
    }
  });

  it("TTL 三档递增且都在 Rust 的 30 分钟兜底之内", () => {
    const values = [DEV_TTL_MS.short, DEV_TTL_MS.medium, DEV_TTL_MS.long];
    expect(values[0]).toBeLessThan(values[1]!);
    expect(values[1]).toBeLessThan(values[2]!);
    expect(values[2]!).toBeLessThanOrEqual(30 * 60 * 1000);
  });
});
