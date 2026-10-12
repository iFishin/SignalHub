//! 需要你注意时发系统通知。
//!
//! 灯是**被动**的：你不盯着屏幕它就白搭，而「不用看终端」恰恰意味着你可能
//! 根本没在看灯。通知补上这半边 —— 把「你得看一眼」变成「它来叫你」。
//!
//! 这里刻意把**决策**和**发送**分开：决策全是纯函数 / 纯状态（能单测），
//! 只有最外面那层 `send` 碰 `AppHandle`（测试里造不出来）。

use crate::config::Settings;
use crate::state::{SessionView, Signal, Snapshot};
use std::sync::Mutex;
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_notification::{Action, ActionType, NotificationExt};

/// 决定「要不要弹通知」的聚合：**静音掉的组不参与**。
///
/// 与 `snapshot.aggregate` 分开是刻意的：那个是全局的（托盘、钩子、`/v1/state`
/// 都用它），而静音只该影响「打扰你」的那一部分。静音一个组不该让托盘变色，
/// 也不该让钩子不触发 —— 那两件事看的是「世界是什么样」，不是「要不要吵你」。
pub fn alerting_aggregate(snapshot: &Snapshot, muted: &[String]) -> Signal {
    // 暂停监控时不提醒。**这一条必须单独判**：组自己的 aggregate 不看 `paused`
    // （那是全局开关，只作用在 `Store::aggregate` 上），照搬「取各组最大值」会让
    // 「暂停」变成「只熄灯、照样弹通知」—— 而用户点暂停的意思正是别打扰我。
    if snapshot.paused {
        return Signal::Off;
    }

    snapshot
        .groups
        .iter()
        .filter(|group| !is_muted(&group.name, muted))
        .map(|group| group.aggregate)
        .max_by_key(|state| state.priority())
        // 全被静音（或一个组都没有）时没有可告警的东西。返回 Offline 而不是
        // 保留某个组的旧值：Offline 不是紧急状态，闸门会直接放行成「不弹」。
        .unwrap_or(Signal::Offline)
}

/// 这一组被静音了吗。列表在 `Settings::clamped` 里已经规整过（去空白、去重、
/// 排序），所以这里只做精确比较，不再 trim。
fn is_muted(name: &str, muted: &[String]) -> bool {
    muted.iter().any(|muted| muted == name)
}

/// 通知正文：胜出会话的项目名 + 它自己的说明。
///
/// 只在**没被静音**的组里找优先级最高的那个会话 —— 否则静音组的消息还是会
/// 出现在正文里，用户看到的就是一条「明明静音了却还在吵我」的通知。
fn body(snapshot: &Snapshot, muted: &[String]) -> String {
    let Some((_, session)) = alerting_session(snapshot, muted) else {
        return String::new();
    };

    let project = session.cwd.as_deref().map(project_name).unwrap_or_default();
    match (project.is_empty(), session.message.as_deref()) {
        (false, Some(message)) => format!("{project} · {message}"),
        (false, None) => project,
        (true, Some(message)) => message.to_string(),
        (true, None) => String::new(),
    }
}

/// 决定这条通知的那个会话：**排除静音组之后**优先级最高的那个，连同它所在的组名。
///
/// 标题、正文、动作按钮的目标都跟着它，所以只能算一次 —— 三处各算一遍的话，
/// 一旦算得不一致，就会出现「标题说的是 A 组、按钮却作用到 B 组」的通知。
fn alerting_session<'a>(
    snapshot: &'a Snapshot,
    muted: &[String],
) -> Option<(&'a str, &'a SessionView)> {
    snapshot
        .groups
        .iter()
        .filter(|group| !is_muted(&group.name, muted))
        .flat_map(|group| {
            group
                .sessions
                .iter()
                .map(move |session| (group.name.as_str(), session))
        })
        .max_by_key(|(_, session)| session.state.priority())
}

