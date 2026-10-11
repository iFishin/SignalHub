import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { signalLabel } from "../lib/signals";
import { useLang, useMessages, type Lang, type Messages } from "../lib/i18n";
import { useSettings, updateSettings, type Settings, type Shape, type Palette, type Hook, type StateStyle, type GroupMeta } from "../lib/useSettings";
import {
  useToggles,
  setAutostart,
  setPassThrough,
} from "../lib/useToggles";
import { useStats } from "../lib/useStats";
import { useTimeline, type TimelineSegment } from "../lib/useTimeline";
import { colorVars, resolveColors } from "../lib/palettes";
import { presets, isActivePreset, stylePresets, isActiveStylePreset } from "../lib/presets";
import { SIGNALS, REPORTABLE_SIGNALS, type Snapshot } from "../lib/types";

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

type Tab = "status" | "look" | "style" | "alert" | "connect" | "hooks" | "system";

function tabOptions(m: Messages): { id: Tab; label: string }[] {
  return [
    { id: "status", label: m.tabStatus },
    { id: "look", label: m.tabLook },
    { id: "style", label: m.tabStyle },
    { id: "alert", label: m.tabAlert },
    { id: "connect", label: m.tabConnect },
    { id: "hooks", label: m.tabHooks },
    { id: "system", label: m.tabSystem },
  ];
}

interface Endpoint {
  port: number | null;
  /** 完整 token，不是前几位 —— 对接方要拿它去调接口。 */
  token: string | null;
  /** `~/.signalhub`，token / port / config.json / hooks.log 都在里面。 */
  dir: string | null;
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

export function SettingsPanel({ snapshot }: Props) {
  const settings = useSettings();
  const toggles = useToggles();
  const stats = useStats();
  const timeline = useTimeline(snapshot.aggregate);
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
      case "status":
        return (
          <>
            <TimelineStrip segments={timeline} m={m} lang={lang} />

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
                  onClick={() => patch({ hidden_groups: [], muted_groups: [] })}
                >
                  {m.restoreAll}
                </button>
              </div>
            )}

