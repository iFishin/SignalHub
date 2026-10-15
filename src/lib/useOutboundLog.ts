import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import type { SignalState } from "./types";

/** 与 Rust 侧 `outbound::Attempt` 对应。 */
export interface Attempt {
  /** epoch 毫秒 */
  at_ms: number;
  /**
   * 目标（网址或命令）。**按它对应到钩子，不要按下标** —— 钩子列表可以增删排序，
   * 下标会错位，于是界面上会显示成「另一条钩子的结果」，比不显示更糟。
   */
  target: string;
  ok: boolean;
  /** 成功是 HTTP 状态码（命令成功为空），失败是原因。 */
  detail: string;
}

/** 兜底刷新间隔。聚合一变也会重取 —— 出站调用就发生在跃迁那一刻。 */
const REFRESH_MS = 15_000;

/**
 * 最近几次出站调用，从旧到新。
 *
 * 与 useTimeline 同构：挂载时取一次 + 定时兜底，`aggregate` 只当刷新信号、不参与
 * 渲染。选它当信号是因为聚合跃迁正是钩子触发的时刻，新结果因此立刻出现；但限定组
 * 的钩子看的是**各组自己**的跃迁，那不一定改变聚合 —— 所以还要靠定时兜底把这类
 * 结果捞上来。
 */
export function useOutboundLog(aggregate: SignalState, nonce = 0): Attempt[] {
  const [attempts, setAttempts] = useState<Attempt[]>([]);

  useEffect(() => {
    if (!inTauri) return;

    const load = () => {
      invoke<Attempt[]>("get_outbound_log")
        .then(setAttempts)
        .catch((err) => console.error("get_outbound_log 失败", err));
    };

    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
    // `nonce` 只是个「现在重取一次」的信号（「发送测试」按钮点完就加一）。
    // 单靠 aggregate 和定时兜底的话，用户点完测试要等最多 15 秒才看到结果 ——
    // 而「点了没反应」正是这一轮要消灭的那种体验。
  }, [aggregate, nonce]);

  return attempts;
}

/**
 * 最近一次打到某个目标的出站调用。
 *
 * 从后往前找第一条 `target` 相同的 —— 日志从旧到新，最后一条就是最新的。找不到
 * 就是「还没触发过」，这与「目标为空」是两回事，由调用方分别处理。
 */
export function lastAttemptFor(
  attempts: Attempt[],
  target: string,
): Attempt | undefined {
  for (let i = attempts.length - 1; i >= 0; i--) {
    if (attempts[i].target === target) return attempts[i];
  }
  return undefined;
}
