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

    // 先订阅再拉取：反过来的话，两步之间发生的变更会被永久丢掉。
    const pending = listen<Snapshot>(STATE_EVENT, (event) =>
      setSnapshot(event.payload),
    );
    invoke<Snapshot>("get_state")
      .then(setSnapshot)
      .catch((err) => console.error("get_state 失败", err));

    return () => {
      pending.then((unlisten) => unlisten());
    };
  }, []);

  return snapshot;
}
