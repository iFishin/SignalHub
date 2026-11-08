//! 入口：读环境变量 → 打开数据目录 → 起 HTTP 服务 → 等 Ctrl-C / SIGTERM。
//!
//! 刻意薄：所有能测的逻辑都在 `store.rs` / `http.rs` / `config.rs` 里。这里是
//! 唯一碰进程全局状态（信号、绑定端口、退出码）的地方。

use signalhub_callme::{router, AppState, Config, Store};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("启动失败：{message}");
            return ExitCode::from(2);
        }
    };

    let store = match Store::open(&config.data_dir, config.limits) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "启动失败：打不开数据目录 {}：{err}",
                config.data_dir.display()
            );
            return ExitCode::from(2);
        }
    };
    let data_dir = config.data_dir.display().to_string();
    let app = router(AppState {
        store: Arc::new(Mutex::new(store)),
        admin_token: config.admin_token.clone(),
        trust_proxy: config.trust_proxy,
    });

    let listener = match tokio::net::TcpListener::bind(&config.listen).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("启动失败：绑不上 {}：{err}", config.listen);
            return ExitCode::from(2);
        }
    };
    let local = listener.local_addr().ok();

    // 生效值打出来，但 **token 只报长度**：这类服务的日志经常被 docker logs
    // 转发出去，凭证不该出现在里面。
    eprintln!(
        "signalhub-callme {} 监听 {}（数据目录 {data_dir}，admin token {} 字符）",
        env!("CARGO_PKG_VERSION"),
        local
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| config.listen.clone()),
        config.admin_token.chars().count(),
    );
    eprintln!(
        "限流：每 IP {} 条/时 · 每链接 {} 条/24h · 正文上限 {} 字 · 保留 {} 天",
        config.limits.ip_hourly,
        config.limits.source_daily,
        config.limits.max_text_chars,
        config.limits.retention_days
    );

    let result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("服务异常退出：{err}");
            ExitCode::FAILURE
        }
    }
}

/// Ctrl-C 或 SIGTERM（docker stop / systemd）都算优雅停机。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => eprintln!("装不上 SIGTERM 处理器（{err}），只能等 Ctrl-C"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    eprintln!("收到停止信号，退出");
}
