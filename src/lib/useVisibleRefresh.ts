import { useEffect } from "react";

/**
 * 窗口重新显形时补拉一次。
 *
 * **为什么需要**：设置窗在启动时以 `visible: false` 建好，之后 `reveal_settings`
 * 只是 `show()` + `set_focus()`（`lib.rs`），组件**不会重新挂载**。于是「只在
 * mount 时取一次」的 hook 永远停在应用启动那一刻 —— 接入页会一直说「还没有收到过
 * 它的上报」，哪怕那个智能体已经报了一整天（issue #11）。
 *
 * 判据和 `useSignal` 里那段一模一样（`focus` 事件 + `visibilitychange`）：设置窗
 * 在隐藏期间会被系统降频，事件可能迟到或被丢掉，这次补拉是对账。
 *
 * **不要用定时轮询**：这几样数据不由状态快照驱动，轮询只是白打 IPC。
 */
export function useVisibleRefresh(refresh: () => void) {
  useEffect(() => {
    const onMaybeShown = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", onMaybeShown);
    window.addEventListener("focus", onMaybeShown);
    return () => {
      document.removeEventListener("visibilitychange", onMaybeShown);
      window.removeEventListener("focus", onMaybeShown);
    };
  }, [refresh]);
}
