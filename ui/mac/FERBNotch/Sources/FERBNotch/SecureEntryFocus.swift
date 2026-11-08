import AppKit

/// A password-style field (an API key, a sign-in code) gets the keyboard only when the owner puts it there: a click in
/// that field or Tab (APP-794).
///
/// While a secure text field holds the keyboard in the active app, macOS turns on secure event input for the whole
/// Mac, and every other app's keyboard watcher (an event tap or a global key monitor) stops seeing keys. Owner,
/// 2026-11-08: "another bug i noticed, i cant open my onemeny clipboard when settings window is open". That clipboard
/// app opens on a key watched through an event tap; the window server logged FERB in front with "secureTextInput is
/// active" (08:52:39, 08:53:51) while the Settings window was open, because opening the window or a page gave its
/// first field, the API key's secure field, the keyboard by itself. APP-790's menu watch only lists FERB's own windows
/// (`NSApp.windows`) and never touches another app's window or keys.
@MainActor
enum SecureEntryFocus {
    /// The event being handled (tests stand in their own).
    static var currentEvent: () -> NSEvent? = { NSApp.currentEvent }
    static let tabKey: UInt16 = 48

    /// The secure field a would-be first responder stands for: the field itself, or its field editor.
    static func secureField(_ responder: NSResponder?) -> NSSecureTextField? {
        if let field = responder as? NSSecureTextField { return field }
        if let editor = responder as? NSTextView, editor.isFieldEditor { return editor.delegate as? NSSecureTextField }
        return nil
    }

    /// Whether the owner put the keyboard in `field` now: a press inside it, or Tab, in its window.
    static func ownerChose(_ field: NSSecureTextField, in window: NSWindow, event: NSEvent?) -> Bool {
        guard let event, event.window === window else { return false }
        switch event.type {
        case .leftMouseDown, .rightMouseDown, .otherMouseDown:
            return field.bounds.contains(field.convert(event.locationInWindow, from: nil))
        case .keyDown:
            return event.keyCode == tabKey
        default:
            return false
        }
    }
}

/// FERB's own app window (Settings, Tasks, Memory…): a secure field never takes the keyboard by itself (APP-794).
final class FERBAppWindow: NSWindow {
    override func makeFirstResponder(_ responder: NSResponder?) -> Bool {
        if let field = SecureEntryFocus.secureField(responder), firstResponder !== responder,
           !SecureEntryFocus.ownerChose(field, in: self, event: SecureEntryFocus.currentEvent()) {
            return false
        }
        return super.makeFirstResponder(responder)
    }
}
