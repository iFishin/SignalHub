import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import { useSettings, updateSettings, type Settings, type Shape, type Palette, type Hook, type StateStyle } from "../lib/useSettings";
import {
  useToggles,
  setAutostart,
  setPassThrough,
} from "../lib/useToggles";
import { useStats } from "../lib/useStats";
import { useTimeline, type TimelineSegment } from "../lib/useTimeline";
import { colorVars, resolveColors } from "../lib/palettes";
import { PRESETS, isActivePreset, STYLE_PRESETS, isActiveStylePreset } from "../lib/presets";
import { SIGNALS, type Snapshot } from "../lib/types";

/** 与 Rust 侧 config.rs 的 MIN_SIZE / MAX_SIZE / DEFAULT_SIZE 保持一致。 */
const SIZES = [
  { label: "小", value: 16 },
  { label: "中", value: 22 },
  { label: "大", value: 32 },
];

/** 与 Rust 侧 `config::Shape` 一致。 */
const SHAPES: { label: string; value: Shape }[] = [
  { label: "圆", value: "circle" },
  { label: "圆角方", value: "rounded" },
  { label: "环", value: "ring" },
];

/** 与 Rust 侧 `config::Palette` 一致。给预设而不是任意取色。 */
const PALETTES: { label: string; value: Palette }[] = [
  { label: "默认", value: "default" },
  { label: "色盲友好", value: "colorblind" },
  { label: "高饱和", value: "vivid" },
];

/** 牌上可以单独关掉的四段内容。顺序就是它们在牌上出现的顺序。 */
const FIELDS: { key: keyof Settings["show"]; label: string }[] = [
  { key: "name", label: "组名" },
  { key: "state", label: "状态" },
  { key: "age", label: "已持续" },
  { key: "message", label: "消息" },
];

/** 与 Rust 侧 config.rs 的 MIN_BLINK_SPEED / MAX_BLINK_SPEED 一致。 */
const BLINK_RANGE = { min: 50, max: 200 };

/**
 * 单个状态的闪烁间隔选项。`""` = 跟随全局，`"0"` = 常亮。
 *
 * 给一列固定档位而不是让用户敲毫秒：10 个状态各敲一次数字太累，而「快一点 /
 * 慢一点」本来也就那么几档。配置文件里手写的、不在这张表里的值会临时补一个
 * 选项进去（见下面的 `extraBlink`），不会显示成空白。
 */
const BLINK_OPTIONS: { label: string; value: string }[] = [
  { label: "跟随", value: "" },
  { label: "常亮", value: "0" },
  { label: "2.0s", value: "2000" },
  { label: "1.2s", value: "1200" },
  { label: "0.7s", value: "700" },
  { label: "0.4s", value: "400" },
  { label: "0.2s", value: "200" },
];

/** 勿扰时段的起止小时。只有小时没有分钟 —— 见 config::QuietHours 的说明。 */
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

/** 「持续多久才提醒」的档位。0 = 立刻。 */
const ALERT_DELAYS: { label: string; value: number }[] = [
  { label: "立刻", value: 0 },
  { label: "3 秒", value: 3 },
  { label: "5 秒", value: 5 },
  { label: "10 秒", value: 10 },
  { label: "30 秒", value: 30 },
  { label: "1 分钟", value: 60 },
];

type Tab = "status" | "look" | "style" | "alert" | "connect" | "hooks" | "system";

const TABS: { id: Tab; label: string }[] = [
  { id: "status", label: "状态" },
  { id: "look", label: "外观" },
  { id: "style", label: "灯效" },
  { id: "alert", label: "提醒" },
  { id: "connect", label: "接入" },
  { id: "hooks", label: "钩子" },
  { id: "system", label: "系统" },
];

/** 状态名的短标签，给钩子过滤条件的那些小按钮用（10 个要挤在一行里换行）。 */
const SHORT_LABEL: Record<string, string> = {
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
};

