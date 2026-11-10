import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./env";

/** 圆点形状。默认 circle 时 permission / error 会额外画成圆角方（色盲友好）。 */
export type Shape = "circle" | "rounded" | "ring";

/** 配色预设。给预设而不是任意取色 —— 红=危险、黄=注意是有语义的。 */
export type Palette = "default" | "colorblind" | "vivid";

/**
 * 牌面质感五档。取值与 Rust `config.rs` 的同名枚举一一对应（`lowercase`），
 * 也和外层 `lamp.css` 的 `[data-sign-*]` 规则一一对应 —— **三处必须同名**，
 * 改名要一起改，否则表现是「设置点了没反应」，界面上看不出任何线索。
 *
 * 为什么是枚举而不是滑杆：这五样各自只有几种「本来就好看」的取值（圆角
 * 0.18 和 0.22 的差别没人看得出来），连续滑杆只会让人反复微调到怀疑自己。
 */
export type SignRadius = "square" | "soft" | "standard" | "round";
export type SignBorder = "none" | "hairline" | "strong";
export type SignTint = "graphite" | "black" | "warm" | "slate";
export type SignText = "compact" | "standard" | "large";
/** `auto` = 需要你动手的那几块牌更实、其余退后一档；`flat` = 所有牌同等分量。 */
export type SignEmphasis = "auto" | "flat";

/**
 * HTTP 转发的**服务预设**：决定 content-type 和 body 形状。
 *
 * Rust 侧是同名的枚举（`serde(rename_all = "lowercase")`），所以这里的字符串就是
 * 配置文件里那个值。`raw` 是默认 —— 原样发 SignalHub 自己的 JSON 快照。
 *
 * 除了 `raw`，其余每一个都只发**一行文字**：那些服务（ntfy / Bark / Slack /
 * Discord / 飞书 / 企业微信 / 钉钉 / Telegram / PushDeer）的形状本来就是「一个 URL
 * + 一段文字」，服务端自己决定怎么排版。给它们发结构化数据，用户收到的是一坨看不懂的
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
  | "telegram"
  | "pushdeer"
  | "custom";

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
  "pushdeer",
  "custom",
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
      /**
       * 自定义请求体模板（只对 `preset: "custom"` 有意义）。
       * 占位符见 Rust 的 `TEMPLATE_PLACEHOLDERS`。
       */
      body?: string | null;
      /** 自定义 content-type（只对 `custom` 有意义）。空 = `application/json`。 */
      content_type?: string | null;
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

/**
 * 一台物理灯（目前只支持 WLED）。
 *
 * **只有地址，没有开关**：填了地址就发、空着就不发，和钩子一个脾气。多一个开关
 * 只会多出一种「配置看起来在、实际不发」的哑谜 —— 要停掉一盏灯就把它删了。
 *
 * 发出去的东西只有颜色（没有组名、会话名、消息内容），见 `devices.rs`。
 */
export interface Device {
  /**
   * 灯的地址。**只填主机**（含端口）：`192.168.1.50`、`wled.local:8080`、
   * `http://192.168.1.50`。带不带 `http://` 都认，路径由 Rust 侧拼
   * （`/json/state`）。
   */
  host: string;
}

/**
 * 另一台机器（多机中枢）。
 *
 * 方向和其他所有配置项都相反：钩子 / 物理灯是**我们往外发**，这个是**我们往外读**
 * —— 每 `machine_poll_ms` 去对端的 `GET /v1/state` 取一次快照，把它的会话按
 * `m:{id}:{会话}` 的名字喂进本机同一个 Store。于是灯、收件箱、统计、物理灯全部
 * 自动看到多机内容。
 *
 * 对端**不用升级**：`GET /v1/state` 从 0.1 就有，只要它开了「允许局域网访问」，
 * 把 token 抄过来就能用。
 */
