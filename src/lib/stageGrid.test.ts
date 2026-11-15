import { describe, expect, it } from "vitest";
import { stageColumns } from "./stageGrid";

/**
 * 这组断言的用处不是「算得对」，而是**别退化回 auto-fit**：列数一旦交给 CSS，
 * 5 组会排成 4 + 1、9 组排成 4 + 4 + 1，而这两种排法在真机上都很难看，却不会有
 * 任何报错。
 */
describe("展示模式的列数", () => {
  it("1..12 组各自排成挑好的那一种", () => {
    const expected = [1, 1, 2, 3, 2, 3, 3, 4, 4, 3, 5, 4, 4];
    for (let count = 0; count < expected.length; count += 1) {
      expect(stageColumns(count)).toBe(expected[count]);
    }
  });

  it("12 组以内不留孤零零一张", () => {
    for (let count = 4; count <= 12; count += 1) {
      // 「最后一行恰好 1 张」= count % cols === 1
      expect(count % stageColumns(count)).not.toBe(1);
    }
  });

  it("列数永远不超过组数", () => {
    for (let count = 1; count <= 60; count += 1) {
      expect(stageColumns(count)).toBeGreaterThanOrEqual(1);
      expect(stageColumns(count)).toBeLessThanOrEqual(count);
    }
  });

  it("长尾组数按方阵走，不是一条窄带", () => {
    expect(stageColumns(16)).toBe(4);
    expect(stageColumns(25)).toBe(5);
    expect(stageColumns(30)).toBe(6);
  });

  it("拿到脏数据不抛错", () => {
    expect(stageColumns(Number.NaN)).toBe(1);
    expect(stageColumns(Number.POSITIVE_INFINITY)).toBe(1);
    expect(stageColumns(-3)).toBe(1);
    expect(stageColumns(2.7)).toBe(2);
  });
});
