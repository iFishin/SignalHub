import type { Messages } from "./i18n";
import {
  CORNER_SLOTS,
  EDGE_SLOTS,
  type EdgeLight,
  type LightTrigger,
  type Settings,
  type SignFields,
  type SignLights,
  type StateStyle,
} from "./useSettings";

/**
 * 外观预设。
 *
 * 为什么要有：外观页上那几个旋钮（大小、透明度、显示哪几段、闪烁节奏、圆角、
 * 描边、字号）是互相牵制的 —— 牌面调透了就得把字调少，字多了就得把牌调大。
 * 让用户一个个试，很难试出一套协调的组合，而「我就想要个安静的小灯」本来也
 * 不是七个数字的问题。
 *
 * 刻意**不含形状和配色**：那两项是语义性的（形状承载色盲区分，配色承载
 * 「红 = 危险」），和「牌面多大、多透、显示什么」不是一类东西。混进同一个预设里
 * 会让人以为换个风格就会顺手把配色改掉。
 *
 * **同理不含牌面底色（`sign_tint`）和分层（`sign_emphasis`）**：底色要和桌面
 * 协调，换预设时突变会很突兀；分层是「谁在叫我」的语义机制，不是装饰。圆角、
 * 描边、字号、字体可以被预设接管，正因为它们只是形态 —— 不承载任何含义。
 *
 * **牌间距（`sign_gap`）也不在预设里**：它是唯一影响窗口高度的一项，而预设
 * 的承诺是「换风格不改窗口大小」。用户手动调过间距，也不该被一次换风格重置。
 */
export interface Preset {
  id: string;
  label: string;
  /** 一行说明它是什么样，鼠标悬停显示 */
  hint: string;
  apply: Pick<
    Settings,
    | "size"
    | "collapsed"
    | "show"
    | "sign_opacity"
    | "blink_speed"
    | "sign_radius"
    | "sign_border"
    | "sign_text"
    | "sign_font"
  >;
}

const ALL_FIELDS: SignFields = {
  name: true,
  state: true,
  age: true,
  message: true,
  dot: true,
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
        // 折叠态一块牌都没有，质感三项只在切回展开时看得出来 —— 仍然要写全，
        // 否则「从紧凑切到轻量再展开」会留着上一套的圆角和字号。
        sign_radius: "standard",
        sign_border: "hairline",
        sign_text: "standard",
        sign_font: "system",
      },
    },
    {
      id: "compact",
      label: m.presetCompact,
      hint: m.presetCompactHint,
      apply: {
        size: 16,
        collapsed: false,
        show: { name: true, state: true, age: true, message: false, dot: true },
        sign_opacity: 80,
        blink_speed: 100,
        // 小牌配小圆角：0.32×16 只有 5px，但 0.18 更利落。字号收到紧凑档，
        // 16px 的牌上能多塞两个字 —— 这一档整个诉求就是密度。
        sign_radius: "soft",
        sign_border: "hairline",
        sign_text: "compact",
        sign_font: "system",
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
        sign_radius: "standard",
        sign_border: "hairline",
        sign_text: "standard",
        sign_font: "system",
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
        // 大牌 + 强描边：这一档是「隔着一米也要看清」，边缘得立得住。
        // 圆角只用 soft —— 32px 的牌配 round（0.72×）会圆成一坨，反而不利落。
        sign_radius: "soft",
        sign_border: "strong",
        sign_text: "large",
        sign_font: "system",
      },
    },
  ];
}

/**
 * 当前外观是不是正好等于这套预设（用来高亮选中的那一个）。
 *
 * 逐字段比而不是比对象引用：设置一路经过 JSON 序列化往返，引用早就变了。
 * 五个显示字段也要逐个比 —— 只比 `show` 的引用会让「只关掉消息」被误判成
 * 还停留在预设上。
 */
export function isActivePreset(preset: Preset, settings: Settings): boolean {
  const {
    size,
    collapsed,
    show,
    sign_opacity,
    blink_speed,
    sign_radius,
    sign_border,
    sign_text,
    sign_font,
  } = preset.apply;
  return (
    settings.size === size &&
    settings.collapsed === collapsed &&
    settings.sign_opacity === sign_opacity &&
    settings.blink_speed === blink_speed &&
    settings.sign_radius === sign_radius &&
    settings.sign_border === sign_border &&
    settings.sign_text === sign_text &&
    settings.sign_font === sign_font &&
    (["name", "state", "age", "message", "dot"] as const).every(
      (field) => settings.show[field] === show[field],
    )
  );
}