/// 取路径最后一段。两种分隔符都认，免得 Windows 上写的路径在别处解析错。
fn project_name(cwd: &str) -> String {
    cwd.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

/// 前端推过来的「现在是几点（本地时间）」。
///
/// Rust 这边没有时区数据，只有 UTC —— 所以「现在几点」这个**事实**只能由前端提供。
/// 但「在不在勿扰窗口内」的**判断**留在 Rust（`config::QuietHours::covers`）：
/// 跨午夜那种边界逻辑只有一处实现才敢信，而 Rust 这边能单测。
///
/// 存 `Option`：前端还没推过第一帧之前是「不知道」。不知道时按「不在勿扰」
/// 处理（`covers` 拿不到小时就不会被调用），宁可多弹一次也不要因为一个尚未
/// 建立的事实而永久静音。
#[derive(Default)]
pub struct LocalHour(Mutex<Option<u8>>);

impl LocalHour {
    pub fn set(&self, hour: u8) {
        *self.0.lock().expect("local hour poisoned") = Some(hour);
    }

    pub fn get(&self) -> Option<u8> {
        *self.0.lock().expect("local hour poisoned")
    }
}

/// 告警闸门。
///
/// 它替掉了原来那个 `should_alert(previous, current)`：那个只能表达「跃迁时弹一次」，
/// 而阈值需要「等一会儿再判」，还要能被状态变化取消。
#[derive(Default)]
pub struct Gate {
    /// 当前「值得注意」的聚合值（已排除静音组）。`None` = 还没见过任何状态。
    alerting: Option<Signal>,
    /// 这个 episode 已经通知过了
    notified: bool,
    /// 这个 episode 已经排过一次延迟检查了
    pending: bool,
    /// 第几轮。**重复提醒靠它认身份。**
    ///
    /// 状态离开又回到同一个值时是一轮新的（`alerting` 变过，只是恰好变了回来）。
    /// 上一轮排下的提醒任务必须认得出这件事并失效 —— 否则它会替新一轮发一条
    /// 「已等待 20 分钟」，而新一轮其实才刚开始。
    episode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 现在就弹
    Now,
    /// 等 `alert_after_secs` 之后再判
    Later,
    /// 不弹
    None,
}

impl Gate {
    /// 记下这次的聚合值，返回该怎么处理。
    ///
    /// `now` 不用在这里，计时由调用方负责（它才知道怎么排延迟任务）。
    pub fn observe(&mut self, alerting: Signal, threshold: Duration, quiet: bool) -> Decision {
        // 聚合值一变就是新的一轮：`notified` / `pending` 都要清零，否则新一轮
        // 会被上一轮的「已通知」挡住，或者上一轮排的定时器会替它把检查提前。
        if self.alerting != Some(alerting) {
            self.alerting = Some(alerting);
            self.notified = false;
            self.pending = false;
            // 轮次加一：上一轮排下的重复提醒因此失效（见 `episode` 的说明）
            self.episode = self.episode.wrapping_add(1);
        }

        // 不是紧急状态就永远不弹 —— 落回普通状态不需要通知
        if !is_urgent(alerting) {
            return Decision::None;
        }

        // 勿扰时段：这一轮**彻底**不弹，并标记「已通知」，这样之前排下的定时器
        // 到点时会被 confirm 判成 false。刻意不做「延后补发」—— 补发意味着早上
        // 醒来面对一整夜攒下的通知，那正是勿扰想避免的事。灯照常显示状态，
        // 所以什么信息都没丢。
        if quiet {
            self.notified = true;
            return Decision::None;
        }

        // 这个 episode 已经通知过了。这是老 `should_alert` 的「只在跃迁时弹一次」：
        // 一轮工具调用会推几十次同样的状态，每次都弹就是骚扰。
        if self.notified {
            return Decision::None;
        }

        // 阈值非零：先排一次延迟检查。只排一次 —— 同一轮里的第二次推送
        // 不该再排一个定时器，否则一轮几十次推送就是几十个定时器。
        if threshold > Duration::ZERO {
            if self.pending {
                return Decision::None;
            }
            self.pending = true;
            return Decision::Later;
        }

        // 阈值是 0，立刻弹
        self.notified = true;
        Decision::Now
    }

    /// 延迟到点后重新判：状态还是那个、而且还没通知过，才返回 true。
    ///
    /// 也顺手把 `notified` 置上，于是重复到点的定时器不会再弹第二次。
    pub fn confirm(&mut self, alerting: Signal) -> bool {
        if self.alerting == Some(alerting) && !self.notified {
            self.notified = true;
            true
        } else {
            false
        }
    }

    /// 现在是第几轮。重复提醒的任务拿它认身份 —— 对不上就说明那一轮已经过去了。
    pub fn episode(&self) -> u64 {
        self.episode
    }
}

/// 这个状态已经持续了多久（毫秒）。取**最久的那一组**：几组同时处于同一个紧急
/// 状态时，只要有一组卡了很久，这个状态就该按最久的算。
///
/// 和 `sustained` 用同一个事实来源（`GroupView::since_ms`）—— 各算一份的话，两处
/// 迟早会对「到底卡了多久」给出不同的答案。
pub fn stuck_for_ms(snapshot: &Snapshot, muted: &[String], state: Signal) -> u64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    snapshot
        .groups
        .iter()
        .filter(|group| !is_muted(&group.name, muted))
        .filter(|group| group.aggregate == state)
        .map(|group| now_ms.saturating_sub(group.since_ms))
        .max()
        .unwrap_or(0)
}

/// 这个状态值得打扰用户吗。抽出来是为了让「哪些状态算紧急」只有一处定义 ——
/// `observe` 和调用方都用它，免得两边漂移。
pub fn is_urgent(state: Signal) -> bool {
    matches!(state, Signal::Permission | Signal::Attention | Signal::Error)
}

/// 第一次通知之后再提醒几次，间隔递增。
///
/// 5 分钟就先提一次：卡住的是智能体，它**真的什么都不做**，所以越早把人叫回来越好。
/// 之后越拖越久再提，50 分钟之后停 —— 到那时还没人管，再响多半也没用，而一直响就
/// 成了骚扰。
///
/// 放在这里而不是 `notify.rs`：它是**告警策略**（提几次、隔多久），而那个文件只管
/// 怎么排任务。分开放的话，这个常量就没法单测了。
pub const REMIND_GAPS: [Duration; 3] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
];

