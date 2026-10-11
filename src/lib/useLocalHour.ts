import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";

/** 推送间隔。跨小时的那一刻最多晚一分钟被发现，无所谓。 */
const REFRESH_MS = 60_000;

/**
 * 把「现在是几点（本地时间）」推给 Rust。
 *
 * 为什么由前端推：Rust 那边只有 UTC，没有时区数据（连主目录都是读环境变量实现的，
 * 刻意不引入 `dirs` 之类的依赖）。所以「现在几点」这个**事实**只能从这里来。
 *
 * 但「在不在勿扰窗口内」的判断留在 Rust（`config::QuietHours::covers`）——
 * 跨午夜那种边界逻辑只有一处实现才敢信，而且那边能单测。分工是：前端提供事实，
 * Rust 做判断。
 *
 * 两个窗口都会推（谁活着谁推），值一样，多推一次没有任何代价。
 */
export function useLocalHour(): void {
  useEffect(() => {
    if (!inTauri) return;

    const push = () => {
      invoke("set_local_hour", { hour: new Date().getHours() }).catch((err) =>
        console.error("set_local_hour 失败", err),
      );
    };

    push();
    // 每分钟一次：机器休眠再唤醒、时区变化、跨小时，都能在一分钟内跟上。
    // 一次 IPC 只有几个字节，比在 Rust 里重新实现一遍时区划算得多。
    const timer = window.setInterval(push, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, []);
}