export interface Machine {
  /**
   * 显示名，同时也是**命名空间**（远程会话的 id 和组名都挂在它下面）。
   *
   * 所以它必须非空、唯一，而且**改了等于换了一台机器** —— 旧会话会以新名字重新
   * 进来，旧的那些在 TTL 到期后自然消失。
   */
  id: string;
  /**
   * 对端地址。`192.168.1.20:41320` / `http://host:port` 都认；顺手把
   * `/v1/state` 粘进来也认。空着 = 「刚添加还没填」，不会发请求。
   */
  url: string;
  /** 对端的 token（对端 `~/.signalhub/token` 的内容）。 */
  token: string;
  /** 关掉但留着配置。 */
  enabled: boolean;
}

/**
 * 一台机器的健康状态（`list_machines` / `probe_machine` 的返回）。
 *
 * **只读**：轮询线程是唯一写入方。界面上显示它，但改不了它 —— 那是事实，不是设置。
 */
export interface MachineHealth {
  id: string;
  ok: boolean;
  /** 最近一次成功的时间（epoch 毫秒，0 = 从来没成功过）。 */
  last_ok_ms: number;
  /** 最近一次成功拉取的耗时（毫秒）。 */
  latency_ms: number;
  /** 最近一次失败的原因。成功时为 null。 */
  error: string | null;
  /** 这次拉回来几个会话。 */
  sessions: number;
}

/**
 * Call Me —— 用户自部署的收信服务器（见 `docs/callme.md`）。
 *
 * 和 `Machine` 同一方向（**我们往外读**），但拉回来的东西不进 `Store`：
 * 留言不是状态信号，它有自己的页面（设置面板「Call Me」）。
 *
 * 默认全空 = 关闭，轮询线程一个请求都不发。
 */
