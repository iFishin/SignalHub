/**
 * Tauri 2 会注入 __TAURI_INTERNALS__。用它判断运行环境，
 * 好让纯浏览器里跑 `npm run dev` 时前端不崩（调不到 IPC 会抛异常）。
 */
export const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type WindowKind = "light" | "settings";

/** 灯球和设置窗共用同一个 Vite 应用，靠 tauri.conf.json 里的查询参数分流。 */
export function windowKind(): WindowKind {
  return new URLSearchParams(window.location.search).get("window") === "settings"
    ? "settings"
    : "light";
}
