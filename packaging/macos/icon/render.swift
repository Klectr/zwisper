// Renders the Zwisper app icon at 1024×1024 on Apple's macOS icon grid
// (824 px body, 100 px margin for the shadow).
//
//   swiftc -O -o /tmp/render-icon packaging/macos/icon/render.swift
//   /tmp/render-icon <out.png> [size]      (size defaults to 1024)
//
// Every size is drawn natively. At 32 px and below the mark is simplified
// (bigger, thicker bars, no glow) so the four bars stay distinct; a shrunken
// 1024 render turns into a blur there. make-icns.sh builds AppIcon.icns.
import AppKit

let out = URL(fileURLWithPath: CommandLine.arguments[1])
let px = CommandLine.arguments.count > 2 ? Int(CommandLine.arguments[2])! : 1024
let small = px <= 32
let S: CGFloat = 1024                       // design units; scaled to `px` below
func hex(_ v: UInt32, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255, blue: CGFloat(v & 0xff) / 255, alpha: a)
}
// Catppuccin Mocha
let crust = hex(0x11111b), mantle = hex(0x181825), surface0 = hex(0x313244)
let mauve = hex(0xcba6f7), blue = hex(0x89b4fa), lavender = hex(0xb4befe)

/// A continuous-corner ("squircle") rounded rect, closer to Apple's icon shape than
/// a plain circular-arc rounded rect.
func squircle(_ r: CGRect, radius: CGFloat) -> NSBezierPath {
    let p = NSBezierPath()
    let k: CGFloat = 1.28   // how far the curve reaches along each edge, in radii
    let c: CGFloat = 0.55   // control-point pull
    let (x0, y0, x1, y1) = (r.minX, r.minY, r.maxX, r.maxY)
    let e = radius * k
    p.move(to: NSPoint(x: x0 + e, y: y0))
    p.line(to: NSPoint(x: x1 - e, y: y0))
    p.curve(to: NSPoint(x: x1, y: y0 + e), controlPoint1: NSPoint(x: x1 - e * (1 - c), y: y0), controlPoint2: NSPoint(x: x1, y: y0 + e * (1 - c)))
    p.line(to: NSPoint(x: x1, y: y1 - e))
    p.curve(to: NSPoint(x: x1 - e, y: y1), controlPoint1: NSPoint(x: x1, y: y1 - e * (1 - c)), controlPoint2: NSPoint(x: x1 - e * (1 - c), y: y1))
    p.line(to: NSPoint(x: x0 + e, y: y1))
    p.curve(to: NSPoint(x: x0, y: y1 - e), controlPoint1: NSPoint(x: x0 + e * (1 - c), y: y1), controlPoint2: NSPoint(x: x0, y: y1 - e * (1 - c)))
    p.line(to: NSPoint(x: x0, y: y0 + e))
    p.curve(to: NSPoint(x: x0 + e, y: y0), controlPoint1: NSPoint(x: x0, y: y0 + e * (1 - c)), controlPoint2: NSPoint(x: x0 + e * (1 - c), y: y0))
    p.close()
    return p
}

let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
let ctx = NSGraphicsContext(bitmapImageRep: rep)!
NSGraphicsContext.current = ctx
let cg = ctx.cgContext
cg.scaleBy(x: CGFloat(px) / S, y: CGFloat(px) / S)

let body = CGRect(x: 100, y: 100, width: 824, height: 824)
let shape = squircle(body, radius: 185)

// Drop shadow (sits in the 100 px margin, as Apple's template does). Skipped when
// small, where it only reads as a grey halo.
cg.saveGState()
if !small {
cg.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: NSColor.black.withAlphaComponent(0.38).cgColor)
}
crust.setFill(); shape.fill()
cg.restoreGState()

// Body: deep Mocha gradient, lighter at the top.
cg.saveGState()
shape.addClip()
NSGradient(colors: [hex(0x2b2b42), hex(0x1e1e2e), crust], atLocations: [0, 0.45, 1], colorSpace: .sRGB)!
    .draw(in: body, angle: -90)
// Soft mauve/blue glow behind the bars (dropped when small: it just muddies).
if !small { NSGradient(colors: [mauve.withAlphaComponent(0.30), blue.withAlphaComponent(0.10), mauve.withAlphaComponent(0)],
           atLocations: [0, 0.45, 1], colorSpace: .sRGB)!
    .draw(fromCenter: NSPoint(x: S / 2, y: S / 2 - 20), radius: 0, toCenter: NSPoint(x: S / 2, y: S / 2 - 20), radius: 430, options: []) }
cg.restoreGState()

// Edge: faint light rim at the top fading out, hairline all round.
cg.saveGState()
shape.addClip()
let rim = squircle(body.insetBy(dx: 2, dy: 2), radius: 183)
rim.lineWidth = 4
NSColor.white.withAlphaComponent(0.10).setStroke(); rim.stroke()
cg.restoreGState()
surface0.withAlphaComponent(0.9).setStroke()
shape.lineWidth = 2; shape.stroke()

// The mark: four waveform bars (same proportions as the site logo, on a 32-unit grid),
// scaled into the middle of the body.
let grid: CGFloat = (small ? 0.98 : 0.78) * 824 / 32   // units → px; fills more when small
let ox = S / 2 - 16 * grid, oy = S / 2 - 16 * grid
let tiny = px <= 16   // ~13 px of body: four bars can't keep whole-pixel gaps, so use three
let bars: [(x: CGFloat, y: CGFloat, h: CGFloat)] = tiny
    ? [(4.5, 11, 10), (13, 5, 22), (21.5, 9, 14)]
    : [(7, 13, 6), (12, 9, 14), (17, 6, 20), (22, 11, 10)]
let barW: CGFloat = tiny ? 6 : small ? 4.2 : 3.2

// Glow under the bars.
if !small {
cg.saveGState()
cg.setShadow(offset: .zero, blur: 60, color: mauve.withAlphaComponent(0.55).cgColor)
for b in bars {
    let r = NSRect(x: ox + (b.x - 0.1) * grid, y: oy + (32 - b.y - b.h) * grid, width: barW * grid, height: b.h * grid)
    mauve.setFill(); NSBezierPath(roundedRect: r, xRadius: barW * grid / 2, yRadius: barW * grid / 2).fill()
}
cg.restoreGState()
}

// Bars with a lavender → mauve → blue gradient.
for b in bars {
    let r = NSRect(x: ox + (b.x - 0.1) * grid, y: oy + (32 - b.y - b.h) * grid, width: barW * grid, height: b.h * grid)
    let path = NSBezierPath(roundedRect: r, xRadius: barW * grid / 2, yRadius: barW * grid / 2)
    cg.saveGState()
    path.addClip()
    NSGradient(colors: [lavender, mauve, blue], atLocations: [0, 0.5, 1], colorSpace: .sRGB)!
        .draw(in: NSRect(x: ox, y: oy + 5 * grid, width: 32 * grid, height: 22 * grid), angle: -90)
    cg.restoreGState()
}

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: out)
print("wrote", out.path)
