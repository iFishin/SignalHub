import { describe, expect, it } from "vitest";
import { MAX_LAMPS, perGroupActive } from "./lampLayout";
import { DEFAULTS, type Settings } from "./useSettings";
import type { GroupView, SignalState } from "./types";

const withSettings = (patch: Partial<Settings>): Settings => ({ ...DEFAULTS, ...patch });

function group(name: string, aggregate: SignalState = "working"): GroupView {
  return {
    name,
    default_label: null,
    aggregate,
    message: null,
    since_ms: 0,
    stale_progress: null,
    sessions: [],
  };
}

describe("perGroupActive", () => {
  /** 默认布局是单灯 —— 新能力默认关（红线 3）。 */
  it("is off in the default single-lamp layout", () => {
    expect(perGroupActive(DEFAULTS, [group("a"), group("b")])).toBe(false);
  });

  it("is on in the per-group layout while the groups fit", () => {
    const settings = withSettings({ lamp_layout: "per-group" });
    expect(perGroupActive(settings, [group("a"), group("b")])).toBe(true);
  });

  /**
   * 组数超过硬上限就回落单灯。**这条必须和 Rust 一致** —— 不然主灯一边以为只画
   * 全局、一边以为要画各组，窗口尺寸立刻对不上。
   */
  it("falls back to single once the visible groups exceed the cap", () => {
    const settings = withSettings({ lamp_layout: "per-group" });
    const many = Array.from({ length: MAX_LAMPS + 1 }, (_, i) => group(`g${i}`));
    expect(perGroupActive(settings, many)).toBe(false);
    // 恰好到上限仍然成立
    expect(perGroupActive(settings, many.slice(0, MAX_LAMPS))).toBe(true);
  });

  /** 被隐藏的组不占名额 —— 和 Rust 的 `shows_group` 同一个判断。 */
  it("does not count hidden groups toward the cap", () => {
    const settings = withSettings({
      lamp_layout: "per-group",
      hidden_groups: ["g0"],
    });
    const many = Array.from({ length: MAX_LAMPS + 1 }, (_, i) => group(`g${i}`));
    expect(perGroupActive(settings, many)).toBe(true);
  });
});
