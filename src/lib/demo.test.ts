import { describe, expect, it } from "vitest";
import {
  DEMO_STEPS,
  demoAggregate,
  demoSnapshot,
  nextDemoStep,
  type DemoGroupId,
} from "./demo";

const LABELS: Record<DemoGroupId, string> = {
  build: "构建",
  api: "接口",
  review: "审阅",
};

describe("demo steps", () => {
  it("每一步的 aggregate 都等于各组里最急的那个", () => {
    for (const step of DEMO_STEPS) {
      expect(step.aggregate, step.id).toBe(demoAggregate(step));
    }
  });

  it("最后一步是空的，但记得刚完成", () => {
    const last = DEMO_STEPS[DEMO_STEPS.length - 1];
    expect(last.rows).toHaveLength(0);
    expect(last.aggregate).toBe("offline");
    expect(last.lastDoneAgoMs).toBeGreaterThan(0);

    const now = 1_700_000_000_000;
    const frame = demoSnapshot(DEMO_STEPS.length - 1, now, LABELS);
    expect(frame.groups).toHaveLength(0);
    expect(frame.lastDoneMs).toBe(now - last.lastDoneAgoMs!);
  });

  it("有牌的步骤里，「完成」不是转沉默", () => {
    const done = DEMO_STEPS.find((step) => step.id === "done")!;
    expect(done.rows.every((row) => row.state === "done")).toBe(true);
    expect(done.rows.some((row) => row.state === "stale")).toBe(false);
  });

  it("需要授权那一步，红的那块排在最前", () => {
    const permission = DEMO_STEPS.find((step) => step.id === "permission")!;
    expect(permission.rows[0].state).toBe("permission");
    expect(permission.aggregate).toBe("permission");
  });

  it("循环：最后一步的下一步是第一步", () => {
    expect(nextDemoStep(DEMO_STEPS.length - 1)).toBe(0);
    expect(nextDemoStep(0)).toBe(1);
  });

  it("demoSnapshot 给出可直接渲染的 GroupView", () => {
    const now = 1_700_000_000_000;
    const frame = demoSnapshot(2, now, LABELS);
    expect(frame.groups).toHaveLength(3);
    expect(frame.groups[0].default_label).toBe("构建");
    expect(frame.groups[0].since_ms).toBe(now - 6_000);
    expect(frame.groups[0].sessions).toEqual([]);
    expect(frame.groups[0].stale_progress).toBeNull();
  });
});
