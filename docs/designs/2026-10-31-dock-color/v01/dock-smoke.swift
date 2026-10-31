// Standalone visual check; never starts Crossbow or touches its network/configuration.
import AppKit

@MainActor
final class ProbeDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let url = Bundle.main.url(forResource: "icon", withExtension: "icns"),
              let data = try? Data(contentsOf: url), let icon = NSImage(data: data) else {
            NSApplication.shared.terminate(nil)
            return
        }
        NSApplication.shared.applicationIconImage = icon
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 420, height: 300),
            styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Crossbow 图标色彩验证"
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 420, height: 300))
        let image = NSImageView(frame: NSRect(x: 146, y: 135, width: 128, height: 128))
        image.image = icon
        image.imageScaling = .scaleProportionallyUpOrDown
        content.addSubview(image)
        let label = NSTextField(labelWithString: "#3F52CF · B1 原色")
        label.alignment = .center
        label.font = .systemFont(ofSize: 17, weight: .medium)
        label.frame = NSRect(x: 40, y: 85, width: 340, height: 28)
        content.addSubview(label)
        let hint = NSTextField(labelWithString: "请比较窗口图标与 Dock 中新增的测试图标")
        hint.alignment = .center
        hint.textColor = .secondaryLabelColor
        hint.frame = NSRect(x: 20, y: 42, width: 380, height: 24)
        content.addSubview(hint)
        window.contentView = content
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
@main
enum ProbeMain {
    @MainActor
    static func main() {
        let app = NSApplication.shared
        let delegate = ProbeDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        // AppKit's delegate property is weak; retain it through the blocking loop.
        withExtendedLifetime(delegate) { app.run() }
    }
}
