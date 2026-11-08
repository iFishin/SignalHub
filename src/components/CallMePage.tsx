import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatAge } from "../lib/age";
import {
  filterBySource,
  fullLink,
  lastSeqOf,
  newestFirst,
  openableUrl,
  readOf,
  unreadBySource,
  unreadCount,
  type CallMeMessage,
  type CallMeProbe,
  type CallMeSource,
  type CallMeState,
} from "../lib/callme";
import type { Messages } from "../lib/i18n";
import type { Settings } from "../lib/useSettings";
import { useNow } from "../lib/useNow";

/**
 * 设置面板的「Call Me」页。
 *
 * 三节：服务器（地址 / token / 间隔 / 测试）→ 来源（每条链接的额度与吊销）→
 * 留言（按来源筛选、已读、打开出处）。
 *
 * 单独成文件而不是塞进 `SettingsPanel`：那个文件已经 3900 多行，多机那种规模的
 * 内联再来一份会把它推过 4500。这个组件只拿 `settings` / `patch` / 文案 / 拉取
 * 状态，不碰窗口和事件。
 */

/** 轮询间隔的档位（秒）。默认 15 秒，最多 5 分钟 —— 留言不是实时信号。 */
const POLL_CHOICES = [5, 15, 30, 60, 300];

interface Props {
  settings: Settings;
  patch: (next: Partial<Settings>) => void;
  m: Messages;
  state: CallMeState | null;
  refresh: () => void;
}

