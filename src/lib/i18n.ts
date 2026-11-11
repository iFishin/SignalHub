import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import { useSettings, type HttpPreset, type Settings } from "./useSettings";
import type { SignalState } from "./types";
import type { DemoGroupId, DemoScenarioId } from "./demo";
import type { HealthCheck, HealthLevel } from "./health";
import type { DigestFact } from "./digest";
import type { InsightFact } from "./insights";
import type { DraftProblem } from "./customAgentForm";

export type Lang = "zh" | "en";

/**
 * 一次接入 / 卸载改了什么，用来拼一行结果。
 *
 * 只放**数量**，不放 Rust 那边的字符串数组 —— 文案要分语言，而数组内容
 * （事件名）是给机器看的，界面上只关心「加了几条、改了几条、摘了几个」。
 */
export interface HookResult {
  added: number;
  updated: number;
  already: number;
  removed: number;
}

/* ---- 体检 / 复盘的措辞 ----
 *
 * 这两块刻意写成模块级函数，而不是塞进 `MESSAGES` 的对象字面量里：它们要按
 * 结构化事实（`HealthCheck` / `DigestFact`）分岔，写成一张对象表反而要第二套
 * 查表。界面和「复制报告」都调同一个函数，所以两处不可能说两套话。
 *
 * 缩写和 `SettingsPanel.formatDuration` 同型（`45s` / `12m` / `2h14m`）——
 * 数字写法不是语言问题，别在两种语言里各实现一遍。 */
