import { useEffect, useMemo, useRef, useState } from "react";
import { SignalLamp } from "./SignalLamp";
import { DEMO_STEPS, demoSnapshot, nextDemoStep, type DemoGroupId } from "../lib/demo";
import { useMessages } from "../lib/i18n";
import { useSettings, type Settings } from "../lib/useSettings";

interface Props {
  /** 设置窗里那一页空间小，牌缩小一档；独立演示页用大一号。 */
  compact?: boolean;
}

/**
 * 一段会自己播的「产品说明」。
 *
 * 画面**直接复用 SignalLamp 的 preview 模式**，不是另抄一份牌的标记 —— 灯牌
 * 改了颜色、形状、节奏，这里跟着变，不会慢慢变成一份过时的说明书。
 * 交互全部关掉：这是给人看的，不是给人点的（点一下不该打开收件箱）。
 */
export function DemoStage({ compact = false }: Props) {
  const m = useMessages();
  const settings = useSettings();
  const [step, setStep] = useState(0);
  const [playing, setPlaying] = useState(true);

  // since_ms / lastDoneMs 都以挂载时刻为基准固定下来，牌上的「已持续」和
  // 「刚完成 · N 分钟前」由 SignalLamp 自己的钟往前走。这样 DemoStage 不必每秒
  // 重渲染一次（也就不会因为 groups 换了引用而反复触发跃迁判定）。
  const startedAt = useRef(Date.now()).current;

  // 每一步一个定时器，暂停时自然不再排下一个。逐步而不是一条长 timeline，
  // 是为了「点上一/下一步」和「点进度条」能立刻对齐，不用处理剩余时间。
  useEffect(() => {
    if (!playing) return;
    const timer = window.setTimeout(
      () => setStep((current) => nextDemoStep(current)),
      DEMO_STEPS[step].durationMs,
    );
    return () => window.clearTimeout(timer);
  }, [playing, step]);

  const demoSettings: Settings = useMemo(
    () => ({
      ...settings,
      // 演示一律展开、一律不半透明：安静模式会把「空闲」压到几乎看不见，
      // 那正好是这段剧本开头要讲清楚的第一帧。
      collapsed: false,
      quiet: false,
      size: compact ? 22 : 30,
      sign_opacity: 100,
      show: { name: true, state: true, age: true, message: true },
      // 不继承用户对真实组起的名 / 隐藏的组 —— 剧本里的组是固定的三个
      group_meta: {},
      hidden_groups: [],
      lamp_layout: "single",
    }),
    [settings, compact],
  );

  const labels: Record<DemoGroupId, string> = m.demoGroup;
  const frame = useMemo(
    () => demoSnapshot(step, startedAt, labels),
    [step, startedAt, labels],
  );
  const info = m.demoStep[DEMO_STEPS[step].id];
  const total = DEMO_STEPS.length;

  const goTo = (index: number) => {
    setStep((index + total) % total);
    setPlaying(false);
  };

  return (
    <div className="demo" data-compact={compact || undefined}>
      <div className="demo-stage">
        <div className="demo-lamp">
          <SignalLamp
            preview
            aggregate={frame.aggregate}
            groups={frame.groups}
            settings={demoSettings}
            paused={false}
            lastDoneMs={frame.lastDoneMs}
          />
        </div>
        <span className="demo-badge">{m.demoNotReal}</span>
      </div>

      <div className="demo-caption" aria-live="polite">
        <div className="demo-caption-head">
          <span className="demo-step-no">{m.demoStepCount(step + 1, total)}</span>
          <h2>{info.title}</h2>
        </div>
        <p>{info.body}</p>
      </div>

      <div className="demo-controls">
        <div className="demo-progress" role="tablist" aria-label={m.demoTitle}>
          {DEMO_STEPS.map((item, index) => (
            <button
              key={item.id}
              type="button"
              role="tab"
              className="demo-progress-seg"
              data-active={index === step}
              data-done={index < step}
              aria-selected={index === step}
              aria-label={m.demoStepCount(index + 1, total)}
              onClick={() => goTo(index)}
            />
          ))}
        </div>
        <div className="demo-buttons">
          <button type="button" className="btn" onClick={() => goTo(step - 1)}>
            {m.demoPrev}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => setPlaying((value) => !value)}
          >
            {playing ? m.demoPause : m.demoPlay}
          </button>
          <button type="button" className="btn" onClick={() => goTo(step + 1)}>
            {m.demoNext}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              setStep(0);
              setPlaying(true);
            }}
          >
            {m.demoReplay}
          </button>
        </div>
      </div>
    </div>
  );
}
