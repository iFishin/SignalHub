import type { Settings, SignFields } from "./useSettings";

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
export const PRESETS: Preset[] = [
  {
    id: "light",
    label: "单灯",
    hint: "折叠成一颗灯，只有颜色，最不打扰",
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
    label: "紧凑",
    hint: "窄一条，留组名和状态，不要消息",
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
    label: "标准",
    hint: "组名 / 状态 / 已持续 / 消息都显示",
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
    label: "醒目",
    hint: "更大更实，闪得也快一点",
    apply: {
      size: 32,
      collapsed: false,
      show: ALL_FIELDS,
      sign_opacity: 100,
      blink_speed: 140,
    },
  },
];

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
