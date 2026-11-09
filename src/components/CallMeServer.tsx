import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatAge } from "../lib/age";
import type {
  CallMeLocalStatus,
  CallMeProbe,
  CallMeState,
} from "../lib/callme";
import type { Messages } from "../lib/i18n";
import type { Settings } from "../lib/useSettings";

const POLL_CHOICES = [5, 15, 30, 60, 300];
const DEFAULT_LOCAL_PORT = 43877;

/** 已填的地址是不是「就是这台机器」。只用来决定要不要拦一下确认。 */
function isLoopback(url: string): boolean {
  return /^(https?:\/\/)?(127\.0\.0\.1|localhost|\[::1\])([:/]|$)/i.test(url.trim());
}

const REMOTE_COMMAND = `git clone https://github.com/iFishin/SignalHub.git
cd SignalHub
docker build -t signalhub-callme ./server
read -rsp "CALLME_ADMIN_TOKEN: " CALLME_ADMIN_TOKEN && echo
docker run -d --name signalhub-callme --restart unless-stopped \\
  -p 127.0.0.1:8080:8080 \\
  -e CALLME_ADMIN_TOKEN -e CALLME_TRUST_PROXY=true \\
  -v callme-data:/data signalhub-callme`;

interface Props {
  settings: Settings;
  url: string;
  token: string;
  setUrl: (value: string) => void;
  setToken: (value: string) => void;
  commit: (next: Partial<Settings["callme"]>) => void;
  m: Messages;
  state: CallMeState | null;
  /** 「上次成功 X 分钟前」用的钟，由 CallMePage 传入（面板壳控制走不走）。 */
  now: number;
  /** 本机服务启动成功后回传新地址与 token：父组件据此自动建第一条链接。 */
  onLocalStarted: (started: { url: string; token: string }) => void;
}

/**
 * Call Me 的服务器页。
 *
 * **常态只做一件事：把服务开起来。** 地址、token、端口、检查间隔和 VPS 部署命令
 * 都在折叠里 —— 这个功能的使用者是智能体，用户不该为了贴一条链接去配四个字段。
 * 折起来而不是删掉：连自己 VPS、换端口、调轮询是真实需求，只是低频。
 */
