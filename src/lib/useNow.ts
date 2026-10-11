import { useEffect, useState } from "react";

/**
 * 一个会走的钟：每 `intervalMs` 返回一个新的「现在」。
 *
 * 存在的理由很具体：牌上「已持续」那一段是**算出来的**（`now - since`），而灯球只在
 * 状态变化时才重绘 —— 于是它永远停在刚变化那一刻算出的那个值上，也就是 **0s**。
 * 定时器不是锦上添花，它就是那个让数字动起来的东西。
 *
 * 一秒一次，因为「已持续」在 60 秒以内是逐秒显示的（`45s`）。超过之后显示的是
 * 分钟 / 小时，这一秒一次的 tick 大部分时候算出来的字符串没变 —— React 不会因此碰
 * DOM，所以代价只是一次很小的 diff，不会每秒重绘窗口。
 *
 * `enabled` 为假时**不起定时器**：折叠形态和关掉「已持续」的牌上根本没有这一格。
 */
export function useNow(enabled: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!enabled) return;
    // 先立刻对一次表：从「不开」切到「开」时可能已经过了很久（比如用户刚把
    // 「已持续」打开），只等下一个 tick 的话会先显示一个陈旧的数字。
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [enabled, intervalMs]);

  return now;
}
