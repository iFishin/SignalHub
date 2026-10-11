use crate::notify;
use crate::state::Store;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

/// 托盘字形的位图尺寸。
///
/// `tray-icon` 会把**像素高当成 18pt**（见它的 macos 实现：
/// `icon_height = 18.0; icon_width = width / (height / 18)`），宽度按宽高比算。
/// 所以这里给高分辨率位图换清晰度，而宽高比决定它在菜单栏里占多宽 ——
/// 2.16:1 渲染出来约 38.9 × 18pt。三颗点排成一行本来约 3.7:1，那样会有 66pt 宽，
/// 菜单栏放不下，所以刻意压扁：点距收紧，点本身就小了。
const TRAY_GLYPH_W: u32 = 432;
const TRAY_GLYPH_H: u32 = 200;

/// 画三颗圆点，返回 RGBA 位图。
///
/// 用代码画而不是放一张 PNG：`Image::from_bytes` 需要开 tauri 的 `image-png`
/// 特性，会把整个 `image` crate 拖进依赖树；而托盘字形就是三个圆，属于几何
/// 而不是美术 —— 代码画既不添依赖，也能单测。
///
/// 只填 alpha，RGB 全 0：这是**模板图**，由系统按菜单栏明暗决定画成黑还是白。
fn tray_glyph_rgba() -> Vec<u8> {
    let w = TRAY_GLYPH_W as f32;
    let h = TRAY_GLYPH_H as f32;

    // 左右各留 4% 余量，不然首尾两颗点会顶到边缘
    let margin = (w * 0.04).round();
    let usable = w - 2.0 * margin;
    // 由 3d + 2·(d/3) = usable 解出直径，点距取直径的三分之一
    let diameter = usable * 3.0 / 11.0;
    let gap = (usable - 3.0 * diameter) / 2.0;
    let radius = diameter / 2.0;
    let cy = h / 2.0;
    let centers: Vec<f32> = (0..3)
        .map(|i| margin + radius + i as f32 * (diameter + gap))
        .collect();

    let mut rgba = vec![0u8; (TRAY_GLYPH_W * TRAY_GLYPH_H * 4) as usize];
    for y in 0..TRAY_GLYPH_H {
        for x in 0..TRAY_GLYPH_W {
            // 按到圆心的距离算覆盖率，边缘自然过渡 1px（三个圆互不重叠，取最大）
            let coverage = centers.iter().fold(0.0f32, |best, cx| {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let distance = (dx * dx + dy * dy).sqrt();
                best.max((radius + 0.5 - distance).clamp(0.0, 1.0))
            });

            let index = ((y * TRAY_GLYPH_W + x) * 4) as usize;
            rgba[index + 3] = (coverage * 255.0).round() as u8;
        }
    }
    rgba
}

/// 托盘是这台没有 Dock 图标、窗口又不可关闭的应用**唯一**的退出入口，
/// 所以它在 Phase 1 就必须存在，不属于后续打磨项。
pub fn init(app: AppHandle) -> tauri::Result<()> {
    // 这些构造器都要求传 &M（M: Manager），所以统一用 &app
    let toggle = MenuItem::with_id(&app, "toggle", "显示 / 隐藏灯球", true, None::<&str>)?;
    let settings = MenuItem::with_id(&app, "settings", "设置…", true, None::<&str>)?;
    let pause = MenuItem::with_id(&app, "pause", "暂停 / 恢复监控", true, None::<&str>)?;
    // 故意不显示「开 / 关」状态：快捷键也能切换它，菜单里的状态会漂移。
    // 准确状态在设置面板里（get_toggles）。
    let pass_through = MenuItem::with_id(&app, "pass_through", "切换点击穿透", true, None::<&str>)?;
    // 折叠后只剩一个点，没有别的入口切回来 —— 所以托盘这一项是必需的
    let collapsed = MenuItem::with_id(&app, "collapsed", "展开 / 折叠外形", true, None::<&str>)?;
    let quit = MenuItem::with_id(&app, "quit", "退出 SignalHub", true, None::<&str>)?;
    let menu = Menu::with_items(
        &app,
        &[&toggle, &settings, &pause, &pass_through, &collapsed, &quit],
    )?;

    TrayIconBuilder::with_id("signalhub")
        // 托盘用单独的单色字形，**不用应用图标**：应用图标是深色圆角方块 +
        // 三颗彩色圆点，放进菜单栏会是一坨深色（深色菜单栏上几乎看不见），
        // 而且彩色图标和菜单栏里其它单色图标格格不入。
        //
        // 模板图由系统按明暗主题自动反色（icon_as_template），所以只用 alpha。
        .icon(tauri::image::Image::new_owned(
            tray_glyph_rgba(),
            TRAY_GLYPH_W,
            TRAY_GLYPH_H,
        ))
        .icon_as_template(true)
        .tooltip("SignalHub")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "toggle" => toggle_lamp(app),
            "settings" => crate::reveal_settings(app),
            "pause" => toggle_monitor(app),
            "pass_through" => {
                let enabled = app
                    .state::<crate::SharedUi>()
                    .lock()
                    .expect("ui poisoned")
                    .pass_through;
                if let Err(err) = crate::set_pass_through_impl(app, !enabled) {
                    eprintln!("[signalhub] 托盘切换点击穿透失败: {err}");
                }
            }
            "collapsed" => {
                let collapsed = app
                    .state::<crate::SharedSettings>()
                    .lock()
                    .expect("settings poisoned")
                    .collapsed;
                if let Err(err) = crate::set_collapsed(app, !collapsed) {
                    eprintln!("[signalhub] 托盘切换折叠失败: {err}");
                }
            }
            _ => {}
        })
        .build(&app)?;

    Ok(())
}

