//! 自动更新。
//!
//! **macOS 与 Windows 的 NSIS 安装版**支持自动更新（判据是 [`supported`]）。Linux
//! 与 Windows 的 portable 版返回 `supported: false`，界面维持「手动下载」并如实说明。
//!
//! - **Windows**：更新包是**裸 `setup.exe`**（见 `latest.json` 的 `windows-x86_64`），
//!   插件的 `extract_exe` 路径直接认 PE 文件、**不吃 `zip` feature** —— 所以 `Cargo.toml`
//!   里 updater 仍用 `default-features = false`，没把那个历史上的 yank 依赖请回来。
//!   安装器由 NSIS 打出来，updater 调起它替换自己；**portable 是解压即用的裸包，没有
//!   安装器身份**，判据是 `bundle_type()`：NSIS 安装版返回 `Some(Nsis)`，portable 是
//!   `None`（见 `tauri_utils::platform::bundle_type`）。给 portable 显示一个点了没反应的
//!   「安装」按钮没有意义，宁可明说手动下载。
//! - **Linux**：AppImage 那条路走得通（更新包是没压缩的 ELF），但 deb 那份要靠
//!   `dpkg -i` 提权，两条路都没在真机验过。**在能动真机验收之前不打开**。
//!
//! **自定义更新源 / 代理**（`Settings::update_endpoint` / `update_proxy` /
//! `update_no_proxy`）：内网连不上 GitHub、或者公司给了更新镜像时用。它们只作用于
//! 更新请求，经 `updater_builder()` 逐次构建；**公钥仍是编译进去的那把**，指向镜像
//! 不会让出验签这道门。三项全空 / false 就是加它们之前的行为。
//!
//! 三条边界（是产品判断，不是实现细节）：
//!
//! 1. **默认不自动装。** 检查可以自动，装必须由用户点。常驻应用悄悄换掉自己的
//!    二进制是用户不该遭遇的事 —— 何况这个包没有 Apple 签名。
//! 2. **自动检查失败是静默的。** 它是常驻应用，不能因为 GitHub 抽风就弹一个错误。
//!    失败只写 `hooks.log`，失败原因也**不**记进给界面看的状态里。只有用户**主动**
//!    点「检查更新」时才把失败摆出来 —— 那正是他要的答案。
//! 3. **验签用的是 Tauri 的密钥对**（`tauri.conf.json` 里的公钥），和 Apple 证书
//!    是两套完全独立的东西。校验的是「这个包确实是我们发的」，不是「Apple 认识我们」。

use crate::config::Settings;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Updater, UpdaterExt};
use url::Url;

/// 检查结果变了就推这个事件（前端 `useUpdate.ts` 里的字面量必须与之一致）
pub const UPDATE_EVENT: &str = "signalhub://update";

/// 下载进度事件名。同上的契约。
pub const PROGRESS_EVENT: &str = "signalhub://update-progress";

/// 这个平台 / 这份安装能不能自动更新。判据见模块头。
///
/// 不是常量：Windows 上 portable（裸包）和安装版（NSIS）是同一份二进制，只能靠
/// `bundle_type()` 在运行时区分。要打开 Linux：先看模块头那两条真机验收。
pub fn supported() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        // `Some(Nsis)` = 安装版；`None` = portable 裸包（见模块头）
        return matches!(
            tauri::utils::platform::bundle_type(),
            Some(tauri::utils::config::BundleType::Nsis)
        );
    }
    #[allow(unreachable_code)]
    false
}

/// 为什么这份安装不能用自动更新 —— 摆到界面上，而不是只显示一句「不支持」。
/// portable 那条尤其要说清「换安装版就能用」，否则用户会以为功能整个不存在。
pub fn unsupported_reason() -> &'static str {
    #[cfg(target_os = "windows")]
    if !supported() {
        return "免安装版（portable）没有安装器身份，无法自动替换自己。请到发布页下载 -setup.exe 安装版，之后即可在设置里一键更新。";
    }
    #[cfg(target_os = "linux")]
    {
        let _ = supported();
        return "Linux 的自动更新还没在真机上验收过（deb 需要管理员权限）。请到发布页手动下载 AppImage 或 deb。";
    }
    #[allow(unreachable_code)]
    ""
}

