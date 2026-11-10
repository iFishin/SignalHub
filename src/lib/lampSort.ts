import {
  DEFAULT_SIGN_LABEL_TEMPLATE,
  type GroupMeta,
  type Settings,
  type SignLabel,
} from "./useSettings";
import type { GroupView } from "./types";

/**
 * 置顶排序的比较器。
 *
 * **必须是稳定的**（JS 的 `Array.prototype.sort` 从 ES2019 起保证稳定）：没置顶的组
 * 要原样保持服务端给的**紧急度**顺序，置顶组之间按权重、同权重时也保持原顺序 ——
 * 一条规则说清两件事。
 *
 * **与 Rust 的 `visible_groups` 里那个 `sort_by_key` 是同一条规则的两份实现。**
 * 两边必须算出同一批可见的牌，否则窗口高度会对不上：多算一块牌 → 底下空一条，
 * 少算一块 → 最后一块被 `overflow: hidden` 切掉。这也是它被从组件里摘出来、
 * 单独放一个文件并配单测的原因 —— 埋在 `SignalLamp.tsx` 里就没人会去测它。
 */
export function comparePinned(
  a: GroupView,
  b: GroupView,
  meta: Record<string, GroupMeta>,
): number {
  const left = meta[a.name]?.order ?? null;
  const right = meta[b.name]?.order ?? null;
  // 两边都没置顶：返回 0 让**稳定排序**把它们留在服务端给的紧急度顺序上
  if (left === right) return 0;
  if (left === null) return 1; // 没置顶的排到所有置顶组之后
  if (right === null) return -1;
  return left - right;
}

/** 会话标题的偏好。前后端 `Settings` 里那两个字段的组合，抽出来给纯函数用。 */
export interface LabelPref {
  mode: SignLabel;
  template: string;
}

export const DEFAULT_LABEL_PREF: LabelPref = {
  mode: "session",
  template: DEFAULT_SIGN_LABEL_TEMPLATE,
};

/** 从整份设置里只取拼标题要用的两项，免得每个组件都自己写一遍对象字面量。 */
export function labelPrefOf(
  settings: Pick<Settings, "sign_label" | "sign_label_template">,
): LabelPref {
  return { mode: settings.sign_label, template: settings.sign_label_template };
}

/** 拼标题的原料。只从**快照自带的会话**里取，不另造一份服务端字段。 */
export interface LabelParts {
  /** 会话 / 项目名：工作目录的末段。 */
  session: string | null;
  /** 上报方给的 `agent`。 */
  agent: string | null;
  /** 原始组名（按会话分组时就是会话 id）。 */
  group: string;
}

/**
 * 与 Rust `config.rs::project_name` 同一条规则：按 `/` 或 `\` 取最后一个非空段。
 * 不自己造一套「取末两级」之类的花样，否则灯上和转发摘要里的项目名会对不上。
 */
function projectName(cwd: string): string {
  const parts = cwd.split(/[/\\]/).filter((part) => part !== "");
  return parts.length > 0 ? parts[parts.length - 1] : cwd;
}

/**
 * 这一组是不是「按会话分出来的」。判据和 Rust `state.rs::session_label` 一样：
 * 只有那种情况下组名才会恰好等于组内某个会话的 id。显式给 `group`、按目录分组、
 * 远程组都取不到，此时返回 `null`，标题继续用服务端给的 `default_label`。
 *
 * **为什么不另加一个服务端字段**：`GroupView` 已经带着 `sessions[]`（`agent` /
 * `cwd` 都在里面），再传一份「拆好的标题原料」就是两处真值；而且那会让每个构造
 * `GroupView` 的 Rust 测试和桩都多一个字段要填。
 */
export function labelParts(group: GroupView): LabelParts | null {
  const session = group.sessions.find((item) => item.id === group.name);
  if (!session) return null;
  const agent = session.agent?.trim() || null;
  const project = session.cwd ? projectName(session.cwd) || null : null;
  if (!agent && !project) return null;
  return { session: project, agent, group: group.name };
}

