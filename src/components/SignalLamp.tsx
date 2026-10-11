import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import type { Palette, Shape } from "../lib/useSettings";
import type { SessionView, SignalState } from "../lib/types";

/** 与 Rust 侧 `config::MAX_DOTS` 一致。 */
const MAX_DOTS = 3;
/** 超过这个位移就算拖窗口，否则算点击。 */
const DRAG_THRESHOLD_PX = 4;

interface Props {
  aggregate: SignalState;
  sessions: SessionView[];
  /** 圆点直径（px）。由设置决定，Rust 侧据此同步调整窗口尺寸。 */
  size: number;
  /** 安静模式：没事时压到几乎看不见，悬停恢复。 */
  quiet: boolean;
  /** 形状 / 外壳 / 配色 —— 都通过根元素的 data 属性交给 CSS 处理。 */
  shape: Shape;
  bezel: boolean;
  palette: Palette;
}

/** 取路径最后一段作为项目名，比整条路径更适合做提示。 */
function projectName(session: SessionView): string {
  if (!session.cwd) return session.id;
  // 两种分隔符都认，这样 Windows 上写的路径在别处也解析得对
  const parts = session.cwd.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : session.id;
}

function tooltip(aggregate: SignalState, sessions: SessionView[]): string {
  if (sessions.length === 0) return SIGNAL_LABEL[aggregate];
  if (sessions.length === 1) {
    const only = sessions[0];
    return only.message
      ? `${SIGNAL_LABEL[only.state]} — ${only.message}`
      : SIGNAL_LABEL[only.state];
  }
  const lines = sessions.map((session) => {
    const detail = session.message ? ` — ${session.message}` : "";
    return `${projectName(session)} · ${SIGNAL_LABEL[session.state]}${detail}`;
  });
  return [`${SIGNAL_LABEL[aggregate]}（${sessions.length} 个会话）`, ...lines].join("\n");
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

export function SignalLamp({ aggregate, sessions, size, quiet, shape, bezel, palette }: Props) {
  const [expanded, setExpanded] = useState(false);

  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    // 窗口尺寸由 Rust 算（它才知道会话数和设置），前端只负责「点了一下」
    if (inTauri) {
      invoke("set_lamp_expanded", { expanded: next }).catch(console.error);
    }
  };

  const drag = useDragOrClick(toggle);

  // 会话列表已按优先级排序，所以截断后剩下的恰好是最紧急的那几个
  const visible = sessions.slice(0, MAX_DOTS);
  const dots: { key: string; state: SignalState; progress: number | null }[] =
    visible.length > 0
      ? visible.map((session) => ({
          key: session.id,
          state: session.state,
          progress: session.stale_progress,
        }))
      : [{ key: "none", state: aggregate, progress: null }];

  if (expanded) {
    // 展开时不再压暗 —— 用户正主动看着它
    const rows =
      sessions.length > 0
        ? sessions
        : [
            {
              id: "",
              state: aggregate,
              agent: null,
              cwd: null,
              message: null,
              age_ms: 0,
              stale_progress: null,
            } satisfies SessionView,
          ];

    return (
      <div
        className="lamp-root lamp-panel-root"
        data-aggregate={aggregate}
        data-quiet="false"
        data-shape={shape}
        data-bezel={bezel}
        data-palette={palette}
        title="点击收起"
        onClick={toggle}
      >
        <div className="lamp-panel">
          <div className="lamp-panel-head" data-state={aggregate}>
            <span className="lamp-head-dot" />
            <span className="lamp-panel-title">{SIGNAL_LABEL[aggregate]}</span>
            {sessions.length > 0 && (
              <span className="lamp-panel-count">{sessions.length} 个会话</span>
            )}
          </div>
          <ul className="lamp-rows">
            {rows.map((session) => {
              const name = projectName(session);
              return (
                <li key={session.id || "none"} className="lamp-row" data-state={session.state}>
                  <span className="lamp-row-dot" />
                  <div className="lamp-row-body">
                    <div className="lamp-row-line">
                      {name && <span className="lamp-row-id">{name}</span>}
                      <span className="lamp-row-meta">{SIGNAL_LABEL[session.state]}</span>
                    </div>
                    {session.message && (
                      <div className="lamp-row-message">{session.message}</div>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      </div>
    );
  }

  return (
    <div
      className="lamp-root"
      style={{ "--size": `${size}px` } as React.CSSProperties}
      data-aggregate={aggregate}
      data-quiet={quiet}
      data-shape={shape}
      data-bezel={bezel}
      data-palette={palette}
      title={tooltip(aggregate, sessions)}
      {...drag}
    >
      {dots.map((dot) => (
        <span
          key={dot.key}
          className="dot"
          data-state={dot.state}
          // stale 的细环按剩余宽限期缩短，值由服务端算好（前端不必复刻 TTL）
          style={
            dot.progress === null
              ? undefined
              : ({ "--progress": dot.progress } as React.CSSProperties)
          }
        />
      ))}
    </div>
  );
}
