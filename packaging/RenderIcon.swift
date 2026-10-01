import AppKit
import Foundation

let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    for retina in [false, true] {
        let pixels = size * (retina ? 2 : 1)
        let image = NSImage(size: NSSize(width: pixels, height: pixels))
        image.lockFocus()
        let factor = CGFloat(pixels) / 1024
        NSGraphicsContext.current!.cgContext.scaleBy(x: factor, y: factor)
        let background = NSBezierPath(roundedRect: NSRect(x: 72, y: 72, width: 880, height: 880), xRadius: 190, yRadius: 190)
        NSGradient(starting: NSColor(calibratedRed: 0.14, green: 0.22, blue: 0.31, alpha: 1),
            ending: NSColor(calibratedRed: 0.04, green: 0.08, blue: 0.14, alpha: 1))!.draw(in: background, angle: -90)
        NSColor.white.withAlphaComponent(0.94).setStroke()
        let display = NSBezierPath(roundedRect: NSRect(x: 210, y: 310, width: 604, height: 410), xRadius: 50, yRadius: 50)
        display.lineWidth = 32; display.stroke()
        NSColor(calibratedRed: 0.98, green: 0.31, blue: 0.26, alpha: 1).setFill()
        NSBezierPath(ovalIn: NSRect(x: 422, y: 425, width: 180, height: 180)).fill()
        NSColor.white.withAlphaComponent(0.94).setStroke()
        let base = NSBezierPath()
        base.move(to: NSPoint(x: 352, y: 240)); base.line(to: NSPoint(x: 672, y: 240))
        base.lineWidth = 30; base.lineCapStyle = .round; base.stroke()
        image.unlockFocus()
        let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
        let suffix = retina ? "@2x" : ""
        try bitmap.representation(using: .png, properties: [:])!.write(to: directory.appendingPathComponent("icon_\(size)x\(size)\(suffix).png"))
    }
}
