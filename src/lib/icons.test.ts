import { describe, expect, it } from "vitest";
import { ICONS, type IconName } from "./icons";

describe("图标数据", () => {
  // 图标一旦漏配，`Icon` 会渲染出一枚空白的 svg：不报错、不留痕，只能在真窗口
  // 里靠肉眼发现。这里把「每个名字都真的有笔画」变成一条会失败的测试。
  it("每个名字都有笔画，且都是 M 开头的路径", () => {
    const names = Object.keys(ICONS) as IconName[];
    expect(names.length).toBeGreaterThan(20);
    for (const name of names) {
      expect(ICONS[name].length).toBeGreaterThan(0);
      for (const path of ICONS[name]) {
        expect(path.trim()).not.toBe("");
        expect(path.startsWith("M")).toBe(true);
      }
    }
  });
});
