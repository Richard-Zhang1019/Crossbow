// Native render proof, not a screenshot of the user's Dock. No app/network control.
import AppKit
import ImageIO

let oldPath = CommandLine.arguments[1]
let icnsPath = CommandLine.arguments[2]
let outputPath = CommandLine.arguments[3]
guard let source = NSImage(data: try Data(contentsOf: URL(fileURLWithPath: icnsPath))) else {
    fatalError("Cannot decode the supplied ICNS")
}
let oldImage = NSImage(contentsOfFile: oldPath)!
let notesImage = NSWorkspace.shared.icon(forFile: "/System/Applications/Notes.app")
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
app.applicationIconImage = source
let runtimeImage = app.applicationIconImage!
let scale: CGFloat = 2
let width = 900, height = 230
let bitmap = CGContext(data: nil, width: width * 2, height: height * 2,
    bitsPerComponent: 8, bytesPerRow: width * 8,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
bitmap.scaleBy(x: scale, y: scale)
let context = NSGraphicsContext(cgContext: bitmap, flipped: false)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = context
NSColor(srgbRed: 0.94, green: 0.95, blue: 0.97, alpha: 1).setFill()
NSRect(x: 0, y: 0, width: width, height: height).fill()
let items = [(oldImage, "修复前 · 近满画布"), (notesImage, "系统备忘录 · 尺寸参照"), (runtimeImage, "修复后 · Dock 专用留白")]
let paragraph = NSMutableParagraphStyle()
paragraph.alignment = .center
for (index, item) in items.enumerated() {
    let x = CGFloat(index * 300)
    item.0.draw(in: NSRect(x: x + 102, y: 100, width: 96, height: 96),
        from: .zero, operation: .sourceOver, fraction: 1)
    (item.1 as NSString).draw(in: NSRect(x: x + 8, y: 48, width: 284, height: 28),
        withAttributes: [.font: NSFont.systemFont(ofSize: 16, weight: .medium),
            .foregroundColor: NSColor(srgbRed: 0.16, green: 0.2, blue: 0.27, alpha: 1),
            .paragraphStyle: paragraph])
}
context.flushGraphics()
NSGraphicsContext.restoreGraphicsState()
let destination = CGImageDestinationCreateWithURL(URL(fileURLWithPath: outputPath) as CFURL, "public.png" as CFString, 1, nil)!
CGImageDestinationAddImage(destination, bitmap.makeImage()!, nil)
precondition(CGImageDestinationFinalize(destination), "Cannot write the native render proof")
