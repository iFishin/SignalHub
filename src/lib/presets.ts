import type { Messages } from "./i18n";
import type { Settings, SignFields, StateStyle } from "./useSettings";

/**
 * 外观预设。
 *
 * 为什么要有：外观页上那几个旋钮（大小、透明度、显示哪几段、闪烁节奏）是互相
 * 牵制的 —— 牌面调透了就得把字调少，字多了就得把牌调大。让用户一个个试，很难
 * 试出一套协调的组合，而「我就想要个安静的小灯」本来也不是四个数字的问题。
 *
 * 刻意**不含形状和配色**：那两项是语义性的（形状承载色盲区分，配色承载
 * 「红 = 危险」），和「牌面多大、多透、显示什么」不是一类东西。混进同一个预设里
 * 会让人以为换个风格就会顺手把配色改掉。
 */
export interface Preset {
  id: string;
  label: string;
  /** 一行说明它是什么样，鼠标悬停显示 */
  hint: string;
  apply: Pick<
    Settings,
    "size" | "collapsed" | "show" | "sign_opacity" | "blink_speed"
  >;
}

const ALL_FIELDS: SignFields = {
  name: true,
  state: true,
  age: true,
  message: true,
};

/** 与 Rust 侧 config.rs 的 MIN_SIZE / MAX_SIZE 对齐 —— 32 是允许的最大值。 */
export function presets(m: Messages): Preset[] {
  return [
    {
      id: "light",
      label: m.presetLight,
      hint: m.presetLightHint,
      apply: {
        size: 22,
        collapsed: true,
        show: ALL_FIELDS,
        sign_opacity: 86,
        blink_speed: 100,
      },
    },
    {
      id: "compact",
      label: m.presetCompact,
      hint: m.presetCompactHint,
      apply: {
        size: 16,
        collapsed: false,
        show: { name: true, state: true, age: true, message: false },
        sign_opacity: 80,
        blink_speed: 100,
      },
    },
    {
      id: "standard",
      label: m.presetStandard,
      hint: m.presetStandardHint,
      apply: {
        size: 22,
        collapsed: false,
        show: ALL_FIELDS,
        sign_opacity: 86,
        blink_speed: 100,
      },
    },
    {
      id: "bold",
      label: m.presetBold,
      hint: m.presetBoldHint,
      apply: {
        size: 32,
        collapsed: false,
        show: ALL_FIELDS,
        sign_opacity: 100,
        blink_speed: 140,
      },
    },
  ];
}

/**
 * 当前外观是不是正好等于这套预设（用来高亮选中的那一个）。
 *
 * 逐字段比而不是比对象引用：设置一路经过 JSON 序列化往返，引用早就变了。
 * 四个显示字段也要逐个比 —— 只比 `show` 的引用会让「只关掉消息」被误判成
 * 还停留在预设上。
 */
export function isActivePreset(preset: Preset, settings: Settings): boolean {
  const { size, collapsed, show, sign_opacity, blink_speed } = preset.apply;
  return (
    settings.size === size &&
    settings.collapsed === collapsed &&
    settings.sign_opacity === sign_opacity &&
    settings.blink_speed === blink_speed &&
    (["name", "state", "age", "message"] as const).every(
      (field) => settings.show[field] === show[field],
    )
  );
}

/**
 * 逐个状态的灯效预设：颜色 / 形状 / 节奏三样一起给。
 *
 * 与上面那几套的区别是它们回答的是**两个不同的问题**：那几套管「整块牌有多大、
 * 多透、显示哪几段」，这几套管「红灯和绿灯怎么区分开」。混在一起会变成一套
 * 谁也说不清的组合。
 *
 * 每套给的是**完整的一份 `styles` 表**，应用就是整份替换 —— 所以「当前是哪一套」
 * 可以靠整体相等判断出来（见 `isActiveStylePreset`）。用户改掉任意一格之后
 * 高亮就会消失，那是对的：它已经不是那一套了。
 */
export interface StylePreset {
  id: string;
  label: string;
  hint: string;
  styles: Record<string, StateStyle>;
}

export function stylePresets(m: Messages): StylePreset[] {
  return [
    {
      id: "follow",
      label: m.stylePresetFollow,
      hint: m.stylePresetFollowHint,
      styles: {},
    },
    {
      id: "shape",
      label: m.stylePresetShape,
      hint: m.stylePresetShapeHint,
      styles: {
        permission: { shape: "rounded" },
        error: { shape: "rounded" },
      },
    },
    {
      id: "rhythm",
      label: m.stylePresetRhythm,
      hint: m.stylePresetRhythmHint,
      styles: {
        thinking: { blink_ms: 400 },
        working: { blink_ms: 1600 },
        attention: { blink_ms: 1000 },
        permission: { blink_ms: 900 },
        error: { blink_ms: 300 },
        idle: { blink_ms: 0 },
        stale: { blink_ms: 0 },
        offline: { blink_ms: 0 },
        off: { blink_ms: 0 },
      },
    },
    {
      id: "calm",
      label: m.stylePresetCalm,
      hint: m.stylePresetCalmHint,
      styles: {
        idle: { blink_ms: 0 },
        thinking: { blink_ms: 0 },
        working: { blink_ms: 0 },
        stale: { blink_ms: 0 },
        offline: { blink_ms: 0 },
        off: { blink_ms: 0 },
      },
    },
  ];
}

/** 这一格有没有被覆盖过（空对象和全是 null 都算没有）。 */
function hasAny(style: StateStyle | undefined): boolean {
  return (
    !!style &&
    (style.color != null || style.shape != null || style.blink_ms != null)
  );
}

/** 逐字段比，不比对象引用也不比 JSON 字符串 —— 键顺序在 Rust 那边往返一趟会变。 */
function sameStyle(a: StateStyle, b: StateStyle): boolean {
  return (
    (a.color ?? null) === (b.color ?? null) &&
    (a.shape ?? null) === (b.shape ?? null) &&
    (a.blink_ms ?? null) === (b.blink_ms ?? null)
  );
}

export function isActiveStylePreset(
  preset: StylePreset,
  settings: Settings,
): boolean {
  const want = Object.keys(preset.styles).filter((key) =>
    hasAny(preset.styles[key]),
  );
  const have = Object.keys(settings.styles).filter((key) =>
    hasAny(settings.styles[key]),
  );
  if (want.length !== have.length) return false;

  return want.every(
    (key) =>
      have.includes(key) &&
      sameStyle(preset.styles[key], settings.styles[key]),
  );
}