fn toggle_lamp(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn toggle_monitor(app: &AppHandle) {
    let store = app.state::<Arc<Mutex<Store>>>();
    let (changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        // 暂停状态不在 Store 的公开读接口里，从快照取当前值再取反
        let next = !guard.snapshot().paused;
        let changed = guard.set_paused(next);
        (changed, guard.snapshot())
    };
    if changed {
        notify::push(app, &snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(rgba: &[u8], x: u32, y: u32) -> u8 {
        rgba[((y * TRAY_GLYPH_W + x) * 4 + 3) as usize]
    }

    #[test]
    fn the_glyph_fills_a_full_rgba_buffer() {
        assert_eq!(
            tray_glyph_rgba().len(),
            (TRAY_GLYPH_W * TRAY_GLYPH_H * 4) as usize
        );
    }

    /// 模板图只靠 alpha 表达形状 —— RGB 必须全 0，否则系统反色时会露出偏色
    #[test]
    fn the_glyph_uses_alpha_only() {
        let rgba = tray_glyph_rgba();
        assert!(
            rgba.chunks(4).all(|pixel| pixel[..3] == [0, 0, 0]),
            "RGB 应全为 0"
        );
    }

    #[test]
    fn the_corners_are_transparent() {
        let rgba = tray_glyph_rgba();
        let corners = [
            (0, 0),
            (TRAY_GLYPH_W - 1, 0),
            (0, TRAY_GLYPH_H - 1),
            (TRAY_GLYPH_W - 1, TRAY_GLYPH_H - 1),
        ];
        for (x, y) in corners {
            assert_eq!(alpha_at(&rgba, x, y), 0, "({x},{y}) 应透明");
        }
    }

    /// 中线横着扫一遍：应该是三段不透明、中间隔着两段透明，而且三颗点等大等距。
    /// 这条同时守住了「点顶到边缘」和「三颗点变成一团」两种退化。
    #[test]
    fn the_middle_row_is_three_equal_separated_dots() {
        let rgba = tray_glyph_rgba();
        let y = TRAY_GLYPH_H / 2;

        let mut runs: Vec<(u32, u32)> = Vec::new();
        for x in 0..TRAY_GLYPH_W {
            let solid = alpha_at(&rgba, x, y) > 200;
            match runs.last_mut() {
                Some(last) if solid && x == last.1 + 1 => last.1 = x,
                _ if solid => runs.push((x, x)),
                _ => {}
            }
        }
        assert_eq!(runs.len(), 3, "应是三颗点，实际 {runs:?}");

        let widths: Vec<u32> = runs.iter().map(|(a, b)| b - a + 1).collect();
        let gaps: Vec<u32> = runs.windows(2).map(|w| w[1].0 - w[0].1 - 1).collect();
        assert!(
            widths.windows(2).all(|w| w[0].abs_diff(w[1]) <= 1),
            "三颗点应等大: {widths:?}"
        );
        assert!(
            gaps.windows(2).all(|w| w[0].abs_diff(w[1]) <= 1),
            "点距应相等: {gaps:?}"
        );
        assert!(gaps[0] > 4, "点之间要留出空隙: {gaps:?}");

        // 首尾点不能顶到画布边缘，否则菜单栏里看着是贴边的
        assert!(runs[0].0 > 4, "左边应留白: {runs:?}");
        assert!(TRAY_GLYPH_W - 1 - runs[2].1 > 4, "右边应留白: {runs:?}");
    }
}