/// 启动后多久查第一次。
///
/// 不立刻查：启动那几秒是用户在盯着灯的时候，不该跟首帧抢资源，也不该让一个
/// 网络请求挡在前面。十几秒足够错开。
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);

/// 之后每多久查一次。24 小时 —— 更新不是实时信息，一天一次足够。
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 检查结果。**全是纯数据**，可以直接序列化给前端。
///
/// 刻意不把插件的 `Update` 存进来：那个对象带着 HTTP 客户端和一堆内部状态，跨命令
/// 共享是自找麻烦。装的时候重新查一次 —— 那本来也更对：装的是**此刻**的最新版，
/// 而不是十分钟前看到的那一版。
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateState {
    /// 这个平台能不能自动更新；false 时界面只说一句「请手动下载」
    pub supported: bool,
    /// 当前版本，例如 `0.2.4`
    pub current: String,
    /// 有可装的更新时非空
    pub available: Option<Available>,
    /// 上次检查的时刻（Unix 毫秒）；没查过是 0
    pub checked_at_ms: u64,
    /// 上次**手动**检查的失败原因；成功后清空
    pub error: Option<String>,
    /// `supported: false` 时为什么 —— portable 换安装版就能用，Linux 是没验过。
    /// 支持的平台上是空串。
    pub unsupported_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    /// 发布说明（来自 `latest.json` 的 `notes`，可能为空）
    pub notes: String,
}

/// 下载进度。`installing` 为真时 `downloaded` 已经没有意义了。
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub installing: bool,
}

pub type SharedUpdate = Arc<Mutex<UpdateState>>;

/// 读当前设置里的更新源 / 代理，构造这一次的 updater。
///
/// 每次查 / 装都**重新读设置**而不是缓存：用户在面板里改完端点，下一次「检查更新」
/// 就该走新的。三项为空 / false 时等价于 `app.updater()` —— 插件配置里那条 GitHub
/// 端点 + 系统代理，公网用户的观感和加设置之前一字不差。
///
/// `no_proxy` 优先于 `proxy`（与插件自身的 `else if` 顺序一致），免得「勾了忽略系统
/// 代理却还带着手填的代理」这种自相矛盾的状态。
fn build_updater(app: &AppHandle) -> Result<Updater, String> {
    let (endpoint, proxy, no_proxy) = {
        let guard = app
            .try_state::<Arc<Mutex<Arc<Settings>>>>()
            .ok_or_else(|| "设置状态尚未就绪".to_string())?;
        let settings = guard.lock().expect("settings poisoned");
        (
            settings.update_endpoint.clone(),
            settings.update_proxy.clone(),
            settings.update_no_proxy,
        )
    };

    let mut builder = app.updater_builder();
    if !endpoint.is_empty() {
        let url = Url::parse(&endpoint)
            .map_err(|err| format!("更新源地址不是有效的 URL: {err}"))?;
        builder = builder
            .endpoints(vec![url])
            .map_err(|err| format!("更新源地址不可用: {err}"))?;
    }
    if no_proxy {
        builder = builder.no_proxy();
    } else if !proxy.is_empty() {
        let url =
            Url::parse(&proxy).map_err(|err| format!("代理地址不是有效的 URL: {err}"))?;
        builder = builder.proxy(url);
    }
    builder.build().map_err(|err| err.to_string())
}

/// 查一次，**网络错误原样返回**。由调用方决定是记日志还是摆给用户看（见模块头）。
async fn query(app: &AppHandle) -> Result<Option<Available>, String> {
    let updater = build_updater(app)?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(Available {
            version: update.version.clone(),
            notes: update.body.clone().unwrap_or_default(),
        })),
        Ok(None) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

/// 把**运行时事实**盖到快照上：当前版本、这个平台能不能自动更新。
///
/// 它们不属于「检查结果」—— 一次都没查过、或者这个平台根本不查，界面也照样该报准
/// 版本号。非 macOS 上永远不会走到 `refresh`，只靠它填的话那边会一直空着。
fn stamp(app: &AppHandle, state: &mut UpdateState) {
    state.current = app.package_info().version.to_string();
    state.supported = supported();
    state.unsupported_reason = if state.supported {
        String::new()
    } else {
        unsupported_reason().to_string()
    };
}