            {snapshot.groups.length === 0 ? (
              <p className="empty">{m.emptySessions}</p>
            ) : (
              snapshot.groups.map((group) => {
                const hidden = settings.hidden_groups.includes(group.name);
                const muted = settings.muted_groups.includes(group.name);
                return (
                  <div className="group" key={group.name} data-hidden={hidden}>
                    <div className="group-head" data-state={group.aggregate}>
                      <span className="state-dot" />
                      <span className="group-name">
                        {settings.group_meta[group.name]?.name ?? group.name}
                      </span>
                      {/* 改过名就把原始组名也标出来 —— 用户得知道它对应的是
                          哪一路上报，否则灯上那个「构建」到底是谁就说不清了 */}
                      {settings.group_meta[group.name]?.name && (
                        <span className="group-raw">{group.name}</span>
                      )}
                      {/* 两个标记本身就是开关：点一下恢复。做成按钮而不是纯标签，
                          是因为恢复这件事必须在这里做得到 —— 灯上右键固然也行，
                          但那个组可能已经被藏起来、根本右键不到。 */}
                      {hidden && (
                        <button
                          type="button"
                          className="badge"
                          title={m.hiddenBadgeHint}
                          onClick={() => unhide(group.name)}
                        >
                          {m.hidden}
                        </button>
                      )}
                      {muted && (
                        <button
                          type="button"
                          className="badge"
                          title={m.mutedBadgeHint}
                          onClick={() => setMuted(group.name, false)}
                        >
                          {m.muted}
                        </button>
                      )}
                      <span className="group-meta">
                        {signalLabel(lang, group.aggregate)} ·{" "}
                        {m.sessions(group.sessions.length)}
                      </span>
                      {!muted && (
                        <button
                          type="button"
                          className="btn btn-tiny"
                          title={m.muteHint}
                          onClick={() => setMuted(group.name, true)}
                        >
                          {m.mute}
                        </button>
                      )}
                    </div>
                    {/* 显示偏好：显示名 + 置顶。放在组头下面单独一行，而不是塞进
                        组头 —— 那里已经有状态点、两个标记、会话数和静音按钮了。 */}
                    <div className="group-edit">
                      <span className="muted-text">{m.displayName}</span>
                      <input
                        className="input input-tiny"
                        spellCheck={false}
                        placeholder={m.displayNamePlaceholder(group.name)}
                        value={
                          nameDraft?.group === group.name
                            ? nameDraft.value
                            : (settings.group_meta[group.name]?.name ?? "")
                        }
                        onChange={(event) =>
                          setNameDraft({ group: group.name, value: event.target.value })
                        }
                        onBlur={commitNameDraft}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") event.currentTarget.blur();
                        }}
                      />
                      <label className="pin" title={m.pinHint}>
                        <span className="muted-text">{m.pin}</span>
                        <Switch
                          on={settings.group_meta[group.name]?.order != null}
                          onChange={(on) => setPinned(group.name, on)}
                        />
                      </label>
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
          </>
        );

      case "look":
        return (
          <>
            <Row label={m.presets}>
              <div className="chips chips-tight">
                {presets(m).map((preset) => (
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
            <Row label={m.presets}>
              <div className="chips chips-tight">
                {stylePresets(m).map((preset) => (
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
                  blink !== "" && !blinkChoices.some((o) => o.value === blink)
                    ? blink
                    : null;

                return (
                  <div className="style-row" key={state} data-state={state}>
                    {/* 原生取色器。onChange 在拖动时连续触发，也就连续保存 ——
                        这是刻意的：改颜色必须能实时看到效果。 */}
                    <input
                      type="color"
                      className="style-swatch"
                      title={m.lightColor}
                      value={resolved[state]}
                      onChange={(event) =>
                        setStyle(state, { color: event.target.value })
                      }
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
                            event.target.value === ""
                              ? null
                              : Number(event.target.value),
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
          </>
        );
      }

      case "connect": {
        const endpoints: [string, string, string][] = [
          ["POST", "/v1/state", m.apiEndpointReport],
          ["GET", "/v1/state", m.apiEndpointState],
          ["GET", "/v1/sessions", m.apiEndpointSessions],
          ["DELETE", "/v1/sessions/{id}", m.apiEndpointClear],
          ["GET", "/v1/health", m.apiEndpointHealth],
        ];

        return (
          <>
            {/* ---- 接口速查 ----
                接入方最需要的那几样都在这一页：端口、token、端点表、状态词表、
                能直接粘的示例。**这一份是跟着包走的** —— 装了 .app 的人手上没有
                仓库，而 docs/ 刻意没打进 bundle（见 lib.rs 的 DOCS_URL）。 */}
            <div className="section">
              <div className="section-title">{m.apiTitle}</div>
              <div className="kv">
                <span className="k">{m.port}</span>
                <span className="mono">{endpoint?.port ?? "—"}</span>
              </div>
              <div className="kv">
                <span className="k">{m.apiToken}</span>
                <span className="mono token">{endpoint?.token ?? "—"}</span>
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

            {/* 转发搬到了「钩子」页（多条、可过滤、支持命令），这里只留入口 */}
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

      case "hooks":
        return (
          <>
            {settings.hooks.length === 0 && <p className="empty">{m.noHooks}</p>}

            {settings.hooks.map((hook, index) => {
              const target = hook.type === "http" ? hook.url : hook.command;
              const shown =
                hookDraft?.index === index ? hookDraft.value : (target ?? "");
              const configured = (target ?? "").trim().length > 0;

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
                </div>
              );
            })}

            <button type="button" className="btn" onClick={addHook}>
              {m.addHook}
            </button>
            <p className="hint">{m.hooksHint}</p>
          </>
        );

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

      {/* 顶部栏：**不随页签变**的那几个入口。
          目前是「暂停监控」和两个「打开某个目录」—— 它们是任何时候都可能想按的，
          塞进某个页签里就得先找一遍。以后要加同类的东西（检查更新、导入导出设置、
          版本号……）也放这儿，别再加一层导航。 */}
      <nav className="topbar">
        <label className="topbar-pause" title={m.pauseHint}>
          <span className="muted-text">{m.pauseMonitoring}</span>
          <Switch on={snapshot.paused} onChange={setPaused} />
        </label>
        <div className="topbar-actions">
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
      </nav>

      <div className="tabs" role="tablist">
        {tabs.map((entry) => (
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
          {m.quit}
        </button>
      </div>
    </div>
  );
}
