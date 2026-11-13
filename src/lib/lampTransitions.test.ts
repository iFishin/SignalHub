import { describe, expect, it } from "vitest";
import { transitions, type SignSignature } from "./lampTransitions";
import type { GroupView, SignalState } from "./types";

/** 只填跃迁判断用得到的字段，其余留空。 */
function group(name: string, aggregate: SignalState, snoozeUntil = 0): GroupView {
  return {
    name,
    default_label: null,
    aggregate,
    message: null,
    since_ms: 0,
    stale_progress: null,
    snooze_until_ms: snoozeUntil,
    sessions: [],
  };
}

/** 「这一组此刻在推迟里吗」——和 `SignalLamp` 里传进去的是同一条判据。 */
const snoozedAt = (now: number) => (g: GroupView) => g.snooze_until_ms > now;

describe("transitions", () => {
  /**
   * **首帧必须不闪。** ref 初值是空表，第一帧没有任何组「变了」——不挡掉的话，
   * 应用一启动满屏的牌都会闪一下，读成「应用自己抖了一下」。
   */
  it("does not flag anything on the first frame", () => {
    const { changed, next } = transitions({}, [group("a", "working"), group("b", "error")]);
    expect(changed).toEqual([]);
    expect(next).toEqual({ a: "working", b: "error" } satisfies SignSignature);
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

  /**
   * 按下「推迟」的那一刻要闪一下 —— 那是这个动作唯一的即时反馈（牌会同时变淡，
   * 但变淡是个静态差异，用户不一定在看）。状态本身没变，所以签名里必须带推迟。
   */
  it("flags a group that has just been snoozed", () => {
    const now = 1_000_000;
    const before = transitions({}, [group("a", "permission")]);
    const after = transitions(
      before.next,
      [group("a", "permission", now + 60_000)],
      snoozedAt(now),
    );

    expect(after.changed).toEqual(["a"]);
  });

  /**
   * **到点解除推迟那一下也要闪**：用户说的是「过十五分钟再说」，而推迟刻意不发
   * 系统通知 —— 牌自己变回满强度并闪一下，就是那句「时间到了」。
   *
   * 这一帧的 `groups` 和上一帧**逐字相同**（后端到点不推帧），差别只在钟走过了
   * 那个时刻。所以判据必须从 `snoozed` 传进来，不能只看 `group.snooze_until_ms`。
   */
  it("flags the same group when its snooze expires", () => {
    const now = 1_000_000;
    const snapshot = [group("a", "permission", now + 60_000)];
    const before = transitions({}, snapshot, snoozedAt(now));
    const sameFrameLater = transitions(before.next, snapshot, snoozedAt(now + 60_001));

    expect(sameFrameLater.changed).toEqual(["a"]);
  });

  /** 推迟期间状态也没变、钟也没过点 —— 不闪（每秒的 tick 不能变成每秒一闪）。 */
  it("stays quiet while a snooze is simply running", () => {
    const now = 1_000_000;
    const snapshot = [group("a", "permission", now + 60_000)];
    const before = transitions({}, snapshot, snoozedAt(now));
    const nextTick = transitions(before.next, snapshot, snoozedAt(now + 1_000));

    expect(nextTick.changed).toEqual([]);
  });
});
