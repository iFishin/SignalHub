//! 物理灯（WLED）：把**状态色**发到灯上，让桌面之外也能一眼看见。
//!
//! 和 `outbound.rs` 的关系：那边发的是「一段给人看的文字」，这边发的是「一个颜色」。
//! 所以这边没有 `when` 过滤、没有组、没有消息 —— **发出去的东西里只有三个字节的
//! RGB**（红线 3）。灯可能挂在别人也看得见的地方，而 WLED 自己的界面会把收到的
//! 东西回显出来。
//!
//! 三条约束，和出站钩子同源：
//! 1. **不阻塞。** 触发点在状态变更路径上（每次快照推送都会走到），同步发网络请求
//!    会把状态机卡住。
//! 2. **失败不影响别的灯，也不影响状态机。** 失败只写日志 + stderr。
//! 3. **不发没变过的颜色。** 一次工具调用能推几十次状态，而灯只在「变」的时候才
//!    需要发 —— 这一条同时也把「灯被拔掉之后每秒敲一次」挡住了。
//!
//! 和钩子一样，结果记进**同一个** `outbound.json` / `hooks.log`（见 [`crate::outbound`]）：
//! 用户只需要一个地方看「我配的东西到底发出去了没有」。

use crate::config::{Device, Settings};
use crate::outbound;
use crate::state::Snapshot;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;

/// 每台灯的运行时状态。**不落盘** —— 它描述的是「这一刻灯上是什么颜色」，
/// 重启之后重新发一次即可（第一次一定发，见 [`sync`]）。
#[derive(Default)]
pub struct DeviceRuntime {
    /// 上一次**成功**发出去的颜色。`None` = 还没成功过（新加的灯、刚改过地址）。
    sent: Option<String>,
    /// 有没有一条重试链正在跑。防止每次状态推送都叠一条。
    retrying: bool,
}

/// 按**地址**索引，理由和钩子的日志一样：列表可以增删，下标会错位。
pub type SharedDevices = Arc<Mutex<HashMap<String, DeviceRuntime>>>;

/// 失败后的重试节奏（毫秒），第 `n` 次重试用第 `n` 个。
///
/// **有限次**：灯拔了之后不该永远敲它 —— 那会把日志刷满、也把「最近一次结果」
/// 变成一片红色。合计约 112 秒，够覆盖「灯刚重启」「路由器刚醒」这两类。
/// 用完还不成功就等下一次状态推送（那时会重开一条链）。
const RETRY_DELAYS_MS: [u64; 5] = [2_000, 5_000, 15_000, 30_000, 60_000];

/// 颜色过渡时长（WLED 的 `transition`，单位 100ms）。
///
/// 700ms：灯跟状态走的时候硬切很扎眼，而再长就会出现「已经批准了、灯还是红的」
/// 那种滞后。
const TRANSITION: u8 = 7;

/// 单次请求的上限。灯在局域网里，超过这个时间基本就是不可达。
const TIMEOUT: Duration = Duration::from_secs(4);

/// 把当前聚合的状态色同步到所有配好的灯上。**不阻塞**，随时可以调。
///
/// 每次快照推送都会调它（`notify` 那条路），所以这里必须便宜：算一个色号、锁一下、
/// 比对，没变就直接返回。
pub fn sync(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) {
    let color = settings.state_color(snapshot.aggregate.name()).to_string();
    let runtime = app.state::<SharedDevices>();
    let mut start = Vec::new();

    {
        let mut guard = runtime.lock().expect("device state poisoned");
        // 已经删掉的灯把它的记录一起清掉：不清的话，用户改回同一个地址时会因为
        // 「上次发过这个颜色」而跳过 —— 表现为「改回来了却不亮」。
        guard.retain(|host, _| settings.devices.iter().any(|device| &device.host == host));
        for device in &settings.devices {
            // 空地址 = 「刚点了添加、还没填」，不是一盏灯（和钩子的空目标一样）
            if device.host.trim().is_empty() {
                continue;
            }
            let entry = guard.entry(device.host.clone()).or_default();
            // 已经发过这个颜色 / 已经有一条链在跑（它自己会读到最新状态）
            if entry.sent.as_deref() == Some(color.as_str()) || entry.retrying {
                continue;
            }
            entry.retrying = true;
            start.push(device.clone());
        }
    }

    for device in start {
        attempt(app.clone(), device, 0);
    }
}