export interface CallMeConfig {
  /** 服务器地址，例如 `https://callme.example.com`。空 = 关闭。 */
  url: string;
  /** 服务器上的 `CALLME_ADMIN_TOKEN`。 */
  token: string;
  /** 轮询间隔（毫秒，5000–300000）。留言不是实时信号，默认 15 秒。 */
  poll_ms: number;
}

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
  /**
   * 停留时长（毫秒）：这个状态**静默多久算过期**。没写 = 用内置默认
   * （`working` 5 分钟、`idle` 10 分钟…，由后端 `state_ttl_defaults` 提供）。
   *
   * 它不是外观：它决定「智能体不出声之后，这块牌还在灯上待多久」。**只在下次上报时
   * 生效**，不追溯已经亮着的牌（那边 `expires_at` 是上报那一刻写死的）。
   */
  ttl_ms?: number | null;
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
  /**
   * 哪些状态变化时弹系统通知 —— `notify` 总开关之下的一层过滤。
   *
   * **空数组 = 一条都不弹**，和 `hooks[].when` 的「空 = 不过滤」刻意不同：那里
   * 的空当「不过滤」讲是对的，这里的空要是照搬，等于给每个状态都弹通知。
   *
   * 名字取自 `NOTIFY_STATES`（`offline` / `off` 不在里面，见那边的注释）；
   * Rust 侧 `config::NOTIFY_STATES` 是同一份集合。
   */
  notify_when: string[];
  /** 出站钩子。 */
  hooks: Hook[];
  /**
   * 物理灯（WLED）。聚合变化时把**状态色**发给它们。
   *
   * 默认空 = 一台都不发（新增外发默认关）。
   */
  devices: Device[];
  /**
   * **从灯上审批工具调用**（默认关）。
   *
   * 全项目唯一会改变智能体行为的开关：打开之后 Claude Code 每次要问你批准某个
   * 工具调用时，确认框会被搬到灯上。开关本身由 Rust 的 `set_tool_approval` 改
   * （它同时改配置里的 hook），这里只是读出来画那个开关。
   */
  approve_tools: boolean;
  /**
   * **Time Machine 备份中**→「备份」组（B2 的内置采集器，默认关）。
   *
   * 开关由 Rust 的 `set_collector` 改；采集线程在 Rust 那边一直跑，每一跳自己
   * 看这个开关，所以前端只有读它这一件事。
   */
  collect_backup: boolean;
  /**
   * **会议中**（麦克风 / 摄像头被占用）→「会议」组（B2 的内置采集器，默认关）。
   *
   * 只有 macOS 有这个采集器；开关同样由 `set_collector` 改。
   */
  collect_meeting: boolean;
  shape: Shape;
  palette: Palette;
  /**
   * 牌面质感：圆角 / 描边 / 底色 / 字号 / 分层。
   *
   * **只改 CSS，不改窗口几何** —— 牌宽高仍由后端按 size 和倍率算，所以这
   * 五项随便调都不会让窗口裁掉最后一块牌。
   */
  sign_radius: SignRadius;
  sign_border: SignBorder;
  sign_tint: SignTint;
  sign_text: SignText;
  sign_emphasis: SignEmphasis;
  /** 逐个状态的外观覆盖（颜色 / 形状 / 节奏）。没写的项跟随全局。 */
  styles: Record<string, StateStyle>;
  /** 折叠成一颗单纯的灯（只剩灯，没有牌面）。**默认 false = 展开成牌。** */
  collapsed: boolean;
  /**
   * 灯上单击的主操作。`collapse` = 折叠 / 展开（默认），`inbox` = 打开收件箱，
   * `none` = 什么都不做。长按圆环菜单与拖拽不受影响。
   */
  click_action: "collapse" | "inbox" | "none";
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
  /**
   * **灵动效果**（默认开）：灯珠外的柔光呼吸、跃迁涟漪、牌面内高光与弹簧转场。
   *
   * 纯前端观感，不进任何状态判断、也不影响 Rust 算窗口尺寸。关掉就逐像素回到
   * 0.4.4 的样子 —— 常驻动画仍然只有 opacity，适合「只想要一颗朴素灯」的人。
   */
  lively: boolean;
  /** 需要你注意时，系统通知是否带提示音。 */
  sound: boolean;
  /** 勿扰时段：这段时间里不弹通知、不出声。 */
  quiet_hours: QuietHours;
  /** 「需要你注意」的状态要**持续**这么多秒才弹通知。0 = 立刻弹。 */
  alert_after_secs: number;
  /**
   * **预设 1**：授权卡住超过 60 秒时，用「转发」里的钩子再推一次手机
   * （Bark / PushDeer / ntfy……）。默认关，没有钩子就没有东西可发。
   */
  rule_stuck_push: boolean;
  /**
   * **预设 2**：有智能体在「思考 / 执行」时暂停通知和重复提醒。
   *
   * 只管「打扰」这一层：灯和上报照常，忙完也不补发（和勿扰时段同一个语义）。
   */
  rule_busy_silence: boolean;
  /**
   * **预设 3**：会议组存在时把它画成蓝色并暂停通知。需要「会议中」采集器。
   */
  rule_meeting: boolean;
  /** 托盘图标是否跟随当前聚合状态变色。 */
  tray_follows_state: boolean;
  /** 界面语言。"auto" = 跟随系统。 */
  locale: "auto" | "zh" | "en";
  /**
   * 设置窗口的亮 / 深色偏好。"auto" = 跟随系统外观。
   *
   * 只作用设置窗和演示页 —— 灯球 / 收件箱是透明窗口的固定深色 HUD，不跟这个开关。
   * 解析成实际外观的活儿在 `lib/theme.ts`（Rust 不知道系统外观）。
   */
  theme: "auto" | "light" | "dark";
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
  /** 只读 Web 面板（`GET /`）：手机 / 平板上打开一个链接就能看灯。**默认关。** */
  web_panel: boolean;
  /**
   * 在 Web 面板上审批（`docs/plan-approval.md` A4）。**默认关。**
   *
   * `approve_tools` 关着时它什么都不做。单独一个开关的理由：面板本来连审批详情
   * 都不拉，打开它就是**明确同意把命令原文 / 文件路径送到那个页面上**。
   * 它不是安全边界 —— 拿到 token 的人本来就能直接 curl。
   */
  web_approve: boolean;
  /**
   * 其他机器 —— **多机中枢**。默认空数组 = 一条请求都不发。
   *
   * 我们是**拉**的一方（见 `Machine`）：轮询器把对端的会话按命名空间喂进本机同一个
   * 状态机，所以灯、收件箱、统计、物理灯全部零改动就看到了多机内容。
   */
  machines: Machine[];
  /**
   * 轮询间隔（毫秒，1000–10000）。远程会话的 TTL 是它的 3 倍 —— 所以这个值同时
   * 决定了「对端掉线后多久在灯上显形」。
   */
  machine_poll_ms: number;
  /**
   * Call Me：自部署的收信服务器。**默认空 = 一条请求都不发。**
   *
   * 留言单独成页（不进收件箱）：它在语义上是「有人在你的文章下面留了话」，
   * 不是「哪个智能体卡住了」。
   */
  callme: CallMeConfig;
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
  // 和 Rust 的 `config::DEFAULT_NOTIFY_STATES` 必须一致：这里是「加这个开关之前
  // 的行为」，也是界面上 ↺ 要回到的那三个。
  notify_when: ["permission", "attention", "error"],
  hooks: [],
  devices: [],
  approve_tools: false,
  collect_backup: false,
  collect_meeting: false,
  shape: "circle",
  palette: "default",
  // 五项默认全等于「加这些开关之前」的样子 —— 和后端 `Settings::default` 一致
  sign_radius: "standard",
  sign_border: "hairline",
  sign_tint: "graphite",
  sign_text: "standard",
  sign_emphasis: "auto",
  styles: {},
  collapsed: false,
  click_action: "collapse",
  show: { name: true, state: true, age: true, message: true },
  sign_opacity: 86,
  hidden_groups: [],
  muted_groups: [],
  blink_speed: 100,
  lively: true,
  sound: true,
  quiet_hours: { enabled: false, start: 22, end: 8 },
  alert_after_secs: 5,
  rule_stuck_push: false,
  rule_busy_silence: false,
  rule_meeting: false,
  tray_follows_state: true,
  locale: "auto",
  theme: "auto",
  group_meta: {},
  group_by: "none",
  lan_access: false,
  web_panel: false,
  web_approve: false,
  machines: [],
  machine_poll_ms: 2000,
  callme: { url: "", token: "", poll_ms: 15_000 },
};

