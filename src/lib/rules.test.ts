import { describe, expect, it } from "vitest";
import { MEETING_BLUE, meetingTint } from "./rules";
import type { GroupView } from "./types";

function group(sessionIds: string[], name = "会议"): GroupView {
  return {
    name,
    default_label: null,
    aggregate: "working",
    message: null,
    since_ms: 0,
    stale_progress: null,
    sessions: sessionIds.map((id) => ({
      id,
      state: "working",
      agent: "system",
      cwd: null,
      message: null,
      age_ms: 0,
      stale_progress: null,
    })),
  };
}

describe("meetingTint", () => {
  it("paints the meeting group blue when the preset is on", () => {
    expect(meetingTint(group(["meeting"]), true)).toBe(MEETING_BLUE);
  });

  /**
   * 开关关着时永远不上色 —— 用户没开预设，蓝色不该出现。
   * 这条比「会不会上色」更要紧：默认行为必须一点不变。
   */
  it("never paints when the preset is off", () => {
    expect(meetingTint(group(["meeting"]), false)).toBeUndefined();
  });

  /**
   * 认 session id，不认组名：组名跟着界面语言变，用户还能改名。
   * 一个叫「会议」但 session 是别的东西的组，不能冒充会议。
   */
  it("matches by session id, not by group name", () => {
    expect(meetingTint(group(["claude-code"], "会议"), true)).toBeUndefined();
    expect(meetingTint(group(["meeting"], "随便改的名字"), true)).toBe(MEETING_BLUE);
  });

  it("ignores groups with no sessions", () => {
    expect(meetingTint(group([]), true)).toBeUndefined();
  });
});
