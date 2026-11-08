//! 配置：全部来自环境变量（12-factor，Docker 友好）。
//!
//! 不提供配置文件：这个服务的配置项一只手数得过来，而配置文件会引出
//! 「文件在哪、谁有权限、热加载还是重启」一串问题。环境变量在 docker run /
//! compose / systemd 里都是一行。
//!
//! **启动时打印生效值**（`main.rs` 里），因为「我明明改了环境变量但它还是
//! 老行为」是这类服务最常见的求助。

use crate::store::Limits;
use std::collections::HashMap;
use std::path::PathBuf;

/// 一次运行的全部参数。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `IP:端口`。默认 `0.0.0.0:8080`（容器里必须绑 0.0.0.0，否则宿主机转发不进来）。
    pub listen: String,
    /// 客户端拉取 / 管理来源用的 Bearer token。**必填**。
    pub admin_token: String,
    pub data_dir: PathBuf,
    pub limits: Limits,
    /// 反代后面置 true，才信任 `X-Forwarded-For`。
    pub trust_proxy: bool,
}

impl Config {
    /// 从进程环境读。
    pub fn from_env() -> Result<Config, String> {
        Config::from_pairs(std::env::vars())
    }

    /// 纯函数版本：给测试用，不碰全局环境（并行测试改 `env::set_var` 会互相踩）。
    pub fn from_pairs<I, K, V>(pairs: I) -> Result<Config, String>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let map: HashMap<String, String> = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let admin_token = get(&map, "CALLME_ADMIN_TOKEN").unwrap_or_default();
        if admin_token.is_empty() {
            return Err(
                "没有设置 CALLME_ADMIN_TOKEN。它是客户端拉取留言用的凭证，公网服务不能裸奔 —— \
                 用 `openssl rand -hex 24` 生成一个，客户端里填同样的值。"
                    .to_string(),
            );
        }
        if admin_token.chars().count() < 16 {
            return Err(format!(
                "CALLME_ADMIN_TOKEN 太短（{} 个字符）。至少 16 个，建议 `openssl rand -hex 24`。",
                admin_token.chars().count()
            ));
        }

        Ok(Config {
            listen: get(&map, "CALLME_LISTEN").unwrap_or_else(|| "0.0.0.0:8080".to_string()),
            admin_token,
            data_dir: PathBuf::from(
                get(&map, "CALLME_DATA_DIR").unwrap_or_else(|| "./data".to_string()),
            ),
            limits: Limits {
                ip_hourly: num(&map, "CALLME_IP_HOURLY", 10, 1, 10_000)? as u32,
                source_daily: num(&map, "CALLME_SOURCE_DAILY", 50, 1, 100_000)? as u32,
                max_text_chars: num(&map, "CALLME_MAX_TEXT", 4_000, 100, 100_000)? as usize,
                retention_days: num(&map, "CALLME_RETENTION_DAYS", 90, 1, 3_650)?,
            },
            trust_proxy: flag(&map, "CALLME_TRUST_PROXY"),
        })
    }
}

fn get(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn num(
    map: &HashMap<String, String>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let Some(raw) = get(map, key) else {
        return Ok(default);
    };
    let value: u64 = raw
        .parse()
        .map_err(|_| format!("{key} 不是数字：{raw:?}"))?;
    if value < min || value > max {
        return Err(format!("{key} 要在 {min}..={max} 之间，现在是 {value}"));
    }
    Ok(value)
}

fn flag(map: &HashMap<String, String>, key: &str) -> bool {
    matches!(
        get(map, key).map(|v| v.to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Vec<(String, String)> {
        vec![(
            "CALLME_ADMIN_TOKEN".to_string(),
            "0123456789abcdef01234567".to_string(),
        )]
    }

    #[test]
    fn defaults_are_sane() {
        let config = Config::from_pairs(base()).expect("应当能解析");
        assert_eq!(config.listen, "0.0.0.0:8080");
        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.limits.ip_hourly, 10);
        assert_eq!(config.limits.source_daily, 50);
        assert_eq!(config.limits.max_text_chars, 4_000);
        assert_eq!(config.limits.retention_days, 90);
        assert!(!config.trust_proxy);
    }

    #[test]
    fn missing_admin_token_is_refused() {
        let err = Config::from_pairs(Vec::<(String, String)>::new()).unwrap_err();
        assert!(err.contains("CALLME_ADMIN_TOKEN"), "{err}");
    }

    #[test]
    fn short_admin_token_is_refused() {
        let err = Config::from_pairs(vec![("CALLME_ADMIN_TOKEN", "short")]).unwrap_err();
        assert!(err.contains("太短"), "{err}");
    }

    #[test]
    fn numbers_are_overridable_and_validated() {
        let config = Config::from_pairs(vec![
            ("CALLME_ADMIN_TOKEN", "0123456789abcdef01234567"),
            ("CALLME_IP_HOURLY", "3"),
            ("CALLME_SOURCE_DAILY", "7"),
            ("CALLME_MAX_TEXT", "500"),
            ("CALLME_RETENTION_DAYS", "14"),
        ])
        .unwrap();
        assert_eq!(config.limits.ip_hourly, 3);
        assert_eq!(config.limits.source_daily, 7);
        assert_eq!(config.limits.max_text_chars, 500);
        assert_eq!(config.limits.retention_days, 14);

        let err = Config::from_pairs(vec![
            ("CALLME_ADMIN_TOKEN", "0123456789abcdef01234567"),
            ("CALLME_IP_HOURLY", "0"),
        ])
        .unwrap_err();
        assert!(err.contains("CALLME_IP_HOURLY"), "{err}");

        let err = Config::from_pairs(vec![
            ("CALLME_ADMIN_TOKEN", "0123456789abcdef01234567"),
            ("CALLME_SOURCE_DAILY", "abc"),
        ])
        .unwrap_err();
        assert!(err.contains("不是数字"), "{err}");
    }

    #[test]
    fn trust_proxy_flag() {
        for value in ["1", "true", "yes", "on", "TRUE"] {
            let config = Config::from_pairs(vec![
                ("CALLME_ADMIN_TOKEN", "0123456789abcdef01234567"),
                ("CALLME_TRUST_PROXY", value),
            ])
            .unwrap();
            assert!(config.trust_proxy, "{value} 应当为真");
        }
        for value in ["0", "false", "no", ""] {
            let config = Config::from_pairs(vec![
                ("CALLME_ADMIN_TOKEN", "0123456789abcdef01234567"),
                ("CALLME_TRUST_PROXY", value),
            ])
            .unwrap();
            assert!(!config.trust_proxy, "{value} 应当为假");
        }
    }
}
