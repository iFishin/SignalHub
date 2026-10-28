import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { useLang, useMessages } from "../lib/i18n";
import { useNow } from "../lib/useNow";
import { useSettings } from "../lib/useSettings";
import { signalLabel } from "../lib/signals";
import type { Approval } from "../lib/useApprovals";
import type { Snapshot } from "../lib/types";
import { buildInbox, formatAge, type InboxRow } from "../lib/inboxRows";

interface Props {
  snapshot: Snapshot;
  approvals: Approval[];
  decide: (id: string, decision: "allow" | "deny") => void;
}

/**
 * Rust `jump_to_session` 的返回。`kind` 决定下一步：真正切了窗口就收起收件箱，
 * 只是打开目录 / 没找到就留在原地把原因说清楚。
 */
interface JumpResult {
  kind: "tmux" | "terminal" | "app" | "folder" | "not_found" | "unsupported";
  detail: string;
}

/** 提示条停留时长。够读完一句话，又不至于挡着下一行。 */
const TOAST_MS = 3200;

function stop(event: React.MouseEvent) {
  event.stopPropagation();
}

/**
 * 收件箱：把「谁需要你」从灯上搬到一个能点的列表里。
 *
 * 它**不是设置面板的另一个页签**：设置是低频的配置，收件箱是高频的操作。把它做成
 * 独立窗口，才能贴着灯出现、Esc 收起、点一行直接跳走 —— 这三件事在设置面板里都
 * 很别扭。
 *
 * 数据仍然只有两个来源：`useSignal` 的快照（会话）和 `useApprovals` 的登记表
 * （待审批）。组件本身不轮询、不另开 IPC。
 */
