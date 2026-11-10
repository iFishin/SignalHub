import { useEffect, useMemo, useRef, useState } from "react";
import { SignalLamp } from "./SignalLamp";
import {
  DEMO_SCENARIOS,
  DEMO_STEPS,
  demoSnapshot,
  nextDemoStep,
  prevDemoStep,
  scenarioStart,
  scenarioSteps,
  stepScenario,
  type DemoGroupId,
  type DemoScenarioId,
} from "../lib/demo";
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
 *
 * **按场景播，不是一条长剧本**：上面一排场景 chip，每个场景自己循环。想看
 * 「折叠成一颗灯」不用先等 20 秒别的步骤 —— 那正是这类演示最容易劝退人的地方。
 */
export function DemoStage({ compact = false }: Props) {
  const m = useMessages();
  const settings = useSettings();
  // `step` 始终是 `DEMO_STEPS` 的全局下标，场景只是它的一段区间。
  const [step, setStep] = useState(0);
  const [playing, setPlaying] = useState(true);

  // since_ms / lastDoneMs 都以挂载时刻为基准固定下来，牌上的「已持续」和
  // 「刚完成 · N 分钟前」由 SignalLamp 自己的钟往前走。这样 DemoStage 不必每秒
  // 重渲染一次（也就不会因为 groups 换了引用而反复触发跃迁判定）。
  const startedAt = useRef(Date.now()).current;

  const scenario = stepScenario(step);
  const start = scenarioStart(scenario);
  const steps = scenarioSteps(scenario);
  const offset = step - start;

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

  const labels: Record<DemoGroupId, string> = m.demoGroup;
  // frame 里的 collapsed / quiet 由剧本给，不是用户设置 —— 演示要能展示
  // 「收起来」「安静」这两种真实存在的形态，而不只是跟着用户当前的设置走。
  const frame = useMemo(
    () => demoSnapshot(step, startedAt, labels),
    [step, startedAt, labels],
  );

  const demoSettings: Settings = useMemo(
    () => ({
      ...settings,
      collapsed: frame.collapsed,
      quiet: frame.quiet,
      shape: frame.shape ?? settings.shape,
      size: compact ? 22 : 30,
      sign_opacity: 100,
      show: { name: true, state: true, age: true, message: true, dot: true },
      // 不继承用户的个性化：组名、隐藏的组、逐状态的颜色 / 形状覆盖。
      // 演示讲的是产品默认语义，不是「你把这盏灯调成了什么样」。
      // 字体和间距同理 —— 换过等宽 / 宽松间距的人，看演示时该看到默认那张牌。
      sign_font: "system",
      sign_gap: "standard",
      group_meta: {},
      hidden_groups: [],
      styles: {},
      lamp_layout: "single",
    }),
    [settings, compact, frame.collapsed, frame.quiet, frame.shape],
  );

  const info = m.demoStep[DEMO_STEPS[step].id];

  /** 切场景 / 逐步都保持「场景内循环」，这样首尾相接不会跳画面。 */
  const goTo = (index: number) => {
    setStep(index);
    setPlaying(false);
  };

  const pickScenario = (id: DemoScenarioId) => {
    setStep(scenarioStart(id));
    // 点场景 = 想看这一段，所以顺便恢复播放；暂停只用于「停在某一步看细节」。
    setPlaying(true);
  };

  return (
    <div className="demo" data-compact={compact || undefined}>
      <div className="demo-scenarios" role="tablist" aria-label={m.demoTitle}>
        {DEMO_SCENARIOS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            data-active={item.id === scenario}
            aria-selected={item.id === scenario}
            onClick={() => pickScenario(item.id)}
          >
            {m.demoScenario[item.id]}
          </button>
        ))}
      </div>

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
          <span className="demo-step-no">
            {m.demoStepCount(offset + 1, steps.length)}
          </span>
          <h2>{info.title}</h2>
        </div>
        <p>{info.body}</p>
      </div>

      <div className="demo-controls">
        <div className="demo-progress" role="tablist" aria-label={m.demoTitle}>
          {steps.map((item, index) => (
            <button
              key={item.id}
              type="button"
              role="tab"
              className="demo-progress-seg"
              data-active={index === offset}
              data-done={index < offset}
              aria-selected={index === offset}
              aria-label={m.demoStepCount(index + 1, steps.length)}
              onClick={() => goTo(start + index)}
            />
          ))}
        </div>
        <div className="demo-buttons">
          <button
            type="button"
            className="btn"
            onClick={() => goTo(prevDemoStep(step))}
          >
            {m.demoPrev}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => setPlaying((value) => !value)}
          >
            {playing ? m.demoPause : m.demoPlay}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => goTo(nextDemoStep(step))}
          >
            {m.demoNext}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              setStep(start);
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
