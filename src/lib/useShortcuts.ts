import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";

/**
 * 一条全局快捷键现在的处境。
 *
 * `action` 是稳定的动作名（`lamp` / `mute` / `clear` / `pass_through`），界面按它取
 * 本地化的名字；`label` 是人话写法，**由 Rust 从注册的那份组合派生** —— 前端不自己
 * 拼组合，那会变成「文案和真正生效的那个键漂移」的第二处真值。
 */
export interface ShortcutStatus {
  action: string;
  label: string;
  /** 抢到了没有。false 时界面要**明说**（多半是被别的程序占了）。 */
  ok: boolean;
}

/**
 * 四个全局快捷键。**只在启动时算一次**（注册就那一次），所以这里没有事件订阅 ——
 * 不像 `useToggles` 那样会被别的入口改。
 */
export function useShortcuts(): ShortcutStatus[] {
  const [shortcuts, setShortcuts] = useState<ShortcutStatus[]>([]);

  useEffect(() => {
    if (!inTauri) return;
    invoke<ShortcutStatus[]>("get_shortcuts")
      .then(setShortcuts)
      .catch((err) => console.error("get_shortcuts 失败", err));
  }, []);

  return shortcuts;
}
