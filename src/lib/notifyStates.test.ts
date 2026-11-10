import { describe, expect, it } from "vitest";
import { NOTIFY_STATES, SIGNALS } from "./types";
import { DEFAULTS } from "./useSettings";

describe("通知可选的状态", () => {
  it("不提供 offline / off —— 它们不代表发生了什么事", () => {
    expect(NOTIFY_STATES).not.toContain("offline");
    expect(NOTIFY_STATES).not.toContain("off");
    // stale 要给：一个会话沉默到快消失，是值得知道的事
    expect(NOTIFY_STATES).toContain("stale");
  });

  it("全部来自 SIGNALS，不另造状态名", () => {
    for (const state of NOTIFY_STATES) {
      expect(SIGNALS).toContain(state);
    }
  });

  /**
   * 默认值必须是**界面勾得出来**的。
   *
   * 反过来说：默认里若出现一个 chip 上没有的状态，用户会看到「三个都勾着，
   * 却有一个永远关不掉」—— 因为界面上根本没有对应的按钮。Rust 侧
   * `the_default_states_are_all_selectable` 钉的是同一条。
   */
  it("默认值全都能在界面上勾选", () => {
    expect(DEFAULTS.notify_when.length).toBeGreaterThan(0);
    for (const name of DEFAULTS.notify_when) {
      expect(NOTIFY_STATES).toContain(name);
    }
  });
});
