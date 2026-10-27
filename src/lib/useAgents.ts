import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./env";
import { useVisibleRefresh } from "./useVisibleRefresh";

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
  /**
   * 装过，但配置里那条命令是**旧版本**写的（和当前版本会写的那条不同）。
   *
   * **`installed && stale` = 升级后没重装**：功能还在，行为停在旧版本 —— 典型是
   * Claude Code 的 `SIGNALHUB_PID="$PPID"` 前缀（0.3.2 起才有）。和 `command_ok`
   * （程序还跑不跑得起来）是两个不同的问题：那条答「还跑得动吗」，这条答
   * 「跑的是不是这一版的命令」。插件式接入不参与（源码版本要另立判据）。
   */
  stale: boolean;
  /**
   * 装进去的是 HTTP 式（`true`）还是命令式（`false`）；`null` = 没装。
   *
   * 两种模式写进同一个配置，界面要说得出「你在跑哪条路」—— 否则用户看到「已接入」
   * 也不知道自己改不改 PATH 有没有关系。HTTP 式没有命令可解析，所以那时
   * `command_ok` 无意义。
   */
  http: boolean | null;
}

/** 与 Rust 侧 receipts.rs 的 ReceiptView 对应。 */
export interface Receipt {
  /** CLI 里用的信号源名字（`claude-code` / `opencode` / 你自己的脚本名） */
  agent: string;
  /** 最后一次上报的墙上时刻（Unix 毫秒）。`0` = 从来没收到过。 */
  last_epoch_ms: number;
  /** 最后一次上报的状态名（`working` / `permission` …），没报过是空串 */
  state: string;
  /**
   * 最后一次上报**声明**的信号源类型：`agent` / `system` / `custom`。
   *
   * 空串 = 没声明（老版本 CLI / 手写 curl 不带这个字段）。只有 `system` 会被单独
   * 分出一组，其余都按「不是我们认识的智能体」落到自定义那一组 —— 见
   * `groupReceipts`。
   */
  source: string;
  /** 本次运行以来收到多少条 */
  count: number;
}

/**
 * 每个信号源「最近一次上报」的凭证。
 *
 * **它回答的是接入页以前回答不了的那个问题**：接入到底有没有在报。以前只能说
 * 「配置文件里写着我们的命令」——而装了却没反应的接入看起来一模一样。
 *
 * 只在挂载和手动 `refresh()` 时取：这份数据不由状态快照驱动，定时轮询只是白打
 * IPC。相对时间由调用方用 `formatAge(last_epoch_ms, now)` 算（见 `useNow`）。
 *
 * 同理，设置窗重新显形时也要补拉一次 —— 否则「有没有在报」会一直停在应用启动
 * 那一刻的答案（见 `useVisibleRefresh`）。
 */
export function useReceipts() {
  const [receipts, setReceipts] = useState<Receipt[]>([]);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    invoke<Receipt[]>("agent_receipts")
      .then(setReceipts)
      .catch((err) => console.error("agent_receipts 失败", err));
  }, []);

  useEffect(refresh, [refresh]);
  useVisibleRefresh(refresh);

  return { receipts, refresh };
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
 *
 * 设置窗重新显形时补拉一次（`useVisibleRefresh`）：用户可能在别处装了东西，
 * 也可能只是想让「最近一次上报」跟上。
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
  useVisibleRefresh(refresh);

  return { agents, refresh };
}

/**
 * 最近的 hook 调用记录，**最新的在后**。
 *
 * 只在挂载和手动 `refresh()` 时取：这个文件回答的是「我刚才那次接入到底有没有
 * 生效」，是「点了按钮想看看」的场景 —— 定时轮询既没必要（它不由状态快照驱动），
 * 也会让这个面板一直打 IPC。文件里每行自带时间，所以不刷新也不会误导。
 *
 * **窗口重新显形时补拉一次**：设置窗一直开着、启动那一刻的尾巴会一直留着，
 * 用户第二天打开还看到昨晚的日志（见 `useVisibleRefresh`）。
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
  useVisibleRefresh(refresh);

  return { lines, refresh };
}

/**
 * 把「收到过上报的信号源」分成界面上的两组。
 *
 * 上面那张表是**我们认识的智能体**（`detect_agents` 的 8 行，每行带接入按钮），
 * 所以这里只处理剩下的：
 *
 *   - **系统** = 上报方自己声明了 `source: "system"`（将来的内置采集器，以及用户
 *     自己盯系统状态的守护脚本）。
 *   - **自定义** = 其余全部（用户自己的脚本、另一台机器、手写的一条 curl、还没
 *     一键接入的智能体）。
 *
 * **「是不是智能体」由调用方判定**（`knownIds`）：那份清单在 `agents` 里，这一层
 * 不该再抄一份 8 个名字 —— 抄了就会在加第 9 家时漏掉一处。
 */
export function groupReceipts(
  receipts: Receipt[],
  knownIds: Set<string>,
): { system: Receipt[]; custom: Receipt[] } {
  const others = receipts.filter((r) => !knownIds.has(r.agent));
  return {
    system: others.filter((r) => r.source === "system"),
    custom: others.filter((r) => r.source !== "system"),
  };
}
