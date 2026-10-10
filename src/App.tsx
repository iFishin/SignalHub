import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SignalLamp } from "./components/SignalLamp";
import { SettingsPanel } from "./components/SettingsPanel";
import { useSignal } from "./lib/useSignal";
import { useSettings } from "./lib/useSettings";
import { inTauri, windowKind } from "./lib/env";
import { SIGNALS } from "./lib/types";

/**
 * dev 调试：按 1–8 直接切状态。走的是与 HTTP 上报完全相同的 Store 路径，
 * 所以能顺带验证聚合、TTL、事件推送三条链路。仅 debug 构建、且仅灯球窗口注册。
 */
function useDevKeys() {
  useEffect(() => {
    if (!import.meta.env.DEV || !inTauri || windowKind() !== "light") return;

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const index = Number(event.key) - 1;
      if (!Number.isInteger(index) || index < 0 || index >= SIGNALS.length) return;
      event.preventDefault();
      invoke("set_state", { state: SIGNALS[index] }).catch(console.error);
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
  useDevKeys();
  useQuitShortcut();

  if (windowKind() === "settings") {
    return <SettingsPanel snapshot={snapshot} />;
  }

  return (
    <SignalLamp
      state={snapshot.aggregate}
      message={snapshot.message}
      size={settings.size}
    />
  );
}
