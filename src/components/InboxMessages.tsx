import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatAge } from "../lib/age";
import {
  lastSeqOf,
  newestFirst,
  openableUrl,
  readOf,
  unreadCount,
  type CallMeMessage,
  type CallMeState,
} from "../lib/callme";
import type { Messages } from "../lib/i18n";

/**
 * 面板里的「留言」页签：只负责读。
 *
 * 和设置面板里的「留言」页（`CallMeMessages`）是**同一份数据、两种密度**。设置窗
 * 有 1000px 宽，能放来源筛选、大标题和卡片；面板只有 420px、还常驻在屏幕角落，
 * 所以这里砍成一列「谁发来的 · 多久以前 · 说了什么」，来源名放在行里而不是
 * 一排 chips 上 —— 筛选是「整理」的动作，而这个面板回答的是「刚才有人找我吗」。
 *
 * 不自己订阅数据：`useCallMe` 由面板壳持有（页签上的未读数也要用它），这里只收
 * 一份 state，两个地方就不会各自记一份「读到第几条」。
 */

interface Props {
  m: Messages;
  state: CallMeState | null;
  refresh: () => void;
  /** 「多久以前」由面板壳那只钟提供，和其它页签用的是同一个 now。 */
  now: number;
  /** 面板里没有配置界面，空态把人送回设置。 */
  onConfigure: () => void;
  /**
   * 演示模式：**只画，不动真东西**。标记已读 / 全部已读 / 复制 / 打开来源这些
   * 动作一概不执行（它们会改这台机器的状态或调起系统）。展开收起是纯本地
   * state，留着无妨 —— 演示里能点开一条看看，比一张死图好懂。
   */
  preview?: boolean;
  /** 演示里初始展开哪一条（seq）。真面板永远是 null，由用户自己点开。 */
  initialOpenSeq?: number | null;
}

