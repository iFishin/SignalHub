import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";

/**
 * 一条等着你点的审批（从灯上审批，见 `approvals.rs`）。
 *
 * `detail` 是「这次调用到底要干什么」（命令全文 / 文件路径 / 输入 JSON），
 * **只进这个本地界面**：状态上报用的是 `target` 那个短词，通知和出站钩子都不带它。
 * 命令里可能有 `API_KEY=…`，而那是「只发状态」那条红线的范围。
 */
export interface Approval {
  id: string;
  session: string;
  tool: string;
  /** 牌上那行短词（`执行 npm`），和灯上的「正在做什么」是同一句话。 */
  target: string;
  detail: string;
  /** `detail` 被截断过。 */
  truncated: boolean;
  /**
   * 能不能在灯上放行。**看不全就不给放行**（长命令只能拒绝、或者回终端自己看）——
   * 这条规则在 Rust 的登记表里挡着，这里只是照它画不画那个按钮。
   */
  allowable: boolean;
  age_ms: number;
}

/** 事件名必须和 Rust 的 `APPROVALS_EVENT` 一致。 */
const APPROVALS_EVENT = "signalhub://approvals";

/**
 * 待审批列表 + 一个「替用户点一下」的动作。
 *
 * 为什么**不能**把这个列表塞进状态快照里（那样就不用这套订阅了）：快照是
 * `/v1/state` 的返回，也就是只读 Web 面板在手机上看到的东西 —— 而 `detail` 是
 * 命令全文。它只该出现在本机这个窗口里。
 *
 * 变化由 Rust 主动推（`Registry::on_change`），不轮询：这是**有时限**的东西
 * （最多 45 秒），一秒的延迟就意味着少一秒的思考时间。
 */
export function useApprovals(): {
  approvals: Approval[];
  decide: (id: string, decision: "allow" | "deny") => void;
} {
  const [approvals, setApprovals] = useState<Approval[]>([]);

  const refresh = useCallback(() => {
    if (!inTauri) return;
    invoke<Approval[]>("get_approvals")
      .then(setApprovals)
      .catch((err) => console.error("get_approvals 失败", err));
  }, []);

  useEffect(() => {
    if (!inTauri) return;
    // 挂载时取一次：窗口比登记表活得久的情况是存在的（灯窗重建、切换多灯布局），
    // 只靠事件的话那一刻会漏掉已经挂着的那一条。
    refresh();
    const unlisten = listen(APPROVALS_EVENT, refresh);
    return () => {
      unlisten.then((off) => off());
    };
  }, [refresh]);

  const decide = useCallback(
    (id: string, decision: "allow" | "deny") => {
      invoke("decide_approval", { id, decision })
        // 失败**不改本地列表**：超时那条 Rust 会推一次变化，界面跟着就没了；
        // 而「看不全不能放行」那种拒绝是立刻返回的，列表本来就该还在。
        .catch((err) => console.error("decide_approval 失败", err));
    },
    [],
  );

  return { approvals, decide };
}