/// 查一次并写进共享状态，顺手推给界面。
///
/// `silent` = 自动那条路（失败只留日志、不写进 `error`）。
async fn refresh(app: &AppHandle, silent: bool) -> UpdateState {
    let shared = app.state::<SharedUpdate>();
    if !supported() {
        // 不支持的平台 / 安装版上连网都不必碰：界面只会看到 `supported: false`
        let mut state = UpdateState::default();
        stamp(app, &mut state);
        return state;
    }

    let result = query(app).await;

    let state = {
        let mut state = shared.lock().expect("update state poisoned");
        stamp(app, &mut state);
        state.checked_at_ms = crate::spool::now_ms();
        match result {
            Ok(available) => {
                if available.is_some() {
                    // 有更新才留痕；「已是最新」每天写一行是纯噪音
                    crate::config::append_hook_log(&format!(
                        "{} [update] 有新版本 {}\n",
                        crate::config::utc_hms(),
                        available.as_ref().map(|it| it.version.as_str()).unwrap_or("-"),
                    ));
                }
                // `available` 要**整个换掉**而不是只处理 Some：远端把某个版本撤回过
                // 之后，我们还留着「有新版本」就是在骗人。
                state.available = available;
                state.error = None;
            }
            // 静默那条路：只记日志，不写 `error`。用户没点过「检查更新」，
            // 就不该在我们的界面上看到一条他没法解释的失败。
            Err(err) => {
                crate::config::append_hook_log(&format!(
                    "{} [update] 检查更新失败: {err}\n",
                    crate::config::utc_hms(),
                ));
                if !silent {
                    state.error = Some(err);
                }
            }
        }
        state.clone()
    };

    let _ = app.emit(UPDATE_EVENT, &state);
    state
}

/// 界面打开时读一次（**不联网**）。
#[tauri::command]
pub fn get_update_state(app: AppHandle, state: tauri::State<SharedUpdate>) -> UpdateState {
    let mut snapshot = state.lock().expect("update state poisoned").clone();
    // 版本号和平台能力是运行时事实，不依赖「查过没有」—— 见 `stamp`
    stamp(&app, &mut snapshot);
    snapshot
}

/// 用户点「检查更新」。失败会如实返回 —— 那正是他点它的原因。
#[tauri::command]
pub async fn check_update(app: AppHandle) -> UpdateState {
    refresh(&app, false).await
}

/// 下载并安装，然后重启。
///
/// **只有用户点了才会走到这里**（红线：默认不自动装）。过程中推下载进度。
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    if !supported() {
        // 界面不该显示这个按钮；真被调到就说清楚为什么，不要装作在装
        return Err(unsupported_reason().to_string());
    }

    let updater = build_updater(&app)?;
    // 重新查一次（见 `UpdateState` 的注释）：装的是此刻的最新版
    let update = updater
        .check()
        .await
        .map_err(|err| format!("检查更新失败: {err}"))?
        .ok_or_else(|| "已经是最新版本了".to_string())?;

    let handle = app.clone();
    let progress = move |downloaded: usize, total: Option<u64>| {
        let _ = handle.emit(
            PROGRESS_EVENT,
            Progress { downloaded: downloaded as u64, total, installing: false },
        );
    };
    // 「下载完了，正在装」这一步在 macOS 上要换 bundle，可能是几秒的静默，
    // 界面得知道自己在等什么 —— 否则看起来就是卡住了。
    let handle = app.clone();
    let finished = move || {
        let _ = handle.emit(
            PROGRESS_EVENT,
            Progress { downloaded: 0, total: None, installing: true },
        );
    };

    update
        .download_and_install(progress, finished)
        .await
        // 验签失败、网络中断、写不进 Application 目录 —— 都到这里
        .map_err(|err| format!("安装更新失败: {err}"))?;

    // 不重启的话，跑着的还是旧二进制：用户点了「安装并重启」，结果什么都没变。
    // `restart()` 不会返回。
    app.restart();
}

/// 启动后自动查：先等 [`FIRST_CHECK_DELAY`]，之后每 [`CHECK_INTERVAL`] 一次。
///
/// 只有支持自动更新的平台 / 安装版才起任务。整条路是**静默**的（模块头第 2 条）。
pub fn spawn_auto_check(app: AppHandle) {
    if !supported() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            refresh(&app, true).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}
