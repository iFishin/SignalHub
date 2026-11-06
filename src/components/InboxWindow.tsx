import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "../lib/env";
import { useLang, useMessages } from "../lib/i18n";
import { useNow } from "../lib/useNow";
import { useSettings } from "../lib/useSettings";
import { signalLabel } from "../lib/signals";
import type { Approval, DenyReason } from "../lib/useApprovals";
import type { Snapshot } from "../lib/types";
import { buildInbox, formatAge, type InboxRow } from "../lib/inboxRows";
import {
  buildAwaySummary,
  isQuiet,
  type AwayEvent,
  type AwaySummary,
} from "../lib/awaySummary";
import { dangerLabel, reasonLabel } from "../lib/approvalLabels";
import { HoldButton } from "./HoldButton";

interface Props {
  snapshot: Snapshot;
  approvals: Approval[];
  decide: (
    id: string,
    decision: "allow" | "deny",
    reason?: DenyReason,
    custom?: string,
  ) => void;
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

/**
 * 「上次看过收件箱」的时刻。
 *
 * 存 localStorage 而不是设置文件：它是**运行时的观察记录**，不是用户偏好 ——
 * 进「配置导出」里没有意义，换台机器也不该跟着走。两个窗口同源，所以读写的
 * 是同一份。
 */
const LAST_SEEN_KEY = "signalhub.inbox.lastSeen";

function readLastSeen(): number | null {
  try {
    const raw = window.localStorage.getItem(LAST_SEEN_KEY);
    const value = raw === null ? Number.NaN : Number(raw);
    return Number.isFinite(value) ? value : null;
  } catch {
    // 存储被禁用（某些隐私策略）时不该把收件箱弄挂 —— 没有基准就不出摘要。
    return null;
  }
}

function writeLastSeen(ms: number) {
  try {
    window.localStorage.setItem(LAST_SEEN_KEY, String(ms));
  } catch {
    /* 同上：记不住就不记，摘要自己会退化成「没有基准」 */
  }
}

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

  // ---- 「你离开的这段时间」----
  //
  // 只在**打开收件箱**时算一次：摘要说的是「离开期间发生了什么」，不是「现在
  // 怎么样」—— 后者就是下面那份列表。基准是上次打开的时刻。
  const [away, setAway] = useState<AwaySummary | null>(null);
  const awaySettled = useRef(false);
  useEffect(() => {
    if (awaySettled.current) return;
    // 快照是异步来的：第一帧模型是空的，这时算出来必然是「什么都没发生」。
    // 等真的有了行再算。收件箱本来就空就一直不算 —— 那种情况也没有摘要可给。
    if (all.length === 0) return;
    awaySettled.current = true;

    const now = Date.now();
    const lastSeen = readLastSeen();
    // 事件流水是增强，不是依赖：读不到（盘坏、旧版本 Rust、非 Tauri 环境）就
    // 安静地退回第一版「只看快照」，而不是把整块摘要藏起来。
    const fallback = () => setAway(buildAwaySummary(model, now, lastSeen));
    if (lastSeen === null || !inTauri) {
      fallback();
      return;
    }
    invoke<AwayEvent[]>("get_events", { sinceMs: lastSeen, limit: 5000 })
      .then((events) => setAway(buildAwaySummary(model, now, lastSeen, events)))
      .catch((err) => {
        console.error("get_events 失败", err);
        fallback();
      });
  }, [all.length, model]);

  // 打开就记「看过」，关掉再记一次。少了卸载那一次的话，收件箱开着一小时后
  // 关掉，下次打开会把这一小时也算成「你不在」。
  useEffect(() => {
    writeLastSeen(Date.now());
    return () => writeLastSeen(Date.now());
  }, []);

