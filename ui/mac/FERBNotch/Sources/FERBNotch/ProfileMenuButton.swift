import AppKit
import SwiftUI

/// Keep the profile glyph outside NSPopUpButton/SwiftUI Menu's intrinsic layout.
/// Lazy menu creation must never change the header's width or image position.
struct ProfileMenuButton: NSViewRepresentable {
    let model: NotchModel

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeNSView(context: Context) -> NSButton {
        let button = NSButton(frame: NSRect(x: 0, y: 0, width: 26, height: 26))
        button.setButtonType(.momentaryChange)
        button.isBordered = false
        button.title = ""
        button.imagePosition = .imageOnly
        button.imageScaling = .scaleNone
        button.image = NSImage(systemSymbolName: "person.crop.circle.fill", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 18, weight: .regular))
        button.image?.size = NSSize(width: 18, height: 18)
        button.contentTintColor = .white.withAlphaComponent(0.65)
        button.focusRingType = .none
        button.toolTip = "Profile and options"
        button.setAccessibilityLabel("Profile and options")
        button.target = context.coordinator
        button.action = #selector(Coordinator.showMenu(_:))
        return button
    }

    func updateNSView(_ button: NSButton, context: Context) {}

    @MainActor
    final class Coordinator: NSObject {
        private let model: NotchModel
        init(model: NotchModel) { self.model = model }

        @objc func showMenu(_ button: NSButton) {
            let menu = NSMenu()
            menu.autoenablesItems = false
            add("Open app window", to: menu) { [model] in model.onOpenWindow?() }
            add("Conversation history", to: menu, action: model.history)
            add("Settings…", to: menu, key: ",") { [model] in model.onSettings?() }
            if model.session.phase == .result {
                menu.addItem(.separator())
                add("Reveal in Finder", to: menu, action: model.revealDocument)
            }
            if !model.runningTasks.isEmpty {
                menu.addItem(.separator())
                for (taskID, goal) in model.runningTasks.sorted(by: { $0.key < $1.key }) {
                    add("Running: " + goal, to: menu) { [model] in model.selectRunningTask(taskID) }
                }
            }
            menu.addItem(.separator())
            let connection = NSMenuItem(title: model.connectionLine, action: nil, keyEquivalent: "")
            let connectionMenu = NSMenu()
            add("Reconnect to FERB", to: connectionMenu) { [model] in
                model.disconnectDaemon()
                model.connectDaemon()
            }
            connection.submenu = connectionMenu
            menu.addItem(connection)
            menu.addItem(.separator())
            add("Hide FERB", to: menu) { [model] in model.collapse() }
            add("Quit FERB", to: menu) { NSApplication.shared.terminate(nil) }
            model.setProfileMenuOpen(true)
            defer { model.setProfileMenuOpen(false) }
            menu.popUp(positioning: nil, at: NSPoint(x: 0, y: -4), in: button)
        }

        private func add(_ title: String, to menu: NSMenu, enabled: Bool = true,
                         key: String = "", action: @escaping () -> Void) {
            let command = MenuCommand(action)
            let item = NSMenuItem(title: title, action: #selector(MenuCommand.run), keyEquivalent: key)
            item.target = command
            item.representedObject = command // NSMenuItem's target is weak.
            item.isEnabled = enabled
            menu.addItem(item)
        }
    }
}

@MainActor
private final class MenuCommand: NSObject {
    private let action: () -> Void
    init(_ action: @escaping () -> Void) { self.action = action }
    @objc func run() { action() }
}