export function CallMePage({ settings, patch, m, state, refresh }: Props) {
  const now = useNow(true);
  const c = m.callme;

  // 地址 / token 用「草稿 + 失焦提交」：每敲一个字符就写一次 config.json
  // 既浪费又会让轮询线程中途读到半截地址（会去连一个不存在的域名）。
  const [url, setUrl] = useState(settings.callme.url);
  const [token, setToken] = useState(settings.callme.token);
  const [probe, setProbe] = useState<CallMeProbe | "testing" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [newLabel, setNewLabel] = useState("");
  const [newNote, setNewNote] = useState("");
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CallMeSource | null>(null);

  const [filter, setFilter] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
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

  const markRead = (message: CallMeMessage) => {
    if (message.seq <= readOf(state?.read ?? {}, message.source_id)) return;
    invoke("callme_mark_read", { sourceId: message.source_id, seq: message.seq })
      .then(() => refresh())
      .catch(reported);
  };

  const toggle = (message: CallMeMessage) => {
    const next = !expanded[message.seq];
    setExpanded((prev) => ({ ...prev, [message.seq]: next }));
    // 展开即「读到了」：点开一条还要再点一次「标为已读」是多余的一步。
    if (next) markRead(message);
  };

  const sources = state?.sources ?? [];
  const read = state?.read ?? {};
  const messages = newestFirst(state?.messages ?? []);
  const shown = filterBySource(messages, filter);
  const unread = unreadBySource(messages, read);
  const totalUnread = unreadCount(messages, read);
  const labelOf = (sourceId: string) =>
    sources.find((source) => source.id === sourceId)?.label ?? sourceId;

  const markAllRead = () => {
    const scope = filter ? sources.filter((source) => source.id === filter) : sources;
    const pending = scope
      .map((source) => ({
        source,
        seq: lastSeqOf(messages, source.id),
      }))
      .filter(({ source, seq }) => seq > readOf(read, source.id));
    if (pending.length === 0) return;
    Promise.all(
      pending.map(({ source, seq }) =>
        invoke("callme_mark_read", { sourceId: source.id, seq }),
      ),
    )
      .then(() => refresh())
      .catch(reported);
  };

  // 「测试」优先于轮询线程给的上一拍结果：那是用户刚点的，必须立刻可见。
  const status =
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

  const emptyMessages = !state?.enabled
    ? c.messagesEmptyNoServer
    : sources.length === 0
      ? c.messagesEmptyNoSource
      : c.messagesEmpty;

  // 刚建出来的那条可能还没回到 `state.sources` 里（轮询是 15 秒一跳），先补在
  // 列表最前面，用户复制完链接就能直接去贴。
  const shownSources =
    created && !sources.some((source) => source.id === created.id)
      ? [created, ...sources]
      : sources;

  return (
    <>
      <p className="hint">{c.hint}</p>

      {/* ---- ① 服务器 ---- */}
      <div className="section">
        <div className="section-title">{c.serverTitle}</div>
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
              onChange={(event) =>
                commit({ poll_ms: Number(event.target.value) })
              }
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
          <span className="hook-result mono" data-ok={statusOk}>
            {status}
          </span>
        </div>
        {error && <p className="hook-warn">{error}</p>}
      </div>

      {/* ---- ② 来源 ---- */}
      <div className="section">
        <div className="section-title">{c.sourcesTitle}</div>
        {shownSources.length === 0 && (
          <p className="empty">{c.sourcesEmpty}</p>
        )}

        {shownSources.map((source) => {
          const link = fullLink(url || settings.callme.url, source);
          return (
            <div className="callme-source" key={source.id}>
              <div className="callme-source-head">
                <span className="callme-source-title">
                  {source.label}
                  {source.revoked && (
                    <span className="callme-tag">{c.revoked}</span>
                  )}
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

      {/* ---- ③ 留言 ---- */}
      <div className="section">
        <div className="section-title">
          {c.messagesTitle}
          {totalUnread > 0 && (
            <span className="callme-count">{c.unread(totalUnread)}</span>
          )}
        </div>

        <div className="chips">
          <button
            type="button"
            className="chip"
            data-active={filter === null}
            onClick={() => setFilter(null)}
          >
            {c.filterAll}
          </button>
          {sources.map((source) => (
            <button
              type="button"
              className="chip"
              key={source.id}
              data-active={filter === source.id}
              onClick={() => setFilter(source.id)}
            >
              {source.label}
              {(unread[source.id] ?? 0) > 0 && ` ${unread[source.id]}`}
            </button>
          ))}
        </div>

        {shown.length === 0 ? (
          <p className="empty">{emptyMessages}</p>
        ) : (
          shown.map((message) => {
            const isUnread =
              message.seq > readOf(read, message.source_id);
            const isOpen = !!expanded[message.seq];
            const from = openableUrl(message.context);
            return (
              <article
                className="callme-msg"
                data-unread={isUnread}
                key={message.id}
              >
                <button
                  type="button"
                  className="callme-msg-head"
                  aria-expanded={isOpen}
                  onClick={() => toggle(message)}
                >
                  <span className="callme-dot" aria-hidden="true" />
                  <span className="callme-msg-source">
                    {labelOf(message.source_id)}
                  </span>
                  {message.agent && (
                    <span className="muted-text">{message.agent}</span>
                  )}
                  <span className="callme-msg-age mono">
                    {formatAge(message.received_ms, now)}
                  </span>
                </button>
                {message.title && (
                  <div className="callme-msg-title">{message.title}</div>
                )}
                <div className="callme-msg-body" data-open={isOpen}>
                  {message.text}
                </div>
                {isOpen && (
                  <div className="row-start callme-msg-actions">
                    {from && (
                      <button
                        type="button"
                        className="btn btn-tiny"
                        onClick={() =>
                          invoke("open_url", { url: from }).catch(reported)
                        }
                      >
                        {c.openSource}
                      </button>
                    )}
                    {message.context && !from && (
                      <span className="muted-text">{message.context}</span>
                    )}
                    <button
                      type="button"
                      className="btn btn-tiny"
                      onClick={() => copy(`text-${message.seq}`, message.text)}
                    >
                      {copied === `text-${message.seq}`
                        ? c.copiedText
                        : c.copyText}
                    </button>
                  </div>
                )}
              </article>
            );
          })
        )}

        {totalUnread > 0 && (
          <button type="button" className="btn btn-tiny" onClick={markAllRead}>
            {c.markAllRead}
          </button>
        )}
      </div>
    </>
  );
}