  const awayParts = useMemo(() => {
    if (!away) return [];
    if (isQuiet(away)) return [m.inbox.awayQuiet];
    const parts: string[] = [];
    // 顺序 = 该处理的顺序：等你 > 失败 > 完成 > 在跑。
    if (away.need > 0) parts.push(m.inbox.awayNeed(away.need));
    if (away.failed > 0) parts.push(m.inbox.awayFailed(away.failed));
    if (away.done > 0) parts.push(m.inbox.awayDone(away.done));
    if (away.running > 0) parts.push(m.inbox.awayRunning(away.running));
    // 「跑完就被删掉」的那些只在事件流水里，收件箱列表上找不到 —— 单独说出来，
    // 否则用户数一遍列表会以为摘要多报了。
    if (away.transientDone > 0) parts.push(m.inbox.awayTransientDone(away.transientDone));
    if (away.transientFailed > 0) parts.push(m.inbox.awayTransientFailed(away.transientFailed));
    return parts;
  }, [away, m.inbox]);

  // 哪一行正在选拒绝理由，以及「自定义」那一格的字。**只可能有一行**：
  // 同时给两条挑理由没有意义，而收件箱本来就是一条一条处理的。
  const [reasonFor, setReasonFor] = useState<string | null>(null);
  const [customReason, setCustomReason] = useState("");

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

