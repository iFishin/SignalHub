/**
 * 「设置 → 接入 → 自定义智能体」里那张添加表单的**轻量预校验**。
 *
 * ⚠️ 这里不是规则的真值。唯一真值在 Rust 的 `agents::validate` / `insert`
 * （CLI 与 IPC 共用同一份源码）。这里只挡住最常见的手误，省掉一次 IPC 往返；
 * 少挡一条不会漏过去 —— Rust 的 `Err(String)` 会原样显示在表单下方。
 * 刻意**不复刻** `normalize_id`（大写转小写、`_` 转 `-`）和 id 长度上限：
 * 复刻一份就会在「`My_Agent` 到底合不合法」上和 Rust 分叉，而那正是这个项目
 * 最忌讳的两处真值。id 的字符、长度、内置重名一律交给 Rust 判。
 */

export interface AgentDraft {
  id: string;
  name: string;
  family: string;
  config: string;
  marker: string;
  command: string;
}

export type DraftProblem =
  | "idRequired"
  | "nameRequired"
  | "familyRequired"
  | "configRequired"
  | "configParent"
  | "commandSpaces";

export const EMPTY_AGENT_DRAFT: AgentDraft = {
  id: "",
  name: "",
  family: "",
  config: "",
  marker: "",
  command: "",
};

/** 只检查「不看配置文件也知道填错了」的那几条。返回 null 就是可以提交。 */
export function checkAgentDraft(draft: AgentDraft, families: string[]): DraftProblem | null {
  if (!draft.id.trim()) return "idRequired";
  if (!draft.name.trim()) return "nameRequired";

  // families 来自运行时的 `custom_agents_info`，不从这里抄一份清单。
  // 数据还没回来（空数组）时不拦：Rust 会给出「不认识的 family」那句准确的话。
  if (families.length > 0 && !families.includes(draft.family)) return "familyRequired";

  const config = draft.config.trim();
  if (!config) return "configRequired";
  // `..` 只按路径段认，别把 `foo..bar.json` 这种正常文件名也拦掉。
  if (/(^|[/\\])\.\.([/\\]|$)/.test(config)) return "configParent";

  // marker 是可选的，同规则；command 只能是命令名，不能带参数或空格。
  if (/(^|[/\\])\.\.([/\\]|$)/.test(draft.marker.trim())) return "configParent";
  if (/\s/.test(draft.command.trim())) return "commandSpaces";

  return null;
}
