import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import type { SignalState } from "./types";

export interface StatRow {
  state: SignalState;
  label: string;
  seconds: number;
}

/** 刷新间隔。统计只用来「看一眼」，不必更密。 */
const REFRESH_MS = 30_000;

/**
 * 今日各状态的累计时长，按时长从多到少。
 *
 * 本地日期由**前端**传给 Rust —— 那边只有 UTC（没有时区数据），
 * 而「今日」按用户所在时区算才对。`toLocaleDateString("sv")` 给的是
 * 本地时区的 ISO 格式（YYYY-MM-DD），比手工拼 Date 的字段稳。
 */
export function useStats(): StatRow[] {
  const [rows, setRows] = useState<StatRow[]>([]);

  useEffect(() => {
    if (!inTauri) return;

    const load = () => {
      const today = new Date().toLocaleDateString("sv");
      invoke<StatRow[]>("get_stats", { today })
        .then(setRows)
        .catch((err) => console.error("get_stats 失败", err));
    };

    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, []);

  return rows;
}
