#!/usr/bin/env swift
// scripts/render-demo.swift
//
// 重画 README 顶部那张 `docs/assets/demo.gif`。**不是产品代码**，只是素材生成器：
//
//     sh scripts/render-demo.sh
//
// 分两步：`render-demo.sh` 开临时目录并合成 GIF，本脚本只把 PNG 帧画进去。
// 为什么不用录屏：真窗口里的状态要等智能体真的跑一次才出现，而这里要演示的是
// **默认灯效本身**（颜色 + 闪烁节奏 + 安静模式 + 审批气泡）。逐帧画反而能和代码
// 对上 —— 下面的常量全部抄自 `src/styles/state.css` 与 `src/styles/lamp.css`：
//
//     绿 #16a34a / 黄 #d99a06 / 红 #d0342c / 灰 #9ca3af   (state.css)
//     thinking 640ms · working 1600ms · permission 900ms   (lamp.css)
//     done = 420ms 跃迁脉冲                                (lamp.css)
//     permission / error 的灯珠是圆角方形，其余是圆形       (README)
//     危险命令的「允许」要按住 800ms                        (README)
//
// 改了这些默认值就该重跑一次。画面刻意不带自然语言文案：状态名就是接口里的常量，
// 按钮只有符号，所以同一张 GIF 能同时放进中英两份 README。
//
// 依赖：macOS（AppKit/CoreGraphics）+ PATH 上的 ffmpeg。帧由调用方写在临时目录。

import AppKit
import Foundation

// MARK: - 画布与常量

let W: CGFloat = 880
let H: CGFloat = 480
let FPS = 12

func C(_ hex: UInt32, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
            green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: a)
}

let green: UInt32 = 0x16a34a
let amber: UInt32 = 0xd99a06
let red: UInt32 = 0xd0342c
let gray: UInt32 = 0x9ca3af
let bgTop: UInt32 = 0x0A0E13
let bgBottom: UInt32 = 0x141B25
let panel: UInt32 = 0x1B2230
let panelLine: UInt32 = 0x2C3646
let fg: UInt32 = 0xE8EDF4
let muted: UInt32 = 0x8592A3
let dim: UInt32 = 0x4A5566

enum Kind { case steady, blink, pulse }

struct Phase {
    let name: String
    let dur: Double
    let color: UInt32
    let period: Double
    let kind: Kind
    let note: String
    let square: Bool
    let bubble: Bool
}

let phases: [Phase] = [
    Phase(name: "IDLE",       dur: 1.2, color: green, period: 0,    kind: .steady, note: "steady · quiet mode",        square: false, bubble: false),
    Phase(name: "THINKING",   dur: 1.6, color: green, period: 0.64, kind: .blink,  note: "pulse 640ms",                square: false, bubble: false),
    Phase(name: "WORKING",    dur: 1.6, color: green, period: 1.60, kind: .blink,  note: "pulse 1600ms",               square: false, bubble: false),
    Phase(name: "PERMISSION", dur: 2.8, color: red,   period: 0.90, kind: .blink,  note: "pulse 900ms · hold 800ms",   square: true,  bubble: true),
    Phase(name: "WORKING",    dur: 1.2, color: green, period: 1.60, kind: .blink,  note: "pulse 1600ms",               square: false, bubble: false),
    Phase(name: "DONE",       dur: 1.2, color: green, period: 0,    kind: .pulse,  note: "pulse-in 420ms",             square: false, bubble: false),
]
let total = phases.reduce(0.0) { $0 + $1.dur }

func clamp(_ v: Double, _ lo: Double = 0, _ hi: Double = 1) -> Double {
    min(max(v, lo), hi)
}

/// 当前灯珠亮度。`.steady` 的 0.55 就是「安静模式」下几乎透明的那块牌。
func brightness(_ p: Phase, _ local: Double) -> CGFloat {
    switch p.kind {
    case .steady:
        return 0.55
    case .blink:
        let wave = 0.5 + 0.5 * sin(2 * .pi * local / p.period - .pi / 2)
        return CGFloat(0.26 + 0.74 * wave)
    case .pulse:
        return CGFloat(0.55 + 0.45 * max(0, 1 - local / 0.42))
    }
}

