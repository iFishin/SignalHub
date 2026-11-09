/**
 * Call Me 的线上类型与纯函数。
 *
 * 类型与 Rust 侧 `src-tauri/src/callme.rs` 的 `CallMeState` / `SourceView` /
 * `MessageView` 一一对应（snake_case）。纯函数放这里，是为了能用 vitest 盯住
 * 「未读怎么算」「链接怎么拼」这两件容易悄悄错、又不容易在界面上一眼看出来的事。
 */

export interface CallMeSource {
  id: string;
  label: string;
  note: string | null;
  /** URL 里的令牌。只给本人看（来源管理区），不要展示在别处。 */
  token: string;
  /** 服务器给的相对路径（`/c/<token>`）。完整链接要拼自己配的域名。 */
  link: string;
  created_ms: number;
  revoked: boolean;
  /** 名字还是服务器给的占位，等第一条带标题的留言来命名。旧服务器没这个字段。 */
  auto_label?: boolean;
  total: number;
  /** 近 24 小时收到几条（与服务器配额同口径）。 */
  today: number;
  last_received_ms: number;
}

export interface CallMeMessage {
  seq: number;
  id: string;
  source_id: string;
  received_ms: number;
  title: string | null;
  text: string;
  /** 智能体自报的名字，不可信，只当提示。 */
  agent: string | null;
  /** 智能体自报的「从哪读到链接的」。可能是 URL，也可能是一句描述。 */
  context: string | null;
}

export interface CallMeState {
  enabled: boolean;
  ok: boolean;
  last_ok_ms: number;
  latency_ms: number;
  error: string | null;
  sources: CallMeSource[];
  /** 按 seq 升序。 */
  messages: CallMeMessage[];
  /** 每个来源读到第几条（`source_id → last_read_seq`）。 */
  read: Record<string, number>;
}

export interface CallMeProbe {
  ok: boolean;
  latency_ms: number;
  error: string | null;
  version: string | null;
  sources: number;
}

/** 桌面进程内的本机服务。与 Rust `callme_local::LocalStatus` 对应。 */
export interface CallMeLocalStatus {
  running: boolean;
  url: string | null;
  token: string | null;
  port: number;
  data_dir: string | null;
  error: string | null;
}

/**
 * 服务器地址归一化 —— 与 Rust 侧 `callme::endpoint` 同一套规矩。
 *
 * 默认补 **https**：这是公网服务，和多机中枢（默认局域网、补 http）不同。
 */
export function normalizeBase(raw: string): string {
  const trimmed = raw.trim().replace(/\/+$/, "");
  const base = trimmed.replace(/\/v1\/callme$/, "").replace(/\/+$/, "");
  if (/^https?:\/\//.test(base)) return base;
  return `https://${base}`;
}

/** 完整可复制的链接：自己配的域名 + 服务器给的相对路径。 */
export function fullLink(base: string, source: CallMeSource): string {
  const root = normalizeBase(base);
  const path = source.link.startsWith("/") ? source.link : `/${source.link}`;
  return `${root}${path}`;
}

/** 某个来源读到第几条（没记过 = 0）。 */
export function readOf(read: Record<string, number>, sourceId: string): number {
  return read[sourceId] ?? 0;
}

/**
 * 未读条数。`seq > 已读游标` 的都算未读 —— 游标是「读到第几条」，
 * 比逐条记 id 集合更省、也不会因为消息被服务端修剪而漂移。
 */
export function unreadCount(
  messages: CallMeMessage[],
  read: Record<string, number>,
  sourceId?: string,
): number {
  return messages.filter(
    (message) =>
      (sourceId === undefined || message.source_id === sourceId) &&
      message.seq > readOf(read, message.source_id),
  ).length;
}

/** 每个来源各自的未读条数（给筛选 chip 上的数字用）。 */
export function unreadBySource(
  messages: CallMeMessage[],
  read: Record<string, number>,
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const message of messages) {
    if (message.seq <= readOf(read, message.source_id)) continue;
    out[message.source_id] = (out[message.source_id] ?? 0) + 1;
  }
  return out;
}

/** 某个来源已收到的最大 seq（「全部已读」要写这个值）。 */
export function lastSeqOf(
  messages: CallMeMessage[],
  sourceId: string,
): number {
  let max = 0;
  for (const message of messages) {
    if (message.source_id === sourceId && message.seq > max) max = message.seq;
  }
  return max;
}

/** 按来源过滤（`null` = 全部）。顺序保持不变（新→旧由调用方决定）。 */
export function filterBySource(
  messages: CallMeMessage[],
  sourceId: string | null,
): CallMeMessage[] {
  if (sourceId === null) return messages;
  return messages.filter((message) => message.source_id === sourceId);
}

/**
 * 从 `context` 里取一个可打开的地址。
 *
 * **只认 http/https**：它是智能体自报的不可信输入，`file://` / `javascript:` 之类
 * 不该被交给系统去「打开」。Rust 侧 `open_url` 还有一道同样的校验，两道都留着 ——
 * 这一道决定按钮要不要显示。
 */
export function openableUrl(context: string | null | undefined): string | null {
  if (!context) return null;
  const trimmed = context.trim();
  return /^https?:\/\//.test(trimmed) ? trimmed : null;
}

/** 留言列表展示顺序：新→旧。服务端给的是旧→新（游标语义需要）。 */
export function newestFirst(messages: CallMeMessage[]): CallMeMessage[] {
  return [...messages].sort((a, b) => b.seq - a.seq);
}
