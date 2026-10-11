import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import { useSettings, updateSettings, type Settings, type Shape, type Palette, type Hook } from "../lib/useSettings";
import {
  useToggles,
  setAutostart,
  setPassThrough,
} from "../lib/useToggles";
import { useStats } from "../lib/useStats";
import { colorVars, resolveColors } from "../lib/palettes";
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

type Tab = "status" | "look" | "colors" | "connect" | "hooks" | "system";

const TABS: { id: Tab; label: string }[] = [
  { id: "status", label: "状态" },
  { id: "look", label: "外观" },
  { id: "colors", label: "颜色" },
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

/** 把秒数写成人看的时长：45s / 12m / 2h14m。 */
function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h${rest}m`;
}

export function SettingsPanel({ snapshot }: Props) {
  const settings = useSettings();
  const toggles = useToggles();
  const stats = useStats();
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
        return snapshot.groups.length === 0 ? (
          <p className="empty">
            暂无会话。接入智能体或调用上报接口后，这里会列出来。
          </p>
        ) : (
          <>
            {snapshot.groups.map((group) => (
              <div className="group" key={group.name}>
                <div className="group-head" data-state={group.aggregate}>
                  <span className="state-dot" />
                  <span className="group-name">{group.name}</span>
                  <span className="group-meta">
                    {SIGNAL_LABEL[group.aggregate]} · {group.sessions.length} 个会话
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
            ))}
          </>
        );

      case "look":
        return (
          <>
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
            <Row label="配色">
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
            <Row label="保持在最前">
              <Switch
                on={settings.always_on_top}
                onChange={(on) => patch({ always_on_top: on })}
              />
            </Row>
            <Row label="安静模式">
              <Switch on={settings.quiet} onChange={(on) => patch({ quiet: on })} />
            </Row>
            <p className="hint">
              形状只影响牌里那颗灯。改成圆角方或环会让 permission / error 的方角区分
              失效，此时建议配合色盲友好配色。
            </p>
          </>
        );

      case "colors": {
        const resolved = resolveColors(settings);
        const overridden = Object.keys(settings.colors).length;
        return (
          <>
            <div className="colors">
              {SIGNALS.map((state) => (
                <label key={state} className="color-row" data-state={state}>
                  <span className="state-dot" />
                  <span className="color-name">{SIGNAL_LABEL[state]}</span>
                  {/* 原生取色器。onChange 在拖动时连续触发，也就连续保存 ——
                      这是刻意的：改颜色必须能实时看到效果，而 set_settings
                      只是一次很小的 IPC + 写文件。 */}
                  <input
                    type="color"
                    value={resolved[state]}
                    onChange={(event) =>
                      patch({
                        colors: { ...settings.colors, [state]: event.target.value },
                      })
                    }
                  />
                </label>
              ))}
            </div>
            <div className="row" style={{ marginTop: 10 }}>
              <span className="muted-text">
                {overridden > 0 ? `已自定义 ${overridden} 项` : "全部使用配色预设"}
              </span>
              <button
                type="button"
                className="btn"
                disabled={overridden === 0}
                onClick={() => patch({ colors: {} })}
              >
                恢复默认
              </button>
            </div>
            <p className="hint">
              只覆盖你改过的状态，其余仍用「外观」页选的配色。描边和细环会跟着
              填充色自动派生，不用单独调。
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
            <Row label="需要时弹通知">
              <Switch on={settings.notify} onChange={(on) => patch({ notify: on })} />
            </Row>
            <p className="hint">
              需要授权、需要关注或出错时弹一条系统通知。
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
