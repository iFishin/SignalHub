import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { inTauri } from "../lib/env";
import { signalLabel } from "../lib/signals";
import { labelOf } from "../lib/lampSort";
import { useLang, useMessages, type Lang, type Messages } from "../lib/i18n";
import { useSettings, updateSettings, type Settings, type Shape, type Palette, type Hook, type StateStyle, type GroupMeta } from "../lib/useSettings";
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
import { useAgents, useHookLog, type AgentStatus, type HookChange } from "../lib/useAgents";
import { colorVars, resolveColors } from "../lib/palettes";
import { presets, isActivePreset, stylePresets, isActiveStylePreset } from "../lib/presets";
import { SIGNALS, REPORTABLE_SIGNALS, type SignalState, type Snapshot } from "../lib/types";

/**
 * 给「只用命令行上报」的智能体抄的那条命令。
 *
 * **不进 i18n**：它就是一条 shell 命令，`--agent pi` 里的名字是给上报体用的标识，
 * 翻成中文只会让人抄错。会话 id 留成占位符 —— 那个值只有 Pi Agent 自己知道。
 */
const PI_AGENT_COMMAND = 'signalhub working --agent pi --session "<session-id>"';

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
 * 页签。
 *
 * **七个，不再有「灯效」**：那一页和「外观」是同一件事的两层（全局默认 + 逐状态
 * 覆盖），分成两个页签只会让人在两个之间来回找「改颜色到底在哪」。
 *
 * 「指南」排在接入之后、系统之前 —— 它是一份手册，属于「看」而不是「调」，放在
 * 一堆开关中间会打断手感。
 */
type Tab = "status" | "history" | "look" | "alert" | "connect" | "hooks" | "guide" | "system";

