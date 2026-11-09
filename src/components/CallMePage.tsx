import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatAge } from "../lib/age";
import { fullLink, type CallMeSource, type CallMeState } from "../lib/callme";
import type { Messages } from "../lib/i18n";
import type { Settings } from "../lib/useSettings";
import { Icon, IconButton } from "./Icon";
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
 *
 * 链接这一页的原则：**有效链接才占位置**。吊销过的收进默认折叠的一段，能恢复
 * 也能彻底删除；名称点一下就改，用户不必在新建时猜智能体会从哪篇文章过来。
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

  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CallMeSource | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  // 改名是行内编辑：回车 / 失焦提交，Esc 取消。用 ref 记「取消」是因为
  // setRenamingId(null) 会让 input 卸载，state 更新赶不上随之而来的 blur。
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const cancelRename = useRef(false);
  // 一次只让一条链接在飞：连点两下「删除」或「新建」不该发两次。
  const [pending, setPending] = useState<string | null>(null);

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

  // 建链接不需要用户填任何东西：名称 / 出处由第一条留言带过来（见 server 的
  // `create_source` 与 `accept`），用户只该点一下按钮。`override` 是给「刚启动
  // 本机服务」用的 —— 那时父组件的 url / token 还是上一帧的值。
  const createSource = (override?: { url: string; token: string }) => {
    if (creating) return;
    setError(null);
    setCreating(true);
    invoke<CallMeSource>("callme_add_source", {
      url: override?.url ?? url,
      token: override?.token ?? token,
      label: "",
      note: null,
    })
      .then((source) => {
        setCreated(source);
        refresh();
      })
      .catch(reported)
      .finally(() => setCreating(false));
  };

  const revoke = (source: CallMeSource) => {
    if (!window.confirm(c.revokeConfirm)) return;
    setError(null);
    setPending(source.id);
    invoke("callme_revoke_source", { url, token, id: source.id })
      .then(() => {
        // 刚建出来那条的「已创建」卡片别再显示一条已经失效的链接。
        setCreated((previous) => (previous?.id === source.id ? null : previous));
        refresh();
      })
      .catch(reported)
      .finally(() => setPending(null));
  };

  const restore = (source: CallMeSource) => {
    setError(null);
    setPending(source.id);
    invoke("callme_restore_source", { url, token, id: source.id })
      .then(() => refresh())
      .catch(reported)
      .finally(() => setPending(null));
  };

  /** 彻底删除与吊销分开：只有删除才把位置让出来。 */
  const remove = (source: CallMeSource) => {
    if (!window.confirm(c.deleteConfirm)) return;
    setError(null);
    setPending(source.id);
    invoke("callme_delete_source", { url, token, id: source.id })
      .then(() => {
        // 刚建出来、还没回到 state.sources 的那条若被删了，别再把它补回列表。
        setCreated((previous) => (previous?.id === source.id ? null : previous));
        refresh();
      })
      .catch(reported)
      .finally(() => setPending(null));
  };

  const startRename = (source: CallMeSource) => {
    cancelRename.current = false;
    setRenameDraft(source.label);
    setRenamingId(source.id);
  };

  const submitRename = (source: CallMeSource) => {
    if (renamingId !== source.id) return;
    if (cancelRename.current) {
      cancelRename.current = false;
      setRenamingId(null);
      return;
    }
    const next = renameDraft.trim();
    setRenamingId(null);
    if (!next || next === source.label) return;
    setError(null);
    setPending(source.id);
    invoke("callme_rename_source", {
      url,
      token,
      id: source.id,
      label: next,
    })
      .then(() => {
        // 刚建的那条可能还没回到 state.sources（轮询 15 秒一跳），先本地补上
        // 新名字，否则改完名列表里还挂着旧的占位名。
        setCreated((previous) =>
          previous?.id === source.id
            ? { ...previous, label: next, auto_label: false }
            : previous,
        );
        refresh();
      })
      .catch(reported)
      .finally(() => setPending(null));
  };

  const openUsage = (link: string) =>
    invoke("open_url", { url: link }).catch(reported);

  /**
   * 启动本机服务之后：一条有效链接都没有的话，顺手建好第一条并切回「链接」
   * 子页。配置这个功能的动作因此只剩「点一下启动」。全都是吊销着的不算 ——
   * 那说明用户之前已经建过，该由他决定恢复还是新建。
   */
  const onLocalStarted = (started: { url: string; token: string }) => {
    if ((state?.sources ?? []).some((source) => !source.revoked) || created) return;
    createSource(started);
    setSub("links");
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
  const activeSources = shownSources.filter((source) => !source.revoked);
  const revokedSources = shownSources.filter((source) => source.revoked);

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
            {shownSources.length > 0 && activeSources.length === 0 && (
              <p className="empty">{c.activeEmpty}</p>
            )}

            <div className="callme-source-list">
              {activeSources.map((source) => {
                const link = fullLink(url || settings.callme.url, source);
                const busy = pending === source.id;
                return (
                  <div className="callme-source" key={source.id}>
                    <div className="callme-source-head">
                      {renamingId === source.id ? (
                        <input
                          className="input callme-rename-input"
                          autoFocus
                          spellCheck={false}
                          aria-label={c.renamePlaceholder}
                          placeholder={c.renamePlaceholder}
                          value={renameDraft}
                          onChange={(event) => setRenameDraft(event.target.value)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") event.currentTarget.blur();
                            if (event.key === "Escape") {
                              cancelRename.current = true;
                              event.currentTarget.blur();
                            }
                          }}
                          onBlur={() => submitRename(source)}
                        />
                      ) : (
                        <button
                          type="button"
                          className="callme-source-title callme-rename-trigger"
                          title={c.renameLink}
                          aria-label={`${c.renameLink}：${source.label}`}
                          onClick={() => startRename(source)}
                        >
                          <Icon name="pencil" size={12} />
                          <span>{source.label}</span>
                          {source.auto_label && (
                            <span className="callme-tag">{c.pendingName}</span>
                          )}
                        </button>
                      )}
                      <span className="muted-text mono">
                        {c.todayCount(source.today)} · {c.totalCount(source.total)}
                      </span>
                      <span className="callme-source-actions">
                        <button
                          type="button"
                          className="btn btn-tiny"
                          onClick={() => copy(source.id, link)}
                        >
                          {copied === source.id ? c.copiedLink : c.copyLink}
                        </button>
                        <IconButton
                          icon="external"
                          label={c.openUsage}
                          onClick={() => openUsage(link)}
                        />
                        <IconButton
                          icon="ban"
                          label={c.revoke}
                          disabled={busy}
                          onClick={() => revoke(source)}
                        />
                        <IconButton
                          icon="trash"
                          label={c.deleteLink}
                          danger
                          disabled={busy}
                          onClick={() => remove(source)}
                        />
                      </span>
                    </div>
                    {source.note && <div className="hint">{source.note}</div>}
                    <div className="callme-link mono" title={link}>
                      {link}
                    </div>
                  </div>
                );
              })}
            </div>

            {revokedSources.length > 0 && (
              <details className="callme-revoked">
                <summary>{c.revokedTitle(revokedSources.length)}</summary>
                <p className="hint">{c.revokedHint}</p>
                {revokedSources.map((source) => (
                  <div
                    className="callme-source callme-source-revoked"
                    key={source.id}
                  >
                    <div className="callme-source-head">
                      <span className="callme-source-title muted-text">
                        {source.label}
                      </span>
                      <span className="muted-text mono">
                        {c.totalCount(source.total)}
                      </span>
                      <span className="callme-source-actions">
                        <IconButton
                          icon="undo"
                          label={c.restoreLink}
                          disabled={pending === source.id}
                          onClick={() => restore(source)}
                        />
                        <IconButton
                          icon="trash"
                          label={c.deleteLink}
                          danger
                          disabled={pending === source.id}
                          onClick={() => remove(source)}
                        />
                      </span>
                    </div>
                  </div>
                ))}
              </details>
            )}

            <div className="callme-new">
              <button
                type="button"
                className="btn btn-tiny"
                disabled={creating || !url.trim() || !token.trim()}
                onClick={() => createSource()}
              >
                <Icon name="plus" size={12} />
                {creating ? c.creating : c.addSource}
              </button>
              <span className="hint">{c.autoNameHint}</span>
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
            onLocalStarted={onLocalStarted}
          />
        </LocalPanel>
      )}

      {error && <p className="hook-warn">{error}</p>}
    </>
  );
}
