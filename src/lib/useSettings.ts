import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";

/** 圆点形状。默认 circle 时 permission / error 会额外画成圆角方（色盲友好）。 */
export type Shape = "circle" | "rounded" | "ring";

/** 配色预设。给预设而不是任意取色 —— 红=危险、黄=注意是有语义的。 */
export type Palette = "default" | "colorblind" | "vivid";

/**
 * HTTP 转发的**服务预设**：决定 content-type 和 body 形状。
 *
 * Rust 侧是同名的枚举（`serde(rename_all = "lowercase")`），所以这里的字符串就是
 * 配置文件里那个值。`raw` 是默认 —— 原样发 SignalHub 自己的 JSON 快照。
 *
 * 除了 `raw`，其余每一个都只发**一行文字**：那些服务（ntfy / Bark / Slack /
 * Discord / 飞书 / 企业微信 / 钉钉 / Telegram）的形状本来就是「一个 URL + 一段
 * 文字」，服务端自己决定怎么排版。给它们发结构化数据，用户收到的是一坨看不懂的
 * JSON —— 而「转发看起来什么都没发生」正是 R4 要解决的问题。
 */
export type HttpPreset =
  | "raw"
  | "text"
  | "ntfy"
  | "bark"
  | "slack"
  | "discord"
  | "lark"
  | "wecom"
  | "dingtalk"
  | "telegram";

/**
 * 预设的选项顺序。**和 Rust 的 `HttpPreset` 枚举顺序一致** —— 两边列出来的顺序
 * 不一样时，用户按记忆去找「上次那个」就会找不到。
 */
export const HTTP_PRESETS: HttpPreset[] = [
  "raw",
  "text",
  "ntfy",
  "bark",
  "slack",
  "discord",
  "lark",
  "wecom",
  "dingtalk",
  "telegram",
];

/** 钩子能做的事。`command` 会执行本地命令，见 docs/api.md 里的安全说明。 */
export type HookAction =
  | {
      type: "http";
      url: string;
      /**
       * 额外请求头 —— 鉴权靠它（`Authorization: Bearer …`、ntfy 的 `X-Token`）。
       * Rust 侧已经规整过：去空白、丢空键、上限 16 条。
       */
      headers: Record<string, string>;
      /** 发什么形状，见 `HttpPreset`。 */
      preset: HttpPreset;
    }
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
  /**
   * 牌宽倍率。**1.0 = 原样**。只作用于展开态的牌 —— 折叠态是一排正方形点，
   * 长宽倍率对它没有意义。
   */
  sign_w_mult: number;
  /** 牌高倍率。**1.0 = 原样**（单行 / 带消息两行都乘）。 */
  sign_h_mult: number;
  /** 多灯布局：`single` 一盏灯画一叠牌（默认），`per-group` 每个可见组一盏灯。 */
  lamp_layout: "single" | "per-group";
  always_on_top: boolean;
  /** 拖完之后离屏幕左右边缘 12px 以内就吸附贴齐。默认关。 */
  snap_to_edge: boolean;
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
  /**
   * 上报没带 `group` 时怎么兜底分组。
   *
   * - `"none"` = 不分组（所有上报归 `default`）
   * - `"cwd"` = 按上报里的 `cwd` 分组，一个工作目录一块牌
   * - `"session"` = 按会话分组，**一个会话一块牌** —— 同一个智能体并行开几个
   *   窗口 / worktree 时，只有这样才能看出它们状态各不相同
   *
   * **改这个会立刻改变灯上的牌数**，所以默认是 `"none"`。
   */
  group_by: "none" | "cwd" | "session";
  /**
   * 把 HTTP 服务暴露到局域网，让别的机器 / 容器 / WSL 里的智能体也能上报。
   *
   * **默认关**。打开后网内任何人拿到 token 就能上报 —— 「拒绝带 Origin 的请求」
   * 那道防线挡的是网页偷偷打本机端口，对 curl 这类非浏览器客户端无效。
   * **改完要重启应用才生效**：监听地址只在启动时绑定一次。
   */
  lan_access: boolean;
}

/**
 * 与 Rust 侧 `config::Settings::default()` 一致。
 *
 * **导出给单测用**：`lampStyle` / `lampSort` 那些纯函数要一份设置当输入，而手抄一份
 * 默认值会随着真值演进悄悄漂移 —— 那正是这个仓库反复在防的那类 bug（见
 * `config.rs` 里「两个常量漂移」的注释）。测试里用 `{...DEFAULTS, ...覆盖}` 就够了。
 */
export const DEFAULTS: Settings = {
  size: 22,
  sign_w_mult: 1,
  sign_h_mult: 1,
  lamp_layout: "single",
  always_on_top: true,
  snap_to_edge: false,
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
  group_by: "none",
  lan_access: false,
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
