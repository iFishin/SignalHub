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
 * 一条出站钩子：状态变化时执行 `action`。
 *
 * `when` 为空 = 任何状态变化都触发。值是状态名，含 `stale` / `offline` 这类
 * 派生状态（用户可能只想知道「会话沉默了」）。
 *
 * Rust 侧用 `#[serde(flatten)]`，所以 `type` 是和 `when` 平级的字段，
 * 而不是嵌在 `action` 对象里。
 */
export type Hook = { when: string[]; groups: string[] } & HookAction;

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

/**
 * 某一个状态的个性化外观。三样都可以单独覆盖，**没写的跟随全局**。
 *
 * 用「可选的字段」而不是填满的表格：默认值会随版本演进，存一份填满的表会让
 * 那些改进永远到不了用户那儿。`null` 和缺字段都读作「跟随」。
 */
export interface StateStyle {
  /** 颜色 `#rrggbb`。没写 = 用配色方案里那个状态的默认色 */
  color?: string | null;
  /** 灯珠形状。没写 = 用全局形状（全局是「圆」时 permission / error 仍是圆角方） */
  shape?: Shape | null;
  /** 闪烁周期（毫秒）。**0 = 常亮**。没写 = 按全局节奏倍率算默认周期 */
  blink_ms?: number | null;
}

/**
 * 某一组的显示偏好。
 *
 * **只影响灯上怎么显示**，不影响聚合、通知和钩子 —— 组名是上报方给的标识
 * （`build-1`、`ci`），这里改的是「怎么称呼它、排在第几位」。隐藏 / 静音的匹配
 * 也仍然按**原始组名**，不按显示名。
 */
export interface GroupMeta {
  /** 显示名。没写就用上报方给的原始组名。 */
  name?: string | null;
  /**
   * 置顶权重。`number` = 置顶，越小越靠前；`null`/缺省 = 不置顶，按紧急程度排。
   *
   * 用权重而不是一个布尔：置顶的组之间也要有确定的顺序，而「先置顶的排前面」
   * 正是用户按下开关的顺序。
   */
  order?: number | null;
}

/**
 * 勿扰时段。这段时间里不弹通知、不出声，但灯照常显示。
 *
 * 起止都按**本地时间**，只有小时没有分钟。本地时区只有前端知道，所以由前端把
 * 当前小时推给 Rust，**由 Rust 判断在不在窗口内** —— 跨午夜那种边界逻辑只有
 * 一处实现才敢信。
 */
export interface QuietHours {
  enabled: boolean;
  /** 起始小时（0–23） */
  start: number;
  /** 结束小时（0–23），**不含**这一小时 */
  end: number;
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
  /** 逐个状态的外观覆盖（颜色 / 形状 / 节奏）。没写的项跟随全局。 */
  styles: Record<string, StateStyle>;
  /** 折叠成一颗单纯的灯（只剩灯，没有牌面）。**默认 false = 展开成牌。** */
  collapsed: boolean;
  /** 展开的牌上显示哪几段内容 */
  show: SignFields;
  /** 牌面背景的不透明度（0–100）。调低能看见桌面。 */
  sign_opacity: number;
  /**
   * 被右键「隐藏」掉的组。**只影响灯上显示哪几块牌。**
   *
   * 与 `muted_groups` 是两件事：那个管「要不要吵我」，这个管「灯上显不显示」。
   */
  hidden_groups: string[];
  /**
   * 被静音的组。**只影响通知和提示音** —— 灯上照常显示，聚合和钩子也不受影响。
   */
  muted_groups: string[];
  /** 闪烁节奏倍率（50–200，100 = 原始速度）。数字越大闪得越快。对没有单独设节奏的状态生效。 */
  blink_speed: number;
  /** 需要你注意时，系统通知是否带提示音。 */
  sound: boolean;
  /** 勿扰时段：这段时间里不弹通知、不出声。 */
  quiet_hours: QuietHours;
  /** 「需要你注意」的状态要**持续**这么多秒才弹通知。0 = 立刻弹。 */
  alert_after_secs: number;
  /** 托盘图标是否跟随当前聚合状态变色。 */
  tray_follows_state: boolean;
  /** 界面语言。"auto" = 跟随系统。 */
  locale: "auto" | "zh" | "en";
  /** 逐组的显示偏好（显示名 / 置顶）。键是**上报方给的原始组名**。 */
  group_meta: Record<string, GroupMeta>;
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
  styles: {},
  collapsed: false,
  show: { name: true, state: true, age: true, message: true },
  sign_opacity: 86,
  hidden_groups: [],
  muted_groups: [],
  blink_speed: 100,
  sound: true,
  quiet_hours: { enabled: false, start: 22, end: 8 },
  alert_after_secs: 5,
  tray_follows_state: true,
  locale: "auto",
  group_meta: {},
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
