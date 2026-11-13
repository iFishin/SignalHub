import type { IconName } from "./icons";
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

/** 左栏的五个折叠组。id 与语言无关：切语言不该让展开状态认不出自己。 */
export type GroupId = "watch" | "lamp" | "notify" | "connect" | "help";

export interface TabEntry {
  id: Tab;
  label: string;
  /** 图标只是扫读用的锚点，不参与选中态（选中态靠字重，见 settings.css）。 */
  icon: IconName;
}

export interface TabGroup {
  id: GroupId;
  title: string;
  entries: TabEntry[];
}

/**
 * 每个页签属于哪一组。`system` 沉底、不属于任何组，所以是 null。
 *
 * 这是折叠导航的唯一真值；`tabGroups` 里每条记录的归属由
 * `settingsNavigation.test.ts` 盯着，加页签忘了登记会直接测试失败。
 */
export const TAB_GROUP: Record<Tab, GroupId | null> = {
  status: "watch",
  history: "watch",
  messages: "watch",
  look: "lamp",
  behavior: "lamp",
  groups: "lamp",
  alert: "notify",
  hooks: "notify",
  connect: "connect",
  machines: "connect",
  callme: "connect",
  guide: "help",
  demo: "help",
  system: null,
};

export function groupForTab(tab: Tab): GroupId | null {
  return TAB_GROUP[tab];
}

/** 搜索别名是导航数据，不是界面全文索引。中英混放是有意的。 */
export const TAB_ALIASES: Record<Tab, string[]> = {
  status: ["状态", "现在", "时间线", "status", "now", "timeline"],
  history: ["历史", "近 7 天", "统计", "今日时长", "history", "stats", "week"],
  messages: ["留言", "收信", "评论", "反馈", "call me", "callme", "message", "feedback"],
  look: ["外观", "颜色", "形状", "大小", "透明度", "预设", "皮肤", "主题", "效果", "呼吸", "脉冲", "扫光", "渐隐", "光晕", "圆角", "描边", "底色", "字号", "质感", "轮廓", "胶囊", "拱形", "菱形", "方形", "灯珠", "牌形", "上边", "下边", "边灯", "角灯", "六灯位", "边框光", "粗细", "明暗", "会话标题", "标题", "组名", "智能体名", "agent 名称", "穿透角灯", "行尾四格", "第 1 格", "刻线", "短刻", "长刻", "拼接板", "折叠", "折叠排列", "折叠内容", "竖排", "横排", "序号", "状态词", "一颗灯", "look", "color", "size", "preset", "skin", "theme", "effect", "breathe", "pulse", "sweep", "fade", "glow", "radius", "border", "tint", "font", "outline", "diamond", "capsule", "square", "shape", "edge", "corner", "tick", "slot", "board", "fold", "vertical", "horizontal", "index", "frame light", "title", "label", "agent name"],
  behavior: ["行为", "静默超时", "超时", "安静模式", "点击穿透", "触发", "什么时候亮", "上边", "下边", "左上", "右上", "右下", "左下", "边灯", "角灯", "六灯位", "行尾四格", "第 1 格", "刻线", "短刻", "长刻", "指定状态", "指定时段", "指定时间", "沉默到期", "置顶触发", "behavior", "ttl", "timeout", "trigger", "schedule", "state", "time", "slot", "tick"],
  groups: ["分组", "置顶", "多灯", "每组分灯", "归位", "灯的位置", "散落", "拖动", "位置记忆", "group", "pin", "lamp count", "reset", "position", "drag"],
  alert: ["提醒", "通知", "勿扰", "提示音", "声音", "状态", "alert", "notify", "dnd", "sound", "state"],
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
      id: "watch",
      title: m.railWatch,
      entries: [
        { id: "status", label: m.tabStatus, icon: "pulse" },
        { id: "history", label: m.tabHistory, icon: "clock" },
        { id: "messages", label: m.tabMessages, icon: "mail" },
      ],
    },
    {
      id: "lamp",
      title: m.railLamp,
      entries: [
        { id: "look", label: m.tabLook, icon: "eye" },
        { id: "behavior", label: m.tabBehavior, icon: "sliders" },
        { id: "groups", label: m.tabGroups, icon: "layers" },
      ],
    },
    {
      id: "notify",
      title: m.railNotify,
      entries: [
        { id: "alert", label: m.tabAlert, icon: "bell" },
        { id: "hooks", label: m.tabHooks, icon: "forward" },
      ],
    },
    {
      id: "connect",
      title: m.railConnect,
      entries: [
        { id: "connect", label: m.tabConnect, icon: "plug" },
        { id: "machines", label: m.tabMachines, icon: "monitors" },
        { id: "callme", label: m.tabCallMe, icon: "link" },
      ],
    },
    {
      id: "help",
      title: m.railHelp,
      entries: [
        { id: "guide", label: m.tabGuide, icon: "book" },
        { id: "demo", label: m.tabDemo, icon: "play" },
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
