import type { Settings, Shape, StateStyle, SignGap, SignCorner, SignCorners, LightTrigger } from "./useSettings";
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
 * 这一块牌属于哪一档视觉分量。
 *
 * 灯上同时会有好几块牌，而它们抢的是同一份余光。早先每块牌的牌底、点径、
 * 字重、状态词长度全都一样 —— 于是「等你授权的那块」和「刚跑完的那块」读起来
 * 一样响，只能逐块读字才知道该先看谁。分层就是回答「谁在叫我」。
 *
 * **只有两档，因为规则要能被记住**：
 * - `need` —— 三件真的需要你动手的事：授权、关注、出错；
 * - `calm` —— 其余七个（推理中 / 执行中 / 完成 / 空闲 / 沉默 / 离线 / 暂停），
 *   它们只是「现在怎么样」，不是「等你做什么」。
 *
 * **`done` 归 calm 是刻意的**：它值得看一眼，但不需要行动；而「安静模式」里
 * done 不算安静（见 lamp.css 那段压暗规则），所以它仍然会显形，只是不抢前排。
 *
 * 视觉落点在 `lamp.css` 的 `.sign[data-tone]`：need 的点径 0.72×、calm 的
 * 0.55×，calm 的牌底再乘 0.72、组名降到 400 字重。**没有一处动窗口几何** ——
 * 点径是 CSS 变量，牌宽高仍由 `config.rs` 与 lamp.css 共享那两个值。
 */
export type SignTone = "need" | "calm";

const NEEDS_YOU: readonly SignalState[] = ["permission", "attention", "error"];

export function toneOf(state: SignalState): SignTone {
  return NEEDS_YOU.includes(state) ? "need" : "calm";
}

/**
 * 「只在有活动时亮」的反面：这些状态下牌上已经没什么在发生了。
 *
 * 和「安静模式」压暗的那张表（lamp.css `[data-quiet][data-aggregate]`）**不是同一份**：
 * 那边刻意不含 `done`（完成值得看一眼），这边含它（完成不是「还在活动」）。
 * 两个问题各自一张表，别为了省一行把它们合并。
 */
const INACTIVE_STATES: readonly SignalState[] = ["idle", "done", "stale", "offline", "off"];

/** 边框光触发判定的输入。`quiet` = **此刻整盏灯是否被安静模式压暗**（不是开关本身）。 */
export interface LightContext {
  state: SignalState;
  quiet: boolean;
}

/**
 * 这盏边框光现在该不该亮。
 *
 * 「外观」决定亮起来长什么样，「行为」决定凭什么现在亮 —— 这条纯函数就是后者，
 * 边灯和四角角灯共用它（各传各的 trigger）。抽成纯函数是因为灯的每一帧都在
 * 重渲染，判定藏在 JSX 里的话四档只能靠真机盯着灯等状态切换来验证。
 *
 * 注意 `quiet` 传进来的是**已经算好的**「此刻被压暗了吗」：安静模式看的是整灯
 * 聚合状态，而边灯/角灯是逐块牌画的，两处的粒度不一样，混在这里算就会得到
 * 「红灯被压暗了、绿灯却没有」这种自相矛盾的画面。
 */
