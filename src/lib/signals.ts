import type { SignalState } from "./types";

/** 状态的中文名。灯球和设置面板共用，避免两处措辞不一致。 */
export const SIGNAL_LABEL: Record<SignalState, string> = {
  off: "已暂停监控",
  idle: "空闲",
  done: "已完成",
  thinking: "推理中",
  working: "执行中",
  attention: "需要关注",
  permission: "需要授权",
  error: "出错 / 阻塞",
};
