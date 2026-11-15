import Foundation

/// The owner's keyboard shortcut to open FERB (APP-818: any key combination with ⌘, ⌃ or ⌥, no default):
/// a virtual key code and Carbon modifiers, as `RegisterEventHotKey` takes them. Saved under one defaults key.
public struct KeyCombo: Equatable, Hashable {
    public let keyCode: UInt32
    public let modifiers: UInt32

    // Carbon's modifier bits (Events.h: cmdKey, shiftKey, optionKey, controlKey).
    public static let command: UInt32 = 1 << 8
    public static let shift: UInt32 = 1 << 9
    public static let option: UInt32 = 1 << 11
    public static let control: UInt32 = 1 << 12
    public static let allModifiers = command | shift | option | control

    public init(keyCode: UInt32, modifiers: UInt32) {
        self.keyCode = keyCode
        self.modifiers = modifiers & Self.allModifiers
    }

    /// From AppKit's device-independent modifier flags (NSEvent.ModifierFlags raw values).
    public init(keyCode: UInt16, cocoaModifiers flags: UInt) {
        var carbon: UInt32 = 0
        if flags & (1 << 20) != 0 { carbon |= Self.command }
        if flags & (1 << 17) != 0 { carbon |= Self.shift }
        if flags & (1 << 19) != 0 { carbon |= Self.option }
        if flags & (1 << 18) != 0 { carbon |= Self.control }
        self.init(keyCode: UInt32(keyCode), modifiers: carbon)
    }

    // MARK: Validation

    public enum Problem: Equatable {
        /// APP-818: the combination must hold ⌘, ⌃ or ⌥ (a plain or Shift-only key would take a letter from typing).
        case needsModifier
        /// One of FERB's own keys (named): the notch's and window's ⌘, ⌘W ⌘Q ⌘M ⌘R, editing ⌘C ⌘X ⌘A ⌘V ⌘Z ⇧⌘Z
        /// (APP-717), and Return / Escape, which submit and close.
        case ferbOwn(String)
        /// A modifier key alone is no shortcut.
        case modifierOnly
    }

    /// Return, keypad Enter and Escape: the chat's own keys whatever the modifiers.
    static let ownKeys: Set<UInt32> = [36, 76, 53]
    /// Modifier keys' own codes (⌘ ⇧ ⇪ ⌥ ⌃, right-hand ones, fn).
    static let modifierKeys: Set<UInt32> = [54, 55, 56, 57, 58, 59, 60, 61, 62, 63]
    /// ⌘ alone with: , W Q M R C X A V Z, and ⇧⌘Z.
    static let ownCommandKeys: [UInt32: String] = [43: "⌘,", 13: "⌘W", 12: "⌘Q", 46: "⌘M", 15: "⌘R", 8: "⌘C",
                                                    7: "⌘X", 0: "⌘A", 9: "⌘V", 6: "⌘Z"]

    public var problem: Problem? {
        if Self.modifierKeys.contains(keyCode) { return .modifierOnly }
        if modifiers & (Self.command | Self.control | Self.option) == 0 { return .needsModifier }
        if Self.ownKeys.contains(keyCode) { return .ferbOwn(display) }
        if modifiers == Self.command, let own = Self.ownCommandKeys[keyCode] { return .ferbOwn(own) }
        if modifiers == Self.command | Self.shift, keyCode == 6 { return .ferbOwn("⇧⌘Z") }
        return nil
    }

    public static func describe(_ problem: Problem) -> String {
        switch problem {
        case .needsModifier: return "Use ⌘ Command, ⌃ Control or ⌥ Option in the shortcut."
        case .modifierOnly: return "Add a key to the modifiers."
        case .ferbOwn(let keys): return "\(keys) is one of FERB's own keys. Pick another."
        }
    }

    // MARK: Recording

    /// What a key pressed while Settings records the shortcut does: Escape cancels, Delete removes the shortcut, a
    /// combination FERB can't use explains why (recording goes on), anything else is the new shortcut.
    public enum Recorded: Equatable {
        case cancel, clear
        case invalid(Problem)
        case record(KeyCombo)
    }

