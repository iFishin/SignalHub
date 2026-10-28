import { describe, expect, it } from "vitest";
import {
  lampVars,
  resolveAnimations,
  resolveShapes,
  signTextNeedsContrast,
  stateOverridden,
  isQuietDimmed,
  SIGN_TRANSLUCENT_BELOW,
} from "./lampStyle";
import { DEFAULTS, type Settings } from "./useSettings";

const withSettings = (patch: Partial<Settings>): Settings => ({ ...DEFAULTS, ...patch });

describe("resolveShapes", () => {
  /**
   * 默认（全局是圆）时，`permission` / `error` 画成**圆角方**。
   *
   * 这不是装饰：红绿是红绿色盲最难分的一对，而这两个状态恰恰最需要你注意 ——
   * 单靠颜色分不开就得靠形状。
   */
  it("rounds permission and error when the global shape is a circle", () => {
    const shapes = resolveShapes(withSettings({ shape: "circle" }));
    expect(shapes.permission).toBe("rounded");
    expect(shapes.error).toBe("rounded");
    // 别的状态仍然是圆 —— 特判只针对那两个
    for (const state of ["idle", "working", "thinking", "done", "attention", "stale"] as const) {
      expect(shapes[state]).toBe("circle");
    }
  });

  /**
   * 全局换成别的形状时**不再特判** —— 用户明确选了那个外形，就别替他改。
   *
   * 设成圆环时如果还把 permission 画成圆角方，那就等于「用户的选择被静默推翻了」。
   */
  it("stops special-casing once the global shape is not a circle", () => {
    for (const shape of ["ring", "rounded"] as const) {
      const shapes = resolveShapes(withSettings({ shape }));
      expect(shapes.permission).toBe(shape);
      expect(shapes.error).toBe(shape);
      expect(shapes.idle).toBe(shape);
    }
  });

  /**
   * 逐状态的显式覆盖**优先于全局，也优先于特判**。
   *
   * 用户给 permission 单独设了圆环，那就是他要的（也许他另有区分办法）——
   * 特判该让路。这一条是「特判只作用于跟随全局的那部分」的落点。
   */
  it("lets a per-state override beat both the global shape and the special case", () => {
    const shapes = resolveShapes(
      withSettings({
        shape: "circle",
        styles: { permission: { shape: "ring" }, idle: { shape: "rounded" } },
      }),
    );
    expect(shapes.permission).toBe("ring");
    expect(shapes.idle).toBe("rounded");
    // 没覆盖的仍然走特判
    expect(shapes.error).toBe("rounded");
  });

  /** 每个状态都要有值 —— 漏一个的话那张牌上会没有 `data-shape`，绘制直接不对。 */
  it("covers every state", () => {
    const shapes = resolveShapes(DEFAULTS);
    for (const [state, shape] of Object.entries(shapes)) {
      expect(shape, `${state} 没有形状`).toBeTruthy();
    }
    expect(Object.keys(shapes)).toHaveLength(10);
  });
});

