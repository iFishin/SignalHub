//! 在桌面进程里运行一个**仅供本机使用**的 Call Me 服务。
//!
//! 远程 VPS 仍运行 `server/` 的独立二进制；这里复用的是它的 library，不在服务器
//! crate 里反向依赖 Tauri。两条部署路径因此共用路由、存储、限流与鉴权，不会出现
//! 「本机测试一套行为、公网又是另一套」。
//!
//! 刻意不监听 `0.0.0.0`：桌面应用不是公网网关。外部智能体要访问时，用户必须明确
//! 配置公网反代 / 隧道，或把服务部署到 VPS。任务由 tokio 托管，应用退出即停止，
//! 不留下脱离应用生命周期的孤儿进程。

use serde::{Deserialize, Serialize};
use signalhub_callme::{router, AppState, Limits, Store};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

pub const DEFAULT_PORT: u16 = 43877;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(default)]
pub struct LocalStatus {
    pub running: bool,
    pub url: Option<String>,
    pub token: Option<String>,
    pub port: u16,
    pub data_dir: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Runtime {
    task: Option<JoinHandle<()>>,
    status: LocalStatus,
}

pub type SharedLocal = Arc<Mutex<Runtime>>;

pub fn state() -> SharedLocal {
    Arc::new(Mutex::new(Runtime::default()))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct LocalConfig {
    token: String,
    port: u16,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            port: DEFAULT_PORT,
        }
    }
}

fn config_path() -> Result<PathBuf, String> {
    crate::config::data_dir()
        .map(|dir| dir.join("callme-local.json"))
        .map_err(|err| format!("找不到本机 Call Me 配置目录：{err}"))
}

fn data_dir() -> Result<PathBuf, String> {
    crate::config::data_dir()
        .map(|dir| dir.join("callme-server"))
        .map_err(|err| format!("找不到本机 Call Me 数据目录：{err}"))
}

fn load_config(path: &Path) -> LocalConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<LocalConfig>(&raw).ok())
        .filter(|config| config.token.chars().count() >= 16)
        .unwrap_or_default()
}

fn save_config(path: &Path, config: &LocalConfig) -> Result<(), String> {
    let body = serde_json::to_string_pretty(config).map_err(|err| err.to_string())?;
    crate::config::atomic_write(path, &body, Some(0o600))
        .map_err(|err| format!("保存本机 Call Me 配置失败：{err}"))
}

fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|err| format!("生成管理 token 失败：{err}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn load_or_create_config(path: &Path, port: u16) -> Result<LocalConfig, String> {
    let mut config = load_config(path);
    let mut changed = false;
    if config.token.is_empty() {
        config.token = new_token()?;
        changed = true;
    }
    if config.port != port {
        config.port = port;
        changed = true;
    }
    if changed || !path.exists() {
        save_config(path, &config)?;
    }
    Ok(config)
}

/// 读取当前状态。已退出任务的 JoinHandle 在这里回收，不另起轮询线程。
pub fn status(shared: &SharedLocal) -> LocalStatus {
    let mut runtime = shared.lock().expect("local callme poisoned");
    if runtime.task.as_ref().is_some_and(JoinHandle::is_finished) {
        runtime.task = None;
        runtime.status.running = false;
    }
    if runtime.status.port == 0 {
        runtime.status.port = DEFAULT_PORT;
    }
    runtime.status.clone()
}

pub async fn start(shared: SharedLocal, port: u16) -> Result<LocalStatus, String> {
    if !(1024..=u16::MAX).contains(&port) {
        return Err("端口要在 1024–65535 之间".to_string());
    }
    start_in(shared, &config_path()?, &data_dir()?, port).await
}

async fn start_in(
    shared: SharedLocal,
    config_path: &Path,
    data_dir: &Path,
    port: u16,
) -> Result<LocalStatus, String> {
    {
        let runtime = shared.lock().expect("local callme poisoned");
        if runtime.task.as_ref().is_some_and(|task| !task.is_finished()) {
            return Ok(runtime.status.clone());
        }
    }

    let config = load_or_create_config(config_path, port)?;
    let store = Store::open(data_dir, Limits::default())
        .map_err(|err| format!("打开本机 Call Me 数据失败：{err}"))?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|err| format!("本机端口 {port} 启动失败：{err}。换个端口再试"))?;
    let address = listener
        .local_addr()
        .map_err(|err| format!("读取本机监听地址失败：{err}"))?;
    let url = format!("http://{address}");
    let app_state = AppState {
        store: Arc::new(Mutex::new(store)),
        admin_token: config.token.clone(),
        trust_proxy: false,
    };

    {
        let mut runtime = shared.lock().expect("local callme poisoned");
        runtime.status = LocalStatus {
            running: true,
            url: Some(url),
            token: Some(config.token),
            port: address.port(),
            data_dir: Some(data_dir.to_string_lossy().into_owned()),
            error: None,
        };
    }

    let task_shared = Arc::clone(&shared);
    let task = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            router(app_state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
        let mut runtime = task_shared.lock().expect("local callme poisoned");
        runtime.status.running = false;
        if let Err(err) = result {
            runtime.status.error = Some(format!("本机 Call Me 服务已停止：{err}"));
        }
    });
    shared.lock().expect("local callme poisoned").task = Some(task);
    Ok(status(&shared))
}

pub fn stop(shared: &SharedLocal) -> LocalStatus {
    let mut runtime = shared.lock().expect("local callme poisoned");
    if let Some(task) = runtime.task.take() {
        task.abort();
    }
    runtime.status.running = false;
    runtime.status.error = None;
    runtime.status.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "signalhub-callme-local-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn local_config_is_private_and_reused() {
        let dir = temp_dir("config");
        let path = dir.join("callme-local.json");
        let first = load_or_create_config(&path, 45678).unwrap();
        assert_eq!(first.token.len(), 48);
        assert_eq!(first.port, 45678);
        let second = load_or_create_config(&path, 45678).unwrap();
        assert_eq!(first.token, second.token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 起真服务、真发一个请求、再停掉。
    ///
    /// **必须 `multi_thread`**：`#[tokio::test]` 默认是单线程运行时，而 `axum::serve`
    /// 是 `tokio::spawn` 出来的任务 —— 测试线程里用阻塞的 `ureq` 发请求会把唯一的
    /// 执行线程占住，服务任务永远没机会被 poll，于是一起等下去（本仓踩过一次：
    /// 这个用例挂了一个多小时，`cargo test` 既不失败也不退出）。
    /// 另外给 client 配了超时：即便将来又写出同类的死锁，也是 5 秒后失败，不是挂死。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_server_starts_answers_and_stops() {
        let dir = temp_dir("serve");
        let shared = state();
        let started = start_in(
            shared.clone(),
            &dir.join("local.json"),
            &dir.join("data"),
            0,
        )
        .await
        .unwrap();
        assert!(started.running);
        assert_ne!(started.port, 0);
        let url = format!("{}/v1/callme/health", started.url.unwrap());
        let client = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(3)))
                .timeout_per_call(Some(Duration::from_secs(5)))
                .build(),
        );
        let body = client
            .get(&url)
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("\"ok\":true"));
        assert!(!stop(&shared).running);
        let _ = std::fs::remove_dir_all(dir);
    }
}
