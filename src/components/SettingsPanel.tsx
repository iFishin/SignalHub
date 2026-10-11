import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { SIGNAL_LABEL } from "../lib/signals";
import { useSettings, updateSettings, type Settings } from "../lib/useSettings";
import type { Snapshot } from "../lib/types";

/** 与 Rust 侧 config.rs 的 MIN_SIZE / MAX_SIZE / DEFAULT_SIZE 保持一致。 */
const SIZES = [
  { label: "小", value: 16 },
  { label: "中", value: 22 },
  { label: "大", value: 32 },
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

export function SettingsPanel({ snapshot }: Props) {
  const settings = useSettings();
  const [endpoint, setEndpoint] = useState<Endpoint | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    invoke<Endpoint>("get_endpoint").then(setEndpoint).catch(() => {
      // 端口/token 读不到就是「服务还没起来」，界面上显示为 — 即可
    });
  }, []);

  const reported = (err: unknown) => setError(String(err));

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

  return (
    <div className="settings">
      <h1 data-state={snapshot.aggregate}>
        <span className="state-dot" />
        {SIGNAL_LABEL[snapshot.aggregate]}
      </h1>
      <p className="headline-message">
        {snapshot.paused
          ? "监控已暂停：灯球显示熄灭，但会话仍在记录"
          : (snapshot.message ?? `${snapshot.sessions.length} 个会话`)}
      </p>

      <div className="section">
        <div className="section-title">灯球</div>
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
        <Row label="保持在最前">
          <Switch
            on={settings.always_on_top}
            onChange={(on) => patch({ always_on_top: on })}
          />
        </Row>
        <Row label="安静模式">
          <Switch on={settings.quiet} onChange={(on) => patch({ quiet: on })} />
        </Row>
        <p className="hint">空闲或沉默时圆点几乎透明，悬停恢复。</p>
      </div>

      <div className="section">
        <div className="section-title">监控</div>
        <Row label="暂停监控">
          <Switch on={snapshot.paused} onChange={setPaused} />
        </Row>
        <Row label="需要时弹通知">
          <Switch on={settings.notify} onChange={(on) => patch({ notify: on })} />
        </Row>
        <p className="hint">需要授权、需要关注或出错时弹一条系统通知。</p>
      </div>

      <div className="section">
        <div className="section-title">接入</div>
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
      </div>

      <div className="section">
        <div className="section-title">会话</div>
        {snapshot.sessions.length === 0 ? (
          <p className="empty" style={{ margin: 0 }}>
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
        )}
      </div>

      {error && <p className="error">{error}</p>}

      <div className="section">
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
