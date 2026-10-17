//! 只读 Web 面板（`GET /`）：手机 / 平板打开一个链接就能看灯。
//!
//! **它只是一个静态模板 + 一次注入**。数据不在这里 —— 页面里的 JS 去拉
//! `/v1/state`（那条路要 token，token 由打开的人写在 URL 上：`/?t=<token>`）。
//! 所以这个路由**不需要鉴权、也不泄密**：拿不到 token 的人看到的是一个空壳。
//!
//! 注进去的三样东西都是**既有事实的搬运**，不新增第二份真值：
//!
//! - **状态名**：`lang::signal_label` —— 和系统通知用同一张表，所以手机上说的
//!   和通知里说的一样（用户偏好 `auto` 时前端解析好推给 Rust，见 `lang.rs`）。
//! - **颜色**：`Settings::state_color` —— Rust 侧唯一的「状态 → 颜色」入口，
//!   物理灯（WLED）也从那里取。它按「用户逐个状态改过的颜色 > 配色预设」解析，
//!   和灯上一致（见 `palettes.ts` 的 `resolveColors`）。
//! - **组显示名**：`group_meta` 里用户改过的名字，和灯上一样。
//!
//! 页面模板在同目录的 `panel.html`，编译期嵌进二进制（`include_str!`）——
//! 运行时不读盘，也不需要额外的构建步骤。

use crate::config::{Lang, Settings};
use crate::state::Signal;

/// 页面模板。
const TEMPLATE: &str = include_str!("panel.html");

/// 要发给浏览器的整页。
pub fn page(settings: &Settings, lang: Lang) -> String {
    let config = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "colors": state_colors(settings),
        "labels": labels(lang),
        "groups": group_labels(settings),
        "refresh_ms": REFRESH_MS,
    });

    // **`<` 必须转义掉。** 颜色和组名是用户可写的字符串（`config.json` 能手改），
    // 而这段 JSON 是嵌在 `<script>` 里的 —— 一个 `</script>` 就能从数据里跳出来
    // 变成代码。`\u003c` 在 JSON 里还是 `<`，只是不再能拼出那个闭合标签。
    let config = serde_json::to_string(&config)
        .unwrap_or_else(|_| "{}".to_string())
        .replace('<', "\\u003c");

    TEMPLATE.replace("__CONFIG__", &config)
}

/// 页面自己的刷新间隔。
///
/// 1 秒：这是**看**的东西，慢半拍就没意义了；而一次快照是几 KB 的本地请求，
/// 一台手机看一整天也不算什么。刻意不做 SSE / WebSocket —— 那要在这个只读页面上
/// 加一条长连接和它的重连逻辑，而收益只是省下每秒钟一次几 KB 的轮询。
const REFRESH_MS: u64 = 1000;

/// 每个状态最终用哪个颜色。
///
/// 逐个问 [`Settings::state_color`] —— 那是 Rust 侧唯一的「状态 → 颜色」入口
/// （物理灯也从那里取）。按**全部状态名**遍历而不是 `parse`：聚合出来的
/// `stale` / `offline` 也要有颜色，而 `parse` 刻意不接受它们。
fn state_colors(settings: &Settings) -> serde_json::Value {
    let mut colors = serde_json::Map::new();
    for name in Signal::ALL_NAMES {
        colors.insert(
            name.to_string(),
            serde_json::json!(settings.state_color(name)),
        );
    }
    serde_json::Value::Object(colors)
}

/// 状态名 → 那句话。和系统通知同一张表（`lang::signal_label`）。
fn labels(lang: Lang) -> serde_json::Value {
    let mut labels = serde_json::Map::new();
    for name in Signal::ALL_NAMES {
        let Some(state) = Signal::from_name(name) else { continue };
        labels.insert(name.to_string(), serde_json::json!(crate::lang::signal_label(lang, state)));
    }
    serde_json::Value::Object(labels)
}

/// 用户在「外观 → 各组的显示」里改过的组名。键是上报方给的原始组名。
///
/// 灯上就是按这个显示（`lampSort.ts` 的 `labelOf`），面板得跟着 —— 否则同一个组
/// 在两个地方叫两个名字，用户会以为那是两个组。
fn group_labels(settings: &Settings) -> serde_json::Value {
    let mut labels = serde_json::Map::new();
    for (group, meta) in &settings.group_meta {
        if let Some(name) = meta.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            labels.insert(group.clone(), serde_json::json!(name));
        }
    }
    serde_json::Value::Object(labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Palette;

    /// 三套配色的四个语义色都要注进去，而且**缺省配色就是灯上那四个色号**。
    #[test]
    fn the_page_carries_the_palette_colors() {
        let html = page(&Settings::default(), Lang::Zh);
        assert!(html.contains("#16a34a"), "正常那档的绿不在");
        assert!(html.contains("#d0342c"), "危险那档的红不在");
        assert!(!html.contains("__CONFIG__"), "占位符必须被换掉");

        let vivid = Settings {
            palette: Palette::Vivid,
            ..Settings::default()
        };
        assert!(page(&vivid, Lang::Zh).contains("#00b34a"));
    }

    /// 用户逐个状态改过的颜色**压过预设** —— 和灯上的优先级一致。
    #[test]
    fn a_per_state_override_wins() {
        let mut settings = Settings::default();
        settings.styles.insert(
            "error".to_string(),
            crate::config::StateStyle {
                color: Some("#123456".to_string()),
                ..Default::default()
            },
        );
        let html = page(&settings, Lang::Zh);
        assert!(html.contains("#123456"), "逐个状态的颜色没进去");
        assert!(html.contains("\"error\":\"#123456\""), "要挂在 error 上：{html}");
    }

    /// 注入的是 JSON，而颜色 / 组名是用户可写的字符串 —— `</script>` 必须逃不掉。
    #[test]
    fn user_written_values_cannot_break_out_of_the_script_tag() {
        let mut settings = Settings::default();
        settings.group_meta.insert(
            "ci".to_string(),
            crate::config::GroupMeta {
                name: Some("</script><script>alert(1)</script>".to_string()),
                order: None,
            },
        );
        let html = page(&settings, Lang::Zh);
        assert!(!html.contains("</script><script>alert"), "数据逃出了 script 标签");
        assert!(html.contains("\\u003c/script"), "该转义成 \\u003c");
    }

    /// 语言跟着应用走：中文那套词是系统通知里的原话。
    #[test]
    fn the_labels_follow_the_language() {
        assert!(page(&Settings::default(), Lang::Zh).contains("需要授权"));
        assert!(page(&Settings::default(), Lang::En).contains("Needs permission"));
    }
}