export function InboxWindow({ snapshot, approvals, decide }: Props) {
  const m = useMessages();
  const lang = useLang();
  const settings = useSettings();
  const model = useMemo(
    () => buildInbox(snapshot, approvals, settings.group_meta),
    [snapshot, approvals, settings.group_meta],
  );

  // 所有段摊平成一条键盘路径：↑↓ 不会在「需要你」和「运行中」之间断掉。
  const all = useMemo(
    () => [...model.need, ...model.running, ...model.done, ...model.quiet],
    [model],
  );
  const indexByKey = useMemo(
    () => new Map(all.map((row, index) => [row.key, index])),
    [all],
  );

  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  // 选中的那一行消失（审批被处理、会话过期）时自动接住第一行；否则键盘焦点会
  // 停在一个已经不在列表里的 key 上，下一次 Enter 什么也不会发生。
  useEffect(() => {
    if (selectedKey && all.some((row) => row.key === selectedKey)) return;
    setSelectedKey(all[0]?.key ?? null);
  }, [all, selectedKey]);

  const selected = all.find((row) => row.key === selectedKey) ?? null;

  // 时间显示要自己走：快照只在状态变化时推，而「等了 3 分钟」不能停在推送那一刻。
  const now = useNow(true);
  const snapshotAt = useRef(Date.now());
  useEffect(() => {
    snapshotAt.current = Date.now();
  }, [snapshot, approvals]);
  const ageOf = useCallback(
    (row: InboxRow) => row.ageMs + Math.max(0, now - snapshotAt.current),
    [now],
  );

  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number | null>(null);
  const say = useCallback((text: string) => {
    setToast(text);
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), TOAST_MS);
  }, []);
  useEffect(
    () => () => {
      if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    },
    [],
  );

  const hide = useCallback(() => {
    if (!inTauri) return;
    invoke("hide_inbox").catch((err) => console.error("hide_inbox 失败", err));
  }, []);

  const jump = useCallback(
    (row: InboxRow) => {
      if (!inTauri) return;
      invoke<JumpResult>("jump_to_session", { session: row.session })
        .then((result) => {
          // 真正切了窗口 / 应用就把它收起来，让用户直接落在目标上；
          // 只打开目录或没找到时留在收件箱里，把原因说清楚。
          if (result.kind === "tmux" || result.kind === "terminal" || result.kind === "app") {
            hide();
          } else {
            say(result.detail);
          }
        })
        .catch((err) => say(String(err)));
    },
    [hide, say],
  );

  const decideRow = useCallback(
    (row: InboxRow, decision: "allow" | "deny") => {
      if (!row.approval) return;
      if (decision === "allow" && !row.approval.allowable) {
        say(m.inbox.allowBlocked);
        return;
      }
      decide(row.approval.id, decision);
      // 真正的结果由 Rust 推回来（那一行消失 / 还留着）；这里只给一个即时反馈。
      say(decision === "allow" ? m.inbox.allowed : m.inbox.denied);
    },
    [decide, m.inbox.allowBlocked, m.inbox.allowed, m.inbox.denied, say],
  );

  const toggleMute = useCallback(
    (row: InboxRow) => {
      if (!row.group) return;
      const muted = settings.muted_groups.includes(row.group);
      invoke("mute_group", { group: row.group, muted: !muted })
        .then(() => say(muted ? m.inbox.unmuted(row.label) : m.inbox.muted(row.label)))
        .catch((err) => say(String(err)));
    },
    [settings.muted_groups, m.inbox, say],
  );

  const clear = useCallback(
    (row: InboxRow) => {
      invoke("clear_session", { session: row.session })
        .then(() => say(m.inbox.cleared))
        .catch((err) => say(String(err)));
    },
    [m.inbox.cleared, say],
  );

  const openSettings = useCallback(() => {
    // 直接开设置：设置窗拿到焦点时收件箱会自己 blur 收起（见 rust 的 `wire`），
    // 不在这里先 hide —— 两条 IPC 并发时，hide 可能晚于 show 执行，把激活策略
    // 又切回 Accessory。
    invoke("open_settings").catch((err) => console.error("open_settings 失败", err));
  }, []);

  // 键盘：收件箱的主要操作方式。按钮照样能点，但连续处理几条时手不用离开键盘。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const index = all.findIndex((row) => row.key === selectedKey);
      const move = (delta: number) => {
        if (all.length === 0) return;
        // index < 0 说明选中项刚消失：往下走时从「第 0 项之前」开始，往上走时就停在 0。
        const base = index < 0 ? (delta > 0 ? -1 : 0) : index;
        const next = Math.min(all.length - 1, Math.max(0, base + delta));
        setSelectedKey(all[next].key);
        event.preventDefault();
      };

      switch (event.key) {
        case "ArrowDown":
        case "j":
          move(1);
          return;
        case "ArrowUp":
        case "k":
          move(-1);
          return;
        case "Enter":
          if (selected) {
            jump(selected);
            event.preventDefault();
          }
          return;
        case "Escape":
          hide();
          event.preventDefault();
          return;
        default:
          break;
      }

      if (selected?.approval && event.key.toLowerCase() === "a") {
        decideRow(selected, "allow");
        event.preventDefault();
      } else if (selected?.approval && event.key.toLowerCase() === "d") {
        decideRow(selected, "deny");
        event.preventDefault();
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [all, decideRow, hide, jump, selected, selectedKey]);

  // 选中的行滚进视野 —— 键盘走到底部时列表要跟着走。
  useEffect(() => {
    if (!selectedKey) return;
    const index = indexByKey.get(selectedKey);
    if (index === undefined) return;
    document
      .querySelector(`[data-inbox-index="${index}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [indexByKey, selectedKey]);

  const renderRow = (row: InboxRow) => {
    const index = indexByKey.get(row.key) ?? 0;
    const isSelected = row.key === selectedKey;
    const muted = row.group !== "" && settings.muted_groups.includes(row.group);
    const approval = row.approval;
    return (
      <div
        key={row.key}
        className="inbox-row"
        data-inbox-index={index}
        data-state={row.state}
        data-selected={isSelected || undefined}
        title={row.cwd ?? undefined}
        onClick={() => setSelectedKey(row.key)}
        onDoubleClick={() => jump(row)}
      >
        <span className="inbox-dot" aria-hidden />
        <div className="inbox-body">
          <div className="inbox-line">
            <span className="inbox-name">{row.label}</span>
            {row.agent && <span className="inbox-agent">{row.agent}</span>}
            <span className="inbox-state">{signalLabel(lang, row.state)}</span>
            <span className="inbox-age">
              {row.approval || row.state === "permission" || row.state === "attention" || row.state === "error"
                ? m.inbox.waiting(formatAge(ageOf(row)))
                : formatAge(ageOf(row))}
            </span>
          </div>
          {row.message && <div className="inbox-message">{row.message}</div>}
          {approval && (
            <div className="inbox-approval">
              <div className="inbox-approval-head">
                {approval.tool}
                {approval.target && <span className="inbox-target"> · {approval.target}</span>}
              </div>
              <pre className="inbox-detail">{approval.detail}</pre>
              {approval.truncated && <div className="inbox-warn">{m.inbox.truncated}</div>}
              {!approval.allowable && !approval.truncated && (
                <div className="inbox-warn">{m.inbox.allowBlocked}</div>
              )}
            </div>
          )}
          <div className="inbox-actions">
            {approval && (
              <>
                <button
                  type="button"
                  className="inbox-btn inbox-btn-allow"
                  disabled={!approval.allowable}
                  title={approval.allowable ? m.inbox.approvalHint : m.inbox.allowBlocked}
                  onMouseDown={stop}
                  onDoubleClick={stop}
                  onClick={(event) => {
                    stop(event);
                    decideRow(row, "allow");
                  }}
                >
                  {m.inbox.allow}
                </button>
                <button
                  type="button"
                  className="inbox-btn"
                  onMouseDown={stop}
                  onDoubleClick={stop}
                  onClick={(event) => {
                    stop(event);
                    decideRow(row, "deny");
                  }}
                >
                  {m.inbox.deny}
                </button>
              </>
            )}
            <button
              type="button"
              className="inbox-btn"
              onMouseDown={stop}
              onDoubleClick={stop}
              onClick={(event) => {
                stop(event);
                jump(row);
              }}
            >
              {m.inbox.jump}
            </button>
            {row.group && (
              <button
                type="button"
                className="inbox-btn"
                onMouseDown={stop}
                onDoubleClick={stop}
                onClick={(event) => {
                  stop(event);
                  toggleMute(row);
                }}
              >
                {muted ? m.inbox.unmute : m.inbox.mute}
              </button>
            )}
            <button
              type="button"
              className="inbox-btn"
              onMouseDown={stop}
              onDoubleClick={stop}
              onClick={(event) => {
                stop(event);
                clear(row);
              }}
            >
              {m.inbox.clear}
            </button>
          </div>
        </div>
      </div>
    );
  };

  const section = (title: string, rows: InboxRow[], extra?: string) => {
    if (rows.length === 0) return null;
    return (
      <section className="inbox-section">
        <h2 className="inbox-section-title">
          {title}
          <span className="inbox-section-count">{rows.length}</span>
          {extra && <span className="inbox-section-extra">{extra}</span>}
        </h2>
        {rows.map(renderRow)}
      </section>
    );
  };


  return (
    <div className="inbox-window">
      <header className="inbox-head" data-tauri-drag-region>
        <span className="inbox-title">{m.inbox.title}</span>
        {model.total > 0 && <span className="inbox-count">{model.total}</span>}
        {snapshot.paused && <span className="inbox-paused">{m.inbox.paused}</span>}
        <span className="inbox-spacer" />
        <button
          type="button"
          className="inbox-icon"
          title={m.inbox.settings}
          aria-label={m.inbox.settings}
          onMouseDown={stop}
          onClick={(event) => {
            stop(event);
            openSettings();
          }}
        >
          {m.inbox.settings}
        </button>
        <button
          type="button"
          className="inbox-icon"
          title={m.inbox.closeHint}
          aria-label={m.inbox.close}
          onMouseDown={stop}
          onClick={(event) => {
            stop(event);
            hide();
          }}
        >
          ×
        </button>
      </header>

      <div className="inbox-scroll">
        {all.length === 0 ? (
          <div className="inbox-empty">
            <div className="inbox-empty-title">{m.inbox.emptyTitle}</div>
            <div className="inbox-empty-hint">{m.inbox.emptyHint}</div>
          </div>
        ) : (
          <>
            {section(m.inbox.need, model.need)}
            {section(m.inbox.running, model.running)}
            {section(m.inbox.done, model.done)}
            {section(m.inbox.quiet, model.quiet)}
          </>
        )}
      </div>

      <footer className="inbox-foot">
        <span>{m.inbox.keys}</span>
        {toast && <div className="inbox-toast">{toast}</div>}
      </footer>
    </div>
  );
}
