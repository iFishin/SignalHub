import { ICONS, type IconName } from "../lib/icons";

/**
 * 一枚线性图标。形状来自 `lib/icons.ts`，这里只负责统一尺寸、笔画和取色。
 *
 * `stroke="currentColor"` 是整套图标的关键：它跟着所在按钮的文字色走，选中、
 * hover、禁用都不需要额外的颜色规则，也不会出现「彩色图标 + 扁平文字」的错位。
 * 图标本身对读屏器隐藏 —— 可访问名由外层按钮的 `aria-label` / `title` 给出。
 */
export function Icon({
  name,
  size = 14,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {ICONS[name].map((d, index) => (
        <path key={index} d={d} />
      ))}
    </svg>
  );
}

/**
 * 图标按钮：次要动作统一的长相。透明底、无边框、无圆角色块，hover 才给一层
 * 和 `.rail-item:hover` 同样的浅灰 —— 沿用设置面板「靠灰度分层次」的规则。
 * 文字标签必须传，图标本身不发音也不解释自己。
 */
export function IconButton({
  icon,
  label,
  onClick,
  disabled,
  danger,
}: {
  icon: IconName;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  /** 危险动作用更强的 hover 提示；仍不靠颜色单独表意，删除另有二次确认。 */
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      className="icon-btn"
      data-danger={danger || undefined}
      title={label}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name={icon} />
    </button>
  );
}
