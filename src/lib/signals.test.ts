import { describe, expect, it } from "vitest";
import { JUST_FINISHED_MS, justFinishedLabel, signalLabel } from "./signals";

/** 一个固定的「现在」，避免测试依赖真实时钟。 */
const NOW = 1_700_000_000_000;

describe("justFinishedLabel", () => {
  it("没完成过就什么都不说 —— 调用方退回「没有智能体在上报」", () => {
    expect(justFinishedLabel("zh", 0, NOW)).toBeNull();
  });

  it("刚完成不到一分钟说「刚刚」", () => {
    expect(justFinishedLabel("zh", NOW - 3_000, NOW)).toBe("刚完成 · 刚刚");
    // 边界：59 秒仍然是「刚刚」，60 秒就是 1 分钟前
    expect(justFinishedLabel("zh", NOW - 59_000, NOW)).toBe("刚完成 · 刚刚");
    expect(justFinishedLabel("zh", NOW - 60_000, NOW)).toBe("刚完成 · 1 分钟前");
  });

  it("分钟数向下取整 —— 2 分 59 秒还是「2 分钟前」", () => {
    expect(justFinishedLabel("zh", NOW - (2 * 60 + 59) * 1_000, NOW)).toBe(
      "刚完成 · 2 分钟前",
    );
  });

  it("过了窗口就闭嘴，别把一小时前的旧事说成「刚」", () => {
    // 边界之内（正好 10 分钟）还说；再过 1 毫秒就不说了
    expect(justFinishedLabel("zh", NOW - JUST_FINISHED_MS, NOW)).toBe(
      "刚完成 · 10 分钟前",
    );
    expect(justFinishedLabel("zh", NOW - JUST_FINISHED_MS - 1, NOW)).toBeNull();
  });

  it("时钟回拨时不硬凑一个「-3 分钟前」", () => {
    expect(justFinishedLabel("zh", NOW + 5_000, NOW)).toBeNull();
  });

  it("英文同样分档", () => {
    expect(justFinishedLabel("en", NOW - 5_000, NOW)).toBe(
      "Just finished · moments ago",
    );
    expect(justFinishedLabel("en", NOW - 3 * 60_000, NOW)).toBe(
      "Just finished · 3 min ago",
    );
  });
});

describe("signalLabel", () => {
  it("普通状态照旧走那张表 —— 只有空状态才可能被「刚完成」顶替", () => {
    expect(signalLabel("zh", "offline")).toBe("没有智能体在上报");
    expect(signalLabel("zh", "done")).toBe("已完成");
    expect(signalLabel("en", "offline")).toBe("No agents reporting");
  });
});
