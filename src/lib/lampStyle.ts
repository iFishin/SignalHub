import type { Settings, Shape, StateStyle } from "./useSettings";
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
  // 完成：常亮。原来那一下入场脉冲（`pop`）已经删掉 —— 它动的是 transform，
  // 是这个常驻窗口唯一的 CPU 违例（红线 2）。「落定」的观感现在由**跃迁脉冲**
  // 承担（见 SignalLamp 的 `data-pulse` / lamp.css 的 `pulse-in`），而它更通用：
  // 任何状态变化都会闪，不只 done。
  done: null,
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
 *      用户要的是**某个节奏**，就给最朴素的那种 —— 于是 `done` 这种默认常亮
 *      的状态，只要用户给它设了节奏，也会变成一个真的会闪的灯。
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
 * 牌面淡到这条线以下，牌里的字改成「浅字 + 深描边」（`lamp.css` 的
 * `[data-sign-translucent]`）。
 *
 * **为什么需要它**：牌底是 `rgba(28,28,32, α)`，牌里的字是近白的 `#ececef`（组名）
 * / `#9b9ba3`（状态、消息、时长）。α 高的时候那是一块「深色牌 + 浅字」，对比来自
 * 牌面本身；α 低到一半以下，透出来的桌面比牌面还显眼，字其实是画在**桌面**上的
 * —— 浅色桌面或浅色窗口上，近白的字就没了。
 *
 * **为什么不是按桌面实时反色**：那要读灯后面的像素（macOS 的 CoreGraphics /
 * ScreenCaptureKit），必须先拿到「屏幕录制」权限。这个应用为了一个常驻小灯去要
 * 那种权限不划算，而且它是 ad-hoc 签名、每次更新 cdhash 都会变，授权多半还得重给。
 * 所以改用不依赖背景信息的排法：字本身保持浅色，四周补一圈深色描边。深色桌面上
 * 靠浅字读，浅色桌面上靠描边勾出字形 —— 一套字两边都能读。
 *
 * **为什么是 50**：α = 0.5 是「牌面还托得住浅字」和「字已经在桌面上了」的分界。
 * 往上描边只会让字变脏，往下没有描边就只剩桌面的运气。
 *
 * **它治不了什么**：极浅背景上浅色字心会和背景融在一起，只剩描边的字形（像空心
 * 字）；字号很小时那圈边也会占掉笔画的宽度。这是在不读桌面的前提下能做到的上限。
 */
export const SIGN_TRANSLUCENT_BELOW = 50;

/**
 * 安静模式把没事的牌压到几乎看不见（`lamp.css` 的 `[data-quiet][data-aggregate=…]`
 * 那档 `opacity: 0.15`）。它和「用户把透明度滑到一半以下」走同一条反转逻辑，只是
 * 触发条件不同：安静时牌面被压暗，**透明度等价值≈15**（CSS `opacity: 0.15` 即 alpha
 * 0.15，和滑杆百分比口径同源），必然低于 [`SIGN_TRANSLUCENT_BELOW`]。
 *
 * **为什么要翻**：低透明度下牌底（`rgba(28, 28, 32, alpha)`）透出来的东西是桌面本身；
 * 近白的字落在浅色桌面上就不可读。安静的 0.15 同理 —— 亮白字在「几乎透明」的
 * 牌上就是没人能读的。和滑杆压低走同一条路：浅字 + 深描边。
 *
 * **边界**：安静 + 没事时（idle / stale / offline / off）才压暗；有事（thinking /
 * working / done …）时安静不压，不该翻。那层判断在组件里（`SignalLamp` 按
 * `[data-aggregate]` 决定压不压），这里只认「这次牌面是不是处于低透明度」。
 */
export const QUIET_FADE_OPACITY = 15;

/**
 * 安静模式下当前聚合是否被压暗（`lamp.css` 的安静规则一字不差：idle / stale /
 * offline / off；且用户没开「穿透」——穿透时灯是常亮的，安静那道压暗规则被
 * `[data-quiet][data-pass-through]` 兜住）。
 */
export function isQuietDimmed(
  quiet: boolean,
  aggregate: SignalState,
  passThrough = false,
): boolean {
  const DIMMED: readonly SignalState[] = ["idle", "stale", "offline", "off"];
  return quiet && !passThrough && (DIMMED as readonly string[]).includes(aggregate);
}

/**
 * 牌面要不要换成「浅字 + 深描边」。**单函数、单真相**：
 *
 * - 用户把滑杆压到一半以下 → `sign_opacity` 本身 < [`SIGN_TRANSLUCENT_BELOW`] → 换。
 * - 安静把牌压到几乎透明 → 视为 [`QUIET_FADE_OPACITY`] → 换。
 *
 * 返回 true 时 `SignalLamp` / `SettingsPanel` 就挂 `data-sign-translucent`
 * （颜色和描边都在 `lamp.css` 里）。
 */
export function signTextNeedsContrast(sign_opacity: number, quietDimmed = false): boolean {
  const effective = quietDimmed ? Math.min(sign_opacity, QUIET_FADE_OPACITY) : sign_opacity;
  return effective < SIGN_TRANSLUCENT_BELOW;
}

/**
 * 这个状态有没有被单独设过。
 *
 * 「设过」= 表里至少有一个字段不是 null。**判据必须和 `PerStateStyles` 里
 * 「空了就删掉整条」那一处一致**：两边不一致就会出现「摘要说 3 个状态设过、
 * 表里却只有 2 行是彩色的」这种对不上的现象，而这种对不上最难查。
 */
export function stateOverridden(style: StateStyle | undefined): boolean {
  if (!style) return false;
  return (
    style.color != null ||
    style.shape != null ||
    style.blink_ms != null ||
    style.ttl_ms != null
  );
}

/**
 * 全部外观变量（颜色 + 动画 + 透明度），挂到窗口根元素上。
 *
 * 挂根上一次就都生效了：一个窗口里可能有多个状态点（灯球多点、设置面板的
 * 会话列表）。颜色变量由 palettes 产出，这里只补上动画变量。
 *
 * **`--sign-alpha` 也从这里出**（它原来是 `SignalLamp` 自己加在内联样式里的）。
 * 设置面板里那张预览牌要复用同一套变量 —— 两处各写一遍的话，预览显示的透明
 * 度迟早和真正的灯对不上，而「预览和灯不一样」比没有预览更糟。
 */
export function lampVars(settings: Settings): React.CSSProperties {
  const style: Record<string, string> = {
    ...(colorVars(settings) as Record<string, string>),
    // 牌宽 / 牌高倍率。CSS 那边把它们乘在牌的基础尺寸上（`.sign`），和 Rust 的
    // `sign_width` / `sign_height` 是两处真值 —— 改一处必须同步另一处（见 lamp.css 文件头）。
    // 1.0（默认）时是恒等，不改变任何现有几何。
    "--sign-w-mult": String(settings.sign_w_mult),
    "--sign-h-mult": String(settings.sign_h_mult),
    // 背景透明度：CSS 里当 `--sign-bg` 的 alpha 用（0 = 全透明，1 = 不透明）
    "--sign-alpha": String(settings.sign_opacity / 100),
  };
  for (const [state, anim] of Object.entries(resolveAnimations(settings))) {
    style[`--anim-${state}`] = anim;
  }
  return style as React.CSSProperties;
}
