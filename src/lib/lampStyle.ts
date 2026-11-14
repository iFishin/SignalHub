import { CORNER_SLOTS, type Settings, type Shape, type StateStyle, type SignGap, type SignCorner, type CornerPos, type SignLights, type EdgeLight, type EdgePos, type LightTrigger, type FoldLabel } from "./useSettings";
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

/**
 * 边框光触发判定的输入。
 *
 * `quiet` = **此刻整盏灯是否被安静模式压暗**（不是开关本身）。后面那些可选字段
 * 是九档条件里「带参数的三档」要用的：挑哪个状态、时段起止、以及逐块牌的事实
 * （沉默进度 / 消息 / 置顶）。不传时一律取默认，这样旧的四档调用方不用改。
 */
export interface LightContext {
  state: SignalState;
  quiet: boolean;
  /** `state` 档：用户挑的状态，默认 `permission`。 */
  matchState?: SignalState;
  /** `schedule` 档：本地时间的分钟数（0–1439）。 */
  from?: number;
  to?: number;
  /** 此刻的本地分钟数。**测试要注入**，别在这里偷偷读 `new Date()` 还指望断言稳定。 */
  minutes?: number;
  /** `stale` / `message` / `pinned` 三档用的牌面事实。 */
  staleProgress?: number | null;
  message?: string | null;
  pinned?: boolean;
}

/** 本地时钟的分钟数。只有调用方没注入 `ctx.minutes` 时才读一次。 */
function localMinutes(): number {
  const now = new Date();
  return now.getHours() * 60 + now.getMinutes();
}

/**
 * 时段判定。`from === to` 记成「整天」而不是「零长度」—— 用户把两个时间拨到
 * 同一格时想表达的显然是「别按时间筛了」，给一个永远不亮的结果没有任何提示。
 * 起 > 止就是跨夜（22:00–06:00），按两段并集算。
 */
/** 分钟数 → `<input type="time">` 要的 `HH:MM`。设置控件和判定之间的唯一换算。 */
export function minutesToClock(minutes: number): string {
  const value = ((minutes % 1440) + 1440) % 1440;
  return `${String(Math.floor(value / 60)).padStart(2, "0")}:${String(value % 60).padStart(2, "0")}`;
}

/** `HH:MM` → 分钟数。用户清空输入框时不写回设置（返回 `null` 让调用方跳过）。 */
export function clockToMinutes(clock: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(clock.trim());
  if (!match) return null;
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  if (hours > 23 || minutes > 59) return null;
  return hours * 60 + minutes;
}

export function inTimeWindow(minutes: number, from: number, to: number): boolean {
  if (from === to) return true;
  return from < to ? minutes >= from && minutes < to : minutes >= from || minutes < to;
}

/**
 * 这盏边框光现在该不该亮。
 *
 * 「外观」决定亮起来长什么样，「行为」决定凭什么现在亮 —— 这条纯函数就是后者，
 * 边灯和四角角灯**共用同一张条件表**（各传各的 trigger）。抽成纯函数是因为灯的
 * 每一帧都在重渲染，判定藏在 JSX 里的话九档只能靠真机盯着灯等状态切换来验证。
 *
 * 注意 `quiet` 传进来的是**已经算好的**「此刻被压暗了吗」：安静模式看的是整灯
 * 聚合状态，而边灯/角灯是逐块牌画的，两处的粒度不一样，混在这里算就会得到
 * 「红灯被压暗了、绿灯却没有」这种自相矛盾的画面。
 *
 * `state` / `schedule` 之外的五档都是**逐块牌**的事实判定，所以边灯设成「指定状态」
 * 时，同一盏灯里也可以有的牌亮、有的不亮 —— 和角灯那套门控是同一个道理。
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
    case "state":
      return ctx.state === (ctx.matchState ?? "permission");
    case "schedule":
      return inTimeWindow(
        ctx.minutes ?? localMinutes(),
        ctx.from ?? 9 * 60,
        ctx.to ?? 18 * 60,
      );
    // 1.0 = 刚转 stale，0.0 = 即将被删（见 types.ts）。后半程才值得再提醒一次。
    case "stale":
      return ctx.staleProgress != null && ctx.staleProgress <= 0.5;
    case "message":
      return (ctx.message ?? "").trim().length > 0;
    case "pinned":
      return ctx.pinned === true;
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
 * 多块牌之间的间距档位 → 相对 `size / 2` 的倍率。
 *
 * **外观里唯一影响窗口几何的一项**：牌竖着叠，间距变大窗口就变高，所以
 * `config.rs::window_size` 必须用同一个倍率算总高（那边是 `SignGap::px`）。
 * 两处真值：这里给 CSS 的 `--sign-gap`（`lamp.css` 里 `.sign` 的 `--row-gap`），
 * Rust 那边给窗口尺寸。`standard` 是 1.0 = 恒等，默认档不许改变任何现有几何。
 * 0.6.7 起它是「板内每行的底部留白」，不再是 `.lamp-root` 的 flex gap。
 */
