import { describe, expect, it } from "vitest";
import {
  applyStylePreset,
  isActivePreset,
  isActiveStylePreset,
  presets,
  type StylePreset,
} from "./presets";
import { MESSAGES } from "./i18n";
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

/**
 * 外观预设（尺寸 / 折叠 / 显示字段 / 透明度 / 节奏 + 圆角 / 描边 / 字号）。
 *
 * 这几条钉的是「点一下能不能回到同一套」：预设是个按钮，它的选中态完全由
 * 逐字段比对算出来 —— 少比一个字段，用户点完不会高亮，看着像没生效。
 */
describe("外观预设", () => {
  const m = MESSAGES.zh;
  const apply = (preset: { apply: Partial<Settings> }): Settings => ({
    ...DEFAULTS,
    ...preset.apply,
  });

  it("点完之后每一套都能被认出来", () => {
    for (const preset of presets(m)) {
      expect(isActivePreset(preset, apply(preset)), preset.id).toBe(true);
    }
  });

  it("默认外观正好落在「标准」这套上", () => {
    // 第一次打开不该一套都不高亮 —— 那等于告诉用户「你现在什么都不是」
    const standard = presets(m).find((preset) => preset.id === "standard");
    expect(standard).toBeDefined();
    expect(isActivePreset(standard!, DEFAULTS)).toBe(true);
  });

  it("质感三项也参与判断：改了圆角就不再是这一套", () => {
    const standard = presets(m).find((preset) => preset.id === "standard")!;
    expect(isActivePreset(standard, { ...apply(standard), sign_radius: "round" })).toBe(false);
    expect(isActivePreset(standard, { ...apply(standard), sign_border: "none" })).toBe(false);
    expect(isActivePreset(standard, { ...apply(standard), sign_text: "large" })).toBe(false);
  });

  it("不碰牌底色和分层 —— 它们不是风格，是材质和语义", () => {
    // 关掉分层会改变「谁在叫我」的可读性，换底色会跟桌面打架；
    // 换个预设顺手把这两样改掉，正是这套预设最该避免的那种意外
    for (const preset of presets(m)) {
      expect(preset.apply).not.toHaveProperty("sign_tint");
      expect(preset.apply).not.toHaveProperty("sign_emphasis");
    }
  });

  it("三套展开态预设的质感互不相同，否则换了看不出变化", () => {
    const expanded = presets(m).filter((preset) => !preset.apply.collapsed);
    const keys = expanded.map(
      (preset) =>
        `${preset.apply.sign_radius}/${preset.apply.sign_border}/${preset.apply.sign_text}`,
    );
    expect(new Set(keys).size).toBe(expanded.length);
  });
});