export function lightOn(trigger: LightTrigger, ctx: LightContext): boolean {
  switch (trigger) {
    case "always":
      return true;
    case "attention":
      return toneOf(ctx.state) === "need";
    case "active":
      return !INACTIVE_STATES.includes(ctx.state);
    case "quiet":
      return !ctx.quiet;
  }
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
  // 是这个常驻窗口唯一的 CPU 违例（红线 2）。「落定」的观感现在由**跃迁闪**
  // 承担（见 SignalLamp 的 `data-pulse` / lamp.css 的 `edge-flash`），而它更通用：
  // 任何状态变化都会闪一下描边，不只 done。
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
 * 每个状态的柔光呼吸 `animation` 简写，挂在灯珠的 `--halo-anim` 上。
 *
 * 灯珠本身的闪烁（`--anim-*`）是 `pulse`（opacity 1↔0.25），柔光不必跟着
 * 同步——它应该比灯珠「慢一拍」、范围更柔，读起来像灯珠在「呼吸」而不是「闪」。
 * 所以柔光用**自己的** keyframe `halo-breathe`（opacity 0.6↔0.16），时长是灯珠
 * 节奏的 **2 倍**——同样的节奏感，但更缓。
 *
 * - 常亮状态（done / idle / stale / offline / off）→ `"none"`：一圈稳定的柔光，
 *   不动。常亮是「没有变化」，柔光也不该动。
 * - 用户单独设了 `blink_ms` 的状态 → 柔光也跟着那个节奏的 2 倍呼吸。
 * - 全局倍率同样作用于柔光——和灯珠同步呼吸，用户调快闪频率时柔光也跟着变。
 */
export function resolveHaloAnimations(settings: Settings): Record<SignalState, string> {
  const speed = settings.blink_speed / 100;
  const out = {} as Record<SignalState, string>;
  for (const state of SIGNALS) {
    const blink = settings.styles[state]?.blink_ms;
    // 用户显式设成常亮
    if (blink === 0) {
      out[state] = "none";
      continue;
    }
    // 用户显式设了节奏：柔光用 2 倍时长
    if (typeof blink === "number" && blink > 0) {
      out[state] = `halo-breathe ${blink * 2}ms ease-in-out infinite`;
      continue;
    }
    // 没单独设：跟基础配方走，但时长 ×2
    const recipe = BASE_ANIMATION[state];
    out[state] = recipe
      ? `halo-breathe ${Math.round(recipe.ms * 2 / speed)}ms ease-in-out infinite`
      : "none";
  }
  return out;
}

/**
 * 牌面不透明度滑杆的**下限**（百分比）。前端唯一真值：设置页滑杆的 `min` 读它，
 * `--sign-alpha` 也由它夹；Rust 那边是 `config::MIN_SIGN_OPACITY`，同一个数字
 * ——手改 config.json、或旧版本存下的更小值，都会在 `Settings::clamped()` 里被
 * 夹上来，所以灯永远不会渲染出比这更淡的牌。
 *
 * **为什么要有下限（而不是继续打补丁）**：牌里的字是浅色的（组名 `#ececef`、
 * 状态 `#c2c2cb`），它的对比**全部**来自牌面那层深色。α 低于这条线时透出来的桌面
 * 比牌面还显眼，字其实是画在桌面上 —— 近白的程序窗口上就淡得看不见。
 *
 * 早先的补法是：α < 50 时给牌里的字加一圈深色 `text-shadow` 描边。**那是个坏
 * 补丁**：笔画被边吃掉、小字号直接糊成一团、近白字心在黑边上读起来像空心字。
 * 用户一眼就看得出「这是被救过的」，观感比「淡一点但干净」差得多。可读性这件事
 * 应该在下限上回答掉，而不是渲染时补救。
 *
 * **为什么是 65 而不是更低的 50**：这条线是拿对照图量出来的，不是算出来的。
 * 同一块牌按 50/60/65/70/78/86 排一列，铺在近白窗口（`#fbfbfc`）和浅灰壁纸
 * （`#cfd3da`）上比：50% 时牌面在白底上合成 `#8b8b8d`，次要文字（当时的
 * `#9b9ba3`）对比只有约 1.1:1 —— 那不是「淡」，是消失，用户说的「根本看不清」
 * 就是这个。60% 好转但余量太小，65% 才稳（约 3:1）。
 *
 * **它与「灰字太灰」是两件事，得一起改**：就算不透明度满档，原来的 `#9b9ba3`
 * 在浅底上也只有 3.9:1，同样偏弱 —— 所以 `--sign-muted` 同时从 `#9b9ba3` 提到
 * `#c2c2cb`（见 lamp.css）。只抬下限不动字色，等于让一个灰字配一块更深的灰底。
 *
 * **想要隐形效果的人有另一条路**：安静模式（没事时整盏压到 `opacity: 0.15`，
 * 见 lamp.css）。那条路上本来就不指望你读字，所以也就不存在「字读不清」。
 */
export const SIGN_OPACITY_MIN = 65;

/**
 * calm 档（[`toneOf`]）牌底相对用户设定值的压淡倍数，以及它的**托底**。
 *
 * `need` 档用用户设的 α 原样；`calm` 档再退一档（见 lamp.css 的
 * `.sign[data-tone="calm"]`）。这是「谁在等我」的层次，不是可读性手段。
 *
 * **为什么要托底**：默认 0.86 × 0.72 = 0.62 读得清；但滑杆到下限 0.65 时，
 * 0.65 × 0.72 = 0.468 就跌破可读线了 —— 而「多组 + 低透明度」恰恰是最常见的组合，
 * 也正是描边时代最难看的那种画面。所以压淡取 `max(α × 0.72, 0.6)`：低端时
 * 层次差被吃平（calm 与 need 只差 0.05，主要靠点径 0.55/0.72 和组名字重分先后），
 * 换来两块牌都还读得出字。**这个取舍是刻意的**：分层是锦上添花，读不清是硬伤；
 * 默认档（86%）上是 0.86 vs 0.619，层次照旧。
 *
 * **托底必须低于下限**：一开始图省事写成 `= SIGN_OPACITY_MIN / 100`，结果下限抬到
 * 65 之后托底变成 0.65 > 默认档的 0.619 —— 反而把最常见的那档 calm 牌拉实了，
 * 分层在默认设置下就没了。两个数是一个「乘积的下界」和一个「滑块的下界」，不是
 * 同一件事，别合并。
 *
 * **乘数为什么在 JS 而不是 CSS 的 `calc()`**：一是没必要去赌各图层的 webview 对
 * `max()` 出现在 alpha 位上的支持（赌输的表现是整条 `background` 失效、牌底变透明，
 * 比原来难看十倍）；二是这个数值得有单测。
 */
export const CALM_ALPHA_FACTOR = 0.72;
/** calm 档自身的可读下界（见上：必须低于 `SIGN_OPACITY_MIN`，否则吃掉默认档的层次）。 */
export const CALM_ALPHA_FLOOR = 0.6;

/**
 * 多块牌之间的间距档位 → 相对 `size / 2` 的倍率。
 *
 * **外观里唯一影响窗口几何的一项**：牌竖着叠，间距变大窗口就变高，所以
 * `config.rs::window_size` 必须用同一个倍率算总高（那边是 `SignGap::px`）。
 * 两处真值：这里给 CSS 的 `--sign-gap`（`lamp.css` 的 `.lamp-signs { gap }`），
 * Rust 那边给窗口尺寸。`standard` 是 1.0 = 恒等，默认档不许改变任何现有几何。
 */
export const SIGN_GAP_MULT: Record<SignGap, number> = {
  tight: 0.5,
  standard: 1,
  loose: 2,
};

/**
 * 牌面四角的顺序：顺时针，从左上开始。
 *
 * 顺序**只影响设置面板里四行控件的排列**，不影响灯 —— 每枚角灯的位置由 `pos`
 * 决定，和这个数组的次序无关。写在这里是为了让「设置里第一行 = 左上角」有唯一
 * 出处，而不是两处各写一遍 `["tl","tr","br","bl"]`。
 */
export const CORNER_POSITIONS = ["tl", "tr", "br", "bl"] as const;

/** 这一块牌的四角要挂什么（[`cornerLights`] 的输入）。 */
export interface CornerFacts {
  /** 这一组被置顶了（`group_meta[组].order` 有值）。 */
  pinned: boolean;
  /** 这一组被静音了（`muted_groups` 里有它）。 */
  muted: boolean;
  /**
   * 沉默进度：`null` = 没在沉默。
   *
   * 只有 `stale` 的组才有值，语义和灯珠 `--progress` 完全一样（1 = 刚开始沉默，
   * 0 = 马上就要消失）。复用它而不是另算一份，两边才不会给出不同的答案。
   */
  staleProgress: number | null;
}

export interface CornerLight {
  pos: (typeof CORNER_POSITIONS)[number];
  kind: SignCorner;
}

/**
 * 四角最终画哪几枚 —— 只返回**该亮**的那些，不亮的不返回。
 *
 * **不亮 ≠ 画一个暗点**：一枚「置顶」角灯在没置顶的组上必须整个消失。画暗的话，
 * 用户看到的是一枚「快灭了」的灯，而它其实在说反话。所以这里做的是过滤，
 * 不是给 `data-on` 之类的东西。
 *
 * 唯一的例外是 `state`：它没有条件，配了就一直在 —— 那正是「关掉圆点之后仍然
 * 要有颜色」的用法。
 *
 * `gate` 是「行为」页那道触发开关的结果（`lightOn`）。**它排在一切条件之前**：
 * 触发方式说「现在别亮」的时候，连无条件亮的 `state` 那枚也得消失 —— 否则用户
 * 设了「只在需要我处理时亮」，空闲的牌上却还挂着一枚常亮的状态点，等于没设。
 *
 * 纯函数，有单测（`lampStyle.test.ts`）：卡片组件每帧都在重渲染，判定逻辑藏在
 * JSX 里的话，五种情况只能靠肉眼看灯验，而这盏灯不是所有机器上都摆得出来。
 */
export function cornerLights(
  corners: SignCorners,
  facts: CornerFacts,
  gate = true,
): CornerLight[] {
  if (!gate) return [];
  const out: CornerLight[] = [];
  for (const pos of CORNER_POSITIONS) {
    const kind = corners[pos];
    if (kind === "empty") continue;
    if (kind === "pinned" && !facts.pinned) continue;
    if (kind === "muted" && !facts.muted) continue;
    if (kind === "stale" && facts.staleProgress === null) continue;
    out.push({ pos, kind });
  }
  return out;
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
 * **`--sign-alpha` / `--sign-alpha-calm` 也从这里出**（前者原来是 `SignalLamp`
 * 自己加在内联样式里的）。设置面板里那张预览牌要复用同一套变量 —— 两处各写一遍
 * 的话，预览显示的透明度迟早和真正的灯对不上，而「预览和灯不一样」比没有预览更糟。
 */
export function lampVars(settings: Settings): React.CSSProperties {
  // 牌面的两种不透明度：用户设的那档（need + 一切非分层的牌），和 calm 档压淡后
  // 的那档（带托底，见 [`CALM_ALPHA_FACTOR`]）。两个都在这里算好，CSS 只负责取用
  // —— `max()` 不放进 alpha 位，理由见上。
  const alpha = settings.sign_opacity / 100;
  const style: Record<string, string> = {
    ...(colorVars(settings) as Record<string, string>),
    // 牌宽 / 牌高倍率。CSS 那边把它们乘在牌的基础尺寸上（`.sign`），和 Rust 的
    // `sign_width` / `sign_height` 是两处真值 —— 改一处必须同步另一处（见 lamp.css 文件头）。
    // 1.0（默认）时是恒等，不改变任何现有几何。
    "--sign-w-mult": String(settings.sign_w_mult),
    "--sign-h-mult": String(settings.sign_h_mult),
    // 牌间距倍率。**唯一会影响窗口高度的一项** —— 倍率在 `SIGN_GAP_MULT`，
    // Rust 的 `SignGap::px` 是同一套数字（两处真值，见 lamp.css 文件头）。
    "--sign-gap": String(SIGN_GAP_MULT[settings.sign_gap] ?? 1),
    // 背景透明度：CSS 里当 `--sign-bg` 的 alpha 用（0 = 全透明，1 = 不透明）
    "--sign-alpha": String(alpha),
    "--sign-alpha-calm": String(Math.max(alpha * CALM_ALPHA_FACTOR, CALM_ALPHA_FLOOR)),
  };
  for (const [state, anim] of Object.entries(resolveAnimations(settings))) {
    style[`--anim-${state}`] = anim;
  }
  return style as React.CSSProperties;
}
