// Renders Trav's app icon and menu bar glyph (run via `make icons`).
//
//   swift scripts/app-icon.swift <icon.png> <glyph.png>
//
// icon.png:  1024×1024 on Apple's icon grid (824 px tile, 100 px margins):
//            an ink tile lit from above, a glowing "T" whose stem is a download
//            arrow, and a progress bar beneath.
// glyph.png: the same mark in black on transparent, for the menu bar (macOS tints it).
import AppKit
import CoreGraphics

let args = CommandLine.arguments
guard args.count == 3 else {
    FileHandle.standardError.write(Data("usage: swift scripts/app-icon.swift <icon.png> <glyph.png>\n".utf8))
    exit(2)
}

let size: CGFloat = 1024
let space = CGColorSpace(name: CGColorSpace.displayP3)!

func canvas() -> CGContext {
    let ctx = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
                        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    // Top-left origin, like the coordinates below.
    ctx.translateBy(x: 0, y: size)
    ctx.scaleBy(x: 1, y: -1)
    return ctx
}

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: space, components: [
        CGFloat((hex >> 16) & 255) / 255, CGFloat((hex >> 8) & 255) / 255, CGFloat(hex & 255) / 255, alpha,
    ])!
}

func save(_ ctx: CGContext, to path: String) {
    let png = NSBitmapImageRep(cgImage: ctx.makeImage()!).representation(using: .png, properties: [:])!
    try! png.write(to: URL(fileURLWithPath: path))
}

func fill(_ ctx: CGContext, _ path: CGPath, gradient colors: [CGColor], _ locations: [CGFloat], from: CGPoint, to: CGPoint) {
    ctx.saveGState()
    ctx.addPath(path)
    ctx.clip()
    let g = CGGradient(colorsSpace: space, colors: colors as CFArray, locations: locations)!
    ctx.drawLinearGradient(g, start: from, end: to, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()
}

/// The "T" whose stem ends in a download arrow, as one shape.
let mark: CGPath = {
    let bar = CGPath(roundedRect: CGRect(x: 282, y: 268, width: 460, height: 104), cornerWidth: 52, cornerHeight: 52, transform: nil)
    let stem = CGPath(rect: CGRect(x: 460, y: 330, width: 104, height: 270), transform: nil)
    let head = CGMutablePath()
    head.move(to: CGPoint(x: 330, y: 560))
    head.addLine(to: CGPoint(x: 694, y: 560))
    head.addLine(to: CGPoint(x: 512, y: 752))
    head.closeSubpath()
    let rounded = head.union(head.copy(strokingWithWidth: 44, lineCap: .round, lineJoin: .round, miterLimit: 4))
    return bar.union(stem).union(rounded)
}()
let track = CGPath(roundedRect: CGRect(x: 300, y: 812, width: 424, height: 34), cornerWidth: 17, cornerHeight: 17, transform: nil)
let progress = CGPath(roundedRect: CGRect(x: 300, y: 812, width: 263, height: 34), cornerWidth: 17, cornerHeight: 17, transform: nil)

// MARK: App icon

do {
    let ctx = canvas()
    let tileRect = CGRect(x: 100, y: 100, width: 824, height: 824)
    let tile = CGPath(roundedRect: tileRect, cornerWidth: 185, cornerHeight: 185, transform: nil)

    // Soft shadow under the tile, as in Apple's template.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 14), blur: 28, color: color(0x000000, 0.32))
    ctx.addPath(tile)
    ctx.setFillColor(color(0x000000))
    ctx.fillPath()
    ctx.restoreGState()

    // Ink, lit from above, with a warm glow behind the mark.
    fill(ctx, tile, gradient: [color(0x2A2D35), color(0x15171C), color(0x0B0C0F)], [0, 0.55, 1],
         from: CGPoint(x: 512, y: 100), to: CGPoint(x: 512, y: 924))
    ctx.saveGState()
    ctx.addPath(tile)
    ctx.clip()
    let glow = CGGradient(colorsSpace: space, colors: [color(0xFF8A3D, 0.38), color(0xFF8A3D, 0)] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 512, y: 520), startRadius: 0,
                           endCenter: CGPoint(x: 512, y: 520), endRadius: 360, options: [])
    ctx.restoreGState()

    // The mark: an ember gradient with a glow and a glassy upper highlight.
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: 46, color: color(0xFF7A2F, 0.75))
    ctx.addPath(mark)
    ctx.setFillColor(color(0xFF8A3D))
    ctx.fillPath()
    ctx.restoreGState()
    fill(ctx, mark, gradient: [color(0xFFD08A), color(0xFF9A47), color(0xFF6A2B)], [0, 0.45, 1],
         from: CGPoint(x: 512, y: 268), to: CGPoint(x: 512, y: 760))
    ctx.saveGState()
    ctx.addPath(mark)
    ctx.clip()
    fill(ctx, CGPath(rect: CGRect(x: 0, y: 0, width: size, height: 470), transform: nil),
         gradient: [color(0xFFFFFF, 0.38), color(0xFFFFFF, 0)], [0, 1], from: CGPoint(x: 512, y: 268), to: CGPoint(x: 512, y: 470))
    ctx.restoreGState()

    // Progress bar.
    ctx.addPath(track)
    ctx.setFillColor(color(0xFFFFFF, 0.12))
    ctx.fillPath()
    fill(ctx, progress, gradient: [color(0xFFB466), color(0xFF7A2F)], [0, 1], from: CGPoint(x: 300, y: 0), to: CGPoint(x: 563, y: 0))

    // Hairline rim light along the top edge.
    ctx.saveGState()
    ctx.addPath(tile)
    ctx.clip()
    ctx.addPath(CGPath(roundedRect: tileRect.insetBy(dx: 1.5, dy: 1.5), cornerWidth: 184, cornerHeight: 184, transform: nil))
    ctx.setLineWidth(3)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    fill(ctx, CGPath(rect: CGRect(x: 0, y: 0, width: size, height: size), transform: nil),
         gradient: [color(0xFFFFFF, 0.22), color(0xFFFFFF, 0)], [0, 1], from: CGPoint(x: 512, y: 100), to: CGPoint(x: 512, y: 520))
    ctx.restoreGState()

    save(ctx, to: args[1])
}

// MARK: Menu bar glyph

do {
    let ctx = canvas()
    let bounds = mark.boundingBox.union(track.boundingBox)
    let scale = size * 0.8 / bounds.height
    ctx.translateBy(x: size / 2, y: size * 0.1)
    ctx.scaleBy(x: scale, y: scale)
    ctx.translateBy(x: -bounds.midX, y: -bounds.minY)
    ctx.addPath(mark)
    ctx.addPath(track)
    ctx.setFillColor(color(0x000000))
    ctx.fillPath()
    save(ctx, to: args[2])
}
