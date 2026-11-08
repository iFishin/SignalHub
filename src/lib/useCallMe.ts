import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";
import type { CallMeState } from "./callme";

/** 与 Rust 侧 `callme::CALLME_EVENT` 必须一致。 */
const CALLME_EVENT = "signalhub://callme";

/**
 * Call Me 的拉取状态。
 *
 * 和 `useSignal` 同一套「先订阅再拉取」：反过来的话，两步之间到达的新留言会被
 * 永久丢掉。轮询线程是唯一写入方，这里只读。
 */
export function useCallMe(): {
  state: CallMeState | null;
  refresh: () => void;
} {
  const [state, setState] = useState<CallMeState | null>(null);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    invoke<CallMeState>("get_callme")
      .then(setState)
      .catch((err) => console.error("get_callme 失败", err));
  }, []);

  useEffect(() => {
    if (!inTauri) return;

    const pending = listen<CallMeState>(CALLME_EVENT, (event) =>
      setState(event.payload),
    );
    refresh();

    // 设置窗隐藏期间 webview 会被降频，事件可能迟到 —— 显形时对账一次。
    const onMaybeShown = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", onMaybeShown);
    window.addEventListener("focus", onMaybeShown);

    return () => {
      document.removeEventListener("visibilitychange", onMaybeShown);
      window.removeEventListener("focus", onMaybeShown);
      pending.then((unlisten) => unlisten());
    };
  }, [refresh]);

  return { state, refresh };
}
