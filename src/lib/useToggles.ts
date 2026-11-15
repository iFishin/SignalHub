import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";

/**
 * 由插件（而非 Settings）持有的几个开关。
 *
 * 开机自启的真值在 autostart 插件那边（它管着 LaunchAgent plist），
 * 穿透与展示模式是瞬态状态，快捷键文案由 Rust 定义一次 —— 都不适合塞进
 * Settings。`stage_since_ms` 是展示模式开启的时刻（0 = 没开），由 Rust 记在
 * 内存里，重启即清零：大屏拿它当「离开这段时间」的起点，不算一条新数据通道。
 */
export interface Toggles {
  autostart: boolean;
  pass_through: boolean;
  pass_through_shortcut: string;
  stage: boolean;
  stage_shortcut: string;
  stage_since_ms: number;
}

const TOGGLES_EVENT = "signalhub://toggles";

const DEFAULTS: Toggles = {
  autostart: false,
  pass_through: false,
  pass_through_shortcut: "",
  stage: false,
  stage_shortcut: "",
  stage_since_ms: 0,
};

export function useToggles(): Toggles {
  const [toggles, setToggles] = useState<Toggles>(DEFAULTS);

  useEffect(() => {
    if (!inTauri) return;

    // 必须订阅：穿透和自启也能从托盘菜单与全局快捷键改，
    // 不订阅的话设置面板里的开关会一直显示过时的状态
    const pending = listen<Toggles>(TOGGLES_EVENT, (event) =>
      setToggles(event.payload),
    );
    invoke<Toggles>("get_toggles")
      .then(setToggles)
      .catch((err) => console.error("get_toggles 失败", err));

    return () => {
      pending.then((unlisten) => unlisten());
    };
  }, []);

  return toggles;
}

export function setAutostart(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_autostart", { enabled });
}

export function setPassThrough(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_pass_through", { enabled });
}

/** 进入 / 退出展示模式。四条入口（系统页 / 演示页 / 托盘 / 快捷键）都汇到 Rust 的同一处。 */
export function setStage(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_stage", { enabled });
}
