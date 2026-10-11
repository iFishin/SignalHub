import { MESSAGES, type Lang } from "./i18n";
import type { SignalState } from "./types";

/** 状态名。灯球和设置面板共用，避免两处措辞不一致。 */
export function signalLabel(lang: Lang, state: SignalState): string {
  return MESSAGES[lang].signal[state];
}
