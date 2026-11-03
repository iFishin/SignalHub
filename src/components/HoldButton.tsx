import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 按住才生效的按钮（`docs/plan-approval.md` A3）。
 *
 * 只给**危险命令**的「允许」用：按住 0.8 秒才算数。它挡的不是「坏人」，是
 * **手滑** —— 一条 `rm -rf /` 不该因为一次误触就放行；而故意要放行的人，
 * 多按 0.8 秒几乎无感。
 *
 * 为什么不是「再点一次确认」：那会多一个需要读的弹窗，而这里的用户正在
 * 等着一条命令跑；按住是**不需要读**的摩擦。
 */
const HOLD_MS = 800;

interface Props {
  onConfirm: () => void;
  disabled?: boolean;
  className?: string;
  /** 无障碍提示：按钮文字之外要说明「这是按住生效的」。 */
  title?: string;
  ariaLabel?: string;
  /** 透传，好让气泡那套 `[data-decision="allow"]` 的描边照常生效。 */
  "data-decision"?: string;
  "data-state"?: string;
  children: React.ReactNode;
}

export function HoldButton({
  onConfirm,
  disabled,
  className,
  title,
  ariaLabel,
  "data-decision": dataDecision,
  "data-state": dataState,
  children,
}: Props) {
  const [progress, setProgress] = useState(0);
  const startedAt = useRef<number | null>(null);
  const frame = useRef<number | null>(null);

  const stop = useCallback(() => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = null;
    startedAt.current = null;
    setProgress(0);
  }, []);

  const tick = useCallback(() => {
    if (startedAt.current === null) return;
    const elapsed = performance.now() - startedAt.current;
    const next = Math.min(elapsed / HOLD_MS, 1);
    setProgress(next);
    if (next >= 1) {
      // **先停再回调**：`onConfirm` 会让这条气泡消失，组件随即卸载；
      // 不停掉的话下一帧还会往一个已经不在的组件里 setState。
      stop();
      onConfirm();
      return;
    }
    frame.current = requestAnimationFrame(tick);
  }, [onConfirm, stop]);

  const start = useCallback(() => {
    if (disabled || startedAt.current !== null) return;
    startedAt.current = performance.now();
    frame.current = requestAnimationFrame(tick);
  }, [disabled, tick]);

  // 卸载时把动画帧收掉 —— 气泡被「超时」或「别处点了」带走时走这条路。
  useEffect(() => stop, [stop]);

  return (
    <button
      type="button"
      className={className}
      disabled={disabled}
      title={title}
      aria-label={ariaLabel}
      data-decision={dataDecision}
      data-state={dataState}
      // CSS 变量驱动那条进度条；不用 state 存宽度，省一次重排
      style={{ ["--hold" as string]: progress }}
      onPointerDown={start}
      onPointerUp={stop}
      onPointerLeave={stop}
      onPointerCancel={stop}
      onKeyDown={(event) => {
        // 键盘也要能用：按住空格 / 回车，和按住鼠标是同一件事
        if (event.key === " " || event.key === "Enter") {
          event.preventDefault();
          start();
        }
      }}
      onKeyUp={(event) => {
        if (event.key === " " || event.key === "Enter") stop();
      }}
      onBlur={stop}
    >
      <span className="hold-fill" aria-hidden="true" />
      <span className="hold-label">{children}</span>
    </button>
  );
}
