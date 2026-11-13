import { describe, expect, it } from "vitest";
import { MESSAGES } from "./i18n";
import {
  TAB_ALIASES,
  TAB_GROUP,
  groupForTab,
  matchesTab,
  tabGroups,
  tabIds,
  tabLabel,
  type Tab,
} from "./settingsNavigation";
import { ICONS } from "./icons";

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

  // 左栏默认折叠靠 TAB_GROUP 决定「展开哪一组」。加页签忘了登记、或把一页
  // 放进了另一组，表现是「切过去之后左栏展开错组」，界面上不容易一眼看出。
  it("每组有稳定 id，组内每一页的归属都和 TAB_GROUP 一致", () => {
    const groups = tabGroups(zh);
    expect(new Set(groups.map((group) => group.id)).size).toBe(groups.length);
    for (const group of groups) {
      expect(group.entries.length).toBeGreaterThan(0);
      for (const entry of group.entries) {
        expect(TAB_GROUP[entry.id]).toBe(group.id);
        expect(groupForTab(entry.id)).toBe(group.id);
      }
    }
    expect(TAB_GROUP.system).toBeNull();
    expect(groupForTab("system")).toBeNull();
  });

  it("TAB_GROUP 覆盖全部页签，每个页签都有一枚真图标", () => {
    const ids = tabIds(zh);
    expect(Object.keys(TAB_GROUP).sort()).toEqual([...ids].sort());
    for (const id of ids) {
      expect(TAB_GROUP[id]).not.toBeUndefined();
    }
    for (const group of tabGroups(zh)) {
      for (const entry of group.entries) {
        expect(ICONS[entry.icon]?.length).toBeGreaterThan(0);
      }
    }
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

  // 「全部归位」挂在「分组」页（和「一盏灯 / 每组分灯」同一节），别名写错页
  // 的表现是搜到了、点进去却找不到按钮 —— 界面上看不出任何报错。
  it("搜「归位」落在分组页，中英别名都在", () => {
    for (const q of ["归位", "灯的位置", "位置记忆", "reset", "position"]) {
      expect(matchesTab("groups", zh.tabGroups, q)).toBe(true);
    }
    expect(matchesTab("groups", zh.tabGroups, "  RESET  ")).toBe(true);
    expect(matchesTab("look", zh.tabLook, "归位")).toBe(false);
  });

  it("归位的五段文案两种语言都齐，英文那份不含中文", () => {
    for (const key of [
      "resetLamps",
      "resetLampsAction",
      "resetLampsDone",
      "resetLampsEmpty",
      "resetLampsHint",
    ] as const) {
      expect(zh[key].trim()).not.toBe("");
      expect(en[key].trim()).not.toBe("");
      expect(en[key]).not.toMatch(/[\u4e00-\u9fff]/);
    }
    // 「没有开着组灯」时清了记忆但没摆灯，不能复用「已归位」这个词 —— 那会让
    // 按钮显示一个没发生过的动作。两种语言都得真的分开写。
    expect(zh.resetLampsEmpty).not.toBe(zh.resetLampsDone);
    expect(en.resetLampsEmpty).not.toBe(en.resetLampsDone);
  });
});
