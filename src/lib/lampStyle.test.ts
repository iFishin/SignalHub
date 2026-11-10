import { describe, expect, it } from "vitest";
import {
  lampVars,
  resolveAnimations,
  resolveHaloAnimations,
  resolveShapes,
  stateOverridden,
  toneOf,
  CALM_ALPHA_FACTOR,
  CALM_ALPHA_FLOOR,
  SIGN_OPACITY_MIN,
} from "./lampStyle";
import { DEFAULTS, type Settings } from "./useSettings";
import { SIGNALS } from "./types";

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

/**
 * 分层只回答一个问题：「谁在等我」。
 *
 * 这份表就是 `lamp.css` 里 `.sign[data-tone="need"]` 那一档的定义 —— 改这里
 * 等于改灯上哪几块会变大、变实。所以逐条钉死，别让它悄悄漂。
 */
describe("toneOf", () => {
  it("puts the three actionable states in need", () => {
    for (const state of ["permission", "attention", "error"] as const) {
      expect(toneOf(state)).toBe("need");
    }
  });

  /**
   * `done` 刻意归 calm。
   *
   * 它值得看一眼，但不需要你动手 —— 而 need 的定义是「等你做什么」。把 done
   * 也算进去的话，一次跑完十个任务的上午，整叠牌会全是「要你」的分量，分层就白做了。
   */
  it("keeps done in calm even though it is worth a glance", () => {
    expect(toneOf("done")).toBe("calm");
  });

  it("covers every state with exactly one tone", () => {
    for (const state of SIGNALS) {
      expect(["need", "calm"]).toContain(toneOf(state));
    }
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
    const vars = lampVars(withSettings({ sign_opacity: 72 })) as Record<string, string>;
    expect(vars["--sign-alpha"]).toBe("0.72");
  });

  /**
   * calm 档的牌底不透明度：`α × 0.72`，**带托底**。
   *
   * 托底那半条是本条测试的重点：滑杆到下限 65% 时 `0.65 × 0.72 = 0.468` 会让次要
   * 文字落在桌面上 —— 而「多组 + 低透明度」正是描边时代最难看、也是用户最常遇到
   * 的画面。期望值按 `max(α × 0.72, 0.6)` 写，不写死具体数字串，改托底值时跟着走。
   */
  it("dims the calm tone but never below the readable floor", () => {
    const vars = (opacity: number) =>
      lampVars(withSettings({ sign_opacity: opacity })) as Record<string, string>;
    // 默认档：正常压淡，托底不参与（0.86 × 0.72 = 0.6192）
    expect(vars(86)["--sign-alpha-calm"]).toBe(String(0.86 * CALM_ALPHA_FACTOR));
    // 滑杆到底：托底接管，仍读得清
    expect(vars(SIGN_OPACITY_MIN)["--sign-alpha-calm"]).toBe(String(CALM_ALPHA_FLOOR));
    // 满不透明：还是比 need 档淡，层次在
    expect(Number(vars(100)["--sign-alpha-calm"])).toBeLessThan(1);
  });

  /**
   * 下限必须是**滑杆能停到的最淡值**，而且它得让牌面读得出来。这条常量是前端唯一
   * 真值（Rust 的 `config::MIN_SIGN_OPACITY` 是同名同义的一份），被改回 20、30 之类
   * 的话，描边删掉之后字就真的没法读了 —— 那正是这次改动要根治的病。
   *
   * **65 是拿对照图量出来的**：50% 时在近白窗口上次要文字对比只有约 1.1:1（等于消失），
   * 60% 余量太小，65% 才稳。这条断言的作用是「别有人为了更透，悄悄把它调回去」。
   *
   * 托底必须**低于**下限：图省事让两者相等的话，下限 65 时托底（0.65）会高过默认档的
   * 乘积（0.619），把最常见那档的 calm 分层直接吃掉。
   */
  it("keeps the opacity floor at a level that still holds the text", () => {
    expect(SIGN_OPACITY_MIN).toBe(65);
    expect(CALM_ALPHA_FLOOR).toBe(0.6);
    expect(CALM_ALPHA_FLOOR).toBeLessThan(SIGN_OPACITY_MIN / 100);
    // 默认档的 calm 必须仍明显淡于 need —— 托底不能吃掉日常的分层
    expect(0.86 * CALM_ALPHA_FACTOR).toBeGreaterThan(CALM_ALPHA_FLOOR);
  });

  /**
   * 牌间距倍率推给 CSS。**它是外观里唯一影响窗口几何的一项**：Rust 的
   * `window_size` 用 `SignGap::px` 算同一个值，两边倍率必须一致（见 `SIGN_GAP_MULT`）。
   *
   * 默认必须是 1 —— 写成别的值，升级上来的人灯就变高 / 变矮了。
   */
  it("carries the sign gap multiplier and defaults to identity", () => {
    const vars = (gap: Settings["sign_gap"]) =>
      lampVars(withSettings({ sign_gap: gap })) as Record<string, string>;
    expect(vars("standard")["--sign-gap"]).toBe("1");
    expect(vars("tight")["--sign-gap"]).toBe("0.5");
    expect(vars("loose")["--sign-gap"]).toBe("2");
    // 三档必须互不相同，否则换了看不出变化
    expect(new Set([vars("tight")["--sign-gap"], vars("standard")["--sign-gap"], vars("loose")["--sign-gap"]]).size).toBe(3);
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

describe("resolveHaloAnimations", () => {
  /**
   * 柔光呼吸的时长是灯珠节奏的 **2 倍**。
   *
   * 柔光不该和灯珠同步闪 —— 那是「闪」，不是「呼吸」。慢一倍读起来才是呼吸。
   * 这条测试盯住 2 倍这个关系：改了一边忘了改另一边，这里会红。
   */
  it("breathes at twice the lamp rhythm", () => {
    const halo = resolveHaloAnimations(DEFAULTS);
    expect(halo.thinking).toBe("halo-breathe 1280ms ease-in-out infinite");
    expect(halo.working).toBe("halo-breathe 3200ms ease-in-out infinite");
    expect(halo.attention).toBe("halo-breathe 2000ms ease-in-out infinite");
    expect(halo.permission).toBe("halo-breathe 1800ms ease-in-out infinite");
    expect(halo.error).toBe("halo-breathe 680ms ease-in-out infinite");
  });

  /**
   * 常亮状态 → 柔光也不动。
   *
   * 「常亮」的意思是「没有变化」，柔光再呼吸就变成「一颗一直在跳的灯」——
   * 而 `done`（任务完成）恰恰是**最该静下来**的那一刻。
   */
  it("stays still for the always-on states", () => {
    const halo = resolveHaloAnimations(DEFAULTS);
    expect(halo.done).toBe("none");
    expect(halo.idle).toBe("none");
    expect(halo.stale).toBe("none");
    expect(halo.offline).toBe("none");
    expect(halo.off).toBe("none");
  });

  /**
   * 用户给某个状态单独设了节奏 → 柔光跟着那个节奏（同样 2 倍）。
   *
   * `blink_ms: 0` 是用户明确要「常亮」，柔光也不该动 —— 和默认常亮一视同仁。
   */
  it("follows a per-state blink override", () => {
    const halo = resolveHaloAnimations(
      withSettings({ styles: { thinking: { blink_ms: 300 } } }),
    );
    expect(halo.thinking).toBe("halo-breathe 600ms ease-in-out infinite");

    const off = resolveHaloAnimations(
      withSettings({ styles: { thinking: { blink_ms: 0 } } }),
    );
    expect(off.thinking).toBe("none");
  });

  /**
   * 全局倍率同样作用于柔光 —— 灯珠和柔光得同步加速，否则「呼吸」和「闪烁」
   * 会各走各的，读起来像两个东西在动。
   */
  it("scales with the global blink speed", () => {
    const fast = resolveHaloAnimations(withSettings({ blink_speed: 200 }));
    expect(fast.thinking).toBe("halo-breathe 640ms ease-in-out infinite");

    const slow = resolveHaloAnimations(withSettings({ blink_speed: 50 }));
    expect(slow.thinking).toBe("halo-breathe 2560ms ease-in-out infinite");
  });
});
