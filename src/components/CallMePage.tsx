import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatAge } from "../lib/age";
import { fullLink, type CallMeSource, type CallMeState } from "../lib/callme";
import type { Messages } from "../lib/i18n";
import type { Settings } from "../lib/useSettings";
import { CallMeServer } from "./CallMeServer";
import { LocalPanel, LocalTabs, PageHeader } from "./settings/SettingsPage";

/**
 * 连接 →「Call Me」页：只负责配置和资源管理。
 *
 * 阅读那一段拆去了工作台的「留言」页（CallMeMessages）；这里留两个局部子页：
 * 链接（默认 —— 已配置的人大多数时候只来复制一条链接）和服务器。
 *
 * 地址 / token 的草稿放在这层而不是子页里：切子页不丢草稿（子页只切显示，
 * 组件不卸载），新建链接用的也一直是同一份值。
 */

export type CallMeSub = "links" | "server";

interface Props {
  settings: Settings;
  patch: (next: Partial<Settings>) => void;
  m: Messages;
  state: CallMeState | null;
  refresh: () => void;
  /** 子页由面板壳持有：留言页的「去配置 / 去新建链接」要能跳进来并落到指定子页。 */
  sub: CallMeSub;
  setSub: (next: CallMeSub) => void;
  /** 摘要行里「上次成功 X 分钟前」用的钟，由面板壳按可见性控制。 */
  now: number;
}

export function CallMePage({
  settings,
  patch,
  m,
  state,
  refresh,
  sub,
  setSub,
  now,
}: Props) {
  const c = m.callme;

  // 地址 / token 用「草稿 + 失焦提交」：每敲一个字符就写一次 config.json
  // 既浪费又会让轮询线程中途读到半截地址（会去连一个不存在的域名）。
  const [url, setUrl] = useState(settings.callme.url);
  const [token, setToken] = useState(settings.callme.token);
  const [error, setError] = useState<string | null>(null);

  const [newLabel, setNewLabel] = useState("");
  const [newNote, setNewNote] = useState("");
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CallMeSource | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const reported = (err: unknown) => setError(String(err));

  const commit = (next: Partial<Settings["callme"]>) =>
    patch({ callme: { ...settings.callme, ...next } });

  const copy = (what: string, text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(what);
        window.setTimeout(() => setCopied(null), 1500);
      })
      .catch(reported);
  };

  const createSource = () => {
    if (!newLabel.trim() || creating) return;
    setError(null);
    setCreating(true);
    invoke<CallMeSource>("callme_add_source", {
      url,
      token,
      label: newLabel.trim(),
      note: newNote.trim() || null,
    })
      .then((source) => {
        setCreated(source);
        setNewLabel("");
        setNewNote("");
        refresh();
      })
      .catch(reported)
      .finally(() => setCreating(false));
  };

  const revoke = (source: CallMeSource) => {
    if (!window.confirm(c.revokeConfirm)) return;
    setError(null);
    invoke("callme_revoke_source", { url, token, id: source.id })
      .then(() => refresh())
      .catch(reported);
  };

  const sources = state?.sources ?? [];

  // 链接子页顶部的一行连接摘要：已连接 / 未配置 / 错误。答案来自轮询线程，
  // 不是这里现探的 —— 要看「现在通不通」去服务器子页点「测试」。
  const summary = state?.error
    ? state.error
    : state?.ok
      ? c.lastOk(formatAge(state.last_ok_ms, now))
      : state?.enabled
        ? c.never
        : c.unconfigured;
  const summaryOk = state?.ok ?? false;

  // 刚建出来的那条可能还没回到 `state.sources` 里（轮询是 15 秒一跳），先补在
  // 列表最前面，用户复制完链接就能直接去贴。
  const shownSources =
    created && !sources.some((source) => source.id === created.id)
      ? [created, ...sources]
      : sources;

  return (
    <>
      <PageHeader title={m.tabCallMe} hint={m.pageHint.callme} />
      <LocalTabs
        label={m.tabCallMe}
        value={sub}
        onChange={setSub}
        items={[
          { id: "links" as const, label: c.linksTab },
          { id: "server" as const, label: c.serverTab },
        ]}
      />

      {sub === "links" && (
        <LocalPanel id="links">
          <div className="section section-flush">
            <div className="row-start callme-toolbar">
              <span className="hook-result mono" data-ok={summaryOk}>
                {summary}
              </span>
              <button
                type="button"
                className="btn btn-tiny"
                onClick={() => setSub("server")}
              >
                {c.openServerTab}
              </button>
            </div>
          </div>

          <div className="section">
            <div className="section-title">{c.sourcesTitle}</div>
            {shownSources.length === 0 && <p className="empty">{c.sourcesEmpty}</p>}

            {shownSources.map((source) => {
              const link = fullLink(url || settings.callme.url, source);
              return (
                <div className="callme-source" key={source.id}>
                  <div className="callme-source-head">
                    <span className="callme-source-title">
                      {source.label}
                      {source.revoked && <span className="callme-tag">{c.revoked}</span>}
                    </span>
                    <span className="muted-text mono">
                      {c.todayCount(source.today)} · {c.totalCount(source.total)}
                    </span>
                    <button
                      type="button"
                      className="btn btn-tiny"
                      onClick={() => copy(source.id, link)}
                    >
                      {copied === source.id ? c.copiedLink : c.copyLink}
                    </button>
                    {!source.revoked && (
                      <button
                        type="button"
                        className="btn btn-tiny"
                        onClick={() => revoke(source)}
                      >
                        {c.revoke}
                      </button>
                    )}
                  </div>
                  {source.note && <div className="hint">{source.note}</div>}
                  <div className="callme-link mono" title={link}>
                    {link}
                  </div>
                </div>
              );
            })}

            <div className="callme-new">
              <input
                className="input"
                spellCheck={false}
                placeholder={c.labelPlaceholder}
                aria-label={c.labelPlaceholder}
                value={newLabel}
                onChange={(event) => setNewLabel(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") createSource();
                }}
              />
              <input
                className="input"
                spellCheck={false}
                placeholder={c.notePlaceholder}
                aria-label={c.notePlaceholder}
                value={newNote}
                onChange={(event) => setNewNote(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") createSource();
                }}
              />
              <button
                type="button"
                className="btn btn-tiny"
                disabled={!newLabel.trim() || creating || !url.trim() || !token.trim()}
                onClick={createSource}
              >
                {creating ? c.creating : c.addSource}
              </button>
            </div>

            {created && (
              <div className="callme-created">
                <div className="section-title">{c.createdTitle}</div>
                <p className="hint">{c.createdHint}</p>
                <div className="callme-link mono">
                  {fullLink(url || settings.callme.url, created)}
                </div>
                <button
                  type="button"
                  className="btn btn-tiny"
                  onClick={() =>
                    copy("created", fullLink(url || settings.callme.url, created))
                  }
                >
                  {copied === "created" ? c.copiedLink : c.copyLink}
                </button>
              </div>
            )}
          </div>
        </LocalPanel>
      )}

      {sub === "server" && (
        <LocalPanel id="server">
          <CallMeServer
            settings={settings}
            url={url}
            token={token}
            setUrl={setUrl}
            setToken={setToken}
            commit={commit}
            m={m}
            state={state}
            now={now}
          />
        </LocalPanel>
      )}

      {error && <p className="hook-warn">{error}</p>}
    </>
  );
}