// MARK: - 绘图小工具（一律用「左上原点」的屏幕坐标）

/// CGContext 原点是左下角，所有矩形在这里翻一次 y。
func R(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> CGRect {
    CGRect(x: x, y: H - y - h, width: w, height: h)
}

func P(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: H - y) }

func roundPath(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) -> CGPath {
    CGPath(roundedRect: R(x, y, w, h), cornerWidth: r, cornerHeight: r, transform: nil)
}

func fillRound(_ ctx: CGContext, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat,
               _ r: CGFloat, _ color: NSColor) {
    ctx.addPath(roundPath(x, y, w, h, r))
    ctx.setFillColor(color.cgColor)
    ctx.fillPath()
}

func strokeRound(_ ctx: CGContext, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat,
                 _ r: CGFloat, _ color: NSColor, _ width: CGFloat = 1) {
    ctx.addPath(roundPath(x, y, w, h, r))
    ctx.setStrokeColor(color.cgColor)
    ctx.setLineWidth(width)
    ctx.strokePath()
}

func font(_ size: CGFloat, _ weight: NSFont.Weight, _ mono: Bool) -> NSFont {
    mono ? NSFont.monospacedSystemFont(ofSize: size, weight: weight)
         : NSFont.systemFont(ofSize: size, weight: weight)
}

func textWidth(_ s: String, _ size: CGFloat, _ weight: NSFont.Weight = .regular, _ mono: Bool = false) -> CGFloat {
    (s as NSString).size(withAttributes: [.font: font(size, weight, mono)]).width
}

/// `top` 是文字行顶边；`draw(at:)` 要的是左下角，这里按字号估一个行高。
func T(_ s: String, _ x: CGFloat, _ top: CGFloat, _ size: CGFloat, _ color: NSColor,
       _ weight: NSFont.Weight = .regular, _ mono: Bool = false) {
    (s as NSString).draw(at: NSPoint(x: x, y: H - top - size * 1.28),
                         withAttributes: [.foregroundColor: color, .font: font(size, weight, mono)])
}

func TRight(_ s: String, _ right: CGFloat, _ top: CGFloat, _ size: CGFloat, _ color: NSColor,
            _ weight: NSFont.Weight = .regular, _ mono: Bool = false) {
    T(s, right - textWidth(s, size, weight, mono), top, size, color, weight, mono)
}

func dot(_ ctx: CGContext, _ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat, _ color: NSColor) {
    ctx.setFillColor(color.cgColor)
    ctx.fillEllipse(in: R(cx - r, cy - r, r * 2, r * 2))
}

/// 鼠标箭头（纯路径，省得引用系统光标图）。
func cursor(_ ctx: CGContext, _ x: CGFloat, _ y: CGFloat) {
    let p = CGMutablePath()
    p.move(to: P(x, y))
    p.addLine(to: P(x, y + 19))
    p.addLine(to: P(x + 5, y + 14))
    p.addLine(to: P(x + 9, y + 22))
    p.addLine(to: P(x + 12, y + 20))
    p.addLine(to: P(x + 8, y + 12))
    p.addLine(to: P(x + 15, y + 11))
    p.closeSubpath()
    ctx.setShadow(offset: CGSize(width: 0, height: -2), blur: 4, color: C(0x000000, 0.6).cgColor)
    ctx.addPath(p)
    ctx.setFillColor(C(0xFFFFFF, 0.95).cgColor)
    ctx.setStrokeColor(C(0x0A0E13, 0.8).cgColor)
    ctx.setLineWidth(1)
    ctx.drawPath(using: .fillStroke)
    ctx.setShadow(offset: .zero, blur: 0, color: nil)
}

