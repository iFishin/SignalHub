import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { motion, useReducedMotion } from "motion/react";
import { inTauri } from "../lib/env";
import { requestSettingsTab } from "../lib/settingsIntent";
import { signalLabel, justFinishedLabel } from "../lib/signals";
import { useLang, useMessages, type Messages, type Lang } from "../lib/i18n";
import { lampVars, resolveShapes, resolveHaloAnimations, toneOf, cornerLights, edgeLightKinds, edgeBlinks, lightBlink, rhythmDelay, type LightContext } from "../lib/lampStyle";
import { transitions, type SignSignature } from "../lib/lampTransitions";
import { whyLines } from "../lib/lampWhy";
import { labelOf, labelPrefOf, orderVisible, type LabelPref } from "../lib/lampSort";
import { useNow } from "../lib/useNow";
import { formatRemaining } from "../lib/age";
import { useToggles } from "../lib/useToggles";
import { ApprovalBubble, MAX_APPROVAL_BUBBLES } from "./ApprovalBubble";
import { Icon } from "./Icon";
import { LampMenu } from "./LampMenu";
import type { Approval } from "../lib/useApprovals";
import type { GroupMeta, Settings } from "../lib/useSettings";
import type { GroupView, SignalState } from "../lib/types";
import { meetingTint } from "../lib/rules";

/** 与 Rust 侧 `config::MAX_SIGNS` 一致。超出的牌服务端已经截断。 */
const MAX_SIGNS = 4;
/** 超过这个位移就算拖窗口，否则算点击 / 长按。 */
const DRAG_THRESHOLD_PX = 4;
/**
 * 按住多久算长按（呼出圆环菜单）。
 *
 * 480ms：比 macOS 的长按（约 500ms）略早一点，手指压下去到「觉得它该出来了」
 * 之间不用等太久；又明显长于一次有意的点击，不会误触。**这个值也是单击能
 * 「立刻响应」的前提** —— 见 `useDragOrClick` 里那段说明。
 */
const LONG_PRESS_MS = 480;
/** 跃迁脉冲的时长（ms）。必须和 lamp.css 里 `edge-flash` 的一致。 */
const PULSE_MS = 420;
/** 折叠态悬停临时展开的时长（ms）。到点或鼠标移开就收回折叠态。 */
const PEEK_MS = 3000;
/**
 * 折叠态悬停多久才真的展开（ms）。
 *
 * **这个延迟是必须的，不是手感糖**：折叠态窗口只有 42px 宽（22px 档一颗点），
 * 而展开会把窗口撑到 174px —— 鼠标只是从灯旁边划过去，桌面上就突然多出一块
 * 牌盖住东西。240ms 是「停下来了」和「路过」的分界，比操作系统的 hover 提示
 * （通常 400ms+）短，不会让人觉得迟钝。移开不延迟，立刻收回。
 */
const PEEK_DELAY_MS = 240;

interface Props {
  aggregate: SignalState;
  groups: GroupView[];
  /**
   * 整份设置都传进来，而不是拆成一堆 prop。
   * 外观（大小 / 形态 / 形状 / 安静 / 透明度 / 节奏）和配色都要用，拆开传只会越来越长。
   */
  settings: Settings;
  /** 监控暂停中。灯上要给一个持续可见的提示 —— 颜色上 `off` 和 `offline` 太近。 */
  paused: boolean;
  /**
   * 等着你点的审批（从灯上审批，见 `useApprovals`）。
   *
   * **只在主灯上传**：气泡是「有没有事要我做」，和某一组无关 —— 而组灯是给
   * 「这一组现在怎么样」用的。多灯模式下主灯仍然是那一盏（它退成全局灯）。
   */
  approvals?: Approval[];
  onDecide?: (id: string, decision: "allow" | "deny") => void;
  /**
   * 最近一次「完成」的墙钟时刻（epoch 毫秒），0 = 没见过。
   *
   * 只在**一组都没有**的时候用：那一刻占位牌说的是「没有智能体在上报」，而刚干完
   * 活的人想看的恰恰是「刚完成」。`done` 的牌到期是被整个删掉的，会话没了之后
   * 这份记忆是唯一还知道「刚完成」的东西（见 `lib/signals.ts`）。
   */
  lastDoneMs: number;
  /**
   * 演示 / 预览模式：**只画，不动真东西**。
   *
   * 演示窗口和设置里的「演示」页复用这同一个组件，为的是画面永远和真灯一致。
   * 但拖拽、单击折叠、右键菜单、清组这些交互全都得关掉 ——
   * 它们是「操作这个应用」，不是「看这个应用长什么样」。默认 false，真灯一行不改。
   */
  preview?: boolean;
  /**
   * 预览里要不要假装在点击穿透。穿透是这台机器的**瞬态**状态，不进 Settings，
   * 演示剧本却需要画那一枚角灯 —— 所以给一个只在 `preview` 下生效的覆盖值。
   * 不传 = 预览里一律当没穿透。
   */
  previewPassThrough?: boolean;
}

/** 首次启动引导只记一次。存 localStorage 而不是设置里：它纯粹是「这个窗口有没有
 *  打扰过你」，不该进配置导出、也不该跨机器同步。 */
const FIRST_RUN_KEY = "signalhub.firstRunGuideDone";

function useFirstRunGuide(visible: boolean) {
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem(FIRST_RUN_KEY) === "1";
    } catch {
      return false;
    }
  });
  const dismiss = () => {
    setDismissed(true);
    try {
      localStorage.setItem(FIRST_RUN_KEY, "1");
    } catch {
      // 隐私模式下记不住就算了，这次会话里 setDismissed 仍然生效
    }
  };
  return { show: visible && !dismissed, dismiss };
}

/**
 * 「已持续多久」。牌上那一段短文本，所以只给一个单位、不写秒。
 *
 * `now` 由调用方传进来（见 `useNow`），**不在这里读 `Date.now()`** —— 那样它就只
 * 在重绘时才被求值，而这个组件只在状态变化时重绘，于是永远显示 0s。
 */