describe("resolveAnimations", () => {
  /**
   * 基础配方（全局倍率 100 = 原速）。
   *
   * 这些时长**抄自 `lamp.css`**（那边的注释写着「时长是唯一真相」），所以这条测试
   * 同时也是「两份抄本还一致吗」的警报。
   */
  it("uses the base recipe at normal speed", () => {
    const anim = resolveAnimations(DEFAULTS);
    expect(anim.thinking).toBe("pulse 640ms ease-in-out infinite");
    expect(anim.working).toBe("pulse 1600ms ease-in-out infinite");
    expect(anim.attention).toBe("pulse 1000ms ease-in-out infinite");
    expect(anim.permission).toBe("pulse 900ms ease-in-out infinite");
    // error 用 steps 硬切做频闪，一眼和 permission 的平滑闪烁分开
    expect(anim.error).toBe("pulse 340ms steps(2, end) infinite");
    // done 是**常亮** —— 它原来那次性感的入场脉冲（transform 的 pop）已经删掉，
    // 「落定」改由通用的跃迁脉冲承担（任何状态变化都会闪）
    expect(anim.done).toBe("none");
  });

  /** idle / stale / offline / off 本来就不闪，靠不透明度区分。 */
  it("leaves the not-working states solid", () => {
    const anim = resolveAnimations(DEFAULTS);
    for (const state of ["idle", "stale", "offline", "off"] as const) {
      expect(anim[state], `${state} 不该闪`).toBe("none");
    }
  });

  /**
   * 全局倍率是**加速**：数字越大周期越短。这条容易写反（直接乘而不是除）。
   */
  it("divides the period by the speed multiplier", () => {
    expect(resolveAnimations(withSettings({ blink_speed: 200 })).working).toBe(
      "pulse 800ms ease-in-out infinite",
    );
    expect(resolveAnimations(withSettings({ blink_speed: 50 })).working).toBe(
      "pulse 3200ms ease-in-out infinite",
    );
  });

  /** `blink_ms: 0` 是**常亮**，不是一个 0ms 的动画。 */
  it("reads an explicit zero as steady-on, not a zero-length animation", () => {
    const anim = resolveAnimations(withSettings({ styles: { error: { blink_ms: 0 } } }));
    expect(anim.error).toBe("none");
  });

  /**
   * 显式节奏给的是**朴素 pulse** —— 用户要的是「某个节奏」。
   *
   * 所以连 `done` 这种默认常亮的状态，只要用户给它设了节奏，也会变成一个真的会闪的灯。
   */
  it("turns any explicit rhythm into a plain pulse", () => {
    const anim = resolveAnimations(withSettings({ styles: { done: { blink_ms: 500 } } }));
    expect(anim.done).toBe("pulse 500ms ease-in-out infinite");
  });

  /**
   * **全局倍率不动已经单独设过节奏的状态。**
   *
   * 用户把 error 调成 300ms 是他明确的意图，不该被一个全局旋钮悄悄改掉。
   * 这条是这几条里最容易写错的：直接把倍率乘到所有状态上，表面上「滑杆生效了」，
   * 实际上抹掉了用户逐个调过的值。
   */
  it("leaves an explicitly paced state alone when the global speed changes", () => {
    const fast = withSettings({
      blink_speed: 200,
      styles: { error: { blink_ms: 300 } },
    });
    const anim = resolveAnimations(fast);
    expect(anim.error).toBe("pulse 300ms ease-in-out infinite");
    // 同时没设过节奏的那几个仍然跟着倍率走
    expect(anim.working).toBe("pulse 800ms ease-in-out infinite");
  });
});

describe("lampVars", () => {
  /**
   * 牌宽 / 牌高的倍率要作为 CSS 变量推给样式表（`.sign` 拿它们乘基础尺寸）。
   *
   * 这一条钉的是「前端确实把设置带到了 CSS」—— 变量名写错的话，CSS 会静默回落到
   * `var(..., 1)`，滑杆就完全不生效，而那种错没有任何东西会报出来。
   */
  it("carries the sign multipliers through as CSS variables", () => {
    const vars = lampVars(withSettings({ sign_w_mult: 1.4, sign_h_mult: 0.9 })) as Record<
      string,
      string
    >;
    expect(vars["--sign-w-mult"]).toBe("1.4");
    expect(vars["--sign-h-mult"]).toBe("0.9");
  });

  /** 默认 1 = 恒等 —— 升级不该让任何人的灯变形。 */
  it("defaults to 1 so existing geometry is unchanged", () => {
    const vars = lampVars(DEFAULTS) as Record<string, string>;
    expect(vars["--sign-w-mult"]).toBe("1");
    expect(vars["--sign-h-mult"]).toBe("1");
  });

  /**
   * 背景透明度也从这里出（它原本是 `SignalLamp` 自己加的内联变量）。
   *
   * 这一条钉的是**设置页那张预览牌和真正的灯用的是同一份变量** —— 两边各写一遍
   * 的话，预览显示的透明度和灯迟早对不上，而「预览和灯不一样」比没有预览更糟。
   */
  it("carries the sign alpha for the settings preview", () => {
    const vars = lampVars(withSettings({ sign_opacity: 40 })) as Record<string, string>;
    expect(vars["--sign-alpha"]).toBe("0.4");
  });
});

