import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";

/**
 * 每个状态**内置**的停留时长（毫秒）：`{ working: 300000, done: 30000, ... }`。
 *
 * 为什么需要它：「逐个状态覆盖」那张表的「停留时长」一列，没设过的那一项要标成
 * 「跟随 5m」—— 这一列问的恰恰是「这个状态静默多久算过期」，只写「跟随」两个字
 * 等于没答。而那个数的真值在 Rust（`Signal::default_ttl`，见 `state_ttl_defaults`），
 * 在前端再写一份的话，改了那边这边不会报错，只会**悄悄显示错的**。
 *
 * 拿不到（不在 Tauri 里、或后端还没有这一步）就返回空表：那一格退化成「跟随」，
 * 功能不受影响。
 */
export function useStateTtls(): Record<string, number> {
  const [defaults, setDefaults] = useState<Record<string, number>>({});

  useEffect(() => {
    if (!inTauri) return;
    invoke<Record<string, number>>("state_ttl_defaults")
      .then(setDefaults)
      .catch(() => {});
  }, []);

  return defaults;
}
