import { describe, expect, it } from "vitest";
import { applyStylePreset, isActiveStylePreset, type StylePreset } from "./presets";
import { DEFAULTS, type Settings } from "./useSettings";

/** 预设只该动形状和节奏；颜色与静默超时是用户的东西。 */
describe("applyStylePreset", () => {
  it("keeps each state's colour and TTL", () => {
    const merged = applyStylePreset(
      {
        error: { color: "#ff0000", ttl_ms: 60_000 },
        idle: { ttl_ms: 120_000 },
      },
      {
        permission: { shape: "rounded" },
        error: { shape: "rounded" },
      },
    );

    expect(merged.error).toEqual({
      color: "#ff0000",
      ttl_ms: 60_000,
      shape: "rounded",
      blink_ms: null,
    });
    // 预设没提到的状态：颜色 / TTL 留着，形状和节奏回到「跟随全局」
    expect(merged.idle).toEqual({
      color: null,
      ttl_ms: 120_000,
      shape: null,
      blink_ms: null,
    });
    // 预设新加的状态只有它自己那两样
    expect(merged.permission).toEqual({
      color: null,
      ttl_ms: null,
      shape: "rounded",
      blink_ms: null,
    });
  });

  it("follows global (empty preset) without wiping colour or TTL", () => {
    const merged = applyStylePreset(
      { idle: { blink_ms: 0, color: "#123456", ttl_ms: 1_000 } },
      {},
    );
    expect(merged.idle).toEqual({
      color: "#123456",
      ttl_ms: 1_000,
      shape: null,
      blink_ms: null,
    });
  });

  it("drops entries that end up completely empty", () => {
    expect(applyStylePreset({ idle: {} }, {})).toEqual({});
    expect(applyStylePreset({ idle: { blink_ms: 0 } }, {})).toEqual({});
  });
});

/** 「当前是哪一套」只看预设拥有的那两项 —— 颜色和 TTL 与该判断无关。 */
describe("isActiveStylePreset", () => {
  const preset: StylePreset = {
    id: "rhythm",
    label: "rhythm",
    hint: "",
    styles: { working: { blink_ms: 1_600 } },
  };
  const settings = (styles: Settings["styles"]): Settings => ({
    ...DEFAULTS,
    styles,
  });

  it("ignores colour and TTL overrides", () => {
    expect(
      isActiveStylePreset(
        preset,
        settings({ working: { blink_ms: 1_600, color: "#00ff00", ttl_ms: 30_000 } }),
      ),
    ).toBe(true);
  });

  it("is not active when the rhythm differs", () => {
    expect(
      isActiveStylePreset(preset, settings({ working: { blink_ms: 900 } })),
    ).toBe(false);
  });

  it("is not active when another state also overrides the rhythm", () => {
    expect(
      isActiveStylePreset(
        preset,
        settings({ working: { blink_ms: 1_600 }, idle: { blink_ms: 0 } }),
      ),
    ).toBe(false);
  });
});
