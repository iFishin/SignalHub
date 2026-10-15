import { describe, expect, it } from "vitest";
import { transitions, type AggregateMap } from "./lampTransitions";
import type { GroupView, SignalState } from "./types";

/** 只填跃迁判断用得到的字段，其余留空。 */
function group(name: string, aggregate: SignalState): GroupView {
  return {
    name,
    default_label: null,
    aggregate,
    message: null,
    since_ms: 0,
    stale_progress: null,
    sessions: [],
  };
}

describe("transitions", () => {
  /**
   * **首帧必须不闪。** ref 初值是空表，第一帧没有任何组「变了」——不挡掉的话，
   * 应用一启动满屏的牌都会闪一下，读成「应用自己抖了一下」。
   */
  it("does not flag anything on the first frame", () => {
    const { changed, next } = transitions({}, [group("a", "working"), group("b", "error")]);
    expect(changed).toEqual([]);
    expect(next).toEqual({ a: "working", b: "error" } satisfies AggregateMap);
  });

  /** 只有真的换了状态的那一组闪。 */
  it("flags only the group whose aggregate changed", () => {
    const { changed } = transitions({ a: "working", b: "error" }, [
      group("a", "working"),
      group("b", "done"),
    ]);
    expect(changed).toEqual(["b"]);
  });

  /**
   * 连续两帧同状态不闪 —— 一轮工具调用会推几十帧，每次状态没变都闪就成了骚扰，
   * 而「闪」只有变的时候才有意义。
   */
  it("does not flag anything when two frames carry the same states", () => {
    const { changed } = transitions({ a: "working", b: "error" }, [
      group("a", "working"),
      group("b", "error"),
    ]);
    expect(changed).toEqual([]);
  });

  /** 一个组第一次出现在灯上（不是从别的状态变过来）不算跃迁。 */
  it("does not treat a newly appearing group as a transition", () => {
    const { changed } = transitions({ a: "working" }, [group("a", "working"), group("b", "error")]);
    expect(changed).toEqual([]);
  });

  /**
   * 消失的组要**被忘掉**：否则它下次回来时上一帧还记着旧状态，会被当成一次
   * 说不清的跃迁（其实是「灯重新出现」）。
   */
  it("forgets a group that disappeared so its return is not a transition", () => {
    const first = transitions({ a: "working", b: "error" }, [group("a", "working")]);
    expect(first.next).toEqual({ a: "working" });

    const second = transitions(first.next, [group("a", "working"), group("b", "error")]);
    expect(second.changed).toEqual([]);
  });
});