export function InboxMessages({
  m,
  state,
  refresh,
  now,
  onConfigure,
  preview = false,
  initialOpenSeq = null,
}: Props) {
  const c = m.callme;
  // 同时只展开一条：这个面板高度有限，展开两条就会把「刚才那条」顶出视野。
  const [openSeq, setOpenSeq] = useState<number | null>(initialOpenSeq);
  const [copied, setCopied] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const read = state?.read ?? {};
  const sources = state?.sources ?? [];
  // `state.messages` 按 seq 升序（Rust 侧如此），这里翻成最新在前。
  const messages = useMemo(() => newestFirst(state?.messages ?? []), [state?.messages]);
  const unread = unreadCount(messages, read);

  // 来源被彻底删掉后旧留言仍在，但名字找不到了 —— 显示「已删除的链接」比把
  // 8 位内部 id 摊给用户好。
  const labelOf = (sourceId: string) =>
    sources.find((source) => source.id === sourceId)?.label ?? c.deletedSource;

  const markRead = (message: CallMeMessage) => {
    if (message.seq <= readOf(read, message.source_id)) return;
    // 演示里绝不发 IPC：展开一条留言是纯本地的看点，不该顺手把真面板里那条
    // 标成已读。
    if (preview) return;
    invoke("callme_mark_read", { sourceId: message.source_id, seq: message.seq })
      .then(refresh)
      .catch((err) => setError(String(err)));
  };

  const toggle = (message: CallMeMessage) => {
    const next = openSeq === message.seq ? null : message.seq;
    setOpenSeq(next);
    // 展开即「读到了」：点开一条还要再点一次「标为已读」是多余的一步。
    if (next !== null) markRead(message);
  };

  const markAllRead = () => {
    if (preview) return;
    const pending = sources
      .map((source) => ({ source, seq: lastSeqOf(messages, source.id) }))
      .filter(({ source, seq }) => seq > readOf(read, source.id));
    if (pending.length === 0) return;
    Promise.all(
      pending.map(({ source, seq }) =>
        invoke("callme_mark_read", { sourceId: source.id, seq }),
      ),
    )
      .then(refresh)
      .catch((err) => setError(String(err)));
  };

  const copy = (message: CallMeMessage) => {
    // 演示里不碰剪贴板：看演示不该悄悄改掉用户刚复制的东西。
    if (preview) return;
    navigator.clipboard
      .writeText(message.text)
      .then(() => {
        setCopied(message.seq);
        window.setTimeout(() => setCopied(null), 1500);
      })
      .catch((err) => setError(String(err)));
  };

  // 空态按原因区分：没服务器 / 连不上 / 没链接 / 没留言。说错原因会让人去修错的东西
  // ——「连不上」时按「没有链接」去解释，用户会去新建一条注定失败的链接。
  const emptyAction = (label: string, detail?: string) => (
    <>
      <p className="inbox-empty-hint">{label}</p>
      {detail && <p className="inbox-empty-error">{detail}</p>}
      <div className="inbox-empty-actions">
        <button type="button" className="inbox-btn" onClick={onConfigure}>
          {c.goConfigure}
        </button>
      </div>
    </>
  );

  const renderEmpty = () => {
    if (!state?.enabled) return emptyAction(c.messagesEmptyNoServer);
    if (!state.ok && state.error) return emptyAction(c.messagesEmptyOffline, state.error);
    if (sources.length === 0) return emptyAction(c.messagesEmptyNoSource);
    return <p className="inbox-empty-hint">{c.messagesEmpty}</p>;
  };

  if (messages.length === 0) {
    return (
      <div className="inbox-scroll">
        {/* 这一页的空态不写标题：页签已经把「留言」说过了，再写一遍只是重复；
            这里该说的是「为什么空」（没有服务器 / 连不上 / 没链接 / 真没有）。 */}
        <div className="inbox-empty">{renderEmpty()}</div>
      </div>
    );
  }

  return (
    <div className="inbox-scroll">
      {/* 全部已读只在真有未读时出现：它是「清一下」的工具，不是常驻按钮。 */}
      {unread > 0 && (
        <div className="inbox-msg-tools">
          <span className="inbox-msg-unread">{c.unread(unread)}</span>
          <button type="button" className="inbox-icon" onClick={markAllRead}>
            {c.markAllRead}
          </button>
        </div>
      )}

      {messages.map((message) => {
        const isUnread = message.seq > readOf(read, message.source_id);
        const isOpen = openSeq === message.seq;
        const from = openableUrl(message.context);
        return (
          <div
            key={message.id}
            className="inbox-row inbox-msg-row"
            data-unread={isUnread || undefined}
            data-open={isOpen || undefined}
            onClick={() => toggle(message)}
          >
            <span className="inbox-dot" aria-hidden />
            <div className="inbox-body">
              <div className="inbox-line">
                <span className="inbox-name">{labelOf(message.source_id)}</span>
                <span className="inbox-age">{formatAge(message.received_ms, now)}</span>
              </div>
              {message.title && <div className="inbox-msg-title">{message.title}</div>}
              <div className="inbox-msg-text">{message.text}</div>
              {/* 元信息放在正文之后：留言的名字和内容才是要读的，agent 名字是
                  补充 —— 和收件箱行相反，那边「哪个项目」才是第一眼要找的。 */}
              <div className="inbox-meta">
                {message.agent && <span className="inbox-agent">{message.agent}</span>}
              </div>
              {isOpen && (
                <div className="inbox-actions" onClick={(event) => event.stopPropagation()}>
                  {from && (
                    <button
                      type="button"
                      className="inbox-btn"
                      onClick={() => {
                        // 演示里不打开浏览器：那是「操作这台机器」，不是看演示。
                        if (preview) return;
                        invoke("open_url", { url: from }).catch((err) =>
                          setError(String(err)),
                        );
                      }}
                    >
                      {c.openSource}
                    </button>
                  )}
                  {message.context && !from && (
                    <span className="inbox-msg-context">{message.context}</span>
                  )}
                  <button type="button" className="inbox-btn" onClick={() => copy(message)}>
                    {copied === message.seq ? c.copiedText : c.copyText}
                  </button>
                </div>
              )}
            </div>
          </div>
        );
      })}

      {error && <p className="inbox-warn">{error}</p>}
    </div>
  );
}
