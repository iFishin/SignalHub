//! 自动更新。
//!
//! **只在 macOS 上真正生效**（判据是 [`SUPPORTED`]）。原因分两半，一半在上游、
//! 一半在「没验过」：
//!
//! - **上游**：插件装更新要用 `zip`，而它要求的 `zip = "^4"` 被 crates.io 全量 yank
//!   了（见 `Cargo.toml` 的依赖注释）—— Windows 那条路因此技术上做不了。
//! - **没验过**：Linux 的 AppImage 那条路其实走得通（更新包是**没压缩**的 ELF，
//!   不吃 `zip` feature），但 deb 装的那份要靠 `dpkg -i` 提权（普通用户跑不动），
//!   而两条路都没在真机上验过。**在能动真机验收之前不打开** —— 好过一个「点了报
//!   权限错」的按钮。
//!
//! 于是 Linux / Windows 上返回 `supported: false`，界面维持「手动下载」并如实说明。
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

use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// 检查结果变了就推这个事件（前端 `useUpdate.ts` 里的字面量必须与之一致）
pub const UPDATE_EVENT: &str = "signalhub://update";

/// 下载进度事件名。同上的契约。
pub const PROGRESS_EVENT: &str = "signalhub://update-progress";

/// 这个平台能不能自动更新。**只有 macOS** —— 理由（上游的 zip + Linux 没验过）
/// 见模块头。
///
/// 要打开 Linux：`cfg!(any(target_os = "macos", target_os = "linux"))`，但先把
/// AppImage / deb 两条路在真机上跑通（AppImage 要能替换正在跑的那个文件，deb 要
/// 处理 `dpkg -i` 的权限）。
pub const SUPPORTED: bool = cfg!(target_os = "macos");

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

/// 查一次，**网络错误原样返回**。由调用方决定是记日志还是摆给用户看（见模块头）。
async fn query(app: &AppHandle) -> Result<Option<Available>, String> {
    let updater = app.updater().map_err(|err| err.to_string())?;
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
    state.supported = SUPPORTED;
}

/// 查一次并写进共享状态，顺手推给界面。
///
/// `silent` = 自动那条路（失败只留日志、不写进 `error`）。
async fn refresh(app: &AppHandle, silent: bool) -> UpdateState {
    let shared = app.state::<SharedUpdate>();
    if !SUPPORTED {
        // 不支持的平台上连网都不必碰：界面只会看到 `supported: false`
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
    if !SUPPORTED {
        // 界面不该显示这个按钮；真被调到就说清楚为什么，不要装作在装
        return Err("这个平台暂不支持自动更新，请到发布页手动下载".to_string());
    }

    let updater = app.updater().map_err(|err| err.to_string())?;
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
/// 只要 macOS；别的平台连任务都不起。整条路是**静默**的（见模块头的第 2 条）。
pub fn spawn_auto_check(app: AppHandle) {
    if !SUPPORTED {
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
