import type { Settings, Shape } from "./useSettings";
import { SIGNALS, type SignalState } from "./types";
import { colorVars } from "./palettes";

/**
 * 逐个状态的形状与节奏解析。
 *
 * 颜色仍然归 palettes.ts 管；这里只管另外两样。拆开是因为颜色的解析规则是
 * 「预设 + 覆盖」，而形状 / 节奏的规则是「全局 + 覆盖 + 若干特判」，两套逻辑
 * 混在一起会互相干扰。全部外观变量最后由 lampVars 汇总。
 */

/**
 * 每个状态最终用哪个形状。
 *
 * 规则：`styles[state].shape` 优先；没写就用全局 `settings.shape`。
 *
 * **特判**：全局是「圆」时 permission / error 仍画成圆角方。红绿是红绿色盲
 * 最难分的一对，而这两个状态恰恰是最需要你注意的 —— 单靠颜色分不开就得靠
 * 形状。这条规则原来写在 CSS 里（`[data-shape="circle"] .dot[data-state=...]`），
 * 现在形状是**逐元素**的（一个窗口里多块牌可以是不同形状），只能挪到 TS 里算。
 *
 * 注意特判只作用于**跟随全局**的那部分：用户显式给某个状态指定了形状，
 * 就照用户说的来（他要给 permission 画圆环，那是有理由的）。
 */
export function resolveShapes(settings: Settings): Record<SignalState, Shape> {
  const out = {} as Record<SignalState, Shape>;
  for (const state of SIGNALS) {
    const own = settings.styles[state]?.shape;
    if (own) {
      out[state] = own;
    } else if (
      settings.shape === "circle" &&
      (state === "permission" || state === "error")
    ) {
      out[state] = "rounded";
    } else {
      out[state] = settings.shape;
    }
  }
  return out;
}

/**
 * 动画基础配方，逐条抄自 lamp.css（**那边的时长是唯一真相**）。
 *
 * 抄成表而不是在 CSS 里留着再覆盖：全局倍率要在 JS 里做除法，两边都留一份
 * 迟早对不上。CSS 里只保留同值的兜底，让样式表单独也能用。
 *
 * `null` = 这个状态本来就不闪（靠不透明度区分，见 CSS）。
 */
const BASE_ANIMATION: Record<
  SignalState,
  { name: string; timing: string; iterations: string; ms: number } | null
> = {
  thinking: { name: "pulse", timing: "ease-in-out", iterations: "infinite", ms: 640 },
  working: { name: "pulse", timing: "ease-in-out", iterations: "infinite", ms: 1600 },
  attention: { name: "pulse", timing: "ease-in-out", iterations: "infinite", ms: 1000 },
  permission: { name: "pulse", timing: "ease-in-out", iterations: "infinite", ms: 900 },
  error: { name: "pulse", timing: "steps(2, end)", iterations: "infinite", ms: 340 },
  // 完成：常亮 + 一次入场脉冲。这一下**不跟着节奏走** —— 它是一次性的「收到」。
  done: { name: "pop", timing: "ease-out", iterations: "1", ms: 380 },
  idle: null,
  stale: null,
  offline: null,
  off: null,
};

/**
 * 每个状态最终的 `animation` 简写，或 `"none"`（常亮）。
 *
 * 优先级：
 *   1. 用户显式设了 `blink_ms`：0 = 常亮；> 0 = 一个朴素的 pulse。
 *      用户要的是**某个节奏**，就给最朴素的那种 —— 于是 done 那种一次性
 *      `pop`，只要用户给它设了节奏，也会变成一个真的会闪的灯。
 *   2. 没设：用基础配方，时长除以全局倍率。
 *
 * 全局倍率**只作用于用户没单独设节奏的状态**，这样「我把 error 调成 300ms」
 * 在全局滑杆被拖走之后仍然就是 300ms —— 用户给某个状态定的节奏是他明确的
 * 意图，不该被一个全局旋钮悄悄改掉。
 *
 * 倍率由 Rust 夹在 0.5–2.0，除零不可能发生。时长四舍五入到整毫秒。
 */
export function resolveAnimations(settings: Settings): Record<SignalState, string> {
  const speed = settings.blink_speed / 100;
  const out = {} as Record<SignalState, string>;
  for (const state of SIGNALS) {
    const blink = settings.styles[state]?.blink_ms;
    if (blink === 0) {
      out[state] = "none";
      continue;
    }
    if (typeof blink === "number" && blink > 0) {
      out[state] = `pulse ${blink}ms ease-in-out infinite`;
      continue;
    }
    const recipe = BASE_ANIMATION[state];
    out[state] = recipe
      ? `${recipe.name} ${Math.round(recipe.ms / speed)}ms ${recipe.timing} ${recipe.iterations}`
      : "none";
  }
  return out;
}

/**
 * 全部外观变量（颜色 + 动画），挂到窗口根元素上。
 *
 * 挂根上一次就都生效了：一个窗口里可能有多个状态点（灯球多点、设置面板的
 * 会话列表）。颜色变量由 palettes 产出，这里只补上动画变量。
 */
export function lampVars(settings: Settings): React.CSSProperties {
  const style: Record<string, string> = {
    ...(colorVars(settings) as Record<string, string>),
  };
  for (const [state, anim] of Object.entries(resolveAnimations(settings))) {
    style[`--anim-${state}`] = anim;
  }
  return style as React.CSSProperties;
}
