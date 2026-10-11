import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import type { SignalState } from "./types";

/** 与 Rust 侧 `stats::Segment` 对应。 */
export interface TimelineSegment {
  state: SignalState;
  /** 这一段持续了多久（毫秒） */
  ms: number;
}

/** 兜底刷新间隔。状态一变也会立刻重取，这里只是防止长期不动时条带不更新。 */
const REFRESH_MS = 15_000;

/**
 * 近 30 分钟的状态时间线，从旧到新。
 *
 * 分段和时长都由 Rust 算 —— 那要求 `Instant` 的精确差值，而前端只有墙上时钟。
 * 前端只负责按 `ms` 把每一段画成多宽。
 *
 * `aggregate` 只用来当刷新信号：聚合一变就重取，用户因此能立刻看到刚才那一段
 * 结束了。它**不参与**渲染。
 */
export function useTimeline(aggregate: SignalState): TimelineSegment[] {
  const [segments, setSegments] = useState<TimelineSegment[]>([]);

  useEffect(() => {
    if (!inTauri) return;

    const load = () => {
      invoke<TimelineSegment[]>("get_timeline")
        .then(setSegments)
        .catch((err) => console.error("get_timeline 失败", err));
    };

    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [aggregate]);

  return segments;
}
