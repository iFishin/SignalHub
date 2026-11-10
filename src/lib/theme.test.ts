// 这个仓库没装 @types/node（vite.config.ts 也是靠 @ts-expect-error 绕的），
// 所以读 HTML 走 Vite 的 ?raw，不引 node:fs。vite/client 已经声明了这个模块。
import html from "../../index.html?raw";
import { describe, expect, it } from "vitest";
import { RESOLVED_THEME_KEY, resolveTheme, type ThemeMode } from "./theme";

/**
 * 三种偏好 × 系统明暗的全组合。这里是「强制值必须盖过系统」的唯一守卫：
 * 写成 `systemDark ? "dark" : mode` 之类的实现会在「亮色系统 + 强制深色」
 * 这种组合下静默失效，真机上很难复现。
 */
describe("resolveTheme", () => {
  const cases: [ThemeMode, boolean, "light" | "dark"][] = [
    ["auto", false, "light"],
    ["auto", true, "dark"],
    ["light", false, "light"],
    ["light", true, "light"],
    ["dark", false, "dark"],
    ["dark", true, "dark"],
  ];

  for (const [mode, systemDark, expected] of cases) {
    it(`${mode} + 系统${systemDark ? "深色" : "亮色"} → ${expected}`, () => {
      expect(resolveTheme(mode, systemDark)).toBe(expected);
    });
  }
});

/**
 * `index.html` 的首屏脚本没法 import 模块，key 只能写成字符串。它是这个改动里
 * 唯一一处「两处真值」，用一条测试把它锁住：改了 theme.ts 却忘了改首屏脚本，
 * 表现是深色系统每次启动都闪一帧白底 —— 不报错，只能靠肉眼发现。
 */
describe("首屏缓存", () => {
  it("index.html 读的是同一个 key", () => {
    expect(html).toContain(RESOLVED_THEME_KEY);
  });
});