function humanDuration(seconds: number): string {
  const value = Math.max(0, Math.round(seconds));
  if (value < 60) return `${value}s`;
  const minutes = Math.round(value / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h${rest}m`;
}

function humanWait(ms: number): string {
  // `<1s` 两种语言都这么写，别把中文硬塞进英文那句里。
  return ms < 1000 ? "<1s" : humanDuration(ms / 1000);
}

function zhHealthLine(check: HealthCheck): string {
  const onOff = (value: string) => (value === "on" ? "开" : "关");
  const perms: Record<string, string> = {
    granted: "已允许",
    denied: "已被拒绝",
    prompt: "还没问过系统",
    unknown: "读不出来",
  };
  switch (check.id) {
    case "permission":
      return `系统通知权限：${perms[check.value] ?? check.value}`;
    case "notify":
      return `系统通知：${onOff(check.value)}`;
    case "sound":
      return `提示音：${onOff(check.value)}`;
    case "alertDelay":
      return check.value === "0"
        ? "提醒延迟：状态一变就提醒"
        : `提醒延迟：要等 ${check.value}s 才提醒`;
    case "quiet":
      return check.value === "on"
        ? "勿扰时段：开着（这段时间通知会被压住）"
        : "勿扰时段：没开";
    case "muted":
      return check.value === "0" ? "静音组：没有" : `静音组：${check.value} 个`;
    case "install":
      return check.value === "0"
        ? "已接入的智能体：一个都没有"
        : `已接入的智能体：${check.value} 个`;
    case "installBroken":
      return `跑不起来的接入：${check.value}（配置里那条命令已失效，重装一次就好）`;
    case "installStale":
      return `需要重装的接入：${check.value}（升级后没重装，行为还停在旧版本）`;
    case "receipt":
      return check.value === "0"
        ? "最近一次上报：一次都没收到过"
        : `最近一次上报：${check.value} 前`;
    case "approve":
      return check.level === "warn"
        ? "从灯上审批：已开，但没有 Claude Code 的 HTTP 式接入 —— 确认框不会到灯上"
        : check.level === "ok"
          ? "从灯上审批：已开，Claude Code 走 HTTP 式"
          : "从灯上审批：没开";
    case "custom":
      return check.value === "-"
        ? "自定义预设：没启用"
        : check.value === "0"
          ? "自定义预设：已启用，读得懂"
          : `自定义预设：${check.value} 处读不懂（会被静默忽略）`;
    case "shortcut":
      return check.value === "0"
        ? "全局快捷键：都抢到了"
        : `全局快捷键：${check.value} 个没抢到（多半被别的程序占了）`;
    case "hookLog":
      return check.value === "0"
        ? "hook 日志：还没有记录"
        : `hook 日志：最近有 ${check.value} 行`;
    case "sessions":
      return `当前会话：${check.value} 个`;
  }
}

function enHealthLine(check: HealthCheck): string {
  const onOff = (value: string) => (value === "on" ? "on" : "off");
  const perms: Record<string, string> = {
    granted: "Granted",
    denied: "Denied",
    prompt: "Not asked yet",
    unknown: "Unknown",
  };
  switch (check.id) {
    case "permission":
      return `Notification permission: ${perms[check.value] ?? check.value}`;
    case "notify":
      return `Notifications: ${onOff(check.value)}`;
    case "sound":
      return `Alert sound: ${onOff(check.value)}`;
    case "alertDelay":
      return check.value === "0"
        ? "Alert delay: fires as soon as the state changes"
        : `Alert delay: waits ${check.value}s before firing`;
    case "quiet":
      return check.value === "on"
        ? "Quiet hours: on (notifications are suppressed during this window)"
        : "Quiet hours: off";
    case "muted":
      return check.value === "0" ? "Muted groups: none" : `Muted groups: ${check.value}`;
    case "install":
      return check.value === "0"
        ? "Installed agents: none"
        : `Installed agents: ${check.value}`;
    case "installBroken":
      return `Broken installs: ${check.value} (the configured command no longer resolves; reinstall)`;
    case "installStale":
      return `Outdated installs: ${check.value} (written by an older version; reinstall)`;
    case "receipt":
      return check.value === "0"
        ? "Last report: never received one"
        : `Last report: ${check.value} ago`;
    case "approve":
      return check.level === "warn"
        ? "Approve from lamp: on, but no Claude Code HTTP install — prompts will not reach the lamp"
        : check.level === "ok"
          ? "Approve from lamp: on, Claude Code is on HTTP mode"
          : "Approve from lamp: off";
    case "custom":
      return check.value === "-"
        ? "Custom agent presets: not in use"
        : check.value === "0"
          ? "Custom agent presets: loaded, no problems"
          : `Custom agent presets: ${check.value} problem(s), silently ignored`;
    case "shortcut":
      return check.value === "0"
        ? "Global shortcuts: all registered"
        : `Global shortcuts: ${check.value} failed to register (likely taken by another app)`;
    case "hookLog":
      return check.value === "0"
        ? "Hook log: no entries yet"
        : `Hook log: ${check.value} recent line(s)`;
    case "sessions":
      return `Active sessions: ${check.value}`;
  }
}

function zhDigestFact(fact: DigestFact): string {
  switch (fact.kind) {
    case "empty":
      return "今天还没有任何记录。";
    case "total":
      return `今天记录了 ${humanDuration(fact.seconds)} 的状态时长（空闲 / 离线也算在内）。`;
    case "top":
      return `占得最多的是「${fact.label}」，${humanDuration(fact.seconds)}。`;
    case "busy":
      return `真正在跑（思考 + 执行）${humanDuration(fact.seconds)}。`;
    case "wait":
      return `等你处理（授权 / 需要输入）${humanDuration(fact.seconds)}。`;
    case "approvals": {
      const other = fact.total - fact.allow - fact.deny - fact.expired;
      const tail = other > 0 ? `、其他 ${other}` : "";
      return `审批 ${fact.total} 次：允许 ${fact.allow}、拒绝 ${fact.deny}、超时 ${fact.expired}${tail}；最久等了 ${humanWait(fact.longestWaitMs)}。`;
    }
    case "compare":
      return fact.deltaSeconds > 0
        ? `比之前 ${fact.days} 天的日均多 ${humanDuration(fact.deltaSeconds)}。`
        : fact.deltaSeconds < 0
          ? `比之前 ${fact.days} 天的日均少 ${humanDuration(-fact.deltaSeconds)}。`
          : `和之前 ${fact.days} 天的日均差不多。`;
  }
}

function enDigestFact(fact: DigestFact): string {
  switch (fact.kind) {
    case "empty":
      return "Nothing recorded today yet.";
    case "total":
      return `${humanDuration(fact.seconds)} of aggregate state recorded today (idle / offline included).`;
    case "top":
      return `Most of it was "${fact.label}" — ${humanDuration(fact.seconds)}.`;
    case "busy":
      return `Actually running (thinking + working): ${humanDuration(fact.seconds)}.`;
    case "wait":
      return `Waiting on you (permission / attention): ${humanDuration(fact.seconds)}.`;
    case "approvals": {
      const other = fact.total - fact.allow - fact.deny - fact.expired;
      const tail = other > 0 ? `, other ${other}` : "";
      return `${fact.total} approvals: ${fact.allow} allowed, ${fact.deny} denied, ${fact.expired} expired${tail}; longest wait ${humanWait(fact.longestWaitMs)}.`;
    }
    case "compare":
      return fact.deltaSeconds > 0
        ? `${humanDuration(fact.deltaSeconds)} more than the daily average of the previous ${fact.days} day(s).`
        : fact.deltaSeconds < 0
          ? `${humanDuration(-fact.deltaSeconds)} less than the daily average of the previous ${fact.days} day(s).`
          : `About the same as the daily average of the previous ${fact.days} day(s).`;
  }
}

function zhInsightFact(fact: InsightFact): string {
  switch (fact.kind) {
    case "empty":
      return "这段时间还没有任何事件记录。";
    case "volume":
      return `近 ${fact.days} 天有 ${fact.sessions} 个会话活跃过：${fact.finished} 个完成、${fact.failed} 个失败。`;
    case "peakDay":
      return `${fact.day} 最忙，有 ${fact.sessions} 个会话活跃。`;
    case "topProject":
      return `出场最多的是「${fact.name}」，${fact.sessions} 个会话。`;
    case "topAgent":
      return `最活跃的是 ${fact.name}，${fact.sessions} 个会话。`;
    case "deniedTool":
      return `拒绝过 ${fact.total} 次，最常拒绝的是 ${fact.tool}（${fact.count} 次）。`;
    case "wait":
      return `${fact.count} 次审批一共等了 ${humanWait(fact.totalMs)}，中位数 ${humanWait(fact.medianMs)}。`;
  }
}

function enInsightFact(fact: InsightFact): string {
  switch (fact.kind) {
    case "empty":
      return "No events recorded in this window yet.";
    case "volume":
      return `${fact.sessions} sessions were active over the last ${fact.days} days: ${fact.finished} finished, ${fact.failed} failed.`;
    case "peakDay":
      return `${fact.day} was the busiest, with ${fact.sessions} sessions active.`;
    case "topProject":
      return `Most sessions came from "${fact.name}" — ${fact.sessions}.`;
    case "topAgent":
      return `${fact.name} was the most active, with ${fact.sessions} sessions.`;
    case "deniedTool":
      return `${fact.total} denials; ${fact.tool} was denied most often (${fact.count}).`;
    case "wait":
      return `${fact.count} approvals took ${humanWait(fact.totalMs)} in total, median ${humanWait(fact.medianMs)}.`;
  }
}

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
  /** 左栏的分组标题。页的「轴」在这里写着（见 lib/settingsNavigation.ts）。 */
  railWatch: string;
  railLamp: string;
  railNotify: string;
  railConnect: string;
  railHelp: string;
  tabStatus: string;
  tabHistory: string;
  tabMessages: string;
  /** 「历史」页的正文 */
  historyTitle: string;
  historyHint: string;
  historyToday: string;
  historyEmpty: string;
  tabLook: string;
  tabBehavior: string;
  tabGroups: string;
  tabAlert: string;
  tabConnect: string;
  tabMachines: string;
  tabCallMe: string;
  tabHooks: string;
  tabGuide: string;
  tabSystem: string;
  tabDemo: string;
  /** 接入页的页内子页。 */
  connectTabAgents: string;
  connectTabSources: string;
  connectTabNetwork: string;
  /** 系统页的页内子页。 */
  systemTabGeneral: string;
  systemTabUpdate: string;
  systemTabData: string;
  /**
   * 每个全局页签的一句任务说明（统一页头的第二行）。键集和
   * lib/settingsNavigation.ts 的 Tab 一致，新增页签时 TS 会提醒这里补文案。
   */
  pageHint: Record<
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
    | "system",
    string
  >;

  // 左栏搜索
  /** 搜索框的占位符 */
  railSearch: string;
  /** 一个页签都没匹配上 */
  railSearchEmpty: string;

  // 首次启动引导（占位牌上那一枚）
  lampFirstRun: string;
  lampFirstRunDemo: string;
  lampFirstRunDemoHint: string;

  // 演示
  demoTitle: string;
  demoIntro: string;
  demoBackToSettings: string;
  demoNotReal: string;
  demoGroup: Record<DemoGroupId, string>;
  /** 场景 chip 上的名字。键是 demo.ts 里的 DemoScenarioId。 */
  demoScenario: Record<DemoScenarioId, string>;
  /** 每一步的标题 + 说明。键是 demo.ts 里的 DemoStep.id。 */
  demoStep: Record<string, { title: string; body: string }>;
  demoStepCount: (index: number, total: number) => string;
  demoPrev: string;
  demoNext: string;
  demoPlay: string;
  demoPause: string;
  demoReplay: string;
  /** 状态页「第一次用？」那块上的 CTA */
  demoCta: string;
  demoCtaHint: string;

  // 指南（引导手册）
  guideEntry: string;
  guideWhatTitle: string;
  guideWhat: string;
  guideStepsTitle: string;
  guideStep1: string;
  guideStep2: string;
  guideStep3: string;
  guideFaqTitle: string;
  guideFaqNoLightQ: string;
  guideFaqNoLightA: string;
  guideFaqNoNotifyQ: string;
  guideFaqNoNotifyA: string;

  // 系统通知权限
  notifyPermission: string;
  permGranted: string;
  permDenied: string;
  permPrompt: string;
  permUnknown: string;
  /** Windows 上 AUMID 没注册好：权限显示「已允许」也不能证明通知发得出去。 */
  permUnavailable: string;
  permRequest: string;
  permRetry: string;
  permHint: string;
  permUnavailableHint: string;
  permBackendError: (detail: string) => string;
  /** 一键测试通知：按钮、通知本身、以及发完之后的解释 */
  notifyTest: string;
  notifyTestTitle: string;
  notifyTestBody: string;
  notifyTestResult: (perm: string) => string;

  // 系统体检（设置 → 系统）
  healthTitle: string;
  healthHint: string;
  healthRun: string;
  healthRerun: string;
  healthSummary: (fail: number, warn: number) => string;
  healthVerdict: (level: HealthLevel) => string;
  healthLevel: (level: HealthLevel) => string;
  healthLine: (check: HealthCheck) => string;
  healthReportHeader: (at: string, version: string) => string;

  // 今日复盘（设置 → 历史）
  digestTitle: string;
  digestHint: string;
  digestFact: (fact: DigestFact) => string;
  digestReportHeader: (day: string) => string;

  // 本周洞察（设置 → 历史，紧接今日复盘）
  insightTitle: string;
  insightHint: string;
  insightFact: (fact: InsightFact) => string;
  insightReportHeader: (days: number) => string;
  insightError: string;
  /** 撞到 5000 条上限时追加的一句，别把截断后的数字当全量结论。 */
  insightTruncated: string;

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
  displayName: string;
  displayNamePlaceholder: (raw: string) => string;
  pin: string;
  pinHint: string;
  feedTitle: string;
  feedEmpty: string;

  // 外观页
  presets: string;
  form: string;
  expanded: string;
  singleLight: string;
  lampLayout: string;
  layoutSingle: string;
  layoutPerGroup: string;
  layoutHint: string;
  size: string;
  shape: string;
  palette: string;
  signOpacity: string;
  signOpacityHint: string;
  signWidth: string;
  signHeight: string;
  blinkSpeed: string;
  /** 「灵动效果」开关及其说明（柔光呼吸 + 弹簧转场）。 */
  onSign: string;
  quietMode: string;
  snapToEdge: string;
  snapToEdgeHint: string;
  lookHint: string;
  /** 行尾那个 ↺ 的提示语。整份设置的默认值只有一份（`DEFAULTS`）。 */
  resetToDefault: string;
  /** 外观页里那张「牌长这样」的实时预览的标题。 */
  previewTitle: string;
  previewHint: string;
  /** 预览牌里那个示例组名 / 示例消息（就是画出来的样子，不是真的数据）。 */
  previewGroup: string;
  previewMessage: string;
  sizeSmall: string;
  sizeMedium: string;
  sizeLarge: string;
  shapeCircle: string;
  shapeRounded: string;
  shapeRing: string;
  paletteDefault: string;
  paletteColorblind: string;
  paletteVivid: string;
  /** 外观页的两个子页。「基础」放尺寸 / 形态 / 配色，「质感」放牌的画法。 */
  lookTabBasic: string;
  lookTabTexture: string;
  /** 质感子页顶上那句「只改画法、不动几何」。 */
  textureHint: string;
  signRadius: string;
  radiusSquare: string;
  radiusSoft: string;
  radiusStandard: string;
  radiusRound: string;
  signBorder: string;
  borderNone: string;
  borderHairline: string;
  borderStrong: string;
  signTint: string;
  tintGraphite: string;
  tintBlack: string;
  tintWarm: string;
  tintSlate: string;
  signTextSize: string;
  textCompact: string;
  textStandard: string;
  textLarge: string;
  /** 牌内字体：系统无衬线 / 整块等宽。 */
  signFont: string;
  signFontHint: string;
  fontSystem: string;
  fontMono: string;
  /** 牌间距。三档 —— 它会影响窗口高度。 */
  signGap: string;
  signGapHint: string;
  gapTight: string;
  gapStandard: string;
  gapLoose: string;
  /** 一条边怎么亮。`edgeOff` / `edgeGlow` 是 0.6.4 留下的短词，沿用不改名。 */
  edgeOff: string;
  edgeGlow: string;
  /** 四角在设置里的名字（左上 / 右上 / 右下 / 左下），键和 `CornerPos` 一致。 */
  cornerPos: { tl: string; tr: string; br: string; bl: string };
  /** 每枚角灯能表达什么。`empty` = 不挂。 */
  cornerKind: Record<
    "empty" | "state" | "pinned" | "muted" | "stale" | "passthrough",
    string
  >;
  /** 外观页「边框光」这一节：六个独立灯位共用三把尺子，所以放一起。 */
  frameLight: string;
  frameLightHint: string;
  /** 粗细：边灯线宽与角灯直径共用一档。 */
  lightScale: string;
  lightScaleHint: string;
  scaleThin: string;
  scaleNormal: string;
  scaleThick: string;
  /** 明暗。 */
  lightAlpha: string;
  alphaSoft: string;
  alphaNormal: string;
  alphaStrong: string;
  /** 边框光的颜色来源：状态色 / 中性。
   *  别和逐状态表里的 `lightColor`（灯珠颜色）混了 —— 那是另一列。 */
  frameColor: string;
  frameColorHint: string;
  frameColorState: string;
  frameColorNeutral: string;
  /** 行为页「边框光什么时候亮」这一节。 */
  lightBehavior: string;
  lightBehaviorHint: string;
  triggerAlways: string;
  triggerAttention: string;
  triggerActive: string;
  triggerQuiet: string;
  triggerState: string;
  triggerSchedule: string;
  triggerStale: string;
  triggerMessage: string;
  triggerPinned: string;
  /** 触发条件带参数的那几档：挑哪个状态、时段的起止、以及一句说明。 */
  triggerStatePick: string;
  triggerScheduleFrom: string;
  triggerScheduleTo: string;
  triggerParamsHint: string;
  /** 会话标题：默认不拼智能体名，也允许用户自己写模板。 */
  signLabelTitle: string;
  signLabelHint: string;
  signLabel: string;
  labelSession: string;
  labelWithAgent: string;
  labelCustom: string;
  labelTemplate: string;
  labelTemplateHint: string;
  labelTemplatePlaceholder: string;
  // ---- 0.6.5：整套皮肤 / 六档形状 / 牌轮廓 / 五档效果 / 六个灯位 ------------
  /** 一整套主题：配色 + 牌底 + 形状 + 轮廓 + 效果 + 边框光六槽，一键全换。 */
  skinTitle: string;
  skinHint: string;
  skinPlain: string;
  skinPlainHint: string;
  skinFocus: string;
  skinFocusHint: string;
  skinNeon: string;
  skinNeonHint: string;
  skinPaper: string;
  skinPaperHint: string;
  skinSignal: string;
  skinSignalHint: string;
  shapeSquare: string;
  shapeDiamond: string;
  shapeCapsule: string;
  outlineTitle: string;
  outlineHint: string;
  outlineRect: string;
  outlineCapsule: string;
  outlineLeaf: string;
  outlineArch: string;
  /** 动效风格下拉（替代 0.6.4 那个 `lively` 布尔开关）。 */
  effect: string;
  effectHint: string;
  effectOff: string;
  effectBreathe: string;
  effectPulse: string;
  effectSweep: string;
  effectFade: string;
  /** 上下两条边的画法档位；`edgeOff` / `edgeGlow` 沿用旧 key。 */
  edgeLine: string;
  edgeFade: string;
  /** 行为页六行灯位控件共用的标签。 */
  slotTop: string;
  slotBottom: string;
  slotKind: string;
  slotTrigger: string;
  /** 「闪烁」开关：六行灯位各一枚。 */
  slotBlink: string;
  slotBlinkHint: string;
  slotsHint: string;
  /** 「谁在叫我」那套压淡的强度。 */
  signEmphasis: string;
  emphasisAuto: string;
  emphasisFlat: string;
  emphasisHint: string;
  fieldDot: string;
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
  /** 逐状态表里「这个状态静默多久算过期」那一列 */
  stayDuration: string;
  stayDurationHint: string;
  silenceTimeouts: string;
  silenceTimeoutsHint: string;
  silenceTimeoutsMore: string;
  ttlDone: string;
  ttlWorking: string;
  ttlThinking: string;
  behaviorWindowTitle: string;
  clickAction: string;
  clickActionHint: string;
  /** 三个选项的文案，键和 Rust `config::ClickAction` 的序列化值一一对应。 */
  clickActionLabels: { collapse: string; inbox: string; none: string };
  resetToGlobal: string;
  styleHint: string;
  perStateStyle: string;
  /** 逐状态那段收起时的摘要：「3 个状态单独设过」 */
  perStateSome: (count: number) => string;
  perStateNone: string;
  groupDisplayTitle: string;
  groupDisplayEmpty: string;

  // 提醒页
  notifyWhenNeeded: string;
  /** 「在哪些状态下提醒」那一行 */
  notifyWhen: string;
  notifyWhenHint: string;
  soundWithNotify: string;
  alertAfter: string;
  alertAfterHint: string;
  quietHours: string;
  alertHint: string;
  /** B4 预设规则那一节的标题 / 总说明 */
  rulesTitle: string;
  rulesHint: string;
  ruleStuckPush: string;
  ruleStuckPushHint: string;
  ruleBusySilence: string;
  ruleBusySilenceHint: string;
  ruleMeeting: string;
  ruleMeetingHint: string;
  delayNow: string;

  // 全局入口（左栏底部）与「打开某个目录」那两个按钮（系统页）
  pauseMonitoring: string;
  openConfigDir: string;
  openLogs: string;
  pauseHint: string;

  // 接入页
  /** 分组方式那一行的标题 + 三个取值 + 合并后的说明 */
  groupBy: string;
  groupByNone: string;
  groupByCwd: string;
  groupBySession: string;
  groupByHint: string;
  outboundHooks: string;
  configure: string;
  connectHookHint: string;
  port: string;
  apiTitle: string;
  apiToken: string;
  apiAuthHint: string;
  apiEndpoints: string;
  apiEndpointReport: string;
  apiEndpointState: string;
  apiEndpointSessions: string;
  apiEndpointClear: string;
  apiEndpointHealth: string;
  apiStates: string;
  apiStatesHint: string;
  apiExamples: string;
  copy: string;
  copied: string;
  openDocs: string;
  cliExample: string;

  // 智能体接入
  agentsTitle: string;
  agentsHint: string;
  agentInstalled: string;
  agentDetected: string;
  agentMissing: string;
  /** 名字旁边那枚「自定义」：这条来自 `~/.signalhub/agents.json`，不在我们的清单里 */
  agentCustom: string;
  /** 自定义预设那一节的标题 */
  customAgentsTitle: string;
  /** 自定义预设的说明。参数是能当 `family` 用的那几个名字（`claude-code / codex / …`） */
  customAgentsHint: (families: string) => string;
  /** 这个功能**做不到**什么 —— 能力边界，写在说明下面另起一段 */
  customAgentsLimit: string;
  /** 「打开配置文件」按钮（文件不存在时先建一份带说明的空文件） */
  customAgentsOpen: string;
  /** 手写之外的第三条路：面板里直接填一条，走 `add_custom_agent` IPC 写入 */
  customAgentAddToggle: string;
  customAgentAddTitle: string;
  customAgentAddHint: string;
  customAgentFieldId: string;
  customAgentFieldName: string;
  customAgentFieldFamily: string;
  customAgentFieldConfig: string;
  customAgentFieldMarker: string;
  customAgentFieldCommand: string;
  customAgentReplace: string;
  customAgentSubmit: string;
  customAgentCancel: string;
  /** 预校验的错误文案。Record 保证每个 DraftProblem 都有中英两份，靠 tsc 兜住 */
  customAgentProblems: Record<DraftProblem, string>;
  /** 写入结果：added / unchanged / replaced，参数是归一化后的 id */
  customAgentOutcome: (outcome: "added" | "unchanged" | "replaced", id: string) => string;
  /** 「让智能体自己来配」那一块：标题、说明、按钮、指南页那句引导 */
  setupPromptTitle: string;
  setupPromptHint: string;
  setupPromptCopy: string;
  setupPromptGuide: string;
  /** 「已接入」但配置里那条命令现在跑不起来了 —— 静默失效，最该被看见的一种 */
  agentCommandBroken: string;
  /**
   * 「已接入」但配置里那条命令是**旧版本**写的（升级后没重装）。
   *
   * 和 `agentCommandBroken` 不同：那条是「跑不起来」，这条是「跑得起来、行为是旧的」。
   * 不提示的话，界面和正常的「已接入」一模一样，而功能已经停在上一版。
   */
  agentUpdateAvailable: string;
  /** 就地把旧命令改写成当前版本的命令（不用先卸载） */
  agentReinstall: string;
  /** 接入行上的回执：最近一次上报（多久以前 + 报的是什么状态） */
  agentReceiptAgo: (age: string, state: string) => string;
  /** 接入行上的回执：本次运行以来收到多少条 */
  agentReceiptCount: (n: number) => string;
  /** 接入行上的回执：装了、但一条上报都没收到过 */
  agentReceiptNever: string;
  /** 「系统信号源」一节：上报方声明了 `source: "system"` 的 */
  systemSourcesTitle: string;
  systemSourcesHint: string;
  /** 那一节空着时说的话（顺带把「怎么报一个」写出来） */
  systemSourcesEmpty: string;
  /** 内置采集器（B2）：开关那一行上的名字 */
  collectBackup: string;
  /** 采集器开关那一行的说明（秒数是 Rust 那边的间隔，跟它走） */
  collectBackupHint: (secs: number) => string;
  /** 会议中采集器（B2）：开关那一行上的名字 */
  collectMeeting: string;
  collectMeetingHint: (secs: number) => string;
  /** 认不出的采集器 id：后端加了新的、前端没跟上时兜底，不该出现在正常流程里 */
  collectUnknown: (id: string) => string;
  /** 「自定义」一节：不在智能体清单里、也没声明成系统的 */
  otherSourcesTitle: string;
  otherSourcesHint: string;
  /** 「试一下」按钮 */
  agentTry: string;
  /** 正在试 */
  agentTryBusy: string;
  /** 真跑了一遍接入命令，且它成功了 */
  agentTryRanOk: (command: string) => string;
  /** 真跑了一遍，但命令失败了（退出码 / 输出） */
  agentTryRanFailed: (command: string, code: string, output: string) => string;
  /** 插件式接入：只能内部注入，证不了对方会不会调我们 */
  agentTryPlugin: (states: string) => string;
  /** HTTP 式接入：真发了一次 HTTP 到本机端点，端点答了 */
  agentTryHttpOk: (url: string) => string;
  /** HTTP 式接入：那一次没打成（端点没起来 / 局域网模式下被关掉） */
  agentTryHttpFailed: (url: string, output: string) => string;
  /** 接入页底部「复制诊断信息」按钮 */
  diagnoseCopy: string;
  diagnoseHint: string;
  /** 「系统」页：hook 调用记录的尾巴 */
  diagLogTitle: string;
  diagLogHint: string;
  diagLogEmpty: string;
  diagRefresh: string;
  agentConnect: string;
  agentDisconnect: string;
  /**
   * Claude Code 的第二条路：**HTTP 方式**接入（不需要命令存在）。
   *
   * 文案里必须点出代价 —— 开了局域网访问时这个端点会自己关掉（它没有 token，
   * 见 Rust 侧 `server::hook_report`）。不写这一句的话，用户会在打开局域网之后
   * 遇到一次「突然没反应」而不知道为什么。
   */
  agentConnectHttp: string;
  /** 已接入、想换另一条路时的按钮 */
  agentSwitchHttp: string;
  agentSwitchCommand: string;
  /** 已接入时的接入方式说明（只在 Claude Code 那一行显示） */
  agentModeHttp: string;
  /** 从灯上审批：那个开关 */
  approveTools: string;
  approveToolsHint: string;
  /** 接入页顶部的适用范围说明：开关只在 Claude Code 那一行出现，别家的人得看得到 */
  approveScopeHint: string;
  /** 待审批气泡里的按钮 / 说明 */
  approvalAllow: string;
  approvalDeny: string;
  approvalTooLong: string;
  approvalMore: (n: number) => string;
  /** `{}` 已经等了多少秒 */
  approvalWaiting: (secs: number) => string;
  /**
   * 危险命令横幅（`docs/plan-approval.md` A3）。键和 `hookmap::danger` 一一对应 ——
   * 那边只给稳定的键，翻成人话是这一层的事。
   */
  approvalDanger: {
    delete: string;
    "force-push": string;
    rewrite: string;
    sql: string;
    "pipe-shell": string;
    disk: string;
    privilege: string;
    permissions: string;
  };
  /** 按住确认那句提示（危险命令的「允许」要按住 0.8 秒）。 */
  approvalHold: string;
  /** 展开拒绝理由的小按钮 */
  approvalReason: string;
  approvalReasonUnsafe: string;
  approvalReasonSafer: string;
  approvalReasonExplain: string;
  approvalReasonCustom: string;
  approvalReasonCustomPlaceholder: string;
  /** 历史页的「审批记录」那一节（A1） */
  approvalLogTitle: string;
  approvalLogHint: string;
  approvalLogEmpty: string;
  approvalLogDecision: (decision: string) => string;
  approvalLogSource: (source: string) => string;
  agentModeCommand: string;
  /** 「其他工具」那一节 —— 它们没有能写 hook 的配置文件 */
  cliAgentsTitle: string;
  cliAgentsHint: string;
  /** 那一节里「包一条命令」那一行 */
  cliAgentsWrap: string;
  /** 第二行的名字：「在自己的事件钩子里调」 */
  cliAgentsHookName: string;
  /** 「试一下」那一节 */
  tryItTitle: string;
  tryItHint: string;
  hookResult: (change: HookResult) => string;
  /**
   * 插件式接入的结果。**不并进 `hookResult`**：那个函数数的是「几个事件」，
   * 而插件没有事件清单 —— 它写的是**一个文件**，用户能去 `cat` 的那个。
   */
  hookPluginWritten: (path: string) => string;
  hookPluginRemoved: (path: string) => string;

  // 转发页
  noHooks: string;
  hookN: (n: number) => string;
  noTarget: string;
  devicesTitle: string;
  noDevices: string;
  deviceN: (n: number) => string;
  addDevice: string;
  devicePlaceholder: string;
  devicesHint: string;
  type: string;
  command: string;
  addHook: string;
  hooksHint: string;
  commandPlaceholder: string;
  /** 组过滤那一排的小标题。和上面的状态过滤是两回事，得分开 */
  hookGroups: string;
  /** 组过滤的说明：空 = 全局聚合，非空 = 看各组自己的跃迁 */
  hookGroupsHint: string;
  /** 悬停提示：这一组当前没有上报，但它还挂在钩子上 */
  groupNotReporting: string;
  /** 从没触发过的钩子 */
  neverFired: string;

  // 转发页（R4：预设 / 请求头 / 发送测试）
  /** 「发什么形状」那一行的标签 */
  hookPreset: string;
  /** 预设显示名。键是 Rust `HttpPreset` 的 serde 名（= `HTTP_PRESETS` 里的值）。 */
  presetNames: Record<HttpPreset, string>;
  /**
   * 地址输入框的占位符 —— **按预设给**。光看名字猜不出该填什么地址，而填错地址
   * 是这一列里最常见的失败（PushDeer 的 `…/message/push?pushkey=…` 尤其是）。
   */
  presetUrls: Record<HttpPreset, string>;
  /** 预设说明：`raw` 之外每一个都只发一行文字 */
  presetHint: string;
  /** 自定义请求体那一节的标题 */
  hookBody: string;
  hookBodyPlaceholder: string;
  /** 占位符清单 + JSON 转义那句话 */
  hookBodyHint: string;
  /** 自定义 content-type 的标签 */
  hookContentType: string;
  /** 请求头那一节的标题 */
  hookHeaders: string;
  headerNamePlaceholder: string;
  headerValuePlaceholder: string;
  addHeader: string;
  headersHint: string;
  /** 头名重复（大小写不敏感）。添加按钮据此禁用 —— 顺序在这一层还在，能挡住。 */
  headerDuplicate: string;
  /** 「发送测试」按钮 */
  sendTest: string;
  /** 一条都没发出去（没有填好目标的转发） */
  testNothing: string;
  /** 发了 n 条，结果看下面每条的最近一次结果 */
  testSent: (n: number) => string;
  /** 「多久以前」的后缀，如「2m前」/「2m ago」 */
  firedAgo: (age: string) => string;
  /** 提醒页里指向转发页的一行 */
  alertForwardHint: string;

  // 系统页
  alwaysOnTop: string;
  trayFollowsState: string;
  launchAtLogin: string;
  clickThrough: string;
  systemHintTray: string;
  systemHintPassThrough: (shortcut: string) => string;
  shortcutFallback: string;
  shortcutsTitle: string;
  /** 按动作名取本地化的名字（`lamp` / `mute` / `clear` / `pass_through`）。 */
  shortcutActions: Record<string, string>;
  shortcutTaken: string;
  shortcutsHint: string;
  language: string;
  langAuto: string;
  langZh: string;
  langEn: string;
  /** 设置窗口的亮 / 深色偏好（灯球与收件箱不跟随，见 themeHint）。 */
  theme: string;
  themeAuto: string;
  themeLight: string;
  themeDark: string;
  themeHint: string;

  // 配置导入 / 导出
  configBackup: string;
  exportConfig: string;
  importConfig: string;
  configBackupHint: string;
  exportDone: (path: string) => string;
  importDone: string;
  configExportTitle: string;
  configImportTitle: string;
  configFilterName: string;

  // 自动更新
  updateTitle: string;
  updateCurrent: (version: string) => string;
  checkUpdate: string;
  checkingUpdate: string;
  updateLatest: string;
  updateAvailable: (version: string) => string;
  updateDownloadInstall: string;
  updateDownloading: (progress: string) => string;
  updateInstalling: string;
  updateNotes: string;
  updateHint: string;
  updateUnsupported: string;
  updateFailed: (reason: string) => string;
  openReleases: string;

  // 局域网访问
  lanAccess: string;
  lanAccessHint: string;
  lanAddress: string;
  lanRestart: string;
  /** 「只读 Web 面板」那一节 */
  webPanel: string;
  webPanelUrl: string;
  webPanelHint: string;
  /** 面板开着**且**允许局域网访问时的代价（同一 WiFi 下任何设备都能看） */
  webPanelLanWarning: string;
  /** 「在面板上审批」（A4） */
  webApprove: string;
  webApproveHint: string;
  /** 面板审批开着、但「从灯上审批」本身是关的 */
  webApproveNeedsTools: string;

  quit: string;

  // 灯球 tooltip
  /** 灯上的悬停说明按「单击做什么」分三份，免得改了设置之后提示还在说旧话。 */
  lampHint: { collapse: string; inbox: string; none: string };
  /**
   * 空状态：「刚完成 · N 分钟前」。
   *
   * `minutes` 是向下取整的分钟数，0 表示不到一分钟（说「刚刚」）。这句话顶替的是
   * `signal.offline` 那句「没有智能体在上报」—— 见 `lib/signals.ts`。
   */
  justFinished: (minutes: number) => string;
  groupsCount: (n: number) => string;
  /** 截断标记的 tooltip：「+3」指的是哪几组 */
  moreGroups: (n: number) => string;
  /** 折叠切换的一次性反馈 */
  expandNow: string;
  collapseNow: string;
  /** 持续可见的状态指示 */
  lampPaused: string;
  /** 会话行尾那枚 × 的悬停提示 */
  clearSessionHint: string;
  lampPassThrough: string;
  /** 「+N」截断后，tooltip 里列出被收起来的组之前的引导 */
  moreGroupsLead: string;

  /**
   * 圆环菜单（长按灯触发）里那几项。
   *
   * 单独一组而不是复用托盘的 `pauseMonitoring` / `quit`：托盘那几句是**菜单项**
   * 的语气（「暂停监控」），而圆环里是**图标下的短标签**，位置只有 60px 宽，
   * 长了就折行。两处措辞该各自为政，不该为了少几个字符串硬凑成一份。
   */
  lampMenuInbox: string;
  lampMenuLabel: string;
  lampMenuSettings: string;
  lampMenuPause: string;
  lampMenuResume: string;
  lampMenuQuit: string;
  /** 圆环菜单打开时，中心那颗灯珠的悬停说明（「点空白处关闭」） */
  lampMenuCloseHint: string;

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

  // 收件箱（0.4）
  /** 「多机」页（多机中枢，见 docs/plan-multi-machine.md）。 */
  machines: {
    title: string;
    hint: string;
    empty: string;
    add: string;
    name: string;
    nameHint: string;
    url: string;
    urlHint: string;
    token: string;
    tokenHint: string;
    enabled: string;
    test: string;
    testing: string;
    remove: string;
    /** 连通时那一行：延迟 + 会话数。 */
    connected: (ms: number, sessions: number) => string;
    /** 从来没成功连上过。 */
    never: string;
    /** 上次成功是什么时候。 */
    lastOk: (ago: string) => string;
    poll: string;
    pollHint: string;
    pollSeconds: (seconds: number) => string;
    note: string;
    peerHint: string;
  };

  /** 「Call Me」配置页与「留言」阅读页（自部署收信服务器，见 docs/callme.md）。 */
  callme: {
    /** 页内子页的两个名字。 */
    linksTab: string;
    serverTab: string;
    /** 「留言」页里跳去管理链接的次级入口。 */
    manageLinks: string;
    urlLabel: string;
    urlPlaceholder: string;
    urlHint: string;
    tokenLabel: string;
    tokenPlaceholder: string;
    tokenHint: string;
    /** 服务器子页的一句前提说明（自部署 / TLS 归反代）。 */
    connectionHint: string;
    pollLabel: string;
    pollSeconds: (seconds: number) => string;
    test: string;
    testing: string;
    /** 连通时那一行。 */
    connected: (ms: number) => string;
    /** 上次成功是什么时候。 */
    lastOk: (ago: string) => string;
    never: string;
    unconfigured: string;
    /** 链接子页顶部「去服务器子页」的入口。 */
    openServerTab: string;
    sourcesTitle: string;
    sourcesEmpty: string;
    addSource: string;
    creating: string;
    /** 新建按钮旁边：为什么不用填名字。 */
    autoNameHint: string;
    /** 来源列表里占位名旁边的灰字。 */
    pendingName: string;
    copyLink: string;
    copiedLink: string;
    /** 点标题改名；回车 / 失焦提交，Esc 取消。 */
    renameLink: string;
    renamePlaceholder: string;
    /** 在系统浏览器打开这条链接 —— 智能体看到的就是 GET 返回的用法说明。 */
    openUsage: string;
    revoke: string;
    revokeConfirm: string;
    revoked: string;
    /** 彻底删除（不是吊销）：位置让出来，已收到的留言保留。 */
    deleteLink: string;
    deleteConfirm: string;
    restoreLink: string;
    /** 吊销的链接默认收在这个折叠区里，不再占主列表。 */
    revokedTitle: (n: number) => string;
    revokedHint: string;
    /** 有效链接一条不剩、但还有吊销着的时候，主列表给的出路。 */
    activeEmpty: string;
    /** 删掉来源后，旧留言找不到来源名时的兜底。 */
    deletedSource: string;
    /** 这条链接近 24 小时收到几条（与服务器配额同口径）。 */
    todayCount: (n: number) => string;
    totalCount: (n: number) => string;
    createdTitle: string;
    createdHint: string;
    filterAll: string;
    /** 空态的原因：没服务器 / 连不上 / 没链接 / 真没有。 */
    messagesEmptyNoServer: string;
    messagesEmptyOffline: string;
    messagesEmptyNoSource: string;
    messagesEmpty: string;
    /** 筛选器收窄之后一条都不剩。 */
    filterNoResults: string;
    /** 空态上的出路。 */
    goConfigure: string;
    checkServer: string;
    goCreateLink: string;
    unread: (n: number) => string;
    markAllRead: string;
    openSource: string;
    copyText: string;
    copiedText: string;
    /** 本机一键服务（桌面进程内跑的收信服务）。 */
    localTitle: string;
    localHint: string;
    localPort: string;
    localPortInvalid: string;
    /** 已经填了公网地址时，点「启动」会覆盖它 —— 先问一句。 */
    localReplaceConfirm: string;
    localStart: string;
    localStop: string;
    localStarting: string;
    localStopping: string;
    localRunning: string;
    localStopped: string;
    localOnlyHint: string;
    /** 服务器子页里收起来的那部分：地址 / token / 轮询 / 端口。 */
    advancedTitle: string;
    /** VPS 部署指引。 */
    remoteTitle: string;
    remoteHint: string;
    remoteStep1: string;
    remoteStep2: string;
    remoteStep3: string;
    copyCommand: string;
    copiedCommand: string;
    remoteSecurityHint: string;
  };

  inbox: {
    title: string;
    need: string;
    running: string;
    done: string;
    quiet: string;
    emptyTitle: string;
    emptyHint: string;
    /** 空态上的两条出路：去接入一个智能体 / 先看看它长什么样。 */
    emptyConnect: string;
    emptyDemo: string;
    /** 行尾「更多动作」按钮的无障碍名字（次要动作收在它后面）。 */
    more: string;
    paused: string;
    jump: string;
    allow: string;
    deny: string;
    mute: string;
    unmute: string;
    clear: string;
    /** 「盯住这个任务」：跑完 / 出错 / 消失时通知我，一次性。 */
    watch: string;
    unwatch: string;
    watchSet: (label: string) => string;
    watchCleared: (label: string) => string;
    close: string;
    settings: string;
    truncated: string;
    allowBlocked: string;
    allowed: string;
    denied: string;
    cleared: string;
    muted: (group: string) => string;
    unmuted: (group: string) => string;
    waiting: (age: string) => string;
    keys: string;
    jumpUnsupported: string;
    /** 远程会话的跳转：说明为什么跳不了。 */
    remoteJump: (machine: string) => string;
    approvalHint: string;
    closeHint: string;

    /**
     * 「你离开的这段时间」摘要（plan-round5 下一步第 1 项）。
     *
     * 只给计数，不给列表 —— 列表就是收件箱本身，再列一遍等于把同一份内容画两次。
     * 时长复用 `formatAge` 的紧凑写法（`23m`），和行尾的「等了 3m」是同一套单位。
     */
    awayFor: (ago: string) => string;
    awayDone: (count: number) => string;
    awayFailed: (count: number) => string;
    awayNeed: (count: number) => string;
    awayRunning: (count: number) => string;
    /** 只存在于事件流水里：跑完 / 失败之后已经从收件箱消失的那些。 */
    awayTransientDone: (count: number) => string;
    awayTransientFailed: (count: number) => string;
    /** 离开期间什么都没发生。 */
    awayQuiet: string;
    awayDismiss: string;
  };
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

    railWatch: "工作台",
    railLamp: "灯",
    railNotify: "通知",
    railConnect: "连接",
    railHelp: "帮助",
    tabStatus: "状态",
    tabHistory: "历史",
    tabMessages: "留言",
    historyTitle: "近 7 天",
    historyHint:
      "一天一行，横条的宽度就是那天各状态占用的时间（同一天内部按状态从多到少排，鼠标停上去看某一段多久），右边是当天合计。数据每天落盘，保留 30 天。没有记录的日子不会补一行零 —— 那多半是应用没跑，而不是「灯开着但什么都没干」。",
    historyToday: "今天",
    historyEmpty: "还没有历史数据 —— 用过一天之后这里就有了。",
    tabLook: "外观",
    tabBehavior: "行为",
    tabGroups: "分组",
    tabAlert: "提醒",
    tabConnect: "接入",
    tabMachines: "多机",
    tabCallMe: "Call Me",
    tabHooks: "转发",
    tabGuide: "指南",
    tabSystem: "系统",
    tabDemo: "演示",
    connectTabAgents: "智能体",
    connectTabSources: "其他来源",
    connectTabNetwork: "网络与诊断",
    systemTabGeneral: "通用",
    systemTabUpdate: "更新与诊断",
    systemTabData: "数据",
    pageHint: {
      status: "现在怎么样：正在跑什么、最近发生了什么。",
      history: "过去几天发生了什么：今日复盘、洞察、时长和审批记录。",
      messages: "外面的智能体通过 Call Me 链接发来的留言，展开即已读。",
      look: "灯长什么样：尺寸、形态、配色，以及牌面的质感。",
      behavior: "灯怎么表现：静默超时、安静模式和点击。",
      groups: "会话怎么分组、每组分几盏灯。",
      alert: "什么时候用系统通知提醒你。",
      hooks: "状态变化时往哪里转发：webhook、ntfy、物理灯。",
      connect: "把智能体和信号源接到 SignalHub。",
      machines: "把其他机器的中枢接到这里，一盏灯看全部。",
      callme: "管理 Call Me 收信链接和服务器。",
      guide: "从零开始的接入手册和接口速查。",
      demo: "不用真接一个智能体，先看看它长什么样。",
      system: "这台机器上的这个应用：语言、更新、数据。",
    },

    railSearch: "找设置…",
    railSearchEmpty: "没有匹配的页签",
    lampFirstRun: "第一次用？",
    lampFirstRunDemo: "看演示",
    lampFirstRunDemoHint: "不接智能体，先看这盏灯怎么亮（五段场景，每段 10–20 秒）",

    demoTitle: "看懂 SignalHub 的五种场景",
    demoIntro: "按场景循环播放 —— 想看哪一段点哪一段，不用接任何智能体。",
    demoBackToSettings: "返回设置",
    demoNotReal: "演示画面 · 不会改动任何真实状态",
    demoGroup: { build: "构建", api: "接口", review: "审阅" },
    demoScenario: {
      day: "一天的节奏",
      asking: "有事要你",
      silence: "它不说话了",
      tucked: "收起来的样子",
      look: "用光说话",
    },
    demoStep: {
      idle: {
        title: "接上之后，灯是安静的",
        body: "常亮绿灯 = 空闲待命。安静模式下这一块会淡下去，不抢注意力；有事才显形。",
      },
      thinking: {
        title: "你问了一句",
        body: "开始推理 → 绿灯快闪。不用切到终端，余光就知道它动起来了。",
      },
      working: {
        title: "几个智能体同时干活",
        body: "一组一块牌，最紧急的排最前。牌上写着是哪一组、在做什么、已经多久了。",
      },
      permission: {
        title: "有一组在等你批准",
        body: "需要授权 = 红色、圆角方、平滑闪。红 > 黄 > 绿：旁边两颗绿灯不会把它顶掉。",
      },
      approved: {
        title: "点允许，红灯自己消失",
        body: "那一组恢复工作，全局聚合自然回到绿色 —— 不用手动清状态。",
      },
      done: {
        title: "干完了，亮一下",
        body: "完成是一闪而过的绿灯；默认 3 分钟后这块牌被移除，时长可以在设置里改。",
      },
      empty: {
        title: "牌走了，灯还记得",
        body: "空状态说的是「刚完成 · 1 分钟前」，而不是「没有智能体在上报」—— 这正是你最想确认的那一刻。",
      },
      attention: {
        title: "黄色：值得扫一眼，但不用马上动手",
        body: "需要关注 = 黄色、1 秒一闪。它没卡住，也没等你批什么 —— 只是有件事你应该知道，而不用放下手上正在做的事。",
      },
      both: {
        title: "两种都有时，红在上、黄在中",
        body: "一组的紧急度就是它在牌堆里的位置：红 > 黄 > 绿。你不用自己挑哪块先看 —— 从上往下就是处理顺序。",
      },
      error: {
        title: "同样是红色，节奏不一样",
        body: "出错是频闪（快而硬），等授权是平滑闪。授权在等你点允许，出错在等你去看一眼日志。长时间没有新消息，两者都会转灰：出错 5 分钟，授权 10 分钟。",
      },
      stale: {
        title: "半小时没消息，它就转灰",
        body: "灰不是「空闲」—— 空闲是它还在、只是没在干活；静默是我们已经不知道它还在不在。再没人管，半小时后这块牌会被彻底移除。",
      },
      staleRing: {
        title: "换成圆环，弧长就是剩余时间",
        body: "外观页把点形状改成「圆环」之后，弧长就是它距离被移除还剩多少，会一点点缩到消失。默认的实心点没有地方画进度 —— 这是个可选形状。",
      },
      offline: {
        title: "环走完了，牌就消失",
        body: "这时说的是「没有智能体在上报」，和「刚完成」是两件事。真有事要找你时，它会重新出现。",
      },
      quiet: {
        title: "安静模式：空闲时几乎看不见",
        body: "空闲、静默、没上报时整块压到很淡，只剩一颗浅点；一有事就恢复原来的分量。它让这盏灯平时不占注意力，而不是把提醒关掉。",
      },
      collapsed: {
        title: "收起来，只剩一颗灯",
        body: "单击（设置 →「行为」里可以改成打开收件箱或什么都不做）把整叠牌收成一颗灯。三组聚合成一颗点：只要有一组在等你，它就是红的。",
      },
      collapsedDone: {
        title: "收起来也照样会亮",
        body: "完成时这颗灯会闪一下再落定。折叠只是少占桌面，不是把提醒关掉 —— 这也是很多人平时的默认形态。",
      },
      edgeLine: {
        title: "上边亮，下边可以仍然不亮",
        body: "上边一道状态色的亮线，下边留着「不亮」—— 上下两条边是两个独立灯位，不再是一条轨一起开关。颜色、粗细和明暗在「外观 → 边框光」里一起调，字一个都没多。",
      },
      edgeBoth: {
        title: "上下各挑各的",
        body: "这次上下都设成实线，但它们是两行、两个下拉：想让下边只在出错时亮、上边常亮，也只要各挑一次。四档画法还有辉光和两端淡出。",
      },
      edgeGlow: {
        title: "辉光：最张扬的一档",
        body: "上下两条边都换成辉光，牌外再散一圈光。坐得远、显示器多、或者要它从余光里跳出来时用它；浅色壁纸上会淡一些，那是有意的。",
      },
      cornersState: {
        title: "四个角，四个是非题",
        body: "四枚小灯就落在边框的四个转角上，跟着轮廓走，不占牌面。分别回答：有状态 / 置顶 / 静音 / 将过期。这里四枚都设成「状态色」，先让你看见它们站在哪 —— 四角在「外观」里也是四行、四个下拉。",
      },
      cornersMeaning: {
        title: "条件不成立，就根本不画",
        body: "这一块牌同时满足四条 —— 有状态、被置顶、被静音、正在沉默，所以四枚都亮。任何一条不成立，那枚角灯就整个消失，而不是留一个暗点：暗点会让人以为「这个位置本来就有东西，只是没亮」，于是永远不会去设置里找。四角各自的触发还能在「行为」页单独挑。",
      },
      lightTrigger: {
        title: "什么时候亮，是另一回事",
        body: "两块牌并排：上边被设成「只在要你处理时亮」，于是等批准的那块亮、正在跑的那块不亮。外观管亮起来长什么样，行为管凭什么现在亮 —— 六个灯位各有自己的九档条件。",
      },
      cornerPassthrough: {
        title: "连「点不进去」也能挂在角上",
        body: "左上那枚空心方是「点击穿透」：开着的时候它出现，关掉就整个消失。原先窗口右上角那枚常驻的禁止图标已经撤掉了 —— 要不要看这个状态、放在哪个角，现在由你在「外观 → 边框光」的六行里定。状态本身仍在 tooltip 和托盘菜单里。",
      },
      lightTriggerState: {
        title: "九种条件，六行各判各的",
        body: "上边挑「达到指定状态 → 出错」，下边常亮，四角也常亮：出错那块上下都亮、正在跑那块只剩一条下边，四枚状态角灯两块牌都挂着。除了指定状态，还能按指定时段、沉默将尽、有没有消息、是否置顶来触发 —— 六行共用同一张条件表，但各存各的一档。",
      },
      lookSkin: {
        title: "也能一键换整套",
        body: "这一步是「霓虹」皮肤：配色、牌底、圆角、描边、灯珠形状、牌的轮廓、动效，以及六个灯位的画法和触发，一次全换。「外观 → 皮肤」里一共五套；只想改大小或透明度的话，下面那四套外观预设不动这些。",
      },
    },
    demoStepCount: (index, total) => `${index} / ${total}`,
    demoPrev: "上一步",
    demoNext: "下一步",
    demoPlay: "播放",
    demoPause: "暂停",
    demoReplay: "重播",
    demoCta: "看四段演示",
    demoCtaHint: "还没接智能体？先看一遍这盏灯会怎么亮，不用配任何东西。",

    guideEntry: "第一次用？",
    guideWhatTitle: "这是什么",
    guideWhat:
      "SignalHub 是一盏常驻桌面的信号灯。任何能发一条 HTTP 请求的程序都能让它亮起来 —— 智能体、构建脚本、你自己写的服务，都行。绿灯是在干活，红灯是需要你，黄灯是等你授权；牌上写着是哪一组、卡在哪一步、已经多久了。",
    guideStepsTitle: "三步接上",
    guideStep1:
      "让上报方说话：智能体在「接入」页一键接上，别的程序往下面的接口 POST 一条状态即可。",
    guideStep2:
      "看灯：灯球默认展开成一叠牌，点一下折叠成一颗灯，位置和形态都记得住。",
    guideStep3:
      "别错过：需要你注意时会弹系统通知。「提醒」页可以设勿扰时段、提醒阈值，以及只对某几个状态生效的出站转发。",
    guideFaqTitle: "不工作了怎么办",
    guideFaqNoLightQ: "灯一直是灰的？",
    guideFaqNoLightA:
      "灰 = 没有任何会话在上报，或者「暂停监控」开着。先看顶部那行标题写的是什么，再去「接入」页确认上报方接上了。",
    guideFaqNoNotifyQ: "通知不弹、也没有声音？",
    guideFaqNoNotifyA:
      "先看「提醒」页顶部的状态：显示「已被拒绝」要去系统设置的「通知」里手动打开；Windows 免安装版显示「未注册」的话，说明系统通知所需的开始菜单快捷方式没建好，点旁边的「重新注册」。状态是「已允许」却仍然不弹，也可能是专注模式 / 系统通知开关拦了 —— 应用只能确认把通知交给了系统，看不到系统有没有真的显示。",

    notifyPermission: "系统通知权限",
    permGranted: "已允许",
    permDenied: "已被拒绝",
    permPrompt: "还没申请",
    permUnknown: "读不到",
    permUnavailable: "未注册",
    permRequest: "请求权限",
    permRetry: "重新注册",
    permHint:
      "没有权限时通知会被静默丢弃 —— 不弹、不响，也不报错。显示「已被拒绝」的话，需要去系统设置的「通知」里手动打开 SignalHub。",
    permUnavailableHint:
      "Windows 的原生通知要求应用有一个带身份标识的开始菜单快捷方式，安装版由安装器创建，免安装版则由应用在首次启动时补上。这一步没成功时，通知会被系统直接丢弃；点「重新注册」再试一次，或改用 -setup.exe 安装版。",
    permBackendError: (detail) => `注册失败的具体原因：${detail}`,
    notifyTest: "发一条测试通知",
    notifyTestTitle: "SignalHub 测试通知",
    notifyTestBody: "看到这条，就说明系统通知和提示音都通了。",
    notifyTestResult: (perm) =>
      `已把测试通知交给系统（权限：${perm}）；应用看不到系统是否真的显示了它。没看到的话，去系统设置 →「通知」检查 SignalHub，看看有没有开着专注模式；Windows 免安装版还要确认上面的状态是「已允许」而不是「未注册」。`,

    healthTitle: "系统体检",
    healthHint:
      "一次把通知权限、接入状态、上报回执和快捷键都过一遍。全是本机已有的数据，不改动任何设置。发出问题前点一下，把报告复制过去最省事。",
    healthRun: "跑一次体检",
    healthRerun: "重新体检",
    healthSummary: (fail, warn) =>
      fail > 0
        ? `${fail} 项要处理${warn > 0 ? `，另有 ${warn} 项值得看` : ""}`
        : warn > 0
          ? `${warn} 项值得看，没有必须处理的`
          : "没发现需要处理的问题",
    healthVerdict: (level) =>
      level === "fail" ? "要处理" : level === "warn" ? "值得看" : "都通了",
    healthLevel: (level) =>
      level === "fail" ? "问题" : level === "warn" ? "注意" : level === "ok" ? "正常" : "信息",
    healthLine: zhHealthLine,
    healthReportHeader: (at, version) => `SignalHub 系统体检 · ${at} · v${version}`,

    digestTitle: "今日复盘",
    digestHint:
      "按聚合状态计时，同一时刻只记一个状态，所以各项可以直接相加；空闲 / 离线的时间也在里面。审批只取最近 100 条记录里属于今天的。它看的是时长、不逐会话 —— 出现过又消失的会话请看下方的「本周洞察」。",
    digestFact: zhDigestFact,
    digestReportHeader: (day) => `SignalHub 今日复盘 · ${day}`,

    insightTitle: "本周洞察",
    insightHint:
      "按会话和审批记录数数，数据来自本机近 7 天的事件流水与审批日志（各最多 5000 条，超出只取最新的）。只做加法和挑最大值，不联网、不推测。",
    insightFact: zhInsightFact,
    insightReportHeader: (days) => `SignalHub 本周洞察 · 近 ${days} 天`,
    insightError: "本地事件流水读不出来，这张卡片暂时不显示。",
    insightTruncated:
      "记录数已到上限，这里只统计了最近的一部分，上面的数字可能偏小。",

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
    displayName: "显示名",
    displayNamePlaceholder: (raw) => `留空就用 ${raw}`,
    pin: "置顶",
    pinHint: "置顶的组永远排在灯上最前面；没置顶的仍按紧急程度排",
    feedTitle: "最近消息",
    feedEmpty: "还没有上报过消息。",

    presets: "预设",
    form: "形态",
    expanded: "展开牌面",
    singleLight: "一颗灯",
    lampLayout: "灯的数量",
    layoutSingle: "一盏灯",
    layoutPerGroup: "每组分灯",
    layoutHint:
      "上面那一项定的是有哪些组，这一项定的是它们摆成几盏灯。「每组分灯」让每个组各有一盏能单独拖动的灯，主灯只剩全局状态；每盏灯第一次出现在屏幕右缘，拖到想要的地方后各自记住。超过 8 个组会自动变回一盏灯。",
    size: "大小",
    shape: "形状",
    palette: "配色方案",
    signOpacity: "背景透明度",
    signOpacityHint:
      "最低 65%：牌里的字全靠这层深色托着，再淡就落在桌面上，浅色窗口里会看不清。想让灯更隐形，用「安静模式」而不把它调没。",
    signWidth: "牌宽",
    signHeight: "牌高",
    blinkSpeed: "闪烁节奏",
    onSign: "牌上显示",
    quietMode: "安静模式",
    snapToEdge: "贴边吸附",
    snapToEdgeHint: "拖到屏幕左 / 右边缘附近松手时自动贴齐（上下不吸 —— 任务栏和 Dock 在那边）。",
    lookHint:
      "形状只影响灯珠：改成圆角方或环会让 permission / error 的方角区分失效，此时建议配合色盲友好配色。",
    resetToDefault: "恢复默认",
    previewTitle: "预览",
    previewHint: "改上面的东西，这里立刻跟着变 —— 灯被挡住、收起来、或在另一块屏上时，这里是唯一看得见效果的地方。画的是「有活动时」的样子：安静模式压暗只在真的灯上发生。",
    previewGroup: "订单服务",
    previewMessage: "正在跑测试",
    sizeSmall: "小",
    sizeMedium: "中",
    sizeLarge: "大",
    shapeCircle: "圆",
    shapeRounded: "圆角方",
    shapeRing: "环",
    paletteDefault: "默认",
    paletteColorblind: "色盲友好",
    paletteVivid: "高饱和",
    lookTabBasic: "基础",
    lookTabTexture: "质感",
    textureHint:
      "这几项只改牌面的画法，不改牌子和窗口的大小 —— 怎么调都不会裁掉内容。",
    signRadius: "圆角",
    radiusSquare: "直角",
    radiusSoft: "小",
    radiusStandard: "标准",
    radiusRound: "大",
    signBorder: "描边",
    borderNone: "无",
    borderHairline: "细",
    borderStrong: "明显",
    signTint: "牌底",
    tintGraphite: "石墨",
    tintBlack: "纯黑",
    tintWarm: "暖黑",
    tintSlate: "蓝黑",
    signTextSize: "字号",
    textCompact: "紧凑",
    textStandard: "标准",
    textLarge: "大",
    signFont: "字体",
    signFontHint:
      "「等宽」把整块牌换成终端字形 —— 组名、状态、消息一起换，读起来像一段日志。",
    fontSystem: "系统",
    fontMono: "等宽",
    signGap: "牌间距",
    signGapHint:
      "牌竖着叠，这一项只管它们之间留多宽。它会让窗口跟着变高，所以只有三档。",
    gapTight: "紧凑",
    gapStandard: "标准",
    gapLoose: "宽松",
    edgeOff: "不亮",
    edgeGlow: "辉光",
    cornerPos: { tl: "左上", tr: "右上", br: "右下", bl: "左下" },
    cornerKind: {
      empty: "不挂",
      state: "状态色",
      pinned: "置顶",
      muted: "静音",
      stale: "沉默进度",
      passthrough: "点击穿透",
    },
    frameLight: "边框光",
    frameLightHint:
      "上下两条边和四个角是六个独立灯位，共用同一套粗细、明暗和颜色。每个灯位在「行为」页各有自己的九档触发条件；四角还能挂「点击穿透」。上面两块预览牌始终按常亮画 —— 它们是一张静态样机，不跟着触发条件走。",
    lightScale: "粗细",
    lightScaleHint:
      "边灯的线宽和角灯的大小一起变。分开调很容易调出「一条粗线配四颗小芝麻」 —— 它们本来就是同一条轨上的东西。",
    scaleThin: "细",
    scaleNormal: "标准",
    scaleThick: "粗",
    lightAlpha: "明暗",
    alphaSoft: "淡",
    alphaNormal: "标准",
    alphaStrong: "亮",
    frameColor: "颜色",
    frameColorHint:
      "「状态色」跟着每块牌自己的状态走；「中性」让边框只负责形状，不抢灯珠那点颜色。",
    frameColorState: "状态色",
    frameColorNeutral: "中性",
    lightBehavior: "边框光什么时候亮",
    lightBehaviorHint:
      "「外观」那节管亮起来长什么样，这六行管凭什么现在亮。**六行是同一张九档条件表**：上边可以常亮、下边只在出错时亮、四个角各挂各的，互不牵连。判定逐块牌进行，同一盏灯里可以有牌亮、有牌不亮。",
    triggerAlways: "常亮",
    triggerAttention: "只在要你处理时",
    triggerActive: "只在有活动时",
    triggerQuiet: "跟随安静模式",
    triggerState: "达到指定状态",
    triggerSchedule: "指定时段内",
    triggerStale: "沉默快到期时",
    triggerMessage: "牌上有消息时",
    triggerPinned: "只给置顶的组",
    triggerStatePick: "指定哪个状态",
    triggerScheduleFrom: "从",
    triggerScheduleTo: "到",
    triggerParamsHint:
      "时段的起止按本机时间算，起 > 止就是跨夜（22:00–06:00）；两个时间设成一样表示不按时间筛。这三个参数是六个灯位共用的 —— 六份输入框会把这一页撑成一张表，而「两行都选了指定状态却只有一个状态框」更没法解释。",
    signLabelTitle: "会话标题",
    signLabelHint:
      "按会话分组时，牌宽固定，名字那一行还要和状态词分 —— 默认的「项目 · 智能体」里智能体名几乎每次都被省略号吃掉。现在默认只显示会话 / 项目名；想看智能体名或自己写一套标题都可以。逐组改名仍然优先于这里。",
    signLabel: "标题里写什么",
    labelSession: "只显示会话名",
    labelWithAgent: "会话 · 智能体",
    labelCustom: "自定义模板",
    labelTemplate: "模板",
    labelTemplateHint:
      "可用 {session}（项目名）、{agent}（智能体名）、{group}（原始组名）。空掉的占位符会被清掉，不会留一个孤零零的分隔符。转发摘要和 Web 面板仍用全名 —— 它们不受牌宽限制。",
    labelTemplatePlaceholder: "{session} · {agent}",
    skinTitle: "皮肤",
    skinHint:
      "一键换一整套：配色、牌底、圆角和描边、灯珠形状、牌的轮廓、动效，以及边框光六个灯位。**换肤会连六行触发条件一起覆盖** —— 只想改大小、透明度或显示哪几段，用下面那四套外观预设。",
    skinPlain: "素净",
    skinPlainHint: "回到最初的样子：默认配色、圆形灯珠、柔光呼吸，边框光全关。",
    skinFocus: "专注",
    skinFocusHint: "平时不吵，只在要你处理时上边亮一道细线；左上挂置顶、右上挂静音，动效全关。",
    skinNeon: "霓虹",
    skinNeonHint: "高饱和配色 + 纯黑牌底 + 胶囊轮廓，上下辉光、四角状态色，灯珠随节奏脉冲。",
    skinPaper: "纸感",
    skinPaperHint: "暖黑牌底、明显描边、大圆角和拱形轮廓，方灯珠，整块牌轻轻淡入淡出。",
    skinSignal: "信号",
    skinSignalHint:
      "色盲友好配色 + 菱形灯珠 + 对角圆轮廓，一道高光扫过牌面；上边只在有消息时亮、下边只在要你处理时亮。",
    shapeSquare: "方",
    shapeDiamond: "菱",
    shapeCapsule: "胶囊",
    outlineTitle: "牌的轮廓",
    outlineHint:
      "在圆角之外换整块牌的形状。四档都只用圆角画，不会裁掉四角上的小灯；每档的角灯落点会跟着轮廓一起走。",
    outlineRect: "矩形",
    outlineCapsule: "胶囊",
    outlineLeaf: "对角圆",
    outlineArch: "拱形",
    effect: "效果",
    effectHint:
      "以前只有一个「灵动效果」开关，现在五选一：柔光呼吸就是原来开着的样子。系统开了「减少动态效果」时，五档一律自动降级成静止。",
    effectOff: "关闭",
    effectBreathe: "柔光呼吸",
    effectPulse: "脉冲",
    effectSweep: "扫光",
    effectFade: "渐隐",
    edgeLine: "实线",
    edgeFade: "两端淡出",
    slotTop: "上边",
    slotBottom: "下边",
    slotKind: "画法",
    slotTrigger: "触发",
    slotBlink: "闪烁",
    slotBlinkHint:
      "跟着这块牌的状态节奏闪，和灯珠说的是同一件事。这盏灯本身不画东西时无从闪，开关会变灰。",
    slotsHint:
      "六个灯位各自挑画法和触发条件：上边可以常亮、下边只在出错时亮，四个角也能各挂各的、各判各的 —— 不再是一整条轨一起开关。判定逐块牌进行，同一盏灯里可以有牌亮、有牌不亮。",
    signEmphasis: "分层",
    emphasisAuto: "区分",
    emphasisFlat: "一致",
    emphasisHint:
      "选「区分」时，需要你动手的那几块牌更实、灯珠更大，其余退后一档，好让人一眼知道先看谁。觉得这个差异莫名其妙就选「一致」，所有牌同等分量。",
    fieldDot: "圆点",
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
    stayDuration: "停留时长",
    stayDurationHint:
      "这个状态静默多久算过期。没有「永不过期」—— 不会自己熄灭的灯等于一条永远不撤的告警。改动只在下一次上报时生效，不会追溯已经亮着的牌。",
    silenceTimeouts: "静默超时",
    silenceTimeoutsHint:
      "智能体不出声之后，这块牌还会在灯上待多久。调大它，「还在干活」就不容易被误判成「沉默」；代价是真正崩掉的智能体晚一点被发现。改动只在下一次上报时生效。",
    silenceTimeoutsMore:
      "其余状态（空闲 / 需要关注 / 需要授权 / 出错）在「外观 → 逐个状态覆盖」里单独调。",
    ttlDone: "任务完成后保持",
    ttlWorking: "干活时的静默容忍",
    ttlThinking: "思考时的静默容忍",
    behaviorWindowTitle: "窗口与交互",
    clickAction: "单击灯做什么",
    clickActionHint: "把最顺手的单击留给最常做的事。长按呼出的圆环菜单、拖拽、右键菜单都不变；默认仍是折叠 / 展开，升级不会改变现有手感。",
    clickActionLabels: { collapse: "折叠 / 展开", inbox: "打开收件箱", none: "不做事" },
    resetToGlobal: "恢复成跟随全局",
    styleHint:
      "「跟随」= 形状和节奏用「外观」页选的那两项，停留时长用内置默认（跟在每一项里）。停留时长指的是「智能体不出声之后，这块牌还在灯上待多久」—— 它只在「下一次上报」时生效，不会追溯已经亮着的牌。",
    perStateStyle: "逐个状态覆盖",
    perStateSome: (count) => `${count} 个状态单独设过`,
    perStateNone: "未单独设置",
    groupDisplayTitle: "各组的显示",
    groupDisplayEmpty: "还没有任何分组 —— 接上智能体、或调用上报接口之后，它们会出现在这里。",

    notifyWhenNeeded: "需要时弹通知",
    notifyWhen: "在哪些状态下提醒",
    notifyWhenHint:
      "只在整体状态变成这几个时弹。一个都不选就不弹 —— 想彻底关掉用上面那个总开关。和「转发」的状态过滤是同一套判断，只是「转发」默认不筛、这里默认只筛需要你处理的那三个。",
    soundWithNotify: "通知带提示音",
    alertAfter: "持续多久才提醒",
    alertAfterHint: "状态要持续这么久才弹通知，一闪而过的忽略掉",
    quietHours: "勿扰时段",
    alertHint:
      "弹一条系统通知，告诉你是哪一组、卡在什么状态。声音挂在通知上，所以关掉通知也就没有声音。勿扰时段里不弹也不出声，灯照常显示 —— 而且不补发：早上醒来收到一堆昨晚的通知，正是勿扰想避免的。",
    rulesTitle: "预设规则",
    rulesHint:
      "三条写死的规则，各自一个开关 —— 不做表达式引擎。它们只决定「什么时候打扰你」，不改变灯的聚合、排序和转发。",
    ruleStuckPush: "授权卡住 60 秒 → 再推一次手机",
    ruleStuckPushHint:
      "某组停在「需要授权」超过 60 秒还没人处理时，用「转发」里配好的手机推送钩子（Bark / PushDeer / ntfy……）再发一次 —— 没配钩子就没有东西可发。一动就批准的权限请求不值得吵你。",
    ruleBusySilence: "有会话在跑 → 暂停通知",
    ruleBusySilenceHint:
      "任一智能体处于「思考 / 执行」时，不弹系统通知和重复提醒（授权提醒也一样）。灯照常显示、上报照常进来；忙完那一轮不会补发。",
    ruleMeeting: "会议中 → 灯变蓝 + 静音",
    ruleMeetingHint:
      "配合「会议中」采集器：开会期间会议那块牌画成蓝色，系统通知与提醒暂停；散会后不补发。蓝色只表达「在开会」，不改变排序和托盘。",
    delayNow: "立刻",
    alertForwardHint: "想让状态变化转发到服务器，或跑一条命令？在「转发」页。",

    pauseMonitoring: "暂停监控",
    openConfigDir: "配置目录",
    openLogs: "日志",
    pauseHint: "暂停后灯球显示为熄灭，但会话仍在记录 —— 恢复时立刻回到真实状态",
    outboundHooks: "通知转发",
    configure: "去配置",
    connectHookHint: "状态变化时转发到你的服务器，或跑一条本地命令 —— 在「转发」页配置。",
    groupBy: "分组方式",
    groupByNone: "不分组",
    groupByCwd: "按目录",
    groupBySession: "按会话",
    groupByHint:
      "这一项定的是**有哪些组**（下面那一项定的是这些组摆成几盏灯，两件事）。「不分组」：所有不带 group 的上报归到同一块牌。「按目录」：一个工作目录一块牌，每个项目各占一行。「按会话」：一个会话一块牌 —— 同一个智能体并行开几个窗口 / worktree 时，它们的状态各不相同，只有按会话分才看得出来。三种都只是兜底：上报自己带了 group 的仍然听它的，也是改完从下一次上报开始生效。按目录时分组名就是那条路径；按会话时不会把会话 id 画在牌上，默认显示成「目录末段 · 智能体」（项目在前，因为牌上的名字会被截断，而同一个智能体开好几个窗口时只有项目名能区分它们）。想换别的名字，改下面「各组的显示」。",
    port: "端口",
    apiTitle: "接口速查",
    apiToken: "Token",
    apiAuthHint:
      "本机接口的凭据，别外传。请求要带 Authorization: Bearer <token>，并且不能带 Origin 头（防本地 CSRF）。端口和 token 都写在配置目录下的两个文件里。",
    apiEndpoints: "端点",
    apiEndpointReport: "上报状态",
    apiEndpointState: "当前聚合 + 各组",
    apiEndpointSessions: "会话列表",
    apiEndpointClear: "清除某个会话",
    apiEndpointHealth: "健康检查（不需要鉴权）",
    apiStates: "可上报的状态",
    apiStatesHint:
      "写错状态名会被明确拒绝（400），不会静默忽略 —— 那是「灯没反应」最难排查的一种成因。stale / offline 是聚合派生出来的，不能上报。",
    apiExamples: "示例",
    copy: "复制",
    copied: "已复制",
    openDocs: "完整文档（GitHub）",
    cliExample: `# 或者用 CLI：
signalhub working --session build-1 --agent ci --message "compiling"
signalhub status
signalhub watch --until done --timeout 10m   # CI 里等某个状态`,

    agentsTitle: "智能体接入",
    agentsHint:
      "把 hook 写进智能体自己的配置文件，重启那个智能体就会开始上报状态。只动 hooks 这一个键，写之前先把整个文件备份一份，随时可以卸载。",
    agentInstalled: "已接入",
    agentDetected: "检测到，未接入",
    agentMissing: "未检测到",
    agentCustom: "自定义",
    customAgentsTitle: "自定义智能体",
    customAgentsHint: (families) =>
      `清单里没有的那一家不用等我们适配：在配置文件里写一条，说明它和哪一家同构（${families}）以及配置写在哪，它就会出现在上面那张表里，接入 / 卸载和别的完全一样。改完保存、切回这个窗口就会重新读一遍。`,
    customAgentsLimit:
      "只覆盖「格式同构、路径不同」这一类 —— 事件名是另一套的，仍然得等我们加一家。",
    customAgentsOpen: "打开配置文件",
    customAgentAddToggle: "在面板里添加",
    customAgentAddTitle: "添加一条预设",
    customAgentAddHint:
      "不用直接改配置文件：填完走应用内的写入命令，会先校验、保留已有条目，也不会覆盖坏掉的 JSON。拿不准同构家族就别猜 —— 猜错会装出一份看着正常、却永远不报状态的 hook。",
    customAgentFieldId: "id（上报时的名字）",
    customAgentFieldName: "显示名称",
    customAgentFieldFamily: "同构家族",
    customAgentFieldConfig: "hook 配置文件路径",
    customAgentFieldMarker: "安装痕迹目录（可选）",
    customAgentFieldCommand: "命令名（可选）",
    customAgentReplace: "覆盖同名条目",
    customAgentSubmit: "写入并校验",
    customAgentCancel: "收起",
    customAgentProblems: {
      idRequired: "请填 id。",
      nameRequired: "请填显示名称。",
      familyRequired: "请选一个同构家族。",
      configRequired: "请填 hook 配置文件路径。",
      configParent: "路径里不能有 .. —— 请填你自己的配置文件路径。",
      commandSpaces: "命令名不能带参数或空格。",
    },
    customAgentOutcome: (outcome, id) =>
      outcome === "added"
        ? `已写入 ${id}，列表已刷新。`
        : outcome === "unchanged"
          ? `${id} 已存在且内容相同，没有改动文件。`
          : `已覆盖 ${id}，列表已刷新。`,
    setupPromptTitle: "让智能体自己来配",
    setupPromptHint:
      "不确定自己用的这一家在不在清单里？把整段提示词粘给它，让它自己判断该走哪条路、写好配置、装好 hook。里面带着这台机器当前的清单、路径和端口，不用你先去查文档。",
    setupPromptCopy: "复制接入提示词",
    setupPromptGuide:
      "清单里没有你用的智能体？复制一段提示词给它，让它自己判断并配好 —— 不用先读文档。",
    agentCommandBroken:
      "接入是装上了，但配置里那条命令现在跑不起来 —— 智能体不会报错，灯也不会亮。多半是应用被挪走或删掉了，重新点一次「接入」就会改写成当前路径：",
    agentUpdateAvailable:
      "接入是旧版本写的 —— 命令还在跑，但行为停在上一版（典型：智能体进程号没跟着传，灯会在超时后回到「沉默」）。点「重新接入」就地改写成当前版本的命令：",
    agentReinstall: "重新接入",
    agentReceiptAgo: (age, state) => `最近上报 ${age}前 · ${state}`,
    agentReceiptCount: (n) => `本次运行 ${n} 条`,
    agentReceiptNever:
      "还没有收到过它的上报 —— 用它一次；要是灯还是不亮，看下面「最近的 hook 调用」有没有行。",
    systemSourcesTitle: "系统信号源",
    systemSourcesHint:
      "上报时声明了 source: system 的信号源 —— 内置采集器（会议中、备份中……）会是这一组；你自己盯系统状态的脚本也放这儿。",
    systemSourcesEmpty:
      "还没有。报一个试试：signalhub working --source system --agent meeting",
    collectBackup: "Time Machine 备份中",
    collectBackupHint: (secs) =>
      `每 ${secs} 秒看一次 Time Machine 的状态；开始备份时灯上多一组「备份」，结完自动报「备份完成」。只读系统状态，不读任何文件内容；万一它自己出了问题，限度是「那一组消失」，不会反过来影响别的。`,
    collectMeeting: "会议中（麦克风 / 摄像头被占用）",
    collectMeetingHint: (secs) =>
      `每 ${secs} 秒看一次麦克风和摄像头有没有被别的应用占用；开会时灯上多一组「会议」，结束后自动报「会议结束」。只读「设备在不在用」，**不采集、不保存任何声音或画面**，也不会因此弹权限请求。`,
    collectUnknown: (id) => `采集器「${id}」`,
    otherSourcesTitle: "自定义",
    otherSourcesHint:
      "收到过上报、但不在上面清单里的（你自己的脚本、signalhub run 包的命令、别的机器、手写的一条 curl……）。有这一行就说明上报通道是通的。",
    agentTry: "自检",
    agentTryBusy: "试…",
    agentTryRanOk: (command) =>
      `真的跑了一遍配置里那条命令（${command}）—— 命令跑起来了。`,
    agentTryRanFailed: (command, code, output) =>
      `跑配置里那条命令（${command}）失败了${code ? `（退出码 ${code}）` : ""}${output ? `：${output}` : "。"}`,
    agentTryHttpOk: (url) =>
      `真的发了一次 HTTP 到 ${url} —— 端点答了。这条链路是通的（它证不了对方会不会调我们，那个看上面那行回执）。`,
    agentTryHttpFailed: (url, output) =>
      `发 HTTP 到 ${url} 失败${output ? `：${output}` : "。"}`,
    agentTryPlugin: (states) =>
      `这条接入是插件式，没有能单独运行的东西 —— 刚才是由应用内部注入了一遍 ${states}，证明灯和状态机是好的；它会不会调用我们，看上面那行回执。`,
    diagnoseCopy: "复制诊断信息",
    diagnoseHint:
      "一段纯文本：版本、上报端口、待补报条数、每个信号源的配置路径与那条命令还跑不跑得起来、最近的上报、最近的 hook 日志。接不上时先复制它 —— 该看的东西都在里面，不用在三个页签之间翻。",
    diagLogTitle: "最近的 hook 调用",
    diagLogHint:
      "每次 hook 被调用都会记一行（也在这里记下调用失败）。所以：接了却完全不亮时先看这里 —— 一行都没有，说明事件根本没从智能体那边打过来（多半是那条命令跑不起来，见「接入」页的红色提示）；有行但都写着失败，说明打过来了、只是没送到应用。",
    diagLogEmpty: "还没有记录 —— 一次 hook 都还没跑过。",
    diagRefresh: "刷新",
    agentConnect: "接入",
    agentDisconnect: "卸载",
    agentConnectHttp: "用 HTTP 接入（不需要命令）",
    agentSwitchHttp: "改用 HTTP",
    agentSwitchCommand: "改用命令",
    agentModeHttp: "接入方式 HTTP：不需要那条命令存在，所以「命令不在 PATH 上」这类失败不存在。代价：开了「允许局域网访问」时这个端点会关掉。",
    approveTools: "从灯上审批",
    approveToolsHint:
      "Claude Code 每次要问你批准某个工具调用时，把那个确认框搬到灯上：灯上弹一条气泡，点「允许」或「拒绝」。这是唯一会改变智能体行为的开关 —— 开着的时候它会**等你**（最多 45 秒），没人点就交回终端里的确认框。命令全文只出现在本机这个窗口里，状态上报仍然只有那个短词。",
    approveScopeHint:
      "「从灯上审批」目前只支持 Claude Code，而且要用 HTTP 方式接入 —— 所以那个开关只出现在它的卡片里。其他智能体（含 WorkBuddy）需要授权时，灯照样变红、通知照样发，但确认框仍在智能体自己的界面里，请回到那边点允许 / 拒绝。",
    approvalAllow: "允许",
    approvalDeny: "拒绝",
    approvalTooLong: "这条在灯上看不全，只能在终端里批准（或者在这里拒绝）",
    approvalMore: (n) => `还有 ${n} 条等着`,
    approvalWaiting: (secs) => (secs < 5 ? "刚来" : `等了 ${secs}s`),
    approvalDanger: {
      delete: "这条会删文件",
      "force-push": "这条会改写远端历史",
      rewrite: "这条会丢掉本地改动",
      sql: "这条会删表 / 删库",
      "pipe-shell": "这条会下载并直接执行脚本",
      disk: "这条会直接写磁盘",
      privilege: "这条会提权",
      permissions: "这条会把权限放开",
    },
    approvalHold: "按住 0.8 秒确认",
    approvalReason: "给个理由",
    approvalReasonUnsafe: "太危险",
    approvalReasonSafer: "换个更安全的做法",
    approvalReasonExplain: "先解释再试",
    approvalReasonCustom: "自定义",
    approvalReasonCustomPlaceholder: "想让智能体怎么做？",
    approvalLogTitle: "审批记录",
    approvalLogHint:
      "最近 100 条：什么时候、从哪儿、放行或拒绝了什么。详情只留前 200 个字符。",
    approvalLogEmpty: "还没有审批记录",
    approvalLogDecision: (decision) =>
      ({ allow: "允许", deny: "拒绝", expired: "超时" } as Record<string, string>)[
        decision
      ] ?? decision,
    approvalLogSource: (source) =>
      ({ lamp: "灯上", web: "手机", api: "接口" } as Record<string, string>)[source] ??
      source,
    agentModeCommand: "接入方式 命令",
    cliAgentsTitle: "其他工具",
    cliAgentsHint:
      "两条路，按对方的情况挑。包一条命令：不管对方是什么，只要能跑命令就行 —— 跑起来报「执行中」，退出后按退出码报「已完成 / 出错」，不用它有任何 hook 支持。在它自己的事件钩子里调：对方有事件钩子时更精细（能区分「需要授权」和「出错」）—— 上面列表里没有的、但有 hook 配置的智能体走这条。VS Code 系里跑在集成终端（含任务）的用第一条；只活在侧边栏面板里的扩展（Cline / Roo Code / Continue 这类）接不了 —— 它既没有可包的进程，也没有可写的 hook 配置。",
    cliAgentsWrap: "包一条命令",
    cliAgentsHookName: "在自己的事件钩子里调",
    tryItTitle: "试一下",
    tryItHint:
      "点一下，灯上立刻出现那个状态。它走的是和 HTTP 上报完全相同的链路（聚合、窗口尺寸、今日统计、推送），所以能回答「灯到底通没通」—— 但不验证 HTTP 那一层（服务起没起来、token 对不对），那要用「指南」页那条 curl。牌上会显示 test 组，20 秒后自己走掉，不会留下东西。",
    hookResult: ({ added, updated, already, removed }) => {
      const parts: string[] = [];
      if (added > 0) parts.push(`新增 ${added} 个事件`);
      if (updated > 0) parts.push(`更新 ${updated} 个事件`);
      if (removed > 0) parts.push(`移除 ${removed} 个 handler`);
      if (parts.length === 0) {
        return already > 0 ? `无需改动（${already} 个事件已是最新）` : "无需改动";
      }
      return parts.join("，");
    },
    hookPluginWritten: (path) => `已写入插件：${path}`,
    hookPluginRemoved: (path) => `已删除插件：${path}`,

    noHooks: "还没有钩子。加一条，就能在状态变化时 POST 到某个地址或跑一条命令。",
    hookN: (n) => `钩子 ${n}`,
    noTarget: "未填目标",
    devicesTitle: "物理灯（WLED）",
    noDevices: "还没有灯。填一台 WLED 的地址，状态色就直接发到它上面。",
    deviceN: (n) => `灯 ${n}`,
    addDevice: "+ 添加一盏灯",
    devicePlaceholder: "192.168.1.50 或 wled.local:8080",
    devicesHint:
      "只填主机，带不带 http:// 都认。发出去的东西只有颜色：没有组名、没有会话名、没有消息内容。填完保存就会立刻按当前状态亮一次 —— 那一下就是「通不通」的回答，结果记在上面同一份日志里。",
    type: "类型",
    command: "命令",
    addHook: "+ 添加钩子",
    hooksHint:
      "状态全不选 = 任何变化都触发。命令通过 sh -c 执行，超时 10 秒 —— 配置文件在你手里，所以这里没有额外的开关。",
    commandPlaceholder: "say 完成",
    hookGroups: "只看这几组",
    hookGroupsHint:
      "不选组 = 看全局聚合：整体状态变到上面勾选的状态时才触发。选了组 = 只看这几个组各自的跃迁，不看整体 —— 这样一组出错时，别的组更紧急不会把它盖掉。",
    groupNotReporting: "这一组当前没有上报，仍然挂在钩子上",
    neverFired: "还没触发过",
    firedAgo: (age) => `${age}前`,

    hookPreset: "发什么形状",
    presetNames: {
      raw: "原样 JSON（自建接收端）",
      text: "纯文本",
      ntfy: "ntfy",
      bark: "Bark",
      slack: "Slack",
      discord: "Discord",
      lark: "飞书",
      wecom: "企业微信",
      dingtalk: "钉钉",
      telegram: "Telegram",
      pushdeer: "PushDeer",
      custom: "自定义请求体",
    },
    presetUrls: {
      raw: "https://你的服务器/ingest",
      text: "https://你的服务器/hook",
      ntfy: "https://ntfy.sh/my-topic",
      bark: "https://api.day.app/你的key",
      slack: "https://hooks.slack.com/services/T…/B…/…",
      discord: "https://discord.com/api/webhooks/…/…",
      lark: "https://open.feishu.cn/open-apis/bot/v2/hook/…",
      wecom: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=…",
      dingtalk: "https://oapi.dingtalk.com/robot/send?access_token=…",
      telegram: "https://api.telegram.org/bot<token>/sendMessage?chat_id=…",
      pushdeer: "https://api2.pushdeer.com/message/push?pushkey=…",
      custom: "https://你的服务器/hook",
    },
    presetHint:
      "除了「原样 JSON」和「自定义请求体」，其余都只发下面那行文字 —— 那些服务要的就是「一个地址 + 一句话」，服务端自己排版。选完先用「发送测试」看一眼手机收到的是什么。",
    hookBody: "请求体（自定义）",
    hookBodyPlaceholder: '{"text":"{state} · {group}","message":"{message}"}',
    hookBodyHint:
      "占位符：{state} {aggregate} {group} {message} {duration} {since_ms} {at_ms} {summary}。content-type 是 JSON 时，替换进去的值会自动转义引号和换行；值本身不加引号，字符串位置请自己写双引号。",
    hookContentType: "content-type",
    hookHeaders: "请求头（鉴权用）",
    headerNamePlaceholder: "Authorization",
    headerValuePlaceholder: "Bearer …",
    addHeader: "+ 添加请求头",
    headersHint:
      "要鉴权的地址靠它，例如 Authorization: Bearer …、ntfy 的 X-Token。最多 16 条。",
    headerDuplicate: "这个名字已经有了（头名不区分大小写）",
    sendTest: "发送测试",
    testNothing: "还没有填好目标的转发",
    testSent: (n) => `已发出 ${n} 条 —— 结果在下面每条自己的那一行`,

    alwaysOnTop: "保持在最前",
    trayFollowsState: "托盘跟随状态",
    launchAtLogin: "开机自启",
    clickThrough: "点击穿透",
    systemHintTray:
      "托盘跟随状态时，菜单栏那颗点会变成当前状态的颜色 —— 不看悬浮灯也知道现在什么情况。关掉就回到单色字形，跟着菜单栏明暗自动反色。",
    systemHintPassThrough: (shortcut) =>
      `穿透后圆点不再响应鼠标，按 ${shortcut} 或从托盘菜单退出。该模式不会保留到下次启动。`,
    shortcutFallback: "快捷键",
    shortcutsTitle: "全局快捷键",
    shortcutActions: {
      lamp: "显示 / 隐藏灯",
      mute: "静音最差的那一组",
      clear: "清除最差的那一组",
      inbox: "打开收件箱",
      pass_through: "切换点击穿透",
    },
    shortcutTaken: "被别的程序占用了",
    shortcutsHint:
      "快捷键是固定的（不做可配：真正的故障是被别的程序占了，那件事可配也解决不了）。抢不到的那一个会在上面标出来。「最差的那一组」= 紧急度最高的那一组，和灯上最上面那块牌同一个判据；置顶只改显示顺序。",
    language: "语言",
    langAuto: "跟随系统",
    langZh: "中文",
    langEn: "English",
    theme: "界面主题",
    themeAuto: "跟随系统",
    themeLight: "亮色",
    themeDark: "深色",
    themeHint:
      "只影响设置窗口和演示页；灯和收件箱是透明的深色悬浮牌，在任何系统外观下都保持原样。",

    configBackup: "配置备份",
    exportConfig: "导出…",
    importConfig: "导入…",
    configBackupHint:
      "导出成一份 JSON（颜色、形状、分组、勿扰、钩子、出站转发等全部偏好）。换机器或重装时导入回去就行。导入会先校验一遍：颜色格式不对或状态名不认识会明确报错，而超范围的值（比如尺寸填了 500）会被夹回合法区间，不会整份拒绝。",
    exportDone: (path) => `已导出到 ${path}`,
    importDone: "已导入，设置已生效",
    configExportTitle: "导出设置",
    configImportTitle: "导入设置",
    configFilterName: "SignalHub 设置",

    updateTitle: "自动更新",
    updateCurrent: (version) => `当前版本 v${version}`,
    checkUpdate: "检查更新",
    checkingUpdate: "正在检查…",
    updateLatest: "已经是最新版本",
    updateAvailable: (version) => `有新版本 v${version}`,
    updateDownloadInstall: "下载并安装",
    updateDownloading: (progress) => `下载中 ${progress}`,
    updateInstalling: "正在安装，装完会自动重启",
    updateNotes: "这次改了什么",
    updateHint:
      "更新不会自动装：只有点了「下载并安装」才会下载、验签、替换并重启。启动后每天静默查一次，失败只写日志、不打扰你。",
    updateUnsupported:
      "这个平台暂时只能手动下载。Windows 上装更新要用的那个压缩库被上游全量撤下了；Linux 上两种装法还没在真机上验过（deb 那份还要管理员权限）。宁可说清楚，也不摆一个点了会报错的按钮。",
    updateFailed: (reason) => `检查失败：${reason}`,
    openReleases: "打开发布页",

    lanAccess: "允许局域网访问",
    lanAccessHint:
      "关着的时候只有本机能上报。打开后，别的机器 / 容器 / WSL 里的智能体也能连进来 —— 代价是「同一网段里任何人只要拿到 token 就能上报」：那道拒绝带 Origin 请求的防线挡的是网页偷偷打本机端口，对 curl 这类客户端无效。所以要开的话，请确认这个网络是你信得过的（家里的、公司内网），别开在公共 WiFi 上。",
    lanAddress: "局域网地址",
    lanRestart: "改完要重启应用才生效。",
    webPanel: "只读 Web 面板",
    webPanelUrl: "面板地址",
    webPanelHint:
      "打开后可以在手机 / 平板上看灯（状态、每组在做什么、已持续多久）。默认只读 —— 只有下面那个「在面板上审批」也打开，才会多出允许 / 拒绝两个按钮。改完立刻生效。",
    webPanelLanWarning:
      "注意：局域网访问也开着，所以同一 WiFi 下的任何设备，只要拿到上面那条带 token 的链接，就能看到你的灯。别把链接发到群里，也别在公共 WiFi 上开着。",
    webApprove: "在面板上审批",
    webApproveHint:
      "打开后，等你批准的调用会出现在面板上，可以直接点允许 / 拒绝（拒绝还能选一句理由）。这是明确同意把命令原文和文件路径送到那个页面上 —— 它可能正投在客厅的平板上，或者那条链接被转发过。只在「从灯上审批」也开着时有用。",
    webApproveNeedsTools: "「从灯上审批」现在是关的，所以这个开关暂时不会做任何事。",

    quit: "退出 SignalHub",

    lampHint: {
      collapse: "单击折叠 / 展开 · 长按呼出圆环菜单 · 右键牌面是单组菜单 / 空白处是灯菜单",
      inbox: "单击打开收件箱 · 长按呼出圆环菜单 · 右键牌面是单组菜单 / 空白处是灯菜单",
      none: "单击不做事 · 长按呼出圆环菜单 · 右键牌面是单组菜单 / 空白处是灯菜单",
    },
    justFinished: (minutes) =>
      minutes < 1 ? "刚完成 · 刚刚" : `刚完成 · ${minutes} 分钟前`,
    groupsCount: (n) => `（${n} 组）`,
    moreGroups: (n) => `还有 ${n} 组`,
    expandNow: "已展开",
    collapseNow: "已折叠",
    lampPaused: "已暂停监控",
    clearSessionHint: "清掉这个会话（智能体下次上报会回来）",
    lampPassThrough: "点击穿透",
    moreGroupsLead: "被收起来的组：",

    lampMenuInbox: "收件箱",
    lampMenuLabel: "灯菜单",
    lampMenuSettings: "设置",
    lampMenuPause: "暂停",
    lampMenuResume: "继续",
    lampMenuQuit: "退出",
    lampMenuCloseHint: "点空白处或按 Esc 关闭",

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

    machines: {
      title: "多机中枢",
      hint: "把别的机器上的看板拉过来，合并进同一盏灯 —— 灯、收件箱、统计、物理灯都算上。",
      empty:
        "还没有配置任何机器。加一台之后，它的智能体状态会以「机器名 · 组名」出现在灯上。",
      add: "添加一台",
      name: "名称",
      nameHint: "这台机器在灯上的名字，也是它的命名空间。改了等于换了一台机器。",
      url: "地址",
      urlHint: "对端的局域网地址，如 192.168.1.20:41320。在对端「设置 → 接入」里能看到。",
      token: "Token",
      tokenHint: "对端 ~/.signalhub/token 的内容，同样在「设置 → 接入」里。",
      enabled: "启用",
      test: "测试连接",
      testing: "连接中…",
      remove: "删除",
      connected: (ms, sessions) => `连通 · ${ms}ms · ${sessions} 个会话`,
      never: "还没连上过",
      lastOk: (ago) => `上次成功 ${ago}`,
      poll: "轮询间隔",
      pollHint:
        "多久去每台机器取一次。远程会话的存活时间是它的三倍 —— 所以它也决定了掉线多久之后在灯上显形。",
      pollSeconds: (seconds) => `${seconds} 秒`,
      note: "远程会话只能看：不能在灯上审批，也不能跳转到终端（那个目录在对端机器上）。",
      peerHint: "对端要打开「允许局域网访问」，把 token 抄过来即可 —— 对端不用升级到新版本。",
    },

    callme: {
      linksTab: "链接",
      serverTab: "服务器",
      manageLinks: "管理链接",
      urlLabel: "服务器地址",
      urlPlaceholder: "https://callme.example.com",
      urlHint: "你自己部署的 Call Me 服务器地址。不写 https:// 时按 https 补。",
      tokenLabel: "管理 Token",
      tokenPlaceholder: "服务器上的 CALLME_ADMIN_TOKEN",
      tokenHint: "启动服务器时设置的那个值，客户端用它来拉取留言。",
      connectionHint:
        "服务器要自己部署（或用上面的本机服务），TLS 由反向代理负责；地址和管理 token 只存在你这台机器上。",
      pollLabel: "检查间隔",
      pollSeconds: (seconds) => `${seconds} 秒`,
      test: "测试连接",
      testing: "连接中…",
      connected: (ms) => `已连接 · ${ms}ms`,
      lastOk: (ago) => `上次成功 ${ago}`,
      never: "还没连上过",
      unconfigured: "未配置",
      openServerTab: "服务器设置",
      sourcesTitle: "链接（来源）",
      sourcesEmpty: "还没有链接。点一下「新建链接」，把链接贴到文章里即可 —— 名字和出处不用填。",
      addSource: "新建链接",
      creating: "创建中…",
      autoNameHint: "不用填名字：第一条留言会把标题和出处自动填上。",
      pendingName: "等第一条留言命名",
      copyLink: "复制链接",
      copiedLink: "已复制",
      renameLink: "改名",
      renamePlaceholder: "给这条链接起个名字",
      openUsage: "打开链接看用法",
      revoke: "吊销",
      revokeConfirm: "吊销后这条链接立即失效（已收到的留言保留）。它会收进下面的「已吊销」，随时能恢复或彻底删除。继续？",
      revoked: "已吊销",
      deleteLink: "彻底删除",
      deleteConfirm:
        "删除后这条链接立即失效并从列表移除，位置也让出来。已经收到的留言会保留，但会失去来源名称。这个动作不能撤销，继续？",
      restoreLink: "恢复",
      revokedTitle: (n) => `已吊销 (${n})`,
      revokedHint: "这些链接不再收信。恢复可以继续用，删除则彻底清掉 —— 已收到的留言两种情况下都保留。",
      activeEmpty: "没有在用的链接。新建一条，或在下面恢复一条已吊销的。",
      deletedSource: "已删除的链接",
      todayCount: (n) => `近 24h ${n} 条`,
      totalCount: (n) => `共 ${n} 条`,
      createdTitle: "链接已创建",
      createdHint:
        "复制它，贴到你希望智能体读到的地方。智能体打开这条链接就会看到怎么留言（同一条链接，GET 是用法、POST 是留言），不用你再附一段说明。任何读到它的人都能发消息 —— 额度用完为止，不想再收时可以吊销（可恢复）或彻底删除。",
      filterAll: "全部",
      messagesEmptyNoServer: "还没配置 Call Me 服务器 —— 填好地址和 token 之后才会去拉留言。",
      messagesEmptyOffline: "连不上 Call Me 服务器，先检查地址、token 和服务是否在跑。",
      messagesEmptyNoSource: "还没有链接 —— 智能体没有入口可发。",
      messagesEmpty: "还没有留言。",
      filterNoResults: "这个筛选下没有留言。",
      goConfigure: "去配置 Call Me",
      checkServer: "去检查服务器",
      goCreateLink: "去新建链接",
      unread: (n) => `${n} 条未读`,
      markAllRead: "全部标为已读",
      openSource: "打开出处",
      copyText: "复制全文",
      copiedText: "已复制",
      localTitle: "本机一键服务",
      localHint: "在桌面应用里直接跑一个收信服务 —— 不用装 Docker，不用敲命令。地址和 token 会自动填好；第一次启动还会顺手建好第一条链接，切回「链接」子页复制即可。",
      localPort: "端口",
      localPortInvalid: "端口要在 1024–65535 之间。",
      localReplaceConfirm:
        "你现在填的是一台外部服务器（可能是你的 VPS）。启动本机会把地址和 token 换成 127.0.0.1 这一份，原来的配置会被覆盖 —— 需要的话先把那两行抄下来。继续？",
      localStart: "启动本机服务",
      localStop: "停止",
      localStarting: "启动中…",
      localStopping: "停止中…",
      localRunning: "运行中",
      localStopped: "未运行",
      localOnlyHint:
        "本机服务只监听 127.0.0.1：本机测试够用，但外网访问不到。要让外面的智能体投递，展开下面的「部署到公网」。它跟着应用一起退出，下次要再点一次启动 —— 不会在你不知情的时候常驻。",
      advancedTitle: "高级：手动连接、端口与检查间隔",
      remoteTitle: "部署到公网（VPS）",
      remoteHint: "要让外面的智能体访问，把服务跑在一台公网机器上。三条命令，复制后在服务器上执行：",
      remoteStep1: "在有 Docker 的公网机器上克隆仓库并构建镜像。",
      remoteStep2: "设置 CALLME_ADMIN_TOKEN（至少 16 个字符），启动容器并挂载数据卷。",
      remoteStep3: "用 HTTPS 反向代理把域名指向 127.0.0.1:8080，再把地址和 token 填到上面。",
      copyCommand: "复制命令",
      copiedCommand: "已复制",
      remoteSecurityHint:
        "管理 token 不要发给任何人；反代要加 TLS，并在反代场景下设置 CALLME_TRUST_PROXY=true。数据都在 /data 卷里，备份它就是备份全部。",
    },

    inbox: {
      title: "收件箱",
      need: "需要你",
      running: "运行中",
      done: "刚刚完成",
      quiet: "空闲 / 沉默",
      emptyTitle: "现在没有需要你处理的事",
      emptyHint: "智能体在跑的时候这里会安静；有授权、出错或完成时会出现在这里。",
      emptyConnect: "接入智能体",
      emptyDemo: "看演示",
      more: "更多动作",
      paused: "监控已暂停",
      jump: "跳转",
      allow: "允许",
      deny: "拒绝",
      mute: "静音该组",
      unmute: "取消静音",
      clear: "清除",
      watch: "跑完叫我",
      unwatch: "取消盯",
      watchSet: (label) => `已盯住「${label}」，结束时通知你`,
      watchCleared: (label) => `已取消盯住「${label}」`,
      close: "关闭",
      settings: "设置…",
      truncated: "命令太长，只能拒绝；要放行请回终端里看全。",
      allowBlocked: "这条在灯上看不全，只能拒绝",
      allowed: "已允许，等智能体继续",
      denied: "已拒绝",
      cleared: "已清除这个会话",
      muted: (group) => `已静音「${group}」`,
      unmuted: (group) => `已取消静音「${group}」`,
      waiting: (age) => `等了 ${age}`,
      keys: "↑↓ 选择 · Enter 跳转 · A 允许 · D 拒绝 · M 静音 · W 盯住 · Esc 关闭",
      jumpUnsupported: "精确定位终端目前只在 macOS 上；已尝试打开项目目录。",
      remoteJump: (machine) => `这是 ${machine} 上的会话，跳转只对本机有效`,
      approvalHint: "这是智能体主动请求的授权；允许只对被问到的这一次生效。",
      closeHint: "关闭收件箱（Esc）",
      awayFor: (ago) => `你离开的 ${ago} 里`,
      awayDone: (count) => `${count} 个完成`,
      awayFailed: (count) => `${count} 个失败`,
      awayNeed: (count) => `${count} 个在等你`,
      awayRunning: (count) => `${count} 个还在跑`,
      awayTransientDone: (count) => `另有 ${count} 个跑完就结束了`,
      awayTransientFailed: (count) => `另有 ${count} 个失败后结束了`,
      awayQuiet: "没有新动静",
      awayDismiss: "知道了",
    },
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

    railWatch: "Workspace",
    railLamp: "Lamp",
    railNotify: "Notifications",
    railConnect: "Connections",
    railHelp: "Help",
    tabStatus: "Status",
    tabHistory: "History",
    tabMessages: "Messages",
    historyTitle: "Last 7 days",
    historyHint:
      "One row per day. A segment's width is how long that state occupied the lamp (within a day, longest first — hover one for its duration), and the number on the right is the day's total. Durations are written to disk daily and kept for 30 days. Days with no record get no zero row: that usually means the app was not running, not that the lamp was on doing nothing.",
    historyToday: "Today",
    historyEmpty: "No history yet — it fills in after a day of use.",
    tabLook: "Look",
    tabBehavior: "Behavior",
    tabGroups: "Groups",
    tabAlert: "Alerts",
    tabConnect: "Connect",
    tabMachines: "Machines",
    tabCallMe: "Call Me",
    tabHooks: "Forwarding",
    tabGuide: "Guide",
    tabSystem: "System",
    tabDemo: "Demo",
    connectTabAgents: "Agents",
    connectTabSources: "Other sources",
    connectTabNetwork: "Network & diagnostics",
    systemTabGeneral: "General",
    systemTabUpdate: "Update & diagnostics",
    systemTabData: "Data",
    pageHint: {
      status: "What's happening right now: running sessions and recent events.",
      history: "The past few days: today's recap, insights, durations and approvals.",
      messages: "Notes from outside agents via your Call Me links. Expanding marks them read.",
      look: "What the lamp looks like: size, form, colour, and the texture of the sign.",
      behavior: "How the lamp behaves: quiet timeout, calm mode and clicks.",
      groups: "How sessions map to lamps.",
      alert: "When to notify you through the system.",
      hooks: "Where to forward state changes: webhooks, ntfy, physical lamps.",
      connect: "Hook agents and signal sources up to SignalHub.",
      machines: "Link other machines' hubs and see them all on one lamp.",
      callme: "Manage Call Me links and the receiving server.",
      guide: "The handbook: getting started and the API cheat sheet.",
      demo: "See it work without wiring up a real agent.",
      system: "This app on this machine: language, updates and data.",
    },

    railSearch: "Find a setting…",
    railSearchEmpty: "No matching page",
    lampFirstRun: "New here?",
    lampFirstRunDemo: "See a demo",
    lampFirstRunDemoHint:
      "See the lamp in action without connecting any agent (five scenes, 10–20s each)",

    demoTitle: "SignalHub in five scenes",
    demoIntro: "Four looping scenes — jump to the one you care about, no agent needed.",
    demoBackToSettings: "Back to settings",
    demoNotReal: "Demo only · no real state is changed",
    demoGroup: { build: "Build", api: "API", review: "Review" },
    demoScenario: {
      day: "A day in the life",
      asking: "When it wants you",
      silence: "When it goes quiet",
      tucked: "Tucked away",
      look: "Speaking in light",
    },
    demoStep: {
      idle: {
        title: "Once connected, the lamp is quiet",
        body: "A steady green light means idle. Quiet mode dims it so it stays out of your way until something happens.",
      },
      thinking: {
        title: "You ask a question",
        body: "Thinking → fast green blink. You know it started without switching to the terminal.",
      },
      working: {
        title: "Several agents working at once",
        body: "One card per group, most urgent first. Each card shows the group, what it is doing, and for how long.",
      },
      permission: {
        title: "One group is waiting on you",
        body: "Needs permission = red, rounded square, smooth blink. Red outranks amber and green: two working cards cannot push it down.",
      },
      approved: {
        title: "Allow it, and the red clears itself",
        body: "That group goes back to work and the aggregate returns to green — nothing to clear by hand.",
      },
      done: {
        title: "Done: one flash",
        body: "Completion is a brief green flash; the card is removed after 3 minutes by default, and that timeout is configurable.",
      },
      empty: {
        title: "The card is gone, the lamp remembers",
        body: "The empty state says “Just finished · 1 min ago” instead of “No agents reporting” — exactly when you want to know.",
      },
      attention: {
        title: "Amber: worth a glance, not an action",
        body: "Needs attention = amber, a one-second blink. It is not stuck and it is not waiting for an approval — it just has something you should know, without needing you to stop what you are doing.",
      },
      both: {
        title: "Red on top, amber in the middle",
        body: "A group's urgency is its position in the stack: red > amber > green. You never have to pick which one to read first — top to bottom is the order to handle them.",
      },
      error: {
        title: "Also red, different rhythm",
        body: "An error blinks hard and fast; a pending approval blinks smoothly. The approval waits for you to allow it, the error waits for you to read the log. After a while with no new messages both turn grey: five minutes for an error, ten for an approval.",
      },
      stale: {
        title: "Half an hour without news and it turns grey",
        body: "Grey is not idle — idle means it is there and not working; silent means we no longer know whether it is there at all. Left alone for another half hour, the card is dropped for good.",
      },
      staleRing: {
        title: "Switch to a ring and the arc is the time left",
        body: "Set the dot shape to “ring” under Appearance and the arc becomes how long is left before the group is removed, shrinking until it disappears. The default solid dot has nowhere to draw that progress — it is an optional shape.",
      },
      offline: {
        title: "The ring runs out and the card is gone",
        body: "Now it says “No agents reporting”, which is a different sentence from “Just finished”. When something really needs you, it comes back.",
      },
      quiet: {
        title: "Quiet mode: almost invisible when idle",
        body: "Idle, silent or not reporting, the whole stack fades to a faint dot; the moment something happens it comes back to full weight. It keeps the lamp out of your way without turning notifications off.",
      },
      collapsed: {
        title: "Collapsed to a single light",
        body: "A click (Settings → Behavior can make that click open the inbox or do nothing) folds the stack into one light. Three groups become one dot: if any group wants you, that dot is red.",
      },
      collapsedDone: {
        title: "Collapsed still lights up",
        body: "On completion the light flashes once and settles. Collapsing only saves desktop space — it does not mute anything, which is why many people keep it this way.",
      },
      edgeLine: {
        title: "Light the top, leave the bottom dark",
        body: "One state-coloured line along the top edge; the bottom stays off. The two edges are independent slots now, no longer one rail switched together. Weight, brightness and colour are shared under Appearance → Frame light — not one extra word on the sign.",
      },
      edgeBoth: {
        title: "Two edges, two dropdowns",
        body: "Here both are solid lines, but they are two separate rows: to keep the top always on and light the bottom only on an error, you just pick once in each. The four looks also include glow and a line that fades at both ends.",
      },
      edgeGlow: {
        title: "Glow — the loudest setting",
        body: "Both edges switch to glow, spreading a halo outside the card. Use it when the lamp sits far away, across multiple displays, or when it has to catch your peripheral vision. It is deliberately softer on light wallpapers.",
      },
      cornersState: {
        title: "Four corners, four yes/no questions",
        body: "The four little lights sit on the corners of the outline and follow its shape, so they take no space from the text. Each answers: has a state / pinned / muted / about to go stale. All four are set to “state colour” here so you can see where they sit — and in Appearance they are four separate dropdowns.",
      },
      cornersMeaning: {
        title: "If the condition does not hold, nothing is drawn",
        body: "This card satisfies all four — it has a state, it is pinned, it is muted, and it is going quiet — so all four lights show. Let any one of those fail and that light disappears entirely rather than becoming a dim dot: a dim dot would suggest “there is something here, it is just not lit”, which is exactly why nobody would ever look for the setting. Each corner also has its own trigger under Behavior.",
      },
      lightTrigger: {
        title: "When it lights up is a separate question",
        body: "Two signs side by side with the top edge set to “when you're needed”: the one waiting for approval lights up, the one still working does not. Appearance decides what it looks like lit; Behavior decides why it is lit now — all six slots have their own pick from the same nine-condition list.",
      },
      cornerPassthrough: {
        title: "Even “you can't click it” fits in a corner",
        body: "The hollow square at the top left is click-through: it appears while enabled and vanishes entirely when off. The old always-on ban badge in the window corner is gone — whether to show this state, and in which corner, is now one of the six rows under Appearance → Frame light. The state is still in the tooltip and tray menu.",
      },
      lightTriggerState: {
        title: "Nine conditions, six independent rows",
        body: "The top edge is “in a chosen state → error”, the bottom edge is always on, and so are the four corners: the error card lights both edges, the working one keeps only the bottom line, and four state corners show on both. Besides a chosen state you can trigger on a time window, silence nearly over, a message being shown, or a pinned group — the six rows share one condition list but store their own pick.",
      },
      lookSkin: {
        title: "Or swap the whole look at once",
        body: "This step is the Neon skin: palette, card tint, radius and border, dot shape, card outline, motion, and the look and trigger of all six frame-light slots change together. There are five skins under Appearance → Skin; the four look presets below only touch size, opacity and which fields show.",
      },
    },
    demoStepCount: (index, total) => `${index} / ${total}`,
    demoPrev: "Prev",
    demoNext: "Next",
    demoPlay: "Play",
    demoPause: "Pause",
    demoReplay: "Replay",
    demoCta: "Watch the demo",
    demoCtaHint: "No agent connected yet? See how the lamp behaves first — nothing to configure.",

    guideEntry: "New here?",
    guideWhatTitle: "What this is",
    guideWhat:
      "SignalHub is a status light that sits on your desktop. Anything that can send one HTTP request can light it up — an agent, a build script, a service you wrote yourself. Green means working, red means it needs you, amber means it is waiting on your approval; the card says which group, what step it is on, and how long it has been there.",
    guideStepsTitle: "Three steps to hook it up",
    guideStep1:
      "Get a reporter talking: connect an agent with one click on the Connect tab, or have your program POST a state to the API below.",
    guideStep2:
      "Watch the light: it opens as a stack of cards by default, and a click folds it into a single lamp. Its position and shape are remembered.",
    guideStep3:
      "Do not miss it: a system notification fires when something needs you. The Alerts tab holds quiet hours, the alert threshold, and outbound forwarding that can be limited to specific states.",
    guideFaqTitle: "When it is not working",
    guideFaqNoLightQ: "The light stays grey?",
    guideFaqNoLightA:
      "Grey means no session is reporting, or Pause monitoring is on. Check what the heading at the top says, then confirm the reporter is connected on the Connect tab.",
    guideFaqNoNotifyQ: "No notification, and no sound?",
    guideFaqNoNotifyA:
      "Check the state at the top of the Alerts tab: Denied means you need to turn SignalHub on under Notifications in System Settings; on Windows portable, Not registered means the Start-menu shortcut Windows toasts require could not be created — press Retry there. If it says Granted and nothing appears, Focus or the per-app notification switch may be blocking it: the app can only confirm it handed the notification to the OS, not that the OS displayed it.",

    notifyPermission: "System notification permission",
    permGranted: "Granted",
    permDenied: "Denied",
    permPrompt: "Not asked yet",
    permUnknown: "Unreadable",
    permUnavailable: "Not registered",
    permRequest: "Request",
    permRetry: "Retry",
    permHint:
      "Without permission a notification is dropped silently — no popup, no sound, no error. If it says Denied, you need to turn SignalHub on by hand under Notifications in System Settings.",
    permUnavailableHint:
      "Windows toasts require a Start-menu shortcut carrying the app identity. The installer creates one; on the portable build the app creates one on first launch. When that fails Windows drops every toast — press Retry once, or use the -setup.exe installer instead.",
    permBackendError: (detail) => `Registration error: ${detail}`,
    notifyTest: "Send a test notification",
    notifyTestTitle: "SignalHub test notification",
    notifyTestBody: "If you can see this, notifications and the alert sound both work.",
    notifyTestResult: (perm) =>
      `Handed a test notification to the OS (permission: ${perm}); the app cannot tell whether the OS actually displayed it. If you did not see it, check SignalHub under Notifications in System Settings, whether Focus is on, and on Windows portable make sure the state above is Granted rather than Not registered.`,

    healthTitle: "System check",
    healthHint:
      "One pass over notification permission, installed agents, report receipts and shortcuts. It reads data that already exists on this machine and changes no settings. Copy the report into an issue before filing.",
    healthRun: "Run check",
    healthRerun: "Run again",
    healthSummary: (fail, warn) =>
      fail > 0
        ? `${fail} need attention${warn > 0 ? `, plus ${warn} worth a look` : ""}`
        : warn > 0
          ? `${warn} worth a look, nothing blocking`
          : "Nothing needs attention",
    healthVerdict: (level) =>
      level === "fail" ? "Needs attention" : level === "warn" ? "Worth a look" : "All clear",
    healthLevel: (level) =>
      level === "fail" ? "FAIL" : level === "warn" ? "WARN" : level === "ok" ? "OK" : "INFO",
    healthLine: enHealthLine,
    healthReportHeader: (at, version) => `SignalHub system check · ${at} · v${version}`,

    digestTitle: "Today at a glance",
    digestHint:
      "Aggregate state is timed one state at a time, so the pieces add up; idle and offline are included. Approvals come from today's entries within the most recent 100 records. It measures duration, not individual sessions — see \"This week in focus\" below for sessions that came and went.",
    digestFact: enDigestFact,
    digestReportHeader: (day) => `SignalHub daily digest · ${day}`,

    insightTitle: "This week in focus",
    insightHint:
      "Counts sessions and approvals from the last 7 days of local event and approval logs (up to 5000 entries each; the newest win). Only sums and maxima — nothing leaves this machine and nothing is guessed.",
    insightFact: enInsightFact,
    insightReportHeader: (days) => `SignalHub weekly insights · last ${days} days`,
    insightError: "The local event log could not be read; this card is hidden for now.",
    insightTruncated:
      "The entry cap was reached: only the newest records are counted, so the numbers above may be low.",

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
    displayName: "Display name",
    displayNamePlaceholder: (raw) => `Empty uses ${raw}`,
    pin: "Pin",
    pinHint: "Pinned groups always come first on the lamp; the rest stay in urgency order",
    feedTitle: "Recent messages",
    feedEmpty: "Nothing reported yet.",

    presets: "Presets",
    form: "Form",
    expanded: "Expanded",
    singleLight: "Single light",
    lampLayout: "Lamps",
    layoutSingle: "One lamp",
    layoutPerGroup: "One per group",
    layoutHint:
      "The row above decides which groups exist; this one decides how many lamps they sit on. \"One per group\" gives each group its own draggable lamp and leaves the main lamp showing global state; new lamps appear along the right edge and each remembers where you drag it. More than 8 groups falls back to a single lamp.",
    size: "Size",
    shape: "Shape",
    palette: "Palette",
    signOpacity: "Sign opacity",
    signOpacityHint:
      "65% is the floor: the text sits on that dark plate, so any thinner and it lands on your desktop where pale windows swallow it. For a less obtrusive lamp, use Quiet mode instead.",
    signWidth: "Sign width",
    signHeight: "Sign height",
    blinkSpeed: "Blink speed",
    onSign: "On the sign",
    quietMode: "Quiet mode",
    snapToEdge: "Snap to edges",
    snapToEdgeHint:
      "Snap flush to the left or right screen edge when released nearby (top and bottom are left alone — the taskbar and Dock live there).",
    lookHint:
      "Shape affects only the light: rounded or ring drops the square-corner cue for permission / error, so pair it with the colourblind palette.",
    resetToDefault: "Reset to default",
    previewTitle: "Preview",
    previewHint:
      "Change anything above and this follows immediately — it is the only place you can see the effect while the lamp is hidden, collapsed, or on another screen. This draws the active look; quiet mode dimming only happens on the real lamp.",
    previewGroup: "orders",
    previewMessage: "running tests",
    sizeSmall: "Small",
    sizeMedium: "Medium",
    sizeLarge: "Large",
    shapeCircle: "Circle",
    shapeRounded: "Rounded",
    shapeRing: "Ring",
    paletteDefault: "Default",
    paletteColorblind: "Colourblind",
    paletteVivid: "Vivid",
    lookTabBasic: "Basics",
    lookTabTexture: "Texture",
    textureHint:
      "These change how the sign is drawn, not how big it is — the window is sized from the dot size and the multipliers, so nothing here can clip.",
    signRadius: "Corners",
    radiusSquare: "Square",
    radiusSoft: "Soft",
    radiusStandard: "Standard",
    radiusRound: "Round",
    signBorder: "Border",
    borderNone: "None",
    borderHairline: "Hairline",
    borderStrong: "Strong",
    signTint: "Surface",
    tintGraphite: "Graphite",
    tintBlack: "Black",
    tintWarm: "Warm",
    tintSlate: "Slate",
    signTextSize: "Text size",
    textCompact: "Compact",
    textStandard: "Standard",
    textLarge: "Large",
    signFont: "Typeface",
    signFontHint:
      'Pick "Mono" for a terminal face across the whole sign — group, state and message alike.',
    fontSystem: "System",
    fontMono: "Mono",
    signGap: "Spacing",
    signGapHint:
      "Signs stack vertically; this only sets the gap between them. It changes the window height, so there are just three steps.",
    gapTight: "Tight",
    gapStandard: "Standard",
    gapLoose: "Loose",
    edgeOff: "Off",
    edgeGlow: "Glow",
    cornerPos: { tl: "Top left", tr: "Top right", br: "Bottom right", bl: "Bottom left" },
    cornerKind: {
      empty: "None",
      state: "State colour",
      pinned: "Pinned",
      muted: "Muted",
      stale: "Silence left",
      passthrough: "Click-through",
    },
    frameLight: "Frame light",
    frameLightHint:
      "The top and bottom edges plus the four corners are six independent slots, sharing one width, one brightness and one colour. Each slot has its own nine-condition trigger under Behavior; click-through is one of the corner options. The two preview signs above are always drawn lit — they are a static mock-up and deliberately ignore the triggers.",
    lightScale: "Weight",
    lightScaleHint:
      "The rail's line width and the corner lights' size move together. Set separately they drift into a thick line with four tiny dots — and they are the same rail.",
    scaleThin: "Thin",
    scaleNormal: "Normal",
    scaleThick: "Thick",
    lightAlpha: "Brightness",
    alphaSoft: "Soft",
    alphaNormal: "Normal",
    alphaStrong: "Strong",
    frameColor: "Colour",
    frameColorHint:
      '"State" follows each sign\'s own state; "Neutral" lets the outline carry only the shape and leaves the colour to the dot.',
    frameColorState: "State",
    frameColorNeutral: "Neutral",
    lightBehavior: "When the frame light comes on",
    lightBehaviorHint:
      "Appearance sets what it looks like when lit; these six rows set why each slot is lit now. All six draw from the same nine conditions: the top edge can stay on while the bottom lights only on errors, and the four corners each decide for themselves. Decisions are per sign, so some cards on the lamp can be lit while others stay dark.",
    triggerAlways: "Always",
    triggerAttention: "When you're needed",
    triggerActive: "While active",
    triggerQuiet: "Follow quiet mode",
    triggerState: "In a chosen state",
    triggerSchedule: "During chosen hours",
    triggerStale: "Silence nearly over",
    triggerMessage: "When a message is shown",
    triggerPinned: "Only for pinned groups",
    triggerStatePick: "Which state",
    triggerScheduleFrom: "From",
    triggerScheduleTo: "To",
    triggerParamsHint:
      "Hours use local time; a start later than the end means overnight (22:00–06:00), and equal times mean no time filter. These parameters are shared by all six slots — six copies would turn this page into a spreadsheet, and two 'chosen state' rows with only one picker makes no sense either.",
    signLabelTitle: "Session title",
    signLabelHint:
      "With session grouping the sign has a fixed width and shares its first line with the status word, so the agent in 'project · agent' was almost always cut off by the ellipsis. The default now shows only the session/project name; you can bring the agent back or write your own template. Per-group display names still win.",
    signLabel: "What the title says",
    labelSession: "Session name only",
    labelWithAgent: "Session · agent",
    labelCustom: "Custom template",
    labelTemplate: "Template",
    labelTemplateHint:
      "Tokens: {session} (project), {agent} (agent), {group} (raw group name). Empty tokens are cleaned up, so no orphan separator is left behind. Forwarded summaries and the web panel still use the full name — they aren't width-constrained.",
    labelTemplatePlaceholder: "{session} · {agent}",
    skinTitle: "Skin",
    skinHint:
      "Swap a whole look at once: palette, card tint, radius and border, dot shape, outline, motion, and all six frame-light slots. **Changing skins overwrites all six trigger rows too** — for just size, opacity or which fields show, use the four look presets below.",
    skinPlain: "Plain",
    skinPlainHint: "Back to the original: default palette, round dots, breathing halo, frame light off.",
    skinFocus: "Focus",
    skinFocusHint:
      "Quiet by default — a thin top edge only when you're needed, pinned / muted on the top corners, no motion.",
    skinNeon: "Neon",
    skinNeonHint:
      "Vivid palette, black card, capsule outline, glowing top and bottom edges, state dots in all four corners, pulsing dots.",
    skinPaper: "Paper",
    skinPaperHint:
      "Warm black card, strong border, large radius and an arched outline, square dots, a gentle fade in and out.",
    skinSignal: "Signal",
    skinSignalHint:
      "Colour-blind-safe palette, diamond dots, diagonal-round outline and a sweep of light; top edge only with a message, bottom edge only when you're needed.",
    shapeSquare: "Square",
    shapeDiamond: "Diamond",
    shapeCapsule: "Capsule",
    outlineTitle: "Card outline",
    outlineHint:
      "Change the overall silhouette beyond corner radius. All four are drawn with border-radius only, so corner lights are never clipped; their anchor follows the outline.",
    outlineRect: "Rectangle",
    outlineCapsule: "Capsule",
    outlineLeaf: "Diagonal round",
    outlineArch: "Arch",
    effect: "Effect",
    effectHint:
      "There used to be a single 'Lively motion' switch; now pick one of five. 'Breathing halo' is exactly what that switch used to enable. All five fall back to static when the system asks for reduced motion.",
    effectOff: "Off",
    effectBreathe: "Breathing halo",
    effectPulse: "Pulse",
    effectSweep: "Sweep",
    effectFade: "Fade",
    edgeLine: "Solid line",
    edgeFade: "Fade at ends",
    slotTop: "Top edge",
    slotBottom: "Bottom edge",
    slotKind: "Look",
    slotTrigger: "Trigger",
    slotBlink: "Blink",
    slotBlinkHint:
      "Blinks on this card's state rhythm — the same rhythm its dot uses. When the light draws nothing there is nothing to blink, so the switch greys out.",
    slotsHint:
      "Each of the six slots picks its own look and its own trigger: the top edge can stay on while the bottom lights up only on errors, and the four corners can each hang and judge something different. Judgement is per sign, so one lamp can have lit and unlit cards at once.",
    signEmphasis: "Emphasis",
    emphasisAuto: "Tiered",
    emphasisFlat: "Uniform",
    emphasisHint:
      'With "Tiered", the signs that need you are solider and their dots larger while the rest step back, so it is obvious what to look at first. If that difference feels arbitrary, pick "Uniform" — every sign carries the same weight.',
    fieldDot: "Dot",
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
    stayDuration: "Expiry",
    stayDurationHint:
      "How long this state survives silence. There is no “never expire” — a light that never goes out is an alert nobody can withdraw. A change applies from the next report; it is not retroactive.",
    silenceTimeouts: "Silence timeouts",
    silenceTimeoutsHint:
      "How long a sign stays on the lamp after the agent goes quiet. Raise it and a long task is less likely to be mistaken for silence; the cost is that a genuinely crashed agent is noticed later. Changes apply from the next report.",
    silenceTimeoutsMore:
      "The other states (idle / attention / permission / error) are set individually under Look → Per-state overrides.",
    ttlDone: "Keep after finishing",
    ttlWorking: "Silence while working",
    ttlThinking: "Silence while thinking",
    behaviorWindowTitle: "Window & interaction",
    clickAction: "Click on the lamp",
    clickActionHint: "Give the cheapest gesture to the thing you do most. The long-press radial menu, dragging and right-click menus are unchanged; the default is still collapse / expand, so upgrading does not change any existing behaviour.",
    clickActionLabels: { collapse: "Collapse / expand", inbox: "Open inbox", none: "Do nothing" },
    resetToGlobal: "Reset to follow global",
    styleHint:
      '"Follow" means shape and blink speed come from the Look tab, and the expiry is the built-in default (shown on each row). Expiry is how long a card stays on the lamp after its agent goes quiet — it takes effect on the *next* report, not retroactively.',
    perStateStyle: "Per-state overrides",
    perStateSome: (count) => `${count} state${count === 1 ? "" : "s"} customized`,
    perStateNone: "not customized",
    groupDisplayTitle: "Per-group display",
    groupDisplayEmpty: "No groups yet — they show up here once an agent or the report API tells us about one.",

    notifyWhenNeeded: "Notify when needed",
    notifyWhen: "Notify for these states",
    notifyWhenHint:
      "Only notify when the overall state becomes one of these. None selected means never — use the switch above to turn notifications off entirely. Same filter as in Forwarding; the difference is only the default (that one filters nothing, this one starts on the three that need you).",
    soundWithNotify: "Sound with notification",
    alertAfter: "Alert after",
    alertAfterHint: "A state must last this long before notifying; brief ones are ignored",
    quietHours: "Quiet hours",
    alertHint:
      "A system notification that says which group it is and what state it is stuck in. Sound rides on the notification, so turning notifications off silences it too. During quiet hours nothing pops or sounds and the lamp still shows — and nothing is replayed: waking to a pile of last night's notifications is exactly what quiet hours avoids.",
    rulesTitle: "Presets",
    rulesHint:
      "Three hard-coded rules, each with its own switch — no expression engine. They only decide when to interrupt you; aggregation, ordering and forwarding are untouched.",
    ruleStuckPush: "Approval stuck 60s → push again",
    ruleStuckPushHint:
      "When a group has been waiting for approval for over 60 seconds, send one more message through the phone-push hooks configured in Forwarding (Bark / PushDeer / ntfy…). No hook, nothing to send. Approvals that pass in a second are not worth the noise.",
    ruleBusySilence: "An agent is working → hold notifications",
    ruleBusySilenceHint:
      "While any agent is thinking or working, system notifications and reminders (including approval ones) are held. The lamp and reporting keep working; nothing is replayed when the work finishes.",
    ruleMeeting: "In a meeting → blue lamp + silence",
    ruleMeetingHint:
      "Works with the \"In a meeting\" collector: while a call is on, that sign turns blue and notifications are held; nothing is replayed after it ends. Blue only means \"in a meeting\" — ordering and the tray are unchanged.",
    delayNow: "Now",
    alertForwardHint:
      "Want state changes forwarded to your server, or a command run? See the Forwarding tab.",

    pauseMonitoring: "Pause monitoring",
    openConfigDir: "Config folder",
    openLogs: "Logs",
    pauseHint: "Pausing blanks the lamp, but sessions are still recorded — resuming returns to the real state",
    outboundHooks: "Forwarding",
    configure: "Configure",
    connectHookHint:
      "Forward state changes to your server, or run a local command — set it up on the Forwarding tab.",
    groupBy: "Group signs by",
    groupByNone: "Nothing",
    groupByCwd: "Directory",
    groupBySession: "Session",
    groupByHint:
      "This decides which groups exist; the row below decides how many lamps those groups are laid out on — two different things. \"Nothing\": every report without a group lands on the same sign. \"Directory\": one sign per working directory — each project gets its own row. \"Session\": one sign per session — when a single agent has several windows or worktrees open their states differ, and only per-session grouping shows that. All three are just a fallback: a report that carries its own group still wins, and the change takes effect from the next report on. Directory grouping shows that path; session grouping does not put the session id on the sign — it shows \"last path segment · agent\" by default (project first, because the name gets truncated on the sign and when one agent has several windows open only the project tells them apart). To rename them, use Per-group display below.",
    port: "Port",
    apiTitle: "API quick reference",
    apiToken: "Token",
    apiAuthHint:
      "The credential for the local API — keep it to yourself. Requests need Authorization: Bearer <token> and must not carry an Origin header (local CSRF guard). Both the port and the token live in two files under the config folder.",
    apiEndpoints: "Endpoints",
    apiEndpointReport: "Report a state",
    apiEndpointState: "Current aggregate + groups",
    apiEndpointSessions: "Session list",
    apiEndpointClear: "Remove a session",
    apiEndpointHealth: "Health check (no auth)",
    apiStates: "Reportable states",
    apiStatesHint:
      "A misspelled state is rejected outright (400), never silently ignored — that is the hardest kind of “the lamp did nothing” to diagnose. stale / offline are derived by aggregation and cannot be reported.",
    apiExamples: "Examples",
    copy: "Copy",
    copied: "Copied",
    openDocs: "Full reference (GitHub)",
    cliExample: `# or use the CLI:
signalhub working --session build-1 --agent ci --message "compiling"
signalhub status
signalhub watch --until done --timeout 10m   # wait for a state in CI`,

    agentsTitle: "Connect an agent",
    agentsHint:
      "SignalHub writes hooks into the agent's own config file. Restart the agent and it starts reporting. It only touches the hooks key, backs the file up first, and can be undone any time.",
    agentInstalled: "Installed",
    agentDetected: "Detected, not installed",
    agentMissing: "Not detected",
    agentCustom: "Custom",
    customAgentsTitle: "Custom agents",
    customAgentsHint: (families) =>
      `An agent that is not on the list does not have to wait for us: add an entry to the config file saying which family it mirrors (${families}) and where its config lives, and it shows up in the list above — install and uninstall work exactly the same way. Save the file and come back to this window; it is re-read on focus.`,
    customAgentsLimit:
      "This only covers agents that are format-compatible with a different path. If the event names differ, we still have to add one.",
    customAgentsOpen: "Open the config file",
    customAgentAddToggle: "Add from this panel",
    customAgentAddTitle: "Add a preset",
    customAgentAddHint:
      "No need to edit the file by hand: this goes through the app's write command, which validates the entry, keeps existing ones and refuses to overwrite broken JSON. Do not guess the family — a wrong one installs a hook that looks fine but never reports anything.",
    customAgentFieldId: "id (the reported name)",
    customAgentFieldName: "Display name",
    customAgentFieldFamily: "Compatible family",
    customAgentFieldConfig: "Hook config path",
    customAgentFieldMarker: "Install marker directory (optional)",
    customAgentFieldCommand: "Executable name (optional)",
    customAgentReplace: "Replace an entry with the same id",
    customAgentSubmit: "Write and validate",
    customAgentCancel: "Close",
    customAgentProblems: {
      idRequired: "An id is required.",
      nameRequired: "A display name is required.",
      familyRequired: "Pick a compatible family.",
      configRequired: "A hook config path is required.",
      configParent: "The path cannot contain .. — point at your own config file.",
      commandSpaces: "The executable name cannot contain arguments or spaces.",
    },
    customAgentOutcome: (outcome, id) =>
      outcome === "added"
        ? `Added ${id}; the list has been refreshed.`
        : outcome === "unchanged"
          ? `${id} already exists with the same content; the file was left untouched.`
          : `Replaced ${id}; the list has been refreshed.`,
    setupPromptTitle: "Let the agent set itself up",
    setupPromptHint:
      "Not sure whether your agent is on the list? Paste the whole prompt into it and let it pick the right route, write the config and install the hooks. It carries this machine's current adapter list, paths and port, so there is nothing to look up first.",
    setupPromptCopy: "Copy setup prompt",
    setupPromptGuide:
      "Your agent is not on the list? Copy a prompt into it and let it figure out the setup — no docs to read first.",
    agentCommandBroken:
      "Installed, but the command in the config no longer runs — the agent will not complain and the lamp will not light. Usually the app was moved or deleted; clicking Install again rewrites it to the current path:",
    agentUpdateAvailable:
      "Installed with a command written by an older version — it still runs, but behaves like that version (typically: the agent's process id is not passed along, so the lamp falls back to stale instead of idle). Click Reconnect to rewrite it to the current version's command:",
    agentReinstall: "Reconnect",
    agentReceiptAgo: (age, state) => `Last report ${age} ago · ${state}`,
    agentReceiptCount: (n) => `${n} this run`,
    agentReceiptNever:
      "No report received from it yet — use it once; if the lamp stays dark, check Recent hook calls below for a line.",
    systemSourcesTitle: "System sources",
    systemSourcesHint:
      "Sources that declared source: system when reporting — the built-in collectors (in a meeting, backing up, …) will land here, and so can your own scripts that watch the machine.",
    systemSourcesEmpty:
      "None yet. Try one: signalhub working --source system --agent meeting",
    collectBackup: "Time Machine backups",
    collectBackupHint: (secs) =>
      `Checks Time Machine every ${secs} seconds; while a backup is under way the lamp gains a "Backup" group, and it reports "Backup finished" once it is done. Read-only: it watches system state and reads no file contents. If it ever breaks on its own, the worst that happens is that group disappearing.`,
    collectMeeting: "In a meeting (mic / camera in use)",
    collectMeetingHint: (secs) =>
      `Checks every ${secs} seconds whether another app is using the microphone or camera; while a call is on the lamp gains a "Meeting" group, and it reports "Meeting ended" afterwards. It only reads whether a device is in use — **it never captures or stores any audio or video**, and asking does not trigger a permission prompt.`,
    collectUnknown: (id) => `Collector "${id}"`,
    otherSourcesTitle: "Custom",
    otherSourcesHint:
      "Sources we have received reports from that are not in the list above (your own scripts, commands wrapped by signalhub run, another machine, a hand-written curl…). A line here means the reporting path works.",
    agentTry: "Self-check",
    agentTryBusy: "Trying…",
    agentTryRanOk: (command) => `Ran the command from your config (${command}) — it works.`,
    agentTryRanFailed: (command, code, output) =>
      `Running the command from your config (${command}) failed${code ? ` (exit ${code})` : ""}${output ? `: ${output}` : "."}`,
    agentTryHttpOk: (url) =>
      `Sent a real HTTP request to ${url} — the endpoint answered. This path works (it cannot prove the agent will call us; the receipt above answers that).`,
    agentTryHttpFailed: (url, output) =>
      `Sending HTTP to ${url} failed${output ? `: ${output}` : "."}`,
    agentTryPlugin: (states) =>
      `This integration is a plugin — there is nothing we can run on its own. What you just saw was the app injecting ${states} itself, which proves the lamp and the state machine work; whether it calls us at all is what the receipt above answers.`,
    diagnoseCopy: "Copy diagnostics",
    diagnoseHint:
      "A plain-text block: version, report port, pending reports, each source's config path and whether its command still runs, last reports received, recent hook log. When something will not connect, copy this first — everything worth looking at is in it, and you do not have to hunt across three tabs.",
    diagLogTitle: "Recent hook calls",
    diagLogHint:
      "Every hook invocation is logged here, including failures. So when something is wired up but the lamp stays dark, look here first: no lines at all means the events never left the agent (usually that command does not run — see the red warning on the Connect page); lines that all say failed mean they did leave, but never arrived.",
    diagLogEmpty: "Nothing yet — no hook has run so far.",
    diagRefresh: "Refresh",
    agentConnect: "Connect",
    agentDisconnect: "Disconnect",
    agentConnectHttp: "Connect over HTTP (no command needed)",
    agentSwitchHttp: "Switch to HTTP",
    agentSwitchCommand: "Switch to command",
    agentModeHttp: "Mode: HTTP. No command has to exist, so \"the command is not on PATH\" cannot happen. The trade-off: this endpoint is switched off while \"Allow LAN access\" is on.",
    approveTools: "Approve from the lamp",
    approveToolsHint:
      "When Claude Code wants to ask you to approve a tool call, the confirmation box moves onto the lamp: a bubble appears, and you click Allow or Deny. This is the only switch that changes how the agent behaves — while it is on, the agent **waits for you** (up to 45 seconds); if nobody answers, the question goes back to the terminal. The full command appears only in this local window; what gets reported is still just the short phrase.",
    approveScopeHint:
      "Approve from the lamp currently supports Claude Code only, and only with the HTTP install mode — that is why the switch appears on that card alone. For other agents, including WorkBuddy, a permission request still turns the lamp red and still fires a notification, but the confirmation stays in the agent's own window; go there to allow or deny.",
    approvalAllow: "Allow",
    approvalDeny: "Deny",
    approvalTooLong: "Too long to show here — approve it in the terminal (or deny it here)",
    approvalMore: (n) => `${n} more waiting`,
    approvalWaiting: (secs) => (secs < 5 ? "just now" : `waiting ${secs}s`),
    approvalDanger: {
      delete: "This deletes files",
      "force-push": "This rewrites remote history",
      rewrite: "This discards local changes",
      sql: "This drops tables / databases",
      "pipe-shell": "This downloads and runs a script",
      disk: "This writes to a disk directly",
      privilege: "This escalates privileges",
      permissions: "This opens up permissions",
    },
    approvalHold: "Hold 0.8s to confirm",
    approvalReason: "Add a reason",
    approvalReasonUnsafe: "Too dangerous",
    approvalReasonSafer: "Do it more safely",
    approvalReasonExplain: "Explain first",
    approvalReasonCustom: "Custom",
    approvalReasonCustomPlaceholder: "What should the agent do instead?",
    approvalLogTitle: "Approval log",
    approvalLogHint:
      "Last 100: what was allowed or denied, when, and from where. Details are clipped to 200 characters.",
    approvalLogEmpty: "No approvals yet",
    approvalLogDecision: (decision) =>
      ({ allow: "allowed", deny: "denied", expired: "timed out" } as Record<
        string,
        string
      >)[decision] ?? decision,
    approvalLogSource: (source) =>
      ({ lamp: "lamp", web: "phone", api: "API" } as Record<string, string>)[source] ??
      source,
    agentModeCommand: "Mode: command",
    cliAgentsTitle: "Other tools",
    cliAgentsHint:
      "Two ways; pick by what the tool gives you. Wrap a command: it does not matter what the tool is, as long as it can run a command — it reports Working while it runs and Done / Error by exit code when it finishes, and the tool needs no hook support at all. Call it from its own event hooks: finer-grained when the tool has them (it can tell Needs permission from Error apart) — use this for anything with hook config that is not in the list above. For VS Code: anything running in the integrated terminal (tasks included) uses the first way; extensions that live only in a side panel (Cline, Roo Code, Continue and the like) cannot be hooked at all — they offer neither a process to wrap nor a config file to write.",
    cliAgentsWrap: "Wrap a command",
    cliAgentsHookName: "Call it from its own event hooks",
    tryItTitle: "Try it",
    tryItHint:
      "Click one and the state appears on the lamp immediately. It takes exactly the same path as an HTTP report (aggregation, window sizing, today's stats, pushing to the lamp), so it answers \"is the lamp working at all\" — but it does not exercise the HTTP layer (whether the server is up, whether the token is right), and for that you want the curl on the Guide tab. It shows up as the test group and clears itself after 20 seconds, leaving nothing behind.",
    hookResult: ({ added, updated, already, removed }) => {
      const parts: string[] = [];
      if (added > 0) parts.push(`${plural(added, "event", "events")} added`);
      if (updated > 0) parts.push(`${plural(updated, "event", "events")} updated`);
      if (removed > 0) parts.push(`${plural(removed, "handler", "handlers")} removed`);
      if (parts.length === 0) {
        return already > 0
          ? `Nothing to change (${plural(already, "event", "events")} already up to date)`
          : "Nothing to change";
      }
      return parts.join(", ");
    },
    hookPluginWritten: (path) => `Plugin written: ${path}`,
    hookPluginRemoved: (path) => `Plugin removed: ${path}`,

    noHooks: "No hooks yet. Add one to POST to a URL or run a command on state changes.",
    hookN: (n) => `Hook ${n}`,
    noTarget: "No target",
    devicesTitle: "Physical lights (WLED)",
    noDevices: "No lights yet. Fill in a WLED address and the state colour goes straight to it.",
    deviceN: (n) => `Light ${n}`,
    addDevice: "+ Add a light",
    devicePlaceholder: "192.168.1.50 or wled.local:8080",
    devicesHint:
      "Just the host; with or without http://. Only the colour is sent: no group names, no session names, no message text. Saving lights it up once with the current state — that is the answer to \u201cdoes it work?\u201d, and the result lands in the same log as the hooks above.",
    type: "Type",
    command: "Command",
    addHook: "+ Add hook",
    hooksHint:
      "No states selected = triggers on any change. Commands run via sh -c with a 10-second timeout — the config file is yours, so there is no extra switch here.",
    commandPlaceholder: "say done",
    hookGroups: "Only these groups",
    hookGroupsHint:
      "No group selected watches the global aggregate: it fires when the overall state moves into one of the states ticked above. Pick groups to watch their own transitions instead — that way one group failing is not swallowed by another being more urgent.",
    groupNotReporting: "This group is not reporting right now, but is still on the hook",
    neverFired: "Not fired yet",
    firedAgo: (age) => `${age} ago`,

    hookPreset: "Body format",
    presetNames: {
      raw: "Raw JSON (your own endpoint)",
      text: "Plain text",
      ntfy: "ntfy",
      bark: "Bark",
      slack: "Slack",
      discord: "Discord",
      lark: "Lark",
      wecom: "WeCom",
      dingtalk: "DingTalk",
      telegram: "Telegram",
      pushdeer: "PushDeer",
      custom: "Custom body",
    },
    presetUrls: {
      raw: "https://your.server/ingest",
      text: "https://your.server/hook",
      ntfy: "https://ntfy.sh/my-topic",
      bark: "https://api.day.app/your-key",
      slack: "https://hooks.slack.com/services/T…/B…/…",
      discord: "https://discord.com/api/webhooks/…/…",
      lark: "https://open.feishu.cn/open-apis/bot/v2/hook/…",
      wecom: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=…",
      dingtalk: "https://oapi.dingtalk.com/robot/send?access_token=…",
      telegram: "https://api.telegram.org/bot<token>/sendMessage?chat_id=…",
      pushdeer: "https://api2.pushdeer.com/message/push?pushkey=…",
      custom: "https://your.server/hook",
    },
    presetHint:
      "Everything except Raw JSON and Custom body sends just the one line below — those services want a URL and a sentence, and they do the formatting. Pick one and hit Send test to see what lands on your phone.",
    hookBody: "Request body (custom)",
    hookBodyPlaceholder: '{"text":"{state} · {group}","message":"{message}"}',
    hookBodyHint:
      "Placeholders: {state} {aggregate} {group} {message} {duration} {since_ms} {at_ms} {summary}. With a JSON content-type the substituted values are escaped for you (quotes and newlines included); the value itself carries no quotes, so write the quotes in the template.",
    hookContentType: "content-type",
    hookHeaders: "Headers (for auth)",
    headerNamePlaceholder: "Authorization",
    headerValuePlaceholder: "Bearer …",
    addHeader: "+ Add header",
    headersHint:
      "Endpoints that need auth go here — Authorization: Bearer …, ntfy's X-Token. Up to 16.",
    headerDuplicate: "That name is already here (header names are case-insensitive)",
    sendTest: "Send test",
    testNothing: "No forwarding target filled in yet",
    testSent: (n) => `Sent ${n} — results are on each hook's own line below`,

    alwaysOnTop: "Always on top",
    trayFollowsState: "Tray follows state",
    launchAtLogin: "Launch at login",
    clickThrough: "Click-through",
    systemHintTray:
      "With tray-follows-state the menu-bar dot takes the current state's colour — you can tell what's going on without looking at the lamp. Turned off, it's a monochrome glyph that inverts with the menu bar's light/dark.",
    systemHintPassThrough: (shortcut) =>
      `In click-through the dot stops responding to the mouse; press ${shortcut} or quit from the tray menu. The mode is not kept for the next launch.`,
    shortcutFallback: "the shortcut",
    shortcutsTitle: "Global shortcuts",
    shortcutActions: {
      lamp: "Show / hide the lamp",
      mute: "Mute the worst group",
      clear: "Clear the worst group",
      inbox: "Open the inbox",
      pass_through: "Toggle click-through",
    },
    shortcutTaken: "taken by another app",
    shortcutsHint:
      "The combinations are fixed (making them configurable would not fix the real failure \u2014 another app already owning one). Any that could not be registered are flagged above. \u201cThe worst group\u201d is the most urgent one, the same rule that puts a sign at the top of the lamp; pinning only changes the display order.",
    language: "Language",
    langAuto: "System",
    langZh: "中文",
    langEn: "English",
    theme: "Theme",
    themeAuto: "System",
    themeLight: "Light",
    themeDark: "Dark",
    themeHint:
      "Only the settings window and the demo page follow this. The lamp and inbox are translucent dark overlays and stay the same under every system appearance.",

    configBackup: "Configuration backup",
    exportConfig: "Export…",
    importConfig: "Import…",
    configBackupHint:
      "Exports a JSON file with every preference (colours, shapes, groups, quiet hours, hooks, forwarding). Import it back on another machine or after a reinstall. Import validates first: a bad colour or an unknown state name is reported as an error, while out-of-range values (a size of 500, say) are clamped rather than rejecting the whole file.",
    exportDone: (path) => `Exported to ${path}`,
    importDone: "Imported — settings applied",
    configExportTitle: "Export settings",
    configImportTitle: "Import settings",
    configFilterName: "SignalHub settings",

    updateTitle: "Updates",
    updateCurrent: (version) => `Version v${version}`,
    checkUpdate: "Check for updates",
    checkingUpdate: "Checking…",
    updateLatest: "You're up to date",
    updateAvailable: (version) => `Version v${version} is available`,
    updateDownloadInstall: "Download and install",
    updateDownloading: (progress) => `Downloading ${progress}`,
    updateInstalling: "Installing — the app will restart when it's done",
    updateNotes: "What's new",
    updateHint:
      "Updates are never installed on their own: nothing is downloaded until you click download, and the app verifies the signature, swaps itself in, and restarts. It also checks silently once a day; failures only go to the log.",
    updateUnsupported:
      "Automatic updates aren't available on this platform yet — download manually from the releases page. On Windows the archive library the updater needs was yanked upstream; on Linux neither install path has been verified on real hardware (the .deb needs root). Better to say so than to show a button that errors out.",
    updateFailed: (reason) => `Check failed: ${reason}`,
    openReleases: "Open releases page",

    lanAccess: "Allow LAN access",
    lanAccessHint:
      "Turned off, only this machine can report. Turned on, agents on other machines, in containers, or under WSL can reach it — at the cost that anyone on the same network who has the token can report. The \"reject requests carrying Origin\" defence stops a web page poking the local port, not a client like curl. So only turn it on for a network you trust (home, an office intranet) — never on public WiFi.",
    lanAddress: "LAN address",
    lanRestart: "Takes effect after restarting the app.",
    webPanel: "Read-only web panel",
    webPanelUrl: "Panel URL",
    webPanelHint:
      "Turn this on to watch the lamp from a phone or tablet (the state, what each group is doing, how long it has been that way). Read-only by default — the allow / deny buttons only appear when \"Approve from the panel\" below is also on. Takes effect immediately.",
    webPanelLanWarning:
      "Note: LAN access is on too, so any device on the same WiFi can see your lamp as long as it has the link above (which carries the token). Don't share that link, and don't leave this on while you're on public WiFi.",
    webApprove: "Approve from the panel",
    webApproveHint:
      "Turn this on and the calls waiting for your approval show up on the panel, with allow / deny buttons (deny can carry a reason). This is an explicit yes to sending the raw command text and file paths to that page — it may be sitting on a tablet in the living room, or that link may have been forwarded. Only does anything while \"Approve from the lamp\" is also on.",
    webApproveNeedsTools: "\"Approve from the lamp\" is off, so this switch won't do anything yet.",

    quit: "Quit SignalHub",

    lampHint: {
      collapse: "Click to collapse / expand · long-press for the radial menu · right-click a sign for its group menu, empty space for the lamp menu",
      inbox: "Click to open the inbox · long-press for the radial menu · right-click a sign for its group menu, empty space for the lamp menu",
      none: "Click does nothing · long-press for the radial menu · right-click a sign for its group menu, empty space for the lamp menu",
    },
    justFinished: (minutes) =>
      minutes < 1 ? "Just finished · moments ago" : `Just finished · ${minutes} min ago`,
    groupsCount: (n) => ` (${plural(n, "group", "groups")})`,
    moreGroups: (n) => `+${n} more`,
    expandNow: "Expanded",
    collapseNow: "Collapsed",
    lampPaused: "Monitoring paused",
    clearSessionHint: "Clear this session (the agent will report again next time)",
    lampPassThrough: "Click-through",
    moreGroupsLead: "Hidden groups: ",

    lampMenuInbox: "Inbox",
    lampMenuLabel: "Lamp menu",
    lampMenuSettings: "Settings",
    lampMenuPause: "Pause",
    lampMenuResume: "Resume",
    lampMenuQuit: "Quit",
    lampMenuCloseHint: "Click empty space or press Esc to close",

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

    machines: {
      title: "Multi-machine hub",
      hint: "Pull other machines' boards in and merge them into one light — cards, inbox, stats and the physical lamp included.",
      empty:
        "No machines yet. Once you add one, its agent states show up as “machine · group” on the light.",
      add: "Add a machine",
      name: "Name",
      nameHint:
        "How this machine is called on the light, and its namespace. Renaming is the same as swapping in a different machine.",
      url: "Address",
      urlHint:
        "The peer's LAN address, e.g. 192.168.1.20:41320. Shown on the peer under Settings → Connect.",
      token: "Token",
      tokenHint: "The contents of the peer's ~/.signalhub/token — also under Settings → Connect.",
      enabled: "Enabled",
      test: "Test",
      testing: "Connecting…",
      remove: "Remove",
      connected: (ms, sessions) => `Connected · ${ms}ms · ${sessions} sessions`,
      never: "Never connected",
      lastOk: (ago) => `Last ok ${ago}`,
      poll: "Poll interval",
      pollHint:
        "How often to fetch each machine. A remote session lives three times this long — so it also sets how quickly a dropped peer shows up on the light.",
      pollSeconds: (seconds) => `${seconds}s`,
      note: "Remote sessions are read-only here: no approvals from the light, and no jumping to a terminal (that directory is on the other machine).",
      peerHint:
        "The peer needs “Allow LAN access” turned on and its token copied over — it does not need to be upgraded.",
    },

    callme: {
      linksTab: "Links",
      serverTab: "Server",
      manageLinks: "Manage links",
      urlLabel: "Server address",
      urlPlaceholder: "https://callme.example.com",
      urlHint: "Your self-hosted Call Me server. https:// is assumed when omitted.",
      tokenLabel: "Admin token",
      tokenPlaceholder: "The server's CALLME_ADMIN_TOKEN",
      tokenHint: "The value set when starting the server; the client uses it to fetch messages.",
      connectionHint:
        "The server is self-hosted (or use the local server above); TLS is handled by your reverse proxy. The address and token stay on this machine only.",
      pollLabel: "Check every",
      pollSeconds: (seconds) =>
        seconds < 60 ? `${seconds}s` : seconds === 60 ? "1 min" : `${seconds / 60} min`,
      test: "Test",
      testing: "Connecting…",
      connected: (ms) => `Connected · ${ms}ms`,
      lastOk: (ago) => `Last ok ${ago}`,
      never: "Never connected",
      unconfigured: "Not configured",
      openServerTab: "Server settings",
      sourcesTitle: "Links (sources)",
      sourcesEmpty: "No links yet. Hit “New link” and paste it into your article — no name or location needed.",
      addSource: "New link",
      creating: "Creating…",
      autoNameHint: "No name needed: the first note fills in its title and where it came from.",
      pendingName: "Named by the first note",
      copyLink: "Copy link",
      copiedLink: "Copied",
      renameLink: "Rename",
      renamePlaceholder: "Name this link",
      openUsage: "Open link to read the instructions",
      revoke: "Revoke",
      revokeConfirm:
        "Revoking disables this link immediately (messages already received are kept). It moves into the Revoked section below, where you can restore or delete it. Continue?",
      revoked: "Revoked",
      deleteLink: "Delete permanently",
      deleteConfirm:
        "Deleting disables this link, removes it from the list and frees its slot. Messages already received are kept but lose this link's name. This cannot be undone. Continue?",
      restoreLink: "Restore",
      revokedTitle: (n) => `Revoked (${n})`,
      revokedHint:
        "These links no longer accept notes. Restore to use one again, or delete to clear it out. Received messages are kept either way.",
      activeEmpty: "No active links. Create a new one, or restore a revoked one below.",
      deletedSource: "Deleted link",
      todayCount: (n) => `${n} in 24h`,
      totalCount: (n) => `${n} total`,
      createdTitle: "Link created",
      createdHint:
        "Copy it and paste it where you want agents to find it. Opening the link shows an agent how to post (GET explains, POST leaves the note), so you don't have to paste instructions alongside it. Anyone who reads it can post until the daily quota runs out; revoke it (reversible) or delete it for good whenever you want.",
      filterAll: "All",
      messagesEmptyNoServer:
        "No Call Me server yet — messages are only fetched once the address and token are filled in.",
      messagesEmptyOffline:
        "Can't reach the Call Me server — check the address, the token and whether the server is running.",
      messagesEmptyNoSource: "No links yet — agents have nowhere to post.",
      messagesEmpty: "No messages yet.",
      filterNoResults: "No messages under this filter.",
      goConfigure: "Set up Call Me",
      checkServer: "Check the server",
      goCreateLink: "Create a link",
      unread: (n) => `${n} unread`,
      markAllRead: "Mark all read",
      openSource: "Open source",
      copyText: "Copy text",
      copiedText: "Copied",
      localTitle: "Local one-click server",
      localHint: "Run a receiving server right inside the desktop app — no Docker, no commands. The address and token get filled in for you; the first start also creates the first link and takes you to the Links tab to copy it.",
      localPort: "Port",
      localPortInvalid: "Port must be between 1024 and 65535.",
      localReplaceConfirm:
        "You currently point at an external server (your VPS, probably). Starting the local one replaces the address and token with the 127.0.0.1 pair and overwrites that configuration — copy those two fields first if you need them. Continue?",
      localStart: "Start local server",
      localStop: "Stop",
      localStarting: "Starting…",
      localStopping: "Stopping…",
      localRunning: "Running",
      localStopped: "Not running",
      localOnlyHint:
        "The local server listens on 127.0.0.1 only: fine for testing, unreachable from the internet. For outside agents, expand “Deploy to the public internet” below. It stops with the app, so you start it again next session — nothing keeps listening behind your back.",
      advancedTitle: "Advanced: manual connection, port and polling",
      remoteTitle: "Deploy to the public internet (VPS)",
      remoteHint: "To let outside agents in, run the server on a public machine. Three commands — copy and run them on your server:",
      remoteStep1: "Clone the repo and build the image on a Docker-capable public machine.",
      remoteStep2: "Set CALLME_ADMIN_TOKEN (16+ characters), start the container with a data volume.",
      remoteStep3: "Point an HTTPS reverse proxy at 127.0.0.1:8080, then fill in the address and token above.",
      copyCommand: "Copy commands",
      copiedCommand: "Copied",
      remoteSecurityHint:
        "Never share the admin token. Terminate TLS at the proxy and set CALLME_TRUST_PROXY=true behind one. All data lives in the /data volume — back that up and you've backed up everything.",
    },

    inbox: {
      title: "Inbox",
      need: "Needs you",
      running: "Running",
      done: "Just finished",
      quiet: "Idle / quiet",
      emptyTitle: "Nothing needs you right now",
      emptyHint:
        "While agents are running this stays quiet; approvals, errors and completions show up here.",
      emptyConnect: "Connect an agent",
      emptyDemo: "See the demo",
      more: "More actions",
      paused: "Monitoring paused",
      jump: "Jump",
      allow: "Allow",
      deny: "Deny",
      mute: "Mute group",
      unmute: "Unmute",
      clear: "Clear",
      watch: "Notify me when done",
      unwatch: "Stop watching",
      watchSet: (label) => `Watching “${label}” — you'll get a notice when it ends`,
      watchCleared: (label) => `Stopped watching “${label}”`,
      close: "Close",
      settings: "Settings…",
      truncated:
        "The command is too long to show in full; it can only be denied here. Open the terminal to allow it.",
      allowBlocked: "Too long to allow from here — deny it or check the terminal",
      allowed: "Allowed — waiting for the agent to continue",
      denied: "Denied",
      cleared: "Session cleared",
      muted: (group) => `Muted “${group}”`,
      unmuted: (group) => `Unmuted “${group}”`,
      waiting: (age) => `waiting ${age}`,
      keys: "↑↓ select · Enter jump · A allow · D deny · M mute · W watch · Esc close",
      jumpUnsupported: "Exact terminal focus is macOS-only for now; tried opening the project folder.",
      remoteJump: (machine) => `This session is on ${machine} — jumping only works for local sessions`,
      approvalHint: "This is a permission the agent asked for; allowing applies only to this request.",
      closeHint: "Close the inbox (Esc)",
      awayFor: (ago) => `While you were away for ${ago}`,
      awayDone: (count) => `${count} finished`,
      awayFailed: (count) => `${count} failed`,
      awayNeed: (count) => `${count} waiting on you`,
      awayRunning: (count) => `${count} still running`,
      awayTransientDone: (count) => `${count} more finished and cleared`,
      awayTransientFailed: (count) => `${count} more failed and cleared`,
      awayQuiet: "Nothing new",
      awayDismiss: "Got it",
    },
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
 * 与 useLocalClock 同构：只在语言真的变了的时候推一次（依赖是解析后的 lang，
 * 而不是整份 settings —— 后者每次渲染都是新对象）。
 */
export function useLangPush(): void {
  const lang = useLang();
  useEffect(() => {
    if (!inTauri) return;
    invoke("set_lang", { lang }).catch((err) => console.error("set_lang 失败", err));
  }, [lang]);
}
