/**
 * 设置窗口的亮 / 深色主题解析。
 *
 * 偏好字面量（`auto` / `light` / `dark`）的真值在 Rust `config.rs` 的 `Theme`，
 * 「怎么把偏好 + 系统外观算成一个具体外观」只有这里的 `resolveTheme` 一份。
 * CSS 只读结果（`<html data-theme>`），**不再判断 `prefers-color-scheme`** ——
 * 否则「跟随系统 / 强制亮 / 强制深」的语义会散在两处，改一处漏一处。
 *
 * 灯球和收件箱刻意不读这套结果：它们是透明窗口上的固定深色 HUD，理由写在
 * `styles/theme.css` 与 `styles/inbox.css` 文件头。
 */
import { useEffect } from "react";

export type ThemeMode = "auto" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

/** `index.html` 的首屏脚本读同一个 key —— 改名字必须两处同步。 */
export const RESOLVED_THEME_KEY = "signalhub-theme-resolved";

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** 纯函数：偏好 + 系统当前是否深色 → 实际该用哪套。 */
export function resolveTheme(mode: ThemeMode, systemDark: boolean): ResolvedTheme {
  if (mode === "light") return "light";
  if (mode === "dark") return "dark";
  return systemDark ? "dark" : "light";
}

/** 当前系统外观。非浏览器环境（单测）按亮色算。 */
export function systemPrefersDark(): boolean {
  return typeof window !== "undefined" && window.matchMedia(DARK_QUERY).matches;
}

/**
 * 把解析结果写进 `<html>` 和 localStorage。
 *
 * localStorage 那份**不是第二份配置**，只是首屏缓存：生产构建把 CSS 提成 `<head>`
 * 里的 `<link>`，浏览器在 React 挂载之前就应用它了；等 `useTheme` 的 effect 跑起来
 * 再设 `data-theme`，深色系统会先闪一帧白底。`index.html` 里那段同步脚本靠这个
 * 缓存在第一帧就定好外观。手改 config.json 绕开缓存时会有一次跳变，仅此而已。
 */
export function applyTheme(mode: ThemeMode): ResolvedTheme {
  const resolved = resolveTheme(mode, systemPrefersDark());
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = resolved;
  }
  try {
    localStorage.setItem(RESOLVED_THEME_KEY, resolved);
  } catch {
    // 隐私模式 / 禁用存储：只是少了首屏缓存，当前会话的主题照常生效。
  }
  return resolved;
}

/**
 * 跟随设置里的 `theme`。`ready` = `get_settings` 已经回来：IPC 之前 React 手里
 * 只有 `DEFAULTS`（`auto`），拿它去 apply 会用系统外观盖掉首屏脚本按缓存设好的
 * 主题 —— 用户选了「深色」而系统是亮色时，就会先白一帧再变回去。所以未就绪
 * 时什么都不做，让首屏那一帧原样留着。
 *
 * 只有 `Auto` 需要对系统外观变化作出反应，别的模式不挂监听器 —— mode 变化会
 * 重跑 effect，不会留下一个还在按旧偏好响应的监听。
 */
export function useTheme(mode: ThemeMode, ready: boolean): void {
  useEffect(() => {
    if (!ready) return;
    applyTheme(mode);
    if (mode !== "auto" || typeof window === "undefined") return;

    const query = window.matchMedia(DARK_QUERY);
    const onChange = () => {
      applyTheme("auto");
    };
    // 老 WebKitGTK 只有 `addListener`；这个项目的 Linux 包仍是 beta，别让主题
    // 切换在那边直接抛异常（和 state.css 里 color-mix 的兜底是同一条理由）。
    if (typeof query.addEventListener === "function") {
      query.addEventListener("change", onChange);
      return () => query.removeEventListener("change", onChange);
    }
    query.addListener(onChange);
    return () => query.removeListener(onChange);
  }, [mode, ready]);
}
