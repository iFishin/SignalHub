import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SignalLamp } from "./components/SignalLamp";
import { SettingsPanel } from "./components/SettingsPanel";
import { useSignal } from "./lib/useSignal";
import { useSettings } from "./lib/useSettings";
import { usePalettePush } from "./lib/palettes";
import { useLocalClock } from "./lib/useLocalClock";
import { useApprovals } from "./lib/useApprovals";
import { useLangPush } from "./lib/i18n";
import { inTauri, windowGroup, windowKind } from "./lib/env";
import { perGroupActive } from "./lib/lampLayout";
import { REPORTABLE_SIGNALS } from "./lib/types";

/**
 * dev 调试：按 1–8 直接切状态。走的是与 HTTP 上报完全相同的 Store 路径，
 * 所以能顺带验证聚合、TTL、事件推送三条链路。仅 debug 构建、且仅灯球窗口注册。
 *
 * 遍历 REPORTABLE_SIGNALS 而不是全部状态：stale / offline 是派生出来的，
 * 服务端会拒绝上报。
 */
function useDevKeys() {
  useEffect(() => {
    if (!import.meta.env.DEV || !inTauri || windowKind() !== "light") return;

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const index = Number(event.key) - 1;
      if (!Number.isInteger(index) || index < 0 || index >= REPORTABLE_SIGNALS.length) return;
      event.preventDefault();
      invoke("set_state", { state: REPORTABLE_SIGNALS[index] }).catch(console.error);
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}

/** 托盘菜单是主要退出入口，这里只是照顾 Cmd/Ctrl+Q 的键盘习惯。 */
function useQuitShortcut() {
  useEffect(() => {
    if (!inTauri) return;

    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "q") {
        event.preventDefault();
        invoke("quit").catch(console.error);
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}

export default function App() {
  // 两个窗口共用这套订阅；hook 必须无条件调用，所以放在分支之前。
  const snapshot = useSignal();
  const settings = useSettings();
  // 三件「只有前端才知道的事实」推给 Rust：现在几点 + 今天几号（勿扰时段和统计
  // 跨天要用）、每个状态解析出来的颜色（托盘图标要用）。见各自的注释。
  useLocalClock();
  usePalettePush(settings);
  useLangPush();
  useDevKeys();
  useQuitShortcut();
  // 等着你点的审批（从灯上审批）。**两个窗口都会订阅**，但只有灯窗会画 ——
  // 设置窗读它是为了那个开关旁边能说「现在有一条在等」。
  const { approvals, decide } = useApprovals();

  if (windowKind() === "settings") {
    return <SettingsPanel snapshot={snapshot} />;
  }


  // 多灯：这个灯窗只看自己那一组（`?group=`）。主灯在「每组分灯」生效时退成一颗
  // 全局灯 —— 传空组让 SignalLamp 走它现有的占位 / 全局分支。
  //
  // **判断必须和 Rust 的 `per_group_active` 一致**（见 lib/lampLayout.ts）。
  const myGroup = windowGroup();
  const one = myGroup === null ? undefined : snapshot.groups.find((g) => g.name === myGroup);
  const groups =
    myGroup !== null
      ? snapshot.groups.filter((g) => g.name === myGroup)
      : perGroupActive(settings, snapshot.groups)
        ? []
        : snapshot.groups;

  return (
    <SignalLamp
      // 组灯用**这一组**的聚合（角标 / 底色都该跟着它），主灯才用全局聚合
      aggregate={one ? one.aggregate : snapshot.aggregate}
      groups={groups}
      settings={settings}
      // 暂停是**灯上**要持续可见的状态，而 `off`（暂停时的聚合值）和 `offline`
      // 都是很淡的灰，只靠颜色分不出来。
      paused={snapshot.paused}
      // 待审批的气泡**只在主灯上**（组灯是给「这一组现在怎么样」用的，而审批是
      // 「有没有事要我做」）。多灯模式下主灯仍然是那一盏 —— 它退成全局灯。
      approvals={myGroup === null ? approvals : undefined}
      onDecide={myGroup === null ? decide : undefined}
    />
  );
}