// MARK: - 一帧

func drawFrame(_ t: Double) -> NSBitmapImageRep {
    guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(W), pixelsHigh: Int(H),
                                     bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                                     isPlanar: false, colorSpaceName: .deviceRGB,
                                     bytesPerRow: 0, bitsPerPixel: 0) else { fatalError("bitmap") }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext

    // 找到当前阶段
    var acc = 0.0
    var idx = phases.count - 1
    var local = 0.0
    for (i, p) in phases.enumerated() {
        if t < acc + p.dur { idx = i; local = t - acc; break }
        acc += p.dur
    }
    let ph = phases[idx]
    let b = brightness(ph, local)
    let stateColor = C(ph.color)

    // ---- 背景：渐变 + 细网格 + 状态色光斑 ----
    let bg = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                        colors: [C(bgTop).cgColor, C(bgBottom).cgColor] as CFArray,
                        locations: [0, 1])!
    ctx.drawLinearGradient(bg, start: CGPoint(x: 0, y: H), end: CGPoint(x: 0, y: 0), options: [])

    ctx.setStrokeColor(C(0xFFFFFF, 0.030).cgColor)
    ctx.setLineWidth(1)
    for gx in stride(from: CGFloat(0), to: W, by: 32) {
        ctx.move(to: CGPoint(x: gx, y: 0)); ctx.addLine(to: CGPoint(x: gx, y: H))
    }
    for gy in stride(from: CGFloat(0), to: H, by: 32) {
        ctx.move(to: CGPoint(x: 0, y: gy)); ctx.addLine(to: CGPoint(x: W, y: gy))
    }
    ctx.strokePath()

    let halo = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                          colors: [stateColor.withAlphaComponent(0.13).cgColor,
                                   stateColor.withAlphaComponent(0).cgColor] as CFArray,
                          locations: [0, 1])!
    let hc = CGPoint(x: 690, y: H - 150)
    ctx.drawRadialGradient(halo, startCenter: hc, startRadius: 0, endCenter: hc, endRadius: 330, options: [])

    // ---- 左上：当前状态 + 它真实的节奏参数 ----
    T("SIGNALHUB", 36, 26, 12, C(dim), .medium, true)
    T("LIVE STATES", 36, 46, 10, C(dim), .regular, true)
    T(ph.name, 34, 80, 36, stateColor, .bold, true)
    T(ph.note, 36, 128, 12, C(muted), .regular, true)
    T("DEFAULT RHYTHM", 36, 152, 10, C(dim), .regular, true)

    // 三颗小点呼应应用图标：只点亮当前状态所属的那一组色
    let tri: [(UInt32, CGFloat)] = [(green, 36), (amber, 54), (red, 72)]
    for (c, x) in tri {
        let on = (ph.color == green && c == green) || (ph.color == amber && c == amber)
              || (ph.color == red && c == red) || (ph.color == gray && c == green)
        dot(ctx, x + 5, 186, 5, C(c, on ? 0.95 : 0.22))
    }

    // ---- 悬浮灯牌 ----
    let cardX: CGFloat = 300, cardY: CGFloat = 118, cardW: CGFloat = 540, cardH: CGFloat = 116
    let quiet = ph.kind == .steady ? 0.62 : 1.0
    ctx.saveGState()
    ctx.setAlpha(CGFloat(quiet))
    ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 34, color: C(0x000000, 0.55).cgColor)
    fillRound(ctx, cardX, cardY, cardW, cardH, 22, C(panel, 0.96))
    ctx.setShadow(offset: .zero, blur: 0, color: nil)
    strokeRound(ctx, cardX, cardY, cardW, cardH, 22, C(panelLine), 1)

    // 灯珠：permission / error 是圆角方形（形状通道，不只靠颜色）
    let dx: CGFloat = cardX + 46, dy: CGFloat = cardY + cardH / 2, dr: CGFloat = 21
    for (rr, aa) in [(44.0, 0.055), (33.0, 0.09), (25.0, 0.16)] {
        dot(ctx, dx, dy, CGFloat(rr), stateColor.withAlphaComponent(CGFloat(aa) * b))
    }
    if ph.square {
        fillRound(ctx, dx - dr, dy - dr, dr * 2, dr * 2, 8, stateColor.withAlphaComponent(b))
    } else {
        dot(ctx, dx, dy, dr, stateColor.withAlphaComponent(b))
    }
    dot(ctx, dx - 6, dy - 7, 5, C(0xFFFFFF, 0.18 * b))

    T("DEFAULT", cardX + 90, cardY + 24, 11, C(muted), .medium, true)
    T(ph.name, cardX + 88, cardY + 46, 27, C(fg), .semibold, true)
    TRight("00:12", cardX + cardW - 26, cardY + 30, 12, C(muted), .regular, true)
    TRight("group · 1 session", cardX + cardW - 26, cardY + 78, 10, C(dim), .regular, true)
    ctx.restoreGState()

    // ---- 中段的空位：一条随状态变化的上报事件流 ----
    // 审批阶段这块地方让给气泡；其余阶段用真实 hook 事件名说明「灯为什么会变」，
    // 顺手避免画面在 IDLE / THINKING 时塌成一大片空白。
    if !ph.bubble {
        let lx: CGFloat = 300, ly: CGFloat = 266
        fillRound(ctx, lx, ly, 2, 68, 1, C(panelLine))
        T("INBOUND", lx + 18, ly + 2, 10, C(dim), .medium, true)
        let event: String
        switch ph.name {
        case "THINKING": event = "hook  UserPromptSubmit"
        case "WORKING":  event = "hook  PreToolUse"
        case "DONE":     event = "hook  Stop"
        default:         event = "hook  —"
        }
        T(event, lx + 18, ly + 22, 12, C(muted), .regular, true)
        let body = ph.name == "IDLE"
            ? "no active session"
            : "POST /v1/state  {\"state\":\"\(ph.name.lowercased())\"}"
        T(body, lx + 18, ly + 43, 12, C(fg, 0.75), .regular, true)
    }

    // ---- 审批气泡：出现 → 光标移入 → 按住 800ms → 收起 ----
    if ph.bubble {
        let a = CGFloat(min(clamp(local / 0.35), clamp((ph.dur - local) / 0.35)))
        if a > 0.01 {
            ctx.saveGState()
            ctx.setAlpha(a)
            let bx: CGFloat = 300, by: CGFloat = 252, bw: CGFloat = 540, bh: CGFloat = 162
            let shownH = bh * (0.86 + 0.14 * a)
            fillRound(ctx, bx, by, bw, shownH, 18, C(panel, 0.98))
            strokeRound(ctx, bx, by, bw, shownH, 18, C(panelLine), 1)

            // 危险横幅
            ctx.saveGState()
            ctx.addPath(roundPath(bx + 1, by + 1, bw - 2, 30, 17))
            ctx.clip()
            fillRound(ctx, bx + 1, by + 1, bw - 2, 34, 17, C(red, 0.92))
            ctx.restoreGState()
            // 警告图标画成路径：等宽字体里的 "!" 看着像字母 I，不用字符凑合
            let warn = CGMutablePath()
            warn.move(to: P(bx + 21, by + 7))
            warn.addLine(to: P(bx + 31, by + 24))
            warn.addLine(to: P(bx + 11, by + 24))
            warn.closeSubpath()
            ctx.addPath(warn)
            ctx.setFillColor(C(0xFFFFFF, 0.92).cgColor)
            ctx.fillPath()
            // 三角里镂一个感叹号：不这么做，纯白三角看着像播放按钮
            ctx.setFillColor(C(red, 0.92).cgColor)
            ctx.fill(R(bx + 20, by + 12, 2.2, 6))
            dot(ctx, bx + 21.1, by + 20.3, 1.3, C(red, 0.92))
            T("DESTRUCTIVE  ·  HOLD 800MS", bx + 40, by + 8, 11, C(0xFFFFFF), .bold, true)

            T("$ rm -rf node_modules", bx + 20, by + 44, 16, C(0xFFD9D6), .medium, true)
            T("@ ~/src/my-project", bx + 20, by + 70, 11, C(muted), .regular, true)

            // 两枚按钮只有符号，不写自然语言
            let denyX = bx + bw - 186, allowX = bx + bw - 96, btnY = by + 104
            fillRound(ctx, denyX, btnY, 78, 34, 9, C(0x2A3342))
            T("✕", denyX + 33, btnY + 7, 17, C(muted), .bold)
            fillRound(ctx, allowX, btnY, 78, 34, 9, C(green, 0.92))
            T("✓", allowX + 33, btnY + 7, 17, C(0xFFFFFF), .bold)

            // 按住进度环
            let hold = clamp((local - 1.05) / 0.8)
            if hold > 0 {
                let c = CGPoint(x: allowX + 39, y: H - (btnY + 17))
                ctx.setStrokeColor(C(0xFFFFFF, 0.95).cgColor)
                ctx.setLineWidth(3)
                ctx.addArc(center: c, radius: 24, startAngle: -.pi / 2,
                           endAngle: -.pi / 2 + 2 * .pi * CGFloat(hold), clockwise: false)
                ctx.strokePath()
            }

            // 光标从右侧滑到「允许」上
            let move = CGFloat(clamp((local - 0.45) / 0.6))
            let cx = (bx + bw - 30) + (allowX + 56 - (bx + bw - 30)) * move
            let cy = (by + 132) + (btnY + 16 - (by + 132)) * move
            cursor(ctx, cx, cy)
            ctx.restoreGState()
        }
    }

    // ---- 底部时间轴 ----
    let segY: CGFloat = 428, gap: CGFloat = 8
    let segW = (W - 72 - gap * CGFloat(phases.count - 1)) / CGFloat(phases.count)
    for (i, p) in phases.enumerated() {
        let x = 36 + CGFloat(i) * (segW + gap)
        let on = i == idx
        fillRound(ctx, x, segY, segW, 4, 2, on ? C(p.color, 0.95) : C(panelLine, 0.85))
        T(p.name, x, segY + 12, 9, on ? C(muted) : C(dim), on ? .medium : .regular, true)
    }

    NSGraphicsContext.restoreGraphicsState()
    return rep
}

