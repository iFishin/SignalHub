import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";

/** 推送间隔。跨小时的那一刻最多晚一分钟被发现，无所谓。 */
const REFRESH_MS = 60_000;

/**
 * 把「现在几点 + 今天几号（本地时间）」推给 Rust。
 *
 * 为什么由前端推：Rust 那边只有 UTC，没有时区数据（连主目录都是读环境变量实现的，
 * 刻意不引入 `dirs` 之类的依赖）。所以这两个**事实**只能从这里来。
 *
 * 但判断留在 Rust ——「在不在勿扰窗口内」走 `config::QuietHours::covers`，跨午夜
 * 那种边界逻辑只有一处实现才敢信，而且那边能单测。分工是：前端提供事实，
 * Rust 做判断。
 *
 * **日期也要推**（不只是「几点」）：Rust 用它判断统计跨没跨天。原先日期只在设置面板
 * 查询统计时带过去，而灯球是常驻应用 —— 整天不开面板是常态，于是「今日时长」会
 * 横跨多天一路累加。跟着这个每分钟一次的心跳走，跨天就能在分钟级被发现。
 *
 * 两个窗口都会推（谁活着谁推），值一样，多推一次没有任何代价。
 */
export function useLocalClock(): void {
  useEffect(() => {
    if (!inTauri) return;

    const push = () => {
      // `toLocaleDateString("sv")` 给的是本地时区的 ISO 日期（YYYY-MM-DD）。
      // **必须和 useStats 用同一个写法** —— 否则这里推的日期和那边查询带的日期
      // 字符串不等，会白白触发一次「跨天」把统计清掉。
      const today = new Date().toLocaleDateString("sv");
      invoke("set_local_clock", { hour: new Date().getHours(), today }).catch((err) =>
        console.error("set_local_clock 失败", err),
      );
    };

    push();
    // 每分钟一次：机器休眠再唤醒、时区变化、跨小时、跨天，都能在一分钟内跟上。
    // 一次 IPC 只有几个字节，比在 Rust 里重新实现一遍时区划算得多。
    const timer = window.setInterval(push, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, []);
}
