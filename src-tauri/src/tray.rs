use crate::notify;
use crate::state::{Signal, Store};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
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
///
/// **这个压扁的比例只属于三点字形**。跟随状态的单点在 `TRAY_DOT_SIZE` 里另取
/// 方形边长 —— 单点没有「排成一行」的问题，套用 2.16:1 会被拉成一个椭圆。
const TRAY_GLYPH_W: u32 = 432;
const TRAY_GLYPH_H: u32 = 200;

/// 跟随状态的单点位图边长。
///
/// 高仍取 `TRAY_GLYPH_H`，菜单栏里就还是 18pt 高；宽**等于高**，因为单点要的是
/// 一个正圆 —— 三点的 2.16:1 是为塞下三个点而刻意压扁的，跟单点无关。
const TRAY_DOT_SIZE: u32 = TRAY_GLYPH_H;

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

/// 解析 `#rrggbb`（也容忍 `#rgb`）成 RGB。
///
/// 颜色表由前端算好跨 IPC 传过来，属于系统边界 —— 这里的判断不能 panic。
/// 先逐字节确认全是 ASCII 十六进制字符再切片，否则一个 `#你好` 会在按字节
/// 切分时踩到非字符边界而崩。
///
/// 解析失败一律返回 `None`，由调用方退回单色模板图。**绝不拿一个猜出来的颜色
/// 去画**：那样用户看到的是「颜色不对但说不清哪里不对」的托盘，比干脆不变色难查得多。
fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| -> u8 {
        // 上面已确认全是十六进制字符，所以 to_digit 必定成功
        (hex.as_bytes()[i] as char).to_digit(16).expect("已校验为十六进制") as u8
    };
    match hex.len() {
        // #f00 展开成 #ff0000：每位乘 17（0xF → 0xFF）
        3 => Some([nibble(0) * 17, nibble(1) * 17, nibble(2) * 17]),
        6 => Some([
            nibble(0) * 16 + nibble(1),
            nibble(2) * 16 + nibble(3),
            nibble(4) * 16 + nibble(5),
        ]),
        _ => None,
    }
}

/// 画一颗实心圆点，返回 RGBA 位图。
///
/// 和三点的模板图同一套做法：按到圆心的距离算覆盖率，边缘自然过渡 1px；
/// 四周留一圈余量，免得圆点顶到画布边缘、在菜单栏里看着是贴边的。
///
/// 与模板图相反，这里是**彩色**图：RGB 填请求的颜色，由调用方同时把模板标志
/// 关掉 —— 模板图会被系统按菜单栏明暗整个反色，颜色会被抹得一点不剩。
fn dot_rgba(color: [u8; 3], size: u32) -> Vec<u8> {
    let side = size as f32;
    // 每边留 16% 余量：`tray-icon` 把像素高当成 18pt（见上面模板图的说明），
    // 所以画布 200px 会被压成 18pt，圆点占 68% ≈ 12pt —— 和原来三点图里
    // 单颗点的大小（约 9.7pt）是一个量级。留 10% 的话点是 14.4pt，在菜单栏里
    // 会明显比周围的图标重，看着像一坨。
    //
    // 小尺寸下 round 可能把余量抹成 0，那也是对的：宁可圆点顶到边，也不能让
    // 余量把整颗点吃掉（小图标会因此整颗消失）。
    let margin = (side * 0.16).round();
    let radius = (side - 2.0 * margin) / 2.0;
    let center = side / 2.0;

    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);

            let index = ((y * size + x) * 4) as usize;
            rgba[index] = color[0];
            rgba[index + 1] = color[1];
            rgba[index + 2] = color[2];
            rgba[index + 3] = (coverage * 255.0).round() as u8;
        }
    }
    rgba
}

/// 不跟随状态时用的字形：三点单色，和原来一样。
///
/// **两个平台走的是两条完全不同的路：**
///
/// - macOS：只填 alpha，交给系统按菜单栏明暗反色（模板图）。
/// - 其它平台：**没有模板着色这回事**。`is_template` 参数在非 macOS 上是被直接
///   丢掉的（`tray-icon` 里那个方法连图都不收），托盘实现把原始位图交给系统、
///   没有任何主题逻辑。于是那张 RGB 全 0 的图在 Windows 上就是**纯黑** ——
///   深色任务栏上等于看不见。所以那些平台上直接涂成中性灰。
///
/// 想要真正的黑白自适应得读 Windows 的 `SystemUsesLightTheme` 注册表键，
/// 为一个图标引一个注册表依赖不值得；中性灰在明暗两种任务栏上都有对比度。
#[cfg(target_os = "macos")]
fn monochrome_glyph() -> (Vec<u8>, u32, u32, bool) {
    (tray_glyph_rgba(), TRAY_GLYPH_W, TRAY_GLYPH_H, true)
}

