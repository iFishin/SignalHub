import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import { useSettings, updateSettings, type Settings, type Shape, type Palette } from "../lib/useSettings";
import {
  useToggles,
  setAutostart,
  setPassThrough,
} from "../lib/useToggles";
import { useStats } from "../lib/useStats";
import type { Snapshot } from "../lib/types";

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

type Tab = "status" | "look" | "connect" | "system";

const TABS: { id: Tab; label: string }[] = [
  { id: "status", label: "状态" },
  { id: "look", label: "外观" },
  { id: "connect", label: "接入" },
  { id: "system", label: "系统" },
];

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

  // 转发地址的编辑草稿。null 表示「没在编辑」，显示已保存的值。
  const [webhookDraft, setWebhookDraft] = useState<string | null>(null);

  const commitWebhook = () => {
    if (webhookDraft === null) return;
    const next = webhookDraft.trim();
    setWebhookDraft(null);
    if (next === (settings.webhook_url ?? "")) return; // 没改就别白存一次
    setError(null);
    updateSettings({ ...settings, webhook_url: next || null }).catch(reported);
  };

  const patch = (next: Partial<Settings>) => {
    setError(null);
    updateSettings({ ...settings, ...next }).catch(reported);
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
        return snapshot.sessions.length === 0 ? (
          <p className="empty">
            暂无会话。接入智能体或调用上报接口后，这里会列出来。
          </p>
        ) : (
          <ul className="sessions">
            {snapshot.sessions.map((session) => (
              <li key={session.id} className="session" data-state={session.state}>
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
            <Row label="外壳">
              <Switch on={settings.bezel} onChange={(on) => patch({ bezel: on })} />
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
              外壳是一圈中性细环，让它读起来像指示灯而不是普通色块。改成圆角方或环
              会让 permission / error 的方角区分失效，此时建议配合色盲友好配色。
            </p>
          </>
        );

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

            {/* 转发地址只在失焦或回车时提交 —— 每敲一个字就存盘和发 IPC 太浪费，
                而且中间态（还没敲完的 URL）会被校验拒绝。 */}
            <div className="field">
              <label htmlFor="webhook">转发到</label>
              <input
                id="webhook"
                className="input"
                type="url"
                spellCheck={false}
                placeholder="https://ntfy.sh/my-topic"
                value={webhookDraft ?? settings.webhook_url ?? ""}
                onChange={(event) => setWebhookDraft(event.target.value)}
                onBlur={commitWebhook}
                onKeyDown={(event) => {
                  if (event.key === "Enter") event.currentTarget.blur();
                }}
              />
            </div>
            <p className="hint">
              填了就把需要注意的状态 POST 到该地址，与系统通知同一个触发时机。
              留空则不转发。
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
    // 会话列表里的状态点也跟着配色走，否则面板和灯球会对不上
    <div className="settings" data-palette={settings.palette}>
      <h1 data-state={snapshot.aggregate}>
        <span className="state-dot" />
        {SIGNAL_LABEL[snapshot.aggregate]}
        {snapshot.sessions.length > 0 && (
          <span className="headline-count">{snapshot.sessions.length} 个会话</span>
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