/// 系统「默认提示音」的名字。**没有三平台通吃的写法**，只能逐平台取值：
///
/// - macOS 认这个哨兵串就播默认音效；换成别的名字，它会当成自定义音效去
///   `/System/Library/Sounds` 里找同名文件，找不到就一声不吭。
/// - Windows 的 toast 只认 `Default` / `IM` / `Mail` / `Reminder` / `SMS` /
///   `Alarm*` 这几个内置标识，解析不了的串会被丢掉 —— 拿 macOS 那个哨兵喂它，
///   结果就是静音。
/// - Linux 走 XDG 的 `sound-name` 提示，而 dunst / GNOME 这些常见的通知守护进程
///   压根不理会它。传个名字过去只会让人以为配好了，不如老实返回 `None`。
#[cfg(target_os = "macos")]
const DEFAULT_SOUND: Option<&str> = Some("NSUserNotificationDefaultSoundName");
#[cfg(target_os = "windows")]
const DEFAULT_SOUND: Option<&str> = Some("Default");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const DEFAULT_SOUND: Option<&str> = None;

/// 把开关翻译成「挂在通知上的声音名」。抽出来是为了能在没有 AppHandle 的单测里
/// 验证这个映射 —— `AppHandle` 在测试里根本造不出来。
fn sound_name(sound: bool) -> Option<&'static str> {
    if sound { DEFAULT_SOUND } else { None }
}

/// 通知上那组动作按钮的类型 id。
///
/// 系统要求先把按钮注册成一个「类型」，通知再按 id 引用它 —— 这是 macOS 的
/// `UNNotificationCategory` 那一套，插件在三个桌面上都抹平了。
const ACTION_TYPE_ID: &str = "signalhub.alert";

/// 「打开项目目录」。**放第一个是刻意的**：macOS 上按钮收在「选项」菜单里，
/// 而点「选项」本身会上报第一个动作，所以第一个必须是点了也无害的那个。
const ACTION_REVEAL: &str = "reveal";

/// 「静音该组」
const ACTION_MUTE: &str = "mute";

/// 最近一条告警通知说的是哪一组、哪个目录。
///
/// 按钮是**用户点的时候**才回调的，而那时状态可能早就变了：从当前快照重新算
/// 「优先级最高的会话」会指向另一个会话，用户点的按钮就作用到了别处。所以在发
/// 通知的那一刻把目标记下来。
///
/// 只记最后一条就够：告警本身有节流（同一个 episode 只弹一次），同时存在两条
/// 待处理的通知实际上不会发生。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertTarget {
    /// 决定这条通知的那个会话所在的项目目录。「打开项目目录」用它。
    pub cwd: Option<String>,
    /// 它所属的组。「静音该组」用它。
    pub group: String,
}

#[derive(Default)]
pub struct LastAlert(Mutex<Option<AlertTarget>>);

impl LastAlert {
    fn set(&self, target: AlertTarget) {
        *self.0.lock().expect("alert target poisoned") = Some(target);
    }

    fn get(&self) -> Option<AlertTarget> {
        self.0.lock().expect("alert target poisoned").clone()
    }
}

/// 这条通知说的会话是谁。
fn target_for(snapshot: &Snapshot, muted: &[String]) -> AlertTarget {
    match alerting_session(snapshot, muted) {
        Some((group, session)) => AlertTarget {
            cwd: session.cwd.clone(),
            group: group.to_string(),
        },
        // 没有会话时不该走到这里（没会话的聚合是 Offline，不是紧急状态），
        // 但真走到了就留一个空目标：按钮点下去什么也不做，比作用到别处强。
        None => AlertTarget::default(),
    }
}

/// 发一条通知。`sound` 决定是否带系统提示音。任何失败只记 stderr ——
/// 通知发不出去不该影响状态机。
///
/// 声音是挂在系统通知上的（`NotificationBuilder::sound`），所以它只有在通知本身
/// 发得出去时才存在 —— 这正是 `Settings::sound` 与 `Settings::notify` 的耦合：
/// 关掉通知，这里根本不会被调用，声音自然也一并没了。
pub fn send(app: &tauri::AppHandle, snapshot: &Snapshot, sound: bool, muted: &[String]) {
    send_alert(app, snapshot, sound, muted, body(snapshot, muted));
}

/// 重复提醒：状态还卡着，再提一次，正文里带上**已经等了多久**。
///
/// 为什么要有这个：智能体卡在等授权上时是**真的什么都不做**，没人去管它就永远停在
/// 那儿。而第一次通知只弹一次 —— 你要是正好在开会、没看见那条，回来只看到一盏红
/// 灯，完全不知道它已经卡了一小时。
///
/// 正文不是把第一次的通知重发一遍，而是明说等了多久：**「还没解决」和「已经卡了
/// 20 分钟」是两条不同的信息**，后者才推得动人。
pub fn send_reminder(app: &tauri::AppHandle, snapshot: &Snapshot, sound: bool, muted: &[String]) {
    let state = alerting_aggregate(snapshot, muted);
    let minutes = stuck_for_ms(snapshot, muted, state) / 60_000;
    let lang = crate::current_lang(app);

    let mut text = body(snapshot, muted);
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&crate::lang::stuck_for(lang, minutes));

    send_alert(app, snapshot, sound, muted, text);
}

