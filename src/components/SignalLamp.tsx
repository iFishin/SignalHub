import { SIGNAL_LABEL } from "../lib/signals";
import type { SessionView, SignalState } from "../lib/types";

/** 与 Rust 侧 `config::MAX_DOTS` 一致。 */
const MAX_DOTS = 3;

interface Props {
  aggregate: SignalState;
  sessions: SessionView[];
  /** 圆点直径（px）。由设置决定，Rust 侧据此同步调整窗口尺寸。 */
  size: number;
  /** 安静模式：没事时压到几乎看不见，悬停恢复。 */
  quiet: boolean;
}

/** 取路径最后一段作为项目名，比整条路径更适合做悬停提示。 */
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

  // 多会话时把「哪个项目在什么状态」列出来 —— 这是多个点想回答的问题
  const lines = sessions.map((session) => {
    const detail = session.message ? ` — ${session.message}` : "";
    return `${projectName(session)} · ${SIGNAL_LABEL[session.state]}${detail}`;
  });
  return [`${SIGNAL_LABEL[aggregate]}（${sessions.length} 个会话）`, ...lines].join("\n");
}

export function SignalLamp({ aggregate, sessions, size, quiet }: Props) {
  // 会话列表已按优先级排序，所以截断后剩下的恰好是最紧急的那几个
  const visible = sessions.slice(0, MAX_DOTS);

  // 一个会话都没有时也要显示一个点：那个点表示 offline（或暂停时的 off）
  const dots: { key: string; state: SignalState; progress: number | null }[] =
    visible.length > 0
      ? visible.map((session) => ({
          key: session.id,
          state: session.state,
          progress: session.stale_progress,
        }))
      : [{ key: "none", state: aggregate, progress: null }];

  return (
    // "deep" 让圆点的任意后代都能拖动窗口
    <div
      className="lamp-root"
      style={{ "--size": `${size}px` } as React.CSSProperties}
      data-aggregate={aggregate}
      data-quiet={quiet}
      data-tauri-drag-region="deep"
      title={tooltip(aggregate, sessions)}
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