#[cfg(not(target_os = "macos"))]
fn monochrome_glyph() -> (Vec<u8>, u32, u32, bool) {
    (
        paint(tray_glyph_rgba(), NEUTRAL_GRAY),
        TRAY_GLYPH_W,
        TRAY_GLYPH_H,
        false,
    )
}

/// 中性灰。亮色任务栏（约 `#f3f3f3`）和暗色任务栏（约 `#202020`）上都够看清，
/// 这也是它存在的全部理由 —— 不是「好看的颜色」，是「两边都不糊的颜色」。
#[cfg(not(target_os = "macos"))]
const NEUTRAL_GRAY: [u8; 3] = [0x8a, 0x8a, 0x8e];

/// 把模板图（RGB 全 0、只有 alpha）涂成一个固定颜色，保留 alpha。
#[cfg(not(target_os = "macos"))]
fn paint(mut rgba: Vec<u8>, color: [u8; 3]) -> Vec<u8> {
    for pixel in rgba.chunks_mut(4) {
        pixel[..3].copy_from_slice(&color);
    }
    rgba
}

/// 让托盘图标跟上当前状态。
///
/// `colors` 是前端算好的「状态名 → `#rrggbb`」。**颜色表不在 Rust 里复制一份**：
/// 它是「配色方案 + 用户逐个覆盖」解析出来的，只存在于前端的 `lib/palettes.ts`。
/// 复制一份到这里就意味着两处会漂移，而用户改了颜色托盘不变是最难注意到的那种 bug。
pub fn sync(app: &AppHandle, state: Signal, follows: bool, colors: &BTreeMap<String, String>) {
    // 托盘一定存在（init 里建的），但真拿不到就静默跳过 —— 一个图标画不出来，
    // 不该让状态上报整条链路跟着崩。
    let Some(tray) = app.tray_by_id("signalhub") else {
        return;
    };

    // 关掉跟随、或这个状态没有可用颜色时，退回单色字形（见 monochrome_glyph）。
    let rgb = if follows {
        colors.get(state.name()).and_then(|hex| parse_hex_color(hex))
    } else {
        None
    };

    let (rgba, width, height, is_template) = match rgb {
        Some(color) => (
            dot_rgba(color, TRAY_DOT_SIZE),
            TRAY_DOT_SIZE,
            TRAY_DOT_SIZE,
            false,
        ),
        None => monochrome_glyph(),
    };

    // 用 set_icon_with_as_template 而不是先 set_icon 再 set_icon_as_template：
    // macOS 上分两步会让图标渲染两次、肉眼可见地闪一下（见 tauri 源码注释）。
    // 非 macOS 平台它会退化成 set_icon，模板标志本来也只有 macOS 认。
    if let Err(err) = tray.set_icon_with_as_template(
        Some(tauri::image::Image::new_owned(rgba, width, height)),
        is_template,
    ) {
        eprintln!("[signalhub] 更新托盘图标失败: {err}");
    }
}

