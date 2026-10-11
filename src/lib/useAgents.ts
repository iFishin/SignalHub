import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";

/** 与 Rust 侧 lib.rs 的 AgentStatus 对应。 */
export interface AgentStatus {
  /** CLI 里用的名字（`claude-code`），接入 / 卸载命令拿它当参数 */
  id: string;
  /** 显示名，已经是英文品牌名 */
  name: string;
  /** 这台机器上有没有它（看配置目录和 PATH） */
  detected: boolean;
  /** 我们有没有给它装过 hook */
  installed: boolean;
  /** 配置文件路径，界面上显示用 */
  config: string;
}

/** 与 Rust 侧 lib.rs 的 HookChange 对应。 */
export interface HookChange {
  /** 本次新写入的事件名 */
  added: string[];
  /** 已装过但命令变了、原地更新的事件名 */
  updated: string[];
  /** 已装过且命令相同，跳过的事件名 */
  already: string[];
  /** 卸载时摘掉了几个 handler */
  removed_handlers: number;
  config: string;
  /** 备份文件路径，没有备份就是 null */
  backup: string | null;
}

/**
 * 机器上有哪些智能体、我们接入了哪些。
 *
 * 只在挂载时取一次，接入 / 卸载后由调用方手动 `refresh()` —— 这份列表不像状态
 * 快照那样随时在变，定时轮询只是白费调用。检测结果也不决定能不能装：没检测到的
 * 条目照样显示、照样装得动（见 Rust 侧 detect_agents 的说明）。
 */
export function useAgents() {
  const [agents, setAgents] = useState<AgentStatus[]>([]);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    invoke<AgentStatus[]>("detect_agents")
      .then(setAgents)
      .catch((err) => console.error("detect_agents 失败", err));
  }, []);

  useEffect(refresh, [refresh]);

  return { agents, refresh };
}
