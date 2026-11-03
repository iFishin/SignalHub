import type { Messages } from "./i18n";
import type { DenyReason } from "./useApprovals";

/**
 * 危险分类的键 → 一句人话（`docs/plan-approval.md` A3）。
 *
 * 键由 Rust 的 `hookmap::danger` 给，翻译在 i18n 里。认不出来的键返回 `null`：
 * **不画横幅比画一条错的强** —— 红横幅一旦说错话，用户下次就不信它了。
 */
export function dangerLabel(m: Messages, key: string | null | undefined): string | null {
  if (!key) return null;
  return (m.approvalDanger as Record<string, string>)[key] ?? null;
}

/** 拒绝理由的键 → 界面上那个词。 */
export function reasonLabel(m: Messages, reason: DenyReason): string {
  switch (reason) {
    case "unsafe":
      return m.approvalReasonUnsafe;
    case "safer":
      return m.approvalReasonSafer;
    case "explain":
      return m.approvalReasonExplain;
    case "custom":
      return m.approvalReasonCustom;
  }
}
