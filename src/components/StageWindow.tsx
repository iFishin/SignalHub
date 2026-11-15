import { useEffect, useMemo } from "react";
import type { CSSProperties } from "react";
import type { Snapshot } from "../lib/types";
import type { Settings } from "../lib/useSettings";
import { useMessages, useLang } from "../lib/i18n";
import { signalLabel } from "../lib/signals";
import { formatAge } from "../lib/age";
import { labelOf, labelPrefOf, orderVisible } from "../lib/lampSort";
import { buildInbox, formatAge as formatSpan } from "../lib/inboxRows";
import { buildAwaySummary, isQuiet } from "../lib/awaySummary";
import { useNow } from "../lib/useNow";
import { setStage, useToggles } from "../lib/useToggles";
import { inTauri } from "../lib/env";
import { stageColumns } from "../lib/stageGrid";
import { lampVars, resolveShapes, rhythmDelay } from "../lib/lampStyle";

/**
 * 展示模式的大屏看板。
 *
 * **这不是被放大的灯牌。** `lamp.css` 的几何全挂在 `--size` 上（22px 档），另外
 * 还有圆环 196px、气泡 132px、1px 描边、14px 留白这些写死的 px；把 `--size`
 * 抬到几百只会得到一块比例崩掉、又被 `overflow:hidden` 裁掉的桌面小牌。这里
 * 另起一套 `stage.css` 网格，只复用 `state.css` 的状态色与 `signalLabel` /
 * `formatAge` / `buildInbox` 这些纯函数。
 *
 * **但那盏灯必须搬上来。** 第一版只画了「组名 + 状态词 + 时长」，看起来像一块
 * 状态表 —— 而这一屏的灵魂是灯珠：颜色、形状（含「全局是圆时 permission / error
 * 画圆角方」的色盲特判）、以及该状态自己的闪烁节奏。所以每张卡里都有一颗
 * `.stage-dot`，它从 `lampVars(settings)` 拿 `--c-*` 与 `--anim-*`、从
 * `resolveShapes` 拿形状、从 `rhythmDelay` 拿呼吸相位 —— 和桌面上的灯**同源**，
 * 不是另画一个圆。尺寸是唯一分开的地方：这里按卡片宽度算（见 `stage.css`），
 * 因为 `--size` 那套 px 抬到这个尺度会全部失衡。
 *
 * **只画灯牌本来就有的语义**：组名、状态词、持续时长。收件箱详情、审批气泡、
 * 命令内容一律不搬上来 —— 会议投屏时这就是天然的脱敏，不需要两个档位。
 *
 * 顶部第二行是「离开这段时间」摘要，复用收件箱那份 `buildAwaySummary`：
 * 起点是**打开展示模式那一刻**（Rust 记在内存里的 `stage_since_ms`，不落盘），
 * 正好对应用户走开的时刻；不足 10 分钟不显示，和收件箱同一条阈值。
 */
