import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";
import type { Snapshot } from "./types";

/** 与 Rust 侧 sweep::STATE_EVENT 必须一致。 */
const STATE_EVENT = "signalhub://state";

/** 初始值取 offline：在拿到真实状态之前不该假装自己是绿的，也不该假装有人在上报。 */
const INITIAL: Snapshot = {
  aggregate: "offline",
  paused: false,
  since_ms: 0,
  message: null,
  groups: [],
};

export function useSignal(): Snapshot {
  const [snapshot, setSnapshot] = useState<Snapshot>(INITIAL);

  useEffect(() => {
    if (!inTauri) return;

    const refetch = () =>
      invoke<Snapshot>("get_state")
        .then(setSnapshot)
        .catch((err) => console.error("get_state 失败", err));

    // 先订阅再拉取：反过来的话，两步之间发生的变更会被永久丢掉。
    const pending = listen<Snapshot>(STATE_EVENT, (event) =>
      setSnapshot(event.payload),
    );
    refetch();

    // 重新显形时补拉一次。Rust 侧是**广播**（见 notify.rs 的 emit），所以设置窗
    // 理论上一直收得到；但它在启动时就以 `visible: false` 加载起来，隐藏期间
    // webview 会被系统降频、事件可能迟到或被丢掉 —— 那正是「打开面板看到的是很久
    // 以前那一刻」的来源。这次补拉是对账，不是唯一的数据来源。
    // 两个事件都听：`focus` 是 Tauri 真正 show + set_focus 时会来的那个，
    // `visibilitychange` 兜住 webview 自己判定可见性的情况。
    const onMaybeShown = () => {
      if (document.visibilityState === "visible") refetch();
    };
    document.addEventListener("visibilitychange", onMaybeShown);
    window.addEventListener("focus", onMaybeShown);

    return () => {
      document.removeEventListener("visibilitychange", onMaybeShown);
      window.removeEventListener("focus", onMaybeShown);
      pending.then((unlisten) => unlisten());
    };
  }, []);

  return snapshot;
}
