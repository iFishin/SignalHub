import { useRef, type KeyboardEvent, type ReactNode } from "react";

/** 每页统一回答「我在哪儿、这一页能做什么」。主操作放右侧。 */
export function PageHeader({
  title,
  hint,
  children,
}: {
  title: string;
  hint: string;
  /** 页面级主操作（如「全部已读」「刷新」），渲染在右侧。 */
  children?: ReactNode;
}) {
  return (
    <header className="page-header">
      <div className="page-header-main">
        <h2>{title}</h2>
        <p>{hint}</p>
      </div>
      {children && <div className="page-header-actions">{children}</div>}
    </header>
  );
}

export interface LocalTabItem<T extends string> {
  id: T;
  label: string;
  /**
   * 一句「这一页放什么」。给了就渲染成标题下面那行小字。
   *
   * 只给**分页之间差别不明显**的页面用（外观的「基础 / 质感」就是）：光看两个
   * 词猜不出后面藏着什么，用户也就不会去点第二页。差别本来就一目了然的地方
   * （接入的智能体 / 来源 / 网络）不必加 —— 每格都塞两行会让切换条变高变吵。
   */
  hint?: string;
}

/**
 * 复杂页面的第二层导航。最多这一层，不再往下嵌。
 *
 * **不是一排小标签，是页内的分区切换**：等宽铺满、1px 轨道、选中格换底色。
 * 0.6.6 之前这里是 12.5px 的透明文字按钮，用户的原话是「明明是大的功能切换，
 * 但这里就是普通的按钮，很容易忽略掉」—— 一个切换换掉大半页内容，长得却像
 * 页脚链接，是这个控件最贵的一处误导。轨道与 `.seg` 同一套语言（细线 + 浅灰
 * 底 + 正文字色），只是尺子放大一档，不再另起一套视觉。
 *
 * 粘在内容区顶端：子页比一屏长时，滚到一半也看得见另一边还有一页。
 *
 * 左右方向键只切换页内子页；Tab 键仍按浏览器顺序进入内容。调用方保持父组件
 * 挂载，因此服务器地址、token 等未提交草稿不会因切子页丢失。
 */
export function LocalTabs<T extends string>({
  label,
  items,
  value,
  onChange,
}: {
  label: string;
  items: LocalTabItem<T>[];
  value: T;
  onChange: (next: T) => void;
}) {
  const refs = useRef<Partial<Record<T, HTMLButtonElement | null>>>({});

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const index = items.findIndex((item) => item.id === value);
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const next = items[(index + delta + items.length) % items.length];
    onChange(next.id);
    refs.current[next.id]?.focus();
  };

  return (
    <div className="local-tabs" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
      {items.map((item) => (
        <button
          key={item.id}
          ref={(node) => {
            refs.current[item.id] = node;
          }}
          type="button"
          role="tab"
          id={`local-tab-${item.id}`}
          aria-selected={value === item.id}
          aria-controls={`local-panel-${item.id}`}
          tabIndex={value === item.id ? 0 : -1}
          data-active={value === item.id}
          onClick={() => onChange(item.id)}
        >
          <span className="tab-label">{item.label}</span>
          {item.hint && <span className="tab-hint">{item.hint}</span>}
        </button>
      ))}
    </div>
  );
}

export function LocalPanel({ id, children }: { id: string; children: ReactNode }) {
  return (
    <div
      className="local-panel"
      role="tabpanel"
      id={`local-panel-${id}`}
      aria-labelledby={`local-tab-${id}`}
    >
      {children}
    </div>
  );
}
