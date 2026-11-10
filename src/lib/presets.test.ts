import { describe, expect, it } from "vitest";
import {
  applyStylePreset,
  isActivePreset,
  isActiveStylePreset,
  presets,
  skins,
  isActiveSkin,
  type StylePreset,
} from "./presets";
import { MESSAGES } from "./i18n";
import { DEFAULTS, SLOT_IDS, type Settings } from "./useSettings";

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

  it("字体也参与判断：换成等宽就不再是这一套", () => {
    const standard = presets(m).find((preset) => preset.id === "standard")!;
    expect(isActivePreset(standard, { ...apply(standard), sign_font: "mono" })).toBe(false);
  });

  it("关掉圆点就不再是这一套", () => {
    const standard = presets(m).find((preset) => preset.id === "standard")!;
    expect(
      isActivePreset(standard, {
        ...apply(standard),
        show: { ...standard.apply.show, dot: false },
      }),
    ).toBe(false);
  });

  it("不碰牌底色、分层和间距 —— 材质、语义和几何", () => {
    // 关掉分层会改变「谁在叫我」的可读性，换底色会跟桌面打架，
    // 而间距是唯一影响窗口高度的一项（预设的承诺是「换风格不改窗口大小」）；
    // 换个预设顺手把这三样改掉，正是这套预设最该避免的那种意外
    for (const preset of presets(m)) {
      expect(preset.apply).not.toHaveProperty("sign_tint");
      expect(preset.apply).not.toHaveProperty("sign_emphasis");
      expect(preset.apply).not.toHaveProperty("sign_gap");
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

/**
 * 整套皮肤（0.6.5）。
 *
 * 和上面那四套「外观预设」是**两种东西**：预设只改大小 / 显示段 / 圆角，
 * 皮肤一次改十一项，含六个灯位的画法**和**触发。这里钉住三件容易悄悄坏的：
 * 每套都得是合法值、选中态得逐项比（漏一项就永远不高亮）、以及换肤确实覆盖了灯位。
 */
describe("整套皮肤", () => {
  const m = MESSAGES.zh;
  const all = skins(m);
  const applied = (id: string): Settings => ({
    ...DEFAULTS,
    ...all.find((skin) => skin.id === id)!.apply,
  });

  it("五套都在，且每套都有名字和一句说明", () => {
    expect(all.map((skin) => skin.id)).toEqual([
      "plain",
      "focus",
      "neon",
      "paper",
      "signal",
    ]);
    for (const skin of all) {
      expect(skin.label.length).toBeGreaterThan(0);
      expect(skin.hint.length).toBeGreaterThan(0);
    }
  });

  it("不碰窗口几何：尺寸、间距、透明度、折叠一概不动", () => {
    for (const skin of all) {
      expect(skin.apply).not.toHaveProperty("size");
      expect(skin.apply).not.toHaveProperty("sign_gap");
      expect(skin.apply).not.toHaveProperty("sign_opacity");
      expect(skin.apply).not.toHaveProperty("collapsed");
      expect(skin.apply).not.toHaveProperty("sign_w_mult");
      expect(skin.apply).not.toHaveProperty("sign_h_mult");
    }
  });

  it("六槽写全，且各套的画法 / 触发组合互不相同", () => {
    const combos = new Set<string>();
    for (const skin of all) {
      const lights = skin.apply.sign_lights;
      for (const id of SLOT_IDS) {
        expect(lights[id].kind, `${skin.id}/${id}/kind`).toBeTruthy();
        expect(lights[id].trigger, `${skin.id}/${id}/trigger`).toBeTruthy();
      }
      combos.add(SLOT_IDS.map((id) => `${lights[id].kind}:${lights[id].trigger}`).join("|"));
    }
    expect(combos.size).toBe(all.length);
  });

  it("换肤会覆盖六行触发，选中态因此也跟着这六行走", () => {
    const focus = all.find((skin) => skin.id === "focus")!;
    expect(focus.apply.sign_lights.top.trigger).toBe("attention");
    expect(isActiveSkin(focus, applied("focus"))).toBe(true);
    // 只改一格的触发，就不该再高亮成「正在用这套皮肤」
    const tweaked = applied("focus");
    tweaked.sign_lights = {
      ...tweaked.sign_lights,
      top: { ...tweaked.sign_lights.top, trigger: "always" },
    };
    expect(isActiveSkin(focus, tweaked)).toBe(false);
    // 只改一个标量也一样
    expect(
      isActiveSkin(focus, { ...applied("focus"), effect: "sweep" }),
    ).toBe(false);
  });

  it("素净就是默认外观，选中它等于把灯拉回出厂样子", () => {
    const plain = all.find((skin) => skin.id === "plain")!;
    expect(isActiveSkin(plain, DEFAULTS)).toBe(true);
  });
});