export const SIGN_GAP_MULT: Record<SignGap, number> = {
  tight: 0.5,
  standard: 1,
  loose: 2,
};

/**
 * 折叠窄板一格宽度相对 `size` 的倍率。**和 Rust `config::FoldLabel::width_mult`
 * 是两处真值**，改一处必须同步另一处。`none` 不画板，格子就是一颗 `size` 见方的点。
 *
 * 数字是按「一格还能放下几个汉字」反推的，不是按「组名排完」——折叠的用途是
 * 让出桌面，一格只要够认出是哪一组。组名档 4.0（22px 档 88px、窗口 116px）
 * 对展开牌 154px / 窗口 182px 窄 36%；6.0 那版只窄 22px，等于没折叠。
 *
 * 窗口宽只由设置决定，组名 / 状态词再长也不会让窗口跳一下（超长一律省略号）。
 *
 * **这三档是窗口的容量上限，不是板宽**：0.6.8 起 `.fold-row` 只拿它当
 * `max-width`，窄板按实际文字收，短词右侧不再留一截深色空板（见 `lamp.css`
 * 「折叠态带文字」那节）。改这里**只**影响窗口留白，别为了「和板对齐」改小。
 */
export const FOLD_WIDTH_MULT: Record<FoldLabel, number> = {
  none: 1,
  index: 2.4,
  state: 3.5,
  name: 4,
};

/**
 * 牌面四角的顺序：顺时针，从左上开始。
 *
 * 顺序**只影响设置面板里四行控件的排列**，不影响灯 —— 每枚角灯的位置由 `pos`
 * 决定，和这个数组的次序无关。写在这里是为了让「设置里第一行 = 左上角」有唯一
 * 出处，而不是两处各写一遍 `["tl","tr","br","bl"]`。
 */
export const CORNER_POSITIONS = CORNER_SLOTS;

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
  /**
   * 此刻是不是在点击穿透（`useToggles` 的瞬态状态，不进 Settings）。
   * 可选是因为它是**全局**事实而不是组的属性：设置页预览想看这枚灯时直接给 true，
   * 老调用方不给也没关系（那一枚就按「条件不成立」不画）。
   */
  passThrough?: boolean;
}

export interface CornerLight {
  pos: CornerPos;
  kind: SignCorner;
  /** 这一枚要不要闪（槽里的 `blink`）。关掉的灯根本不返回，所以这里恒为「亮着且要闪」。 */
  blink: boolean;
}

/**
 * 上下两条边现在各画成什么样。
 *
 * 0.6.5 之前整盏灯只有一个 `data-edge`，上下永远一模一样。现在每条边各判各的
 * 触发条件：同一块牌可以「上边常亮、下边只在出错时亮」。判据和四角共用同一份
 * `LightContext`，所以「指定状态 / 指定时段」这些带参档位在六个槽里是同一个答案。
 *
 * 不亮返回 `"off"` 而不是 `null`：CSS 那边只要一个 `[data-edge-top="off"]` 就能
 * 决定画不画，不用再给每枚灯挂一个 `data-on`。
 */
