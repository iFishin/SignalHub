import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";

/** 圆点形状。默认 circle 时 permission / error 会额外画成圆角方（色盲友好）。 */
export type Shape = "circle" | "rounded" | "ring";

/** 配色预设。给预设而不是任意取色 —— 红=危险、黄=注意是有语义的。 */
export type Palette = "default" | "colorblind" | "vivid";

/** 与 Rust 侧 `config::Settings` 对应（JSON 字段名保持 snake_case）。 */
export interface Settings {
  size: number;
  always_on_top: boolean;
  /** 安静模式：空闲 / 沉默 / 离线时圆点几乎透明，悬停恢复。 */
  quiet: boolean;
  /** 需要你注意时弹系统通知。 */
  notify: boolean;
  /** 出站转发目标；null 表示不转发。 */
  webhook_url: string | null;
  shape: Shape;
  /** 给圆点套一圈中性细环，让它读起来像指示灯而不是 UI 色块。 */
  bezel: boolean;
  palette: Palette;
}

/** 与 Rust 侧 `config::Settings::default()` 一致。 */
const DEFAULTS: Settings = {
  size: 22,
  always_on_top: true,
  quiet: true,
  notify: true,
  webhook_url: null,
  shape: "circle",
  bezel: true,
  palette: "default",
};

const SETTINGS_EVENT = "signalhub://settings";

export function useSettings(): Settings {
  const [settings, setSettings] = useState<Settings>(DEFAULTS);

  useEffect(() => {
    if (!inTauri) return;

    // 先订阅再拉取：反过来的话，两步之间发生的变更会被永久丢掉。
    const pending = listen<Settings>(SETTINGS_EVENT, (event) =>
      setSettings(event.payload),
    );
    invoke<Settings>("get_settings")
      .then(setSettings)
      .catch((err) => console.error("get_settings 失败", err));

    return () => {
      pending.then((unlisten) => unlisten());
    };
  }, []);

  return settings;
}

export function updateSettings(next: Settings): Promise<Settings> {
  return invoke<Settings>("set_settings", { settings: next });
}
