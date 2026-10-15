import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { inTauri } from "../lib/env";
import { signalLabel } from "../lib/signals";
import { useLang, useMessages, type Messages, type Lang } from "../lib/i18n";
import { lampVars, resolveShapes } from "../lib/lampStyle";
import { transitions, type AggregateMap } from "../lib/lampTransitions";
import { labelOf, orderVisible } from "../lib/lampSort";
import { useNow } from "../lib/useNow";
import { useToggles } from "../lib/useToggles";
import type { GroupMeta, Settings } from "../lib/useSettings";
import type { GroupView, SignalState } from "../lib/types";

/** 与 Rust 侧 `config::MAX_SIGNS` 一致。超出的牌服务端已经截断。 */
const MAX_SIGNS = 4;
/** 超过这个位移就算拖窗口，否则算点击。 */
const DRAG_THRESHOLD_PX = 4;
/** 跃迁脉冲的时长（ms）。必须和 lamp.css 里 `pulse-in` 的一致。 */
const PULSE_MS = 420;
/** 折叠态悬停临时展开的时长（ms）。到点或鼠标移开就收回折叠态。 */
const PEEK_MS = 3000;

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
  extra: string[],
): string {
  const hint = m.lampHint;

  // 没有组时 tooltip 只剩一句状态。Hints 仍然拼上 —— 空灯球正需要「右键能进设置」。
  if (groups.length === 0) return [signalLabel(lang, aggregate), hint, ...extra].join("\n");

  if (groups.length === 1 && extra.length === 0) {
    const only = groups[0];
    const head = `${labelOf(only, meta)} · ${signalLabel(lang, only.aggregate)}`;
    const detail = only.message ? ` — ${only.message}` : "";
    return `${head}${detail}\n${hint}`;
  }

  // 多组时把「哪个组在什么状态」列出来 —— 这是多块牌想回答的问题。
  // 被 `+N` 收起来的组也列在这儿：牌上看不见，tooltip 是全窗口唯一能容纳它的地方，
  // 否则那块标记只说得出「有更多」，说不出是什么。
  const listed = groups.map((group) => {
    const detail = group.message ? ` — ${group.message}` : "";
    return `${labelOf(group, meta)} · ${signalLabel(lang, group.aggregate)}${detail}`;
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
): string {
  return [m.moreGroups(hidden.length), m.moreGroupsLead + hidden.map((g) => labelOf(g, meta)).join(", ")].join(
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
 * 同一个元素上既要能拖窗口、又要能响应点击。
 *
 * 不能用 `data-tauri-drag-region`：它让系统在 mousedown 时接管鼠标，之后的
 * click 事件就不可靠了 —— 于是「点一下折叠」会时灵时不灵。改成自己判断：
 * 按下后位移超过阈值才算拖拽（此时才把拖拽交给系统），否则算点击。
 *
 * 只认主键，所以右键原地就能弹菜单，不会顺带被当成一次点击。
 */
function useDragOrClick(onClick: () => void) {
  const origin = useRef<{ x: number; y: number } | null>(null);
  const dragging = useRef(false);

  return {
    onMouseDown: (event: React.MouseEvent) => {
      if (event.button !== 0) return; // 只认主键
      origin.current = { x: event.clientX, y: event.clientY };
      dragging.current = false;
    },
    onMouseMove: (event: React.MouseEvent) => {
      if (!origin.current || dragging.current) return;
      const moved = Math.hypot(
        event.clientX - origin.current.x,
        event.clientY - origin.current.y,
      );
      if (moved < DRAG_THRESHOLD_PX) return;
      dragging.current = true;
      if (inTauri) {
        getCurrentWindow().startDragging().catch(console.error);
      }
    },
    onMouseUp: () => {
      const wasClick = origin.current !== null && !dragging.current;
      origin.current = null;
      dragging.current = false;
      if (wasClick) onClick();
    },
  };
}

/**
 * 弹某一组的右键菜单。菜单本身是原生的，见 Rust 侧的 `menu.rs`。
 *
 * 无论有没有组名都要 `preventDefault` —— 否则占位那块牌上会冒出 webview 自己的
 * 右键菜单（dev 模式下是「重新加载」那种），和这个应用格格不入。
 */
function openGroupMenu(event: React.MouseEvent, group: string) {
  event.preventDefault();
  event.stopPropagation();
  if (!inTauri) return;
  // 占位那块牌的名字是空的，它不对应任何真实的组 —— 组菜单对它只会藏掉一个叫 ""
  // 的组，什么也不会发生。这时弹的是**灯自己那张菜单**：第一次装上、还没接任何
  // 上报方时，这是灯上唯一能进设置的地方。
  if (group) {
    invoke("open_group_menu", { group }).catch(console.error);
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

export function SignalLamp({ aggregate, groups, settings, paused }: Props) {
  const { size, quiet, collapsed, show, sign_opacity, hidden_groups } = settings;
  const m = useMessages();
  const lang = useLang();
  // 穿透也能从托盘和全局快捷键改，所以订阅而不是读一次 —— 灯上那枚角标必须跟着变。
  const toggles = useToggles();

  // 形状是逐状态的（一个窗口里多块牌可以不一样），所以先整份算出来，再按每块牌
  // 的状态取。颜色和动画走 lampVars 的 CSS 变量，shape走元素属性 —— 形状不是
  // 「一个值」，没法塞进一个根变量里。
  const shapes = resolveShapes(settings);

  // 折叠态悬停**临时**展开：只改渲染 + 让 Rust 把窗口算大，**不改设置** —— 退出
  // 重开仍是折叠态（这是这一项的验收之一）。3s 到点、或鼠标移开就回去。窗口尺寸
  // 由 Rust 按同一个标志算（命令 `set_peek`；多灯下只有被指着的那盏会变），所以
  // 前端和窗口不会打架。
  const [peek, setPeek] = useState(false);

  // 真的展开了（点一下 / 托盘）就把 peek 收掉，免得它残留着和设置打仗
  useEffect(() => {
    if (!collapsed) setPeek(false);
  }, [collapsed]);

  // 把「谁在被 hover」推给 Rust —— 它按这个把那个窗口算成展开态
  useEffect(() => {
    if (!inTauri) return;
    invoke("set_peek", { peek }).catch(console.error);
  }, [peek]);

  // 3s 到点自动收回。effect 挂在 `peek` 上：一直 hover 着也只数一次
  useEffect(() => {
    if (!peek) return;
    const timer = window.setTimeout(() => setPeek(false), PEEK_MS);
    return () => window.clearTimeout(timer);
  }, [peek]);

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

  // 跃迁脉冲：哪一块牌刚**换了状态**就闪一下。判据在 `lib/lampTransitions.ts`
  // （纯函数、有单测）：拿上一帧的每组聚合和这一帧比。**首帧不闪** —— ref 初值
  // 是空表，第一帧没有任何组「变了」。
  const prevAggregates = useRef<AggregateMap>({});
  const [pulses, setPulses] = useState<string[]>([]);
  const pulseTimer = useRef<number | null>(null);

  useEffect(() => {
    const { changed, next } = transitions(prevAggregates.current, groups);
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
  }, [groups]);

  // 卸载时兜一下，别让计时器在组件没了之后还回调 setState
  useEffect(
    () => () => {
      if (pulseTimer.current !== null) window.clearTimeout(pulseTimer.current);
    },
    [],
  );

  // 穿透优先于暂停：那个状态下窗口点不动、拖不动，用户得先知道自己在里面。
  const flag = paused || toggles.pass_through;
  const flagTitle = toggles.pass_through
    ? [m.lampPassThrough, toggles.pass_through_shortcut]
        .filter((line) => line !== "")
        .join("\n")
    : paused
      ? m.lampPaused
      : "";

  // 「已持续」是算出来的，所以得有东西让它随时间重绘 —— 否则它永远停在状态刚变化
  // 那一刻，也就是 0s。折叠形态和关掉这一格的牌上没有这个数字，不起定时器。
  const now = useNow(show.age && expanded);

  const toggle = () => {
    if (!inTauri) return;
    // 窗口尺寸由 Rust 算（它才知道可见牌的高度和设置），前端只负责「点了一下」。
    // 形态存在设置里，所以下次启动还是用户上次选的那个。
    invoke("toggle_collapsed").catch(console.error);
  };

  const drag = useDragOrClick(toggle);

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
            sessions: [],
          },
        ];

  return (
    <div
      className="lamp-root"
      style={
        {
          "--size": `${size}px`,
          "--sign-alpha": String(sign_opacity / 100),
          ...lampVars(settings),
        } as React.CSSProperties
      }
      data-aggregate={aggregate}
      data-quiet={quiet && !toggles.pass_through}
      data-paused={paused || undefined}
      data-pass-through={toggles.pass_through || undefined}
      data-collapsed={!expanded}
      data-flash={flash || undefined}
      title={tooltip(m, lang, aggregate, groups, settings.group_meta, [
        paused ? m.lampPaused : "",
        toggles.pass_through ? m.lampPassThrough : "",
      ].filter((line) => line !== ""))}
      // 折叠态被指着时临时展开（见上），移开 / 超时收回
      onMouseEnter={() => {
        if (collapsed) setPeek(true);
      }}
      onMouseLeave={() => setPeek(false)}
      // 空白处（牌之间、留白）也要吃掉右键，否则会冒出 webview 自己的菜单；
      // 顺手弹的是灯自己那张（进设置 / 暂停 / 折叠 / 退出）
      onContextMenu={openLampMenu}
      {...drag}
    >
      {rows.map((group, index) =>
        !expanded ? (
          <span
            // 折叠后只有点，没有可读的文字，所以 key 用下标；组名可能为空（占位那块）
            key={index}
            className="dot lamp-dot"
            data-state={group.aggregate}
            data-shape={shapes[group.aggregate]}
            style={progressStyle(group)}
            onContextMenu={(event) => openGroupMenu(event, group.name)}
          />
        ) : (
          <div
            key={group.name || index}
            className="sign"
            data-has-message={show.message && group.message ? "true" : "false"}
            // 刚换了状态就闪一下（`pulses` 是这一帧跃迁的组名）。折叠态的点不带
            // 这个 —— 点的状态动画本身就会重放，重复闪反而糊。
            data-pulse={pulses.includes(group.name) || undefined}
            onContextMenu={(event) => openGroupMenu(event, group.name)}
          >
            <span
              className="dot"
              data-state={group.aggregate}
              data-shape={shapes[group.aggregate]}
              style={progressStyle(group)}
            />
            <div className="sign-body">
              <div className="sign-line">
                {show.name && group.name && (
                  <span className="sign-name">{labelOf(group, settings.group_meta)}</span>
                )}
                {show.state && (
                  <span className="sign-state">{signalLabel(lang, group.aggregate)}</span>
                )}
              </div>
              {show.message && group.message && (
                <div className="sign-message">{group.message}</div>
              )}
            </div>
            {show.age && <span className="sign-age">{formatAge(group.since_ms, now)}</span>}
            {flag && index === rows.length - 1 && (
              <span
                className="sign-flag"
                // 挂上 data-state 才能从 state.css 拿到 `--state`（当前状态色）。
                // 不带这个属性的话描边落回 --sign-line 那圈中性灰，穿透时就不显眼了。
                data-state={aggregate}
                title={flagTitle}
              >
                {toggles.pass_through ? "⇥" : "⏸"}
              </span>
            )}
            {hidden.length > 0 && index === rows.length - 1 && (
              <span className="sign-more" title={moreGroupsTip(m, hidden, settings.group_meta)}>
                +{hidden.length}
              </span>
            )}
          </div>
        ),
      )}
      {/* 折叠态没有牌面可寄住，角标只能自己占一个点的位置（见 CSS 的
          .lamp-dot-flag / .lamp-dot-more），Rust 的 `collapsed_window_size`
          因此要按「点数 + 角标数」算宽 —— 少留一个点位那枚角标就被裁掉了。 */}
      {!expanded && flag && (
        <span className="lamp-dot-flag" data-state={aggregate} title={flagTitle}>
          {toggles.pass_through ? "⇥" : "⏸"}
        </span>
      )}
      {/* 展开态那条同样的提示挂在最后一块牌上；折叠态看不到牌面，**不留这一枚
          就等于「还有几个智能体在跑」被悄悄抹掉了** —— 而那正是这盏灯要回答的。
          条件与 Rust 的 `config::more_badge` 是同一个。 */}
      {!expanded && hidden.length > 0 && (
        <span className="lamp-dot-more" title={moreGroupsTip(m, hidden, settings.group_meta)}>
          +{hidden.length}
        </span>
      )}
    </div>
  );
}