/**
 * 逐个状态的灯效预设：形状 / 节奏两样一起给。
 *
 * 与上面那几套的区别是它们回答的是**两个不同的问题**：那几套管「整块牌有多大、
 * 多透、显示哪几段」，这几套管「红灯和绿灯怎么区分开」。混在一起会变成一套
 * 谁也说不清的组合。
 *
 * **预设只管形状和节奏。** `styles` 这张表里还住着每个状态的颜色和静默超时
 * （见 `useSettings.ts` 的 `StateStyle`）—— 它们不是「灯效预设」该决定的东西，
 * 应用预设时必须原样留着（见 `applyStylePreset`）。早先的实现是整份替换，
 * 点一下预设就把用户调好的颜色和 TTL 静默删了（issue #8）。
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

/**
 * 把预设合并进现有的 `styles` 表。
 *
 * 预设拥有**形状和节奏**；颜色与静默超时是用户自己的，原样保留。整份替换会把
 * 它们一起删掉 —— 而界面上没有任何提示，用户只会觉得「我明明调过」。
 */
export function applyStylePreset(
  current: Record<string, StateStyle>,
  preset: Record<string, StateStyle>,
): Record<string, StateStyle> {
  const states = new Set([...Object.keys(current), ...Object.keys(preset)]);
  const merged: Record<string, StateStyle> = {};
  for (const state of states) {
    const kept = current[state] ?? {};
    const own = preset[state] ?? {};
    const style: StateStyle = {
      color: kept.color ?? null,
      ttl_ms: kept.ttl_ms ?? null,
      shape: own.shape ?? null,
      blink_ms: own.blink_ms ?? null,
    };
    // 一样都没写的条目没有作用（Rust 的 `clamped()` 也会丢掉），先在这里丢：
    // 否则「这一格改过没有」的判断会和保存之后不一致
    if (
      style.color == null &&
      style.ttl_ms == null &&
      style.shape == null &&
      style.blink_ms == null
    ) {
      continue;
    }
    merged[state] = style;
  }
  return merged;
}

/** 预设管的那两项（形状 / 节奏）。颜色和 TTL 不参与「是哪一套」的判断。 */
function hasShapeOrRhythm(style: StateStyle | undefined): boolean {
  return !!style && (style.shape != null || style.blink_ms != null);
}

/** 逐字段比，不比对象引用也不比 JSON 字符串 —— 键顺序在 Rust 那边往返一趟会变。 */
function sameShapeOrRhythm(a: StateStyle, b: StateStyle): boolean {
  return (
    (a.shape ?? null) === (b.shape ?? null) &&
    (a.blink_ms ?? null) === (b.blink_ms ?? null)
  );
}

export function isActiveStylePreset(
  preset: StylePreset,
  settings: Settings,
): boolean {
  const want = Object.keys(preset.styles).filter((key) =>
    hasShapeOrRhythm(preset.styles[key]),
  );
  const have = Object.keys(settings.styles).filter((key) =>
    hasShapeOrRhythm(settings.styles[key]),
  );
  if (want.length !== have.length) return false;

  return want.every(
    (key) =>
      have.includes(key) &&
      sameShapeOrRhythm(preset.styles[key], settings.styles[key]),
  );
}

// ---- 整套主题皮肤（0.6.5） -------------------------------------------------

/**
 * 皮肤和上面那四套「外观预设」回答的是**两个不同的问题**：
 *
 * - `presets()` 管「牌多大、多透、显示哪几段」，刻意不碰配色和形状（那两样
 *   承载语义）；换一套不会改窗口大小。
 * - `skins()` 管「这一整套看起来是什么风格」：配色、牌底、圆角 / 描边、灯珠
 *   形状、牌轮廓、动效、边框光六个灯位，一键全换。
 *
 * 分开是因为「我就想要个安静的小灯」和「我想换一身皮」不是同一件事；混成一套
 * 的话，换个配色会顺手把牌宽、字号、显示段一起冲掉，而那些是用户一个个调过的。
 *
 * **皮肤会整份覆盖 `sign_lights`**（包括六行触发条件）—— 这是「整套」的代价，
 * 也是它和四套尺寸预设最不一样的地方。界面上要把这句话写出来，否则用户精心
 * 调的六行触发被一次换肤冲掉会完全找不到原因。
 */
export interface Skin {
  id: string;
  label: string;
  hint: string;
  apply: Pick<
    Settings,
    | "palette"
    | "sign_tint"
    | "sign_radius"
    | "sign_border"
    | "shape"
    | "sign_outline"
    | "effect"
    | "sign_light_scale"
    | "sign_light_alpha"
    | "sign_light_color"
    | "sign_lights"
  >;
}

