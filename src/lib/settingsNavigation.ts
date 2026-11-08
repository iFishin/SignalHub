import type { Messages } from "./i18n";

/** 全局页签。新增功能先在这里登记，搜索、持久化和深链才会一起认识它。 */
export type Tab =
  | "status"
  | "history"
  | "messages"
  | "look"
  | "behavior"
  | "groups"
  | "alert"
  | "hooks"
  | "connect"
  | "machines"
  | "callme"
  | "guide"
  | "demo"
  | "system";

export interface TabEntry {
  id: Tab;
  label: string;
}

export interface TabGroup {
  title: string;
  entries: TabEntry[];
}

/** 搜索别名是导航数据，不是界面全文索引。中英混放是有意的。 */
export const TAB_ALIASES: Record<Tab, string[]> = {
  status: ["状态", "现在", "时间线", "status", "now", "timeline"],
  history: ["历史", "近 7 天", "统计", "今日时长", "history", "stats", "week"],
  messages: ["留言", "收信", "评论", "反馈", "call me", "callme", "message", "feedback"],
  look: ["外观", "颜色", "形状", "大小", "透明度", "预设", "灵动", "柔光", "呼吸", "光晕", "look", "color", "size", "preset", "lively", "glow"],
  behavior: ["行为", "静默超时", "超时", "安静模式", "点击穿透", "behavior", "ttl", "timeout"],
  groups: ["分组", "置顶", "多灯", "每组分灯", "group", "pin", "lamp count"],
  alert: ["提醒", "通知", "勿扰", "提示音", "声音", "alert", "notify", "dnd", "sound"],
  hooks: ["转发", "出站", "webhook", "slack", "ntfy", "wled", "物理灯", "hooks", "forward"],
  connect: ["接入", "智能体", "采集器", "cli", "hook", "token", "局域网", "web 面板", "审批", "诊断", "connect", "agent", "panel"],
  machines: ["多机", "远程", "hub", "machine", "remote"],
  callme: ["call me", "callme", "服务器", "链接", "来源", "部署", "token", "server", "source", "deploy"],
  guide: ["指南", "手册", "怎么接", "guide", "help", "faq"],
  demo: ["演示", "预览", "看看", "demo", "preview", "tour"],
  system: ["系统", "语言", "开机自启", "导入", "导出", "更新", "日志", "快捷键", "system", "language", "update", "shortcut", "backup"],
};

/** 五组 + 沉底系统。只有内容工作区、领域配置和帮助三种轴，不按开发历史堆页。 */
export function tabGroups(m: Messages): TabGroup[] {
  return [
    {
      title: m.railWatch,
      entries: [
        { id: "status", label: m.tabStatus },
        { id: "history", label: m.tabHistory },
        { id: "messages", label: m.tabMessages },
      ],
    },
    {
      title: m.railLamp,
      entries: [
        { id: "look", label: m.tabLook },
        { id: "behavior", label: m.tabBehavior },
        { id: "groups", label: m.tabGroups },
      ],
    },
    {
      title: m.railNotify,
      entries: [
        { id: "alert", label: m.tabAlert },
        { id: "hooks", label: m.tabHooks },
      ],
    },
    {
      title: m.railConnect,
      entries: [
        { id: "connect", label: m.tabConnect },
        { id: "machines", label: m.tabMachines },
        { id: "callme", label: m.tabCallMe },
      ],
    },
    {
      title: m.railHelp,
      entries: [
        { id: "guide", label: m.tabGuide },
        { id: "demo", label: m.tabDemo },
      ],
    },
  ];
}

export function tabIds(m: Messages): Tab[] {
  return [...tabGroups(m).flatMap((group) => group.entries.map((entry) => entry.id)), "system"];
}

export function tabLabel(m: Messages, tab: Tab): string {
  if (tab === "system") return m.tabSystem;
  return tabGroups(m)
    .flatMap((group) => group.entries)
    .find((entry) => entry.id === tab)?.label ?? m.tabStatus;
}

export function matchesTab(id: Tab, label: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    label.toLowerCase().includes(q) ||
    TAB_ALIASES[id].some((alias) => alias.toLowerCase().includes(q))
  );
}
