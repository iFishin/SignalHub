import { useMemo, useState } from "react";
import { buildInbox, dirName, formatAge, type InboxRow } from "../lib/inboxRows";
import { unreadCount } from "../lib/callme";
import { DEFAULT_LABEL_PREF } from "../lib/lampSort";
import { signalLabel } from "../lib/signals";
import { isQuiet } from "../lib/awaySummary";
import { dangerLabel } from "../lib/approvalLabels";
import type { Lang, Messages } from "../lib/i18n";
import type { DemoPanelFrame } from "../lib/demoPanel";
import { InboxMessages } from "./InboxMessages";

/**
 * 演示用的**只读**收件箱面板。
 *
 * 为什么不直接渲染 `InboxWindow`：那个组件会在 window 上挂键盘监听、读改
 * `localStorage` 的「上次看过」、`invoke` 一堆命令、还带一个能拖着改尺寸的把手。
 * 设置页里的演示是在**真的 Tauri 进程**中跑的（`inTauri === true`），把它塞进来
 * 等于演示一动就改真状态。所以这里是一份只读投影：
 *
 * - 数据走 `buildInbox()` 这个**真实纯函数**，分段、排序、去重全和产品一致；
 * - 视觉复用 `inbox.css` 的 `.inbox-*` 类名，改了样式两边一起变；
 * - 留言页直接复用 `InboxMessages` **组件本体**（给它 preview 开关）；
 * - 所有动作退化成 `<span>`，不产生可点、可聚焦的元素，也不挂任何监听。
 *
 * 收件箱的行标记是这里唯一「重画」的部分 —— `InboxWindow` 的 `renderRow` 和
 * 十几个交互回调长在一起，抽出来要动那个高频组件，风险不值当。**改那边的行
 * 结构时请同步这里**；`css.test.ts` 里那条测试会盯着这里用到的类名没漂出去。
 */

interface Props {
  frame: DemoPanelFrame;
  m: Messages;
  lang: Lang;
  now: number;
}

/**
 * 只读的动作按钮。`span` 而不是 `button` —— 它长得像按钮，但不该被点到。
 *
 * `unavailable` 要照真收件箱里 `disabled` 的样子画（`InboxWindow` 给「允许」
 * 挂 `disabled={!allowable}`）：灰掉、但**仍然带着绿色描边**。演示若把它画成
 * 普通按钮，等于告诉用户「这条能点」，那正是这条规则要避免的误会。
 */
function Action({
  children,
  allow = false,
  unavailable = false,
}: {
  children: React.ReactNode;
  allow?: boolean;
  unavailable?: boolean;
}) {
  return (
    <span
      className={allow ? "inbox-btn inbox-btn-allow" : "inbox-btn"}
      data-disabled={allow && unavailable ? "true" : undefined}
    >
      {children}
    </span>
  );
}

function Row({ row, m, lang }: { row: InboxRow; m: Messages; lang: Lang }) {
  const dir = row.cwd ? dirName(row.cwd) : "";
  const approval = row.approval;
  const danger = dangerLabel(m, approval?.danger);
  // 真面板里「等了多久」会自己走钟；演示里用帧里的固定值，重播时读数不跳。
  const age = formatAge(row.ageMs);
  const waiting =
    approval || row.state === "permission" || row.state === "attention" || row.state === "error";

  return (
    <div className="inbox-row" data-state={row.state}>
      <span className="inbox-dot" aria-hidden="true" />
      <div className="inbox-body">
        <div className="inbox-line">
          <span className="inbox-name">{row.label}</span>
          <span className="inbox-age">{waiting ? m.inbox.waiting(age) : age}</span>
        </div>
        <div className="inbox-meta">
          {row.machine && <span className="inbox-machine">{row.machine}</span>}
          {row.agent && <span className="inbox-agent">{row.agent}</span>}
          {dir && <span className="inbox-dir">{dir}</span>}
          <span className="inbox-state">{signalLabel(lang, row.state)}</span>
        </div>
        {row.message && <div className="inbox-message">{row.message}</div>}

        {approval && (
          <div className="inbox-approval" data-danger={danger ? "1" : undefined}>
            <div className="inbox-approval-head">
              {approval.tool}
              {approval.target && <span className="inbox-target"> · {approval.target}</span>}
            </div>
            {danger && <div className="inbox-danger">{danger}</div>}
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
              <Action allow unavailable={!approval.allowable}>{m.inbox.allow}</Action>
              <Action>{m.inbox.deny}</Action>
              <span className="inbox-btn inbox-btn-reason">▾</span>
            </>
          )}
          <Action>{m.inbox.jump}</Action>
          <span className="inbox-btn inbox-btn-more">⋯</span>
        </div>
      </div>
    </div>
  );
}