    public static func recorded(keyCode: UInt16, cocoaModifiers: UInt) -> Recorded {
        let combo = KeyCombo(keyCode: keyCode, cocoaModifiers: cocoaModifiers)
        if combo.modifiers == 0 {
            if keyCode == 53 { return .cancel }
            if keyCode == 51 || keyCode == 117 { return .clear }
        }
        if let problem = combo.problem { return .invalid(problem) }
        return .record(combo)
    }

    /// Settings' warning for a combination macOS uses too (`conflicts`: SystemShortcuts.conflicts), or nil.
    public static func warning(_ combo: KeyCombo, conflicts: [String]) -> String? {
        guard let first = conflicts.first else { return nil }
        return "macOS also uses \(combo.display) for “\(first)”, so it may never reach FERB. "
            + "Pick another, or change it in System Settings → Keyboard → Keyboard Shortcuts."
    }

    // MARK: Display

    /// The character a key code types on the current keyboard layout (FERB.app sets this to UCKeyTranslate at launch;
    /// nil falls back to the US layout below).
    nonisolated(unsafe) public static var layoutCharacter: (UInt32) -> String? = { _ in nil }

    /// As macOS writes shortcuts: ⌃⌥⇧⌘ then the key, e.g. ⌃⌥F.
    public var display: String {
        var text = ""
        if modifiers & Self.control != 0 { text += "⌃" }
        if modifiers & Self.option != 0 { text += "⌥" }
        if modifiers & Self.shift != 0 { text += "⇧" }
        if modifiers & Self.command != 0 { text += "⌘" }
        return text + Self.keyName(keyCode)
    }

    static let namedKeys: [UInt32: String] = [
        49: "Space", 48: "⇥", 51: "⌫", 117: "⌦", 36: "↩", 76: "⌤", 53: "⎋", 123: "←", 124: "→", 125: "↓", 126: "↑",
        115: "↖", 119: "↘", 116: "⇞", 121: "⇟", 114: "Help", 122: "F1", 120: "F2", 99: "F3", 118: "F4", 96: "F5",
        97: "F6", 98: "F7", 100: "F8", 101: "F9", 109: "F10", 103: "F11", 111: "F12", 105: "F13", 107: "F14",
        113: "F15", 106: "F16", 64: "F17", 79: "F18", 80: "F19", 90: "F20",
    ]
    /// The US layout's characters, for when the current layout can't be read.
    static let usKeys: [UInt32: String] = [
        0: "A", 11: "B", 8: "C", 2: "D", 14: "E", 3: "F", 5: "G", 4: "H", 34: "I", 38: "J", 40: "K", 37: "L", 46: "M",
        45: "N", 31: "O", 35: "P", 12: "Q", 15: "R", 1: "S", 17: "T", 32: "U", 9: "V", 13: "W", 7: "X", 16: "Y", 6: "Z",
        29: "0", 18: "1", 19: "2", 20: "3", 21: "4", 23: "5", 22: "6", 26: "7", 28: "8", 25: "9", 27: "-", 24: "=",
        33: "[", 30: "]", 42: "\\", 41: ";", 39: "'", 50: "`", 43: ",", 47: ".", 44: "/",
    ]

    public static func keyName(_ code: UInt32) -> String {
        if let named = namedKeys[code] { return named }
        if let typed = layoutCharacter(code)?.trimmingCharacters(in: .whitespacesAndNewlines), !typed.isEmpty,
           typed.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) }) {
            return typed.uppercased()
        }
        return usKeys[code] ?? "Key \(code)"
    }

    // MARK: Storage

    /// The defaults key the shortcut is saved under (AppSettings.openShortcut).
    public static let defaultsKey = "shortcut.openFERB"

    public static func saved(in defaults: UserDefaults) -> KeyCombo? {
        guard let stored = defaults.dictionary(forKey: defaultsKey),
              let code = (stored["keyCode"] as? NSNumber)?.uint32Value,
              let mods = (stored["modifiers"] as? NSNumber)?.uint32Value else { return nil }
        let combo = KeyCombo(keyCode: code, modifiers: mods)
        return combo.problem == nil ? combo : nil
    }

    /// Saves the shortcut, or removes it (nil: no shortcut, the default).
    public static func save(_ combo: KeyCombo?, in defaults: UserDefaults) {
        if let combo { defaults.set(["keyCode": combo.keyCode, "modifiers": combo.modifiers], forKey: defaultsKey) }
        else { defaults.removeObject(forKey: defaultsKey) }
    }
}

