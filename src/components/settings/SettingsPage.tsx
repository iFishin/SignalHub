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
}

/**
 * 复杂页面的第二层导航。最多这一层，不再往下嵌。
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
          {item.label}
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
