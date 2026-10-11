import { useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import { lampVars, resolveShapes } from "../lib/lampStyle";
import type { Settings } from "../lib/useSettings";
import type { GroupView, SignalState } from "../lib/types";

/** 与 Rust 侧 `config::MAX_SIGNS` 一致。超出的牌服务端已经截断。 */
const MAX_SIGNS = 4;
/** 超过这个位移就算拖窗口，否则算点击。 */
const DRAG_THRESHOLD_PX = 4;

interface Props {
  aggregate: SignalState;
  groups: GroupView[];
  /**
   * 整份设置都传进来，而不是拆成一堆 prop。
   * 外观（大小 / 形态 / 形状 / 安静 / 透明度 / 节奏）和配色都要用，拆开传只会越来越长。
   */
  settings: Settings;
}

/** 「已持续多久」。牌上那一段短文本，所以只给一个单位、不写秒。 */
function formatAge(sinceMs: number): string {
  if (!sinceMs) return "";
  const seconds = Math.max(0, Math.round((Date.now() - sinceMs) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}

function tooltip(aggregate: SignalState, groups: GroupView[]): string {
  if (groups.length === 0) return SIGNAL_LABEL[aggregate];

  const hint = "左键折叠 / 展开 · 右键单组菜单";
  if (groups.length === 1) {
    const only = groups[0];
    const head = `${only.name} · ${SIGNAL_LABEL[only.aggregate]}`;
    const detail = only.message ? ` — ${only.message}` : "";
    return `${head}${detail}\n${hint}`;
  }

  // 多组时把「哪个组在什么状态」列出来 —— 这是多块牌想回答的问题
  return [
    `${SIGNAL_LABEL[aggregate]}（${groups.length} 组）`,
    ...groups.map((group) => {
      const detail = group.message ? ` — ${group.message}` : "";
      return `${group.name} · ${SIGNAL_LABEL[group.aggregate]}${detail}`;
    }),
    hint,
  ].join("\n");
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
  // 占位那块牌的名字是空的，它不对应任何真实的组 —— 弹出来只会藏掉一个叫 ""
  // 的组，什么也不会发生，不如干脆不弹
  if (!group || !inTauri) return;
  invoke("open_group_menu", { group }).catch(console.error);
}

export function SignalLamp({ aggregate, groups, settings }: Props) {
  const { size, quiet, collapsed, show, sign_opacity, hidden_groups } = settings;

  // 形状是逐状态的（一个窗口里多块牌可以不一样），所以先整份算出来，再按每块牌
  // 的状态取。颜色和动画走 lampVars 的 CSS 变量，形状走元素属性 —— 形状不是
  // 「一个值」，没法塞进一个根变量里。
  const shapes = resolveShapes(settings);

  const toggle = () => {
    if (!inTauri) return;
    // 窗口尺寸由 Rust 算（它才知道可见牌的高度和设置），前端只负责「点了一下」。
    // 形态存在设置里，所以下次启动还是用户上次选的那个。
    invoke("toggle_collapsed").catch(console.error);
  };

  const drag = useDragOrClick(toggle);

  // 服务端已按优先级排好并截断，这里**不要再排** —— 窗口高度按「哪几块可见」算，
  // 两边必须算出同一批牌。隐藏掉的和截断规则两边也要一致（见 Rust 侧 visible_groups）。
  const visible = groups
    .filter((group) => !hidden_groups.includes(group.name))
    .slice(0, MAX_SIGNS);

  // 一组都看不见时也留一块，否则窗口会塌成一条缝 —— 那样既看不见状态，也没法拖它。
  // 名字留空：它不对应任何一个真实的组，右键菜单对它没有意义。
  const shown: GroupView[] =
    visible.length > 0
      ? visible
      : [{ name: "", aggregate, message: null, since_ms: 0, sessions: [] }];

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
      data-quiet={quiet}
      data-collapsed={collapsed}
      title={tooltip(aggregate, groups)}
      // 空白处（牌之间、留白）也要吃掉右键，否则会冒出 webview 自己的菜单
      onContextMenu={(event) => event.preventDefault()}
      {...drag}
    >
      {shown.map((group, index) =>
        collapsed ? (
          <span
            // 折叠后只有点，没有可读的文字，所以 key 用下标；组名可能为空（占位那块）
            key={index}
            className="dot lamp-dot"
            data-state={group.aggregate}
            data-shape={shapes[group.aggregate]}
            onContextMenu={(event) => openGroupMenu(event, group.name)}
          />
        ) : (
          <div
            key={group.name || index}
            className="sign"
            data-has-message={show.message && group.message ? "true" : "false"}
            onContextMenu={(event) => openGroupMenu(event, group.name)}
          >
            <span className="dot" data-state={group.aggregate} data-shape={shapes[group.aggregate]} />
            <div className="sign-body">
              <div className="sign-line">
                {show.name && group.name && (
                  <span className="sign-name">{group.name}</span>
                )}
                {show.state && (
                  <span className="sign-state">{SIGNAL_LABEL[group.aggregate]}</span>
                )}
              </div>
              {show.message && group.message && (
                <div className="sign-message">{group.message}</div>
              )}
            </div>
            {show.age && <span className="sign-age">{formatAge(group.since_ms)}</span>}
          </div>
        ),
      )}
    </div>
  );
}