/** 「你离开的这段时间」那一句，和 InboxWindow 的 awayParts 用同一套措辞与顺序。 */
function awayParts(
  away: NonNullable<DemoPanelFrame["away"]>,
  m: Messages,
): string[] {
  if (isQuiet(away)) return [m.inbox.awayQuiet];
  const parts: string[] = [];
  if (away.need > 0) parts.push(m.inbox.awayNeed(away.need));
  if (away.failed > 0) parts.push(m.inbox.awayFailed(away.failed));
  if (away.done > 0) parts.push(m.inbox.awayDone(away.done));
  if (away.running > 0) parts.push(m.inbox.awayRunning(away.running));
  if (away.transientDone > 0) parts.push(m.inbox.awayTransientDone(away.transientDone));
  if (away.transientFailed > 0) parts.push(m.inbox.awayTransientFailed(away.transientFailed));
  return parts;
}

export function DemoPanel({ frame, m, lang, now }: Props) {
  // 页签由帧给初值。切帧时 DemoStage 换 key 重挂载，所以「这一帧该看哪一页」
  // 每次都对；用户在这帧里点一下切到另一半，也只是本地 state，不改任何东西。
  const [tab, setTab] = useState(frame.tab);
  // **真实分段逻辑**：一行 = 一个会话，一条审批只画一次，需要你的排最前。
  const model = useMemo(
    () => buildInbox(frame.snapshot, frame.approvals, {}, DEFAULT_LABEL_PREF),
    [frame.snapshot, frame.approvals],
  );
  const unread = unreadCount(frame.callme.messages, frame.callme.read);
  const away = frame.away;
  const parts = away ? awayParts(away, m) : [];

  const section = (title: string, rows: InboxRow[], tone?: "need") =>
    rows.length === 0 ? null : (
      <section className="inbox-section" data-tone={tone}>
        <h2 className="inbox-section-title">
          {title}
          <span className="inbox-section-count">{rows.length}</span>
        </h2>
        {rows.map((row) => (
          <Row key={row.key} row={row} m={m} lang={lang} />
        ))}
      </section>
    );

  return (
    <div className="inbox-window demo-inbox" data-preview="true">
      <header className="inbox-head">
        <div className="inbox-tabs" role="tablist" aria-label={m.inbox.title}>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "inbox"}
            className="inbox-tab"
            data-active={tab === "inbox" || undefined}
            onClick={() => setTab("inbox")}
          >
            {m.inbox.title}
            {model.need.length > 0 && (
              <span className="inbox-tab-badge" data-urgent="1">
                {model.need.length}
              </span>
            )}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "messages"}
            className="inbox-tab"
            data-active={tab === "messages" || undefined}
            onClick={() => setTab("messages")}
          >
            {m.tabMessages}
            {unread > 0 && <span className="inbox-tab-badge">{unread}</span>}
          </button>
        </div>
        <span className="inbox-spacer" />
        {/* 这两枚是窗口壳上的真按钮，演示里只留样子、不点。 */}
        <span className="inbox-icon">{m.inbox.settings}</span>
        <span className="inbox-icon">×</span>
      </header>

      {tab === "messages" ? (
        <InboxMessages
          m={m}
          state={frame.callme}
          refresh={() => {}}
          now={now}
          onConfigure={() => {}}
          preview
          initialOpenSeq={frame.openSeq}
        />
      ) : (
        <>
          {away && parts.length > 0 && (
            <div className="inbox-away" role="status">
              <span className="inbox-away-when">
                {m.inbox.awayFor(formatAge(away.awayMs))}
              </span>
              <span className="inbox-away-parts">{parts.join(" · ")}</span>
            </div>
          )}
          <div className="inbox-scroll">
            {model.need.length + model.running.length + model.done.length + model.quiet.length ===
            0 ? (
              <div className="inbox-empty">
                <div className="inbox-empty-title">{m.inbox.emptyTitle}</div>
                <div className="inbox-empty-hint">{m.inbox.emptyHint}</div>
                <div className="inbox-empty-actions">
                  <span className="inbox-btn inbox-btn-allow">{m.inbox.emptyConnect}</span>
                  <span className="inbox-btn">{m.inbox.emptyDemo}</span>
                </div>
              </div>
            ) : (
              <>
                {section(m.inbox.need, model.need, "need")}
                {section(m.inbox.running, model.running)}
                {section(m.inbox.done, model.done)}
                {section(m.inbox.quiet, model.quiet)}
              </>
            )}
          </div>
        </>
      )}

      <footer className="inbox-foot">
        {tab === "inbox" ? m.inbox.keys : m.inbox.messagesKeys}
      </footer>
    </div>
  );
}