function formatAge(sinceMs: number, now: number): string {
  if (!sinceMs) return "";
  const seconds = Math.max(0, Math.round((now - sinceMs) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}

function tooltip(
  m: Messages,
  lang: Lang,
  aggregate: SignalState,
  groups: GroupView[],
  meta: Record<string, GroupMeta>,
  labelPref: LabelPref,
  extra: string[],
  /** 一组都没有时那句话说这句 —— 「刚完成 · N 分钟前」顶替「没有智能体在上报」 */
  emptyLabel: string,
  /** 按「单击做什么」挑好的那一份提示，免得改了设置之后 tooltip 还在说旧话。 */
  hint: string,
): string {

  // 没有组时 tooltip 只剩一句状态。Hints 仍然拼上 —— 空灯球正需要「右键能进设置」。
  if (groups.length === 0) return [emptyLabel, hint, ...extra].join("\n");

  if (groups.length === 1 && extra.length === 0) {
    const only = groups[0];
    const head = `${labelOf(only, meta, labelPref)} · ${signalLabel(lang, only.aggregate)}`;
    const detail = only.message ? ` — ${only.message}` : "";
    return `${head}${detail}\n${hint}`;
  }

  // 多组时把「哪个组在什么状态」列出来 —— 这是多块牌想回答的问题。
  // 被 `+N` 收起来的组也列在这儿：牌上看不见，tooltip 是全窗口唯一能容纳它的地方，
  // 否则那块标记只说得出「有更多」，说不出是什么。
  const listed = groups.map((group) => {
    const detail = group.message ? ` — ${group.message}` : "";
    return `${labelOf(group, meta, labelPref)} · ${signalLabel(lang, group.aggregate)}${detail}`;
  });

  return [
    ...(groups.length > 1 ? [`${signalLabel(lang, aggregate)}${m.groupsCount(groups.length)}`] : []),
    ...listed,
    ...extra,
    hint,
  ].join("\n");
}

/** 「+N」标记的 tooltip：只说得出「还有几组」，说不出是哪几组。
 *
 *  单独摘出来是因为它和 `tooltip` 的用途不同（后者是全窗口的悬停说明，
 *  这里是那一枚小标记自己的）—— 复用一个字符串会让语义糊掉。 */
function moreGroupsTip(
  m: Messages,
  hidden: GroupView[],
  meta: Record<string, GroupMeta>,
  labelPref: LabelPref,
): string {
  return [m.moreGroups(hidden.length), m.moreGroupsLead + hidden.map((g) => labelOf(g, meta, labelPref)).join(", ")].join(
    "\n",
  );
}

/**
 * 沉默进度 → CSS 变量 `--progress`。
 *
 * `lamp.css` 的 `ring` 形状用它算 `conic-gradient` 的弧长（1.0 = 满圈，
 * 0.0 = 环消失）。只有 `stale` 组才有值，别的状态不设这个变量 —— CSS 那边
 * 有 `var(--progress, 1)` 兜底，缺省就是满圈。
 *
 * **返回 `undefined` 而不是 `{}`**：React 会把 `undefined` 当成「不设 style」，
 * 省掉一个空的 `style` 属性；而在 `{}` 和 `undefined` 之间来回切换会让 React
 * 多碰一次 DOM。
 */
function progressStyle(group: GroupView): React.CSSProperties | undefined {
  if (group.stale_progress === null) return undefined;
  return { "--progress": String(group.stale_progress) } as React.CSSProperties;
}

/**
 * 同一个元素上既要能拖窗口、又要能响应单击和长按。
 *
 * 不能用 `data-tauri-drag-region`：它让系统在 mousedown 时接管鼠标，之后的
 * click 事件就不可靠了 —— 于是「点一下折叠」会时灵时不灵。改成自己判断：
 * 按下后位移超过阈值才算拖拽（此时才把拖拽交给系统），否则算点击或长按。
 *
 * **三种手势互斥，判据只有一条时间线**：
 *   - 按住不动满 `LONG_PRESS_MS` → 长按（圆环菜单），之后松开**不再**算点击；
 *   - 期间移动超过阈值 → 拖拽，长按作废、松开也不算点击；
 *   - 没超时也没移动就松开 → 单击。
 *
 * **单击是立刻执行的，没有等待窗口。** 这一版之前是「单击等 280ms 看是不是
 * 双击」，因为那时单击开收件箱、双击折叠，两者不能同时发生。现在折叠挪到了
 * 单击、双击取消（单击已经是折叠，双击等于折叠两次，纯属噪音），那 280ms
 * 就没有存在的理由了 —— 去掉它，点一下灯牌立刻就有反应。
 *
 * 只认主键，所以右键原地就能弹菜单，不会顺带被当成一次点击。
 */
/** 牌尾那几枚小动作必须**吃掉鼠标事件**。
 *
 *  `useDragOrClick` 挂在 `.lamp-root` 上：事件冒上去，按下会开始记拖拽原点、松开
 *  会被当成「点了一下灯」→ 折叠。所以 mousedown / mouseup / click 三个都要拦。
 */
function stopMouse(event: React.MouseEvent) {
  event.stopPropagation();
}

function useDragOrClick(onClick: () => void, onLongPress: () => void) {
  const origin = useRef<{ x: number; y: number } | null>(null);
  const dragging = useRef(false);
  /** 这一轮按下已经变成长按了 —— 松开时不能再算一次单击。 */
  const longPressed = useRef(false);
  const timer = useRef<number | null>(null);

  const clearTimer = () => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  };

  useEffect(() => () => clearTimer(), []);

  return {
    onMouseDown: (event: React.MouseEvent) => {
      if (event.button !== 0) return; // 只认主键
      origin.current = { x: event.clientX, y: event.clientY };
      dragging.current = false;
      longPressed.current = false;
      clearTimer();
      timer.current = window.setTimeout(() => {
        timer.current = null;
        longPressed.current = true;
        // 长按成立后就把原点清掉：菜单一开窗口会变大，鼠标相对位置跟着变，
        // 这时还留着原点的话，接下来任何一点移动都会被判成「拖窗口」。
        origin.current = null;
        onLongPress();
      }, LONG_PRESS_MS);
    },
    onMouseMove: (event: React.MouseEvent) => {
      if (!origin.current || dragging.current) return;
      const moved = Math.hypot(
        event.clientX - origin.current.x,
        event.clientY - origin.current.y,
      );
      if (moved < DRAG_THRESHOLD_PX) return;
      dragging.current = true;
      // 开始拖了，长按作废 —— 否则拖到一半圆环会弹出来
      clearTimer();
      if (inTauri) {
        getCurrentWindow().startDragging().catch(console.error);
      }
    },
    onMouseUp: () => {
      clearTimer();
      const wasClick = origin.current !== null && !dragging.current && !longPressed.current;
      origin.current = null;
      dragging.current = false;
      longPressed.current = false;
      if (!wasClick) return;
      onClick();
    },
  };
}

/**
 * 弹某一组的右键菜单。菜单本身是原生的，见 Rust 侧的 `menu.rs`。
 *
 * 无论有没有组名都要 `preventDefault` —— 否则占位那块牌上会冒出 webview 自己的
 * 右键菜单（dev 模式下是「重新加载」那种），和这个应用格格不入。
 */
function openGroupMenu(event: React.MouseEvent, group: string, why: string[] = []) {
  event.preventDefault();
  event.stopPropagation();
  if (!inTauri) return;
  // 占位那块牌的名字是空的，它不对应任何真实的组 —— 组菜单对它只会藏掉一个叫 ""
  // 的组，什么也不会发生。这时弹的是**灯自己那张菜单**：第一次装上、还没接任何
  // 上报方时，这是灯上唯一能进设置的地方。
  if (group) {
    invoke("open_group_menu", { group, why }).catch(console.error);
  } else {
    invoke("open_lamp_menu").catch(console.error);
  }
}

/** 空白处（牌之间、留白）的右键：吃掉 webview 自己的菜单，弹灯自己那张。 */
function openLampMenu(event: React.MouseEvent) {
  event.preventDefault();
  if (!inTauri) return;
  invoke("open_lamp_menu").catch(console.error);
}

export function SignalLamp({
  aggregate,
  groups,
  settings,
  paused,
  approvals,
  onDecide,
  lastDoneMs,
  preview = false,
  previewPassThrough = false,
}: Props) {
  // 待审批的条数：Rust 的 `approval_area` 按**同一个数**给窗口留高度（见 lamp.css
  // 的 `--bubble-h`）。数错了就是气泡被裁掉一半 —— 而那两个按钮是唯一的出路。
  const pending = approvals?.length ?? 0;
  // 窗口高度只按**画出来的**条数留（Rust 的 `approval_area` 同样夹到 3 条）。
  // 用真实条数算留白的话，4 条以上时牌子和气泡会叠在一起 —— 多出来的那几条
  // 本来就不画（见 `ApprovalBubble`）。
  const bubbles = Math.min(pending, MAX_APPROVAL_BUBBLES);
  const { size, quiet, collapsed, show, hidden_groups } = settings;
  // 折叠态的两个旋钮。展开态一律不读它们 —— 牌永远是竖着叠的一块板。
  const foldAxis = settings.fold_axis;
  const foldLabel = settings.fold_label;
  const m = useMessages();
  const lang = useLang();
  // 穿透也能从托盘和全局快捷键改，所以订阅而不是读一次 —— 配了「穿透」角灯的
  // 那一刻必须立刻跟着变。**牌上那枚常驻的 ban 角标已经删掉了**（0.6.4）：现在
  // 它只是四角里可选的一枚灯，用户没挂就不显示；状态本身仍进 tooltip / 托盘。
  const toggles = useToggles();
  // 演示里不继承本机的「点击穿透」：那是这台机器的真实状态，不该出现在一段和
  // 产品无关的剧本里。hook 仍然无条件调用（React 规则），只是读的时候盖掉；
  // 剧本想演示这枚角灯时用 `previewPassThrough` 显式打开。
  const passThrough = preview ? previewPassThrough : toggles.pass_through;

  // 形状是逐状态的（一个窗口里多块牌可以不一样），所以先整份算出来，再按每块牌
  // 的状态取。颜色和动画走 lampVars 的 CSS 变量，shape走元素属性 —— 形状不是
  // 「一个值」，没法塞进一个根变量里。
  const shapes = resolveShapes(settings);
  // 牌上的名字、tooltip 和「+N」那一行必须说同一个名字。标题偏好从整份设置里
  // 只取两项，别的组件（收件箱 / 分组页）也走同一个 `labelPrefOf`。
  const labelPref = labelPrefOf(settings);

  // 0.6.5：效果从布尔开关扩成五档。柔光呼吸（`breathe`，也就是旧开关开着的
  // 样子）才有逐状态的 halo animation；其余四档由 CSS 各自的 `[data-effect]`
  // 规则画，不走 `--halo-anim`。挂在每个灯珠上（不是根上），这样一个窗口里
  // 多块牌的节奏可以不同。
  const effect = settings.effect;
  const haloAnims = effect === "breathe" ? resolveHaloAnimations(settings) : null;
  // motion 弹簧也尊重系统的「减少动态效果」。motion 的 useReducedMotion 返回
  // boolean | null（null = 未检测），按 falsy 处理。
  const reduceMotion = useReducedMotion();
  const motionOn = effect !== "off" && !reduceMotion;

  // 折叠态悬停**临时**展开：只改渲染 + 让 Rust 把窗口算大，**不改设置** —— 退出
  // 重开仍是折叠态（这是这一项的验收之一）。3s 到点、或鼠标移开就回去。窗口尺寸
  // 由 Rust 按同一个标志算（命令 `set_peek`；多灯下只有被指着的那盏会变），所以
  // 前端和窗口不会打架。
  const [peek, setPeek] = useState(false);

  // 真的展开了（点一下 / 托盘）就把 peek 收掉，免得它残留着和设置打仗
  useEffect(() => {
    if (!collapsed) setPeek(false);
  }, [collapsed]);

  // 把「谁在被 hover」推给 Rust —— 它按这个把那个窗口算成展开态。
  // 演示里绝对不能推：它会去改**真灯窗**的尺寸。
  useEffect(() => {
    if (!inTauri || preview) return;
    invoke("set_peek", { peek }).catch(console.error);
  }, [peek, preview]);

  // 3s 到点自动收回。effect 挂在 `peek` 上：一直 hover 着也只数一次
  useEffect(() => {
    if (!peek) return;
    const timer = window.setTimeout(() => setPeek(false), PEEK_MS);
    return () => window.clearTimeout(timer);
  }, [peek]);

  // 悬停意图：进入后等 `PEEK_DELAY_MS` 才展开（理由见那个常量），移开立刻收回。
  // 定时器单独持一个 ref，不放进 effect —— 它由鼠标事件驱动，不是渲染状态的派生。
  const peekTimer = useRef<number | null>(null);
  const armPeek = () => {
    if (!collapsed) return;
    if (peekTimer.current !== null) window.clearTimeout(peekTimer.current);
    peekTimer.current = window.setTimeout(() => {
      peekTimer.current = null;
      setPeek(true);
    }, PEEK_DELAY_MS);
  };
  const disarmPeek = () => {
    if (peekTimer.current !== null) {
      window.clearTimeout(peekTimer.current);
      peekTimer.current = null;
    }
    setPeek(false);
  };
  useEffect(
    () => () => {
      if (peekTimer.current !== null) window.clearTimeout(peekTimer.current);
    },
    [],
  );

  // 折叠设置 + 临时展开，合成「这一帧到底画哪一态」。**下面所有渲染分支都读它**，
  // 别再直接读 `collapsed` —— 一处分歧就会画成「窗口是展开的大、内容却只有点」。
  const expanded = !collapsed || peek;

  const [flash, setFlash] = useState(false);
  const flashTimer = useRef<number | null>(null);
  const firstRun = useRef(true);

  // 折叠切换的一次性反馈。**只动 opacity**（见 P3：transform 会让这个常驻窗口
  // 每帧重排）。单组时切完连窗口尺寸都不变，没这点反馈等于点了没反应。
  //
  // **开场那一次不闪**：`useEffect` 在挂载时也会跑，不挡掉的话每次启动都白闪一下，
  // 而用户根本没点过 —— 那会读成「应用自己抖了一下」。
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    setFlash(true);
    if (flashTimer.current !== null) window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setFlash(false), 420);
    return () => {
      if (flashTimer.current !== null) window.clearTimeout(flashTimer.current);
    };
  }, [collapsed]);

  // 「已持续」是算出来的，所以得有东西让它随时间重绘 —— 否则它永远停在状态刚变化
  // 那一刻，也就是 0s。折叠形态和关掉这一格的牌上没有这个数字，不起定时器。
  //
  // 「刚完成 · N 分钟前」同理：它也在数分钟，**而且不依赖「已持续」开没开**。
  // 只按 `show.age` 决定起不起钟的话，把「已持续」关掉的用户会看到那句话冻在
  // 打开面板那一刻的分钟数上。它只在一组都没有时才需要 —— 有会话时那句话不出现。
  //
  // **有组在推迟时也必须走**：推迟那一格显示的是「还有 14m」这种倒计时，它也是
  // 算出来的；更要紧的是牌在到点那一刻要自己变回满强度 —— 后端到点不推帧
  // （那对后端不是一次变化），没有这个钟，一块被推迟的牌会永远停在淡的那一档。
  // 判据用 `> 0` 而不是和 `now` 比：`now` 正是这里要产出的东西，不能反过来依赖它。
  //
  // 推迟这一档**不受 `expanded` 门控**：折叠形态里那颗还在闪的红点比什么都催人，
  // 收起来之后同样要变暗、同样要到点自己亮回来。别的两种（已持续 / 刚完成）只有
  // 展开态才画，所以它们仍在 `expanded` 里面。
  //
  // **必须在下面那个跃迁 effect 之前求值**：那一帧要拿 `now` 判断「推迟是不是
  // 刚好到点了」，而到点这件事只体现在 `now` 上。
  const snoozeTick = groups.some((group) => group.snooze_until_ms > 0);
  const ticking =
    snoozeTick ||
    (expanded && (show.age || (groups.length === 0 && lastDoneMs > 0)));
  const now = useNow(ticking);

  // 跃迁脉冲：哪一块牌刚**换了状态**就闪一下。判据在 `lib/lampTransitions.ts`
  // （纯函数、有单测）：拿上一帧的每组签名和这一帧比。**首帧不闪** —— ref 初值
  // 是空表，第一帧没有任何组「变了」。
  //
  // 签名里含「是否在推迟」，所以**到点解除推迟那一下也会闪**（牌从淡的一档回到
  // 满强度）。那正是用户说「过十分钟再说」之后要的提示，而推迟刻意不发通知。
  // 依赖里有 `now`：解除推迟不改 `groups`（后端到点不推帧），只有钟在走。
  const prevAggregates = useRef<SignSignature>({});
  const [pulses, setPulses] = useState<string[]>([]);
  const pulseTimer = useRef<number | null>(null);

  useEffect(() => {
    const { changed, next } = transitions(
      prevAggregates.current,
      groups,
      (group) => group.snooze_until_ms > now,
    );
    prevAggregates.current = next;
    if (changed.length === 0) return;

    setPulses(changed);
    if (pulseTimer.current !== null) window.clearTimeout(pulseTimer.current);
    // 到时把 data-pulse 摘掉：一次性动画挂着不放，同一组再变时就不会重新触发
    pulseTimer.current = window.setTimeout(() => {
      pulseTimer.current = null;
      setPulses([]);
    }, PULSE_MS);
    // **没有 cleanup**：这条 effect 只是把「谁刚变了」记下来，摘掉脉冲由上面那个
    // 超时负责。在这里返回清理函数的话，下一帧一进来就把上一个计时器清掉、脉冲
    // 永远摘不掉了。
  }, [groups, now]);

  // 卸载时兜一下，别让计时器在组件没了之后还回调 setState
  useEffect(
    () => () => {
      if (pulseTimer.current !== null) window.clearTimeout(pulseTimer.current);
    },
    [],
  );

  // 这枚角标现在**只表示暂停**。穿透状态搬到角灯里之后，它不再画 `ban`：
  // 一是用户已经在「外观 → 四角」里决定要不要看它，二是两种全局状态抢同一枚
  // 12px 方块，穿透一开就把「暂停」盖掉，反而少了一条信息。
  const flag = !preview && paused;
  const flagTitle = !preview && paused ? m.lampPaused : "";

  /**
   * 一组都没有时，占位牌上那一句话。
   *
   * `off`（暂停）不会走到这里 —— 那时聚合是 `off` 而不是 `offline`，而暂停有它
   * 自己更该说的话（「已暂停监控」）。
   */
  const emptyLabel =
    (aggregate === "offline" && justFinishedLabel(lang, lastDoneMs, now)) ||
    signalLabel(lang, aggregate);

  // 第一次打开、一组都没有时，占位牌自己就是引导入口。真有组上来（或用户点掉）
  // 就自动收起 —— 引导不该赖在已经用起来的灯上。折叠成一颗点时没有牌面可放字，
  // 也就不显示（等用户展开再给）。
  const guide = useFirstRunGuide(
    !preview && !paused && expanded && groups.length === 0,
  );

  const openDemo = () => {
    guide.dismiss();
    // 「打开设置并翻到演示页」的纸条（见 `lib/settingsIntent.ts`）—— 设置窗的组件
    // 启动时就挂载好了，翻页只能靠这张纸。
    requestSettingsTab("demo");
    if (inTauri) {
      invoke("open_settings").catch(console.error);
    } else {
      // 浏览器里没有设置窗，直接去独立演示页 —— 也是录 README 动图走的那条路
      window.location.href = "?window=demo";
    }
  };

  const toggleCollapsed = () => {
    if (!inTauri || preview) return;
    // 窗口尺寸由 Rust 算（它才知道可见牌的高度和设置），前端只负责「点了一下」。
    // 形态存在设置里，所以下次启动还是用户上次选的那个。
    invoke("toggle_collapsed").catch(console.error);
  };

  const openInbox = () => {
    if (!inTauri || preview) return;
    invoke("toggle_inbox").catch(console.error);
  };

  const openSettings = () => {
    if (!inTauri || preview) return;
    invoke("open_settings").catch(console.error);
  };

  /** 暂停 / 恢复监控。走 `set_paused` 而不是 `toggle_monitor`：后者是给托盘和
   *  右键菜单用的「我不知道现在什么状态」，而这里 `paused` 就在手边。 */
  const togglePause = () => {
    if (!inTauri || preview) return;
    invoke("set_paused", { paused: !paused }).catch(console.error);
  };

  const quitApp = () => {
    if (!inTauri || preview) return;
    invoke("quit").catch(console.error);
  };

  // 圆环菜单开着没有。它**不是设置**（不落盘），关掉就缩回原尺寸。
  //
  // 菜单开着时窗口是正方形（Rust 的 `config::menu_window_size`），前端这一帧
  // 只画圆环 + 中心那颗灯珠 —— 牌、气泡、角标全都不画，所以下面所有渲染分支
  // 都走不到。
  const [menuOpen, setMenuOpen] = useState(false);

  // 告诉 Rust 菜单开了 / 关了。它据此把窗口算成正方形或者缩回去。
  // 演示里绝对不能推：它会去改**真灯窗**的尺寸（同 `set_peek`）。
  useEffect(() => {
    if (!inTauri || preview) return;
    invoke("set_lamp_menu", { open: menuOpen }).catch(console.error);
  }, [menuOpen, preview]);

  // 折叠态 hover 临时展开和圆环菜单是两件互斥的事：菜单一开就把 peek 收掉，
  // 否则关掉菜单时窗口会先按「展开态」算一次，白闪一下。
  useEffect(() => {
    if (menuOpen) setPeek(false);
  }, [menuOpen]);

  // hook 无条件调用（React 规则），预览时只是不把那组事件挂到根节点上。
  // **长按 = 圆环菜单**始终不变；单击做什么由设置决定（默认折叠 / 展开）。
  // 判据仍在 `useDragOrClick` 那一条时间线上：拖拽和长按优先，单击不抢它们。
  const onClickLamp = () => {
    if (settings.click_action === "inbox") openInbox();
    else if (settings.click_action === "collapse") toggleCollapsed();
    // "none"：按下 / 松开照常判定，只是不产生动作
  };
  const drag = useDragOrClick(onClickLamp, () => setMenuOpen(true));

  // 牌上原来那枚「清除该组」的 × 已撤（见渲染里 `sign` 上方的说明），这条
  // `invoke("clear_group_state")` 的前端入口也就没有调用方了 —— 清除只剩右键菜单
  // 一条路，走的是 Rust 的原生菜单项 `menu.rs::CLEAR`，不需要前端再包一层。

  // 服务端已按**紧急度**排好，这里只叠一层用户的置顶偏好。规则必须和 Rust 侧的
  // `visible_groups` 逐字一致（窗口高度按「哪几块可见」算，两边算出不同的组就会
  // 裁掉最后一块牌）—— 所以它住在 `lib/lampSort.ts` 里、有单测盯着，见那边的说明。
  const visible = orderVisible(groups, hidden_groups, settings.group_meta);

  const shown = visible.slice(0, MAX_SIGNS);
  // 被 `+N` 收起来的组。**必须按同样的规则算** —— 牌数和窗口尺寸都依赖「哪几块可见」，
  // 两边算出不同的组就会裁掉最后一块牌。工具提示里要把这些组列出来。
  const hidden = visible.slice(shown.length);

  // 一组都看不见时也留一块，否则窗口会塌成一条缝 —— 那样既看不见状态，也没法拖它。
  // 名字留空：它不对应任何一个真实的组，右键菜单对它没有意义。
  const rows: GroupView[] =
    shown.length > 0
      ? shown
      : [
          {
            name: "",
            // 占位那块没有对应任何真实分组，自然也没有默认显示名
            default_label: null,
            aggregate,
            message: null,
            since_ms: 0,
            stale_progress: null,
            // 占位牌不对应任何组，也就没有「推迟」可言
            snooze_until_ms: 0,
            sessions: [],
          },
        ];

  /** 预览里右键只吃掉原生菜单，不弹真灯那张（它会改这台机器的状态）。 */
  const handleGroupMenu = (event: React.MouseEvent, group: string, why: string[] = []) => {
    if (preview) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    openGroupMenu(event, group, why);
  };

  // 圆环菜单形态：窗口已经被 Rust 换成正方形，这一帧**只画圆环**。
  // 牌 / 气泡 / 角标一概不画 —— 它们的几何是给另一个尺寸的窗口算的。
  if (menuOpen) {
    return (
      <div
        className="lamp-root lamp-root-menu"
        // `--size` 也得带过来：中心那颗灯珠是按 `--size` 算直径的，
        // 不带的话用户把灯调大之后，菜单里的灯珠还停在默认大小。
        style={{ "--size": `${size}px`, ...lampVars(settings) } as React.CSSProperties}
        data-aggregate={aggregate}
        data-effect={effect}
        // 菜单形态没有「某一块牌」可右键，但 webview 自己的右键菜单（dev 下是
        // 「重新加载」那种）绝不能冒出来 —— 这个窗口里它一直是关掉的。
        onContextMenu={(event) => event.preventDefault()}
      >
        <LampMenu
          aggregate={aggregate}
          onClose={() => setMenuOpen(false)}
          items={[
            // 顺时针从上开始：上 / 右 / 下 / 左。
            // 收件箱放最上面 —— 它原来是左键单击的主动作，改到折叠之后
            // 得给它一个一眼能找到的位置。
            { id: "inbox", icon: "▤", label: m.lampMenuInbox, onSelect: openInbox },
            { id: "settings", icon: "⚙", label: m.lampMenuSettings, onSelect: openSettings },
            {
              id: "pause",
              icon: paused ? "▶" : "‖",
              label: paused ? m.lampMenuResume : m.lampMenuPause,
              onSelect: togglePause,
            },
            { id: "quit", icon: "✕", label: m.lampMenuQuit, onSelect: quitApp, danger: true },
          ]}
        />
      </div>
    );
  }

  // 边框光「跟随安静模式」那一档要判的是**此刻整盏灯有没有被压暗**，不是开关
  // 本身。CSS 的 `[data-quiet][data-aggregate]` 压暗的是 idle / stale / offline /
  // off 四种聚合（done 刻意不在内，它值得看一眼）—— 判据必须和那边一致，否则
  // 会看到「牌压暗了、边框光还亮着」这种自相矛盾的画面。
  const quietDim =
    quiet &&
    !passThrough &&
    !guide.show &&
    ["idle", "stale", "offline", "off"].includes(aggregate);

  /**
   * 展开态的每一行 / 折叠态的每一格。**同一份数组供三种形态用**：
   * 展开时外面套 `.sign-board`（共享外框、圆角、行间细线）；折叠且「只有灯」
   * 时直接是 `.lamp-root` 的一列 / 一排裸点；折叠且带文字时套 `.fold-board`
   * 变成一条窄板，每行仍是「同一颗灯珠 + 一段词」。
   * 三者的格子数都由 `rows.length + (hidden.length > 0 ? 1 : 0)` 决定，
   * 和 Rust `Settings::folded_size` 的 dots + badges 是同一个数。
   */
  const cards = rows.map((group, index) => {
        // B4 预设 3：会议中那一组画成蓝色。`--state` 是圆点 / 圆环颜色的唯一入口，
    // 覆盖它之后 `--edge`（描边）也跟着派生 —— 见 state.css 的 `[data-state]`。
    // 折叠态同样上色：收起来时那一排点正是「开会」最该被看见的地方。
    const tint = meetingTint(group, settings.rule_meeting);
    // 推迟：用户对**这一组**按了「推迟 15 分钟」（见 Rust `state::Store::snooze`）。
    // 判据是绝对时刻和自己的钟，不是后端推来的布尔值 —— 到点时后端不会推帧
    // （那对后端不是一次变化），牌重新变亮全靠这一行。
    //
    // 它只影响**这一行怎么画**：聚合、排序、通知、转发全都不看它。
    const snoozed = group.snooze_until_ms > now;
    // 这一块牌的呼吸相位（见 `lampStyle.ts::rhythmDelay`）。灯珠、柔光、边框光
    // 三处共用一个变量，所以它们永远同相 —— 它们本来就在说同一句话。
    const rhythm = rhythmDelay(settings, group.name, group.aggregate);
    const dotStyle = {
      ...progressStyle(group),
      ...(tint ? { "--state": tint } : {}),
      // 灵动效果：把该状态的柔光呼吸 animation 挂到灯珠上，CSS 的
      // `.dot::before { animation: var(--halo-anim, none) }` 读它。
      ...(haloAnims ? { "--halo-anim": haloAnims[group.aggregate] } : {}),
      // 同屏多块牌时错开呼吸相位（只有「在干活」两种状态带非零值）。
      // **按组名算**，所以同一块牌每次重渲染、每次重启都停在自己那个相位上。
      "--rhythm-delay": rhythm,
    } as React.CSSProperties;
    // 效果不是「关闭」、也没开「减少动态效果」时，灯珠用 motion 弹簧入场。
    // spring 比 ease-out 更「灵动」——有一点过冲，像灯珠落到了位。
    // 只动 opacity 和 scale：**不改盒模型**，scale 走 transform 合成层，
    // 不触发重排（红线 2 只限制「常驻循环动画」，一次性弹簧不在此列）。
    const dotMotion = motionOn
      ? {
          initial: { opacity: 0, scale: 0.6 },
          animate: { opacity: 1, scale: 1 },
          transition: { type: "spring" as const, stiffness: 300, damping: 18, duration: 0.42 },
        }
      : {};
    // 六个灯位共用**这一份**逐块牌事实。判据在 `lampStyle.ts::lightOn`
    // （纯函数、有单测）；每个槽各自拿自己的 trigger 去问，所以上边常亮、
    // 下边只在出错时亮、四个角各挂各的是同一帧里判出来的，不会出现
    // 「轨亮着、四枚灯却熄了」这种半套画面。
    // 九档里带参数的档位（指定状态 / 指定时段 / 沉默将尽、有消息、仅置顶）
    // 都从这份 ctx 取值，六个槽共用一份参数。
    const pinned = settings.group_meta[group.name]?.order != null;
    const lightCtx: LightContext = {
      state: group.aggregate,
      quiet: quietDim,
      matchState: settings.sign_trigger_state,
      from: settings.sign_trigger_from,
      to: settings.sign_trigger_to,
      staleProgress: group.stale_progress,
      message: group.message,
      pinned,
    };
    const edges = edgeLightKinds(settings.sign_lights, lightCtx);
    // 闪不闪和「画成什么样」是两件事，但都按**这块牌自己的**状态节奏走：
    // 同一盏灯里可以有牌在闪、有牌常亮。见 `lampStyle.ts::lightBlink`。
    const edgeBlink = edgeBlinks(settings.sign_lights, lightCtx);

    // 行尾四格：哪几枚该亮。判据在 `lampStyle.ts::cornerLights`（纯函数、有单测）
    // —— 它只返回**该亮**的那些，「不亮」是根本不画，而不是画一个暗点。
    // 每个角先过自己的 trigger，再过这枚灯自己的事实：说「现在别亮」时连
    // 无条件亮的 `state` 那枚也得消失。0.6.5 之前四角共用一个 gate。
    //
    // **占位牌（`name` 为空）不挂**：它不对应任何真实的组，也就没有「置顶 /
    // 静音 / 沉默多久」可言 —— 挂上去的每一枚都会是假的。
    const corners =
      group.name === ""
        ? []
        : cornerLights(
            settings.sign_lights,
            {
              pinned,
              muted: settings.muted_groups.includes(group.name),
              staleProgress: group.stale_progress,
              // 全局事实：开着穿透时每块牌都挂这一枚（如果那一角配了它）。
              passThrough,
            },
            lightCtx,
          );
    // 折叠内容的四档只改「灯珠旁边那一个词」，灯珠本身一模一样。
    // `name` 走和展开牌同一个 `labelOf`（自定义显示名 / 模板都在里面）；占位牌
    // 没有真实组名，就不写字。`index` 是这一帧可见行从 1 开始的序号。
    const foldText =
      foldLabel === "name"
        ? group.name
          ? labelOf(group, settings.group_meta, labelPref)
          : ""
        : foldLabel === "index"
          ? String(index + 1)
          : foldLabel === "state"
            ? signalLabel(lang, group.aggregate)
            : "";
    // 右键：菜单最上面那两行「为什么它在亮」**在按下那一刻**才算 —— 纯展示，
    // 没必要每次状态刷新都为每一行拼两段字符串。判据与点亮六格共用同一份
    // `lightCtx`（见 `lib/lampWhy.ts`）。
    const dotMenu = (event: React.MouseEvent) =>
      handleGroupMenu(
        event,
        group.name,
        whyLines({ group, settings, ctx: lightCtx, lang, m, now, passThrough }),
      );
    // key 用下标：折叠后组名可能为空（占位那块），不能用名字当 key。
    // 右键菜单在裸点档挂在点上，在窄板档挂在整行上 —— 后者点文字也该弹出组菜单。
    const dot = (
      <motion.span
        key={index}
        className="dot lamp-dot"
        data-state={group.aggregate}
        data-shape={shapes[group.aggregate]}
        /* 推迟期间：同形状、同颜色，只是不再闪、暗一档（见 lamp.css）。
           **形状和颜色都不改** —— 改了就变成「另一个状态」了，而它仍然在等你批准。 */
        data-snoozed={snoozed || undefined}
        style={dotStyle}
        {...dotMotion}
        onContextMenu={foldLabel === "none" ? dotMenu : undefined}
      />
    );

    return !expanded ? (
      foldLabel === "none" ? (
        dot
      ) : (
        <div
          key={index}
          className="fold-row"
          data-label={foldLabel}
          /* 折叠形态也要让人看出「这一组被推迟了」：收起来之后牌上只剩一颗点，
             一颗还在闪的红点比什么都催人。点变暗、词变灰，和展开态同一套。 */
          data-snoozed={snoozed || undefined}
          /* 折叠格宽只留「够认出这一组」的几个字（见 `FOLD_WIDTH_MULT`），
             长组名会被省略号截断。原生 title 让用户 hover 时还能读到全名 ——
             不然「折叠省了宽度」就变成「名字再也看不全」了。 */
          title={foldText || undefined}
          onContextMenu={dotMenu}
        >
          {dot}
          <span className="fold-text">{foldText}</span>
        </div>
      )
    ) : (
      <div
        key={group.name || index}
        className="sign"
        data-has-message={show.message && group.message ? "true" : "false"}
        /* 视觉分量：`need` = 正在等你动手，`calm` = 只是现在怎么样。
           判据在 `lampStyle.ts::toneOf`（有单测），CSS 按它给点径 / 牌底 /
           字重 —— 三样都不参与布局，牌宽高一个像素都不变。
           **只有一块牌时不分层**（`shown.length > 1`）：分层的用途是在多块
           之间分先后，只有一块时压淡只是让整盏灯退一档、没有换来任何信息。
           而 `group_by` 默认是 `none` —— 所有上报归一组，也就是**一块牌**，
           这正是最常见的用法，不能让它无缘无故变淡。
           **占位牌也不参与**（`name` 为空 = 一个真实的组都没有）：它不是
           「某一块组牌」，是整盏灯的自述，那句「没有智能体在上报 / 刚完成 ·
           N 分钟前」正是回来时最该读的一句话。 */
        data-tone={
          shown.length > 1 && group.name !== ""
            ? toneOf(group.aggregate)
            : undefined
        }
        /* 推迟：这一行暂时不再催。CSS 只做三件事 —— 套上「退后一档」那套
           （点径 / 薄雾 / 灰字）、停掉闪烁、把「已持续」换成倒计时。
           **状态色、形状、消息一个字都不改**：它仍然在等你批准，只是你说过
           「先放一边」。见 Rust `state::Store::snooze` 的注释。 */
        data-snoozed={snoozed || undefined}
        /* 给 `.sign` 挂 data-state，是为了让 state.css 的 `[data-state] { --state }`
           在**牌**这一层也生效 —— 跃迁时描边闪的是这个状态的颜色（见 lamp.css
           的 `edge-flash`）。刻线和四格记号都从这个 `--state` 取色，所以它必须
           挂在牌上，而不是只挂在灯珠上。 */
        data-state={group.aggregate}
        /* 上下两条边**逐块牌、逐条**判定：同一盏灯里可以有的牌亮边、有的不亮，
           同一块牌也可以只亮上边。`off` 时仍然挂属性（而不是不挂），CSS 的
           「中性描边让位」那条 `:not([data-edge-top="off"])` 才不会误伤
           没开边灯的牌。 */
        data-edge-top={edges.top}
        data-edge-bottom={edges.bottom}
        /* 要闪的边 / 角在牌上只挂一个存在性的属性：动画本身是一条 `--light-blink`
           变量（见下面的 `style`），CSS 只负责「挂了才播」。所以关掉闪烁时
           连一条动画声明都不存在，而不是播一条 0 时长的空动画。 */
        data-blink-top={edgeBlink.top || undefined}
        data-blink-bottom={edgeBlink.bottom || undefined}
        /* `--progress` 也挂到牌上：沉默进度那枚角灯要用它，而它原来只挂在
           灯珠的 style 里。同一个变量、同一个来源，两者不会给出不同的答案。
           `--light-blink` 同理挂在牌上：边的两个伪元素和四枚角灯都从这一层
           继承，于是「这一块牌什么节奏」只有一处答案。 */
        style={
          {
            ...progressStyle(group),
            "--light-blink": lightBlink(settings, group.aggregate),
            // 行沿刻线与行尾四格的闪烁也吃这个相位（见 `.sign` 里那条规则）。
            "--rhythm-delay": rhythm,
          } as React.CSSProperties
        }
        // 刚换了状态就闪一下（`pulses` 是这一帧跃迁的组名）。折叠态的点不带
        // 这个 —— 点的状态动画本身就会重放，重复闪反而糊。
        data-pulse={pulses.includes(group.name) || undefined}
        onContextMenu={dotMenu}
      >
        {/* 圆点也可以关掉（「牌上显示」里的第五个 chip）：有人只想要一块
            纯文字卡。关掉后文字自然左移，牌宽高一个像素都不变 —— 点本来
            就在 flex 行里，不参与 `sign_width` / `sign_height`。 */}
        {show.dot && (
          <motion.span
            className="dot"
            data-state={group.aggregate}
            data-shape={shapes[group.aggregate]}
            style={dotStyle}
            {...dotMotion}
          />
        )}
        <div className="sign-body">
          <div className="sign-line">
            {show.name && group.name && (
              /* 组名**只是一段文本**：改名的入口在设置面板（分组 → 各组的显示），
                 也可以从牌的右键菜单「重命名…」直接跳过去。
                 早先点它就能就地改，但那是一次「点一下就进入编辑状态」的交互 ——
                 灯是常驻在桌面上的，误点一下就开始编辑，比改名的便利更碍事。 */
              <span className="sign-name">{labelOf(group, settings.group_meta, labelPref)}</span>
            )}
            {guide.show && group.name === "" ? (
              /* 第一次用：占位牌自己变成入口。**仍然只占一行**（按钮是
                 flex:none 的小 chip），牌高和 Rust 算出来的一行牌完全一致，
                 不会被 overflow:hidden 裁掉。 */
              <>
                <span className="sign-state">{m.lampFirstRun}</span>
                <button
                  type="button"
                  className="sign-guide"
                  title={m.lampFirstRunDemoHint}
                  onMouseDown={stopMouse}
                  onMouseUp={stopMouse}
                  onClick={(event) => {
                    stopMouse(event);
                    openDemo();
                  }}
                >
                  {m.lampFirstRunDemo}
                </button>
              </>
            ) : (
              show.state && (
                /* 占位牌（`name` 为空，见上面 `rows` 的构造）说 `emptyLabel`：
                   一组都没有时它可能是「刚完成 · N 分钟前」。它没有组名要让位，
                   所以用长句。
                   真组用**两字短词**（`shortSignal`，中文「需要授权」→「授权」、
                   「出错 / 阻塞」→「出错」）。牌宽是固定的 154px（22px 档），
                   长词光状态就吃掉 40–53px，组名常被截成五六个字符；短词把
                   省下的宽度还给组名。也不能干脆不显示状态词 —— `working` 和
                   `done` 默认同色，只靠「慢闪 / 常亮」区分，静态一瞥分不出来。 */
                <span className="sign-state">
                  {group.name === "" ? emptyLabel : m.shortSignal[group.aggregate]}
                </span>
              )
            )}
          </div>
          {show.message && group.message && (
            <div className="sign-message">{group.message}</div>
          )}
        </div>
        {/* 「已持续」那一格。推迟期间**同一个位置换一句话**：`45s` 变成
            「14m 后」—— 说的是「还有多久它才会重新开始催」。位置、字号、宽度
            都不用改，所以牌宽一个像素不动（这点很重要：牌宽是 Rust 算的）。
            推迟时这一格即使「已持续」关着也显示 —— 用户刚按下的动作必须有回应，
            否则他会以为没生效。 */}
        {(show.age || snoozed) && (
          <span className="sign-age">
            {snoozed
              ? m.snoozeLeft(formatRemaining(group.snooze_until_ms, now))
              : formatAge(group.since_ms, now)}
          </span>
        )}
        {/* **牌上不再有任何按钮**（2026-11-10）。原来这里挂着一枚悬停出现的
            ×（「清除该组的状态」），撤掉它有两条理由：
            1. 常驻窗口上的一枚 × 读起来就是「关掉这块牌」，而它做的事只是清一个
               状态 —— 文案得靠 tooltip 纠正，那说明形状本身说错了话；
            2. 它所在的右上角正是当年 `tr` 那枚角灯的落点（0.6.7 起挪到行尾
               第 2 格），一悬停就盖住一盏读者刚配好的灯。
            清除没丢：牌上右键 → 「清除」走同一条 `clear_group`（原生菜单
            `menu.rs::CLEAR`）。首启引导那枚「不再提示」也一并撤了 —— 引导本来
            「真有组上报就自动收起」，加上行内的「看演示」按钮，退出路径够用，
            不值得为它在牌角留一颗常驻的 ×。 */}
        {hidden.length > 0 && index === rows.length - 1 && (
          <span className="sign-more" title={moreGroupsTip(m, hidden, settings.group_meta, labelPref)}>
            +{hidden.length}
          </span>
        )}
        {/* 行尾四格：**绝对定位、不参与 flex**，所以行宽高一个像素都不变。
            排在最后是为了让它们压在行底之上 —— 它们本来就画在右内边距里，
            不会盖住字；这里只是保证「画在行上」而不是「被行挡住」。 */}
        {corners.map((corner) => (
          <span
            key={corner.pos}
            className="sign-corner"
            data-pos={corner.pos}
            data-kind={corner.kind}
            // 只有这一枚自己开了闪烁才挂；值不存在 = 常亮（CSS 靠存在性选）。
            data-blink={corner.blink || undefined}
          />
        ))}
      </div>
    );
  });

  return (
    <div
      className="lamp-root"
      style={
        {
          "--size": `${size}px`,
          // 气泡占多高（见 lamp.css）：条数就是下面那个 `bubbles`，而 Rust 的
          // `approval_area` 按**同一个数**给窗口留高度 —— 三个地方一个数。
          "--approvals": bubbles,
          ...lampVars(settings),
        } as React.CSSProperties
      }
      data-aggregate={aggregate}
      data-quiet={quiet && !passThrough && !guide.show}
      /* 质感五档。挂属性而不是内联变量 —— 和上面 `data-*` 同一套做法，
         倍数仍然只有 `lamp.css` 一个出处（见那边「质感」一节）。
         **菜单形态那棵树不挂**：它只画圆环，一块牌都没有。 */
      data-sign-radius={settings.sign_radius}
      data-sign-border={settings.sign_border}
      data-sign-tint={settings.sign_tint}
      data-sign-text={settings.sign_text}
      data-sign-emphasis={settings.sign_emphasis}
      /* 字体也走属性：整块牌换字形，倍率 / 字体栈都只住在 `lamp.css` 一处。
         间距（`--sign-gap`）不在这里 —— 它影响窗口几何，得和 Rust 的
         `window_size` 用同一个倍率，所以和宽高倍率一起走 `lampVars`。 */
      data-sign-font={settings.sign_font}
      // 整块牌的轮廓。四档都只改 border-radius，不改牌宽高。
      data-sign-outline={settings.sign_outline}
      /* 边框光的三把尺子（粗细 / 明暗 / 颜色来源）。和上面几个 `data-sign-*`
         同一个道理 —— 倍数与取色只住在 `lamp.css` 一处。

         **六个灯位的画法不挂在这里**：每个槽的触发条件是逐行判定的（只在
         要你处理时亮 / 只在出错时亮），所以上下两枚刻线由每行 `.sign` 自己带
         `data-edge-top` / `data-edge-bottom`，行尾四格则只画该亮的那些。
         根上挂一个全局值会让整盏灯一起亮灭，等于没有触发开关。 */
      data-sign-light-scale={settings.sign_light_scale}
      data-sign-light-alpha={settings.sign_light_alpha}
      data-sign-light-color={settings.sign_light_color}
      data-paused={(!preview && paused) || undefined}
      // 有气泡时窗口要高出一块（Rust 算的），CSS 按同一个数把内容区收上去
      data-approvals={pending || undefined}
      data-pass-through={passThrough || undefined}
      data-collapsed={!expanded}
      /* 折叠轴与折叠内容：CSS 按它们决定 flex 方向、要不要画窄板、每格多宽。
         展开态挂了也不会命中任何规则（所有相关选择器都带 `[data-collapsed]`）。 */
      data-fold-axis={foldAxis}
      data-fold-label={foldLabel}
      data-flash={flash || undefined}
      // 五档动效。旧版 `[data-lively]` 那批规则现在归 `[data-effect="breathe"]`，
      // 其余四档各有一套；系统「减少动态效果」时 CSS 把五档一律降级成静止。
      data-effect={effect}
      title={
        preview
          ? undefined
          : tooltip(m, lang, aggregate, groups, settings.group_meta, labelPref, [
              paused ? m.lampPaused : "",
              passThrough ? m.lampPassThrough : "",
            ].filter((line) => line !== ""), emptyLabel, m.lampHint[settings.click_action])
      }
      // 折叠态被指着时临时展开（见上），移开 / 超时收回。**进入带延迟**：
      // 路过不该把窗口撑开；移开不延迟，立刻收回去。
      onMouseEnter={armPeek}
      onMouseLeave={disarmPeek}
      // 空白处（牌之间、留白）也要吃掉右键，否则会冒出 webview 自己的菜单；
      // 顺手弹的是灯自己那张（进设置 / 暂停 / 折叠 / 退出）。预览里只吃掉菜单。
      onContextMenu={preview ? (event) => event.preventDefault() : openLampMenu}
      {...(preview ? {} : drag)}
    >
      {expanded ? (
        <div className="sign-board" data-rows={rows.length}>
          {cards}
        </div>
      ) : foldLabel === "none" ? (
        cards
      ) : (
        /* 带文字时 `+N` 也占一格：它本来就是「还有几组」，不该飘在板外。
           Rust 的 folded_size 把它算进 badges，两边必须同数。 */
        <div
          className="fold-board"
          data-axis={foldAxis}
          data-label={foldLabel}
          data-rows={rows.length + (hidden.length > 0 ? 1 : 0)}
        >
          {cards}
          {hidden.length > 0 && (
            <div className="fold-row" data-label={foldLabel}>
              <span
                className="lamp-dot-more"
                title={moreGroupsTip(m, hidden, settings.group_meta, labelPref)}
              >
                +{hidden.length}
              </span>
              <span className="fold-text" />
            </div>
          )}
        </div>
      )}
      {/* 待审批的气泡。**绝对定位**（见 lamp.css）：不参与 flex 布局，所以折叠态
          那种横排也不用另写一套，牌的宽高一个像素都不变。 */}
      {pending > 0 && onDecide && (
        <ApprovalBubble approvals={approvals ?? []} onDecide={onDecide} />
      )}
      {/* 暂停角标。绝对定位在窗口右上角那一圈留白里（`top` / `right` = 1px，见
          CSS 的 `.lamp-flag`），既不吃牌身宽度，也不在折叠态占一个点距 ——
          所以 Rust 的窗口尺寸完全不用为它变化。
          用线性的 `Icon` 而不是 `⏸` 字符：那个字形在 9px 下只是一团墨。
          **穿透的 `ban` 角标 0.6.4 起不再画**：它成了「四角挂什么」里可选的一枚
          （`.sign-corner[data-kind="passthrough"]`），挂不挂由用户定；开启穿透时
          那条一次性通知、tooltip 的一行文字和托盘菜单仍然在。 */}
      {flag && (
        <span className="lamp-flag" data-state={aggregate} title={flagTitle}>
          <Icon name="pause" size={9} />
        </span>
      )}
      {/* 展开态那条同样的提示挂在最后一块牌上；折叠态看不到牌面，**不留这一枚
          就等于「还有几个智能体在跑」被悄悄抹掉了** —— 而那正是这盏灯要回答的。
          条件与 Rust 的 `config::more_badge` 是同一个。带文字的窄板档不在这里画：
          它作为板内的最后一格，`+N` 也要和别的行一样对齐。 */}
      {!expanded && foldLabel === "none" && hidden.length > 0 && (
        <span className="lamp-dot-more" title={moreGroupsTip(m, hidden, settings.group_meta, labelPref)}>
          +{hidden.length}
        </span>
      )}
    </div>
  );
}