interface Endpoint {
  port: number | null;
  token_preview: string | null;
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
  onChange,
  format,
}: {
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  format: (value: number) => string;
}) {
  return (
    <div className="slider">
      <input
        type="range"
        min={min}
        max={max}
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
 * 近 30 分钟的状态条带。
 *
 * 每一段的**宽度就是它的时长** —— 一眼看出这半小时都在干什么，以及哪一段是不是
 * 卡了很久。今日统计回答「一共多久」，这条回答「什么时候」，两个问题不一样。
 */
function TimelineStrip({ segments }: { segments: TimelineSegment[] }) {
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
            title={`${SIGNAL_LABEL[segment.state]} · ${formatDuration(
              Math.round(segment.ms / 1000),
            )}`}
          />
        ))}
      </div>
      <div className="timeline-axis">
        <span>30 分钟前</span>
        <span>现在</span>
      </div>
    </div>
  );
}

export function SettingsPanel({ snapshot }: Props) {
  const settings = useSettings();
  const toggles = useToggles();
  const stats = useStats();
  const timeline = useTimeline(snapshot.aggregate);
  const [tab, setTab] = useState<Tab>("status");
  const [endpoint, setEndpoint] = useState<Endpoint | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    invoke<Endpoint>("get_endpoint").then(setEndpoint).catch(() => {
      // 端口/token 读不到就是「服务还没起来」，界面上显示为 — 即可
    });
  }, []);

  const reported = (err: unknown) => setError(String(err));

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

  /** 把某一组从「已隐藏」里放回来。 */
  const unhide = (group: string) =>
    patch({
      hidden_groups: settings.hidden_groups.filter((name) => name !== group),
    });

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
      // 目标先留空：空目标不算错（不触发），用户填了才开始工作
      { when: ["permission", "attention", "error"], type: "http", url: "" },
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

  const curlExample = endpoint?.port
    ? `curl -s -X POST http://127.0.0.1:${endpoint.port}/v1/state ` +
      `-H "Authorization: Bearer $(cat ~/.signalhub/token)" ` +
      `-H 'Content-Type: application/json' -d '{"state":"working"}'`
    : null;

  // 每个页签的内容。写成函数而不是子组件，是为了免去把十几个 prop 一路传下去。
  const page = () => {
    switch (tab) {
      case "status":
        return (
          <>
            <TimelineStrip segments={timeline} />

            {(settings.hidden_groups.length > 0 ||
              settings.muted_groups.length > 0) && (
              <div className="row hidden-summary">
                <span className="muted-text">
                  {settings.hidden_groups.length > 0 &&
                    `灯上隐藏 ${settings.hidden_groups.length} 组`}
                  {settings.hidden_groups.length > 0 &&
                    settings.muted_groups.length > 0 &&
                    " · "}
                  {settings.muted_groups.length > 0 &&
                    `静音 ${settings.muted_groups.length} 组`}
                </span>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() => patch({ hidden_groups: [], muted_groups: [] })}
                >
                  全部恢复
                </button>
              </div>
            )}

            {snapshot.groups.length === 0 ? (
              <p className="empty">
                暂无会话。接入智能体或调用上报接口后，这里会列出来。
              </p>
            ) : (
              snapshot.groups.map((group) => {
                const hidden = settings.hidden_groups.includes(group.name);
                const muted = settings.muted_groups.includes(group.name);
                return (
                  <div className="group" key={group.name} data-hidden={hidden}>
                    <div className="group-head" data-state={group.aggregate}>
                      <span className="state-dot" />
                      <span className="group-name">{group.name}</span>
                      {/* 两个标记本身就是开关：点一下恢复。做成按钮而不是纯标签，
                          是因为恢复这件事必须在这里做得到 —— 灯上右键固然也行，
                          但那个组可能已经被藏起来、根本右键不到。 */}
                      {hidden && (
                        <button
                          type="button"
                          className="badge"
                          title="点一下让它在灯上重新显示"
                          onClick={() => unhide(group.name)}
                        >
                          已隐藏
                        </button>
                      )}
                      {muted && (
                        <button
                          type="button"
                          className="badge"
                          title="点一下取消静音"
                          onClick={() => setMuted(group.name, false)}
                        >
                          已静音
                        </button>
                      )}
                      <span className="group-meta">
                        {SIGNAL_LABEL[group.aggregate]} · {group.sessions.length} 个会话
                      </span>
                      {!muted && (
                        <button
                          type="button"
                          className="btn btn-tiny"
                          title="不再因为这一组弹通知 / 出声"
                          onClick={() => setMuted(group.name, true)}
                        >
                          静音
                        </button>
                      )}
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
                                {SIGNAL_LABEL[session.state]}
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
          </>
        );

      case "look":
        return (
          <>
            <Row label="预设">
              <div className="chips chips-tight">
                {PRESETS.map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    className="chip"
                    title={preset.hint}
                    data-active={isActivePreset(preset, settings)}
                    onClick={() => patch(preset.apply)}
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
            </Row>
            <Row label="形态">
              <div className="seg">
                <button
                  type="button"
                  data-active={!settings.collapsed}
                  onClick={() => patch({ collapsed: false })}
                >
                  展开牌面
                </button>
                <button
                  type="button"
                  data-active={settings.collapsed}
                  onClick={() => patch({ collapsed: true })}
                >
                  一颗灯
                </button>
              </div>
            </Row>
            <Row label="大小">
              <div className="seg">
                {SIZES.map((size) => (
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
            <Row label="形状">
              <div className="seg">
                {SHAPES.map((shape) => (
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
            <Row label="配色方案">
              <div className="seg">
                {PALETTES.map((palette) => (
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
            <Row label="背景透明度">
              <Slider
                value={settings.sign_opacity}
                min={0}
                max={100}
                onChange={(sign_opacity) => patch({ sign_opacity })}
                format={(value) => `${value}%`}
              />
            </Row>
            <Row label="闪烁节奏">
              <Slider
                value={settings.blink_speed}
                min={BLINK_RANGE.min}
                max={BLINK_RANGE.max}
                onChange={(blink_speed) => patch({ blink_speed })}
                format={(value) => `${value}%`}
              />
            </Row>
            <Row label="牌上显示">
              <div className="chips chips-tight">
                {FIELDS.map((field) => (
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
            <Row label="安静模式">
              <Switch on={settings.quiet} onChange={(on) => patch({ quiet: on })} />
            </Row>
            <p className="hint">
              形状只影响灯珠：改成圆角方或环会让 permission / error 的方角区分失效，
              此时建议配合色盲友好配色。
            </p>
          </>
        );

      case "style": {
        const resolved = resolveColors(settings);

        /**
         * 改某一个状态的三样里的一样。
         *
         * 三样都没设就把整条记录删掉 —— 留一条全 null 的空记录会让「当前是哪套
         * 预设」判断不出来（预设比的是整份表），而它本身没有任何作用。
         */
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
            <Row label="预设">
              <div className="chips chips-tight">
                {STYLE_PRESETS.map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    className="chip"
                    title={preset.hint}
                    data-active={isActiveStylePreset(preset, settings)}
                    // 预设给的是**完整**一份表，所以是整份替换而不是合并
                    onClick={() => patch({ styles: preset.styles })}
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
            </Row>

            <div className="style-head">
              <span className="style-swatch" />
              <span className="style-name">状态</span>
              <span className="style-col">形状</span>
              <span className="style-col">节奏</span>
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
                  blink !== "" && !BLINK_OPTIONS.some((o) => o.value === blink)
                    ? blink
                    : null;

                return (
                  <div className="style-row" key={state} data-state={state}>
                    {/* 原生取色器。onChange 在拖动时连续触发，也就连续保存 ——
                        这是刻意的：改颜色必须能实时看到效果。 */}
                    <input
                      type="color"
                      className="style-swatch"
                      title="灯珠颜色"
                      value={resolved[state]}
                      onChange={(event) =>
                        setStyle(state, { color: event.target.value })
                      }
                    />
                    <span className="style-name">{SIGNAL_LABEL[state]}</span>
                    <select
                      className="select style-col"
                      title="灯珠形状"
                      value={style.shape ?? ""}
                      onChange={(event) =>
                        setStyle(state, {
                          shape: (event.target.value || null) as Shape | null,
                        })
                      }
                    >
                      <option value="">跟随</option>
                      {SHAPES.map((shape) => (
                        <option key={shape.value} value={shape.value}>
                          {shape.label}
                        </option>
                      ))}
                    </select>
                    <select
                      className="select style-col"
                      title="闪烁间隔"
                      value={blink}
                      onChange={(event) =>
                        setStyle(state, {
                          blink_ms:
                            event.target.value === ""
                              ? null
                              : Number(event.target.value),
                        })
                      }
                    >
                      {BLINK_OPTIONS.map((option) => (
                        <option key={option.value} value={option.value}>
                          {option.label}
                        </option>
                      ))}
                      {extraBlink && <option value={extraBlink}>{extraBlink}ms</option>}
                    </select>
                    <button
                      type="button"
                      className="btn btn-tiny style-reset"
                      title="恢复成跟随全局"
                      disabled={!overridden}
                      onClick={() => resetStyle(state)}
                    >
                      ↺
                    </button>
                  </div>
                );
              })}
            </div>

            <p className="hint">
              「跟随」= 用「外观」页选的那两项（形状、节奏倍率）。
            </p>
          </>
        );
      }

      case "alert": {
        // 阈值给一列固定档位而不是滑块：这是个粗选择（「立刻」还是「等一会儿」），
        // 秒级的精调没有意义。配置文件里手写的不在表里的值会补一个选项进来。
        const extraDelay = ALERT_DELAYS.some((d) => d.value === settings.alert_after_secs)
          ? null
          : settings.alert_after_secs;

        return (
          <>
            <Row label="需要时弹通知">
              <Switch on={settings.notify} onChange={(on) => patch({ notify: on })} />
            </Row>
            <Row label="通知带提示音">
              <Switch on={settings.sound} onChange={(on) => patch({ sound: on })} />
            </Row>
            <Row label="持续多久才提醒">
              <select
                className="select"
                title="状态要持续这么久才弹通知，一闪而过的忽略掉"
                value={settings.alert_after_secs}
                onChange={(event) =>
                  patch({ alert_after_secs: Number(event.target.value) })
                }
              >
                {ALERT_DELAYS.map((delay) => (
                  <option key={delay.value} value={delay.value}>
                    {delay.label}
                  </option>
                ))}
                {extraDelay !== null && (
                  <option value={extraDelay}>{extraDelay} 秒</option>
                )}
              </select>
            </Row>
            <Row label="勿扰时段">
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
            <p className="hint">
              需要授权、需要关注或出错时弹一条系统通知。声音挂在通知上，所以关掉
              通知也就没有声音。勿扰时段里不弹也不出声，灯照常显示 —— 而且不补发：
              早上醒来收到一堆昨晚的通知，正是勿扰想避免的。
            </p>
          </>
        );
      }

      case "connect":
        return (
          <>
            <Row label="暂停监控">
              <Switch on={snapshot.paused} onChange={setPaused} />
            </Row>
            <p className="hint">
              暂停后灯球显示为熄灭，但会话仍在记录 —— 恢复时立刻回到真实状态。
              弹不弹通知在「提醒」页。
            </p>

            {/* 转发搬到了「钩子」页（多条、可过滤、支持命令），这里只留入口 */}
            <div className="row">
              <span>出站钩子</span>
              <button type="button" className="btn" onClick={() => setTab("hooks")}>
                去配置
              </button>
            </div>
            <p className="hint">
              状态变化时可以 POST 到某个地址或跑一条命令 —— 在「钩子」页配置。
            </p>

            <div className="kv">
              <span className="k">端口</span>
              <span className="mono">{endpoint?.port ?? "—"}</span>
            </div>
            <div className="kv">
              <span className="k">Token</span>
              <span className="mono">
                {endpoint?.token_preview ? `${endpoint.token_preview}…` : "—"}
              </span>
            </div>
            {curlExample && <div className="hint mono">{curlExample}</div>}
          </>
        );

      case "hooks":
        return (
          <>
            {settings.hooks.length === 0 && (
              <p className="empty">
                还没有钩子。加一条，就能在状态变化时 POST 到某个地址或跑一条命令。
              </p>
            )}

            {settings.hooks.map((hook, index) => {
              const target = hook.type === "http" ? hook.url : hook.command;
              const shown =
                hookDraft?.index === index ? hookDraft.value : (target ?? "");
              const configured = (target ?? "").trim().length > 0;

              return (
                <div className="hook" key={index}>
                  <div className="hook-head">
                    <span className="hook-title">
                      钩子 {index + 1}
                      {!configured && <span className="hook-warn">未填目标</span>}
                    </span>
                    <button
                      type="button"
                      className="btn btn-tiny"
                      onClick={() =>
                        setHooks(settings.hooks.filter((_, i) => i !== index))
                      }
                    >
                      删除
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
                        {SHORT_LABEL[state] ?? state}
                      </button>
                    ))}
                  </div>

                  <Row label="类型">
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
                        命令
                      </button>
                    </div>
                  </Row>

                  <input
                    className={hook.type === "http" ? "input" : "input mono"}
                    spellCheck={false}
                    placeholder={
                      hook.type === "http" ? "https://ntfy.sh/my-topic" : "say 完成"
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
                </div>
              );
            })}

            <button type="button" className="btn" onClick={addHook}>
              + 添加钩子
            </button>
            <p className="hint">
              状态全不选 = 任何变化都触发。命令通过 sh -c 执行，超时 10 秒 ——
              配置文件在你手里，所以这里没有额外的开关。
            </p>
          </>
        );

      case "system":
        return (
          <>
            <Row label="保持在最前">
              <Switch
                on={settings.always_on_top}
                onChange={(on) => patch({ always_on_top: on })}
              />
            </Row>
            <Row label="托盘跟随状态">
              <Switch
                on={settings.tray_follows_state}
                onChange={(on) => patch({ tray_follows_state: on })}
              />
            </Row>
            <Row label="开机自启">
              <Switch
                on={toggles.autostart}
                onChange={(on) => {
                  setError(null);
                  setAutostart(on).catch(reported);
                }}
              />
            </Row>
            <Row label="点击穿透">
              <Switch
                on={toggles.pass_through}
                onChange={(on) => {
                  setError(null);
                  setPassThrough(on).catch(reported);
                }}
              />
            </Row>
            <p className="hint">
              托盘跟随状态时，菜单栏那颗点会变成当前状态的颜色 —— 不看悬浮灯也
              知道现在什么情况。关掉就回到单色字形，跟着菜单栏明暗自动反色。
            </p>
            <p className="hint">
              穿透后圆点不再响应鼠标，按{" "}
              {toggles.pass_through_shortcut || "快捷键"} 或从托盘菜单退出。
              该模式不会保留到下次启动。
            </p>
          </>
        );
    }
  };

  return (
    // 颜色变量挂在根上，会话列表里的状态点因此和灯球一致
    <div className="settings" style={colorVars(settings)}>
      <h1 data-state={snapshot.aggregate}>
        <span className="state-dot" />
        {SIGNAL_LABEL[snapshot.aggregate]}
        {snapshot.groups.length > 0 && (
          <span className="headline-count">
            {snapshot.groups.length} 组 · {sessionCount} 个会话
          </span>
        )}
      </h1>
      {/* 没有提示文本时整段不渲染 —— 否则会白占一行高度 */}
      {(snapshot.paused || snapshot.message) && (
        <p className="headline-message">
          {snapshot.paused
            ? "监控已暂停：灯球显示熄灭，但会话仍在记录"
            : snapshot.message}
        </p>
      )}

      {stats.length > 0 && (
        <p className="stats">
          今日{" "}
          {stats
            .map((row) => `${row.label} ${formatDuration(row.seconds)}`)
            .join(" · ")}
        </p>
      )}

      <div className="tabs" role="tablist">
        {TABS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            role="tab"
            className="tab"
            data-active={tab === entry.id}
            aria-selected={tab === entry.id}
            onClick={() => setTab(entry.id)}
          >
            {entry.label}
          </button>
        ))}
      </div>

      {/* 固定高度 + 内部滚动：窗口尺寸因此恒定，切页签不会跳 */}
      <div className="page">{page()}</div>

      {error && <p className="error">{error}</p>}

      <div className="footer">
        <button
          type="button"
          className="btn danger"
          onClick={() => void invoke("quit").catch(reported)}
        >
          退出 SignalHub
        </button>
      </div>
    </div>
  );
}