export function edgeLightKinds(
  lights: SignLights,
  ctx?: LightContext,
): Record<EdgePos, EdgeLight> {
  // 不传 ctx = 设置页那张静态预览：只看画法、不看「什么时候亮」。
  // 预览跟着触发走的话，配成「只在出错时亮」就什么都画不出来，会被当成没生效。
  return {
    top: !ctx || lightOn(lights.top.trigger, ctx) ? lights.top.kind : "off",
    bottom: !ctx || lightOn(lights.bottom.trigger, ctx) ? lights.bottom.kind : "off",
  };
}

/**
 * 四角最终画哪几枚 —— 只返回**该亮**的那些，不亮的不返回。
 *
 * **不亮 ≠ 画一个暗点**：一枚「置顶」角灯在没置顶的组上必须整个消失。画暗的话，
 * 用户看到的是一枚「快灭了」的灯，而它其实在说反话。所以这里做的是过滤，
 * 不是给 `data-on` 之类的东西。
 *
 * `ctx` 可选：给了就走**两道判据**，触发排在第一；不给（设置页预览）就只走第二道，
 * 按常亮画。每个角现在有自己的 `trigger`，触发说「现在别亮」
 * 时连无条件亮的 `state` 那枚也得消失 —— 否则用户设了「只在需要我处理时亮」，
 * 空闲的牌上却还挂着一枚常亮的状态点，等于没设。0.6.5 之前四角共用一个 gate，
 * 想让「左上常亮、右上只在出错时亮」根本做不到。
 *
 * 第二道是这枚灯自己的事实（置顶 / 静音 / 沉默 / 穿透）。`state` 没有第二道，
 * 配了就一直在 —— 那正是「关掉圆点之后仍然要有颜色」的用法。
 *
 * 纯函数，有单测（`lampStyle.test.ts`）：卡片组件每帧都在重渲染，判定逻辑藏在
 * JSX 里的话，六种情况只能靠肉眼看灯验，而这盏灯不是所有机器上都摆得出来。
 */
export function cornerLights(
  lights: SignLights,
  facts: CornerFacts,
  ctx?: LightContext,
): CornerLight[] {
  // 不传 ctx = 设置页那张静态预览：只按「挂了什么」「事实成立吗」画，
  // 不套用行为页的九档触发。两处共用同一份事实判据，别在预览里再抄一遍。
  const out: CornerLight[] = [];
  for (const pos of CORNER_POSITIONS) {
    const slot = lights[pos];
    if (ctx && !lightOn(slot.trigger, ctx)) continue;
    const kind = slot.kind;
    if (kind === "empty") continue;
    if (kind === "pinned" && !facts.pinned) continue;
    if (kind === "muted" && !facts.muted) continue;
    if (kind === "stale" && facts.staleProgress === null) continue;
    // 穿透是全局状态：开着的时候**每块牌**都挂，不是只挂第一块。它和 pinned /
    // muted 一样是「条件不成立就不画」，所以没开穿透时牌上连一个空心方都没有。
    if (kind === "passthrough" && facts.passThrough !== true) continue;
    out.push({ pos, kind, blink: slot.blink });
  }
  return out;
}

/**
 * 上下两条边现在要不要闪。
 *
 * 和 [`edgeLightKinds`] 分开而不是合成一个对象：那边回答「画成什么样」（CSS 用
 * `data-edge-top="line"` 这种值直接选规则），这边只是一个开关。合成之后调用方
 * 每处都要多拆一层 `.kind`，而真正需要这两样一起看的地方只有渲染那一处。
 *
 * **不亮就不闪**：`kind` 是 `off` 时返回值恒为 `false` —— 没画出来的东西不存在
 * 「闪」这回事，也不用让 CSS 去判「有没有边灯」。
 */
