import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import { useSettings, type Settings } from "./useSettings";
import type { SignalState } from "./types";

export type Lang = "zh" | "en";

/**
 * 全部界面文案。加一条就得两种语言都加 —— 类型系统会盯着。
 *
 * 用「一张对象表」而不是 `t("key")` 那种查表：`MESSAGES[lang].look` 在编译期
 * 就能保证两种语言都有这一条，而字符串键查表要跑起来才发现漏了。带值的文案
 * 就在表里放一个函数（如 `groupsAndSessions`），接口会把参数个数也定死。
 */
export interface Messages {
  /** 状态名。灯球和设置面板共用，避免两处措辞不一致。 */
  signal: Record<SignalState, string>;
  /** 状态名的短标签，给钩子过滤条件的那些小按钮用（10 个要挤在一行里换行）。 */
  shortSignal: Record<SignalState, string>;

  // 页签
  tabStatus: string;
  tabLook: string;
  tabStyle: string;
  tabAlert: string;
  tabConnect: string;
  tabHooks: string;
  tabSystem: string;

  // 通用动作
  restoreAll: string;
  mute: string;
  muted: string;
  hidden: string;
  delete: string;

  // 摘要 / 头部
  hiddenOnLamp: (n: number) => string;
  mutedGroups: (n: number) => string;
  groupsAndSessions: (groups: number, sessions: number) => string;
  sessions: (n: number) => string;
  seconds: (n: number) => string;
  oneMinute: string;
  today: string;
  pausedBanner: string;

  // 状态页
  timelineStart: string;
  timelineNow: string;
  emptySessions: string;
  hiddenBadgeHint: string;
  mutedBadgeHint: string;
  muteHint: string;

  // 外观页
  presets: string;
  form: string;
  expanded: string;
  singleLight: string;
  size: string;
  shape: string;
  palette: string;
  signOpacity: string;
  blinkSpeed: string;
  onSign: string;
  quietMode: string;
  lookHint: string;
  sizeSmall: string;
  sizeMedium: string;
  sizeLarge: string;
  shapeCircle: string;
  shapeRounded: string;
  shapeRing: string;
  paletteDefault: string;
  paletteColorblind: string;
  paletteVivid: string;
  fieldName: string;
  fieldState: string;
  fieldAge: string;
  fieldMessage: string;

  // 灯效页
  follow: string;
  solid: string;
  state: string;
  rhythm: string;
  lightColor: string;
  lightShape: string;
  blinkInterval: string;
  resetToGlobal: string;
  styleHint: string;

  // 提醒页
  notifyWhenNeeded: string;
  soundWithNotify: string;
  alertAfter: string;
  alertAfterHint: string;
  quietHours: string;
  alertHint: string;
  delayNow: string;

  // 接入页
  pauseMonitoring: string;
  connectHint: string;
  outboundHooks: string;
  configure: string;
  connectHookHint: string;
  port: string;

  // 钩子页
  noHooks: string;
  hookN: (n: number) => string;
  noTarget: string;
  type: string;
  command: string;
  addHook: string;
  hooksHint: string;
  commandPlaceholder: string;

  // 系统页
  alwaysOnTop: string;
  trayFollowsState: string;
  launchAtLogin: string;
  clickThrough: string;
  systemHintTray: string;
  systemHintPassThrough: (shortcut: string) => string;
  shortcutFallback: string;
  language: string;
  langAuto: string;
  langZh: string;
  langEn: string;

  quit: string;

  // 灯球 tooltip
  lampHint: string;
  groupsCount: (n: number) => string;

  // 外观预设
  presetLight: string;
  presetLightHint: string;
  presetCompact: string;
  presetCompactHint: string;
  presetStandard: string;
  presetStandardHint: string;
  presetBold: string;
  presetBoldHint: string;

  // 灯效预设
  stylePresetFollow: string;
  stylePresetFollowHint: string;
  stylePresetShape: string;
  stylePresetShapeHint: string;
  stylePresetRhythm: string;
  stylePresetRhythmHint: string;
  stylePresetCalm: string;
  stylePresetCalmHint: string;
}

/** 英文里数词要分单复数，中文不分。 */
function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

