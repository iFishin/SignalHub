import {
  useEffect,
  useState,
  type ChangeEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { inTauri } from "../lib/env";
import { signalLabel } from "../lib/signals";
import { labelOf } from "../lib/lampSort";
import { signTextInverted, stateOverridden, lampVars, resolveShapes } from "../lib/lampStyle";
import {
  commitHeader,
  withoutHeader,
  MAX_HOOK_HEADERS,
} from "../lib/hookHeaders";
import { useLang, useMessages, type Lang, type Messages } from "../lib/i18n";
import { useSettings, updateSettings, DEFAULTS, HTTP_PRESETS, type Settings, type Shape, type Palette, type HttpPreset, type Hook, type StateStyle, type GroupMeta } from "../lib/useSettings";
import {
  useToggles,
  setAutostart,
  setPassThrough,
} from "../lib/useToggles";
import { useStats } from "../lib/useStats";
import { useHistory, type DayRow } from "../lib/useHistory";
import { useTimeline, type TimelineSegment } from "../lib/useTimeline";
import { useOutboundLog, lastAttemptFor, type Attempt } from "../lib/useOutboundLog";
import { useNow } from "../lib/useNow";
import {
  groupReceipts,
  useAgents,
  useHookLog,
  useReceipts,
  type AgentStatus,
  type HookChange,
} from "../lib/useAgents";
import { useStateTtls } from "../lib/useStateTtls";
import { percent, progressLabel } from "../lib/updateProgress";
import { useUpdate } from "../lib/useUpdate";
import { colorVars, resolveColors } from "../lib/palettes";
import { presets, isActivePreset, stylePresets, isActiveStylePreset } from "../lib/presets";
import { nameChanged, withGroupName } from "../lib/groupMeta";
import { SIGNALS, REPORTABLE_SIGNALS, type SignalState, type Snapshot } from "../lib/types";

/**
 * 给「在自己的事件钩子里调」那一类智能体抄的那条命令。
 *
 * **不进 i18n**：它就是一条 shell 命令，`--agent <name>` 里的名字是给上报体用的
 * 标识（`claude-code` / `cursor` / `goose` …，接入页里都列着），翻成中文只会让人
 * 抄错。会话 id 留成占位符 —— 那个值只有对方自己知道。
 *
 * 以前这里点名 Pi：Pi 现在有了一键接入（插件式，见 hooks.rs），再拿它当例子会让人
 * 以为 Pi 还得手抄。
 */
const HOOK_AGENT_COMMAND =
  'signalhub working --agent <name> --session "<session-id>"';

/**
 * 「包一条命令」那条 —— **通用适配**，见 api.md §4.13。
 *
 * 它和上面那条的区别值得摆在界面上：这条**不需要对方有任何 hook 支持**，只要它是个
 * 能跑的命令。对「市场上那些没听说过的工具」来说，这是唯一一条确定能走通的路。
 */
const WRAP_COMMAND = 'signalhub run --agent <name> -- <命令>';

/**
 * 「试一下」那一排按钮发哪几个状态。
 *
 * 挑的是**视觉上各不相同**的四个：绿闪（在干活）、红闪（等你授权）、红频闪
 * （出错了）、一次性脉冲 + 常亮（完成了）。四种节奏和两种颜色各出现一次，
 * 所以点完这一排，用户对「灯长什么样」就有了完整的印象 —— 比只发一个
 * `working` 有用得多。
 *
 * 顺序按「从没事到有事」排，和状态码表里的优先级大体一致。
 */
const TRY_STATES = ["working", "done", "permission", "error"] as const;

/** 与 Rust 侧 config.rs 的 MIN_SIZE / MAX_SIZE / DEFAULT_SIZE 保持一致。 */
function sizeOptions(m: Messages): { label: string; value: number }[] {
  return [
    { label: m.sizeSmall, value: 16 },
    { label: m.sizeMedium, value: 22 },
    { label: m.sizeLarge, value: 32 },
  ];
}

/** 与 Rust 侧 `config::Shape` 一致。 */
function shapeOptions(m: Messages): { label: string; value: Shape }[] {
  return [
    { label: m.shapeCircle, value: "circle" },
    { label: m.shapeRounded, value: "rounded" },
    { label: m.shapeRing, value: "ring" },
  ];
}

/** 与 Rust 侧 `config::Palette` 一致。给预设而不是任意取色。 */
function paletteOptions(m: Messages): { label: string; value: Palette }[] {
  return [
    { label: m.paletteDefault, value: "default" },
    { label: m.paletteColorblind, value: "colorblind" },
    { label: m.paletteVivid, value: "vivid" },
  ];
}

/** 牌上可以单独关掉的四段内容。顺序就是它们在牌上出现的顺序。 */
function fieldOptions(m: Messages): { key: keyof Settings["show"]; label: string }[] {
  return [
    { key: "name", label: m.fieldName },
    { key: "state", label: m.fieldState },
    { key: "age", label: m.fieldAge },
    { key: "message", label: m.fieldMessage },
  ];
}

/** 与 Rust 侧 config.rs 的 MIN_BLINK_SPEED / MAX_BLINK_SPEED 一致。 */
const BLINK_RANGE = { min: 50, max: 200 };

/** 与 Rust 侧 config.rs 的 MIN_SIGN_MULT / MAX_SIGN_MULT 一致。1.0 = 原样。 */
const SIGN_MULT_RANGE = { min: 0.8, max: 2.5 };

/**
 * 单个状态的闪烁间隔选项。`""` = 跟随全局，`"0"` = 常亮。
 *
 * 给一列固定档位而不是让用户敲毫秒：10 个状态各敲一次数字太累，而「快一点 /
 * 慢一点」本来也就那么几档。配置文件里手写的、不在这张表里的值会临时补一个
 * 选项进去（见下面的 `extraBlink`），不会显示成空白。
 */
function blinkOptions(m: Messages): { label: string; value: string }[] {
  return [
    { label: m.follow, value: "" },
    { label: m.solid, value: "0" },
    { label: "2.0s", value: "2000" },
    { label: "1.2s", value: "1200" },
    { label: "0.7s", value: "700" },
    { label: "0.4s", value: "400" },
    { label: "0.2s", value: "200" },
  ];
}

/** 勿扰时段的起止小时。只有小时没有分钟 —— 见 config::QuietHours 的说明。 */
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

/** 「持续多久才提醒」的档位。0 = 立刻。 */
function alertDelayOptions(m: Messages): { label: string; value: number }[] {
  return [
    { label: m.delayNow, value: 0 },
    { label: m.seconds(3), value: 3 },
    { label: m.seconds(5), value: 5 },
    { label: m.seconds(10), value: 10 },
    { label: m.seconds(30), value: 30 },
    { label: m.oneMinute, value: 60 },
  ];
}

/**
 * 页签：**三组 + 一个沉底的「系统」**。
 *
 * 分组不是为了好看，是因为原来那八个平铺按钮之间的**轴不一致**：「安静模式」
 * 「贴边吸附」在【外观】，而同层概念的「点击穿透」却在【系统】；「语言」也在
 * 【系统】。于是「这个开关在哪儿」的答案取决于它当初加进哪个文件，而不是它在
 * 讲什么 —— 用户只能一页一页找（而改动本身是三个人三次加的，谁都不是故意）。
 *
 * 现在的轴是**这一页在回答什么问题**：
 *
 * - 看：只读的观察（现在 / 这几天）
 * - 灯：它长什么样（外观）、怎么动（行为）、每个组怎么显示（分组）
 * - 通知：怎么告诉你（提醒 = 系统通知，转发 = 发出站钩子）
 * - 开始用：怎么把它接上（接入 + 那份手册）
 *
 * 「系统」不属于上面任何一类 —— 语言、开机自启、导入导出是**这台机器上的这个
 * 应用**，所以由左栏单独渲染在最下面（常见的「齿轮位」）。
 */
type Tab =
  | "status"
  | "history"
  | "look"
  | "behavior"
  | "groups"
  | "alert"
  | "hooks"
  | "connect"
  | "guide"
  | "system";

function tabGroups(m: Messages): { title: string; entries: { id: Tab; label: string }[] }[] {
  return [
    {
      title: m.railWatch,
      entries: [
        { id: "status", label: m.tabStatus },
        // 紧挨着「状态」：一个是「现在怎么样」，一个是「这几天怎么样」，同一件事的
        // 两个时间尺度，分开只是因为合在一页会把「现在」淹掉（那一页自己写着
        // 「只回答现在发生了什么」）。
        { id: "history", label: m.tabHistory },
      ],
    },
    {
      title: m.railLamp,
      entries: [
        { id: "look", label: m.tabLook },
        // 「行为」是这一轮从「外观」和「系统」里切出来的：那两页各拿着一半同层的
        // 东西（安静模式 / 贴边吸附 ↔ 点击穿透 / 保持在最前 / 托盘跟随）。
        { id: "behavior", label: m.tabBehavior },
        // 「分组」原先挂在「外观」的末尾，是那一页唯一**会随会话数变长**的一节 ——
        // 而「灯长什么样」是固定的九行。搬走之后两边都回到各自的量级。
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
        { id: "guide", label: m.tabGuide },
      ],
    },
  ];
}

interface Endpoint {
  port: number | null;
  /** 完整 token，不是前几位 —— 对接方要拿它去调接口。 */
  token: string | null;
  /** `~/.signalhub`，token / port / config.json / hooks.log 都在里面。 */
  dir: string | null;
  /** 「允许局域网访问」这个开关的当前值。 */
  lan_access: boolean;
  /**
   * HTTP 服务**实际**绑到了哪里（`true` = 0.0.0.0）。
   *
   * 和 `lan_access` 分开：监听地址只在启动时绑一次，所以用户改完开关、还没重启
   * 时两者不一致 —— 界面靠这个差值提示「重启才生效」。取不到（服务还没起来）
   * 就是 null，那时不提示。
   */
  bound_lan: boolean | null;
  /** 本机在局域网里的地址。取不到就是 null（完全离线 / 只有回环）。 */
  lan_ip: string | null;
}

interface Props {
  snapshot: Snapshot;
}

function Switch({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  return (
    <button
      type="button"
      className="switch"
      role="switch"
      aria-checked={on}
      data-on={on}
      onClick={() => onChange(!on)}
    >
      <span className="knob" />
    </button>
  );
}

/**
 * 一行的「当前值偏离默认了吗」。偏离了就给出「恢复默认」的回调，否则 `undefined`。
 *
 * 判据是模块级的 `DEFAULTS`（`lib/useSettings.ts`）—— 导出 / 导入、Rust 的
 * `Default::default()` 用的都是它，所以不存在「第二个默认值」。值可能是数组或
 * 对象（牌上显示哪几段、勿扰时段、分组偏好），所以用 JSON 比 —— 这几个都是
 * 小对象，比一次的代价远小于维护一份「本行对应哪个字段」的映射。
 *
 * **凡是背后是 `Settings` 字段的行都该有它**。没有的只有两类：背后是运行期状态
 * 的（点击穿透、开机自启、通知权限）和「一次改好几个字段」的预设那行 —— 给预设
 * 挂一个 ↺，会让人以为撤销的是「上一个预设」，其实会把那几项一起拉回默认。
 */
function resetFor<K extends keyof Settings>(
  settings: Settings,
  key: K,
  patch: (next: Partial<Settings>) => void,
): (() => void) | undefined {
  if (JSON.stringify(settings[key]) === JSON.stringify(DEFAULTS[key])) return undefined;
  return () => patch({ [key]: DEFAULTS[key] } as Partial<Settings>);
}

/**
 * 「牌长这样」的实时预览。
 *
 * **为什么要有它**：改外观的时候你不一定看得见那盏灯 —— 它可能被别的窗口挡住、
 * 可能收成了一排点、可能在你正看着的这块屏之外。没有预览，「牌宽 ×1.4」就是在盲改。
 *
 * 它用的是**真的那套 CSS 变量和类名**（`lampVars` + `.lamp-root` / `.sign` / `.dot`），
 * 所以大小、形态、形状、配色、透明度、闪烁节奏、牌宽高倍率全都跟着变，一个都不用
 * 在这里重算。代价是这份标记是**照着 `SignalLamp` 写的**：那边改了结构，这里要手动
 * 跟上（跟不上的后果止于观感，不会出错）。
 */
function LampPreview({ settings }: { settings: Settings }) {
  const m = useMessages();
  const lang = useLang();
  const shapes = resolveShapes(settings);
  // 画两块：一块「忙碌 + 说明」、一块「等你批准」。后者顺带演示了形状那条特判
  // （全局选了圆，permission / error 仍画圆角方），那是外观页里最容易改坏的东西。
  const cards: { state: SignalState; message?: string }[] = [
    { state: "working", message: m.previewMessage },
    { state: "permission" },
  ];

  return (
    <div className="lamp-preview">
      <div
        className="lamp-root"
        style={
          { "--size": `${settings.size}px`, ...lampVars(settings) } as React.CSSProperties
        }
        data-sign-translucent={signTextInverted(settings.sign_opacity) || undefined}
      >
        {cards.map((card) => (
          <div
            key={card.state}
            className="sign"
            data-has-message={settings.show.message && card.message ? "true" : "false"}
          >
            <span className="dot" data-state={card.state} data-shape={shapes[card.state]} />
            <div className="sign-body">
              <div className="sign-line">
                {settings.show.name && <span className="sign-name">{m.previewGroup}</span>}
                {settings.show.state && (
                  <span className="sign-state">{signalLabel(lang, card.state)}</span>
                )}
              </div>
              {settings.show.message && card.message && (
                <div className="sign-message">{card.message}</div>
              )}
            </div>
            {settings.show.age && <span className="sign-age">{m.seconds(12)}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * 一行设置：左边名字，右边控件。
 *
 * `hint` 是**这一行**的说明，写在名字下面 —— 堆在页尾的那些说明和它解释的那一行
 * 隔了十几行，读的时候根本连不上。`title` 是鼠标停上去的一句话，给「一行放不下、
 * 又不值得单占一行」的补充。
 */
function Row({
  label,
  hint,
  title,
  onReset,
  children,
}: {
  label: string;
  hint?: string;
  title?: string;
  /** 偏离默认时传进来（见 `resetFor`），行尾出现一个 ↺ */
  onReset?: () => void;
  children: React.ReactNode;
}) {
  const m = useMessages();
  return (
    <div className="row" title={title}>
      <span className="row-label">
        {label}
        {/* ↺ 紧跟在名字后面，而不是压在右边控件的后面：控件的位置于是不会因为
            「这一行改过没有」而左右跳。 */}
        {onReset && (
          <button
            type="button"
            className="row-reset"
            title={m.resetToDefault}
            aria-label={m.resetToDefault}
            onClick={onReset}
          >
            ↺
          </button>
        )}
      </span>
      {children}
      {hint && <p className="row-hint">{hint}</p>}
    </div>
  );
}

/**
 * 连续值的控件。
 *
 * 透明度和闪烁节奏都不适合分段按钮 —— 它们是有意义区间里的任意值，硬切成三档
 * 会让用户觉得「差一点但调不过去」。旁边带一个读数，否则拖完不知道调到了几。
 */
function Slider({
  value,
  min,
  max,
  step = 1,
  onChange,
  format,
}: {
  value: number;
  min: number;
  max: number;
  /** 步进。默认 1（透明度 / 节奏都是整数）；倍率用 0.1。 */
  step?: number;
  onChange: (value: number) => void;
  format: (value: number) => string;
}) {
  return (
    <div className="slider">
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        // 拖动时连续触发也就连续保存 —— 和取色器同理：调外观必须能实时看到效果
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span className="slider-value">{format(value)}</span>
    </div>
  );
}

/** 把秒数写成人看的时长：45s / 12m / 2h14m。 */
function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h${rest}m`;
}

/**
 * 「多久以前」。与灯球的 `formatAge` 同一套算法，只是入参是 epoch 毫秒（at_ms）。
 *
 * `now` 由调用方传进来（见 `useNow`），**不在这里读 `Date.now()`** —— 那样它就只
 * 在重绘时才被求值，一个不重绘的面板会把这个数字冻在那一刻。
 */
function formatAge(atMs: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - atMs) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}

/**
 * 上报里的状态名（`working`）→ 界面上的说法（「执行中」）。
 *
 * 认不出来就**照原样显示**：上报方可以自己发明状态名？不 —— `Signal::parse` 只收
 * 那几个。但回执是「盘上读回来的历史」，而状态名随版本增删过，所以旧文件里可能有
 * 一个现在不认得的名字。显示原文（它至少是真的）比显示空白好。
 */
function signalText(m: Messages, state: string): string {
  const names = m.signal as unknown as Record<string, string>;
  return names[state] ?? state;
}

/**
 * 「试一下」的结果 → 一句话。
 *
 * 三种情况分开说，因为**它们的含义不同**：命令跑通了（好）、命令跑起来但失败了
 * （可操作 —— 输出里就是原因）、插件式只能内部注入（如实说明它证不了什么）。
 * 混成一句「已完成」的话，用户就分不出「接对了」和「我们只是自己亮了一下」。
 */
function probeLine(m: Messages, outcome: ProbeOutcome): string {
  if (outcome.kind === "plugin") {
    return m.agentTryPlugin(outcome.states.join(" → "));
  }
  const command = outcome.command ?? "";
  if (outcome.kind === "http") {
    return outcome.ran
      ? m.agentTryHttpOk(command)
      : m.agentTryHttpFailed(command, outcome.output.trim());
  }
  if (outcome.ran && outcome.code === 0) return m.agentTryRanOk(command);
  const code = outcome.code === null ? "" : String(outcome.code);
  return m.agentTryRanFailed(command, code, outcome.output.trim());
}

/**
 * 面板是不是真的在屏幕上。
 *
 * 设置窗的 webview 一直活着，隐藏着也照样收事件、照样重绘 —— 而这个窗口里有几个
 * 每秒级的钟（「多久以前」）。隐藏时把它们停掉，是这块面板唯一需要的省电措施。
 */
function useVisible(): boolean {
  const [visible, setVisible] = useState(() => !document.hidden);

  useEffect(() => {
    const sync = () => setVisible(!document.hidden);
    document.addEventListener("visibilitychange", sync);
    window.addEventListener("focus", sync);
    return () => {
      document.removeEventListener("visibilitychange", sync);
      window.removeEventListener("focus", sync);
    };
  }, []);

  return visible;
}

/**
 * 一条出站结果的一行字：`✓ 200 · 2m前` / `✗ connection refused · 5m前`。
 *
 * 成功时 `detail` 是状态码（命令成功则是空的），失败时是原因 —— 两种都摆出来，
 * 用户要的就是「对方回了几 / 为什么没成」。命令成功没有状态码，那一段就省掉，
 * 不留一个空的分隔点。
 */
function attemptLine(m: Messages, attempt: Attempt, now: number): string {
  const mark = attempt.ok ? "✓" : "✗";
  const detail = attempt.detail.trim();
  const age = m.firedAgo(formatAge(attempt.at_ms, now));
  return detail ? `${mark} ${detail} · ${age}` : `${mark} ${age}`;
}

/** 与 Rust 侧 state::MessageEntry 对应。 */
interface MessageEntry {
  /** epoch 毫秒 */
  at_ms: number;
  session: string;
  group: string;
  state: SignalState;
  message: string;
}

/** 消息流最多渲染这么多行。后端留 200 条，但 420px 的面板里列 200 行没法用，
    而最旧的那些本来也最不重要。 */
const FEED_LIMIT = 50;

/** 兜底刷新间隔。快照一变也会重取，这里只是防止长时间没人上报时列表不动。 */
const FEED_REFRESH_MS = 15_000;

/**
 * 最近上报过的消息，从新到旧。
 *
 * 与 useStats / useTimeline 同构：挂载时取一次 + 定时兜底。这里**另外**跟着
 * `aggregate` 重取 —— 状态一变往往意味着刚有人上报，于是新消息立刻出现，不必
 * 等下一个兜底周期。间隔取 15 秒：比状态慢，但足够让「没改聚合的那一路上报」
 * 也浮上来。
 */
function useMessageFeed(aggregate: SignalState): MessageEntry[] {
  const [entries, setEntries] = useState<MessageEntry[]>([]);

  useEffect(() => {
    if (!inTauri) return;

    const load = () => {
      invoke<MessageEntry[]>("get_messages")
        // 后端给的是从旧到新，这里倒过来 —— 最新的排在最上面
        .then((rows) => setEntries(rows.slice().reverse()))
        .catch((err) => console.error("get_messages 失败", err));
    };

    load();
    const timer = window.setInterval(load, FEED_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [aggregate]);

  return entries;
}

/**
 * 近 30 分钟的状态条带。
 *
 * 每一段的**宽度就是它的时长** —— 一眼看出这半小时都在干什么，以及哪一段是不是
 * 卡了很久。今日统计回答「一共多久」，这条回答「什么时候」，两个问题不一样。
 */
function TimelineStrip({
  segments,
  m,
  lang,
}: {
  segments: TimelineSegment[];
  m: Messages;
  lang: Lang;
}) {
  if (segments.length === 0) return null;

  return (
    <div className="timeline">
      <div className="timeline-bar">
        {segments.map((segment, index) => (
          <span
            key={index}
            className="timeline-seg"
            data-state={segment.state}
            // 时长决定宽度。极短的一段也留 1px —— 否则它会在视觉上消失，而
            // 「只闪了一下」恰恰可能是要看的东西
            style={{ flexGrow: Math.max(segment.ms, 1) }}
            title={`${signalLabel(lang, segment.state)} · ${formatDuration(
              Math.round(segment.ms / 1000),
            )}`}
          />
        ))}
      </div>
      <div className="timeline-axis">
        <span>{m.timelineStart}</span>
        <span>{m.timelineNow}</span>
      </div>
    </div>
  );
}

/**
 * 最近几天的每日合计：**一天一行** —— 日期 + 一条按状态分段的横条 + 当天合计。
 *
 * 和上面那条 30 分钟条带是同一个想法（**宽度就是时长**），只是粒度从分钟变成天。
 * 刻意只给 7 天、不做趋势线也不做同比：这是「我这几天都在忙什么」的一屏。往
 * 分析工具那个方向走，尽头是另一个应用 —— 而这个应用是一盏灯。
 */
function HistoryList({ days, m }: { days: DayRow[]; m: Messages }) {
  if (days.length === 0) return null;

  // **所有天按同一个尺度画**（最大的那天占满整条），而不是各画各的。
  // 各归一化的话，只用了 10 分钟的那天会和用了 8 小时的那天一样满 —— 而
  // 「哪几天真的忙」正是看这一屏要说的事。补一段透明尾巴就把尺度统一了。
  const maxTotal = Math.max(...days.map((day) => day.total_seconds), 0);

  return (
    <div className="history">
      {days.map((day, index) => (
        <div className="history-day" key={day.day}>
          {/* 「今天」只认下标：Rust 保证今天在最前（它在 totals 里，不在 history 里），
              而两边的「今天」都来自同一个本地时钟 —— 再比一次日期字符串反而多出
              一处可以对不上的地方。 */}
          <span className="history-date">
            {index === 0 ? m.historyToday : shortDay(day.day)}
          </span>
          <span className="history-bar">
            {day.rows.map((row) => (
              <span
                key={row.state}
                className="history-seg"
                data-state={row.state}
                // 时长决定宽度。极短的一段也留 1px，否则它会在视觉上消失
                style={{ flexGrow: Math.max(row.seconds, 1) }}
                title={`${row.label} · ${formatDuration(row.seconds)}`}
              />
            ))}
            {/* 补齐到「最忙的那天」：透明，只是把这一天的条压到它该有的比例 */}
            <span
              className="history-rest"
              style={{ flexGrow: Math.max(maxTotal - day.total_seconds, 0) }}
            />
          </span>
          <span className="history-total">{formatDuration(day.total_seconds)}</span>
        </div>
      ))}
    </div>
  );
}

/** `2026-10-12` → `10-12`。年份在「最近几天」这种列表里是纯噪音。 */
function shortDay(day: string): string {
  return day.length === 10 ? day.slice(5) : day;
}

/**
 * 逐个状态的外观覆盖：颜色 / 形状 / 节奏，各自可以只覆盖其中几样。
 *
 * 单独提出来而不是内联在「外观」那个 case 里：它自成一块（一组预设 + 一张表 +
 * 两个改表的 helper），塞进去会把那一段撑得没法读。
 *
 * **三样都没设就把整条记录删掉。** 留一条全 null 的空记录会让「当前是哪套预设」
 * 判断不出来（预设比的是整份表），而它本身没有任何作用。
 */
/**
 * 停留时长的可选档位（毫秒）。**没有「永不过期」** —— 理由在 Rust 那边写着
 * （`config::MIN_STATE_TTL_MS`）：一盏不会自己熄灭的灯就是一条永远不撤的告警。
 *
 * 和节奏那一列同一套做法：窄列里放一个数字输入框，既难调（要自己算毫秒）又容易
 * 输错（0 到底是「不过期」还是「立刻」）。档位全部落在后端允许的区间内。
 */
const TTL_CHOICES_MS = [
  30_000,
  60_000,
  120_000,
  300_000,
  600_000,
  1_800_000,
  3_600_000,
  14_400_000,
  86_400_000,
];

/**
 * 开放「停留时长」的状态。
 *
 * `off` / `offline` 永远不会成为会话状态（前者在 `apply` 里等价于清除会话，后者是
 * 汇总产物），拿了也没用；`stale` 是**静默超时之后**的派生状态，它自己那一段宽限期
 * 是固定的，而且细环画的就是那一段的剩余比例 —— 让它可调就得连那个环的算法一起改。
 */
const TTL_STATES: readonly SignalState[] = [
  "idle",
  "done",
  "thinking",
  "working",
  "attention",
  "permission",
  "error",
];

function PerStateStyles({
  settings,
  patch,
  m,
  lang,
  shapes,
  blinkChoices,
  ttlDefaults,
}: {
  settings: Settings;
  patch: (next: Partial<Settings>) => void;
  m: Messages;
  lang: Lang;
  shapes: { label: string; value: Shape }[];
  blinkChoices: { label: string; value: string }[];
  /** 状态 → 内置停留时长（毫秒）。空表也能跑，只是「跟随」后面不带那个数。 */
  ttlDefaults: Record<string, number>;
}) {
  const resolved = resolveColors(settings);

  const setStyle = (state: string, next: Partial<StateStyle>) => {
    const merged = { ...(settings.styles[state] ?? {}), ...next };
    const styles = { ...settings.styles };
    // 判据和读侧的 `stateOverridden` 必须一致，否则会出现「摘要说 3 个状态设过、
    // 表里却只有 2 行是彩色的」这种对不上的现象（见 `lampStyle.ts` 的说明）。
    const empty =
      merged.color == null &&
      merged.shape == null &&
      merged.blink_ms == null &&
      merged.ttl_ms == null;
    if (empty) delete styles[state];
    else styles[state] = merged;
    patch({ styles });
  };

  const resetStyle = (state: string) => {
    const styles = { ...settings.styles };
    delete styles[state];
    patch({ styles });
  };

  return (
    <>
      <Row label={m.presets}>
        {/* seg 而不是 chip：这是一组里选一个，和下面的形状 / 节奏两列同一件事。
            预设给的是**完整**一份表，所以是整份替换而不是合并。 */}
        <div className="seg">
          {stylePresets(m).map((preset) => (
            <button
              key={preset.id}
              type="button"
              title={preset.hint}
              data-active={isActiveStylePreset(preset, settings)}
              onClick={() => patch({ styles: preset.styles })}
            >
              {preset.label}
            </button>
          ))}
        </div>
      </Row>

      <div className="style-head">
        <span className="style-swatch" />
        <span className="style-name">{m.state}</span>
        <span className="style-col">{m.shape}</span>
        <span className="style-col">{m.rhythm}</span>
        <span className="style-col style-col-tight">{m.stayDuration}</span>
        <span className="style-reset" />
      </div>

      <div className="styles">
        {SIGNALS.map((state) => {
          const style = settings.styles[state] ?? {};
          // 「这一行被改过吗」用共享判据（`stateOverridden`）—— 收起时那行摘要用的
          // 是同一个，两边不一致就会出现摘要和表格对不上的现象。
          const overridden = stateOverridden(style);
          const blink = style.blink_ms == null ? "" : String(style.blink_ms);
          // 手改配置文件可能写出选项里没有的毫秒数，临时补一个选项，
          // 否则 select 会显示成空白 —— 看起来像「没设」，其实设了
          const extraBlink =
            blink !== "" && !blinkChoices.some((o) => o.value === blink) ? blink : null;
          const ttl = style.ttl_ms == null ? "" : String(style.ttl_ms);
          const extraTtl =
            ttl !== "" && !TTL_CHOICES_MS.includes(Number(ttl)) ? ttl : null;
          // 「跟随」后面跟一句内置默认（`跟随 5m`）：这一列问的就是「到底多久」，
          // 只写「跟随」两个字等于没答。数由后端给（`state_ttl_defaults`），拿不到
          // 就退回纯「跟随」。
          const builtinTtl = ttlDefaults[state];
          const ttlLabel =
            builtinTtl == null ? m.follow : `${m.follow} ${formatDuration(builtinTtl / 1000)}`;

          return (
            <div className="style-row" key={state} data-state={state}>
              {/* 原生取色器。onChange 在拖动时连续触发，也就连续保存 ——
                  这是刻意的：改颜色必须能实时看到效果。 */}
              <input
                type="color"
                className="style-swatch"
                title={m.lightColor}
                value={resolved[state]}
                onChange={(event) => setStyle(state, { color: event.target.value })}
              />
              <span className="style-name">{signalLabel(lang, state)}</span>
              <select
                className="select style-col"
                title={m.lightShape}
                value={style.shape ?? ""}
                onChange={(event) =>
                  setStyle(state, {
                    shape: (event.target.value || null) as Shape | null,
                  })
                }
              >
                <option value="">{m.follow}</option>
                {shapes.map((shape) => (
                  <option key={shape.value} value={shape.value}>
                    {shape.label}
                  </option>
                ))}
              </select>
              <select
                className="select style-col"
                title={m.blinkInterval}
                value={blink}
                onChange={(event) =>
                  setStyle(state, {
                    blink_ms:
                      event.target.value === "" ? null : Number(event.target.value),
                  })
                }
              >
                {blinkChoices.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
                {extraBlink && <option value={extraBlink}>{extraBlink}ms</option>}
              </select>
              {/* 停留时长：只对**会被上报的状态**开放，其余三行这一格留白
                  （理由见 `TTL_STATES`）。留白而不是禁用，是因为禁用看起来像
                  「现在不能改」，而事实是「这一档对它没有意义」。 */}
              {TTL_STATES.includes(state) ? (
                <select
                  className="select style-col style-col-tight"
                  title={m.stayDurationHint}
                  value={ttl}
                  onChange={(event) =>
                    setStyle(state, {
                      ttl_ms:
                        event.target.value === "" ? null : Number(event.target.value),
                    })
                  }
                >
                  <option value="">{ttlLabel}</option>
                  {TTL_CHOICES_MS.map((ms) => (
                    <option key={ms} value={String(ms)}>
                      {formatDuration(ms / 1000)}
                    </option>
                  ))}
                  {extraTtl && (
                    <option value={extraTtl}>{formatDuration(Number(extraTtl) / 1000)}</option>
                  )}
                </select>
              ) : (
                <span className="style-col style-col-tight" />
              )}
              <button
                type="button"
                className="btn btn-tiny style-reset"
                title={m.resetToGlobal}
                disabled={!overridden}
                onClick={() => resetStyle(state)}
              >
                ↺
              </button>
            </div>
          );
        })}
      </div>

      <p className="hint">{m.styleHint}</p>
    </>
  );
}

/**
 * 系统通知权限的状态，以及「要一次」的入口。
 *
 * 这一行的理由很具体：**没有权限时通知是静默丢弃的** —— 不弹、不响，也没有任何
 * 地方会报错（从 Dock 启动的话连 stderr 都没地方看）。用户看到的是「通知功能
 * 坏了」，但代码里其实什么都对，只是系统没放行。把状态摆出来，「到底通没通」
 * 才是个能回答的问题。
 *
 * `enabled` 是通知开关：打开开关时 Rust 那边会去申请一次权限，所以这里跟着重取
 * 一次，好让刚点完开关就能看到结果。
 */
function NotifyPermission({ m, enabled }: { m: Messages; enabled: boolean }) {
  const [state, setState] = useState("unknown");

  useEffect(() => {
    if (!inTauri) return;
    invoke<string>("get_notify_permission")
      .then(setState)
      .catch(() => setState("unknown"));
  }, [enabled]);

  const labels: Record<string, string> = {
    granted: m.permGranted,
    denied: m.permDenied,
    prompt: m.permPrompt,
    unknown: m.permUnknown,
  };

  return (
    <div className="section">
      <div className="row">
        <span>{m.notifyPermission}</span>
        <span className="badge" data-on={state === "granted"}>
          {labels[state] ?? m.permUnknown}
        </span>
        {state === "prompt" && (
          <button
            type="button"
            className="btn btn-tiny"
            onClick={() =>
              invoke<string>("request_notify_permission")
                .then(setState)
                .catch((err) => console.error("request_notify_permission 失败", err))
            }
          >
            {m.permRequest}
          </button>
        )}
      </div>
      {state !== "granted" && <p className="hint">{m.permHint}</p>}
    </div>
  );
}

/**
 * 请求头输入框的两个按键。
 *
 * **Esc 不失焦**，只把草稿丢掉：失焦会触发 blur → 提交，那就成了「按 Esc 反而把它
 * 存了进去」。丢完草稿后输入框还在焦点上，但显示的是存着的那个值 —— 接着打字就是
 * 一次新的修改。
 *
 * Enter 也不自己提交，只失焦 —— 提交只走 blur 那一条路，免得同一个草稿被提交两次
 * （第一次改完了，第二次还拿着旧的那份去重算）。
 */
function headerKeys(cancel: () => void) {
  return (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") event.currentTarget.blur();
    if (event.key === "Escape") cancel();
  };
}

/** 与 Rust 侧 lib.rs 的 ProbeOutcome 对应。 */
interface ProbeOutcome {
  /**
   * `command` = 真跑了一遍接入命令；`plugin` = 内部注入；
   * `http` = 真发了一次 HTTP 到本机那个 hook 端点
   */
  kind: "command" | "plugin" | "http";
  ran: boolean;
  code: number | null;
  output: string;
  command: string | null;
  states: string[];
}

export function SettingsPanel({ snapshot }: Props) {
  const settings = useSettings();
  const toggles = useToggles();
  const stats = useStats();
  const timeline = useTimeline(snapshot.aggregate);
  const { days: historyDays, refresh: refreshHistory } = useHistory();
  const feed = useMessageFeed(snapshot.aggregate);
  // 「发送测试」点完就加一，逼 `useOutboundLog` 立刻重取一次。没有它的话结果最多
  // 要等 15 秒（定时兜底）才出现 ——「点了没反应」正是这一轮要消灭的那种体验。
  const [outboundNonce, setOutboundNonce] = useState(0);
  const outboundLog = useOutboundLog(snapshot.aggregate, outboundNonce);
  const { agents, refresh: refreshAgents } = useAgents();
  // 「它到底有没有在报」——接入页以前回答不了的那个问题。和时间有关，所以跟着
  // 上面那个会走的钟（`useNow`）一起算。
  const { receipts, refresh: refreshReceipts } = useReceipts();
  const { lines: logLines, refresh: refreshLog } = useHookLog();
  const m = useMessages();
  const lang = useLang();
  const sizes = sizeOptions(m);
  const shapes = shapeOptions(m);
  const palettes = paletteOptions(m);
  const fields = fieldOptions(m);
  const blinkChoices = blinkOptions(m);
  const alertChoices = alertDelayOptions(m);
  const ttlDefaults = useStateTtls();
  const update = useUpdate();
  const groups = tabGroups(m);
  const [tab, setTab] = useState<Tab>("status");
  // 逐状态那张表默认收起（十行表和上面九行不是一个量级）。它记的是**用户**的
  // 展开意图，所以放在 state 里；切走再切回来会保留（组件没卸载）。
  const [perStateOpen, setPerStateOpen] = useState(false);
  // 发布说明默认收起：它是**可选阅读**（想知道改了什么才看），不该把系统页撑长
  const [notesOpen, setNotesOpen] = useState(false);
  // 收起时那一行摘要要用：有几个状态被单独设过。判据和表里那个 `stateOverridden`
  // 是同一个（见它的说明），所以摘要和表格不可能对不上。
  const perStateCount = SIGNALS.filter((state) => stateOverridden(settings.styles[state])).length;

  // 「多久以前」要跟着时间走。**只在真的显示这几段的页签上开钟**（消息流在
  // 「状态」、出站结果在「转发」、回执在「接入」），其它几页一个定时器都不起 ——
  // 这块面板一秒重绘一次不便宜，没显示的东西不值得为它重绘。
  const visible = useVisible();
  const now = useNow(
    visible && (tab === "status" || tab === "hooks" || tab === "connect"),
  );

  const [endpoint, setEndpoint] = useState<Endpoint | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 正在接入 / 卸载的那个智能体（按 id），用来禁用按钮。同时只可能有一个在飞。
  const [busyAgent, setBusyAgent] = useState<string | null>(null);
  // 每个智能体最近一次接入 / 卸载改了什么，就地显示一行结果 —— 这是用户唯一
  // 能看到「刚才那下到底做了什么」的地方。
  const [agentChanges, setAgentChanges] = useState<Record<string, HookChange>>({});
  // 「试一下」的结果，按 id 就地显示一行 —— 它回答的是「现在这条链路还通不通」，
  // 和上面的回执（「最近有没有在报」）是两个问题。
  const [probeResults, setProbeResults] = useState<Record<string, ProbeOutcome>>({});
  // 配置导出 / 导入的结果，就地显示在按钮下面
  const [backupNotice, setBackupNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    invoke<Endpoint>("get_endpoint").then(setEndpoint).catch(() => {
      // 端口/token 读不到就是「服务还没起来」，界面上显示为 — 即可
    });
  }, []);

  const reported = (err: unknown) => setError(String(err));

  /**
   * 导出 / 导入设置。
   *
   * **选文件走 dialog 插件，读写和校验都在 Rust。** 前端只拿一个路径，不碰文件
   * 内容 —— 导入必须过 Rust 的 `validate_settings` → `clamped()`，前端自己读文件
   * 再塞进 `set_settings` 就绕过了那道检查。
   *
   * 用户点「取消」时插件返回 `null`，那不是错误，什么都不做。
   */
  const exportConfig = async () => {
    setError(null);
    setBackupNotice(null);
    try {
      const path = await save({
        title: m.configExportTitle,
        defaultPath: "signalhub-settings.json",
        filters: [{ name: m.configFilterName, extensions: ["json"] }],
      });
      if (!path) return; // 取消
      await invoke("export_settings", { path });
      setBackupNotice(m.exportDone(path));
    } catch (err) {
      reported(err);
    }
  };

  const importConfig = async () => {
    setError(null);
    setBackupNotice(null);
    try {
      const path = await open({
        title: m.configImportTitle,
        multiple: false,
        directory: false,
        filters: [{ name: m.configFilterName, extensions: ["json"] }],
      });
      if (!path) return; // 取消
      // 导入成功后设置会经 `signalhub://settings` 广播回来，两个窗口一起更新，
      // 所以这里不用手动 setSettings
      await invoke("import_settings", { path });
      setBackupNotice(m.importDone);
    } catch (err) {
      reported(err);
    }
  };

  /**
   * 给一个智能体接入 / 卸载 hook。
   *
   * 接入和卸载走同一段逻辑，只换命令名 —— 结果里那行「新增 N 个事件 / 移除 N 个
   * handler」就是它俩的区别。调用返回后重取一次列表：`installed` 变了，状态标记
   * 和按钮文案都得跟着变。
   */
  /**
   * 接入 / 卸载的**收尾**：接入和换模式两条路都走这里。
   *
   * 抽出来是因为两处的收尾每一步都一样，而漏掉任何一步的表现都是「界面在撒谎」：
   * 不回执就不刷新（会一直显示「还没有收到过它的上报」）、不重取列表（徽章还是旧的
   * 「未接入」）。
   */
  const finishAgentChange = (agent: AgentStatus) => (change: HookChange) => {
    setAgentChanges((prev) => ({ ...prev, [agent.id]: change }));
    refreshAgents();
    // 回执也要重取：装完之后那一行该立刻从「没有回执」变成真实状态，而
    // 「刚接入」这几秒里它多半还是「还没有收到过它的上报」——那正是实话。
    refreshReceipts();
  };

  const changeAgent = (agent: AgentStatus, install: boolean) => {
    setError(null);
    setBusyAgent(agent.id);
    const command = install ? "install_agent_hooks" : "uninstall_agent_hooks";
    invoke<HookChange>(command, { agent: agent.id })
      .then(finishAgentChange(agent))
      .catch(reported)
      .finally(() => setBusyAgent(null));
  };

  /**
   * 换成 **HTTP 方式**接入（目前只有 Claude Code）。
   *
   * 和 `changeAgent` 是同一件事的另一条路：对用户来说就是「再点一次接入，只是写法
   * 不同」，所以 busyAgent、结果行、回执刷新全都共用 —— 两条路各写一套状态，迟早
   * 会出现「换完模式界面还停在旧文字上」。
   */
  const changeAgentHttp = (agent: AgentStatus) => {
    setError(null);
    setBusyAgent(agent.id);
    invoke<HookChange>("install_agent_http_hooks", { agent: agent.id })
      .then(finishAgentChange(agent))
      .catch(reported)
      .finally(() => setBusyAgent(null));
  };

  /**
   * 试一下这条接入：让 Rust 把配置里**实际写着**的那条命令跑一遍。
   *
   * 它比回执更能回答「现在通不通」——尤其是那个最隐蔽的成因：应用从 Dock 起来时
   * PATH 和用户 shell 里的不一样，而配置里写的偏偏是裸命令名。
   */
  const tryAgent = (agent: AgentStatus) => {
    setError(null);
    setBusyAgent(agent.id);
    invoke<ProbeOutcome>("try_agent", { agent: agent.id })
      .then((outcome) => {
        setProbeResults((prev) => ({ ...prev, [agent.id]: outcome }));
        // 命令真的跑起来后，回执那边**应该**马上就有一条；顺手刷一次，用户就能看到
        // 「最近上报 0s 前」和这一行同时出现 —— 那是最有说服力的一刻。
        window.setTimeout(refreshReceipts, 600);
      })
      .catch(reported)
      .finally(() => setBusyAgent(null));
  };

  /**
   * 清掉某一个会话。
   *
   * 清一个**不存在**的会话是幂等的（Rust 侧返回 false、不报错）—— 和 HTTP 的
   * `DELETE /v1/sessions/{id}` 一个脾气。清完不用手动刷新列表：Rust 会把新快照
   * 广播回来，两个窗口一起更新。
   */
  const clearSession = (session: string) => {
    setError(null);
    invoke<boolean>("clear_session", { session }).catch(reported);
  };

  const sessionCount = snapshot.groups.reduce(
    (sum, group) => sum + group.sessions.length,
    0,
  );

  const patch = (next: Partial<Settings>) => {
    setError(null);
    updateSettings({ ...settings, ...next }).catch(reported);
  };

  /** 单独改「牌上显示哪几段」里的一段。 */
  const setField = (key: keyof Settings["show"], on: boolean) =>
    patch({ show: { ...settings.show, [key]: on } });

  /** 把某一组在灯上隐藏 / 重新显示。 */
  const setHidden = (group: string, hidden: boolean) =>
    patch({
      hidden_groups: hidden
        ? [...settings.hidden_groups, group]
        : settings.hidden_groups.filter((name) => name !== group),
    });

  /**
   * 「各组的显示」那一节要列出的组。
   *
   * **不能只取 `snapshot.groups`** —— 那是「此刻正在上报的组」。一组被隐藏之后
   * 可能再也不上报，而恰恰是那时候最需要能把它单独放回来：只列上报中的组，它就
   * 从界面上彻底消失，只剩「全部恢复」那条把所有组一起清掉的路。
   * 所以并上设置里记过的所有组名（改过名的、钉过的、隐藏的、静音的）。
   */
  const knownGroups = [
    ...new Set([
      ...snapshot.groups.map((group) => group.name),
      ...Object.keys(settings.group_meta),
      ...settings.hidden_groups,
      ...settings.muted_groups,
    ]),
  ].sort();

  /**
   * 组名 → 服务端给的默认显示名，给下面那一节用。
   *
   * `knownGroups` 只是名字（要并上已经不上报的组），而默认名在 `GroupView` 上，
   * 所以这里单建一张表。不在表里的组没有默认名，退回原始组名 —— 那正是
   * 「留空就用 X」该显示的东西。
   */
  const defaultLabels = new Map(
    snapshot.groups.map((group) => [group.name, group.default_label]),
  );

  /**
   * 静音 / 取消静音某一组。
   *
   * 与「隐藏」是两件事：静音只让通知和提示音不再因为这一组响起，灯上照常显示；
   * 隐藏只是让它不占灯上的位置。所以两个状态可以同时成立，也各自独立。
   */
  const setMuted = (group: string, muted: boolean) =>
    patch({
      muted_groups: muted
        ? [...settings.muted_groups, group]
        : settings.muted_groups.filter((name) => name !== group),
    });

  /** 改某一组的显示偏好。两样都清空就把整条删掉 —— 空条目没有任何作用。 */
  const setGroupMeta = (group: string, next: GroupMeta) => {
    const group_meta = { ...settings.group_meta };
    const empty = (next.name ?? null) === null && (next.order ?? null) === null;
    if (empty) delete group_meta[group];
    else group_meta[group] = next;
    patch({ group_meta });
  };

  /** 清空显示名 = 回到「跟随上报方给的原始组名」。
   *
   *  改名的算法住在 `lib/groupMeta.ts` 里，因为**灯上那块牌也能就地改名** ——
   *  两处各写一份，就会出现「灯上改完，设置里看着还是旧名字」。 */
  const setGroupName = (group: string, name: string) =>
    patch({ group_meta: withGroupName(settings.group_meta, group, name) });

  /**
   * 置顶 / 取消置顶。
   *
   * 权重取「现有最大值 + 1」，于是置顶组之间的顺序就是用户按下开关的顺序 ——
   * 不必再做一个拖拽界面，而用户心里本来也是「先钉的先看」。
   */
  const setPinned = (group: string, pinned: boolean) => {
    const highest = Math.max(
      -1,
      ...Object.values(settings.group_meta).map((meta) => meta.order ?? -1),
    );
    setGroupMeta(group, {
      ...settings.group_meta[group],
      order: pinned ? highest + 1 : null,
    });
  };

  // 显示名也用「草稿 + 失焦提交」，理由同钩子的输入框：每敲一个字就存一次会把
  // 半截名字写进配置。
  //
  // `seed` 是**聚焦那一刻填进框里的值**，也就是这一组此刻显示的名字（用户起的 →
  // 探测出来的 → 原始组名）。以前填的是「用户自定义名」那个字段本身，没改过名时
  // 是空的 —— 于是用户得照着右边灰色占位里的名字重敲一遍。
  const [nameDraft, setNameDraft] = useState<{ group: string; value: string; seed: string } | null>(
    null,
  );

  const commitNameDraft = () => {
    if (!nameDraft) return;
    const { group, value, seed } = nameDraft;
    setNameDraft(null);
    // 判据与灯上就地改名共用（`lib/groupMeta.ts::nameChanged`），否则会出现
    // 「灯上点一下不会写配置，设置里点一下却写了」
    const current = settings.group_meta[group]?.name ?? "";
    if (!nameChanged(seed, value.trim(), current)) return;
    setGroupName(group, value.trim());
  };

  // 钩子的文本输入用「草稿 + 失焦提交」：每敲一个字就存一次会把半截 URL
  // 写进配置（而且会被校验拒掉）。颜色不一样 —— 取色器要的就是即时反馈。
  const [hookDraft, setHookDraft] = useState<{ index: number; value: string } | null>(null);

  const setHooks = (hooks: Hook[]) => patch({ hooks });

  const updateHook = (index: number, next: Partial<Hook>) =>
    setHooks(
      settings.hooks.map((hook, i) =>
        i === index ? ({ ...hook, ...next } as Hook) : hook,
      ),
    );

  const addHook = () =>
    setHooks([
      ...settings.hooks,
      // 目标先留空：空目标不算错（不触发），用户填了才开始工作。
      // groups 也先空着 = 看全局聚合，这是原有语义，新钩子不该默认变成按组过滤
      // preset / headers 写成默认值：从 command 切过来的钩子没有这两样，写全了
      // 界面就不用到处 `?? {}` 兜底（Rust 侧也有 serde(default) 兜着）。
      {
        when: ["permission", "attention", "error"],
        groups: [],
        type: "http",
        url: "",
        headers: {},
        preset: "raw",
      },
    ]);

  const commitHookDraft = () => {
    if (!hookDraft) return;
    const { index, value } = hookDraft;
    setHookDraft(null);
    const hook = settings.hooks[index];
    if (!hook) return;
    const next = value.trim();
    const current = hook.type === "http" ? hook.url : hook.command;
    if (next === (current ?? "").trim()) return; // 没改就别白存一次
    updateHook(index, hook.type === "http" ? { url: next } : { command: next });
  };

  // ---- 请求头 ---------------------------------------------------------------
  //
  // 和钩子目标一样是「草稿 + 失焦提交」：每敲一个字存一次会把半截头名写进配置，
  // 而半截的头名（`Authoriz`）是一个**真的会被发出去**的请求头 —— 比半截 URL
  // 更糟，因为那个只是发不出去。
  //
  // 这里只留草稿**怎么来、往哪去**；「能不能提交」那几条规则都在 `lib/hookHeaders.ts`
  // 里（改名 = 删旧插新、头名不区分大小写、空名字 = 删行），有单测。
  // `from` 是这一行的**原键名**，新加的一行是 `null`。
  const [headerDraft, setHeaderDraft] = useState<{
    index: number;
    from: string | null;
    name: string;
    value: string;
  } | null>(null);

  const headersOf = (index: number): Record<string, string> => {
    const hook = settings.hooks[index];
    return hook?.type === "http" ? hook.headers : {};
  };

  const setHeaders = (index: number, headers: Record<string, string>) =>
    updateHook(index, { headers });

  const addHeader = (index: number) => {
    setError(null);
    // 已经有一行新行杵在那儿了就别再加第二行 —— 两行空名字谁也分不清哪行是哪个
    if (headerDraft?.index === index && headerDraft.from === null) return;
    setHeaderDraft({ index, from: null, name: "", value: "" });
  };

  const commitHeaderDraft = () => {
    if (!headerDraft) return;
    const { index, from, name, value } = headerDraft;
    setHeaderDraft(null);
    const result = commitHeader(headersOf(index), from, { name, value });
    // 撞名：报一句，**保持原样** —— 悄悄丢掉用户刚敲的那个头名比报错更糟
    if (!result) {
      setError(m.headerDuplicate);
      return;
    }
    // `changed: false`（名字空着的新行、或者什么都没改）就不必白跑一趟 set_settings
    if (result.changed) setHeaders(index, result.headers);
  };

  // 「发送测试」：让 Rust 把所有已配置的转发各发一遍（忽略 when / 组过滤 / 跃迁），
  // 再立刻重取日志。就地显示「发出几条」——「点了什么都不显示」和「真的没配转发」
  // 从外面看是一样的。
  const [testNotice, setTestNotice] = useState<string | null>(null);

  const sendTest = () => {
    setError(null);
    invoke<number>("test_hooks")
      .then((sent) => {
        setTestNotice(sent === 0 ? m.testNothing : m.testSent(sent));
        setOutboundNonce((nonce) => nonce + 1);
      })
      .catch(reported);
  };

  const setPaused = (paused: boolean) => {
    setError(null);
    invoke("set_paused", { paused }).catch(reported);
  };

  // 示例里**填真实的端口和 token**，而不是 `$(cat ~/.signalhub/token)`：
  // 后者在 Windows 的 PowerShell 里跑不了，而这一页的主要读者是「想粘一条命令
  // 试一下」的对接方。代价是 token 会进 shell 历史 —— 它是本机接口的凭据，
  // 能读到它的本来就只有这台机器上的人。
  const curlExample = endpoint?.port
    ? [
        `curl -s -X POST "http://127.0.0.1:${endpoint.port}/v1/state" \\`,
        `  -H "Authorization: Bearer ${endpoint.token ?? "<token>"}" \\`,
        `  -H 'Content-Type: application/json' \\`,
        `  -d '{"state":"working","session":"demo","message":"hi"}'`,
      ].join("\n")
    : null;

  /**
   * 只读 Web 面板的完整链接（含 token）。
   *
   * **地址取的是「实际绑到哪了」**（`bound_lan`）而不是开关：用户刚打开局域网
   * 访问、还没重启时，那个开关是 `true` 而服务仍绑在回环 —— 这时给一个
   * `http://192.168.x.x:…` 的链接是**给不出去的东西**（手机上打不开）。宁可先给
   * 回环那条（本机能用），旁边那句「重启才生效」会告诉他为什么。
   *
   * token 放 query 而不是 header：这是给人**点开**的链接，浏览器里只有 URL 可带。
   */
  const panelUrl =
    endpoint?.port && endpoint.token
      ? `http://${
          endpoint.bound_lan && endpoint.lan_ip ? endpoint.lan_ip : "127.0.0.1"
        }:${endpoint.port}/?t=${endpoint.token}`
      : null;

  // 复制反馈。1.5 秒后自动收回，否则用户不知道刚才那下有没有生效。
  const [copied, setCopied] = useState<string | null>(null);
  const copy = (what: string, text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(what);
        window.setTimeout(() => setCopied(null), 1500);
      })
      .catch(reported);
  };
  const copyLabel = (what: string) => (copied === what ? m.copied : m.copy);

  /**
   * 复制诊断信息。**先取再复制**：那段文本是现拼的（要读配置文件、日志、回执），
   * 所以不能像别的复制按钮那样把字面量直接递给 `copy`。
   *
   * 不带 `agent`：一次给全部。用户问「opencode 怎么接不上」时，贴出去的这段里
   * 也带着别的信号源的状态 —— 那恰好能区分「只有它不行」和「全都不行」。
   */
  const copyDiagnosis = () => {
    setError(null);
    invoke<string>("diagnose", { agent: null })
      .then((text) => copy("diag", text))
      .catch(reported);
  };

  // 每个页签的内容。写成函数而不是子组件，是为了免去把十几个 prop 一路传下去。
  const page = () => {
    switch (tab) {
      case "history":
        return (
          <div className="section">
            <div className="row">
              <span>{m.historyTitle}</span>
              <button type="button" className="btn btn-tiny" onClick={refreshHistory}>
                {m.diagRefresh}
              </button>
            </div>
            <p className="hint">{m.historyHint}</p>
            {historyDays.length === 0 ? (
              <p className="empty">{m.historyEmpty}</p>
            ) : (
              <HistoryList days={historyDays} m={m} />
            )}
          </div>
        );

      case "status":
        return (
          <>
            <TimelineStrip segments={timeline} m={m} lang={lang} />

            {/* 这一页只回答「现在发生了什么」。改显示名、置顶、隐藏、静音这些都是
                **写入偏好**，和这里只读的实时状态刷新频率、心智模型都不同 ——
                它们搬去了「外观」页的「各组的显示」。 */}
            {snapshot.groups.length === 0 ? (
              <div className="section">
                <p className="empty">{m.emptySessions}</p>
                {/* 空态是「第一次用」最可能撞上的地方，所以手册的入口放在这里 ——
                    只挂在左栏里等于等人自己去找。 */}
                <div className="row">
                  <span>{m.guideEntry}</span>
                  <button type="button" className="btn" onClick={() => setTab("guide")}>
                    {m.tabGuide}
                  </button>
                </div>
              </div>
            ) : (
              snapshot.groups.map((group) => {
                const hidden = settings.hidden_groups.includes(group.name);
                const label = labelOf(group, settings.group_meta);
                return (
                  <div className="group" key={group.name} data-hidden={hidden}>
                    <div className="group-head" data-state={group.aggregate}>
                      <span className="state-dot" />
                      <span className="group-name" title={group.name}>
                        {label}
                      </span>
                      {/* 显示名和原始组名不一样时就把原始组名也标出来 —— 用户得知道
                          它对应的是哪一路上报，否则灯上那个名字到底是谁就说不清了。
                          两种情况下会不一样：用户自己改过名，或者这是按会话分组
                          自动起的默认名（原始名是个会话 id）。 */}
                      {label !== group.name && (
                        <span className="group-raw" title={group.name}>
                          {group.name}
                        </span>
                      )}
                      <span className="group-meta">
                        {signalLabel(lang, group.aggregate)} ·{" "}
                        {m.sessions(group.sessions.length)}
                      </span>
                    </div>
                    <ul className="sessions">
                      {group.sessions.map((session) => (
                        <li
                          key={session.id}
                          className="session"
                          data-state={session.state}
                        >
                          <span className="state-dot" />
                          {/* 窄面板里挤成一行会把 id 压没，所以状态/来源单独放上行 */}
                          <div className="session-body">
                            <div className="session-line">
                              <span className="id mono">{session.id}</span>
                              <span className="meta">
                                {signalLabel(lang, session.state)}
                                {session.agent ? ` · ${session.agent}` : ""}
                              </span>
                            </div>
                            {session.message && (
                              <div className="message">{session.message}</div>
                            )}
                          </div>
                          {/* 会话粒度的清除。比牌上那枚细一档：一组里跑着三个会话、
                              卡住一个时，不必把另外两个也清掉。 */}
                          <button
                            type="button"
                            className="btn btn-tiny session-clear"
                            title={m.clearSessionHint}
                            onClick={() => clearSession(session.id)}
                          >
                            ×
                          </button>
                        </li>
                      ))}
                    </ul>
                  </div>
                );
              })
            )}

            {/* 最近消息：灯上只有每个会话**当前**那条，这一节回答「刚才发生了什么」。
                放在最后 —— 它是回顾，不如上面的实时状态要紧。 */}
            <div className="section">
              <div className="section-title">{m.feedTitle}</div>
              {feed.length === 0 ? (
                <p className="muted-text">{m.feedEmpty}</p>
              ) : (
                <ul className="feed">
                  {feed.slice(0, FEED_LIMIT).map((entry, index) => (
                    <li className="feed-item" key={`${entry.at_ms}-${index}`}>
                      <span className="state-dot" data-state={entry.state} />
                      <div className="feed-body">
                        <div className="feed-line">
                          <span className="muted-text">{formatAge(entry.at_ms, now)}</span>
                          <span className="muted-text">{entry.group}</span>
                          <span className="feed-session mono">{entry.session}</span>
                        </div>
                        <div className="feed-message">{entry.message}</div>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </>
        );

      // 「灯怎么动」——这些是**行为**，不是外观：安静模式怎么压暗、拖到边上吸不吸、
      // 鼠标能不能点到它、它在别的窗口上面还是下面、托盘图标跟不跟状态走。
      //
      // 它们原先一半在「外观」、一半在「系统」，而「安静模式」和「点击穿透」是同层
      // 概念（都决定鼠标碰到灯时发生什么）却被拆到两页 —— 想找「点击穿透」的人不会
      // 想到去「系统」里翻。
      case "behavior":
        return (
          <>
            <Row label={m.quietMode} onReset={resetFor(settings, "quiet", patch)}>
              <Switch on={settings.quiet} onChange={(on) => patch({ quiet: on })} />
            </Row>
            <Row
              label={m.snapToEdge}
              hint={m.snapToEdgeHint}
              onReset={resetFor(settings, "snap_to_edge", patch)}
            >
              <Switch on={settings.snap_to_edge} onChange={(on) => patch({ snap_to_edge: on })} />
            </Row>
            <Row label={m.clickThrough}>
              <Switch
                on={toggles.pass_through}
                onChange={(on) => {
                  setError(null);
                  setPassThrough(on).catch(reported);
                }}
              />
            </Row>
            <Row label={m.alwaysOnTop} onReset={resetFor(settings, "always_on_top", patch)}>
              <Switch
                on={settings.always_on_top}
                onChange={(on) => patch({ always_on_top: on })}
              />
            </Row>
            <Row label={m.trayFollowsState} onReset={resetFor(settings, "tray_follows_state", patch)}>
              <Switch
                on={settings.tray_follows_state}
                onChange={(on) => patch({ tray_follows_state: on })}
              />
            </Row>
          </>
        );

      case "look":
        return (
          <>
            {/* 预览放最上面：这一页改的就是它。放在最底下的话，调完「牌宽」
                还得往下滚一次才知道改成了什么。 */}
            <div className="section-title">{m.previewTitle}</div>
            <LampPreview settings={settings} />
            <p className="hint">{m.previewHint}</p>
            <Row label={m.presets}>
              {/* 同上：一组里选一个，所以是 seg 不是 chip。 */}
              <div className="seg">
                {presets(m).map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    title={preset.hint}
                    data-active={isActivePreset(preset, settings)}
                    onClick={() => patch(preset.apply)}
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
            </Row>
            <Row label={m.form} onReset={resetFor(settings, "collapsed", patch)}>
              <div className="seg">
                <button
                  type="button"
                  data-active={!settings.collapsed}
                  onClick={() => patch({ collapsed: false })}
                >
                  {m.expanded}
                </button>
                <button
                  type="button"
                  data-active={settings.collapsed}
                  onClick={() => patch({ collapsed: true })}
                >
                  {m.singleLight}
                </button>
              </div>
            </Row>
            <Row label={m.size} onReset={resetFor(settings, "size", patch)}>
              <div className="seg">
                {sizes.map((size) => (
                  <button
                    key={size.value}
                    type="button"
                    data-active={settings.size === size.value}
                    onClick={() => patch({ size: size.value })}
                  >
                    {size.label}
                  </button>
                ))}
              </div>
            </Row>
            <Row
              label={m.shape}
              hint={m.lookHint}
              onReset={resetFor(settings, "shape", patch)}
            >
              <div className="seg">
                {shapes.map((shape) => (
                  <button
                    key={shape.value}
                    type="button"
                    data-active={settings.shape === shape.value}
                    onClick={() => patch({ shape: shape.value })}
                  >
                    {shape.label}
                  </button>
                ))}
              </div>
            </Row>
            {/* 配色方案和形状、节奏倍率是一类东西：都是「没被逐个状态覆盖时用哪个」。
                所以放这一页，逐状态的覆盖在「灯效」页。 */}
            <Row label={m.palette} onReset={resetFor(settings, "palette", patch)}>
              <div className="seg">
                {palettes.map((palette) => (
                  <button
                    key={palette.value}
                    type="button"
                    data-active={settings.palette === palette.value}
                    onClick={() => patch({ palette: palette.value })}
                  >
                    {palette.label}
                  </button>
                ))}
              </div>
            </Row>
            <Row
              label={m.signOpacity}
              hint={signTextInverted(settings.sign_opacity) ? m.signOpacityHint : undefined}
              onReset={resetFor(settings, "sign_opacity", patch)}
            >
              <Slider
                value={settings.sign_opacity}
                min={0}
                max={100}
                onChange={(sign_opacity) => patch({ sign_opacity })}
                format={(value) => `${value}%`}
              />
            </Row>
            {/* 长宽倍率：只放大 / 缩小**展开态**的牌本身（字号、点径、间距仍由
                「大小」决定）。窗口尺寸照旧按内容算，所以这两个滑杆不会把窗口
                拖成一块空白。折叠态是一排正方形点，不受影响。 */}
            <Row label={m.signWidth} onReset={resetFor(settings, "sign_w_mult", patch)}>
              <Slider
                value={settings.sign_w_mult}
                min={SIGN_MULT_RANGE.min}
                max={SIGN_MULT_RANGE.max}
                step={0.1}
                onChange={(sign_w_mult) => patch({ sign_w_mult })}
                format={(value) => `×${value.toFixed(1)}`}
              />
            </Row>
            <Row label={m.signHeight} onReset={resetFor(settings, "sign_h_mult", patch)}>
              <Slider
                value={settings.sign_h_mult}
                min={SIGN_MULT_RANGE.min}
                max={SIGN_MULT_RANGE.max}
                step={0.1}
                onChange={(sign_h_mult) => patch({ sign_h_mult })}
                format={(value) => `×${value.toFixed(1)}`}
              />
            </Row>
            <Row label={m.blinkSpeed} onReset={resetFor(settings, "blink_speed", patch)}>
              <Slider
                value={settings.blink_speed}
                min={BLINK_RANGE.min}
                max={BLINK_RANGE.max}
                onChange={(blink_speed) => patch({ blink_speed })}
                format={(value) => `${value}%`}
              />
            </Row>
            <Row label={m.onSign} onReset={resetFor(settings, "show", patch)}>
              <div className="chips chips-tight">
                {fields.map((field) => (
                  <button
                    key={field.key}
                    type="button"
                    className="chip"
                    data-active={settings.show[field.key]}
                    onClick={() => setField(field.key, !settings.show[field.key])}
                  >
                    {field.label}
                  </button>
                ))}
              </div>
            </Row>
            {/* 逐状态覆盖和上面的全局项是**两层**：上面是「没被覆盖时用哪个」，
                这里是「某个状态单独长什么样」。默认收起 —— 十行表和上面那九行
                不是一个量级，全摊开会把这一页淹掉，而它回答的其实是「外观」的
                一个子问题。收起时留一行摘要，免得让人以为这里什么都没有。 */}
            <button
              type="button"
              className="disclosure"
              aria-expanded={perStateOpen}
              onClick={() => setPerStateOpen((open) => !open)}
            >
              <span className="caret" aria-hidden="true">
                {perStateOpen ? "▾" : "▸"}
              </span>
              <span className="disclosure-title">{m.perStateStyle}</span>
              <span className="disclosure-note">
                {perStateCount > 0 ? m.perStateSome(perStateCount) : m.perStateNone}
              </span>
            </button>
            {perStateOpen && (
              <PerStateStyles
                settings={settings}
                patch={patch}
                m={m}
                lang={lang}
                shapes={shapes}
                blinkChoices={blinkChoices}
                ttlDefaults={ttlDefaults}
              />
            )}
          </>
        );

      // 各组的显示：显示名 / 置顶 / 隐藏 / 静音。**独立成页**是因为它是整个面板里
      // 唯一**随会话数变长**的一节（按会话分组时一个会话一行），而「灯长什么样」
      // 是固定的九行 —— 混在一起，后者会被前者一点点推走。
      case "groups":
        return (
          <>
            <div className="section section-flush">
              <div className="section-title">{m.groupDisplayTitle}</div>
              {knownGroups.length === 0 ? (
                <p className="muted-text">{m.groupDisplayEmpty}</p>
              ) : (
                <>
                  {(settings.hidden_groups.length > 0 ||
                    settings.muted_groups.length > 0) && (
                    <div className="row hidden-summary">
                      <span className="muted-text">
                        {settings.hidden_groups.length > 0 &&
                          m.hiddenOnLamp(settings.hidden_groups.length)}
                        {settings.hidden_groups.length > 0 &&
                          settings.muted_groups.length > 0 &&
                          " · "}
                        {settings.muted_groups.length > 0 &&
                          m.mutedGroups(settings.muted_groups.length)}
                      </span>
                      <button
                        type="button"
                        className="btn btn-tiny"
                        onClick={() =>
                          patch({ hidden_groups: [], muted_groups: [] })
                        }
                      >
                        {m.restoreAll}
                      </button>
                    </div>
                  )}

                  {knownGroups.map((name) => {
                    const meta = settings.group_meta[name];
                    // 这一组在灯上实际会显示成什么 —— 用户得先看见它，才知道自己在改什么
                    const label = meta?.name ?? defaultLabels.get(name) ?? name;
                    return (
                      <div className="group" key={name}>
                        <div className="group-head">
                          <span className="group-name" title={name}>
                            {label}
                          </span>
                          {/* 显示名和原始组名不一样时就把原始组名标出来，理由同状态页 */}
                          {label !== name && (
                            <span className="group-raw" title={name}>
                              {name}
                            </span>
                          )}
                        </div>
                        <div className="group-edit">
                          <input
                            className="input input-tiny"
                            spellCheck={false}
                            placeholder={m.displayNamePlaceholder(label)}
                            value={
                              nameDraft?.group === name
                                ? nameDraft.value
                                : (meta?.name ?? "")
                            }
                            /* 聚焦时把**此刻显示的名字**填进框里。它就在右边的灰色
                               占位里明摆着，不填的话用户得照着它重敲一遍。填进来的
                               这个值同时充当「动没动过」的判据（`nameChanged`）。 */
                            onFocus={() =>
                              setNameDraft((draft) =>
                                draft?.group === name
                                  ? draft
                                  : { group: name, value: label, seed: label },
                              )
                            }
                            onChange={(event) =>
                              setNameDraft((draft) => ({
                                group: name,
                                value: event.target.value,
                                seed: draft?.group === name ? draft.seed : label,
                              }))
                            }
                            onBlur={commitNameDraft}
                            onKeyDown={(event) => {
                              if (event.key === "Enter") event.currentTarget.blur();
                            }}
                          />
                          <label className="pin" title={m.pinHint}>
                            <span className="muted-text">{m.pin}</span>
                            <Switch
                              on={meta?.order != null}
                              onChange={(on) => setPinned(name, on)}
                            />
                          </label>
                          {/* 「隐藏」和「静音」各是一个开关 —— 原先它们在状态页上是
                              两个「点一下恢复」的徽标加一个「静音」按钮，三种控件
                              表达两个布尔量。 */}
                          <label className="pin" title={m.hiddenBadgeHint}>
                            <span className="muted-text">{m.hidden}</span>
                            <Switch
                              on={settings.hidden_groups.includes(name)}
                              onChange={(on) => setHidden(name, on)}
                            />
                          </label>
                          <label className="pin" title={m.muteHint}>
                            <span className="muted-text">{m.muted}</span>
                            <Switch
                              on={settings.muted_groups.includes(name)}
                              onChange={(on) => setMuted(name, on)}
                            />
                          </label>
                        </div>
                      </div>
                    );
                  })}
                </>
              )}
            </div>
          </>
        );

      case "alert": {
        // 阈值给一列固定档位而不是滑块：这是个粗选择（「立刻」还是「等一会儿」），
        // 秒级的精调没有意义。配置文件里手写的不在表里的值会补一个选项进来。
        const extraDelay = alertChoices.some((d) => d.value === settings.alert_after_secs)
          ? null
          : settings.alert_after_secs;

        return (
          <>
            <Row label={m.notifyWhenNeeded} onReset={resetFor(settings, "notify", patch)}>
              <Switch on={settings.notify} onChange={(on) => patch({ notify: on })} />
            </Row>
            <Row label={m.soundWithNotify} onReset={resetFor(settings, "sound", patch)}>
              <Switch on={settings.sound} onChange={(on) => patch({ sound: on })} />
            </Row>

            {/* 通知关着的时候不显示权限状态 —— 那时候它不说明任何问题 */}
            {settings.notify && <NotifyPermission m={m} enabled={settings.notify} />}

            <Row label={m.alertAfter} onReset={resetFor(settings, "alert_after_secs", patch)}>
              <select
                className="select"
                title={m.alertAfterHint}
                value={settings.alert_after_secs}
                onChange={(event) =>
                  patch({ alert_after_secs: Number(event.target.value) })
                }
              >
                {alertChoices.map((delay) => (
                  <option key={delay.value} value={delay.value}>
                    {delay.label}
                  </option>
                ))}
                {extraDelay !== null && (
                  <option value={extraDelay}>{m.seconds(extraDelay)}</option>
                )}
              </select>
            </Row>
            <Row label={m.quietHours} onReset={resetFor(settings, "quiet_hours", patch)}>
              <div className="hours">
                <Switch
                  on={settings.quiet_hours.enabled}
                  onChange={(on) =>
                    patch({ quiet_hours: { ...settings.quiet_hours, enabled: on } })
                  }
                />
                <select
                  className="select"
                  disabled={!settings.quiet_hours.enabled}
                  value={settings.quiet_hours.start}
                  onChange={(event) =>
                    patch({
                      quiet_hours: {
                        ...settings.quiet_hours,
                        start: Number(event.target.value),
                      },
                    })
                  }
                >
                  {HOURS.map((hour) => (
                    <option key={hour} value={hour}>
                      {String(hour).padStart(2, "0")}:00
                    </option>
                  ))}
                </select>
                <span className="muted-text">→</span>
                <select
                  className="select"
                  disabled={!settings.quiet_hours.enabled}
                  value={settings.quiet_hours.end}
                  onChange={(event) =>
                    patch({
                      quiet_hours: {
                        ...settings.quiet_hours,
                        end: Number(event.target.value),
                      },
                    })
                  }
                >
                  {HOURS.map((hour) => (
                    <option key={hour} value={hour}>
                      {String(hour).padStart(2, "0")}:00
                    </option>
                  ))}
                </select>
              </div>
            </Row>
            <p className="hint">{m.alertHint}</p>
            {/* 想去「转发到服务器」的人多半先来提醒页找，给一条通往「通知转发」的路 */}
            <div className="row">
              <span className="muted-text">{m.alertForwardHint}</span>
              <button
                type="button"
                className="btn btn-tiny"
                onClick={() => setTab("hooks")}
              >
                {m.configure}
              </button>
            </div>
          </>
        );
      }

      case "connect": {
        // 已接入的排最前，其次检测到的，最后没检测到的 —— 有用的在上面。
        // 同组内保持后端给的顺序（Array.sort 是稳定的）。
        const agentRank = (agent: AgentStatus) =>
          agent.installed ? 0 : agent.detected ? 1 : 2;
        const sortedAgents = [...agents].sort(
          (a, b) => agentRank(a) - agentRank(b),
        );
        // 「回执」按信号源名字查：上报方给的名字（`claude-code`）和清单里的 id
        // 是同一套拼写。
        const receiptFor = (id: string) => receipts.find((r) => r.agent === id);
        // 收到过上报、但不在上面清单里的信号源，**不能扔掉**：用户自己的脚本、
        // 另一台机器、手写的一条 curl 都在这一类里，而它们恰恰证明上报通道是通的。
        const knownIds = new Set(agents.map((a) => a.id));
        // 非智能体的信号源按**上报方声明**的那一类分两组：系统 / 自定义。见
        // `groupReceipts` —— 判定放在那边是为了能单测，而「是不是我们认识的智能体」
        // 仍由上面那张清单决定。
        const { system: systemSources, custom: otherSources } = groupReceipts(
          receipts,
          knownIds,
        );

        return (
          <>
            {/* ---- 智能体接入 ----
                这一页现在只做一件事：把智能体接上。接口速查、状态词表那些「查
                细节」的内容搬去了「指南」—— 它们是给已经接好的人读的，混在这里
                会把「点哪个按钮」淹没掉（原来这一页有两倍于可视区的内容）。 */}
            <div className="section">
              <div className="section-title">{m.agentsTitle}</div>
              <p className="hint">{m.agentsHint}</p>
              {sortedAgents.map((agent) => {
                const busy = busyAgent === agent.id;
                const change = agentChanges[agent.id];
                const receipt = receiptFor(agent.id);
                const status = agent.installed
                  ? m.agentInstalled
                  : agent.detected
                    ? m.agentDetected
                    : m.agentMissing;
                return (
                  <div className="agent" key={agent.id}>
                    <div className="agent-info">
                      <div className="agent-line">
                        <span className="agent-name">{agent.name}</span>
                        <span className="badge" data-on={agent.installed}>
                          {status}
                        </span>
                      </div>
                      {/* 路径就是「要写哪儿」，摆出来这条操作才可信 */}
                      {agent.config && (
                        <span className="agent-config mono">{agent.config}</span>
                      )}
                      {/* 装了、但配置里那条命令现在跑不起来了 —— 最该主动指出来的
                          一种「装了却没反应」：智能体执行一个不存在的程序**不会报错**，
                          灯也不会亮，两边都很安静。命令原文也摆出来，用户能自己看出
                          它指向哪（多半是个已经被挪走的旧路径）。 */}
                      {agent.installed && !agent.command_ok && agent.http !== true && (
                        <span className="agent-broken">
                          {m.agentCommandBroken}
                          {agent.command && <code className="mono">{agent.command}</code>}
                        </span>
                      )}
                      {change && (
                        <span className="agent-result muted-text">
                          {/* 插件式接入：说「写到哪个文件」而不是「几个事件」——
                              插件没有事件清单，那个数字对不上任何东西。 */}
                          {change.plugin
                            ? agent.installed
                              ? m.hookPluginWritten(change.plugin)
                              : m.hookPluginRemoved(change.plugin)
                            : m.hookResult({
                                added: change.added.length,
                                updated: change.updated.length,
                                already: change.already.length,
                                removed: change.removed_handlers,
                              })}
                        </span>
                      )}
                      {/* **「它到底有没有在报」—— 这一页此前唯一缺的东西。**
                          装好和装坏以前长得一模一样（都只是「配置里写着我们的
                          命令」），而智能体执行一条不存在的命令是**不会报错**的。
                          有回执才是真的通了。 */}
                      {agent.installed && (
                        <span className="agent-receipt" data-on={!!receipt}>
                          {receipt
                            ? `${m.agentReceiptAgo(formatAge(receipt.last_epoch_ms, now), signalText(m, receipt.state))} · ${m.agentReceiptCount(receipt.count)}`
                            : m.agentReceiptNever}
                        </span>
                      )}
                      {/* **在跑哪条路。** 两种模式写进同一个配置文件，而它们的行为
                          不同（HTTP 式不怕 PATH 问题、但开局域网时会关掉）—— 用户
                          看得见才谈得上换一种。只有 Claude Code 有两种可选。 */}
                      {agent.id === "claude-code" && agent.installed && (
                        <span className="agent-result muted-text">
                          {agent.http ? m.agentModeHttp : m.agentModeCommand}
                        </span>
                      )}
                      {/* 「试一下」的结果：和回执是两个问题 —— 回执问「最近有没有在报」，
                          这一行答「现在这条链路还通不通」。 */}
                      {probeResults[agent.id] && (
                        <span
                          className="agent-result"
                          data-on={probeResults[agent.id].ran}
                        >
                          {probeLine(m, probeResults[agent.id])}
                        </span>
                      )}
                    </div>
                    <div className="agent-buttons">
                      {/* 「试一下」只在已接入时有意义：没接入就没有那条命令可跑。 */}
                      {agent.installed && (
                        <button
                          type="button"
                          className="btn btn-tiny"
                          disabled={busy}
                          onClick={() => tryAgent(agent)}
                        >
                          {busy ? m.agentTryBusy : m.agentTry}
                        </button>
                      )}
                      {/* **HTTP 方式**（只有 Claude Code）。放在「接入」左边：
                          它是同一个动作的另一条路，而不是另一个功能。已接入时它
                          变成「换一种写法」—— 两种模式共用一个配置，换过去是原地
                          改写，不需要先卸载。 */}
                      {agent.id === "claude-code" && (
                        <button
                          type="button"
                          className="btn btn-tiny"
                          disabled={busy}
                          onClick={() =>
                            !agent.installed || !agent.http
                              ? changeAgentHttp(agent)
                              : changeAgent(agent, true)
                          }
                        >
                          {!agent.installed
                            ? m.agentConnectHttp
                            : agent.http
                              ? m.agentSwitchCommand
                              : m.agentSwitchHttp}
                        </button>
                      )}
                      <button
                        type="button"
                        className="btn btn-tiny"
                        disabled={busy}
                        onClick={() => changeAgent(agent, !agent.installed)}
                      >
                        {agent.installed ? m.agentDisconnect : m.agentConnect}
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>

            {/* ---- 系统信号源 ----
                 这一节**空着也显示**：它是「信号源不只有智能体」这个概念唯一的
                 落点，藏起来的话，用户在采集器做出来之前根本不知道有这条路。
                 而且它顺手把用法写出来了 —— 一句话就能把自己的脚本报进来。 */}
            <div className="section">
              <div className="section-title">{m.systemSourcesTitle}</div>
              <p className="hint">{m.systemSourcesHint}</p>
              {systemSources.length === 0 ? (
                <p className="hint">{m.systemSourcesEmpty}</p>
              ) : (
                systemSources.map((source) => (
                  <div className="agent" key={source.agent}>
                    <div className="agent-info">
                      <div className="agent-line">
                        <span className="agent-name mono">{source.agent}</span>
                      </div>
                      <span className="agent-receipt" data-on>
                        {m.agentReceiptAgo(
                          formatAge(source.last_epoch_ms, now),
                          signalText(m, source.state),
                        )}{" "}
                        · {m.agentReceiptCount(source.count)}
                      </span>
                    </div>
                  </div>
                ))
              )}
            </div>

            {/* ---- 收到过上报、但不在清单里的信号源 ----
                单独一节而不是并进上面的列表：那一排每一行都有「接入 / 卸载」
                按钮，而这一类我们无从接入（它不是我们认识的智能体）——混进去
                会让人以为按钮没渲染出来。它有它自己的价值：**它是「上报通道是通的」
                的活证据**，用户自己写的脚本、另一台机器、手抄的一条 curl 都在这里。 */}
            {otherSources.length > 0 && (
              <div className="section">
                <div className="section-title">{m.otherSourcesTitle}</div>
                <p className="hint">{m.otherSourcesHint}</p>
                {otherSources.map((source) => (
                  <div className="agent" key={source.agent}>
                    <div className="agent-info">
                      <div className="agent-line">
                        <span className="agent-name mono">{source.agent}</span>
                      </div>
                      <span className="agent-receipt" data-on>
                        {m.agentReceiptAgo(
                          formatAge(source.last_epoch_ms, now),
                          signalText(m, source.state),
                        )}{" "}
                        · {m.agentReceiptCount(source.count)}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )}

            {/* ---- 只用命令行的智能体 ----
                单独一节、不混进上面那个列表：那一排每一行都有「接入 / 卸载」
                按钮，而这一类没有能写 hook 的配置文件，也就没有可接入、可卸载的
                东西。混进去会让人以为按钮没渲染出来。 */}
            <div className="section">
              <div className="section-title">{m.cliAgentsTitle}</div>
              <p className="hint">{m.cliAgentsHint}</p>
              {/* 「包一条命令」放在 Pi Agent 上面：它是**通用**那条，而 Pi Agent 是
                  具体的一个例子。用户先看到「任何工具都能这么接」，再看到例子，
                  顺序才顺。 */}
              <div className="agent">
                <div className="agent-info">
                  <div className="agent-line">
                    <span className="agent-name">{m.cliAgentsWrap}</span>
                  </div>
                  <span className="agent-config mono">{WRAP_COMMAND}</span>
                </div>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => copy("wrap", WRAP_COMMAND)}
                >
                  {copyLabel("wrap")}
                </button>
              </div>
              {/* 第二行不点名具体的智能体：点名的那家迟早会有一键接入（Pi 就是
                  这么从这一节毕业的），而那句代码抄过去就对不上了。 */}
              <div className="agent">
                <div className="agent-info">
                  <div className="agent-line">
                    <span className="agent-name">{m.cliAgentsHookName}</span>
                  </div>
                  <span className="agent-config mono">{HOOK_AGENT_COMMAND}</span>
                </div>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => copy("hooks", HOOK_AGENT_COMMAND)}
                >
                  {copyLabel("hooks")}
                </button>
              </div>
              <p className="hint">{m.cliAgentsHint}</p>
            </div>

            {/* ---- 一键诊断 ----
                放在智能体列表之后、试一下之前：上面两节是「怎么接」和「接完怎么看」，
                这一节回答的是「接不上时拿什么去问」——把散在三个页签里的东西一次
                拼成一段可以复制出去的文本。 */}
            <div className="section">
              <div className="row">
                <span>{m.diagnoseCopy}</span>
                <button type="button" className="btn btn-tiny" onClick={copyDiagnosis}>
                  {copyLabel("diag")}
                </button>
              </div>
              <p className="hint">{m.diagnoseHint}</p>
            </div>

            {/* ---- 试一下 ----
                放在「配好之后怎么验证」的位置：上面是「怎么接」，下面是「接完怎么
                分」，而这一节回答的是两者之间那个问题 —— 灯到底通没通。

                它跑的是和 HTTP 上报完全相同的 Store 链路，所以看到灯亮了就说明
                聚合、窗口尺寸、推送这几段都是活的。**但 HTTP 那一层它测不到**，
                所以说明里明确指向「指南」页那条 curl，免得有人以为这就等于测通了。 */}
            <div className="section">
              <div className="section-title">{m.tryItTitle}</div>
              <div className="row-start">
                {TRY_STATES.map((state) => (
                  <button
                    key={state}
                    type="button"
                    className="btn btn-tiny"
                    onClick={() => {
                      setError(null);
                      invoke("send_test_report", { state }).catch(reported);
                    }}
                  >
                    {signalLabel(lang, state)}
                  </button>
                ))}
              </div>
              <p className="hint">{m.tryItHint}</p>
            </div>

            {/* ---- 分组 ----
                放在「接入」页而不是「外观」：它管的是**上报进来之后怎么解读**，
                和「谁在往这儿报」是同一件事的两半。外观页管的是已经分好组之后
                怎么称呼、怎么排。

                三个取值用 `seg` 而不是开关：从「按目录」到「按会话」不是「开/
                关」，而开关表达不了「换一种分法」。 */}
            <div className="section">
              <div className="row">
                <span>{m.groupBy}</span>
                <div className="seg">
                  {(["none", "cwd", "session"] as const).map((mode) => (
                    <button
                      key={mode}
                      type="button"
                      data-active={settings.group_by === mode}
                      onClick={() => patch({ group_by: mode })}
                    >
                      {mode === "none"
                        ? m.groupByNone
                        : mode === "cwd"
                          ? m.groupByCwd
                          : m.groupBySession}
                    </button>
                  ))}
                </div>
              </div>
              <p className="hint">{m.groupByHint}</p>
            </div>

            {/* ---- 灯的数量 ----
                紧挨着「分组」：分组定了「有哪些组」，这一项定的就是「这些组摆成
                一盏灯还是一组一盏灯」，是同一个决定的下半句。

                两项用 `seg` 而不是开关：不是「开/关多灯」，而是「换一种摆法」。
                「每组分灯」在组数超过 `MAX_LAMPS` 时会**自动回落**成一盏灯
                （每盏灯都是一个真 webview，Linux 上尤其贵），提示里写明。 */}
            <div className="section">
              <div className="row">
                <span>{m.lampLayout}</span>
                <div className="seg">
                  <button
                    type="button"
                    data-active={settings.lamp_layout === "single"}
                    onClick={() => patch({ lamp_layout: "single" })}
                  >
                    {m.layoutSingle}
                  </button>
                  <button
                    type="button"
                    data-active={settings.lamp_layout === "per-group"}
                    onClick={() => patch({ lamp_layout: "per-group" })}
                  >
                    {m.layoutPerGroup}
                  </button>
                </div>
              </div>
              <p className="hint">{m.layoutHint}</p>
            </div>

            {/* ---- 局域网访问 ----
                和「分组」同页、紧挨着它：两者都是「上报进来这一侧」的事 ——
                一个管怎么解读，一个管从哪能报进来。

                独立一节而不再缩进一层：这是一个**改变安全模型**的开关，混在
                别的行里会被划过去。说明分成两段也是这个原因 —— 代价那一段得
                单独成句，不能藏在操作说明的尾巴上。

                「重启才生效」只在**真的不一致**时才说：`bound_lan` 是服务实际
                绑到哪了，无条件挂一句会让人以为一直没生效。 */}
            <div className="section">
              <div className="row">
                <span>{m.lanAccess}</span>
                <Switch
                  on={settings.lan_access}
                  onChange={(on) => patch({ lan_access: on })}
                />
              </div>
              {endpoint !== null && endpoint.bound_lan !== settings.lan_access && (
                <p className="hint">{m.lanRestart}</p>
              )}
              {settings.lan_access && endpoint?.lan_ip && (
                <div className="kv">
                  <span className="k">{m.lanAddress}</span>
                  <span className="mono">{`http://${endpoint.lan_ip}:${endpoint.port ?? "—"}`}</span>
                  <button
                    type="button"
                    className="btn btn-tiny kv-copy"
                    onClick={() =>
                      copy("lan", `http://${endpoint.lan_ip}:${endpoint.port ?? ""}`)
                    }
                  >
                    {copyLabel("lan")}
                  </button>
                </div>
              )}
              <p className="hint">{m.lanAccessHint}</p>
            </div>

            {/* ---- 只读 Web 面板 ----
                紧挨着「局域网访问」：这两个开关合起来才决定「手机上能不能看到灯」
                —— 一个管从哪能连进来，一个管那边能看到什么。分成两节也是因为
                它们是**两件事**：面板开着、但只绑回环时，就只有本机能看。 */}
            <div className="section">
              <div className="row">
                <span>{m.webPanel}</span>
                <Switch
                  on={settings.web_panel}
                  onChange={(on) => patch({ web_panel: on })}
                />
              </div>
              {settings.web_panel && panelUrl && (
                <div className="kv">
                  <span className="k">{m.webPanelUrl}</span>
                  <span className="mono">{panelUrl}</span>
                  <button
                    type="button"
                    className="btn btn-tiny kv-copy"
                    onClick={() => copy("panel", panelUrl)}
                  >
                    {copyLabel("panel")}
                  </button>
                </div>
              )}
              <p className="hint">{m.webPanelHint}</p>
              {/* 两个开关都开着才提示代价 —— 只在面板开着时说「同一 WiFi 下
                  任何设备都能看」会吓到只在本机看的人 */}
              {settings.web_panel && settings.lan_access && (
                <p className="hint">{m.webPanelLanWarning}</p>
              )}
            </div>

            {/* ---- 通知转发入口 ----
                放在「接入」页靠上的位置、接口速查之前：想「完成后 POST 到我的
                服务器」的人不会去翻「钩子」，得在接入流程里就看见它。 */}
            <div className="section">
              <div className="row">
                <span>{m.outboundHooks}</span>
                <button type="button" className="btn" onClick={() => setTab("hooks")}>
                  {m.configure}
                </button>
              </div>
              <p className="hint">{m.connectHookHint}</p>
            </div>

          </>
        );
      }

      case "guide": {
        // 端点表跟着接口速查一起搬过来了 —— 它就长在这四节里
        const endpoints: [string, string, string][] = [
          ["POST", "/v1/state", m.apiEndpointReport],
          ["GET", "/v1/state", m.apiEndpointState],
          ["GET", "/v1/sessions", m.apiEndpointSessions],
          ["DELETE", "/v1/sessions/{id}", m.apiEndpointClear],
          ["GET", "/v1/health", m.apiEndpointHealth],
        ];

        return (
          <>
            {/* 手册的顺序：先说这是什么，再说怎么接上，然后才是查细节用的速查表，
                最后是「不工作了怎么办」。倒过来写的话，第一次用的人得先读一份
                接口规范才知道这软件是干嘛的。 */}
            <div className="section">
              <div className="section-title">{m.guideWhatTitle}</div>
              <p className="doc">{m.guideWhat}</p>
            </div>

            <div className="section">
              <div className="section-title">{m.guideStepsTitle}</div>
              <ol className="steps">
                <li>{m.guideStep1}</li>
                <li>{m.guideStep2}</li>
                <li>{m.guideStep3}</li>
              </ol>
              <div className="row">
                <span>{m.agentsTitle}</span>
                <button
                  type="button"
                  className="btn"
                  onClick={() => setTab("connect")}
                >
                  {m.tabConnect}
                </button>
              </div>
            </div>

            {/* ---- 接口速查 ----
                接入方最需要的那几样都在这一页：端口、token、端点表、状态词表、
                能直接粘的示例。**这一份是跟着包走的** —— 装了 .app 的人手上没有
                仓库，而 docs/ 刻意没打进 bundle（见 lib.rs 的 DOCS_URL）。 */}
            <div className="section">
              <div className="section-title">{m.apiTitle}</div>
              <div className="kv">
                <span className="k">{m.port}</span>
                <span className="mono">{endpoint?.port ?? "—"}</span>
                {endpoint?.port != null && (
                  <button
                    type="button"
                    className="btn btn-tiny kv-copy"
                    onClick={() => copy("port", String(endpoint.port))}
                  >
                    {copyLabel("port")}
                  </button>
                )}
              </div>
              <div className="kv">
                <span className="k">{m.apiToken}</span>
                <span className="mono token">{endpoint?.token ?? "—"}</span>
                {endpoint?.token && (
                  <button
                    type="button"
                    className="btn btn-tiny kv-copy"
                    onClick={() => copy("token", endpoint.token!)}
                  >
                    {copyLabel("token")}
                  </button>
                )}
              </div>
              <div className="row">
                <span className="muted-text mono">{endpoint?.dir ?? "—"}</span>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() =>
                    invoke("open_app_dir", { what: "config" }).catch(reported)
                  }
                >
                  {m.openConfigDir}
                </button>
              </div>
              <p className="hint">{m.apiAuthHint}</p>
            </div>

            <div className="section">
              <div className="section-title">{m.apiEndpoints}</div>
              {endpoints.map(([method, path, note]) => (
                <div className="kv endpoint" key={method + path}>
                  <span className="method mono">{method}</span>
                  <span className="mono">{path}</span>
                  <span className="muted-text">{note}</span>
                </div>
              ))}
            </div>

            <div className="section">
              <div className="section-title">{m.apiStates}</div>
              <p className="mono states">{REPORTABLE_SIGNALS.join("  ")}</p>
              <p className="hint">{m.apiStatesHint}</p>
            </div>

            <div className="section">
              <div className="section-title">{m.apiExamples}</div>
              {curlExample && (
                <>
                  <pre className="code">{curlExample}</pre>
                  <div className="row">
                    <button
                      type="button"
                      className="btn btn-tiny"
                      onClick={() => copy("curl", curlExample)}
                    >
                      {copyLabel("curl")}
                    </button>
                  </div>
                </>
              )}
              <pre className="code">{m.cliExample}</pre>
              <div className="row">
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => copy("cli", m.cliExample)}
                >
                  {copyLabel("cli")}
                </button>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => invoke("open_docs").catch(reported)}
                >
                  {m.openDocs}
                </button>
              </div>
            </div>

            <div className="section">
              <div className="section-title">{m.guideFaqTitle}</div>
              <p className="faq-q">{m.guideFaqNoLightQ}</p>
              <p className="doc">{m.guideFaqNoLightA}</p>
              <p className="faq-q">{m.guideFaqNoNotifyQ}</p>
              <p className="doc">{m.guideFaqNoNotifyA}</p>
            </div>
          </>
        );
      }

      case "hooks": {
        // 组过滤的选项：当前在上报的组，外加钩子上挂着、此刻没上报的组（下面补
        // chip，见 extraGroups）。没有它，一个暂时消失的组会从选项里静默溜走。
        const reporting = snapshot.groups.map((group) => group.name);

        return (
          <>
            {settings.hooks.length === 0 && <p className="empty">{m.noHooks}</p>}

            {settings.hooks.map((hook, index) => {
              const target = hook.type === "http" ? hook.url : hook.command;
              const shown =
                hookDraft?.index === index ? hookDraft.value : (target ?? "");
              const configured = (target ?? "").trim().length > 0;

              // 钩子上挂着、当前没上报的组：补一个 chip 让它可见可删，而不是
              // 静默留着（同「灯效」页给表外的 blink_ms 补选项的做法）。
              const extraGroups = hook.groups.filter(
                (name) => !reporting.includes(name),
              );
              const groupChoices = [...reporting, ...extraGroups];

              // 按**目标**找最近一次结果，不按下标 —— 钩子可增删排序，下标会错位，
              // 那会显示成「另一条钩子的结果」，比不显示更糟。目标为空则无从匹配。
              const attempt = configured
                ? lastAttemptFor(outboundLog, target)
                : undefined;

              // 正在新加的那一行请求头（还没进设置 —— 空名字的行存了也会被 Rust 丢掉）
              const pendingHeader =
                headerDraft && headerDraft.index === index && headerDraft.from === null
                  ? headerDraft
                  : null;

              return (
                <div className="hook" key={index}>
                  <div className="hook-head">
                    <span className="hook-title">
                      {m.hookN(index + 1)}
                      {!configured && <span className="hook-warn">{m.noTarget}</span>}
                    </span>
                    <button
                      type="button"
                      className="btn btn-tiny"
                      onClick={() =>
                        setHooks(settings.hooks.filter((_, i) => i !== index))
                      }
                    >
                      {m.delete}
                    </button>
                  </div>

                  <div className="chips">
                    {SIGNALS.map((state) => (
                      <button
                        key={state}
                        type="button"
                        className="chip"
                        data-active={hook.when.includes(state)}
                        onClick={() =>
                          updateHook(index, {
                            when: hook.when.includes(state)
                              ? hook.when.filter((name) => name !== state)
                              : [...hook.when, state],
                          })
                        }
                      >
                        {m.shortSignal[state] ?? state}
                      </button>
                    ))}
                  </div>

                  {/* 组过滤。和上面的状态过滤是两回事，所以单独一行带个小标题，
                      免得两排 chip 看起来像同一组条件。不选 = 看全局聚合。 */}
                  <div className="hook-groups">
                    <span className="hook-groups-label">{m.hookGroups}</span>
                    <div className="chips">
                      {groupChoices.map((name) => {
                        const stale = !reporting.includes(name);
                        return (
                          <button
                            key={name}
                            type="button"
                            className="chip"
                            title={stale ? m.groupNotReporting : undefined}
                            data-active={hook.groups.includes(name)}
                            onClick={() =>
                              updateHook(index, {
                                groups: hook.groups.includes(name)
                                  ? hook.groups.filter((group) => group !== name)
                                  : [...hook.groups, name],
                              })
                            }
                          >
                            {name}
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <Row label={m.type}>
                    <div className="seg">
                      <button
                        type="button"
                        data-active={hook.type === "http"}
                        onClick={() =>
                          updateHook(index, {
                            type: "http",
                            url: "",
                            headers: {},
                            preset: "raw",
                          })
                        }
                      >
                        HTTP
                      </button>
                      <button
                        type="button"
                        data-active={hook.type === "command"}
                        onClick={() => updateHook(index, { type: "command", command: "" })}
                      >
                        {m.command}
                      </button>
                    </div>
                  </Row>

                  <input
                    className={hook.type === "http" ? "input" : "input mono"}
                    spellCheck={false}
                    placeholder={
                      hook.type === "http"
                        ? // 占位符**按预设给**：每个服务的地址形状都不一样（PushDeer
                          // 的 `…/message/push?pushkey=…` 尤其猜不出来），而填错地址
                          // 是这一列里最常见的失败
                          m.presetUrls[hook.preset]
                        : m.commandPlaceholder
                    }
                    value={shown}
                    onChange={(event) =>
                      setHookDraft({ index, value: event.target.value })
                    }
                    onBlur={commitHookDraft}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") event.currentTarget.blur();
                    }}
                  />

                  {/* 服务预设：决定发出去的 body 长什么样。默认「原样 JSON」是
                      原先那条路 —— 老钩子升级过来一个字都不用改。 */}
                  {hook.type === "http" && (
                    <>
                      <Row label={m.hookPreset}>
                        <select
                          className="select hook-preset"
                          value={hook.preset}
                          onChange={(event) =>
                            updateHook(index, {
                              preset: event.target.value as HttpPreset,
                            })
                          }
                        >
                          {HTTP_PRESETS.map((preset) => (
                            <option key={preset} value={preset}>
                              {m.presetNames[preset]}
                            </option>
                          ))}
                        </select>
                      </Row>

                      {/* 请求头：鉴权靠它。**空名字的行只活在草稿里**（见 addHeader）
                          —— 存进设置的话会被 Rust 的 clamp 丢掉，表现出来就是
                          「刚点出来的那一行自己没了」。 */}
                      <div className="hook-headers">
                        <span className="hook-headers-label">{m.hookHeaders}</span>
                        {Object.entries(hook.headers).map(([name, value]) => {
                          const draft =
                            headerDraft?.index === index &&
                            headerDraft.from === name
                              ? headerDraft
                              : null;
                          const onEdit = (field: "name" | "value") => (
                            event: ChangeEvent<HTMLInputElement>,
                          ) => {
                            // 从这一行的原值出发改一个字段，另一个字段保持用户
                            // 当前看到的那个（否则点了名字再点值会把名字改回去）
                            setHeaderDraft({
                              index,
                              from: name,
                              name:
                                field === "name"
                                  ? event.target.value
                                  : (draft?.name ?? name),
                              value:
                                field === "value"
                                  ? event.target.value
                                  : (draft?.value ?? value),
                            });
                          };
                          return (
                            <div className="hook-header" key={name}>
                              <input
                                className="input mono hook-header-name"
                                spellCheck={false}
                                placeholder={m.headerNamePlaceholder}
                                value={draft ? draft.name : name}
                                onChange={onEdit("name")}
                                onFocus={() =>
                                  setHeaderDraft({ index, from: name, name, value })
                                }
                                onBlur={commitHeaderDraft}
                                onKeyDown={headerKeys(() => setHeaderDraft(null))}
                              />
                              <input
                                className="input hook-header-value"
                                spellCheck={false}
                                placeholder={m.headerValuePlaceholder}
                                value={draft ? draft.value : value}
                                onChange={onEdit("value")}
                                onFocus={() =>
                                  setHeaderDraft({ index, from: name, name, value })
                                }
                                onBlur={commitHeaderDraft}
                                onKeyDown={headerKeys(() => setHeaderDraft(null))}
                              />
                              <button
                                type="button"
                                className="btn btn-tiny"
                                onClick={() =>
                                  updateHook(index, {
                                    headers: withoutHeader(hook.headers, name),
                                  })
                                }
                              >
                                {m.delete}
                              </button>
                            </div>
                          );
                        })}

                        {/* 正在新加的那一行 */}
                        {pendingHeader && (
                          <div className="hook-header">
                            <input
                              className="input mono hook-header-name"
                              spellCheck={false}
                              autoFocus
                              placeholder={m.headerNamePlaceholder}
                              value={pendingHeader.name}
                              onChange={(event) =>
                                setHeaderDraft({
                                  ...pendingHeader,
                                  name: event.target.value,
                                })
                              }
                              onBlur={commitHeaderDraft}
                              onKeyDown={headerKeys(() => setHeaderDraft(null))}
                            />
                            <input
                              className="input hook-header-value"
                              spellCheck={false}
                              placeholder={m.headerValuePlaceholder}
                              value={pendingHeader.value}
                              onChange={(event) =>
                                setHeaderDraft({
                                  ...pendingHeader,
                                  value: event.target.value,
                                })
                              }
                              onBlur={commitHeaderDraft}
                              onKeyDown={headerKeys(() => setHeaderDraft(null))}
                            />
                          </div>
                        )}
                        {/* 到上限就禁用：Rust 的 clamp 是按键的字典序截断的（确定，
                            但用户看不见），先在这里挡住 —— 不然「加了一条，存下去之后
                            它自己没了」会是一个查不出来的现象。 */}
                        <button
                          type="button"
                          className="btn btn-tiny"
                          disabled={
                            Object.keys(hook.headers).length >= MAX_HOOK_HEADERS
                          }
                          onClick={() => addHeader(index)}
                        >
                          {m.addHeader}
                        </button>
                      </div>
                    </>
                  )}

                  {/* 最近一次结果。目标为空时无从匹配，不渲染 —— 宁可什么都不显示，
                      也不要拿别的钩子的结果显示在这里。 */}
                  {configured && (
                    <span className="hook-result mono">
                      {attempt ? attemptLine(m, attempt, now) : m.neverFired}
                    </span>
                  )}
                </div>
              );
            })}

            {/* 「发送测试」是这一轮补上的**关键**一件：没有它，用户配完只能等
                下一次真实跃迁，而「等不到」和「发失败」从外面看一模一样。 */}
            <div className="row">
              <button type="button" className="btn" onClick={sendTest}>
                {m.sendTest}
              </button>
              {testNotice && <span className="hint">{testNotice}</span>}
            </div>
            <button type="button" className="btn" onClick={addHook}>
              {m.addHook}
            </button>
            <p className="hint">{m.hooksHint}</p>
            <p className="hint">{m.presetHint}</p>
            <p className="hint">{m.hookGroupsHint}</p>
            <p className="hint">{m.headersHint}</p>
          </>
        );
      }

      case "system": {
        // 「现在什么情况」一句话，优先级从上到下：正在装的优先级最高（它最不可
        // 打断），然后是下载中、失败、检查中、有新版本、已是最新。没查过就什么都不
        // 说 —— 空着好过写一句「未知」。
        const updateNote = update.installing
          ? m.updateInstalling
          : update.progress
            ? m.updateDownloading(progressLabel(update.progress))
            : update.failure
              ? m.updateFailed(update.failure)
              : update.checking
                ? m.checkingUpdate
                : update.state.error
                  ? m.updateFailed(update.state.error)
                  : update.state.available
                    ? m.updateAvailable(update.state.available.version)
                    : update.state.checked_at_ms > 0
                      ? m.updateLatest
                      : null;
        const updatePercent = update.progress
          ? update.progress.installing
            ? 100
            : (percent(update.progress) ?? 0)
          : 0;
        return (
          <>
            <Row label={m.launchAtLogin}>
              <Switch
                on={toggles.autostart}
                onChange={(on) => {
                  setError(null);
                  setAutostart(on).catch(reported);
                }}
              />
            </Row>
            {/* 语言：auto 跟随系统，由前端把 navigator.language 解析成 zh / en 后推给 Rust */}
            <Row label={m.language} onReset={resetFor(settings, "locale", patch)}>
              <div className="seg">
                <button
                  type="button"
                  data-active={settings.locale === "auto"}
                  onClick={() => patch({ locale: "auto" })}
                >
                  {m.langAuto}
                </button>
                <button
                  type="button"
                  data-active={settings.locale === "zh"}
                  onClick={() => patch({ locale: "zh" })}
                >
                  {m.langZh}
                </button>
                <button
                  type="button"
                  data-active={settings.locale === "en"}
                  onClick={() => patch({ locale: "en" })}
                >
                  {m.langEn}
                </button>
              </div>
            </Row>
            {/* 自动更新。**只在 macOS 上真的能用**（原因见 Rust 侧 update.rs 顶部），
                别的平台如实说「请手动下载」，而不是摆一个点了没反应的按钮。
                另：**装永远由用户点** —— 检查可以自动，装不行。 */}
            <div className="section">
              <div className="row">
                <span>{m.updateTitle}</span>
                {/* 版本号是 Rust 那边读的运行时常量。第一帧还没回来时**不画** ——
                    宁可暂时空着，也不要闪一个「当前版本 v」。 */}
                <span className="mono">
                  {update.state.current && m.updateCurrent(update.state.current)}
                </span>
              </div>
              {update.state.supported ? (
                <>
                  {updateNote && <p className="hint">{updateNote}</p>}
                  {update.progress && (
                    <div
                      className="update-line"
                      data-installing={update.progress.installing}
                    >
                      <div className="update-bar" data-installing={update.progress.installing}>
                        <span style={{ width: `${updatePercent}%` }} />
                      </div>
                    </div>
                  )}
                  <div className="row-start">
                    <button
                      type="button"
                      className="btn btn-tiny"
                      disabled={update.checking || update.installing}
                      onClick={update.check}
                    >
                      {update.checking ? m.checkingUpdate : m.checkUpdate}
                    </button>
                    {update.state.available && !update.installing && (
                      <button
                        type="button"
                        className="btn btn-tiny"
                        onClick={update.install}
                      >
                        {m.updateDownloadInstall}
                      </button>
                    )}
                  </div>
                  <p className="hint">{m.updateHint}</p>
                  {update.state.available?.notes && (
                    <>
                      <button
                        type="button"
                        className="disclosure"
                        aria-expanded={notesOpen}
                        onClick={() => setNotesOpen((open) => !open)}
                      >
                        <span className="caret" aria-hidden="true">
                          {notesOpen ? "▾" : "▸"}
                        </span>
                        <span className="disclosure-title">{m.updateNotes}</span>
                      </button>
                      {notesOpen && (
                        <pre className="updatenotes mono">{update.state.available.notes}</pre>
                      )}
                    </>
                  )}
                </>
              ) : (
                <>
                  <p className="hint">{m.updateUnsupported}</p>
                  <div className="row-start">
                    <button
                      type="button"
                      className="btn btn-tiny"
                      onClick={() => invoke("open_releases").catch(reported)}
                    >
                      {m.openReleases}
                    </button>
                  </div>
                </>
              )}
            </div>

            <p className="hint">{m.systemHintTray}</p>
            <p className="hint">
              {m.systemHintPassThrough(
                toggles.pass_through_shortcut || m.shortcutFallback,
              )}
            </p>

            {/* 这两个原先挂在顶部栏上。搬到这一页是因为它们是**诊断入口**
                （看配置、看日志），而这一页本来就是「系统层面的事」——
                而顶部栏那个位置现在归左栏了，留在那儿等于占着每一页。
                用 `row-start` 而不是 `row`：后者是「标签在左、控件在右」的
                两端分布，两个并列按钮用它会被拆到两头。 */}
            <div className="section">
              <div className="row-start">
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => invoke("open_app_dir", { what: "config" }).catch(reported)}
                >
                  {m.openConfigDir}
                </button>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => invoke("open_app_dir", { what: "log" }).catch(reported)}
                >
                  {m.openLogs}
                </button>
              </div>
              {/* 把日志的尾巴直接摆出来，省掉「打开文件、拉到最后、找时间」那一串。
                  它回答的是「接了但灯不亮」里最难自己查到的那一半：**事件到底有没有
                  从智能体那边打过来**。
                  和「接入」页那个「试一下」是互补的 —— 那个测的是 app 内部那一段
                  （亮得起来就说明聚合、窗口、推送都是活的），这一段测的是智能体 →
                  HTTP 那一段。两边都通才是真的通。 */}
              <div className="row">
                <span>{m.diagLogTitle}</span>
                <button type="button" className="btn btn-tiny" onClick={refreshLog}>
                  {m.diagRefresh}
                </button>
              </div>
              <p className="hint">{m.diagLogHint}</p>
              {logLines.length === 0 ? (
                <p className="hint">{m.diagLogEmpty}</p>
              ) : (
                <pre className="hooklog mono">{logLines.join("\n")}</pre>
              )}
            </div>

            {/* 配置备份。和上面那节是同一类（文件层面的事），但单独一节 ——
                它有一句要读的说明，而上面那两个只是两个诊断入口。 */}
            <div className="section">
              <div className="row-start">
                <button type="button" className="btn btn-tiny" onClick={exportConfig}>
                  {m.exportConfig}
                </button>
                <button type="button" className="btn btn-tiny" onClick={importConfig}>
                  {m.importConfig}
                </button>
              </div>
              <p className="hint">{backupNotice ?? m.configBackupHint}</p>
            </div>
          </>
        );
      }
    }
  };

  return (
    // 颜色变量挂在根上，会话列表里的状态点因此和灯球一致
    <div className="settings" style={colorVars(settings)}>
      <h1 data-state={snapshot.aggregate}>
        <span className="state-dot" />
        {signalLabel(lang, snapshot.aggregate)}
        {snapshot.groups.length > 0 && (
          <span className="headline-count">
            {m.groupsAndSessions(snapshot.groups.length, sessionCount)}
          </span>
        )}
      </h1>
      {/* 没有提示文本时整段不渲染 —— 否则会白占一行高度 */}
      {(snapshot.paused || snapshot.message) && (
        <p className="headline-message">
          {snapshot.paused ? m.pausedBanner : snapshot.message}
        </p>
      )}

      {stats.length > 0 && (
        <p className="stats">
          {m.today}{" "}
          {stats
            .map((row) => `${row.label} ${formatDuration(row.seconds)}`)
            .join(" · ")}
        </p>
      )}

      {/* 两栏：左栏是「去哪儿」（三组页签 + 沉底的「系统」 + 底部的全局开关），
          右栏是内容。
          原先页签横排在顶部、内容区写死 495px 高 —— 那个高度得手写一个「够大
          又不过分大」的数，改内容就得重新量。改成 flex 之后这条性质由结构保证。 */}
      <div className="settings-body">
        <nav className="rail">
          <div className="rail-nav">
            {groups.map((group) => (
              <div className="rail-group" key={group.title}>
                {/* 分组标题只是标题，不是按钮 —— 点它什么都不该发生（点一下
                    「灯」而跳到某一页会让人以为漏了一层）。 */}
                <div className="rail-group-title">{group.title}</div>
                {group.entries.map((entry) => (
                  <button
                    key={entry.id}
                    type="button"
                    className="rail-item"
                    data-active={tab === entry.id}
                    aria-current={tab === entry.id ? "page" : undefined}
                    onClick={() => setTab(entry.id)}
                  >
                    {entry.label}
                  </button>
                ))}
              </div>
            ))}

            {/* 系统沉在最下面：它不属于上面任何一组，塞进去就得为它编一个它并不
                属于的标题。 */}
            <div className="rail-bottom">
              <button
                type="button"
                className="rail-item"
                data-active={tab === "system"}
                aria-current={tab === "system" ? "page" : undefined}
                onClick={() => setTab("system")}
              >
                {m.tabSystem}
              </button>
            </div>
          </div>

          {/* 全局入口压在最下面：它不是一个页签，是一个开关 —— 放在这一组里
              会让人以为切过去能看到别的东西。 */}
          <div className="rail-foot">
            <label className="row" title={m.pauseHint}>
              <span className="muted-text">{m.pauseMonitoring}</span>
              <Switch on={snapshot.paused} onChange={setPaused} />
            </label>
          </div>
        </nav>

        <div className="page">{page()}</div>
      </div>

      {error && <p className="error">{error}</p>}

      <div className="footer">
        <button
          type="button"
          className="btn danger"
          onClick={() => void invoke("quit").catch(reported)}
        >
          {m.quit}
        </button>
      </div>
    </div>
  );
}
