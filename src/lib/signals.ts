import type { SignalState } from "./types";

/** 状态的中文名。灯球和设置面板共用，避免两处措辞不一致。 */
export const SIGNAL_LABEL: Record<SignalState, string> = {
  // 三档「不在工作」要分清：完全没有智能体 / 智能体不说话了 / 用户主动暂停
  offline: "没有智能体在上报",
  stale: "会话已沉默",
  off: "已暂停监控",

  idle: "空闲",
  done: "已完成",
  thinking: "推理中",
  working: "执行中",
  attention: "需要关注",
  permission: "需要授权",
  error: "出错 / 阻塞",
};
