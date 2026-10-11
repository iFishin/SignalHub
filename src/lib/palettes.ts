import type { Settings, Palette } from "./useSettings";
import { SIGNALS, type SignalState } from "./types";

/**
 * 每套配色的基础色。
 *
 * 给预设而不是只让用户自由取色：红 = 危险、黄 = 注意是有语义的，一上手就
 * 让用户从空白开始配，很容易配出一套自己都认不出的灯。预设是起点，用户可以
 * 在「颜色」页逐个覆盖。
 */
export const PALETTE_COLORS: Record<Palette, Record<SignalState, string>> = {
  // 比常规的 500 号色深一档：扁平色块没有阴影，太亮的颜色在浅色桌面上会糊
  default: {
    idle: "#16a34a",
    thinking: "#16a34a",
    working: "#16a34a",
    done: "#16a34a",
    attention: "#d99a06",
    permission: "#d0342c",
    error: "#d0342c",
    stale: "#9ca3af",
    offline: "#9ca3af",
    off: "#9ca3af",
  },
  // Okabe-Ito 的三个色相。红绿是红绿色盲最难分的一对，所以「正常」用蓝、
  // 「需你注意」用橙/朱红。
  colorblind: {
    idle: "#0072b2",
    thinking: "#0072b2",
    working: "#0072b2",
    done: "#0072b2",
    attention: "#e69f00",
    permission: "#d55e00",
    error: "#d55e00",
    stale: "#8b8f96",
    offline: "#8b8f96",
    off: "#8b8f96",
  },
  // 在花哨或深色的桌面上更容易看清，代价是在白底上略扎眼
  vivid: {
    idle: "#00b34a",
    thinking: "#00b34a",
    working: "#00b34a",
    done: "#00b34a",
    attention: "#ffab00",
    permission: "#ff2d2d",
    error: "#ff2d2d",
    stale: "#9aa1ab",
    offline: "#9aa1ab",
    off: "#9aa1ab",
  },
};

/**
 * 算出每个状态最终用哪个颜色：**用户逐个指定的优先于配色预设**。
 *
 * 所以可以先选一套「色盲友好」，再单独把 error 调成更醒目的红。
 */
export function resolveColors(settings: Settings): Record<SignalState, string> {
  const base = { ...(PALETTE_COLORS[settings.palette] ?? PALETTE_COLORS.default) };
  for (const state of SIGNALS) {
    const custom = settings.colors[state];
    if (custom) base[state] = custom;
  }
  return base;
}

/**
 * 把颜色表转成一组 CSS 变量，挂在窗口根元素上。
 *
 * 挂在根上而不是每个圆点上：一个窗口里可能有多个状态点（灯球多点、设置面板
 * 的会话列表），挂根上一次就都生效了。
 */
export function colorVars(settings: Settings): React.CSSProperties {
  const style: Record<string, string> = {};
  for (const [state, color] of Object.entries(resolveColors(settings))) {
    style[`--c-${state}`] = color;
  }
  return style as React.CSSProperties;
}
