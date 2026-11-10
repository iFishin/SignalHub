import { describe, expect, it } from "vitest";
import {
  DEMO_SCENARIOS,
  DEMO_STEPS,
  demoAggregate,
  demoSnapshot,
  nextDemoStep,
  prevDemoStep,
  scenarioStart,
  scenarioSteps,
  stepScenario,
  type DemoGroupId,
  type DemoScenarioId,
} from "./demo";
import { MESSAGES } from "./i18n";

const LABELS: Record<DemoGroupId, string> = {
  build: "构建",
  api: "接口",
  review: "审阅",
};

const LANGS = ["zh", "en"] as const;
const NOW = 1_700_000_000_000;

describe("demo steps", () => {
  it("每一步的 aggregate 都等于各组里最急的那个", () => {
    for (const step of DEMO_STEPS) {
      expect(step.aggregate, step.id).toBe(demoAggregate(step));
    }
  });

  it("场景首尾相接，摊平后的顺序与声明一致", () => {
    let cursor = 0;
    for (const scenario of DEMO_SCENARIOS) {
      expect(scenario.steps.length, scenario.id).toBeGreaterThanOrEqual(2);
      expect(scenarioStart(scenario.id), scenario.id).toBe(cursor);
      for (const step of scenario.steps) {
        expect(DEMO_STEPS[cursor], step.id).toBe(step);
        expect(stepScenario(cursor), step.id).toBe(scenario.id);
        cursor += 1;
      }
    }
    expect(cursor).toBe(DEMO_STEPS.length);
    expect(scenarioSteps("day")).toBe(DEMO_SCENARIOS[0].steps);
  });

  it("步骤 id 不重复，且中英文都写了文案", () => {
    const ids = DEMO_STEPS.map((step) => step.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const lang of LANGS) {
      for (const id of ids) {
        const info = MESSAGES[lang].demoStep[id];
        expect(info, `${lang}/${id}`).toBeDefined();
        expect(info.title.length, `${lang}/${id}`).toBeGreaterThan(0);
        expect(info.body.length, `${lang}/${id}`).toBeGreaterThan(0);
      }
    }
  });

  it("每个场景中英文都有名字", () => {
    for (const lang of LANGS) {
      for (const scenario of DEMO_SCENARIOS) {
        expect(MESSAGES[lang].demoScenario[scenario.id], `${lang}/${scenario.id}`)
          .toBeTruthy();
      }
    }
  });

  it("「刚完成」那一步是空的，但记得刚完成", () => {
    const empty = DEMO_STEPS.find((step) => step.id === "empty")!;
    expect(empty.rows).toHaveLength(0);
    expect(empty.aggregate).toBe("offline");
    expect(empty.lastDoneAgoMs).toBeGreaterThan(0);

    const index = DEMO_STEPS.indexOf(empty);
    const frame = demoSnapshot(index, NOW, LABELS);
    expect(frame.groups).toHaveLength(0);
    expect(frame.lastDoneMs).toBe(NOW - empty.lastDoneAgoMs!);
  });

  it("「没上报」那一步不冒充「刚完成」", () => {
    const offline = DEMO_STEPS.find((step) => step.id === "offline")!;
    expect(offline.rows).toHaveLength(0);
    expect(offline.aggregate).toBe("offline");
    // 没有 lastDoneAgoMs，牌上才会说「没有智能体在上报」而不是「刚完成」
    expect(offline.lastDoneAgoMs).toBeUndefined();
    expect(demoSnapshot(DEMO_STEPS.indexOf(offline), NOW, LABELS).lastDoneMs).toBe(0);
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

  it("红黄同屏时，红排在黄前面", () => {
    const both = DEMO_STEPS.find((step) => step.id === "both")!;
    expect(both.rows.map((row) => row.state)).toEqual([
      "permission",
      "attention",
      "working",
    ]);
  });

  it("静默那一步不假装有进度，圆环那一步才带真实的环进度", () => {
    const stale = DEMO_STEPS.find((step) => step.id === "stale")!;
    expect(stale.rows[0].state).toBe("stale");
    // 默认形状是实心点 —— 没有地方画进度，就不该给一个画不出来的值
    expect(stale.shape).toBeUndefined();
    expect(stale.rows[0].staleProgress).toBeUndefined();

    const ring = DEMO_STEPS.find((step) => step.id === "staleRing")!;
    expect(ring.shape).toBe("ring");
    const row = ring.rows[0];
    expect(row.state).toBe("stale");
    expect(row.staleProgress).toBeGreaterThan(0);
    expect(row.staleProgress).toBeLessThanOrEqual(1);

    const frame = demoSnapshot(DEMO_STEPS.indexOf(ring), NOW, LABELS);
    expect(frame.shape).toBe("ring");
    expect(frame.groups[0].stale_progress).toBe(row.staleProgress);
  });

  it("折叠 / 安静由剧本决定，其余步骤一律展开不压暗", () => {
    const flags = (id: string) => {
      const frame = demoSnapshot(
        DEMO_STEPS.findIndex((step) => step.id === id),
        NOW,
        LABELS,
      );
      return [frame.collapsed, frame.quiet];
    };

    expect(flags("collapsed")).toEqual([true, false]);
    expect(flags("collapsedDone")).toEqual([true, false]);
    expect(flags("quiet")).toEqual([false, true]);
    expect(flags("working")).toEqual([false, false]);
    expect(flags("idle")).toEqual([false, false]);
  });

  it("循环在场景内：上一步 / 下一步都不跨场景", () => {
    for (const scenario of DEMO_SCENARIOS) {
      const first = scenarioStart(scenario.id);
      const last = first + scenario.steps.length - 1;
      expect(nextDemoStep(last), scenario.id).toBe(first);
      expect(prevDemoStep(first), scenario.id).toBe(last);
      expect(stepScenario(nextDemoStep(first)), scenario.id).toBe(scenario.id);
      expect(stepScenario(prevDemoStep(last)), scenario.id).toBe(scenario.id);
    }
    // 只有一个场景时，globals 的首尾相接仍然成立
    expect(nextDemoStep(0)).toBe(1);
  });

  it("场景 id 覆盖五种用法，且默认从「一天的节奏」开始", () => {
    const ids: DemoScenarioId[] = ["day", "asking", "silence", "tucked", "look"];
    expect(DEMO_SCENARIOS.map((scenario) => scenario.id)).toEqual(ids);
    expect(scenarioStart("day")).toBe(0);
  });

  /**
   * 边灯 / 角灯那一场必须有东西可讲 —— 这两项默认全关，演示是唯一会被新用户
   * 看到的地方。少一步不会报错，只会让那一场变成一条什么都不演示的空镜头。
   */
  it("「用光说话」那一场真的切换了边灯与角灯", () => {
    const steps = scenarioSteps("look");
    const edges = steps.map((step) => step.edge).filter(Boolean);
    expect(edges).toContain("top");
    expect(edges).toContain("frame");
    expect(edges).toContain("glow");
    const withCorners = steps.filter((step) => step.corners);
    expect(withCorners.length).toBeGreaterThan(0);
    // 四枚判据同时成立的最后一步：置顶 + 静音 + 沉默 + 状态，缺一条角灯就不亮
    const meaning = steps.find((step) => step.id === "cornersMeaning");
    expect(meaning?.pinned).toBe(true);
    expect(meaning?.muted).toBe(true);
    expect(meaning?.rows[0].state).toBe("stale");
    expect(meaning?.rows[0].staleProgress).not.toBeUndefined();
  });

  it("demoSnapshot 给出可直接渲染的 GroupView", () => {
    const index = DEMO_STEPS.findIndex((step) => step.id === "working");
    const frame = demoSnapshot(index, NOW, LABELS);
    expect(frame.groups).toHaveLength(3);
    expect(frame.groups[0].default_label).toBe("构建");
    expect(frame.groups[0].since_ms).toBe(NOW - 6_000);
    expect(frame.groups[0].sessions).toEqual([]);
    expect(frame.groups[0].stale_progress).toBeNull();
  });
});