// MARK: - 输出：只画帧
//
// ffmpeg **不在这里启动**：Swift 解释器在受限环境里 fork 子进程会被系统终止。
// 编排交给 `scripts/render-demo.sh`（临时目录 → 两遍合成 → 清理），本脚本只负责
// 把帧画到参数指定的目录。帧率写进 `fps` 文件让 shell 读，避免两处各写一份 12。

let framesDir = CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : "docs/assets/.frames"
try FileManager.default.createDirectory(atPath: framesDir, withIntermediateDirectories: true)
let dir = framesDir as NSString
try String(FPS).write(toFile: dir.appendingPathComponent("fps"), atomically: true, encoding: .utf8)

let frames = Int((total * Double(FPS)).rounded())
print("rendering \(frames) frames @ \(FPS)fps  (\(String(format: "%.1f", total))s loop)", terminator: "")
fflush(stdout)
for i in 0..<frames {
    // 解释器没有 run loop，位图会一直攒在 autorelease 池里 —— 115 帧能吃掉上 GB。
    autoreleasepool {
        let t = Double(i) / Double(FPS)
        let png = drawFrame(t).representation(using: .png, properties: [:])!
        let path = dir.appendingPathComponent(String(format: "f%04d.png", i))
        try! png.write(to: URL(fileURLWithPath: path))
    }
    if i % 20 == 0 { print(".", terminator: ""); fflush(stdout) }
}
print(" done → \(framesDir)")