/// macOS's own keyboard shortcuts (System Settings → Keyboard → Keyboard Shortcuts), read from
/// `com.apple.symbolichotkeys`'s AppleSymbolicHotKeys: a combination macOS takes first never reaches FERB, so Settings
/// warns (never blocks). Entries the file doesn't list keep macOS's defaults (`defaults` below); an entry it lists
/// wins, enabled or not.
public enum SystemShortcuts {
    public struct Entry: Equatable {
        public let name: String
        public let keyCode: UInt32
        /// AppKit modifier flags as the file stores them.
        public let cocoaModifiers: UInt
    }

    /// macOS's default shortcuts, by symbolic hot key id.
    static let defaults: [Int: Entry] = [
        27: Entry(name: "Move focus to next window", keyCode: 50, cocoaModifiers: 1 << 20),
        28: Entry(name: "Save picture of screen as a file", keyCode: 20, cocoaModifiers: 1 << 20 | 1 << 17),
        29: Entry(name: "Copy picture of screen to the clipboard", keyCode: 20, cocoaModifiers: 1 << 20 | 1 << 17 | 1 << 18),
        30: Entry(name: "Save picture of selected area as a file", keyCode: 21, cocoaModifiers: 1 << 20 | 1 << 17),
        31: Entry(name: "Copy picture of selected area to the clipboard", keyCode: 21,
                  cocoaModifiers: 1 << 20 | 1 << 17 | 1 << 18),
        184: Entry(name: "Screenshot and recording options", keyCode: 23, cocoaModifiers: 1 << 20 | 1 << 17),
        32: Entry(name: "Mission Control", keyCode: 126, cocoaModifiers: 1 << 18),
        33: Entry(name: "Application windows", keyCode: 125, cocoaModifiers: 1 << 18),
        52: Entry(name: "Turn Dock hiding on or off", keyCode: 2, cocoaModifiers: 1 << 20 | 1 << 19),
        60: Entry(name: "Select the previous input source", keyCode: 49, cocoaModifiers: 1 << 18),
        61: Entry(name: "Select next source in Input menu", keyCode: 49, cocoaModifiers: 1 << 18 | 1 << 19),
        64: Entry(name: "Show Spotlight search", keyCode: 49, cocoaModifiers: 1 << 20),
        65: Entry(name: "Show Finder search window", keyCode: 49, cocoaModifiers: 1 << 20 | 1 << 19),
        79: Entry(name: "Move left a space", keyCode: 123, cocoaModifiers: 1 << 18),
        81: Entry(name: "Move right a space", keyCode: 124, cocoaModifiers: 1 << 18),
        98: Entry(name: "Show Help menu", keyCode: 44, cocoaModifiers: 1 << 20 | 1 << 17),
    ]

    /// The names of the enabled macOS shortcuts this combination matches (`hotKeys`: AppleSymbolicHotKeys as read).
    public static func conflicts(_ combo: KeyCombo, in hotKeys: [String: Any]) -> [String] {
        var entries: [Int: (Entry?, Bool)] = [:]
        for (id, entry) in defaults { entries[id] = (entry, true) }
        for (key, value) in hotKeys {
            guard let id = Int(key), let item = value as? [String: Any] else { continue }
            let enabled = (item["enabled"] as? NSNumber)?.boolValue ?? false
            let parameters = ((item["value"] as? [String: Any])?["parameters"] as? [Any])?
                .compactMap { ($0 as? NSNumber)?.intValue }
            let name = defaults[id]?.name ?? "a macOS keyboard shortcut"
            if let parameters, parameters.count >= 3, parameters[1] != 65535 {
                entries[id] = (Entry(name: name, keyCode: UInt32(parameters[1]), cocoaModifiers: UInt(parameters[2])), enabled)
            } else {
                entries[id] = (defaults[id], enabled)          // listed without keys: macOS's default keys
            }
        }
        return entries.keys.sorted().compactMap { id in
            guard let (entry, enabled) = entries[id], enabled, let entry,
                  KeyCombo(keyCode: UInt16(clamping: entry.keyCode), cocoaModifiers: entry.cocoaModifiers) == combo
            else { return nil }
            return entry.name
        }
    }
}
