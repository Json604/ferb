import Foundation

/// FERB.app's own preferences, each declared once (owner, 2026-11-05: "what gurantees ferb knowledge about itself is
/// updated too??" — approved: one declaration per setting with its guide line; a test that fails if a saved app
/// preference has no guide entry; the guide carries current non-secret values and is re-sent when one changes).
///
/// A declaration names the UserDefaults keys it is stored under, its name and place in Settings, what it does and how
/// to read its current value as owner-facing text. Settings reads names from here; the guide FERB.app sends the daemon
/// (`about.guide`, APP-4) carries each with its current value, so FERB's self-description follows the app. A value is
/// sent only for a setting marked `shared`; nothing here may ever hold a secret.
public struct AppSetting {
    /// The UserDefaults keys it is stored under: the first is its own; the rest are parts of the same choice.
    public let keys: [String]
    /// Its label in Settings.
    public let name: String
    /// Where the owner changes it.
    public let place: String
    /// What it does, in a few words.
    public let effect: String
    /// Its current value may go to FERB's guide (never a secret; every preference today may).
    public let shared: Bool
    /// The current value as the owner reads it.
    public let read: (UserDefaults) -> String

    public var key: String { keys[0] }

    public init(keys: [String], name: String, place: String, effect: String, shared: Bool = true,
                read: @escaping (UserDefaults) -> String) {
        self.keys = keys
        self.name = name
        self.place = place
        self.effect = effect
        self.shared = shared
        self.read = read
    }

    /// A switch stored as a boolean, `on` when unset.
    public static func toggle(_ key: String, _ name: String, place: String, effect: String,
                              on: Bool = true) -> AppSetting {
        AppSetting(keys: [key], name: name, place: place, effect: effect) { defaults in
            (defaults.object(forKey: key) as? Bool ?? on) ? "on" : "off"
        }
    }
}

public enum AppSettings {
    static let notch = "Settings → Notch & Shortcuts"
    static let sounds = "Settings → Sounds & Notifications → Sounds"

    /// D19 (APP-745): the notch by default, or the sticky window.
    public static let whereFERBOpens = AppSetting(
        keys: [ChatHost.defaultsKey], name: "Where FERB opens", place: notch + " → Where FERB opens",
        effect: "Notch: FERB opens in the notch. Window: FERB opens in a window that stays open until closed (close "
            + "button, ⌘W or Esc), floats above other apps' windows and shows on every Space; the shortcut, the menu bar's Open FERB, a file drop or "
            + "opening FERB.app again opens it") { defaults in
        ChatHost.setting(defaults) == .window ? "Window" : "Notch"
    }
    public static let hidePill = AppSetting.toggle(
        ChatHost.hidePillKey, "Hide the pill", place: notch + " → Where FERB opens (window only)",
        effect: "In window mode the notch handle and pill disappear and the menu bar icon shows FERB's status; the "
            + "menu bar icon, the shortcut and opening FERB.app again still bring FERB back", on: false)
    public static let windowPlace = AppSetting(
        keys: [ChatWindowFrames.defaultsKey], name: "Where the window sits", place: "Drag or resize FERB's window",
        effect: "FERB's window remembers its place and size on each display") { defaults in
        let count = ChatWindowFrames.count(in: defaults)
        return count == 0 ? "the default (top right)" : "saved on \(count) display\(count == 1 ? "" : "s")"
    }
    public static let hover = AppSetting.toggle(
        "notch.hoverEnabled", "Open when I hover over FERB", place: notch + " → Notch & pill",
        effect: "Hovering over the notch or pill opens FERB; off, only the shortcut or a click opens it")
    public static let sendOnRelease = AppSetting.toggle(
        "voice.sendOnRelease", "Send dictation when I let go of the keys", place: notch + " → Keyboard & voice",
        effect: "Dictation is sent as soon as the dictation keys are released, instead of waiting in the composer",
        on: false)
    /// The owner's own keyboard shortcut to open FERB (APP-818: any combination with ⌘, ⌃ or ⌥; none by default).
    public static let openShortcut = AppSetting(
        keys: [KeyCombo.defaultsKey], name: "Keyboard shortcut to open FERB", place: notch + " → Keyboard & voice",
        effect: "Pressing this combination anywhere opens FERB, or closes it when open; none is set at first") { defaults in
        KeyCombo.saved(in: defaults)?.display ?? "none"
    }
    public static let doubleTap = AppSetting.toggle(
        "shortcut.doubleTapRightOption", "Double-tap right Option to open FERB", place: notch + " → Keyboard & voice",
        effect: "Tapping the right Option key twice opens FERB, or closes it when open; holding right ⌘ + ⌥ to dictate "
            + "works either way")
    public static let pointer = AppSetting.toggle(
        "cursor.enabled", "Show FERB's pointer when it acts on screen", place: notch + " → FERB’s pointer",
        effect: "A small pointer shows where FERB clicks or types on screen")
    public static let placement = AppSetting(
        keys: ["notch.docked", "notch.indicatorEdge", "notch.indicatorFraction", "notch.indicatorDisplayID"],
        name: "Where FERB sits", place: "Drag FERB away from the notch, or back into it",
        effect: "FERB sits in the hardware notch, or as a pill on a screen edge where the owner dropped it") { defaults in
        if defaults.object(forKey: "notch.docked") as? Bool ?? true { return "in the notch" }
        let edge = defaults.string(forKey: "notch.indicatorEdge") ?? "top"
        return "a pill on the \(edge) edge of the screen"
    }
    public static let chatHeight = AppSetting(
        keys: ["notch.transcriptHeight"], name: "Chat height", place: "Drag the bottom edge of the open chat",
        effect: "How tall the open chat is (APP-587)") { defaults in
        let height = defaults.double(forKey: "notch.transcriptHeight")
        return height > 0 ? "\(Int(height)) points" : "the default (300 points)"
    }
    public static let chatWidth = AppSetting(
        keys: ["notch.panelWidth"], name: "Chat width", place: "Drag the side edge of the open chat",
        effect: "How wide the open chat is") { defaults in
        let width = defaults.double(forKey: "notch.panelWidth")
        return width > 0 ? "\(Int(width)) points" : "the default"
    }
    public static let soundsOn = AppSetting.toggle(
        "feedback.enabled", "Play feedback sounds", place: sounds, effect: "FERB's sounds play at all")
    public static let volume = AppSetting(
        keys: ["feedback.volume"], name: "Volume", place: sounds,
        effect: "How loud FERB's own sounds are (the Mac's volume still applies)") { defaults in
        guard let volume = defaults.object(forKey: "feedback.volume") as? Double else { return "the default" }
        return "\(Int((volume / 0.5 * 100).rounded()))% of the slider"
    }
    public static let intensity = AppSetting(
        keys: [SoundPolicy.intensityKey], name: "Intensity", place: sounds,
        effect: "Standard or Subtle sounds") { defaults in
        defaults.string(forKey: SoundPolicy.intensityKey) == "subtle" ? "Subtle" : "Standard"
    }

