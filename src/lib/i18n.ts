import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import { useSettings, type HttpPreset, type Settings } from "./useSettings";
import type { SignalState } from "./types";

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
  /** 左栏的分组标题。页的「轴」在这里写着（见 SettingsPanel 的 tabGroups）。 */
  railWatch: string;
  railLamp: string;
  railNotify: string;
  railConnect: string;
  tabStatus: string;
  tabHistory: string;
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
  tabHooks: string;
  tabGuide: string;
  tabSystem: string;

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
  permRequest: string;
  permHint: string;

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
  soundWithNotify: string;
  alertAfter: string;
  alertAfterHint: string;
  quietHours: string;
  alertHint: string;
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
  /** 「已接入」但配置里那条命令现在跑不起来了 —— 静默失效，最该被看见的一种 */
  agentCommandBroken: string;
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
  /** 待审批气泡里的按钮 / 说明 */
  approvalAllow: string;
  approvalDeny: string;
  approvalTooLong: string;
  approvalMore: (n: number) => string;
  /** `{}` 已经等了多少秒 */
  approvalWaiting: (secs: number) => string;
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

  quit: string;

  // 灯球 tooltip
  lampHint: string;
  groupsCount: (n: number) => string;
  /** 截断标记的 tooltip：「+3」指的是哪几组 */
  moreGroups: (n: number) => string;
  /** 折叠切换的一次性反馈 */
  expandNow: string;
  collapseNow: string;
  /** 持续可见的状态指示 */
  lampPaused: string;
  /** 牌上那一小块名字的悬停提示（点它就是改名） */
  renameHint: string;
  /** 牌右上角那枚 × 的悬停提示 */
  lampClearGroup: string;
  /** 会话行尾那枚 × 的悬停提示 */
  clearSessionHint: string;
  lampPassThrough: string;
  /** 「+N」截断后，tooltip 里列出被收起来的组之前的引导 */
  moreGroupsLead: string;

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

    railWatch: "看",
    railLamp: "灯",
    railNotify: "通知",
    railConnect: "开始用",
    tabStatus: "状态",
    tabHistory: "历史",
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
    tabHooks: "转发",
    tabGuide: "指南",
    tabSystem: "系统",

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
      "多半是系统没放行 —— 没权限时通知会被静默丢弃，不弹也不报错。「提醒」页顶部能看到权限状态；显示「已被拒绝」的话，要去系统设置的「通知」里手动打开。",

    notifyPermission: "系统通知权限",
    permGranted: "已允许",
    permDenied: "已被拒绝",
    permPrompt: "还没申请",
    permUnknown: "读不到",
    permRequest: "请求权限",
    permHint:
      "没有权限时通知会被静默丢弃 —— 不弹、不响，也不报错。显示「已被拒绝」的话，需要去系统设置的「通知」里手动打开 SignalHub。",

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
      "「每组分灯」让每个组各有一盏能单独拖动的灯，主灯只剩全局状态；每盏灯第一次出现在屏幕右缘，拖到想要的地方后各自记住。超过 8 个组会自动变回一盏灯。",
    size: "大小",
    shape: "形状",
    palette: "配色方案",
    signOpacity: "背景透明度",
    signOpacityHint:
      "牌面淡过一半，近白的字就压不住了 —— 现在牌里的字自动改用深色。",
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
    resetToGlobal: "恢复成跟随全局",
    styleHint:
      "「跟随」= 形状和节奏用「外观」页选的那两项，停留时长用内置默认（跟在每一项里）。停留时长指的是「智能体不出声之后，这块牌还在灯上待多久」—— 它只在「下一次上报」时生效，不会追溯已经亮着的牌。",
    perStateStyle: "逐个状态覆盖",
    perStateSome: (count) => `${count} 个状态单独设过`,
    perStateNone: "未单独设置",
    groupDisplayTitle: "各组的显示",
    groupDisplayEmpty: "还没有任何分组 —— 接上智能体、或调用上报接口之后，它们会出现在这里。",

    notifyWhenNeeded: "需要时弹通知",
    soundWithNotify: "通知带提示音",
    alertAfter: "持续多久才提醒",
    alertAfterHint: "状态要持续这么久才弹通知，一闪而过的忽略掉",
    quietHours: "勿扰时段",
    alertHint:
      "需要授权、需要关注或出错时弹一条系统通知。声音挂在通知上，所以关掉通知也就没有声音。勿扰时段里不弹也不出声，灯照常显示 —— 而且不补发：早上醒来收到一堆昨晚的通知，正是勿扰想避免的。",
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
      "「不分组」：所有不带 group 的上报归到同一块牌。「按目录」：一个工作目录一块牌，每个项目各占一行。「按会话」：一个会话一块牌 —— 同一个智能体并行开几个窗口 / worktree 时，它们的状态各不相同的，只有按会话分才看得出来。三种都只是兜底：上报自己带了 group 的仍然听它的，也是改完从下一次上报开始生效。按目录时分组名就是那条路径；按会话时不会把会话 id 画在牌上，默认显示成「目录末段 · 智能体」（项目在前，因为牌上的名字会被截断，而同一个智能体开好几个窗口时只有项目名能区分它们）。想换别的名字去「外观 → 各组的显示」。",
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
    agentCommandBroken:
      "接入是装上了，但配置里那条命令现在跑不起来 —— 智能体不会报错，灯也不会亮。多半是应用被挪走或删掉了，重新点一次「接入」就会改写成当前路径：",
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
    approvalAllow: "允许",
    approvalDeny: "拒绝",
    approvalTooLong: "这条在灯上看不全，只能在终端里批准（或者在这里拒绝）",
    approvalMore: (n) => `还有 ${n} 条等着`,
    approvalWaiting: (secs) => (secs < 5 ? "刚来" : `等了 ${secs}s`),
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
      pass_through: "切换点击穿透",
    },
    shortcutTaken: "被别的程序占用了",
    shortcutsHint:
      "快捷键是固定的（不做可配：真正的故障是被别的程序占了，那件事可配也解决不了）。抢不到的那一个会在上面标出来。「最差的那一组」= 紧急度最高的那一组，和灯上最上面那块牌同一个判据；置顶只改显示顺序。",
    language: "语言",
    langAuto: "跟随系统",
    langZh: "中文",
    langEn: "English",

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
      "打开后可以在手机 / 平板上看灯（状态、每组在做什么、已持续多久），只读 —— 面板上没有能改变任何东西的按钮。改完立刻生效。",
    webPanelLanWarning:
      "注意：局域网访问也开着，所以同一 WiFi 下的任何设备，只要拿到上面那条带 token 的链接，就能看到你的灯。别把链接发到群里，也别在公共 WiFi 上开着。",

    quit: "退出 SignalHub",

    lampHint: "左键折叠 / 展开 · 右键单组菜单",
    groupsCount: (n) => `（${n} 组）`,
    moreGroups: (n) => `还有 ${n} 组`,
    expandNow: "已展开",
    collapseNow: "已折叠",
    lampPaused: "已暂停监控",
    renameHint: "点击改名（留空则跟随上报方给的组名）",
    lampClearGroup: "清除该组的状态（智能体下次上报会回来）",
    clearSessionHint: "清掉这个会话（智能体下次上报会回来）",
    lampPassThrough: "点击穿透",
    moreGroupsLead: "被收起来的组：",

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

    railWatch: "Overview",
    railLamp: "Lamp",
    railNotify: "Notifications",
    railConnect: "Getting started",
    tabStatus: "Status",
    tabHistory: "History",
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
    tabHooks: "Forwarding",
    tabGuide: "Guide",
    tabSystem: "System",

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
      "Most likely the system has not let it through — without permission a notification is dropped silently, with no popup and no error. The top of the Alerts tab shows the permission state; if it says Denied, turn SignalHub on by hand under Notifications in System Settings.",

    notifyPermission: "System notification permission",
    permGranted: "Granted",
    permDenied: "Denied",
    permPrompt: "Not asked yet",
    permUnknown: "Unreadable",
    permRequest: "Request",
    permHint:
      "Without permission a notification is dropped silently — no popup, no sound, no error. If it says Denied, you need to turn SignalHub on by hand under Notifications in System Settings.",

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
      "\"One per group\" gives each group its own draggable lamp and leaves the main lamp showing global state; new lamps appear along the right edge and each remembers where you drag it. More than 8 groups falls back to a single lamp.",
    size: "Size",
    shape: "Shape",
    palette: "Palette",
    signOpacity: "Sign opacity",
    signOpacityHint:
      "Past half transparency the panel is too faint to carry light text, so the text in the sign switches to dark.",
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
    resetToGlobal: "Reset to follow global",
    styleHint:
      '"Follow" means shape and blink speed come from the Look tab, and the expiry is the built-in default (shown on each row). Expiry is how long a card stays on the lamp after its agent goes quiet — it takes effect on the *next* report, not retroactively.',
    perStateStyle: "Per-state overrides",
    perStateSome: (count) => `${count} state${count === 1 ? "" : "s"} customized`,
    perStateNone: "not customized",
    groupDisplayTitle: "Per-group display",
    groupDisplayEmpty: "No groups yet — they show up here once an agent or the report API tells us about one.",

    notifyWhenNeeded: "Notify when needed",
    soundWithNotify: "Sound with notification",
    alertAfter: "Alert after",
    alertAfterHint: "A state must last this long before notifying; brief ones are ignored",
    quietHours: "Quiet hours",
    alertHint:
      "A system notification when approval, attention or error happens. Sound rides on the notification, so turning notifications off silences it too. During quiet hours nothing pops or sounds and the lamp still shows — and nothing is replayed: waking to a pile of last night's notifications is exactly what quiet hours avoids.",
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
      "\"Nothing\": every report without a group lands on the same sign. \"Directory\": one sign per working directory — each project gets its own row. \"Session\": one sign per session — when a single agent has several windows or worktrees open their states differ, and only per-session grouping shows that. All three are just a fallback: a report that carries its own group still wins, and the change takes effect from the next report on. Directory grouping shows that path; session grouping does not put the session id on the sign — it shows \"last path segment · agent\" by default (project first, because the name gets truncated on the sign and when one agent has several windows open only the project tells them apart). Rename it under Appearance → Per-group display.",
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
    agentCommandBroken:
      "Installed, but the command in the config no longer runs — the agent will not complain and the lamp will not light. Usually the app was moved or deleted; clicking Install again rewrites it to the current path:",
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
    approvalAllow: "Allow",
    approvalDeny: "Deny",
    approvalTooLong: "Too long to show here — approve it in the terminal (or deny it here)",
    approvalMore: (n) => `${n} more waiting`,
    approvalWaiting: (secs) => (secs < 5 ? "just now" : `waiting ${secs}s`),
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
      pass_through: "Toggle click-through",
    },
    shortcutTaken: "taken by another app",
    shortcutsHint:
      "The combinations are fixed (making them configurable would not fix the real failure \u2014 another app already owning one). Any that could not be registered are flagged above. \u201cThe worst group\u201d is the most urgent one, the same rule that puts a sign at the top of the lamp; pinning only changes the display order.",
    language: "Language",
    langAuto: "System",
    langZh: "中文",
    langEn: "English",

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
      "Turn this on to watch the lamp from a phone or tablet (the state, what each group is doing, how long it has been that way). Read-only — there is nothing on the panel that can change anything. Takes effect immediately.",
    webPanelLanWarning:
      "Note: LAN access is on too, so any device on the same WiFi can see your lamp as long as it has the link above (which carries the token). Don't share that link, and don't leave this on while you're on public WiFi.",

    quit: "Quit SignalHub",

    lampHint: "Left-click to collapse / expand · right-click for a group menu",
    groupsCount: (n) => ` (${plural(n, "group", "groups")})`,
    moreGroups: (n) => `+${n} more`,
    expandNow: "Expanded",
    collapseNow: "Collapsed",
    lampPaused: "Monitoring paused",
    renameHint: "Click to rename (leave empty to follow the group name the reporter sends)",
    lampClearGroup: "Clear this group's state (the agent will report again next time)",
    clearSessionHint: "Clear this session (the agent will report again next time)",
    lampPassThrough: "Click-through",
    moreGroupsLead: "Hidden groups: ",

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
