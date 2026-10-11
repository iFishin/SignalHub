import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";

/** 圆点形状。默认 circle 时 permission / error 会额外画成圆角方（色盲友好）。 */
export type Shape = "circle" | "rounded" | "ring";

/** 配色预设。给预设而不是任意取色 —— 红=危险、黄=注意是有语义的。 */
export type Palette = "default" | "colorblind" | "vivid";

/** 钩子能做的事。`command` 会执行本地命令，见 docs/api.md 里的安全说明。 */
export type HookAction =
  | { type: "http"; url: string }
  | { type: "command"; command: string };

/**
 * 一条出站钩子：聚合变成 `when` 里的某个状态时执行 `action`。
 *
 * `when` 为空 = 任何变化都触发。值是状态名，含 `stale` / `offline` 这类
 * 派生状态（用户可能只想知道「会话沉默了」）。
 *
 * Rust 侧用 `#[serde(flatten)]`，所以 `type` 是和 `when` 平级的字段，
 * 而不是嵌在 `action` 对象里。
 */
export type Hook = { when: string[] } & HookAction;

/** 展开的牌上显示哪几段内容。四段都可以单独关掉 —— 有人只要一眼颜色，
 *  有人要看清「卡在哪一步」。 */
export interface SignFields {
  /** 组名 */
  name: boolean;
  /** 状态名（如「需授权」） */
  state: boolean;
  /** 已持续多久 */
  age: boolean;
  /** 消息 —— 关掉之后牌永远是单行高（窗口高度也跟着少一行） */
  message: boolean;
}

/** 与 Rust 侧 `config::Settings` 对应（JSON 字段名保持 snake_case）。 */
export interface Settings {
  size: number;
  always_on_top: boolean;
  /** 安静模式：空闲 / 沉默 / 离线时圆点几乎透明，悬停恢复。 */
  quiet: boolean;
  /** 需要你注意时弹系统通知。 */
  notify: boolean;
  /** 出站钩子。 */
  hooks: Hook[];
  shape: Shape;
  palette: Palette;
  /** 逐个状态覆盖颜色（`#rrggbb`）。空的表示全部用配色预设。 */
  colors: Record<string, string>;
  /** 折叠成一颗单纯的灯（只剩灯，没有牌面）。**默认 false = 展开成牌。** */
  collapsed: boolean;
  /** 展开的牌上显示哪几段内容 */
  show: SignFields;
  /** 牌面背景的不透明度（0–100）。调低能看见桌面。 */
  sign_opacity: number;
  /** 闪烁节奏倍率（50–200，100 = 原始速度）。数字越大闪得越快。 */
  blink_speed: number;
  /** 需要你注意时，系统通知是否带提示音。 */
  sound: boolean;
  /**
   * 被右键「隐藏」掉的组。
   *
   * **只影响灯上显示哪几块牌**，不影响聚合、通知和钩子 —— 聚合是全局的，
   * 把一块牌藏起来不等于当这一组不存在。
   */
  hidden_groups: string[];
}

/** 与 Rust 侧 `config::Settings::default()` 一致。 */
const DEFAULTS: Settings = {
  size: 22,
  always_on_top: true,
  quiet: true,
  notify: true,
  hooks: [],
  shape: "circle",
  palette: "default",
  colors: {},
  collapsed: false,
  show: { name: true, state: true, age: true, message: true },
  sign_opacity: 86,
  blink_speed: 100,
  sound: true,
  hidden_groups: [],
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