/// 告警通知的公共部分：标题、记下「这条通知说的是谁」、发出去。正文由调用方给
/// （第一次通知和重复提醒只有正文不同）。
///
/// 抽这一层是因为 `LastAlert` 那一步**不能漏**：动作按钮是用户点的时候才回调的，
/// 那时状态可能早就变了，少了这一步按钮就会作用到别的组上。
fn send_alert(
    app: &tauri::AppHandle,
    snapshot: &Snapshot,
    sound: bool,
    muted: &[String],
    body: String,
) {
    // 标题也要用**排除静音组之后**的聚合值：用全局那个的话，全局最高恰好在
    // 一个被静音的组里时，标题会写着那一组的状态，而正文却是另一组的会话 ——
    // 一条自己跟自己不一致的通知。
    let lang = crate::current_lang(app);
    let title = crate::lang::signal_label(lang, alerting_aggregate(snapshot, muted));
    // 记下这条通知说的是谁：按钮回调时状态可能已经变了，见 AlertTarget 的说明
    app.state::<LastAlert>().set(target_for(snapshot, muted));
    show(app, title, &body, sound, Some(ACTION_TYPE_ID));
}

/// 发一条自定义文案的通知。状态通知之外的通告用它（例如「已进入穿透模式」）。
/// 这类通告**不是**告警，固定静音，也没有动作按钮。
pub fn send_message(app: &tauri::AppHandle, title: &str, body: &str) {
    show(app, title, body, false, None);
}

/// 通知的统一出口：标题 + 正文 + 可选的提示音和动作类型，再 show。
///
/// 抽这一层是因为两条路径（告警 / 通告）走的是同一个 builder 链，只是声音和
/// 按钮不同，把 `.title().body().show()` 抄两遍迟早会分叉。
fn show(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    sound: bool,
    action_type: Option<&str>,
) {
    let mut builder = app.notification().builder().title(title).body(body);
    if let Some(name) = sound_name(sound) {
        builder = builder.sound(name);
    }
    if let Some(action_type) = action_type {
        builder = builder.action_type_id(action_type);
    }

    if let Err(err) = builder.show() {
        eprintln!("[signalhub] 发送通知失败: {err}");
    }
}

/// 注册通知上的动作按钮，以及它们被点之后怎么处理。
///
/// **两件事必须一起做**：插件在「没有注册处理函数」时会干脆不显示按钮（否则点
/// 下去没人接）。绑在一个函数里，就不会只做一半 —— 只注册类型不加处理函数的
/// 表现是「按钮从来没见过」，而且不报错。
///
/// 代价：一旦挂上处理函数，插件对**所有**通知都会起一条线程等用户交互（它要靠
/// 这个才知道用户点了什么）。告警本身有节流（一个 episode 只弹一次），所以挂着
/// 的线程最多就是「今天弹过几条通知」那么多，不值得为它绕路。
pub fn wire_actions(app: &tauri::AppHandle) {
    let notification = app.notification();
    let t = crate::lang::text(crate::current_lang(app));

    let actions = vec![
        Action::builder(ACTION_REVEAL, t.action_reveal).build(),
        Action::builder(ACTION_MUTE, t.action_mute).build(),
    ];
    let action_type = ActionType::builder(ACTION_TYPE_ID).actions(actions).build();

    if let Err(err) = notification.register_action_types(vec![action_type]) {
        eprintln!("[signalhub] 注册通知动作类型失败: {err}");
        return;
    }

    let handle = app.clone();
    if let Err(err) = notification.on_action(move |performed| {
        let Some(target) = handle.state::<LastAlert>().get() else {
            return;
        };
        match performed.action_id() {
            // 点通知本体等同于「打开项目目录」—— 「知道了，我去看看」是最自然的
            // 那件事，而什么都不做会让人觉得点了没反应
            "tap" | ACTION_REVEAL => reveal(&target),
            ACTION_MUTE => mute(&handle, &target),
            _ => {}
        }
    }) {
        eprintln!("[signalhub] 注册通知动作处理函数失败: {err}");
    }
}

/// 在文件管理器里打开那个项目目录。
///
/// 具体怎么调系统命令在 `crate::open` 里 —— 那里还管着「打开配置目录」之类的
/// 其它入口，而且**参数一律走独立 argv、不过 shell**（这个路径来自智能体上报的
/// `cwd`，是不可信输入）。
fn reveal(target: &AlertTarget) {
    let Some(cwd) = target.cwd.as_deref() else {
        return;
    };
    // 目录可能已经没了（项目被删、移动盘没挂上）。那就不动 —— 弹一个「找不到」
    // 的报错框比什么都不做更烦人。
    if !std::path::Path::new(cwd).is_dir() {
        return;
    }
    if let Err(err) = crate::open::reveal(cwd) {
        eprintln!("[signalhub] 打开项目目录失败: {err}");
    }
}

/// 从通知里静音那一组。
fn mute(app: &tauri::AppHandle, target: &AlertTarget) {
    if target.group.is_empty() {
        return;
    }
    if let Err(err) = crate::set_group_muted(app, &target.group, true) {
        eprintln!("[signalhub] 从通知里静音该组失败: {err}");
    }
}

/// 出站转发已搬到 `outbound.rs`（钩子列表），这里只负责系统通知。

