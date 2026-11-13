import { describe, expect, it } from "vitest";
import { whyLines } from "./lampWhy";
import { MESSAGES } from "./i18n";
import { DEFAULTS, type SignLights, type Settings } from "./useSettings";
import type { GroupView } from "./types";
import type { LightContext } from "./lampStyle";

const m = MESSAGES.zh;
const now = 1_700_000_000_000;

const withSettings = (patch: Partial<Settings>): Settings => ({ ...DEFAULTS, ...patch });

const lights = (patch: Partial<SignLights>): SignLights => ({
  ...DEFAULTS.sign_lights,
  ...patch,
});

const group = (patch: Partial<GroupView> = {}): GroupView => ({
  name: "billing",
  default_label: null,
  aggregate: "permission",
  message: "批准 npm publish",
  since_ms: now - 45_000,
  stale_progress: null,
  snooze_until_ms: 0,
  sessions: [],
  ...patch,
});

const ctx = (patch: Partial<LightContext> = {}): LightContext => ({
  state: "permission",
  quiet: false,
  ...patch,
});

/** 只关心「几行、每行是什么」，测试里不必重复构造输入。 */
const lines = (patch: {
  group?: Partial<GroupView>;
  settings?: Partial<Settings>;
  ctx?: Partial<LightContext>;
  passThrough?: boolean;
}) =>
  whyLines({
    group: group(patch.group),
    settings: withSettings(patch.settings ?? {}),
    ctx: ctx(patch.ctx),
    lang: "zh",
    m,
    now,
    passThrough: patch.passThrough,
  });

describe("右键菜单的「为什么它在亮」", () => {
  /** 第一行回答三件事：什么状态、在等什么、等了多久 —— 全是牌上已有的事实。 */
  it("第一行摊开状态 / 消息 / 已持续", () => {
    expect(lines({})[0]).toBe("需要授权 · 批准 npm publish · 45s");
  });

  it("没有消息就只留两段，不留空的分隔符", () => {
    expect(lines({ group: { message: null } })[0]).toBe("需要授权 · 45s");
  });

  /**
   * `since_ms` 为 0 = 这一组没有决定聚合的会话（离线 / 占位）。
   * 那时候写一个 55 年前的时长比不写更糟。
   */
  it("没有起点就不写时长", () => {
    expect(lines({ group: { aggregate: "offline", message: null, since_ms: 0 } })[0]).toBe(
      "没有智能体在上报",
    );
  });

  it("太长的消息截断，不让它把菜单撑到半屏", () => {
    const long = "很".repeat(60);
    const line = lines({ group: { message: long } })[0];
    expect(line).toContain(`${"很".repeat(40)}…`);
    expect(line).not.toContain("很".repeat(41));
  });

  /** 六个灯位默认全关 —— 那时候不该冒出一行空的「亮着的灯」。 */
  it("一个灯位都没开时只有一行", () => {
    expect(lines({})).toHaveLength(1);
  });

  it("上边亮着就说上边凭什么亮", () => {
    const out = lines({
      settings: { sign_lights: lights({ top: { kind: "line", trigger: "always", blink: false } }) },
    });
    expect(out).toHaveLength(2);
    expect(out[1]).toBe("上边「常亮」");
  });

  /**
   * 带参数的两档必须把参数说出来：写「达到指定状态」等于没说，
   * 而用户配的是哪个状态，除了这一行没有第二处能查到。
   */
  it("「指定状态」和「指定时段」把参数一起说出来", () => {
    const stateOut = lines({
      settings: {
        sign_trigger_state: "error",
        sign_lights: lights({ top: { kind: "line", trigger: "state", blink: false } }),
      },
    });
    expect(stateOut[1]).toBe("上边「出错 / 阻塞」");

    const scheduleOut = lines({
      // 时段判定要注入「现在几点」：不注入就会读真实时钟，测试在 18:00 之后跑
      // 必然失败（这正是 `LightContext.minutes` 存在的理由）。
      ctx: { minutes: 10 * 60 },
      settings: {
        sign_trigger_from: 9 * 60,
        sign_trigger_to: 18 * 60,
        sign_lights: lights({ bottom: { kind: "line", trigger: "schedule", blink: false } }),
      },
    });
    expect(scheduleOut[1]).toBe("下边「09:00–18:00」");
  });

  /** 角灯解释的是「这枚记号在说什么」，名字取的是 kind 而不是触发。 */
  it("角灯按 kind 说它是什么意思，且只列此刻真的画出来的那些", () => {
    const settings = withSettings({
      group_meta: { billing: { order: 1 } },
      sign_lights: lights({
        tl: { kind: "stale", trigger: "always", blink: false },
        tr: { kind: "pinned", trigger: "always", blink: false },
        br: { kind: "state", trigger: "always", blink: false },
      }),
    });
    const out = whyLines({
      group: group(),
      settings,
      ctx: ctx(),
      lang: "zh",
      m,
      now,
    });
    // stale 那枚没有 stale_progress，条件不成立 → 不出现；pinned / state 出现。
    expect(out[1]).toBe("第 2 格「置顶」 · 第 3 格「状态色」");
  });

  /** 六格全开时最多列三处，多出来的用牌上同一个 `+N` 记号收尾。 */
  it("超过三处用 +N 收尾", () => {
    const out = lines({
      settings: {
        sign_lights: lights({
          top: { kind: "line", trigger: "always", blink: false },
          bottom: { kind: "line", trigger: "always", blink: false },
          tl: { kind: "state", trigger: "always", blink: false },
          tr: { kind: "state", trigger: "always", blink: false },
          br: { kind: "state", trigger: "always", blink: false },
        }),
      },
    });
    expect(out[1]).toBe("上边「常亮」 · 下边「常亮」 · 第 1 格「状态色」 +2");
  });

  /** 触发说不亮时，那一格连解释都不该出现（和「不亮就不画」是同一条规矩）。 */
  it("触发条件不成立时不解释", () => {
    const out = lines({
      group: { aggregate: "working" },
      ctx: { state: "working" },
      settings: { sign_lights: lights({ top: { kind: "line", trigger: "attention", blink: false } }) },
    });
    expect(out).toHaveLength(1);
  });
});
