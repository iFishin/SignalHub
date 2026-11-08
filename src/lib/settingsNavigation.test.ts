import { describe, expect, it } from "vitest";
import { MESSAGES } from "./i18n";
import {
  TAB_ALIASES,
  matchesTab,
  tabGroups,
  tabIds,
  tabLabel,
  type Tab,
} from "./settingsNavigation";

const zh = MESSAGES.zh;
const en = MESSAGES.en;

describe("设置页签导航", () => {
  it("每个页签只出现一次", () => {
    const ids = tabGroups(zh).flatMap((group) => group.entries.map((entry) => entry.id));
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("「系统」不在任何一组里，但仍在 tabIds 的最末尾", () => {
    const grouped = tabGroups(zh).flatMap((group) => group.entries.map((e) => e.id));
    expect(grouped).not.toContain("system");
    const ids = tabIds(zh);
    expect(ids).toContain("system");
    expect(ids[ids.length - 1]).toBe("system");
  });

  // 旧版本把当前页签 id 存进了 localStorage；删掉或改名会让回来的人被送到
  // 「状态」页，而他们找不到的那一页其实还在。这条测试就是那份兼容名单。
  it("历史上出现过的页签 id 依然认得", () => {
    const legacy: Tab[] = [
      "status",
      "history",
      "look",
      "behavior",
      "groups",
      "alert",
      "hooks",
      "connect",
      "machines",
      "callme",
      "guide",
      "demo",
      "system",
    ];
    const ids = tabIds(zh);
    for (const id of legacy) expect(ids).toContain(id);
  });

  // 加了页签却忘了写别名，表现是「搜什么都搜不到它」—— 界面上看不出错。
  it("每个页签在两种语言下都有搜索别名", () => {
    for (const id of tabIds(zh)) {
      expect(TAB_ALIASES[id].length).toBeGreaterThan(0);
    }
  });

  it("每页都有非空标题，两种语言一致", () => {
    for (const id of tabIds(zh)) {
      expect(tabLabel(zh, id)).not.toBe("");
      expect(tabLabel(en, id)).not.toBe("");
    }
    expect(tabLabel(zh, "system")).toBe(zh.tabSystem);
    expect(tabLabel(en, "system")).toBe(en.tabSystem);
  });

  it("分组标题跟着语言走", () => {
    expect(tabGroups(zh).map((group) => group.title)).toEqual([
      zh.railWatch,
      zh.railLamp,
      zh.railNotify,
      zh.railConnect,
      zh.railHelp,
    ]);
  });

  it("搜索：空串全通过，标签和别名都能命中，大小写与首尾空格无关", () => {
    expect(matchesTab("callme", zh.tabCallMe, "")).toBe(true);
    expect(matchesTab("callme", zh.tabCallMe, "   ")).toBe(true);
    expect(matchesTab("callme", zh.tabCallMe, zh.tabCallMe)).toBe(true);
    expect(matchesTab("callme", zh.tabCallMe, "CALLME")).toBe(true);
    expect(matchesTab("callme", zh.tabCallMe, " 部署 ")).toBe(true);
    expect(matchesTab("callme", zh.tabCallMe, "令牌并不存在")).toBe(false);
  });

  it("「留言」与「Call Me」是两页，别名也各自分开", () => {
    const ids = tabIds(zh);
    expect(ids).toContain("messages");
    expect(ids).toContain("callme");
    // 搜「留言」只该落到阅读那一页；配置页的别名里不该出现它，否则
    // 用户搜「留言」会被送进一个不含留言的页面。
    expect(matchesTab("messages", zh.tabMessages, "留言")).toBe(true);
    expect(matchesTab("callme", zh.tabCallMe, "留言")).toBe(false);
  });
});
