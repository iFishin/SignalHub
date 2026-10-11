import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import { colorVars } from "../lib/palettes";
import type { Settings } from "../lib/useSettings";
import type { GroupView, SessionView, SignalState } from "../lib/types";

/** 与 Rust 侧 `config::MAX_SIGNS` 一致。超出的牌服务端已经截断。 */
const MAX_SIGNS = 4;
/** 超过这个位移就算拖窗口，否则算点击。 */
const DRAG_THRESHOLD_PX = 4;

interface Props {
  aggregate: SignalState;
  groups: GroupView[];
  /**
   * 整份设置都传进来，而不是拆成一堆 prop。
   * 外观（大小 / 形状 / 安静）和配色都要用，拆开传只会越来越长。
   */
  settings: Settings;
}

/** 取路径最后一段作为项目名，比整条路径更适合做提示。 */
function projectName(session: SessionView): string {
  if (!session.cwd) return session.id;
  // 两种分隔符都认，这样 Windows 上写的路径在别处也解析得对
  const parts = session.cwd.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : session.id;
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

  if (groups.length === 1) {
    const only = groups[0];
    const head = `${only.name} · ${SIGNAL_LABEL[only.aggregate]}`;
    return only.message ? `${head} — ${only.message}` : head;
  }

  // 多组时把「哪个组在什么状态」列出来 —— 这是多块牌想回答的问题
  return [
    `${SIGNAL_LABEL[aggregate]}（${groups.length} 组）`,
    ...groups.map((group) => {
      const detail = group.message ? ` — ${group.message}` : "";
      return `${group.name} · ${SIGNAL_LABEL[group.aggregate]}${detail}`;
    }),
  ].join("\n");
}

/**
 * 同一个元素上既要能拖窗口、又要能响应点击。
 *
 * 不能用 `data-tauri-drag-region`：它让系统在 mousedown 时接管鼠标，之后的
 * click 事件就不可靠了 —— 于是「点一下展开」会时灵时不灵。改成自己判断：
 * 按下后位移超过阈值才算拖拽（此时才把拖拽交给系统），否则算点击。
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

export function SignalLamp({ aggregate, groups, settings }: Props) {
  const { size, quiet, shape } = settings;
  const [expanded, setExpanded] = useState(false);

  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    // 窗口尺寸由 Rust 算（它才知道可见牌的高度和设置），前端只负责「点了一下」
    if (inTauri) {
      invoke("set_lamp_expanded", { expanded: next }).catch(console.error);
    }
  };

  const drag = useDragOrClick(toggle);

  // 服务端已按优先级排好并截断，这里**不要再排** —— 窗口高度按「哪几块可见」算，
  // 两边必须算出同一批牌
  const signs = groups.slice(0, MAX_SIGNS);
  const shown: GroupView[] =
    signs.length > 0
      ? signs
      : [
          {
            name: SIGNAL_LABEL[aggregate],
            aggregate,
            message: null,
            since_ms: 0,
            sessions: [],
          },
        ];

  if (expanded) {
    const sessions = groups.flatMap((group) => group.sessions);
    const rows = sessions.length > 0 ? sessions : [];
    return (
      <div
        className="lamp-root lamp-panel-root"
        style={colorVars(settings)}
        data-quiet="false"
        title="点击收起"
        onClick={toggle}
      >
        <div className="lamp-panel">
          <div className="lamp-panel-head" data-state={aggregate}>
            <span className="lamp-head-dot" />
            <span className="lamp-panel-title">{SIGNAL_LABEL[aggregate]}</span>
            <span className="lamp-panel-count">
              {groups.length} 组 · {sessions.length} 个会话
            </span>
          </div>
          {rows.length === 0 ? (
            <p className="lamp-empty">暂无会话。</p>
          ) : (
            <ul className="lamp-rows">
              {rows.map((session) => (
                <li
                  key={session.id}
                  className="lamp-row"
                  data-state={session.state}
                >
                  <span className="lamp-row-dot" />
                  <div className="lamp-row-body">
                    <div className="lamp-row-line">
                      <span className="lamp-row-id">{projectName(session)}</span>
                      <span className="lamp-row-meta">
                        {SIGNAL_LABEL[session.state]}
                        {session.agent ? ` · ${session.agent}` : ""}
                      </span>
                    </div>
                    {session.message && (
                      <div className="lamp-row-message">{session.message}</div>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    );
  }

  return (
    <div
      className="lamp-root"
      style={{ "--size": `${size}px`, ...colorVars(settings) } as React.CSSProperties}
      data-aggregate={aggregate}
      data-quiet={quiet}
      data-shape={shape}
      title={tooltip(aggregate, groups)}
      {...drag}
    >
      {shown.map((group, index) => (
        <div
          key={group.name || index}
          className="sign"
          data-state={group.aggregate}
          data-has-message={group.message ? "true" : "false"}
        >
          <span className="dot" />
          <div className="sign-body">
            <div className="sign-line">
              <span className="sign-name">{group.name}</span>
              <span className="sign-state">{SIGNAL_LABEL[group.aggregate]}</span>
            </div>
            {group.message && <div className="sign-message">{group.message}</div>}
          </div>
          <span className="sign-age">{formatAge(group.since_ms)}</span>
        </div>
      ))}
    </div>
  );
}
