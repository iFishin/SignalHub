/**
 * Tauri 2 会注入 __TAURI_INTERNALS__。用它判断运行环境，
 * 好让纯浏览器里跑 `npm run dev` 时前端不崩（调不到 IPC 会抛异常）。
 */
export const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type WindowKind = "light" | "settings" | "inbox" | "demo";

/** 灯球、设置窗、收件箱、独立演示页共用一个 Vite 应用，靠查询参数分流。
 *  `demo` 不是 Tauri 里注册的窗口，是给浏览器 / README 动图准备的纯前端页面。 */
export function windowKind(): WindowKind {
  const kind = new URLSearchParams(window.location.search).get("window");
  if (kind === "settings") return "settings";
  if (kind === "inbox") return "inbox";
  if (kind === "demo") return "demo";
  return "light";
}

/**
 * 这个灯窗显示的**组名**，null = 主灯（画全局 / 全部）。
 *
 * 多灯模式下每个组灯窗的 URL 是 `?window=light&group=<percent-encoded>`（由 Rust
 * 侧的 `lamps::encode_query` 编码）。`URLSearchParams` 会自动把百分号解码回 UTF-8，
 * 所以这里拿到的就是原始组名。
 */
export function windowGroup(): string | null {
  return new URLSearchParams(window.location.search).get("group");
}