/// 托盘是这台没有 Dock 图标、窗口又不可关闭的应用**唯一**的退出入口，
/// 所以它在 Phase 1 就必须存在，不属于后续打磨项。
pub fn init(app: AppHandle) -> tauri::Result<()> {
    let menu = build_menu(&app, crate::current_lang(&app))?;

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
            // 「手动状态」那四项：id → 动作的映射只有一处（见 `manual::action_for`），
            // 托盘这边不认识那几个 id。在这里再写一遍字面量的话，写错一个就是
            // 「点了没反应」—— 而且不报错。
            id if crate::manual::action_for(id).is_some() => {
                crate::manual::action_for(id).expect("上面刚判断过")(app)
            }
            "quit" => app.exit(0),
            "inbox" => crate::inbox::toggle(app),
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

/// 按当前语言重建托盘菜单。
///
/// 菜单是启动时建好就一直挂着的，换语言只能整个换掉 —— 逐项改文案需要把
/// `MenuItem` 存下来，而那些句柄除了这一件事之外没有任何用处。
///
/// 菜单项的 **id 不变**，所以 `on_menu_event` 那个分派不用动。
pub fn rebuild_menu(app: &AppHandle, lang: crate::config::Lang) {
    let Some(tray) = app.tray_by_id("signalhub") else {
        return;
    };
    match build_menu(app, lang) {
        Ok(menu) => {
            if let Err(err) = tray.set_menu(Some(menu)) {
                eprintln!("[signalhub] 重建托盘菜单失败: {err}");
            }
        }
        Err(err) => eprintln!("[signalhub] 构造托盘菜单失败: {err}"),
    }
}

fn build_menu(app: &AppHandle, lang: crate::config::Lang) -> tauri::Result<Menu<tauri::Wry>> {
    let t = crate::lang::text(lang);
    // 收件箱：把「谁需要你」从灯上搬到一个能点的列表里。放在最上面 ——
    // 它是这个应用从「看」到「做」的主入口。
    let inbox = MenuItem::with_id(app, "inbox", t.tray_inbox, true, None::<&str>)?;
    // 这些构造器都要求传 &M（M: Manager），所以统一用 app
    let toggle = MenuItem::with_id(app, "toggle", t.tray_show_hide, true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", t.tray_settings, true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", t.tray_pause, true, None::<&str>)?;
    // 故意不显示「开 / 关」状态：快捷键也能切换它，菜单里的状态会漂移。
    // 准确状态在设置面板里（get_toggles）。
    let pass_through =
        MenuItem::with_id(app, "pass_through", t.tray_pass_through, true, None::<&str>)?;
    // 折叠后只剩一个点，没有别的入口切回来 —— 所以托盘这一项是必需的
    let collapsed = MenuItem::with_id(app, "collapsed", t.tray_collapsed, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", t.tray_quit, true, None::<&str>)?;

    // 手动状态收成一个子菜单：它是「我在干什么」，和上面那几个「这个 app 怎么显示」
    // 不是一类东西，平铺进去会把托盘菜单撑到九项。**四项的文案都是固定的**（不随
    // 状态变）：三个动作都是幂等的「设」，想收掉有「清除手动状态」，所以不需要为了
    // 改文案重建菜单 —— 那也正是菜单状态最容易和真实状态漂移的地方。
    let manual = Submenu::with_items(
        app,
        t.manual_title,
        true,
        &[
            &MenuItem::with_id(
                app,
                crate::manual::MENU_FOCUS,
                t.manual_focus,
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                crate::manual::MENU_MEETING,
                t.manual_meeting,
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                crate::manual::MENU_AWAY,
                t.manual_away,
                true,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                crate::manual::MENU_CLEAR,
                t.manual_clear,
                true,
                None::<&str>,
            )?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            &inbox,
            &toggle,
            &settings,
            &pause,
            &manual,
            &pass_through,
            &collapsed,
            &quit,
        ],
    )
}

/// 显示 / 隐藏灯。托盘的「显示 / 隐藏灯球」和全局快捷键 `⌃⌥L` 都走这里 ——
/// 两条入口各写一遍的话，「切换」到底做了什么迟早分叉。
pub(crate) fn toggle_lamp(app: &AppHandle) {
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

/// 暂停 / 恢复监控。托盘和灯上的右键菜单都走这里 —— 两条入口各写一遍，
/// 「暂停到底做了什么」迟早分叉。
pub(crate) fn toggle_monitor(app: &AppHandle) {
    let store = app.state::<Arc<Mutex<Store>>>();
    let (changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        // 暂停状态不在 Store 的公开读接口里，从快照取当前值再取反
        let now = std::time::Instant::now();
        let next = !guard.snapshot().paused;
        let changed = guard.set_paused(next, now);
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

    /// 取 `size × size` 位图里某个像素的 RGBA。与 `alpha_at` 不同，这里的边长
    /// 是参数 —— 单点的用例要拿小尺寸去逼「余量吃光整颗点」的退化。
    fn dot_pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_dot_buffer_is_square_and_has_the_right_length() {
        let rgba = dot_rgba([0x1f, 0x6f, 0xeb], TRAY_DOT_SIZE);
        assert_eq!(rgba.len(), (TRAY_DOT_SIZE * TRAY_DOT_SIZE * 4) as usize);

        // 单点必须是正圆：宽高比 1:1。三点的 2.16:1 是为塞下三个点而压扁的，
        // 单点若沿用会被拉成椭圆 —— 这条锁住「宽 = 高」。
        assert_eq!(TRAY_DOT_SIZE, TRAY_GLYPH_H, "单点应与字形等高，且宽等于高");

        // 换个小尺寸，长度公式仍成立（不是只对某一个值凑出来的）
        assert_eq!(dot_rgba([0, 0, 0], 5).len(), (5 * 5 * 4) as usize);
    }

    #[test]
    fn the_dot_centre_carries_the_requested_colour() {
        let rgba = dot_rgba([0x1f, 0x6f, 0xeb], TRAY_DOT_SIZE);
        let c = TRAY_DOT_SIZE / 2;
        let pixel = dot_pixel(&rgba, TRAY_DOT_SIZE, c, c);
        assert_eq!(pixel[..3], [0x1f, 0x6f, 0xeb], "中心应是请求的颜色");
        assert_eq!(pixel[3], 255, "中心应完全不透明");
    }

    #[test]
    fn the_dot_corners_are_transparent() {
        let rgba = dot_rgba([0x1f, 0x6f, 0xeb], TRAY_DOT_SIZE);
        let last = TRAY_DOT_SIZE - 1;
        for (x, y) in [(0, 0), (last, 0), (0, last), (last, last)] {
            assert_eq!(
                dot_pixel(&rgba, TRAY_DOT_SIZE, x, y)[3],
                0,
                "({x},{y}) 应透明"
            );
        }
    }

    /// 小图标上余量四舍五入后可能只剩几个像素，圆心必须还在实心范围内 ——
    /// 否则一个尺寸没算好，整颗点会被余量吃光、图标变成全透明。
    #[test]
    fn a_small_dot_still_has_an_opaque_centre() {
        for size in [4u32, 5, 8, 16] {
            let rgba = dot_rgba([0xff, 0x00, 0x00], size);
            let c = size / 2;
            assert!(
                dot_pixel(&rgba, size, c, c)[3] > 200,
                "边长 {size} 时圆心应仍是不透明的"
            );
        }
    }

    #[test]
    fn hex_colours_parse_long_and_short_form() {
        assert_eq!(parse_hex_color("#1f6feb"), Some([0x1f, 0x6f, 0xeb]));
        assert_eq!(parse_hex_color("#FFFFFF"), Some([0xff, 0xff, 0xff]));
        // #rgb 展开成 #rrggbb
        assert_eq!(parse_hex_color("#f00"), Some([0xff, 0x00, 0x00]));
        assert_eq!(parse_hex_color("  #1f6feb  "), Some([0x1f, 0x6f, 0xeb]));

        for bad in ["1f6feb", "#gggggg", "", "#", "nonsense", "#12", "#12345", "#你好"] {
            assert_eq!(parse_hex_color(bad), None, "{bad:?} 应被拒绝");
        }
    }

    /// 非 macOS 平台上**不能**交一张 RGB 全 0 的图 —— 那是纯黑，深色任务栏上
    /// 等于看不见。这条只有在非 macOS 上才会编译进去（本机是 macOS），
    /// 所以它靠 CI 的 Windows / Linux 那两个 job 兜底。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_monochrome_glyph_is_painted_so_a_dark_taskbar_can_see_it() {
        let (rgba, _, _, is_template) = monochrome_glyph();
        assert!(!is_template, "非 macOS 上模板标志没有意义，不该声称自己是模板图");

        let opaque: Vec<&[u8]> = rgba.chunks(4).filter(|p| p[3] > 0).collect();
        assert!(!opaque.is_empty(), "字形不该是空的");
        assert!(
            opaque.iter().all(|p| p[..3] == NEUTRAL_GRAY),
            "有像素的地方都该是中性灰"
        );
        assert!(
            opaque.iter().all(|p| p[..3] != [0, 0, 0]),
            "绝不能是纯黑 —— 这就是这个函数存在的理由"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn painting_keeps_the_alpha_channel() {
        let template = tray_glyph_rgba();
        let painted = paint(template.clone(), [1, 2, 3]);

        assert_eq!(painted.len(), template.len());
        for (before, after) in template.chunks(4).zip(painted.chunks(4)) {
            assert_eq!(before[3], after[3], "alpha 不能动，形状全靠它");
            if before[3] > 0 {
                assert_eq!(&after[..3], &[1, 2, 3]);
            }
        }
    }
}