/// 首次启动时申请通知权限。
///
/// 放在启动时而不是「第一次要发通知时」，是因为弹权限框和发通知没法同时发生
/// —— 那样第一条通知必然丢掉，而第一条往往正是最要紧的那条。
///
/// **也在每次设置变更时调**（见 `lib.rs` 的 `apply_settings`）。原来只在启动时申请，
/// 于是「启动时通知是关着的」用户永远不会被问一次 —— 而没权限时通知是**静默丢弃**
/// 的：不弹、不响、也没有任何地方报错。那正是「通知功能好像坏了」最难查的形态。
pub fn ensure_permission(app: &tauri::AppHandle, settings: &Settings) {
    if !settings.notify {
        // 用户关掉了通知就完全不要打扰他，连权限框也不要弹
        return;
    }

    let notification = app.notification();
    match notification.permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted)
        | Ok(tauri_plugin_notification::PermissionState::Denied) => {}
        // 只有「还没问过」才弹框。**被拒绝之后不再弹** —— 系统本来也不会再问，
        // 反复调只会掩盖真正该做的事：告诉用户去系统设置里打开。
        Ok(_) => {
            if let Err(err) = notification.request_permission() {
                eprintln!("[signalhub] 申请通知权限失败: {err}");
            }
        }
        Err(err) => eprintln!("[signalhub] 读取通知权限失败: {err}"),
    }
}

/// 系统通知权限的当前状态，给界面显示用。取值：`granted` / `denied` / `prompt` / `unknown`。
///
/// 存在的理由很具体：**没权限时 `show()` 是静默失败的** —— 通知不弹、声音不响，而且
/// 没有任何地方会报错（从 Dock 启动的话连 stderr 都没地方看）。用户看到的是「通知
/// 功能坏了」，但其实代码里什么都对，只是系统没放行。界面上能回答这个问题，才不至于
/// 让人去猜。
pub fn permission_state(app: &tauri::AppHandle) -> &'static str {
    match app.notification().permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted) => "granted",
        Ok(tauri_plugin_notification::PermissionState::Denied) => "denied",
        Ok(_) => "prompt",
        Err(_) => "unknown",
    }
}

