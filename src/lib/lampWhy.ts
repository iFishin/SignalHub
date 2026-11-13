import { formatAge } from "./age";
import { signalLabel } from "./signals";
import { cornerLights, edgeLightKinds, minutesToClock, type LightContext } from "./lampStyle";
import { SIGNALS, type GroupView, type SignalState } from "./types";
import type { Lang, Messages } from "./i18n";
import type { LightTrigger, Settings } from "./useSettings";

/**
 * 右键菜单最上面那两行「为什么它在亮」。
 *
 * 起因是一个具体场景：灯突然红了，你知道「有事」，但不知道**是哪一层**在说话 ——
 * 是这一组的真实状态？是我给边框光配的「只在出错时亮」？还是悬停里那句被截断的
 * 消息？答案散在 tooltip、收件箱、设置的三页里，而灯是常驻桌面的东西，最贵的品质
 * 是**可解释**：它得能自己说清自己为什么是这个样子。
 *
 * **为什么措辞在前端算、而不是像别的菜单项那样由 Rust 的 `lang.rs` 出**：这两行
 * 解释的是**亮灯判定**（`lightOn` / `cornerLights` / `edgeLightKinds`），而那套
 * 判定只有 TS 这一份（有单测）。在 Rust 里再实现一遍，等于给「什么时候亮」造出
 * 第二处真值 —— 某天两边分叉，菜单会开始解释一个并不存在的灯。所以 Rust 只负责
 * 把这两行灰字画出来，一个字也不判断。
 *
 * 输出 0–2 行：
 *   1) 这一组是什么状态、在等什么、等了多久；
 *   2) 此刻亮着哪几个边框光灯位、各自凭什么亮（一个都没亮就不出这一行）。
 */

/** 一行里最多列几个灯位。六格全开时全列出来会把菜单撑到半屏宽。 */
const MAX_SLOTS = 3;

/** 消息在菜单里的上限。牌上靠省略号，这里没有省略号可依，自己截。 */
const MAX_MESSAGE = 40;

function clip(text: string, max: number): string {
  const chars = Array.from(text.trim());
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : chars.join("");
}

/** 九档触发在当前设置下的说法。带参数的三档把参数一起说出来。 */
function triggerText(
  trigger: LightTrigger,
  settings: Settings,
  lang: Lang,
  m: Messages,
): string {
  switch (trigger) {
    case "always":
      return m.triggerAlways;
    case "attention":
      return m.triggerAttention;
    case "active":
      return m.triggerActive;
    case "quiet":
      return m.triggerQuiet;
    case "stale":
      return m.triggerStale;
    case "message":
      return m.triggerMessage;
    case "pinned":
      return m.triggerPinned;
    case "state": {
      // 「达到指定状态」不写参数等于没说 —— 用户配的是哪一个只有这里能看到。
      const picked = settings.sign_trigger_state;
      return SIGNALS.includes(picked as SignalState)
        ? signalLabel(lang, picked as SignalState)
        : m.triggerState;
    }
    case "schedule":
      // 时段同理：说不出「几点到几点」，这一档就只是个名词。
      return `${minutesToClock(settings.sign_trigger_from)}–${minutesToClock(settings.sign_trigger_to)}`;
    default:
      return m.triggerAlways;
  }
}

export interface WhyInput {
  group: GroupView;
  settings: Settings;
  /** 和点亮那六格用的是**同一份**逐块牌事实（见 `SignalLamp` 里的 `lightCtx`）。 */
  ctx: LightContext;
  lang: Lang;
  m: Messages;
  now: number;
  /** 此刻是不是在点击穿透（全局瞬态，不在 Settings 里）。 */
  passThrough?: boolean;
}

export function whyLines(input: WhyInput): string[] {
  const { group, settings, ctx, lang, m, now, passThrough } = input;
  const lines: string[] = [];

  // 第一行：状态 · 消息 · 已持续。三样都是牌上已有的东西，这里只是摊开说全。
  const parts = [signalLabel(lang, group.aggregate)];
  if (group.message) parts.push(clip(group.message, MAX_MESSAGE));
  // `since_ms` 为 0 = 这一组没有决定聚合的会话（离线 / 占位），谈不上「持续多久」。
  if (group.since_ms > 0) parts.push(formatAge(group.since_ms, now));
  lines.push(parts.join(" · "));

  // 第二行：亮着的灯位 + 各自凭什么亮。判据全部复用真灯那两个纯函数。
  const edges = edgeLightKinds(settings.sign_lights, ctx);
  const corners = cornerLights(
    settings.sign_lights,
    {
      pinned: settings.group_meta[group.name]?.order != null,
      muted: settings.muted_groups.includes(group.name),
      staleProgress: group.stale_progress,
      passThrough,
    },
    ctx,
  );

  const lit: string[] = [];
  if (edges.top !== "off") {
    lit.push(`${m.slotTop}「${triggerText(settings.sign_lights.top.trigger, settings, lang, m)}」`);
  }
  if (edges.bottom !== "off") {
    lit.push(
      `${m.slotBottom}「${triggerText(settings.sign_lights.bottom.trigger, settings, lang, m)}」`,
    );
  }
  // 角灯那四格解释的是「这枚记号在说什么」（kind），不是触发 —— 触发决定它亮不亮，
  // 而用户看到的形状问的是「这是什么意思」。
  for (const corner of corners) {
    lit.push(`${m.cornerPos[corner.pos]}「${m.cornerKind[corner.kind]}」`);
  }

  if (lit.length > 0) {
    const shown = lit.slice(0, MAX_SLOTS).join(" · ");
    // 多出来的用 `+N`，和牌上「还有几组没显示」是同一个记号，不再发明新词。
    lines.push(lit.length > MAX_SLOTS ? `${shown} +${lit.length - MAX_SLOTS}` : shown);
  }

  return lines;
}
