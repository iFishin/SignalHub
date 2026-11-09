import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatAge } from "../lib/age";
import {
  filterBySource,
  lastSeqOf,
  newestFirst,
  openableUrl,
  readOf,
  unreadBySource,
  unreadCount,
  type CallMeMessage,
  type CallMeState,
} from "../lib/callme";
import type { Messages } from "../lib/i18n";
import { PageHeader } from "./settings/SettingsPage";

/**
 * 工作台的「留言」页：只负责阅读。
 *
 * 从原「Call Me」页拆出来的高频那一段 —— 以前每次看留言都要先经过服务器配置
 * 和链接管理。链接与服务器归 `CallMePage`（连接 → Call Me），这页只拿拉取状态，
 * 不碰设置；空态里的两个按钮负责把人送回配置页的对应子页。
 */

interface Props {
  m: Messages;
  state: CallMeState | null;
  refresh: () => void;
  /** 「去哪配置」：跳到 Call Me 页的某个子页。 */
  goCallMe: (sub: "links" | "server") => void;
  /** 「多久以前」由面板壳里那只在显示本页时才走的钟提供（见 SettingsPanel）。 */
  now: number;
}

export function CallMeMessages({ m, state, refresh, goCallMe, now }: Props) {
  const c = m.callme;
  const [filter, setFilter] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const [copied, setCopied] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reported = (err: unknown) => setError(String(err));

  const sources = state?.sources ?? [];
  const read = state?.read ?? {};
  const messages = newestFirst(state?.messages ?? []);
  const shown = filterBySource(messages, filter);
  const unread = unreadBySource(messages, read);
  const totalUnread = unreadCount(messages, read);
  // 来源被彻底删除后旧留言仍在（服务器保留 append-only 的消息），但名字找不到
  // 了。显示「已删除的链接」比把 8 位内部 id 摊给用户好。
  const labelOf = (sourceId: string) =>
    sources.find((source) => source.id === sourceId)?.label ?? c.deletedSource;

  const copy = (what: string, text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(what);
        window.setTimeout(() => setCopied(null), 1500);
      })
      .catch(reported);
  };

  const markRead = (message: CallMeMessage) => {
    if (message.seq <= readOf(read, message.source_id)) return;
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

  const markAllRead = () => {
    const scope = filter ? sources.filter((source) => source.id === filter) : sources;
    const pending = scope
      .map((source) => ({ source, seq: lastSeqOf(messages, source.id) }))
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

  // 空态按原因区分：没服务器 / 连不上 / 没链接 / 没留言 / 筛选后没有，各自给出路。
  const emptyBody = () => {
    if (!state?.enabled) {
      return (
        <>
          <p className="empty">{c.messagesEmptyNoServer}</p>
          <button type="button" className="btn btn-tiny" onClick={() => goCallMe("server")}>
            {c.goConfigure}
          </button>
        </>
      );
    }
    // 配了但连不上：这时 `sources` 必然是空的，若按「没有链接」去解释，
    // 用户会去新建一条注定失败的链接。先说清是连不上。
    if (!state.ok && state.error) {
      return (
        <>
          <p className="empty">{c.messagesEmptyOffline}</p>
          <p className="hint mono">{state.error}</p>
          <button type="button" className="btn btn-tiny" onClick={() => goCallMe("server")}>
            {c.checkServer}
          </button>
        </>
      );
    }
    if (sources.length === 0) {
      return (
        <>
          <p className="empty">{c.messagesEmptyNoSource}</p>
          <button type="button" className="btn btn-tiny" onClick={() => goCallMe("links")}>
            {c.goCreateLink}
          </button>
        </>
      );
    }
    return <p className="empty">{filter ? c.filterNoResults : c.messagesEmpty}</p>;
  };

  return (
    <>
      <PageHeader title={m.tabMessages} hint={m.pageHint.messages}>
        <button
          type="button"
          className="btn btn-tiny"
          onClick={() => goCallMe("links")}
        >
          {c.manageLinks}
        </button>
        {totalUnread > 0 && (
          <button type="button" className="btn btn-tiny" onClick={markAllRead}>
            {c.markAllRead}
          </button>
        )}
      </PageHeader>

      <div className="section section-flush">
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
          emptyBody()
        ) : (
          shown.map((message) => {
            const isUnread = message.seq > readOf(read, message.source_id);
            const isOpen = !!expanded[message.seq];
            const from = openableUrl(message.context);
            return (
              <article className="callme-msg" data-unread={isUnread} key={message.id}>
                <button
                  type="button"
                  className="callme-msg-head"
                  aria-expanded={isOpen}
                  onClick={() => toggle(message)}
                >
                  <span className="callme-dot" aria-hidden="true" />
                  <span className="callme-msg-source">{labelOf(message.source_id)}</span>
                  {message.agent && <span className="muted-text">{message.agent}</span>}
                  <span className="callme-msg-age mono">
                    {formatAge(message.received_ms, now)}
                  </span>
                </button>
                {message.title && <div className="callme-msg-title">{message.title}</div>}
                <div className="callme-msg-body" data-open={isOpen}>
                  {message.text}
                </div>
                {isOpen && (
                  <div className="row-start callme-msg-actions">
                    {from && (
                      <button
                        type="button"
                        className="btn btn-tiny"
                        onClick={() => invoke("open_url", { url: from }).catch(reported)}
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
                      {copied === `text-${message.seq}` ? c.copiedText : c.copyText}
                    </button>
                  </div>
                )}
              </article>
            );
          })
        )}
      </div>

      {error && <p className="hook-warn">{error}</p>}
    </>
  );
}