export function CallMeServer({
  settings,
  url,
  token,
  setUrl,
  setToken,
  commit,
  m,
  state,
  now,
  onLocalStarted,
}: Props) {
  const c = m.callme;
  const [probe, setProbe] = useState<CallMeProbe | "testing" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [local, setLocal] = useState<CallMeLocalStatus | null>(null);
  const [localBusy, setLocalBusy] = useState<"start" | "stop" | null>(null);
  const [port, setPort] = useState(String(DEFAULT_LOCAL_PORT));
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    invoke<CallMeLocalStatus>("callme_local_status")
      .then((status) => {
        setLocal(status);
        if (status.port > 0) setPort(String(status.port));
      })
      .catch((err) => setError(String(err)));
  }, []);

  const reported = (err: unknown) => setError(String(err));

  const runProbe = () => {
    setError(null);
    setProbe("testing");
    invoke<CallMeProbe>("callme_probe", { url, token })
      .then(setProbe)
      .catch((err) => {
        setProbe(null);
        reported(err);
      });
  };

  const startLocal = () => {
    const parsed = Number(port);
    if (!Number.isInteger(parsed) || parsed < 1024 || parsed > 65535) {
      setError(c.localPortInvalid);
      return;
    }
    // 启动会**改写**设置里的地址与 token。先把话说明白：已经填了公网地址的人
    // 点一下就把 VPS 的配置换掉，是很糟糕的意外。
    if (url.trim() && !isLoopback(url)) {
      if (!window.confirm(c.localReplaceConfirm)) return;
    }
    setError(null);
    setLocalBusy("start");
    invoke<CallMeLocalStatus>("callme_local_start", { port: parsed })
      .then((status) => {
        setLocal(status);
        if (status.url && status.token) {
          // 启动成功才改设置：端口被占用时不能把一份连不上的配置写进去。
          setUrl(status.url);
          setToken(status.token);
          commit({ url: status.url, token: status.token });
          setProbe(null);
          // 地址是刚生成的，父组件那一帧还拿着旧值，所以显式回传。
          onLocalStarted({ url: status.url, token: status.token });
        }
      })
      .catch(reported)
      .finally(() => setLocalBusy(null));
  };

  const stopLocal = () => {
    setError(null);
    setLocalBusy("stop");
    invoke<CallMeLocalStatus>("callme_local_stop")
      .then(setLocal)
      .catch(reported)
      .finally(() => setLocalBusy(null));
  };

  const copyRemote = () => {
    navigator.clipboard
      .writeText(REMOTE_COMMAND)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      })
      .catch(reported);
  };

  const statusText =
    probe === "testing"
      ? c.testing
      : probe
        ? probe.ok
          ? c.connected(probe.latency_ms)
          : probe.error
        : state?.error
          ? state.error
          : state?.ok
            ? c.lastOk(formatAge(state.last_ok_ms, now))
            : state?.enabled
              ? c.never
              : c.unconfigured;
  const statusOk = probe && probe !== "testing" ? probe.ok : (state?.ok ?? false);

  return (
    <>
      <div className="section section-flush">
        <div className="section-title">{c.localTitle}</div>
        <p className="hint">{c.localHint}</p>
        <div className="row-start callme-toolbar">
          {local?.running ? (
            <button
              type="button"
              className="btn btn-tiny"
              disabled={localBusy !== null}
              onClick={stopLocal}
            >
              {localBusy === "stop" ? c.localStopping : c.localStop}
            </button>
          ) : (
            <button
              type="button"
              className="btn"
              disabled={localBusy !== null}
              onClick={startLocal}
            >
              {localBusy === "start" ? c.localStarting : c.localStart}
            </button>
          )}
          <span className="hook-result mono" data-ok={local?.running ?? false}>
            {local?.running ? c.localRunning : c.localStopped}
          </span>
          <span className="hook-result mono" data-ok={statusOk}>
            {statusText}
          </span>
        </div>
        {local?.url && <div className="callme-link mono">{local.url}</div>}
        {local?.data_dir && <div className="hint mono">{local.data_dir}</div>}
        <p className="hint">{c.localOnlyHint}</p>
        {local?.error && <p className="hook-warn">{local.error}</p>}
      </div>

      <details className="callme-advanced">
        <summary>{c.advancedTitle}</summary>
        <div className="section">
          <p className="hint">{c.connectionHint}</p>
          <label className="callme-poll">
            <span className="muted-text">{c.localPort}</span>
            <input
              className="input input-tiny mono"
              inputMode="numeric"
              aria-label={c.localPort}
              value={port}
              disabled={local?.running || localBusy !== null}
              onChange={(event) => setPort(event.target.value)}
            />
          </label>
          <input
            className="input mono"
            spellCheck={false}
            placeholder={c.urlPlaceholder}
            aria-label={c.urlLabel}
            title={c.urlHint}
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            onBlur={() => {
              const next = url.trim();
              setUrl(next);
              if (next !== settings.callme.url) commit({ url: next });
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
          <input
            className="input mono"
            spellCheck={false}
            type="password"
            placeholder={c.tokenPlaceholder}
            aria-label={c.tokenLabel}
            title={c.tokenHint}
            value={token}
            onChange={(event) => setToken(event.target.value)}
            onBlur={() => {
              const next = token.trim();
              setToken(next);
              if (next !== settings.callme.token) commit({ token: next });
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
          <div className="row-start callme-toolbar">
            <label className="callme-poll">
              <span className="muted-text">{c.pollLabel}</span>
              <select
                className="select"
                value={settings.callme.poll_ms}
                onChange={(event) => commit({ poll_ms: Number(event.target.value) })}
              >
                {POLL_CHOICES.map((seconds) => (
                  <option key={seconds} value={seconds * 1000}>
                    {c.pollSeconds(seconds)}
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="btn btn-tiny"
              disabled={!url.trim() || !token.trim() || probe === "testing"}
              onClick={runProbe}
            >
              {probe === "testing" ? c.testing : c.test}
            </button>
          </div>
        </div>
      </details>

      <details className="callme-advanced">
        <summary>{c.remoteTitle}</summary>
        <div className="section">
          <p className="hint">{c.remoteHint}</p>
          <ol className="deploy-steps">
            <li>{c.remoteStep1}</li>
            <li>{c.remoteStep2}</li>
            <li>{c.remoteStep3}</li>
          </ol>
          <pre className="code deploy-command mono">{REMOTE_COMMAND}</pre>
          <button type="button" className="btn btn-tiny" onClick={copyRemote}>
            {copied ? c.copiedCommand : c.copyCommand}
          </button>
          <p className="hint">{c.remoteSecurityHint}</p>
        </div>
      </details>

      {error && <p className="hook-warn">{error}</p>}
    </>
  );
}