export const MESSAGES: Record<Lang, Messages> = {
  zh: {
    signal: {
      // 三档「不在工作」要分清：完全没有智能体 / 智能体不说话了 / 用户主动暂停
      offline: "没有智能体在上报",
      stale: "会话已沉默",
      off: "已暂停监控",
      idle: "空闲",
      done: "已完成",
      thinking: "推理中",
      working: "执行中",
      attention: "需要关注",
      permission: "需要授权",
      error: "出错 / 阻塞",
    },
    shortSignal: {
      idle: "空闲",
      thinking: "推理",
      working: "执行",
      done: "完成",
      attention: "关注",
      permission: "授权",
      error: "出错",
      stale: "沉默",
      offline: "离线",
      off: "暂停",
    },

    tabStatus: "状态",
    tabLook: "外观",
    tabStyle: "灯效",
    tabAlert: "提醒",
    tabConnect: "接入",
    tabHooks: "钩子",
    tabSystem: "系统",

    restoreAll: "全部恢复",
    mute: "静音",
    muted: "已静音",
    hidden: "已隐藏",
    delete: "删除",

    hiddenOnLamp: (n) => `灯上隐藏 ${n} 组`,
    mutedGroups: (n) => `静音 ${n} 组`,
    groupsAndSessions: (groups, sessions) => `${groups} 组 · ${sessions} 个会话`,
    sessions: (n) => `${n} 个会话`,
    seconds: (n) => `${n} 秒`,
    oneMinute: "1 分钟",
    today: "今日",
    pausedBanner: "监控已暂停：灯球显示熄灭，但会话仍在记录",

    timelineStart: "30 分钟前",
    timelineNow: "现在",
    emptySessions: "暂无会话。接入智能体或调用上报接口后，这里会列出来。",
    hiddenBadgeHint: "点一下让它在灯上重新显示",
    mutedBadgeHint: "点一下取消静音",
    muteHint: "不再因为这一组弹通知 / 出声",

    presets: "预设",
    form: "形态",
    expanded: "展开牌面",
    singleLight: "一颗灯",
    size: "大小",
    shape: "形状",
    palette: "配色方案",
    signOpacity: "背景透明度",
    blinkSpeed: "闪烁节奏",
    onSign: "牌上显示",
    quietMode: "安静模式",
    lookHint:
      "形状只影响灯珠：改成圆角方或环会让 permission / error 的方角区分失效，此时建议配合色盲友好配色。",
    sizeSmall: "小",
    sizeMedium: "中",
    sizeLarge: "大",
    shapeCircle: "圆",
    shapeRounded: "圆角方",
    shapeRing: "环",
    paletteDefault: "默认",
    paletteColorblind: "色盲友好",
    paletteVivid: "高饱和",
    fieldName: "组名",
    fieldState: "状态",
    fieldAge: "已持续",
    fieldMessage: "消息",

    follow: "跟随",
    solid: "常亮",
    state: "状态",
    rhythm: "节奏",
    lightColor: "灯珠颜色",
    lightShape: "灯珠形状",
    blinkInterval: "闪烁间隔",
    resetToGlobal: "恢复成跟随全局",
    styleHint: "「跟随」= 用「外观」页选的那两项（形状、节奏倍率）。",

    notifyWhenNeeded: "需要时弹通知",
    soundWithNotify: "通知带提示音",
    alertAfter: "持续多久才提醒",
    alertAfterHint: "状态要持续这么久才弹通知，一闪而过的忽略掉",
    quietHours: "勿扰时段",
    alertHint:
      "需要授权、需要关注或出错时弹一条系统通知。声音挂在通知上，所以关掉通知也就没有声音。勿扰时段里不弹也不出声，灯照常显示 —— 而且不补发：早上醒来收到一堆昨晚的通知，正是勿扰想避免的。",
    delayNow: "立刻",

    pauseMonitoring: "暂停监控",
    connectHint:
      "暂停后灯球显示为熄灭，但会话仍在记录 —— 恢复时立刻回到真实状态。弹不弹通知在「提醒」页。",
    outboundHooks: "出站钩子",
    configure: "去配置",
    connectHookHint: "状态变化时可以 POST 到某个地址或跑一条命令 —— 在「钩子」页配置。",
    port: "端口",

    noHooks: "还没有钩子。加一条，就能在状态变化时 POST 到某个地址或跑一条命令。",
    hookN: (n) => `钩子 ${n}`,
    noTarget: "未填目标",
    type: "类型",
    command: "命令",
    addHook: "+ 添加钩子",
    hooksHint:
      "状态全不选 = 任何变化都触发。命令通过 sh -c 执行，超时 10 秒 —— 配置文件在你手里，所以这里没有额外的开关。",
    commandPlaceholder: "say 完成",

    alwaysOnTop: "保持在最前",
    trayFollowsState: "托盘跟随状态",
    launchAtLogin: "开机自启",
    clickThrough: "点击穿透",
    systemHintTray:
      "托盘跟随状态时，菜单栏那颗点会变成当前状态的颜色 —— 不看悬浮灯也知道现在什么情况。关掉就回到单色字形，跟着菜单栏明暗自动反色。",
    systemHintPassThrough: (shortcut) =>
      `穿透后圆点不再响应鼠标，按 ${shortcut} 或从托盘菜单退出。该模式不会保留到下次启动。`,
    shortcutFallback: "快捷键",
    language: "语言",
    langAuto: "跟随系统",
    langZh: "中文",
    langEn: "English",

    quit: "退出 SignalHub",

    lampHint: "左键折叠 / 展开 · 右键单组菜单",
    groupsCount: (n) => `（${n} 组）`,

    presetLight: "单灯",
    presetLightHint: "折叠成一颗灯，只有颜色，最不打扰",
    presetCompact: "紧凑",
    presetCompactHint: "窄一条，留组名和状态，不要消息",
    presetStandard: "标准",
    presetStandardHint: "组名 / 状态 / 已持续 / 消息都显示",
    presetBold: "醒目",
    presetBoldHint: "更大更实，闪得也快一点",

    stylePresetFollow: "跟随全局",
    stylePresetFollowHint:
      "清掉所有逐状态覆盖，颜色用配色方案、形状和节奏用「外观」页那两项",
    stylePresetShape: "形状区分",
    stylePresetShapeHint:
      "需要授权 / 出错画成圆角方 —— 红绿是红绿色盲最难分的一对，而这两个最需要你注意",
    stylePresetRhythm: "节奏分级",
    stylePresetRhythmHint:
      "每个状态一套自己的节奏：推理快、执行慢、出错频闪，不在工作的常亮",
    stylePresetCalm: "只在需要时闪",
    stylePresetCalmHint: "干活的那几个状态都常亮，只有需要关注 / 授权 / 出错才闪",
  },

  en: {
    signal: {
      offline: "No agents reporting",
      stale: "Session went quiet",
      off: "Monitoring paused",
      idle: "Idle",
      done: "Done",
      thinking: "Thinking",
      working: "Working",
      attention: "Needs attention",
      permission: "Needs permission",
      error: "Error / blocked",
    },
    shortSignal: {
      idle: "Idle",
      thinking: "Think",
      working: "Work",
      done: "Done",
      attention: "Notice",
      permission: "Approve",
      error: "Error",
      stale: "Silent",
      offline: "Offline",
      off: "Paused",
    },

    tabStatus: "Status",
    tabLook: "Look",
    tabStyle: "Style",
    tabAlert: "Alerts",
    tabConnect: "Connect",
    tabHooks: "Hooks",
    tabSystem: "System",

    restoreAll: "Restore all",
    mute: "Mute",
    muted: "Muted",
    hidden: "Hidden",
    delete: "Delete",

    hiddenOnLamp: (n) => `${plural(n, "group", "groups")} hidden on lamp`,
    mutedGroups: (n) => `${plural(n, "group", "groups")} muted`,
    groupsAndSessions: (groups, sessions) =>
      `${plural(groups, "group", "groups")} · ${plural(sessions, "session", "sessions")}`,
    sessions: (n) => plural(n, "session", "sessions"),
    seconds: (n) => `${n}s`,
    oneMinute: "1 min",
    today: "Today",
    pausedBanner: "Monitoring paused: the lamp shows as off, but sessions are still recorded",

    timelineStart: "30 min ago",
    timelineNow: "Now",
    emptySessions: "No sessions. Connect an agent or call the report API and they will show up here.",
    hiddenBadgeHint: "Click to show it on the lamp again",
    mutedBadgeHint: "Click to unmute",
    muteHint: "Stop notifications and sound for this group",

    presets: "Presets",
    form: "Form",
    expanded: "Expanded",
    singleLight: "Single light",
    size: "Size",
    shape: "Shape",
    palette: "Palette",
    signOpacity: "Sign opacity",
    blinkSpeed: "Blink speed",
    onSign: "On the sign",
    quietMode: "Quiet mode",
    lookHint:
      "Shape affects only the light: rounded or ring drops the square-corner cue for permission / error, so pair it with the colourblind palette.",
    sizeSmall: "Small",
    sizeMedium: "Medium",
    sizeLarge: "Large",
    shapeCircle: "Circle",
    shapeRounded: "Rounded",
    shapeRing: "Ring",
    paletteDefault: "Default",
    paletteColorblind: "Colourblind",
    paletteVivid: "Vivid",
    fieldName: "Group",
    fieldState: "State",
    fieldAge: "Age",
    fieldMessage: "Message",

    follow: "Follow",
    solid: "Solid",
    state: "State",
    rhythm: "Rhythm",
    lightColor: "Light colour",
    lightShape: "Light shape",
    blinkInterval: "Blink interval",
    resetToGlobal: "Reset to follow global",
    styleHint: '"Follow" = use the two chosen on the Look tab (shape, blink speed).',

    notifyWhenNeeded: "Notify when needed",
    soundWithNotify: "Sound with notification",
    alertAfter: "Alert after",
    alertAfterHint: "A state must last this long before notifying; brief ones are ignored",
    quietHours: "Quiet hours",
    alertHint:
      "A system notification when approval, attention or error happens. Sound rides on the notification, so turning notifications off silences it too. During quiet hours nothing pops or sounds and the lamp still shows — and nothing is replayed: waking to a pile of last night's notifications is exactly what quiet hours avoids.",
    delayNow: "Now",

    pauseMonitoring: "Pause monitoring",
    connectHint:
      "Paused, the lamp shows as off, but sessions are still recorded — resuming returns to the real state at once. Notification settings are on the Alerts tab.",
    outboundHooks: "Outbound hooks",
    configure: "Configure",
    connectHookHint: "On state changes you can POST to a URL or run a command — set it up on the Hooks tab.",
    port: "Port",

    noHooks: "No hooks yet. Add one to POST to a URL or run a command on state changes.",
    hookN: (n) => `Hook ${n}`,
    noTarget: "No target",
    type: "Type",
    command: "Command",
    addHook: "+ Add hook",
    hooksHint:
      "No states selected = triggers on any change. Commands run via sh -c with a 10-second timeout — the config file is yours, so there is no extra switch here.",
    commandPlaceholder: "say done",

    alwaysOnTop: "Always on top",
    trayFollowsState: "Tray follows state",
    launchAtLogin: "Launch at login",
    clickThrough: "Click-through",
    systemHintTray:
      "With tray-follows-state the menu-bar dot takes the current state's colour — you can tell what's going on without looking at the lamp. Turned off, it's a monochrome glyph that inverts with the menu bar's light/dark.",
    systemHintPassThrough: (shortcut) =>
      `In click-through the dot stops responding to the mouse; press ${shortcut} or quit from the tray menu. The mode is not kept for the next launch.`,
    shortcutFallback: "the shortcut",
    language: "Language",
    langAuto: "System",
    langZh: "中文",
    langEn: "English",

    quit: "Quit SignalHub",

    lampHint: "Left-click to collapse / expand · right-click for a group menu",
    groupsCount: (n) => ` (${plural(n, "group", "groups")})`,

    presetLight: "Single light",
    presetLightHint: "Folds to one light, colour only, least intrusive",
    presetCompact: "Compact",
    presetCompactHint: "Narrow strip: group and state, no message",
    presetStandard: "Standard",
    presetStandardHint: "Shows group / state / age / message",
    presetBold: "Bold",
    presetBoldHint: "Bigger and more solid, blinks a bit faster",

    stylePresetFollow: "Follow global",
    stylePresetFollowHint:
      "Clears every per-state override: colours from the palette, shape and rhythm from the Look tab",
    stylePresetShape: "Shape-coded",
    stylePresetShapeHint:
      "Approval / error drawn as rounded squares — red vs green is the hardest pair for red-green colour blindness, and these two need your attention most",
    stylePresetRhythm: "Rhythm tiers",
    stylePresetRhythmHint:
      "A rhythm per state: thinking fast, working slow, error strobing, non-working solid",
    stylePresetCalm: "Blink only when needed",
    stylePresetCalmHint: "The working states stay solid; only attention / approval / error blink",
  },
};