export function StageWindow({ snapshot, settings }: {
  snapshot: Snapshot;
  settings: Settings;
}) {
  const m = useMessages();
  const lang = useLang();
  const toggles = useToggles();
  // 窗口是**启动时就加载、只是不显示**的静态窗口，所以这一页在没有展示时会一直
  // 挂在后台。`useNow` 的定时器只在真正开着时才起，别让一个隐藏窗口每秒算一次
  // 「已持续」。
  const now = useNow(toggles.stage);

  // 颜色（`--c-*`）与逐状态节奏（`--anim-*`）都由这一份变量提供，挂在 `.stage`
  // 根上 —— 和灯球用的是同一个 `lampVars`，用户在外观里改的配色 / 闪烁速度
  // 会原样反映到大屏，不用第二套映射。
  const vars = useMemo(() => lampVars(settings), [settings]);
  const shapes = useMemo(() => resolveShapes(settings), [settings]);

  // 键盘逃生。静态窗口配的是 focus:false，Rust 在 `present()` 里主动 set_focus；
  // 这里仍保留托盘与全局快捷键两条不依赖焦点的通道（见 stage.rs / tray.rs）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (inTauri) setStage(false).catch((err) => console.error("退出展示模式失败", err));
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const leave = () => {
    if (inTauri) setStage(false).catch((err) => console.error("退出展示模式失败", err));
  };

  // 顺序和隐藏都跟灯牌一致（`orderVisible` 与 Rust 的 visible_groups 同一套规则），
  // 不另排一次 —— 大屏上最要紧的那组必须仍然在最前面。
  const cards = useMemo(
    () =>
      orderVisible(snapshot.groups, settings.hidden_groups, settings.group_meta).map((group) => ({
        key: group.name,
        name: labelOf(group, settings.group_meta, labelPrefOf(settings)),
        state: group.aggregate,
        // 形状与灯珠一致：含色盲特判，不能在大屏上简化成「一律圆形」。
        shape: shapes[group.aggregate],
        sinceMs: group.since_ms,
        // 沉默进度：空心环用它画那截逐渐缩短的弧（和灯珠同一条 `--progress`）。
        progress: group.stale_progress,
        // 呼吸相位按组名散开，多组同时「在干活」时不会整块板同频闪。
        rhythm: rhythmDelay(settings, group.name, group.aggregate),
      })),
    [snapshot.groups, settings, shapes],
  );

  // 「此刻」这一行与离开摘要共用同一份摊平后的行，不新增 IPC：
  // buildInbox 是纯函数， AwaySummary 只读它的分段数组。
  const model = useMemo(
    () => buildInbox(snapshot, [], settings.group_meta, labelPrefOf(settings)),
    [snapshot, settings],
  );

  // 措辞直接复用收件箱那套：`nowParts` 和下面的离开摘要读的是同一份分段，
  // 不该一处写「条」一处写「个」。
  const nowParts = useMemo(() => {
    const parts: string[] = [];
    if (model.need.length > 0) parts.push(m.inbox.awayNeed(model.need.length));
    if (model.done.length > 0) parts.push(m.inbox.awayDone(model.done.length));
    if (model.running.length > 0) parts.push(m.inbox.awayRunning(model.running.length));
    if (parts.length === 0) parts.push(m.inbox.awayQuiet);
    return parts;
  }, [model, m.inbox]);

  // 事件流水那条通道刻意不接：展示模式不引入新的数据来源，`events` 传空数组时
  // `buildAwaySummary` 会安静地只数快照里还在的行（见 awaySummary.ts 的降级）。
  const away = useMemo(
    () => buildAwaySummary(model, now, toggles.stage_since_ms || null, []),
    [model, now, toggles.stage_since_ms],
  );
  const awayParts = useMemo(() => {
    if (!away) return [];
    if (isQuiet(away)) return [m.inbox.awayQuiet];
    const parts: string[] = [];
    // 顺序和收件箱一致：等你 > 失败 > 完成 > 在跑
    if (away.need > 0) parts.push(m.inbox.awayNeed(away.need));
    if (away.failed > 0) parts.push(m.inbox.awayFailed(away.failed));
    if (away.done > 0) parts.push(m.inbox.awayDone(away.done));
    if (away.running > 0) parts.push(m.inbox.awayRunning(away.running));
    return parts;
  }, [away, m.inbox]);

  // 列数按组数算，不交给 `auto-fit`：它只认「一格至少多宽」，于是 5 组会排出
  // 4 + 1、9 组排出 4 + 4 + 1，最后一行永远填不满。规则与取舍见 `lib/stageGrid.ts`。
  const cols = useMemo(() => stageColumns(cards.length), [cards.length]);

  return (
    <div className="stage" data-state={snapshot.aggregate} style={vars}>
      <header className="stage-head">
        <div className="stage-head-main">
          {/* 总览灯：整屏最差的那一档。真实灯球上主要就是看这一颗 —— 远处先
              知道「是红是绿」，再低头找是哪一组。它和有组灯珠共用同一套形状 /
              节奏规则，只是尺寸小一档（见 stage.css 的 `.stage-head-dot`）。 */}
          <span
            aria-hidden="true"
            className="stage-dot stage-head-dot"
            data-shape={shapes[snapshot.aggregate]}
            data-state={snapshot.aggregate}
          />
          <span className="stage-title">{m.stage.title}</span>
          <span className="stage-count">{m.stage.groups(cards.length)}</span>
        </div>
        <div className="stage-head-actions">
          <span className="stage-hint">{m.stage.exitHint(toggles.stage_shortcut)}</span>
          <button type="button" className="stage-exit" onClick={leave}>
            {m.stage.exit}
          </button>
        </div>
      </header>

      <div className="stage-summary" role="status">
        <div className="stage-summary-row">
          <span className="stage-summary-label">{m.stage.now}</span>
          <span className="stage-summary-parts">{nowParts.join(" · ")}</span>
        </div>
        {away && awayParts.length > 0 && (
          <div className="stage-summary-row">
            {/* 复用收件箱那句「你离开的 12m 里」，不另造一份措辞 */}
            <span className="stage-summary-label">
              {m.inbox.awayFor(formatSpan(away.awayMs))}
            </span>
            <span className="stage-summary-parts">{awayParts.join(" · ")}</span>
          </div>
        )}
      </div>

      <main
        className="stage-grid"
        style={{ "--stage-cols": cols } as CSSProperties}
      >
        {cards.length === 0 ? (
          <p className="stage-empty">{m.stage.empty}</p>
        ) : (
          cards.map((card) => (
            <article className="stage-card" data-state={card.state} key={card.key}>
              {/* 灯珠。`aria-hidden` 是因为它和右边那行状态词是同一件事 ——
                  读屏时念两遍没有意义。 */}
              <span
                aria-hidden="true"
                className="stage-dot"
                data-shape={card.shape}
                data-state={card.state}
                style={
                  {
                    "--rhythm-delay": card.rhythm,
                    ...(card.progress != null ? { "--progress": card.progress } : {}),
                  } as CSSProperties
                }
              />
              <span className="stage-card-body">
                <span className="stage-card-state">{signalLabel(lang, card.state)}</span>
                <span className="stage-card-name" title={card.name}>
                  {card.name}
                </span>
                <span className="stage-card-age">{formatAge(card.sinceMs, now)}</span>
              </span>
            </article>
          ))
        )}
      </main>

      <footer className="stage-foot">{m.stage.foot}</footer>
    </div>
  );
}
