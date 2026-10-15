import { describe, expect, it } from "vitest";
import { withGroupName } from "./groupMeta";

describe("withGroupName", () => {
  it("sets a custom name", () => {
    expect(withGroupName({}, "build-1", "构建")).toEqual({
      "build-1": { name: "构建" },
    });
  });

  it("trims the name", () => {
    expect(withGroupName({}, "build-1", "  构建  ")).toEqual({
      "build-1": { name: "构建" },
    });
  });

  it("an empty name drops the entry entirely", () => {
    const meta = withGroupName({ "build-1": { name: "构建" } }, "build-1", "");
    expect(meta).toEqual({});
  });

  it("a whitespace-only name counts as empty too", () => {
    expect(withGroupName({ "build-1": { name: "x" } }, "build-1", "   ")).toEqual({});
  });

  it("keeps the pin when the name is cleared", () => {
    // 改名删掉一个组名，不该顺手把它从置顶里摘出来。
    // 留下来的 `name: null` 和「没有这个键」在两边都是同一个意思。
    const meta = withGroupName({ "build-1": { name: "构建", order: 3 } }, "build-1", "");
    expect(meta).toEqual({ "build-1": { name: null, order: 3 } });
    expect(meta["build-1"].order).toBe(3);
  });

  it("keeps the pin when the name changes", () => {
    const meta = withGroupName({ "build-1": { name: "旧", order: 3 } }, "build-1", "新");
    expect(meta).toEqual({ "build-1": { name: "新", order: 3 } });
  });

  it("leaves other groups alone", () => {
    const meta = withGroupName(
      { "build-1": { name: "构建" }, deploy: { order: 1 } },
      "build-1",
      "打包",
    );
    expect(meta).toEqual({ "build-1": { name: "打包" }, deploy: { order: 1 } });
  });

  it("does not mutate the map it was given", () => {
    const before = { "build-1": { name: "构建" } };
    withGroupName(before, "build-1", "打包");
    expect(before).toEqual({ "build-1": { name: "构建" } });
  });

  it("keys by raw group name, not the display name", () => {
    // 会话分组出来的组：键是会话 id，显示名是别的东西
    const meta = withGroupName({}, "0191f0aa-77bb", "my session");
    expect(Object.keys(meta)).toEqual(["0191f0aa-77bb"]);
  });
});
