/**
 * 展示模式大屏上「每行放几组」。
 *
 * **为什么不用 CSS 的 `auto-fit`**：它只按「一格至少多宽」切列，完全不知道有几
 * 组。实测 5 组在 1600px 上切出 4 列 —— 第一行四张、第二行孤零零一张，那张还
 * 和上面四张一样宽；9 组则是 4 + 4 + 1。大屏是给远处看的，这种「多出来的一张」
 * 比字小得多还刺眼，而且最后一行永远填不满。
 *
 * 所以列数由组数直接定，而不是靠权重算：**同一组数每次必须排成同一个样子**，
 * 而「1.6 的格子比例 + 空格惩罚」这类打分很容易在某个体裁上翻车（试过：3 组会
 * 被算成一列竖排）。表里的每一条都对着真实组数挑过 —— 4 组 2×2 比一条四列窄带
 * 好读；7 组 4 + 3 比 3 + 3 + 1 好看；9 组 3×3 比一行九列强。
 *
 * 超过 12 组退回 `ceil(sqrt(n))`：那时候格子已经小到「排得下」本身就是主要
 * 目标，且灯最多带 8 盏，12 组以上属于长尾。
 */

/** 1..12 组时的列数。下标即组数，`0` 位不用。 */
const COLUMNS = [0, 1, 2, 3, 2, 3, 3, 4, 4, 3, 5, 4, 4] as const;

export function stageColumns(count: number): number {
  if (!Number.isFinite(count)) return 1;
  const total = Math.floor(count);
  if (total <= 0) return 1;
  if (total < COLUMNS.length) return COLUMNS[total];
  return Math.ceil(Math.sqrt(total));
}