    /// One switch per sound the owner can turn on or off (Settings → Sounds → Individual sounds).
    public static var soundSwitches: [AppSetting] {
        SoundSwitch.allCases.map { sound in
            AppSetting.toggle(sound.key, "Sound: " + sound.title, place: sounds + " → Individual sounds",
                              effect: sound.meaning, on: sound.onByDefault)
        }
    }

    /// Every owner-facing app preference, in Settings' order. A new preference is added here (the test enumerates the
    /// keys FERB.app writes and fails on one that is neither declared nor in `internalKeys`).
    public static var all: [AppSetting] {
        [whereFERBOpens, hidePill, windowPlace, hover, openShortcut, doubleTap, sendOnRelease, pointer, placement,
         chatHeight, chatWidth, soundsOn, volume, intensity] + soundSwitches
    }

    /// UserDefaults keys FERB.app writes that are no owner preference, each with why.
    public static let internalKeys: [String: String] = [
        "FERBActiveTaskID": "the task the notch shows now (state, not a choice)",
        "FERBAnnouncedAttentionIDs": "notices already announced (bookkeeping)",
        "FERBAskedNotifications": "whether macOS was asked for notifications (bookkeeping)",
        "FERBConversationArchive": "the saved chat and its copies kept aside (APP-801; data, not a choice; left as it was once FERB's database holds it, APP-808)",
        "FERBConversationID": "the daemon conversation this chat continues (state)",
        "FERBChatHistoryImport": "when FERB's database took over the saved chat, and the archive's bytes then (APP-808, bookkeeping)",
        "FERBChatHistoryOutbox": "chat entries waiting to be written to FERB's database (APP-805; data, not a choice)",
        "FERBIntroBeat": "where the intro tour was left (state)",
        "FERBIntroDone": "whether the intro tour finished (state)",
        "FERBIntroPendingGrant": "a permission the tour waits on across a restart (state)",
        "FERBIntroPlan": "the intro plan in progress (state)",
        "FERBIntroTour": "the tour version last played (APP-635, bookkeeping)",
        "FERBLastLaunchedBuild": "the build that last launched, for update notes (bookkeeping)",
        "FERBLastLaunchedVersion": "the version that last launched, for update notes (APP-750, bookkeeping)",
        "FERBUpdateLastResult": "the last update's result (bookkeeping)",
        "loginItem.defaultApplied": "whether open-at-login's default was applied once (bookkeeping)",
        "loginItem.refusal": "macOS refused open at login (state; the setting itself lives in macOS's Login Items)",
        "feedback.magneticEnabled": "a removed setting (APP-527), only cleared on upgrade",
        "chatWindow.keepOnTop": "a removed setting (Keep on top, APP-772), only cleared on upgrade",
        "concept.arrivalStyle": "the isolated playground's review switch, never in FERB.app's Settings",
    ]

    /// The declared settings with their current values, as `about.guide` carries them (`value` only when shared).
    public static func current(_ defaults: UserDefaults) -> [[String: String]] {
        all.map { setting in
            var row = ["key": setting.key, "name": setting.name, "where": setting.place, "what": setting.effect]
            if setting.shared { row["value"] = setting.read(defaults) }
            return row
        }
    }

    /// The declared keys (each setting's own and its parts).
    public static var declaredKeys: Set<String> { Set(all.flatMap(\.keys)) }
}

extension SoundSwitch {
    /// Settings' name for the sound, and what it means in a few words.
    public var title: String {
        switch self {
        case .needsYou: return "Needs you"
        case .error: return "Something went wrong"
        case .notification: return "Notification"
        case .listeningStart: return "Listening"
        case .listeningEnd: return "Done listening"
        case .tour: return "Intro tour"
        case .summon: return "Open FERB"
        case .paused: return "Paused"
        }
    }

    public var meaning: String {
        switch self {
        case .needsYou: return "A question or approval is waiting for you"
        case .error: return "A task couldn't finish"
        case .notification: return "Reminders, scheduled tasks and notices"
        case .listeningStart: return "Dictation starts"
        case .listeningEnd: return "Dictation stops"
        case .tour: return "The sounds of the first-run tour"
        case .summon: return "You open FERB"
        case .paused: return "You pause FERB"
        }
    }
}
