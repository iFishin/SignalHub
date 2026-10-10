import { SIGNAL_LABEL } from "../lib/signals";
import type { SignalState } from "../lib/types";

interface Props {
  state: SignalState;
  message: string | null;
  /** 圆点直径（px）。由设置决定，Rust 侧同步调整窗口大小以留出边距。 */
  size: number;
}

export function SignalLamp({ state, message, size }: Props) {
  // 悬停提示用原生 title：这段文字里没有任何需要交互的东西，
  // 不值得为它引入一个自绘浮层。
  const title = message ? `${SIGNAL_LABEL[state]} — ${message}` : SIGNAL_LABEL[state];

  return (
    // "deep" 让圆点的任意后代都能拖动窗口；用默认的浅层拖拽会有大片区域拖不动。
    <div className="lamp-root" data-state={state} data-tauri-drag-region="deep" title={title}>
      <span className="lamp" style={{ "--size": `${size}px` } as React.CSSProperties} />
    </div>
  );
}
