import AppKit

// Reproducible artwork in the Anywhere family: code brackets on a desktop display.
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let resources = root.appending(path: "Resources")
let iconset = resources.appending(path: "AppIcon.iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

func render(pixels: Int) throws -> Data {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    let context = NSGraphicsContext(bitmapImageRep: bitmap)!
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    context.cgContext.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)

    let tile = NSBezierPath(roundedRect: NSRect(x: 64, y: 64, width: 896, height: 896), xRadius: 200, yRadius: 200)
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.28)
    shadow.shadowBlurRadius = 28
    shadow.shadowOffset = NSSize(width: 0, height: -12)
    shadow.set()
    NSColor(white: 0.08, alpha: 1).setFill()
    tile.fill()
    NSGraphicsContext.restoreGraphicsState()
    NSGradient(colors: [NSColor(white: 0.21, alpha: 1), NSColor(white: 0.055, alpha: 1)])!
        .draw(in: tile, angle: -90)
    NSColor(white: 1, alpha: 0.13).setStroke()
    tile.lineWidth = 2
    tile.stroke()

    let screen = NSBezierPath(roundedRect: NSRect(x: 222, y: 345, width: 580, height: 400), xRadius: 40, yRadius: 40)
    NSColor(white: 0.96, alpha: 1).setStroke()
    screen.lineWidth = 30
    screen.stroke()

    func line(_ points: [NSPoint], width: CGFloat) {
        let path = NSBezierPath()
        path.move(to: points[0])
        for point in points.dropFirst() { path.line(to: point) }
        path.lineWidth = width
        path.lineCapStyle = .round
        path.lineJoinStyle = .round
        path.stroke()
    }
    line([NSPoint(x: 443, y: 636), NSPoint(x: 352, y: 545), NSPoint(x: 443, y: 454)], width: 35)
    line([NSPoint(x: 581, y: 636), NSPoint(x: 672, y: 545), NSPoint(x: 581, y: 454)], width: 35)
    line([NSPoint(x: 512, y: 340), NSPoint(x: 512, y: 265)], width: 30)
    line([NSPoint(x: 408, y: 257), NSPoint(x: 616, y: 257)], width: 30)
    return bitmap.representation(using: .png, properties: [:])!
}

for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let suffix = scale == 2 ? "@2x" : ""
        try render(pixels: size * scale).write(to: iconset.appending(path: "icon_\(size)x\(size)\(suffix).png"))
    }
}
try render(pixels: 1024).write(to: resources.appending(path: "AppIcon.png"))
let process = Process()
process.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
process.arguments = ["-c", "icns", "-o", resources.appending(path: "AppIcon.icns").path, iconset.path]
try process.run()
process.waitUntilExit()
guard process.terminationStatus == 0 else { fatalError("iconutil failed") }
print("Rendered AppIcon.png and AppIcon.icns")