/** 自定义模板里的占位符。只认这三个，写错了就原样留着（用户能看见自己写错了）。 */
const LABEL_TOKEN = /\{(session|agent|group)\}/g;

/**
 * 把模板里的空占位符收拾干净。
 *
 * 例：模板 `{session} · {agent}` 而 agent 没上报时，直接替换会得到
 * `"SignalHub · "` —— 尾巴上挂个分隔符，比不显示智能体名更像 bug。所以替换完
 * 要把连着的分隔符并成一个、掐掉首尾、并掉多余空白；全空时返回空串让调用方
 * 回落，绝不显示一个孤零零的「·」。
 */
function renderLabelTemplate(template: string, parts: LabelParts): string {
  const values: Record<string, string> = {
    session: parts.session ?? "",
    agent: parts.agent ?? "",
    group: parts.group,
  };
  return template
    .replace(LABEL_TOKEN, (_, key: string) => values[key] ?? "")
    .replace(/(?:\s*[·|\/\\-]\s*){2,}/g, " · ")
    .replace(/^[\s·|\/\\-]+|[\s·|\/\\-]+$/g, "")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/**
 * 这一组在灯上叫什么（**不含用户自定义显示名**，那一级在 [`labelOf`] 里）。
 *
 * 三级：用户设的显示名 > 按标题偏好拼出来的名字 > 服务端默认名 / 原始组名。
 *
 * 0.6.4 之前服务端给的是「目录末段 · 智能体」，牌宽固定 `size × 7`，那一行还要
 * 跟状态词分，`SignalHub · workbuddy` 里的 `workbuddy` 几乎每次都被省略号吃掉。
 * 默认改成只显示会话名（`session`）；`with_agent` 是旧观感，`custom` 让用户自己
 * 拿 `{session}` / `{agent}` / `{group}` 拼一个模板。
 *
 * 注意**只是显示**：隐藏 / 静音的匹配、排序、`/v1/state` 里的组名都仍然是
 * `group.name`。转发摘要和 Web 面板仍用 `default_label` 的全名 —— 它们不受牌宽
 * 限制，多一个智能体名是有用的信息，不是需要削掉的噪音。
 */
export function composeLabel(group: GroupView, pref: LabelPref = DEFAULT_LABEL_PREF): string {
  const fallback = group.default_label ?? group.name;
  const parts = labelParts(group);
  // 不是会话组（显式组名 / 目录分组 / 远程组）：没有「智能体名」可藏，原样显示。
  if (!parts) return fallback;

  // 模板全空 / 只写了一个没上报的占位符时，回落到**短名**而不是那个带智能体名
  // 的 `default_label`：否则「我只想要个干净的标题」会被一个空模板绕回旧观感。
  const short = parts.session ?? parts.agent ?? fallback;
  switch (pref.mode) {
    case "session":
      // 没有项目名时退回智能体名：那时能显示的只有一串会话 id，比 agent 更难读。
      return short;
    case "with_agent":
      return fallback;
    case "custom":
      return renderLabelTemplate(pref.template, parts) || short;
  }
}

export function labelOf(
  group: GroupView,
  meta: Record<string, GroupMeta>,
  pref: LabelPref = DEFAULT_LABEL_PREF,
): string {
  return meta[group.name]?.name ?? composeLabel(group, pref);
}

/**
 * 灯上真正会画出来的组：滤掉隐藏的，再叠一层用户的置顶偏好。
 *
 * 顺序和截断都要和 Rust 的 `visible_groups` 一致（见 `comparePinned` 的说明）。
 * 截断留给调用方 —— 这里只回答「按什么顺序排」。
 */
export function orderVisible(
  groups: GroupView[],
  hidden: string[],
  meta: Record<string, GroupMeta>,
): GroupView[] {
  return groups
    .filter((group) => !hidden.includes(group.name))
    .sort((a, b) => comparePinned(a, b, meta));
}
