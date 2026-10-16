/**
 * 下载进度的**纯**换算。抽出来是为了能单测 —— 这是这条链路里唯一有「算错也
 * 不报错、只是显示得不对」风险的地方：百分比算错会让进度条倒着走。
 */

/** 下载进度的形状，与 Rust 侧 `update::Progress` 一一对应。 */
export type Progress = {
  downloaded: number;
  /** 总量未知时是 null（服务端没给 Content-Length） */
  total: number | null;
  /** 下载完了，正在替换/安装 */
  installing: boolean;
};

/**
 * 字节数human读法。只给「已下载多少」用，所以只要一位小数就够。
 *
 * 1024 进制而不是 1000：这个数会和服务端给的 `Content-Length` 并排出现，
 * 两边用同一套进制才不会看起来对不上。
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}

/**
 * 0–100 的百分比。**总量未知时返回 null** —— 那不是 0%，是「算不出来」，
 * 调用方要据此决定画不画进度条（画一条永远不动的 0% 比不画更让人困惑）。
 */
export function percent(progress: Progress): number | null {
  const { downloaded, total } = progress;
  if (total === null || total <= 0) return null;
  // 夹在 0–100：分块回调有并发，累加偶尔会越一点点，进度条不该因此跑出界
  return Math.min(100, Math.max(0, Math.round((downloaded / total) * 100)));
}

/**
 * 进度那一行字：**优先百分比，总量未知就退回「已下载多少」**。
 *
 * 没有 `Content-Length` 时算不出百分比 —— 那种情况显示一个乱跳的百分比比显示
 * 字节数更糟：用户没法判断它是不是卡住了。
 */
export function progressLabel(progress: Progress): string {
  const value = percent(progress);
  return value === null ? formatBytes(progress.downloaded) : `${value}%`;
}
