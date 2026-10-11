import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SignalLamp } from "./components/SignalLamp";
import { SettingsPanel } from "./components/SettingsPanel";
import { useSignal } from "./lib/useSignal";
import { useSettings } from "./lib/useSettings";
import { usePalettePush } from "./lib/palettes";
import { useLocalHour } from "./lib/useLocalHour";
import { inTauri, windowKind } from "./lib/env";
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
  // 两件「只有前端才知道的事实」推给 Rust：现在几点（勿扰时段要用）、
  // 每个状态解析出来的颜色（托盘图标要用）。见各自的注释。
  useLocalHour();
  usePalettePush(settings);
  useDevKeys();
  useQuitShortcut();

  if (windowKind() === "settings") {
    return <SettingsPanel snapshot={snapshot} />;
  }

  return (
    <SignalLamp
      aggregate={snapshot.aggregate}
      groups={snapshot.groups}
      settings={settings}
    />
  );
}