  // 「跑完叫我」的订阅只活在 Rust 内存里。挂载时拉一次，之后每次点按钮用命令
  // 返回的列表覆盖本地状态 —— 两边始终是同一份真值，不各自记一份。
  const [watched, setWatched] = useState<string[]>([]);
  useEffect(() => {
    if (!inTauri) return;
    invoke<string[]>("get_watches")
      .then(setWatched)
      .catch((err) => console.error("get_watches 失败", err));
  }, []);

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
      // 远程会话的 `cwd` 在对端机器上 —— 在本机打开只会是一个不存在的目录，
      // 而「点了跳转，Finder 弹出一句找不到」比直接说清楚糟得多。
      // 跨机器跳转这一轮不做（见方案 §6）。
      if (row.machine) {
        say(m.inbox.remoteJump(row.machine));
        return;
      }
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
    [hide, m.inbox, say],
  );

  const decideRow = useCallback(
    (row: InboxRow, decision: "allow" | "deny", reason?: DenyReason, custom?: string) => {
      if (!row.approval) return;
      if (decision === "allow" && !row.approval.allowable) {
        say(m.inbox.allowBlocked);
        return;
      }
      decide(row.approval.id, decision, reason, custom);
      // 决定之后理由那一格收起来（无论成功还是超时，那一行都不该还挂着菜单）
      setReasonFor(null);
      setCustomReason("");
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

  const toggleWatch = useCallback(
    (row: InboxRow) => {
      if (!inTauri) return;
      const isWatched = watched.includes(row.session);
      invoke<string[]>(isWatched ? "unwatch_session" : "watch_session", {
        session: row.session,
        label: row.label,
      })
        .then((ids) => {
          setWatched(ids);
          say(isWatched ? m.inbox.watchCleared(row.label) : m.inbox.watchSet(row.label));
        })
        .catch((err) => say(String(err)));
    },
    [watched, m.inbox, say],
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
    const danger = dangerLabel(m, approval?.danger);
    const picking = approval ? reasonFor === approval.id : false;
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
            {/* 机器徽章排在名字前面：一眼就能看出这一行不是本机的，也就不会
                去点那个跳转按钮。 */}
            {row.machine && <span className="inbox-machine">{row.machine}</span>}
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
            <div className="inbox-approval" data-danger={danger ? "1" : undefined}>
              <div className="inbox-approval-head">
                {approval.tool}
                {approval.target && <span className="inbox-target"> · {approval.target}</span>}
              </div>
              {/* 危险横幅：只说「这条会干什么」，不替用户拒绝 */}
              {danger && <div className="inbox-danger">{danger}</div>}
              <pre className="inbox-detail">{approval.detail}</pre>
              {approval.truncated && <div className="inbox-warn">{m.inbox.truncated}</div>}
              {!approval.allowable && !approval.truncated && (
                <div className="inbox-warn">{m.inbox.allowBlocked}</div>
              )}
              {/* 拒绝理由（A2）：三个预设 + 自定义。灯上的气泡只给预设 ——
                  这里窗口更大，能放下一个输入框。 */}
              {picking && (
                <div className="inbox-reasons">
                  {(["unsafe", "safer", "explain"] as DenyReason[]).map((reason) => (
                    <button
                      key={reason}
                      type="button"
                      className="inbox-btn inbox-btn-reason"
                      onMouseDown={stop}
                      onDoubleClick={stop}
                      onClick={(event) => {
                        stop(event);
                        decideRow(row, "deny", reason);
                      }}
                    >
                      {reasonLabel(m, reason)}
                    </button>
                  ))}
                  <form
                    className="inbox-reason-custom"
                    onSubmit={(event) => {
                      event.preventDefault();
                      const text = customReason.trim();
                      if (text) decideRow(row, "deny", "custom", text);
                    }}
                  >
                    <input
                      value={customReason}
                      onChange={(event) => setCustomReason(event.target.value)}
                      onMouseDown={stop}
                      placeholder={m.approvalReasonCustomPlaceholder}
                      maxLength={200}
                      aria-label={m.approvalReasonCustom}
                    />
                    <button
                      type="submit"
                      className="inbox-btn"
                      disabled={!customReason.trim()}
                    >
                      {m.approvalReasonCustom}
                    </button>
                  </form>
                </div>
              )}
            </div>
          )}
          <div className="inbox-actions">
            {approval && (
              <>
                {/* 危险命令的「允许」要按住 0.8 秒（A3）；其余仍是一键 */}
                {danger && approval.allowable ? (
                  <HoldButton
                    className="inbox-btn inbox-btn-allow"
                    title={m.approvalHold}
                    ariaLabel={`${m.inbox.allow} · ${m.approvalHold}`}
                    onConfirm={() => decideRow(row, "allow")}
                  >
                    {m.inbox.allow}
                  </HoldButton>
                ) : (
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
                )}
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
                {/* 展开拒绝理由（A2） */}
                <button
                  type="button"
                  className="inbox-btn inbox-btn-reason"
                  title={m.approvalReason}
                  aria-label={m.approvalReason}
                  aria-expanded={picking}
                  onMouseDown={stop}
                  onDoubleClick={stop}
                  onClick={(event) => {
                    stop(event);
                    setReasonFor(picking ? null : approval.id);
                    setCustomReason("");
                  }}
                >
                  ▾
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
            {row.session && (
              <button
                type="button"
                className="inbox-btn"
                data-active={watched.includes(row.session) || undefined}
                onMouseDown={stop}
                onDoubleClick={stop}
                onClick={(event) => {
                  stop(event);
                  toggleWatch(row);
                }}
              >
                {watched.includes(row.session) ? m.inbox.unwatch : m.inbox.watch}
              </button>
            )}
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

      {/* 「你离开的这段时间」：一句话、可关掉。放在列表**上方**而不是塞进某一段
          —— 它说的不是某一条会话，而是「这段时间整体」，混进分段会被当成其中一类。 */}
      {away && awayParts.length > 0 && (
        <div className="inbox-away" role="status">
          <span className="inbox-away-when">
            {m.inbox.awayFor(formatAge(away.awayMs))}
          </span>
          <span
            className="inbox-away-parts"
            title={
              away.need > 0 && away.longestWaitMs > 0
                ? m.inbox.waiting(formatAge(away.longestWaitMs))
                : undefined
            }
          >
            {awayParts.join(" · ")}
          </span>
          <button
            type="button"
            className="inbox-icon"
            title={m.inbox.awayDismiss}
            aria-label={m.inbox.awayDismiss}
            onClick={() => setAway(null)}
          >
            ×
          </button>
        </div>
      )}

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