/// 主动要一次通知权限，返回要过之后的状态。界面上的「请求权限」按钮走它。
pub fn request_permission(app: &tauri::AppHandle) -> &'static str {
    if let Err(err) = app.notification().request_permission() {
        eprintln!("[signalhub] 申请通知权限失败: {err}");
    }
    permission_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GroupView, SessionView, DEFAULT_GROUP};

    fn session(id: &str, state: Signal, cwd: Option<&str>, message: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_string(),
            state,
            agent: None,
            cwd: cwd.map(str::to_string),
            message: message.map(str::to_string),
            age_ms: 0,
            stale_progress: None,
        }
    }

    fn group(name: &str, aggregate: Signal, sessions: Vec<SessionView>) -> GroupView {
        GroupView {
            name: name.to_string(),
            aggregate,
            message: None,
            since_ms: 0,
            stale_progress: None,
            sessions,
        }
    }

    /// 多组快照。`aggregate` 按真实规则取各组最高优先级 —— 静音的测试正是要
    /// 验证「这个全局值不动，但告警用的那个动了」。
    fn snapshot_of(groups: Vec<GroupView>) -> Snapshot {
        let aggregate = groups
            .iter()
            .map(|group| group.aggregate)
            .max_by_key(|state| state.priority())
            .unwrap_or(Signal::Offline);
        Snapshot {
            aggregate,
            paused: false,
            since_ms: 0,
            message: None,
            groups,
        }
    }

    /// 单组快照，大部分用例用它。
    fn snapshot(aggregate: Signal, sessions: Vec<SessionView>) -> Snapshot {
        snapshot_of(vec![group(DEFAULT_GROUP, aggregate, sessions)])
    }

    // ---- 静音 ---------------------------------------------------------------

    /// 静音只影响「要不要打扰你」，不影响全局聚合 —— 托盘和钩子看的是后者
    #[test]
    fn muting_a_group_only_lowers_the_alerting_aggregate() {
        let snapshot = snapshot_of(vec![
            group("work", Signal::Error, vec![]),
            group("ci", Signal::Permission, vec![]),
        ]);

        assert_eq!(alerting_aggregate(&snapshot, &[]), Signal::Error);
        assert_eq!(
            alerting_aggregate(&snapshot, &["work".into()]),
            Signal::Permission,
            "静音掉最高的那一组后，告警聚合要落到剩下的最高者"
        );
        assert_eq!(snapshot.aggregate, Signal::Error, "全局聚合不该被静音改动");
    }

    /// 暂停监控时不该弹通知。
    ///
    /// 这一条很容易漏：组自己的 aggregate **不看** `paused`（那是全局开关，
    /// 只作用在 `Store::aggregate` 上），所以照搬「取各组最大值」会让「暂停」
    /// 退化成「只熄灯、照样弹通知」—— 而用户点暂停的意思正是别打扰我。
    #[test]
    fn pausing_stops_the_alerts_entirely() {
        let mut snapshot = snapshot_of(vec![group("ci", Signal::Permission, vec![])]);
        assert_eq!(alerting_aggregate(&snapshot, &[]), Signal::Permission);

        snapshot.paused = true;
        let paused = alerting_aggregate(&snapshot, &[]);
        assert_eq!(paused, Signal::Off);
        assert!(!is_urgent(paused), "Off 不是紧急状态，闸门因此不会弹");
    }

    /// 全静音时没有可告警的东西 → Offline（不是紧急状态，闸门直接放行成不弹）
    #[test]
    fn muting_every_group_leaves_nothing_to_alert() {
        let snapshot = snapshot_of(vec![
            group("work", Signal::Error, vec![]),
            group("ci", Signal::Permission, vec![]),
        ]);
        assert_eq!(
            alerting_aggregate(&snapshot, &["work".into(), "ci".into()]),
            Signal::Offline
        );
    }

    /// 正文必须也跟着静音过滤，否则静音组的消息还是会出现在通知里
    #[test]
    fn the_body_ignores_muted_groups() {
        let snapshot = snapshot_of(vec![
            group(
                "work",
                Signal::Error,
                vec![session("a", Signal::Error, Some("/srv/work"), Some("炸了"))],
            ),
            group(
                "ci",
                Signal::Permission,
                vec![session("b", Signal::Permission, Some("/srv/ci"), Some("要授权"))],
            ),
        ]);

        assert_eq!(body(&snapshot, &[]), "work · 炸了", "默认取全局最紧急的那一路");
        assert_eq!(
            body(&snapshot, &["work".into()]),
            "ci · 要授权",
            "静音掉的组不该出现在正文里"
        );
    }

    // ---- 动作按钮的目标 -------------------------------------------------------

    /// 按钮的目标必须和正文说的是**同一个**会话 —— 否则会出现「正文说 ci 要授权，
    /// 点『静音该组』却静音了 work」这种自相矛盾的通知
    #[test]
    fn the_action_target_matches_the_session_the_body_names() {
        let snapshot = snapshot_of(vec![
            group(
                "work",
                Signal::Error,
                vec![session("a", Signal::Error, Some("/srv/work"), Some("炸了"))],
            ),
            group(
                "ci",
                Signal::Permission,
                vec![session("b", Signal::Permission, Some("/srv/ci"), Some("要授权"))],
            ),
        ]);

        assert_eq!(
            target_for(&snapshot, &[]),
            AlertTarget { cwd: Some("/srv/work".into()), group: "work".into() }
        );
        assert_eq!(
            target_for(&snapshot, &["work".into()]),
            AlertTarget { cwd: Some("/srv/ci".into()), group: "ci".into() },
            "静音掉最高的那一组后，目标要跟着落到剩下的那一路"
        );
    }

    /// 没有会话时不该 panic，也不该指向任何真实的组 —— 按钮点下去什么也不做
    #[test]
    fn a_snapshot_with_no_sessions_yields_an_empty_target() {
        assert_eq!(target_for(&snapshot_of(vec![]), &[]), AlertTarget::default());
    }

    /// 会话没有 cwd 时只记组名：「打开项目目录」会自己什么都不做（见 reveal）
    #[test]
    fn a_session_without_a_directory_still_gives_a_group() {
        let snapshot = snapshot_of(vec![group(
            "ci",
            Signal::Error,
            vec![session("a", Signal::Error, None, Some("炸了"))],
        )]);

        let target = target_for(&snapshot, &[]);
        assert_eq!(target.cwd, None);
        assert_eq!(target.group, "ci", "静音该组仍然要能用");
    }

    // ---- 勿扰 ---------------------------------------------------------------

    /// 前端还没推过本地时间时是「不知道」，推了之后要记住
    #[test]
    fn the_local_hour_starts_unknown_and_remembers_what_the_frontend_pushes() {
        let hour = LocalHour::default();
        assert_eq!(hour.get(), None, "前端还没推过就该是「不知道」");
        hour.set(9);
        assert_eq!(hour.get(), Some(9));
    }

    // ---- 闸门 ---------------------------------------------------------------

    /// 紧急状态的定义只有一处 —— `observe` 和调用方都靠它
    #[test]
    fn only_permission_attention_and_error_are_urgent() {
        for state in [Signal::Permission, Signal::Attention, Signal::Error] {
            assert!(is_urgent(state), "{state:?} 应算紧急");
        }
        for state in [
            Signal::Offline,
            Signal::Off,
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
        ] {
            assert!(!is_urgent(state), "{state:?} 不该算紧急");
        }
    }

    /// 首次拿到状态就是需要注意的 → 立刻弹（等价于老 `should_alert(None, Permission)`）
    #[test]
    fn only_the_transition_into_an_urgent_state_alerts() {
        // 普通状态不弹，但随后的跃迁要弹
        let mut gate = Gate::default();
        assert!(matches!(
            gate.observe(Signal::Working, Duration::ZERO, false),
            Decision::None
        ));
        assert!(matches!(
            gate.observe(Signal::Permission, Duration::ZERO, false),
            Decision::Now
        ));

        // 换一个紧急状态 = 新的一轮，也该弹
        assert!(matches!(
            gate.observe(Signal::Attention, Duration::ZERO, false),
            Decision::Now
        ));
        assert!(matches!(
            gate.observe(Signal::Error, Duration::ZERO, false),
            Decision::Now
        ));
    }

    /// 这条是核心：一轮工具调用会触发几十次状态推送，每次都弹就是骚扰
    #[test]
    fn staying_in_an_urgent_state_does_not_alert_again() {
        let mut gate = Gate::default();
        assert!(matches!(
            gate.observe(Signal::Permission, Duration::ZERO, false),
            Decision::Now
        ));
        for _ in 0..3 {
            assert!(matches!(
                gate.observe(Signal::Permission, Duration::ZERO, false),
                Decision::None
            ));
        }
    }

    #[test]
    fn ordinary_states_never_alert() {
        for state in [
            Signal::Offline,
            Signal::Off,
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
        ] {
            // 从「没见过状态」直接来
            let mut fresh = Gate::default();
            assert!(
                matches!(fresh.observe(state, Duration::ZERO, false), Decision::None),
                "{state:?} 不该弹通知"
            );

            // 从紧急状态落下到普通状态也不弹
            let mut gate = Gate::default();
            assert!(matches!(
                gate.observe(Signal::Permission, Duration::ZERO, false),
                Decision::Now
            ));
            assert!(
                matches!(gate.observe(state, Duration::ZERO, false), Decision::None),
                "{state:?} 不该弹通知"
            );
        }
    }

    #[test]
    fn leaving_an_urgent_state_does_not_alert() {
        let mut gate = Gate::default();
        assert!(matches!(
            gate.observe(Signal::Permission, Duration::ZERO, false),
            Decision::Now
        ));
        assert!(matches!(
            gate.observe(Signal::Working, Duration::ZERO, false),
            Decision::None
        ));
        assert!(matches!(
            gate.observe(Signal::Idle, Duration::ZERO, false),
            Decision::None
        ));
    }

    /// 聚合值一变就是新的一轮：`notified` / `pending` 都要清零，否则新一轮会被
    /// 上一轮的「已通知」挡住
    #[test]
    fn a_changed_aggregate_starts_a_fresh_episode() {
        let mut gate = Gate::default();
        assert!(matches!(
            gate.observe(Signal::Permission, Duration::ZERO, false),
            Decision::Now
        ));
        // 落下再回来 = 新的一轮，应该能再弹
        assert!(matches!(
            gate.observe(Signal::Working, Duration::ZERO, false),
            Decision::None
        ));
        assert!(matches!(
            gate.observe(Signal::Permission, Duration::ZERO, false),
            Decision::Now
        ));
    }

    /// 非零阈值：第一次先排延迟，别急着弹 —— 智能体常常一秒内进出 permission
    #[test]
    fn a_nonzero_threshold_defers_the_first_alert() {
        let mut gate = Gate::default();
        assert!(matches!(
            gate.observe(Signal::Permission, Duration::from_secs(5), false),
            Decision::Later
        ));
    }

    /// 同一轮里的第二次推送不该再排一次延迟 —— 一轮工具调用会推几十次状态，
    /// 每次排一个定时器就是把定时器堆爆
    #[test]
    fn a_second_push_in_the_same_episode_does_not_reschedule() {
        let mut gate = Gate::default();
        let threshold = Duration::from_secs(5);
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, false),
            Decision::Later
        ));
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, false),
            Decision::None
        ));
        // 第三次仍然是 None，证明 pending 没被第二次 observe 重置
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, false),
            Decision::None
        ));
    }

    /// 延迟到点后确认：状态还是那个、而且还没通知过，才发
    #[test]
    fn confirm_fires_only_when_the_state_is_unchanged_and_unnotified() {
        let mut gate = Gate::default();
        let threshold = Duration::from_secs(5);
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, false),
            Decision::Later
        ));
        assert!(gate.confirm(Signal::Permission));
        // 已经通知过了，重复到点的定时器不能再弹一次
        assert!(!gate.confirm(Signal::Permission), "同一轮不该被两个定时器弹两次");
    }

    /// 经典场景：阈值还没到，permission 就自己解决了 —— 用户永远不该被打扰
    #[test]
    fn confirm_after_the_state_moved_away_is_false() {
        let mut gate = Gate::default();
        let threshold = Duration::from_secs(5);
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, false),
            Decision::Later
        ));
        // 定时器到点之前，状态先走了
        assert!(matches!(
            gate.observe(Signal::Working, threshold, false),
            Decision::None
        ));
        assert!(!gate.confirm(Signal::Permission), "状态已经不是它了，不该弹");
    }

    /// 勿扰：这一轮彻底不弹，连之前排下的定时器也要作废
    #[test]
    fn quiet_hours_suppress_the_alert_for_the_whole_episode() {
        let mut gate = Gate::default();
        let threshold = Duration::from_secs(5);

        // 先排了一次延迟检查
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, false),
            Decision::Later
        ));
        // 勿扰开始了，新的推送把这一轮标记成「已通知」
        assert!(matches!(
            gate.observe(Signal::Permission, threshold, true),
            Decision::None
        ));
        // 之前那个定时器到点：不该再弹
        assert!(
            !gate.confirm(Signal::Permission),
            "勿扰期间到点的定时器必须作废，不能延后补发"
        );
    }

    /// 立即弹之后也要标记「已通知」，否则同一轮里的后续推送会再弹
    #[test]
    fn an_immediate_alert_marks_the_episode_notified() {
        let mut gate = Gate::default();
        assert!(matches!(
            gate.observe(Signal::Error, Duration::ZERO, false),
            Decision::Now
        ));
        assert!(matches!(
            gate.observe(Signal::Error, Duration::ZERO, false),
            Decision::None
        ));
    }

    // ---- 正文与声音 ---------------------------------------------------------

    #[test]
    fn the_body_names_the_project_and_the_agents_own_words() {
        let snapshot = snapshot(
            Signal::Permission,
            vec![session(
                "a",
                Signal::Permission,
                Some("/Users/me/proj"),
                Some("需要批准: rm -rf build"),
            )],
        );
        assert_eq!(body(&snapshot, &[]), "proj · 需要批准: rm -rf build");
    }

    #[test]
    fn the_body_degrades_gracefully() {
        let with = |cwd: Option<&str>, message: Option<&str>| {
            body(
                &snapshot(
                    Signal::Attention,
                    vec![session("a", Signal::Attention, cwd, message)],
                ),
                &[],
            )
        };
        assert_eq!(with(Some("/srv/build"), None), "build");
        assert_eq!(with(None, Some("agent needs input")), "agent needs input");
        assert_eq!(with(None, None), "");
        // 没有会话时不该 panic
        assert_eq!(body(&snapshot(Signal::Offline, vec![]), &[]), "");
    }

    #[test]
    fn project_name_handles_both_separators_and_trailing_slashes() {
        assert_eq!(project_name("/Users/me/proj"), "proj");
        assert_eq!(project_name("/Users/me/proj/"), "proj");
        assert_eq!(project_name(r"C:\work\api"), "api");
        assert_eq!(project_name("proj"), "proj");
    }

    /// 开关到声音名的映射是唯一能在无 AppHandle 下单测的部分，所以单独钉住它。
    #[test]
    fn the_sound_flag_selects_the_default_sound() {
        assert_eq!(sound_name(true), DEFAULT_SOUND, "开着就该带上本平台的默认音");
        assert_eq!(sound_name(false), None, "关掉就不该带声音");
    }

    // ---- 重复提醒 -------------------------------------------------------------

    /// 现在（epoch 毫秒），给下面几个用例算 since 用。
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟倒退")
            .as_millis() as u64
    }

    fn group_since(name: &str, aggregate: Signal, minutes_ago: u64) -> GroupView {
        let mut group = group(name, aggregate, vec![]);
        group.since_ms = now_ms() - minutes_ago * 60_000;
        group
    }

    /// 轮次只在**状态真的变了**时才推进。
    ///
    /// `observe` 每次状态推送都会被调用（一轮工具调用几十次），要是每次都加一，
    /// 排下的提醒就永远认不出自己那一轮、永远提前失效 —— 等于这个功能不存在。
    #[test]
    fn the_episode_advances_only_when_the_state_changes() {
        let mut gate = Gate::default();

        for _ in 0..5 {
            gate.observe(Signal::Permission, Duration::ZERO, false);
        }
        let first = gate.episode();
        assert_eq!(gate.episode(), first, "同一个状态推五次不该算五轮");

        gate.observe(Signal::Working, Duration::ZERO, false);
        assert_ne!(gate.episode(), first, "状态变了就是新的一轮");

        // **离开又回到同一个值**也是一轮新的：提醒任务必须认得出这件事，否则它会
        // 替新一轮发一条「已等待 20 分钟」，而新一轮才刚开始。
        let second = gate.episode();
        gate.observe(Signal::Permission, Duration::ZERO, false);
        assert_ne!(gate.episode(), second, "回到同一个值也是一轮新的");
    }

    /// 「已经等了多久」取**最久的那一组**：几组同时处于同一个紧急状态时，只要有一组
    /// 卡了很久，这个状态就该按最久的算。
    #[test]
    fn stuck_for_takes_the_longest_matching_group() {
        let snap = snapshot_of(vec![
            group_since("ci", Signal::Permission, 20),
            group_since("docs", Signal::Permission, 1),
            // 别的状态不参与
            group_since("web", Signal::Working, 999),
        ]);

        let ms = stuck_for_ms(&snap, &[], Signal::Permission);
        assert!(
            (20 * 60_000..21 * 60_000).contains(&ms),
            "该取最久的那组（20 分钟），实际 {ms}"
        );
    }

    /// 静音组不算数 —— 和告警本身同一条规则。否则一个被标了「别吵我」的组会一直
    /// 触发提醒，而那正是用户说「别烦我」的意思。
    #[test]
    fn stuck_for_ignores_muted_groups() {
        let snap = snapshot_of(vec![
            group_since("noisy", Signal::Error, 30),
            group_since("ci", Signal::Error, 2),
        ]);

        let ms = stuck_for_ms(&snap, &["noisy".to_string()], Signal::Error);
        assert!(
            (2 * 60_000..3 * 60_000).contains(&ms),
            "被静音的那组不该算进来，实际 {ms}"
        );
    }

    /// 一组都不匹配时是 0，不是 panic、也不是「所有组里最久的那个」
    #[test]
    fn stuck_for_is_zero_when_nothing_matches() {
        let snap = snapshot_of(vec![group_since("ci", Signal::Working, 30)]);
        assert_eq!(stuck_for_ms(&snap, &[], Signal::Error), 0);
        assert_eq!(stuck_for_ms(&snapshot_of(vec![]), &[], Signal::Error), 0);
    }

    /// 间隔必须是递增的。这条不是在测代码，是在钉住那个设计决定：越拖越久再提，
    /// 而不是固定频率地响 —— 后者对「一直卡着」和「刚卡住」一视同仁。
    #[test]
    fn the_reminder_gaps_grow() {
        let gaps: Vec<u64> = REMIND_GAPS.iter().map(|gap| gap.as_secs()).collect();
        assert!(
            gaps.windows(2).all(|pair| pair[0] < pair[1]),
            "重复提醒的间隔应当递增：{gaps:?}"
        );
        assert!(!gaps.is_empty(), "至少得提一次，否则这个功能不存在");
    }
}