const SETTINGS_EVENT = "signalhub://settings";

/** 设置本身 + 「真值到没到」。浏览器里没有 IPC，DEFAULTS 就是真值，直接算已加载。 */
export interface SettingsState {
  settings: Settings;
  loaded: boolean;
}

/**
 * 带加载时序的那一份。首屏脚本（见 `index.html`）先按缓存定主题，而 React 一开始
 * 手里只有 DEFAULTS —— 主题得等 IPC 回来才能覆盖，否则用户选的「深色」会被默认
 * 的「跟随系统」盖掉，闪一帧白。`useTheme` 用这里的 `loaded` 卡住那一刻。
 */
export function useSettingsState(): SettingsState {
  const [state, setState] = useState<SettingsState>({
    settings: DEFAULTS,
    loaded: !inTauri,
  });

  useEffect(() => {
    if (!inTauri) return;

    // 先订阅再拉取：反过来的话，两步之间发生的变更会被永久丢掉。
    const pending = listen<Settings>(SETTINGS_EVENT, (event) =>
      setState({ settings: event.payload, loaded: true }),
    );
    invoke<Settings>("get_settings")
      .then((settings) => setState({ settings, loaded: true }))
      .catch((err) => console.error("get_settings 失败", err));

    return () => {
      pending.then((unlisten) => unlisten());
    };
  }, []);

  return state;
}

/** 只要设置本身；绝大多数组件用这个，不必关心加载时序。 */
export function useSettings(): Settings {
  return useSettingsState().settings;
}

export function updateSettings(next: Settings): Promise<Settings> {
  return invoke<Settings>("set_settings", { settings: next });
}