/** 六个灯位全关的骨架。皮肤只写自己要改的那几个槽，其余保持默认。 */
function lights(patch: Partial<SignLights>): SignLights {
  return {
    top: { kind: "off", trigger: "always", blink: false },
    bottom: { kind: "off", trigger: "always", blink: false },
    tl: { kind: "empty", trigger: "always", blink: false },
    tr: { kind: "empty", trigger: "always", blink: false },
    br: { kind: "empty", trigger: "always", blink: false },
    bl: { kind: "empty", trigger: "always", blink: false },
    ...patch,
  };
}

const edge = (
  kind: EdgeLight,
  trigger: LightTrigger = "always",
  blink = false,
) => ({ kind, trigger, blink });

export function skins(m: Messages): Skin[] {
  return [
    {
      id: "plain",
      label: m.skinPlain,
      hint: m.skinPlainHint,
      apply: {
        palette: "default",
        sign_tint: "graphite",
        sign_radius: "standard",
        sign_border: "hairline",
        shape: "circle",
        sign_outline: "rect",
        effect: "breathe",
        sign_light_scale: "normal",
        sign_light_alpha: "normal",
        sign_light_color: "state",
        sign_lights: lights({}),
      },
    },
    {
      id: "focus",
      label: m.skinFocus,
      hint: m.skinFocusHint,
      apply: {
        palette: "default",
        sign_tint: "slate",
        sign_radius: "soft",
        sign_border: "hairline",
        shape: "circle",
        sign_outline: "rect",
        effect: "off",
        sign_light_scale: "normal",
        sign_light_alpha: "soft",
        sign_light_color: "neutral",
        sign_lights: lights({
          top: edge("line", "attention"),
          tl: { kind: "pinned", trigger: "always", blink: false },
          tr: { kind: "muted", trigger: "always", blink: false },
        }),
      },
    },
    {
      id: "neon",
      label: m.skinNeon,
      hint: m.skinNeonHint,
      apply: {
        palette: "vivid",
        sign_tint: "black",
        sign_radius: "round",
        sign_border: "none",
        shape: "ring",
        sign_outline: "capsule",
        effect: "pulse",
        sign_light_scale: "thick",
        sign_light_alpha: "strong",
        sign_light_color: "state",
        sign_lights: lights({
          top: edge("glow", "active"),
          bottom: edge("glow", "active"),
          tl: { kind: "state", trigger: "always", blink: false },
          tr: { kind: "state", trigger: "always", blink: false },
          br: { kind: "state", trigger: "always", blink: false },
          bl: { kind: "state", trigger: "always", blink: false },
        }),
      },
    },
    {
      id: "paper",
      label: m.skinPaper,
      hint: m.skinPaperHint,
      apply: {
        palette: "default",
        sign_tint: "warm",
        sign_radius: "round",
        sign_border: "strong",
        shape: "square",
        sign_outline: "arch",
        effect: "fade",
        sign_light_scale: "thin",
        sign_light_alpha: "normal",
        sign_light_color: "neutral",
        sign_lights: lights({ top: edge("line") }),
      },
    },
    {
      id: "signal",
      label: m.skinSignal,
      hint: m.skinSignalHint,
      apply: {
        palette: "colorblind",
        sign_tint: "graphite",
        sign_radius: "square",
        sign_border: "strong",
        shape: "diamond",
        sign_outline: "leaf",
        effect: "sweep",
        sign_light_scale: "thick",
        sign_light_alpha: "strong",
        sign_light_color: "state",
        sign_lights: lights({
          top: edge("fade", "message"),
          bottom: edge("line", "attention"),
          tl: { kind: "passthrough", trigger: "always", blink: false },
          br: { kind: "stale", trigger: "always", blink: false },
        }),
      },
    },
  ];
}

/** 六槽逐字段比。键顺序在 Rust 那边往返一趟可能变，所以不比 JSON 字符串。 */
function sameSignLights(a: SignLights, b: SignLights): boolean {
  return (
    EDGE_SLOTS.every(
      (pos) => a[pos].kind === b[pos].kind && a[pos].trigger === b[pos].trigger,
    ) &&
    CORNER_SLOTS.every(
      (pos) => a[pos].kind === b[pos].kind && a[pos].trigger === b[pos].trigger,
    )
  );
}

const SKIN_SCALAR_FIELDS = [
  "palette",
  "sign_tint",
  "sign_radius",
  "sign_border",
  "shape",
  "sign_outline",
  "effect",
  "sign_light_scale",
  "sign_light_alpha",
  "sign_light_color",
] as const;

/** 当前外观是不是正好等于这套皮肤（用来高亮选中的那一个）。 */
export function isActiveSkin(skin: Skin, settings: Settings): boolean {
  return (
    SKIN_SCALAR_FIELDS.every((key) => settings[key] === skin.apply[key]) &&
    sameSignLights(settings.sign_lights, skin.apply.sign_lights)
  );
}
