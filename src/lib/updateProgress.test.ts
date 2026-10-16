import { describe, expect, it } from "vitest";
import { formatBytes, percent, progressLabel } from "./updateProgress";

describe("percent", () => {
  it("按已下载 / 总量算，四舍五入到整数", () => {
    expect(percent({ downloaded: 0, total: 1000, installing: false })).toBe(0);
    expect(percent({ downloaded: 425, total: 1000, installing: false })).toBe(43);
    expect(percent({ downloaded: 1000, total: 1000, installing: false })).toBe(100);
  });

  it("总量未知时是 null，不是 0", () => {
    // 0% 和「算不出来」是两件事：前者会画出一条永远不动的进度条
    expect(percent({ downloaded: 4096, total: null, installing: false })).toBeNull();
    expect(percent({ downloaded: 0, total: 0, installing: false })).toBeNull();
  });

  it("越界夹回 0–100（分块回调有并发，累加偶尔会超一点）", () => {
    expect(percent({ downloaded: 1100, total: 1000, installing: false })).toBe(100);
    expect(percent({ downloaded: -5, total: 1000, installing: false })).toBe(0);
  });
});

describe("formatBytes", () => {
  it("1024 进制，只保留一位小数", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(900)).toBe("900 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024 * 3.25)).toBe("3.3 MB");
  });

  it("坏数（NaN / 负数）当成 0，不显示 NaN", () => {
    expect(formatBytes(Number.NaN)).toBe("0 B");
    expect(formatBytes(-100)).toBe("0 B");
  });
});

describe("progressLabel", () => {
  it("有总量就用百分比", () => {
    expect(progressLabel({ downloaded: 250, total: 1000, installing: false })).toBe("25%");
  });

  it("没有总量就退回已下载多少", () => {
    expect(progressLabel({ downloaded: 2048, total: null, installing: false })).toBe("2.0 KB");
  });
});