function tabOptions(m: Messages): { id: Tab; label: string }[] {
  return [
    { id: "status", label: m.tabStatus },
    // 紧挨着「状态」：一个是「现在怎么样」，一个是「这几天怎么样」，同一件事的
    // 两个时间尺度，分开只是因为合在一页会把「现在」淹掉（那一页自己写着
    // 「只回答现在发生了什么」）。
    { id: "history", label: m.tabHistory },
    { id: "look", label: m.tabLook },
    { id: "alert", label: m.tabAlert },
    { id: "connect", label: m.tabConnect },
    { id: "hooks", label: m.tabHooks },
    { id: "guide", label: m.tabGuide },
    { id: "system", label: m.tabSystem },
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

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="row">
      <span>{label}</span>
      {children}
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
function PerStateStyles({
  settings,
  patch,
  m,
  lang,
  shapes,
  blinkChoices,
}: {
  settings: Settings;
  patch: (next: Partial<Settings>) => void;
  m: Messages;
  lang: Lang;
  shapes: { label: string; value: Shape }[];
  blinkChoices: { label: string; value: string }[];
}) {
  const resolved = resolveColors(settings);

  const setStyle = (state: string, next: Partial<StateStyle>) => {
    const merged = { ...(settings.styles[state] ?? {}), ...next };
    const styles = { ...settings.styles };
    const empty =
      merged.color == null && merged.shape == null && merged.blink_ms == null;
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
        <span className="style-reset" />
      </div>

      <div className="styles">
        {SIGNALS.map((state) => {
          const style = settings.styles[state] ?? {};
          const overridden =
            style.color != null || style.shape != null || style.blink_ms != null;
          const blink = style.blink_ms == null ? "" : String(style.blink_ms);
          // 手改配置文件可能写出选项里没有的毫秒数，临时补一个选项，
          // 否则 select 会显示成空白 —— 看起来像「没设」，其实设了
          const extraBlink =
            blink !== "" && !blinkChoices.some((o) => o.value === blink) ? blink : null;

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

export function SettingsPanel({ snapshot }: Props) {
  const settings = useSettings();
  const toggles = useToggles();
  const stats = useStats();
  const timeline = useTimeline(snapshot.aggregate);
  const { days: historyDays, refresh: refreshHistory } = useHistory();
  const feed = useMessageFeed(snapshot.aggregate);
  const outboundLog = useOutboundLog(snapshot.aggregate);
  const { agents, refresh: refreshAgents } = useAgents();
  const { lines: logLines, refresh: refreshLog } = useHookLog();
  const m = useMessages();
  const lang = useLang();
  const sizes = sizeOptions(m);
  const shapes = shapeOptions(m);
  const palettes = paletteOptions(m);
  const fields = fieldOptions(m);
  const blinkChoices = blinkOptions(m);
  const alertChoices = alertDelayOptions(m);
  const tabs = tabOptions(m);
  const [tab, setTab] = useState<Tab>("status");

  // 「多久以前」要跟着时间走。**只在真的显示这两段的页签上开钟**（消息流在
  // 「状态」、出站结果在「转发」），其它五页一个定时器都不起 —— 这块面板一秒
  // 重绘一次不便宜，没显示的东西不值得为它重绘。
  const visible = useVisible();
  const now = useNow(visible && (tab === "status" || tab === "hooks"));

  const [endpoint, setEndpoint] = useState<Endpoint | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 正在接入 / 卸载的那个智能体（按 id），用来禁用按钮。同时只可能有一个在飞。
  const [busyAgent, setBusyAgent] = useState<string | null>(null);
  // 每个智能体最近一次接入 / 卸载改了什么，就地显示一行结果 —— 这是用户唯一
  // 能看到「刚才那下到底做了什么」的地方。
  const [agentChanges, setAgentChanges] = useState<Record<string, HookChange>>({});
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
  const changeAgent = (agent: AgentStatus, install: boolean) => {
    setError(null);
    setBusyAgent(agent.id);
    const command = install ? "install_agent_hooks" : "uninstall_agent_hooks";
    invoke<HookChange>(command, { agent: agent.id })
      .then((change) => {
        setAgentChanges((prev) => ({ ...prev, [agent.id]: change }));
        refreshAgents();
      })
      .catch(reported)
      .finally(() => setBusyAgent(null));
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

  /** 清空显示名 = 回到「跟随上报方给的原始组名」。 */
  const setGroupName = (group: string, name: string) =>
    setGroupMeta(group, { ...settings.group_meta[group], name: name || null });

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
  const [nameDraft, setNameDraft] = useState<{ group: string; value: string } | null>(null);

  const commitNameDraft = () => {
    if (!nameDraft) return;
    const { group, value } = nameDraft;
    setNameDraft(null);
    const current = settings.group_meta[group]?.name ?? "";
    if (value.trim() === current) return;
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
      { when: ["permission", "attention", "error"], groups: [], type: "http", url: "" },
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

      case "look":
        return (
          <>
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
            <Row label={m.form}>
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
            <Row label={m.size}>
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
            <Row label={m.shape}>
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
            <Row label={m.palette}>
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
            <Row label={m.signOpacity}>
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
            <Row label={m.signWidth}>
              <Slider
                value={settings.sign_w_mult}
                min={SIGN_MULT_RANGE.min}
                max={SIGN_MULT_RANGE.max}
                step={0.1}
                onChange={(sign_w_mult) => patch({ sign_w_mult })}
                format={(value) => `×${value.toFixed(1)}`}
              />
            </Row>
            <Row label={m.signHeight}>
              <Slider
                value={settings.sign_h_mult}
                min={SIGN_MULT_RANGE.min}
                max={SIGN_MULT_RANGE.max}
                step={0.1}
                onChange={(sign_h_mult) => patch({ sign_h_mult })}
                format={(value) => `×${value.toFixed(1)}`}
              />
            </Row>
            <Row label={m.blinkSpeed}>
              <Slider
                value={settings.blink_speed}
                min={BLINK_RANGE.min}
                max={BLINK_RANGE.max}
                onChange={(blink_speed) => patch({ blink_speed })}
                format={(value) => `${value}%`}
              />
            </Row>
            <Row label={m.onSign}>
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
            <Row label={m.quietMode}>
              <Switch on={settings.quiet} onChange={(on) => patch({ quiet: on })} />
            </Row>
            <p className="hint">{m.lookHint}</p>

            {/* 逐状态覆盖和上面的全局项是**两层**，所以单独起一个标题分开 ——
                混在一起会让人以为改颜色是全局的。 */}
            <div className="section-title">{m.perStateStyle}</div>
            <PerStateStyles
              settings={settings}
              patch={patch}
              m={m}
              lang={lang}
              shapes={shapes}
              blinkChoices={blinkChoices}
            />

            {/* 各组的显示。和上面那张表是**两层**：那张表是「某个状态长什么样」，
                这一节是「哪个组叫什么、排在哪、上不上灯、响不响」。原先这些散在
                「状态」页的组卡上，和只读的实时会话混在一起 —— 那一页因此成了
                最长的一页，而两者的刷新频率和心智模型都不一样。 */}
            <div className="section">
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
                            onChange={(event) =>
                              setNameDraft({
                                group: name,
                                value: event.target.value,
                              })
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
            <Row label={m.notifyWhenNeeded}>
              <Switch on={settings.notify} onChange={(on) => patch({ notify: on })} />
            </Row>
            <Row label={m.soundWithNotify}>
              <Switch on={settings.sound} onChange={(on) => patch({ sound: on })} />
            </Row>

            {/* 通知关着的时候不显示权限状态 —— 那时候它不说明任何问题 */}
            {settings.notify && <NotifyPermission m={m} enabled={settings.notify} />}

            <Row label={m.alertAfter}>
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
            <Row label={m.quietHours}>
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
                      {agent.installed && !agent.command_ok && (
                        <span className="agent-broken">
                          {m.agentCommandBroken}
                          {agent.command && <code className="mono">{agent.command}</code>}
                        </span>
                      )}
                      {change && (
                        <span className="agent-result muted-text">
                          {m.hookResult({
                            added: change.added.length,
                            updated: change.updated.length,
                            already: change.already.length,
                            removed: change.removed_handlers,
                          })}
                        </span>
                      )}
                    </div>
                    <button
                      type="button"
                      className="btn btn-tiny"
                      disabled={busy}
                      onClick={() => changeAgent(agent, !agent.installed)}
                    >
                      {agent.installed ? m.agentDisconnect : m.agentConnect}
                    </button>
                  </div>
                );
              })}
            </div>

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
              {/* Pi Agent 点名放在下面：它是「在事件钩子里调」那一类的具体例子 */}
              <div className="agent">
                <div className="agent-info">
                  <div className="agent-line">
                    <span className="agent-name">Pi Agent</span>
                  </div>
                  <span className="agent-config mono">{PI_AGENT_COMMAND}</span>
                </div>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => copy("pi", PI_AGENT_COMMAND)}
                >
                  {copyLabel("pi")}
                </button>
              </div>
              <p className="hint">{m.cliAgentsHint}</p>
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
                        onClick={() => updateHook(index, { type: "http", url: "" })}
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
                        ? "https://ntfy.sh/my-topic"
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

            <button type="button" className="btn" onClick={addHook}>
              {m.addHook}
            </button>
            <p className="hint">{m.hooksHint}</p>
            <p className="hint">{m.hookGroupsHint}</p>
          </>
        );
      }

      case "system":
        return (
          <>
            <Row label={m.alwaysOnTop}>
              <Switch
                on={settings.always_on_top}
                onChange={(on) => patch({ always_on_top: on })}
              />
            </Row>
            <Row label={m.trayFollowsState}>
              <Switch
                on={settings.tray_follows_state}
                onChange={(on) => patch({ tray_follows_state: on })}
              />
            </Row>
            <Row label={m.launchAtLogin}>
              <Switch
                on={toggles.autostart}
                onChange={(on) => {
                  setError(null);
                  setAutostart(on).catch(reported);
                }}
              />
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
            {/* 语言：auto 跟随系统，由前端把 navigator.language 解析成 zh / en 后推给 Rust */}
            <Row label={m.language}>
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

      {/* 两栏：左栏是「去哪儿」（七个页签 + 底部的全局开关），右栏是内容。
          原先页签横排在顶部、内容区写死 495px 高 —— 那个高度得手写一个「够大
          又不过分大」的数，改内容就得重新量。改成 flex 之后这条性质由结构保证。 */}
      <div className="settings-body">
        <nav className="rail">
          <div className="rail-nav">
            {tabs.map((entry) => (
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
