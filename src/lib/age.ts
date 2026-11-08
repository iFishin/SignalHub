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
  const seconds = Math.max(0, Math.round((now - atMs) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}