/// 发一次（或重试一次）。`attempt` 是第几次，0 = 立刻发。
///
/// **每次都重新读一遍「现在该是什么颜色」**：重试链可能跑了两分钟，这中间状态早
/// 就变了 —— 发一个过期颜色等于把灯钉在旧状态上。同理，灯可能已经被删了。
fn attempt(app: tauri::AppHandle, device: Device, round: usize) {
    tauri::async_runtime::spawn(async move {
        if let Some(delay) = round.checked_sub(1).and_then(|i| RETRY_DELAYS_MS.get(i)) {
            tokio::time::sleep(Duration::from_millis(*delay)).await;
        }

        let settings = app
            .state::<crate::SharedSettings>()
            .lock()
            .expect("settings poisoned")
            .clone();

        // 这一台还在配置里吗（地址可能刚被改掉 / 删掉）
        if !settings.devices.iter().any(|now| now.host == device.host) {
            finish(&app, &device.host, None);
            return;
        }
        let color = settings.state_color(
            app.state::<crate::SharedStore>()
                .lock()
                .expect("state store poisoned")
                .snapshot()
                .aggregate
                .name(),
        )
        .to_string();

        if is_sent(&app, &device.host, &color) {
            // 已经是我们想发的颜色了（另一条链刚发完）—— 收工，但别把记录改掉
            finish(&app, &device.host, None);
            return;
        }

        let url = url(&device.host);
        let payload = body(&color);
        let target = target(&device.host);
        let sent = tokio::task::spawn_blocking(move || post(&url, &payload)).await;

        let (ok, detail) = match sent {
            Ok(Ok(detail)) => (true, detail),
            Ok(Err(err)) => (false, err),
            // spawn_blocking 自己被取消（应用正在退出）—— 不值得记一笔
            Err(err) => (false, err.to_string()),
        };

        outbound::record(
            app.state::<crate::SharedOutbound>().inner(),
            target,
            ok,
            &detail,
        );

        if ok {
            finish(&app, &device.host, Some(color));
            return;
        }

        let next = round + 1;
        if next <= RETRY_DELAYS_MS.len() {
            attempt(app.clone(), device, next);
        } else {
            // 这一轮放弃了。**不把颜色记成已发** —— 下一次状态推送会重开一条链，
            // 那正是「灯后来又能连上了」的恢复路径。
            finish(&app, &device.host, None);
        }
    });
}

/// 这条链收工：清掉「正在重试」，成功时把**真正发出去的那个**颜色记下来。
///
/// 记的是发出去的那个，不是「现在该是什么颜色」—— 两者在重试链跑的这两分钟里
/// 可能已经不一样了，记错的话灯会停在旧颜色上而且再也不会被纠正。
fn finish(app: &tauri::AppHandle, host: &str, sent: Option<String>) {
    let runtime = app.state::<SharedDevices>();
    let mut guard = runtime.lock().expect("device state poisoned");
    if let Some(entry) = guard.get_mut(host) {
        entry.retrying = false;
        if sent.is_some() {
            entry.sent = sent;
        }
    }
}

/// 这台灯上是不是已经是我们想发的那个颜色了。
fn is_sent(app: &tauri::AppHandle, host: &str, color: &str) -> bool {
    let runtime = app.state::<SharedDevices>();
    let guard = runtime.lock().expect("device state poisoned");
    guard.get(host).and_then(|entry| entry.sent.as_deref()) == Some(color)
}

/// 灯的地址 → 真正要 POST 的 URL。
///
/// 认三种写法：`192.168.1.50`、`wled.local:8080`、`http://192.168.1.50`。
/// 顺手把粘进来的 `/json/state` 去掉 —— WLED 的文档里到处是这个路径，而
/// 「粘了文档里的地址」是最常见的填法。
pub fn url(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    let host = host.strip_suffix("/json/state").unwrap_or(host);
    let base = if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("http://{host}")
    };
    format!("{}/json/state", base.trim_end_matches('/'))
}

/// 日志和界面里认一台灯用的名字。**用原始地址**，不带拼出来的 scheme / 路径 ——
/// 前端要拿同一句话去对「最近一次结果」（见 `outbound::Attempt::target`）。
pub fn target(host: &str) -> String {
    format!("WLED {}", host.trim())
}

/// 发给 WLED 的 body。
///
/// 只有颜色：
///
/// - **三个颜色槽都设成同一个色**。用哪个槽取决于当前效果（`fx`），只填第一个的话，
///   某些效果下会露出旧颜色。
/// - **`on: true`**。灯亮着是它存在的理由 —— 用户把它关掉多半是想关灯，而不是想
///   关掉通知。真要停掉这盏灯，把这条配置删了（或者把 WLED 的亮度调到 0）。
/// - **不动 `bri`**：亮度是用户在 WLED 那边调好的，我们没理由改它。
/// - **「没动静」也是灰色，不是关灯**：灯亮着本身就是「这条通路还通」的证据，
///   而关灯和「应用挂了」从外面看一模一样。
pub fn body(color: &str) -> String {
    let (r, g, b) = rgb(color).unwrap_or((0x9c, 0xa3, 0xaf));
    serde_json::json!({
        "on": true,
        "transition": TRANSITION,
        "seg": [{ "col": [[r, g, b], [r, g, b], [r, g, b]] }],
    })
    .to_string()
}

