import AppKit
import Carbon
import NotchCore

/// Registers one system-wide hot key with the window server. Carbon's RegisterEventHotKey: no event tap, so no Input
/// Monitoring prompt, and it is delivered while another app has secure input on.
@MainActor
protocol HotKeyRegistering: AnyObject {
    /// Registers the combination (replacing any registered one); nil on success, else why not.
    func register(_ combo: KeyCombo, pressed: @escaping () -> Void) -> String?
    func unregister()
}

/// The owner's shortcut to open FERB (D24): registered at launch and on each change, unregistered while Settings
/// records a new one (so pressing the current one records it instead of toggling FERB). A registration that fails
/// (another app holds the combination) shows in Settings.
@MainActor
final class HotKeyCenter: ObservableObject {
    @Published private(set) var combo: KeyCombo?
    /// Why the saved combination couldn't be registered, or nil.
    @Published private(set) var failure: String?
    @Published private(set) var recording = false
    /// Pressing the shortcut (PanelCoordinator: the same path as double-tap right Option).
    var onPressed: (() -> Void)?

    private let defaults: UserDefaults
    private let registrar: HotKeyRegistering
    private var started = false

    init(defaults: UserDefaults = .standard, registrar: HotKeyRegistering? = nil) {
        self.defaults = defaults
        self.registrar = registrar ?? CarbonHotKeys()
        combo = KeyCombo.saved(in: defaults)
    }

    func start() {
        started = true
        apply()
    }

    func stop() {
        started = false
        registrar.unregister()
    }

    /// Saves a new combination (nil: none, the default) and registers it.
    func set(_ new: KeyCombo?) {
        guard new?.problem == nil else { return }
        KeyCombo.save(new, in: defaults)
        combo = new
        recording = false
        apply()
    }

    func beginRecording() {
        recording = true
        registrar.unregister()
    }

    func endRecording() {
        recording = false
        apply()
    }

    /// macOS's own shortcuts this combination matches (Settings warns, never blocks).
    func systemConflicts(_ combo: KeyCombo) -> [String] {
        SystemShortcuts.conflicts(combo, in: SystemHotKeysFile.read())
    }

    private func apply() {
        registrar.unregister()
        failure = nil
        guard started, !recording, let combo else { return }
        failure = registrar.register(combo) { [weak self] in self?.onPressed?() }
        if let failure { NSLog("FERB: the shortcut %@ couldn't be registered: %@", combo.display, failure) }
    }
}

/// `com.apple.symbolichotkeys`'s AppleSymbolicHotKeys as macOS keeps it (read only).
enum SystemHotKeysFile {
    static func read() -> [String: Any] {
        CFPreferencesCopyAppValue("AppleSymbolicHotKeys" as CFString, "com.apple.symbolichotkeys" as CFString)
            as? [String: Any] ?? [:]
    }
}

/// The real registrar: one EventHotKeyRef, registered exclusively so another app holding the same combination
/// exclusively is reported rather than silently shared.
@MainActor
final class CarbonHotKeys: HotKeyRegistering {
    private static let signature: OSType = 0x4645_5242          // 'FERB'
    private var reference: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private var pressed: (() -> Void)?

    func register(_ combo: KeyCombo, pressed: @escaping () -> Void) -> String? {
        unregister()
        installHandler()
        self.pressed = pressed
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(combo.keyCode, combo.modifiers, EventHotKeyID(signature: Self.signature, id: 1),
                                         GetApplicationEventTarget(), OptionBits(kEventHotKeyExclusive), &ref)
        guard status == noErr, let ref else {
            return status == eventHotKeyExistsErr ? "Another app already uses \(combo.display)."
                                                  : "macOS refused \(combo.display) (error \(status))."
        }
        reference = ref
        return nil
    }

    func unregister() {
        if let reference { UnregisterEventHotKey(reference) }
        reference = nil
    }

    private func installHandler() {
        guard handler == nil else { return }
        var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let context = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, event, context in
            guard let event, let context else { return OSStatus(eventNotHandledErr) }
            var id = EventHotKeyID()
            let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                                           nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            guard status == noErr, id.signature == CarbonHotKeys.signature else { return OSStatus(eventNotHandledErr) }
            let keys = Unmanaged<CarbonHotKeys>.fromOpaque(context).takeUnretainedValue()
            DispatchQueue.main.async { MainActor.assumeIsolated { keys.pressed?() } }
            return noErr
        }, 1, &type, context, &handler)
    }
}

/// The character a key types on the keyboard layout in use now (UCKeyTranslate), for showing shortcuts as the
/// owner's keyboard labels them; KeyCombo falls back to the US layout when it can't be read.
enum KeyboardLayout {
    static func character(_ keyCode: UInt32) -> String? {
        guard let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
              let pointer = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) else { return nil }
        let data = Unmanaged<CFData>.fromOpaque(pointer).takeUnretainedValue() as Data
        return data.withUnsafeBytes { raw -> String? in
            guard let layout = raw.baseAddress?.assumingMemoryBound(to: UCKeyboardLayout.self) else { return nil }
            var dead: UInt32 = 0
            var chars = [UniChar](repeating: 0, count: 4)
            var length = 0
            let status = UCKeyTranslate(layout, UInt16(keyCode), UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()),
                                        OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, chars.count, &length, &chars)
            guard status == noErr, length > 0 else { return nil }
            return String(utf16CodeUnits: chars, count: length)
        }
    }
}