describe("stateOverridden", () => {
  /**
   * 「这个状态被单独设过吗」有**两处**在问：逐状态表里那个 ↺ 按钮，和「外观」页
   * 收起时那行摘要（`3 个状态单独设过`）。判据只有这一份，所以不可能对不上 ——
   * 这条测试就是钉住「只有一份」这件事。
   *
   * 四样都算数，包括后加的**停留时长**：漏掉它的后果是所有「只改了停留时长」的
   * 行看起来没设过，而 Rust 那边的 `is_empty()` 会把整条条目删掉 —— 用户设的值
   * 会在下一次保存设置时凭空消失。
   */
  it("counts every field, including the stay length", () => {
    expect(stateOverridden(undefined)).toBe(false);
    expect(stateOverridden({})).toBe(false);
    expect(stateOverridden({ color: "#ff0000" })).toBe(true);
    expect(stateOverridden({ shape: "ring" })).toBe(true);
    // 0 是「常亮」，它是一个**值**而不是「没设」
    expect(stateOverridden({ blink_ms: 0 })).toBe(true);
    expect(stateOverridden({ ttl_ms: 30_000 })).toBe(true);
  });
});

describe("signTextNeedsContrast", () => {
  /**
   * 钉住那条线本身：**50 不换，49 换**。
   *
   * 这条边界是灯（`SignalLamp.tsx` 挂 `data-sign-translucent`）和样式表
   * （`lamp.css` 里那组「浅字 + 描边」变量）唯一的接口 —— 一边改成 `<=`、另一边
   * 还是 `<`，表现就是「滑杆停在 50，设置页说字已经带描边了，灯上却没有」。之所以
   * 判据写在 TS 里而不是 CSS 里：纯 CSS 没法比较数值，而阈值必须跟着设置值走。
   */
  it("switches below the line, not at it", () => {
    expect(signTextNeedsContrast(SIGN_TRANSLUCENT_BELOW - 1)).toBe(true);
    expect(signTextNeedsContrast(SIGN_TRANSLUCENT_BELOW)).toBe(false);
  });

  /** 两端：全透明（0）要换，默认那档（86）和满不透明都不换。 */
  it("covers both ends", () => {
    expect(signTextNeedsContrast(0)).toBe(true);
    expect(signTextNeedsContrast(DEFAULTS.sign_opacity)).toBe(false);
    expect(signTextNeedsContrast(100)).toBe(false);
  });

  /**
   * 安静模式把没事的牌压到 ≈15（`QUIET_FADE_OPACITY`，CSS 安静档 `opacity: 0.15`），
   * 必然低于换字线 50 —— 于是安静里的「没事」照样要换。
   *
   * **为什么**：低透明度是「字该不该带描边」的统一判据，不是滑杆的事。安静压暗、滑杆
   * 压低，底层都是「牌几乎透明、底色透出桌面」；透明度低于一半就换，这是灯的设计线。
   */
  it("switches when quiet dimmed the sign below the line", () => {
    expect(signTextNeedsContrast(DEFAULTS.sign_opacity, true)).toBe(true);
    expect(signTextNeedsContrast(86, true)).toBe(true);
  });

  /** 没事时安静不压（working / thinking / done …），不该换。 */
  it("does not switch when quiet left the sign bright", () => {
    expect(signTextNeedsContrast(DEFAULTS.sign_opacity, false)).toBe(false);
    expect(signTextNeedsContrast(86, false)).toBe(false);
  });
});

describe("isQuietDimmed", () => {
  /** 没事的聚合：安静会压暗它们。 */
  it("dims the not-working aggregates", () => {
    expect(isQuietDimmed(true, "idle")).toBe(true);
    expect(isQuietDimmed(true, "stale")).toBe(true);
    expect(isQuietDimmed(true, "offline")).toBe(true);
    expect(isQuietDimmed(true, "off")).toBe(true);
  });

  /** 有事的聚合：安静不压，灯亮着呢。 */
  it("does not dim working aggregates", () => {
    expect(isQuietDimmed(true, "thinking")).toBe(false);
    expect(isQuietDimmed(true, "working")).toBe(false);
    expect(isQuietDimmed(true, "done")).toBe(false);
    expect(isQuietDimmed(true, "attention")).toBe(false);
  });

  /** 不穿透时安静生效；用户开了穿透，灯就是常亮的，安静那道压暗规则被兔住。 */
  it("ignores quiet while pass-through is on", () => {
    expect(isQuietDimmed(true, "idle", true)).toBe(false);
    expect(isQuietDimmed(false, "idle")).toBe(false);
  });
});
