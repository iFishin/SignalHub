import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";
import type { Progress } from "./updateProgress";

/** 与 Rust 侧 `update::UPDATE_EVENT` / `PROGRESS_EVENT` 必须一致。 */
const UPDATE_EVENT = "signalhub://update";
const PROGRESS_EVENT = "signalhub://update-progress";

export type Available = { version: string; notes: string };

/** 与 Rust 侧 `update::UpdateState` 一一对应。 */
export type UpdateState = {
  /** 这个平台能不能自动更新。false 时界面只说「请手动下载」，不给按钮 */
  supported: boolean;
  current: string;
  available: Available | null;
  checked_at_ms: number;
  /** 上次**手动**检查的失败原因（自动那次的失败不写进来，见 update.rs） */
  error: string | null;
};

const INITIAL: UpdateState = {
  supported: false,
  current: "",
  available: null,
  checked_at_ms: 0,
  error: null,
};

export type UpdateApi = {
  state: UpdateState;
  /** 正在检查 */
  checking: boolean;
  /** 正在下载 / 安装 */
  installing: boolean;
  progress: Progress | null;
  /** 安装这条命令返回的错误（检查失败在 `state.error` 里，不在这儿） */
  failure: string | null;
  check: () => void;
  install: () => void;
};

/**
 * 自动更新。**目前只在 macOS 上 `supported`**（原因见 Rust 侧 `update.rs` 顶部）。
 *
 * 这里不自己轮询：启动后的每日检查是 Rust 侧的后台任务，查完会推 `UPDATE_EVENT`。
 * 前端只负责（1）订阅那个事件、（2）打开面板时读一次快照、（3）用户点了按钮
 * 之后调命令。检查的逻辑只有 Rust 那一份。
 *
 * 顺带一提「为什么先订阅再 `get_update_state`」：反过来的话，两步之间发生的那次
 * 检查结果会被快照覆盖回旧值。和 `useSignal` 同一条理由。
 */
export function useUpdate(): UpdateApi {
  const [state, setState] = useState<UpdateState>(INITIAL);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;

    const stateEvents = listen<UpdateState>(UPDATE_EVENT, (event) =>
      setState(event.payload),
    );
    const progressEvents = listen<Progress>(PROGRESS_EVENT, (event) =>
      setProgress(event.payload),
    );
    invoke<UpdateState>("get_update_state")
      .then(setState)
      .catch((err) => console.error("get_update_state 失败", err));

    return () => {
      stateEvents.then((off) => off());
      progressEvents.then((off) => off());
    };
  }, []);

  const check = useCallback(() => {
    setChecking(true);
    setFailure(null);
    invoke<UpdateState>("check_update")
      // 检查失败不走 catch：Rust 侧把原因放在返回值的 `error` 里（那是**状态**，
      // 不是异常），这样「检查失败」也能和「有没有新版本」一起原子地更新。
      .then(setState)
      .catch((err) => setFailure(String(err)))
      .finally(() => setChecking(false));
  }, []);

  const install = useCallback(() => {
    setInstalling(true);
    setFailure(null);
    setProgress(null);
    invoke("install_update").catch((err) => {
      // 成功的话不会走到这里 —— Rust 侧装完就重启进程了
      setFailure(String(err));
      setInstalling(false);
      setProgress(null);
    });
  }, []);

  return { state, checking, installing, progress, failure, check, install };
}
