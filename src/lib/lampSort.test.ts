import { describe, expect, it } from "vitest";
import { comparePinned, labelOf, orderVisible } from "./lampSort";
import type { GroupView } from "./types";
import type { GroupMeta } from "./useSettings";

/** 造一个组。`state` 只用来占位 —— 这个文件测的是排序，不是聚合。 */
function group(name: string): GroupView {
  return {
    name,
    aggregate: "working",
    message: null,
    since_ms: 0,
    stale_progress: null,
    sessions: [],
  };
}

const names = (groups: GroupView[]) => groups.map((g) => g.name);

describe("comparePinned / orderVisible", () => {
  /**
   * 没置顶时**原样保序**。
   *
   * 这一条是整套里最要紧的：服务端给的顺序就是**紧急度**顺序（最要紧的在最前），
   * 而灯是拿来「瞥一眼」的 —— 打乱它等于把最重要的那块牌挪到看不见的地方。
   */
  it("keeps the server's urgency order when nothing is pinned", () => {
    const groups = [group("api"), group("web"), group("docs")];
    expect(names(orderVisible(groups, [], {}))).toEqual(["api", "web", "docs"]);
  });

  /**
   * 置顶的排到前面，按权重从小到大。
   *
   * 权重是「先钉的先看」由界面分配的 —— 所以数字小 = 更靠前。
   */
  it("moves pinned groups to the front, ordered by weight", () => {
    const groups = [group("api"), group("web"), group("docs")];
    const meta: Record<string, GroupMeta> = {
      docs: { order: 0 },
      web: { order: 1 },
    };
    expect(names(orderVisible(groups, [], meta))).toEqual(["docs", "web", "api"]);
  });

  /**
   * 置顶**只把置顶的拎到前面**，没置顶的仍然按紧急度排。
   *
   * 这是「pin 而不是自定义排序」的全部含义：不能让用户把顺序整个接管过来，
   * 否则一盏红灯可能被排在绿灯下面。
   */
  it("leaves the unpinned ones in urgency order after the pinned block", () => {
    const groups = [group("red"), group("amber"), group("green"), group("blue")];
    const meta: Record<string, GroupMeta> = { green: { order: 0 } };
    expect(names(orderVisible(groups, [], meta))).toEqual([
      "green",
      "red",
      "amber",
      "blue",
    ]);
  });

  /**
   * 权重相同的置顶组保持它们的原始相对顺序。
   *
   * 靠的是 `Array.prototype.sort` 的**稳定性**（ES2019 起规范保证）。Rust 那边用
   * `sort_by_key`，同样是稳定排序 —— 两边必须都是稳定的，否则同一份数据会算出
   * 不同的牌序，而窗口高度依赖「哪几块可见」。
   */
  it("keeps a stable order among pinned groups with equal weight", () => {
    const groups = [group("a"), group("b"), group("c")];
    const meta: Record<string, GroupMeta> = {
      a: { order: 5 },
      b: { order: 5 },
      c: { order: 5 },
    };
    expect(names(orderVisible(groups, [], meta))).toEqual(["a", "b", "c"]);
  });

  it("filters hidden groups out before ordering", () => {
    const groups = [group("api"), group("web"), group("docs")];
    expect(names(orderVisible(groups, ["web"], {}))).toEqual(["api", "docs"]);
    // 隐藏和置顶可以同时生效：先滤掉，再排
    expect(
      names(orderVisible(groups, ["api"], { docs: { order: 0 } })),
    ).toEqual(["docs", "web"]);
  });

  /**
   * `order` 缺失、`null`、以及整个条目缺失都读作「没置顶」。
   *
   * `null` 这一种是 Rust 侧 `Option<i16>` 序列化过来的形状 —— 漏掉它就等于
   * 「用户在界面上取消置顶之后，牌的顺序和 Rust 算的不一样」。
   */
  it("treats a missing or null order as not pinned", () => {
    const groups = [group("api"), group("web")];
    for (const meta of [
      {},
      { api: {} } as Record<string, GroupMeta>,
      { api: { order: null } } as Record<string, GroupMeta>,
    ]) {
      expect(names(orderVisible(groups, [], meta))).toEqual(["api", "web"]);
    }
  });

  /**
   * 比较器本身要是**自洽的**：`a` 对 `b` 与 `b` 对 `a` 必须相反。
   *
   * 写反一个分支（比如两个 `null` 判断都用同一个返回值）会让排序结果取决于
   * 引擎的实现细节 —— 那种 bug 在小数组上常常看不出来。
   */
  it("is antisymmetric", () => {
    const meta: Record<string, GroupMeta> = { a: { order: 1 }, b: { order: 2 } };
    const [a, b] = [group("a"), group("b")];
    expect(Math.sign(comparePinned(a, b, meta))).toBe(
      -Math.sign(comparePinned(b, a, meta)),
    );
    // 自己和自己比必须是 0
    expect(comparePinned(a, a, meta)).toBe(0);
    expect(comparePinned(group("z"), group("z"), {})).toBe(0);
  });
});

describe("labelOf", () => {
  it("falls back to the reporter's own group name", () => {
    expect(labelOf(group("build-1"), {})).toBe("build-1");
  });

  it("uses the display name when one is set", () => {
    expect(labelOf(group("build-1"), { "build-1": { name: "构建" } })).toBe("构建");
  });

  /**
   * `null` 和 `undefined` 都回落到原始组名。
   *
   * 设置面板存的就是 `null`（它把空输入规整成了 `null`，见 `setGroupName`），
   * Rust 那边是 `Option<String>` —— 所以这条是主路径，不是边角。
   */
  it("falls back when the display name is null or absent", () => {
    expect(labelOf(group("build-1"), { "build-1": { name: null } })).toBe("build-1");
    expect(labelOf(group("build-1"), { "build-1": {} })).toBe("build-1");
  });

  /**
   * 空串**不**回落到原始组名 —— 它会被原样透出去。
   *
   * 界面到不了这个状态（`setGroupName` 把空输入规整成了 `null`），所以只有手改
   * `config.json` 才会碰到。这里把它记下来，是为了让「显示名是空串时牌上会空一块」
   * 这件事有个明确的出处 —— 哪天想改成回落到原始名，改这里、这条会提醒你。
   */
  it("passes an empty display name through (reachable only by hand-editing the config)", () => {
    expect(labelOf(group("build-1"), { "build-1": { name: "" } })).toBe("");
  });
});
