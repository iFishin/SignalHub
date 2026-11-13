/**
 * 「多久以前」。
 *
 * 入参是 epoch 毫秒，`now` 由调用方传进来（见 `useNow`），**不在这里读
 * `Date.now()`** —— 那样它就只在重绘时被求值，一个不重绘的面板会把这个数字
 * 冻在那一刻。
 *
 * 从 `SettingsPanel` 里提出来是为了 Call Me 页复用：两处各写一份的话，
 * 「3 分钟前」什么时候变「1 小时前」就会在两张页面上不一致。
 */
export function formatAge(atMs: number, now: number): string {
  return formatSpan(Math.max(0, now - atMs));
}

/**
 * 「还有多久」。和 [`formatAge`] 同一套单位，方向相反。
 *
 * 单独一个函数而不是传个负数进去：`formatAge` 那个 `Math.max(0, …)` 会把未来
 * 的时刻压成 `0s` —— 正好把要显示的东西抹掉。牌上「推迟到 14m 后」那一格用它。
 */
export function formatRemaining(untilMs: number, now: number): string {
  return formatSpan(Math.max(0, untilMs - now));
}

function formatSpan(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}
