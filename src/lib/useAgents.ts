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
  /**
   * 配置里**实际写着**的那条命令。
   *
   * 注意不是「我们现在会写的那条」—— 两者不同正是问题所在：命令是某次装进去的，
   * 之后 app 被挪走，配置里就留下一条跑不起来的命令。`null` = 没装或读不出来。
   */
  command: string | null;
  /**
   * 那条命令的程序现在还解析得到吗。
   *
   * **`installed && !command_ok` = 一次静默失效的接入**：智能体不报错（它只是执行了
   * 一个不存在的程序），灯也不亮。这是最该主动指出来的一种「装了却没反应」。
   */
  command_ok: boolean;
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
  /**
   * **插件式**接入写下的那个文件（opencode 这类）。
   *
   * 非 null 时界面改口说「已写入 <路径>」，而不是「新增 N 个事件」—— 插件式没有
   * 事件清单这回事（它在生成的源码里自己映射事件），说事件数对不上任何东西。
   */
  plugin: string | null;
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

/**
 * 最近的 hook 调用记录，**最新的在后**。
 *
 * 只在挂载和手动 `refresh()` 时取：这个文件回答的是「我刚才那次接入到底有没有
 * 生效」，是「点了按钮想看看」的场景 —— 定时轮询既没必要（它不由状态快照驱动），
 * 也会让这个面板一直打 IPC。文件里每行自带时间，所以不刷新也不会误导。
 */
export function useHookLog(limit = 40) {
  const [lines, setLines] = useState<string[]>([]);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    invoke<string[]>("get_hook_log", { limit })
      .then(setLines)
      .catch((err) => console.error("get_hook_log 失败", err));
  }, [limit]);

  useEffect(refresh, [refresh]);

  return { lines, refresh };
}