/// `#rrggbb` / `#rgb` → RGB。**认不出来就是 `None`**，由调用方决定兜底。
pub fn rgb(color: &str) -> Option<(u8, u8, u8)> {
    let hex = color.trim().trim_start_matches('#');
    let expand = |c: &str| u8::from_str_radix(c, 16).ok();
    match hex.len() {
        3 => {
            let mut it = hex.chars();
            let (r, g, b) = (it.next()?, it.next()?, it.next()?);
            let pair = |c: char| {
                let mut s = String::new();
                s.push(c);
                s.push(c);
                expand(&s)
            };
            Some((pair(r)?, pair(g)?, pair(b)?))
        }
        6 => Some((
            expand(&hex[0..2])?,
            expand(&hex[2..4])?,
            expand(&hex[4..6])?,
        )),
        _ => None,
    }
}

/// 发一次 POST，返回日志里那句「成功是什么样」。
///
/// **200 不等于成功了。** 地址敲错一个数字，答话的可能是一台路由器或者别的什么
/// HTTP 服务 —— 它照样回 200，而灯一个像素都没动，界面上却写着绿色的 `ok 200`。
/// WLED 对一次 `POST /json/state` 的应答是 `{"success":true}`（见官方 JSON API
/// 文档），所以这里认它。认不出来就把响应体的开头写进日志 —— 「它到底答了什么」
/// 正是用户需要看到的。
fn post(url: &str, body: &str) -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_per_call(Some(TIMEOUT))
        .build();

    let mut response = ureq::Agent::new_with_config(config)
        .post(url)
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|err| err.to_string())?;

    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(512)
        .lossy_utf8(true)
        .read_to_string()
        .unwrap_or_default();

    let accepted = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("success").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);

    if accepted {
        Ok(format!("{status} success"))
    } else {
        Err(format!(
            "{status} 但不是 WLED 的应答（{}）—— 地址填对了吗",
            text.trim().chars().take(80).collect::<String>()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三种写法都认，而且粘文档里的地址也认。
    #[test]
    fn the_host_is_turned_into_the_json_endpoint() {
        assert_eq!(url("192.168.1.50"), "http://192.168.1.50/json/state");
        assert_eq!(url(" wled.local:8080 "), "http://wled.local:8080/json/state");
        assert_eq!(url("http://192.168.1.50"), "http://192.168.1.50/json/state");
        assert_eq!(url("https://wled.example/"), "https://wled.example/json/state");
        // 从 WLED 文档里直接粘过来的那种
        assert_eq!(
            url("http://192.168.1.50/json/state"),
            "http://192.168.1.50/json/state"
        );
    }

    /// 发出去的东西里**只有颜色**：没有组名、没有会话名、没有消息（红线 3）。
    ///
    /// 这条用例是那条红线的把手 —— 以后谁想「顺手把消息也发过去」，先得改这里。
    #[test]
    fn only_the_colour_leaves_the_machine() {
        let body = body("#16a34a");
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["on"], serde_json::json!(true));
        assert_eq!(value["seg"][0]["col"][0], serde_json::json!([22, 163, 74]));
        // 三个颜色槽都是同一个色：用哪个槽取决于当前效果
        assert_eq!(value["seg"][0]["col"].as_array().unwrap().len(), 3);
        assert_eq!(value["seg"][0]["col"][2], serde_json::json!([22, 163, 74]));

        let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["on", "seg", "transition"], "body 里多出了别的字段");
    }

    /// 认不出颜色时不发一个瞎编的颜色，而是兜到「没动静」那个灰。
    #[test]
    fn a_colour_that_makes_no_sense_falls_back_to_grey() {
        assert_eq!(rgb("#16a34a"), Some((0x16, 0xa3, 0x4a)));
        assert_eq!(rgb("#fff"), Some((255, 255, 255)));
        assert_eq!(rgb("16A34A"), Some((0x16, 0xa3, 0x4a)));
        assert_eq!(rgb("#12345"), None);
        assert_eq!(rgb(""), None);
        assert!(body("").contains("156,163,175"), "该兜到灰：{}", body(""));
    }

    /// 日志和界面对一台灯的称呼是同一句话。
    #[test]
    fn the_target_is_the_host_the_user_typed() {
        assert_eq!(target(" 192.168.1.50 "), "WLED 192.168.1.50");
    }

    /// 空地址是「还没填」，不是错 —— 编辑时必然经过这个中间态。
    #[test]
    fn an_empty_host_is_not_a_problem_but_a_paste_accident_is() {
        let empty = Device { host: "  ".into() };
        assert!(crate::config::device_problem(&empty).is_none());
        let fine = Device { host: "wled.local".into() };
        assert!(crate::config::device_problem(&fine).is_none());
        let pasted = Device { host: "192.168.1.50 的灯".into() };
        assert!(crate::config::device_problem(&pasted).is_some());
    }
}