export function edgeBlinks(
  lights: SignLights,
  ctx?: LightContext,
): Record<EdgePos, boolean> {
  const on = (kind: EdgeLight, blink: boolean, trigger: LightTrigger): boolean =>
    kind !== "off" && blink && (!ctx || lightOn(trigger, ctx));
  return {
    top: on(lights.top.kind, lights.top.blink, lights.top.trigger),
    bottom: on(lights.bottom.kind, lights.bottom.blink, lights.bottom.trigger),
  };
}

/**
 * 边灯 / 角灯闪烁的兜底周期。
 *
 * 闪的节奏**跟着这块牌的状态走**（`working` 1600ms、`error` 340ms 硬闪……），
 * 这样两个灯与灯珠说的是同一件事。但 `done` / `idle` 这类状态本来没有节奏 ——
 * 那时候给一个兜底周期，而不是「什么都不发生」：用户明确打开了某一枚灯的闪烁，
 * 打开之后纹丝不动会被当成开关坏了（0.6.5 的角灯就吃过这个亏）。
 */
export const LIGHT_BLINK_FALLBACK_MS = 1400;

/**
 * 这个状态该给边灯 / 角灯用哪条 `animation` 简写。
 *
 * 取值顺序与灯珠一致（用户单独设的节奏 > 基础配方），差别只在 keyframe：
 * 灯珠用 `pulse`（1 ↔ 0.25），这里用 `light-blink` —— 它把两端的透明度挂在
 * `--blink-a` 上，于是「明暗」那一档、以及静音角灯的 0.78、沉默角灯的进度淡出
 * 都不会在闪的时候被抹平。
 *
 * 用户把某个状态显式设成常亮（`blink_ms = 0`）时**仍然会闪**：那个 0 说的是
 * 「灯珠别闪」，而这枚灯的开关是它自己槽里的 `blink`，两者不是一回事。
 */
export function lightBlink(settings: Settings, state: SignalState): string {
  const speed = settings.blink_speed / 100;
  const explicit = settings.styles[state]?.blink_ms;
  if (typeof explicit === "number" && explicit > 0) {
    return `light-blink ${explicit}ms ease-in-out infinite`;
  }
  const recipe = BASE_ANIMATION[state];
  if (recipe) {
    return `light-blink ${Math.round(recipe.ms / speed)}ms ${recipe.timing} infinite`;
  }
  return `light-blink ${Math.round(LIGHT_BLINK_FALLBACK_MS / speed)}ms ease-in-out infinite`;
}

/**
 * 哪些状态的节奏要**错相**。只有「它在干活」这两种。
 *
 * 一盏灯里同时跑着三四个组时，它们的 `working` / `thinking` 是同一个配方、同一个
 * 起点 —— 整块板同频同相地闪，读起来像一盏坏掉的灯，而不是「几件事各自在跑」。
 * 错开相位之后，余光里看到的是错落的呼吸。
 *
 * **要你处理的三种一律不错相**（`attention` / `permission` / `error`）：警报就该
 * 整齐、同步、一眼看见；把它们也打散，等于把「一起在叫你」稀释成「有点吵」。
 * 剩下那些本来就不动（`done` / `idle` / `stale` / `offline` / `off`），相位对它们
 * 没有意义。
 */
const RHYTHM_PHASED: readonly SignalState[] = ["thinking", "working"];

/** 某个状态最终的节奏周期（ms）。口径与 `resolveAnimations` 逐字一致。 */
function rhythmPeriodMs(settings: Settings, state: SignalState): number {
  const speed = settings.blink_speed / 100;
  const explicit = settings.styles[state]?.blink_ms;
  if (explicit === 0) return 0;
  if (typeof explicit === "number" && explicit > 0) return explicit;
  const recipe = BASE_ANIMATION[state];
  return recipe ? Math.round(recipe.ms / speed) : 0;
}

/**
 * 把组名散成一个周期内的相位（0..period-1）。
 *
 * **按名字散，不按序号散**：序号会随「哪一组更紧急」在排序里跳，同一块牌下一次
 * 刷新就换了相位，观感上是「它抖了一下」。组名在一组生命期内不变，于是同一块牌
 * 永远停在自己那个相位上，重启也不变 —— 这条稳定性比「散得均匀」更要紧。
 *
 * FNV-1a：十行就能写完、分布够用、不依赖任何库。`Math.imul` 是必须的 ——
 * 直接 `*` 会在 2^53 之后丢精度，长中文组名算出来的相位就不稳定了。
 */