/** 跟随系统。`navigator.language` 以 zh 开头就是中文，其余一律英文。 */
export function detectLang(): Lang {
  if (typeof navigator === "undefined") return "en";
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

/** 从 settings.locale 解析出实际语言。 */
export function resolveLang(locale: Settings["locale"]): Lang {
  return locale === "auto" ? detectLang() : locale;
}

/** 当前实际语言。跟随设置里的 locale，settings 一变就跟着变。 */
export function useLang(): Lang {
  return resolveLang(useSettings().locale);
}

/** 当前语言的文案表。读 useSettings()，不另开一套 context —— 那会变成第二个真值来源。 */
export function useMessages(): Messages {
  return MESSAGES[useLang()];
}

/**
 * 把解析好的语言推给 Rust。
 *
 * 为什么由前端推：Rust 那边判断不了系统语言（它连时区都没有），而「用户看到的是
 * 哪种语言」只有这里知道。所以由前端把 locale 解析成 zh / en，再推给 Rust。
 *
 * 与 useLocalHour 同构：只在语言真的变了的时候推一次（依赖是解析后的 lang，
 * 而不是整份 settings —— 后者每次渲染都是新对象）。
 */
export function useLangPush(): void {
  const lang = useLang();
  useEffect(() => {
    if (!inTauri) return;
    invoke("set_lang", { lang }).catch((err) => console.error("set_lang 失败", err));
  }, [lang]);
}