export function rhythmPhase(name: string, periodMs: number): number {
  if (!(periodMs > 0)) return 0;
  let hash = 0x811c9dc5;
  for (let i = 0; i < name.length; i += 1) {
    hash = Math.imul(hash ^ name.charCodeAt(i), 0x01000193);
  }
  return (hash >>> 0) % periodMs;
}

/**
 * 挂到一块牌上的 `animation-delay`，用来把「在干活」的呼吸错开。
 *
 * 返回**负值**是刻意的：负延迟让动画一上来就处在自己那个相位上，而不是先停
 * 半拍再开始 —— 后者在切换形态 / 重渲染时会被看成「卡了一下」。
 *
 * 只喂给三个地方，且都靠 `--rhythm-delay` 这一个变量：灯珠（`pulse`）、灯珠的
 * 柔光（周期是灯珠的 2 倍，所以 CSS 里乘 2）、可选的边灯 / 角灯闪烁。同一块牌
 * 的这三样因此永远同相 —— 它们本来就在说同一句话。
 */
export function rhythmDelay(settings: Settings, name: string, state: SignalState): string {
  if (!RHYTHM_PHASED.includes(state)) return "0ms";
  return `${-rhythmPhase(name, rhythmPeriodMs(settings, state))}ms`;
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
 * **`--sign-alpha` 也从这里出**（原来是 `SignalLamp` 自己加在内联样式里的）。
 * 设置面板里那张预览牌要复用同一套变量 —— 两处各写一遍的话，预览显示的透明度
 * 迟早和真正的灯对不上，而「预览和灯不一样」比没有预览更糟。
 * 0.6.7 起不再有 `--sign-alpha-calm`：共享底板后 calm 档改由薄雾 / 字重 / 点径
 * 表达层次（见 lamp.css 的「视觉分量」一节）。
 */
export function lampVars(settings: Settings): React.CSSProperties {
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
  };
  // 折叠态的四个几何数**在这里算成整数 px**，CSS 不再用小数 `calc` 重算一遍：
  // 旧的 `gap: calc(size / 3)` 与 Rust 的 `size / 3` 整除最多差 2/3 px，多点时
  // 累加起来会把最外侧那颗点削掉一线。两处真值是这里 ↔ `config::folded_size`。
  // 带文字时格宽 = size × 档位倍率 × 宽倍率（向上取整，方向和 sign_width 一致），
  // 格高直接取一块单行牌的高度 —— 折叠窄板和展开的行是同一把尺子。
  // 老配置 / 演示帧可能没带这两个新字段，兜底成默认值，别算出 "NaNpx"。
  const foldLabel = settings.fold_label ?? "none";
  const hasFoldText = foldLabel !== "none";
  const cellW = hasFoldText
    ? Math.ceil(settings.size * FOLD_WIDTH_MULT[foldLabel] * settings.sign_w_mult)
    : settings.size;
  const cellH = hasFoldText
    ? Math.ceil(Math.floor((settings.size * 3) / 2) * settings.sign_h_mult)
    : settings.size;
  style["--fold-cell-w"] = `${cellW}px`;
  style["--fold-cell-h"] = `${cellH}px`;
  style["--fold-gap"] = `${Math.floor(settings.size / 3)}px`;
  // 板内一格的水平内边距。它只作用在固定宽度的格子里，不进窗口几何，
  // 所以不必和 Rust 对齐 —— 取整只是为了让小尺寸下左右完全对称。
  style["--fold-pad"] = `${Math.floor(settings.size / 4)}px`;
  for (const [state, anim] of Object.entries(resolveAnimations(settings))) {
    style[`--anim-${state}`] = anim;
  }
  return style as React.CSSProperties;
}
