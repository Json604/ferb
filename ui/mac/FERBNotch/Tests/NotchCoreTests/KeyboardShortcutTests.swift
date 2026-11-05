import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Owner, 2026-11-05: "Custom keyboard shortcuts to open ferb", approved with D24: any key combination with ⌘ or ⌃,
/// no default; double-tap right Option stays and can be switched off; no other double-tap keys for now.
@MainActor
final class KeyboardShortcutTests: XCTestCase {
    private let F: UInt32 = 3
    private var controlOption: UInt32 { KeyCombo.control | KeyCombo.option }

    // decision: APP-740, APP-741
    func testACombinationNeedsCommandOrControlAndIsNeverOneOfFERBsOwnKeys() {
        XCTAssertNil(KeyCombo(keyCode: F, modifiers: controlOption).problem)
        XCTAssertNil(KeyCombo(keyCode: 49, modifiers: KeyCombo.command).problem)          // ⌘Space: allowed, warned
        XCTAssertEqual(KeyCombo(keyCode: F, modifiers: KeyCombo.option).problem, .needsCommandOrControl)
        XCTAssertEqual(KeyCombo(keyCode: F, modifiers: KeyCombo.option | KeyCombo.shift).problem, .needsCommandOrControl)
        XCTAssertEqual(KeyCombo(keyCode: F, modifiers: 0).problem, .needsCommandOrControl)
        XCTAssertEqual(KeyCombo(keyCode: 58, modifiers: KeyCombo.command).problem, .modifierOnly)
        for code: UInt32 in [43, 13, 12, 46, 15, 8, 7, 0, 9, 6] {                    // ⌘, W Q M R C X A V Z
            if case .ferbOwn = KeyCombo(keyCode: code, modifiers: KeyCombo.command).problem {} else {
                XCTFail("⌘ + key \(code) should be FERB's own")
            }
        }
        XCTAssertEqual(KeyCombo(keyCode: 6, modifiers: KeyCombo.command | KeyCombo.shift).problem, .ferbOwn("⇧⌘Z"))
        if case .ferbOwn = KeyCombo(keyCode: 36, modifiers: KeyCombo.command).problem {} else { XCTFail("⌘Return") }
        XCTAssertNil(KeyCombo(keyCode: 13, modifiers: KeyCombo.command | KeyCombo.option).problem)   // ⌥⌘W is not ⌘W
    }

    // decision: APP-740, APP-741
    func testRecordingKeys() {
        XCTAssertEqual(KeyCombo.recorded(keyCode: 53, cocoaModifiers: 0), .cancel)
        XCTAssertEqual(KeyCombo.recorded(keyCode: 51, cocoaModifiers: 0), .clear)
        XCTAssertEqual(KeyCombo.recorded(keyCode: 3, cocoaModifiers: NSEvent.ModifierFlags.option.rawValue),
                       .invalid(.needsCommandOrControl))
        let flags = NSEvent.ModifierFlags([.control, .option, .function]).rawValue      // fn never counts
        XCTAssertEqual(KeyCombo.recorded(keyCode: 3, cocoaModifiers: flags),
                       .record(KeyCombo(keyCode: F, modifiers: controlOption)))
    }

    // decision: APP-740, APP-741
    func testTheShortcutIsSavedUnderOneKeyAndShownInTheCurrentLayout() {
        let defaults = scratchDefaults()
        XCTAssertNil(KeyCombo.saved(in: defaults), "no shortcut by default")
        let combo = KeyCombo(keyCode: F, modifiers: controlOption)
        KeyCombo.save(combo, in: defaults)
        XCTAssertEqual(KeyCombo.saved(in: defaults), combo)
        XCTAssertNotNil(defaults.dictionary(forKey: AppSettings.openShortcut.key))
        XCTAssertEqual(combo.display, "⌃⌥F")
        let saved = KeyCombo.layoutCharacter
        defer { KeyCombo.layoutCharacter = saved }
        KeyCombo.layoutCharacter = { $0 == 3 ? "ä" : nil }                             // another layout's key
        XCTAssertEqual(combo.display, "⌃⌥Ä")
        XCTAssertEqual(KeyCombo(keyCode: 49, modifiers: KeyCombo.command | KeyCombo.shift).display, "⇧⌘Space")
        KeyCombo.save(nil, in: defaults)
        XCTAssertNil(defaults.object(forKey: AppSettings.openShortcut.key))
    }

    // decision: APP-740, APP-741
    func testMacOSShortcutsWarnFromTheSymbolicHotKeysFile() {
        let spotlight = KeyCombo(keyCode: 49, modifiers: KeyCombo.command)
        let inputSource = KeyCombo(keyCode: 49, modifiers: KeyCombo.control)
        let mine = KeyCombo(keyCode: F, modifiers: controlOption)
        // As com.apple.symbolichotkeys keeps it: only what was ever changed is listed.
        let file: [String: Any] = [
            "60": ["enabled": true, "value": ["parameters": [32, 49, 262_144], "type": "standard"]],
            "79": ["enabled": true],
            "15": ["enabled": false],
        ]
        XCTAssertEqual(SystemShortcuts.conflicts(spotlight, in: file), ["Show Spotlight search"])   // macOS's default
        XCTAssertEqual(SystemShortcuts.conflicts(inputSource, in: file), ["Select the previous input source"])
        XCTAssertEqual(SystemShortcuts.conflicts(KeyCombo(keyCode: 123, modifiers: KeyCombo.control), in: file),
                       ["Move left a space"])
        XCTAssertEqual(SystemShortcuts.conflicts(mine, in: file), [])
        let off: [String: Any] = ["64": ["enabled": false, "value": ["parameters": [32, 49, 1_048_576]]]]
        XCTAssertEqual(SystemShortcuts.conflicts(spotlight, in: off), [], "a switched-off shortcut doesn't warn")
        let moved: [String: Any] = ["64": ["enabled": true, "value": ["parameters": [32, 3, 786_432]]]]
        XCTAssertEqual(SystemShortcuts.conflicts(spotlight, in: moved), [])
        XCTAssertEqual(SystemShortcuts.conflicts(mine, in: moved), ["Show Spotlight search"])
        XCTAssertNotNil(KeyCombo.warning(spotlight, conflicts: ["Show Spotlight search"]))
        XCTAssertNil(KeyCombo.warning(mine, conflicts: []))
    }

    // decision: APP-741
    func testTheCenterRegistersChangesUnregistersWhileRecordingAndShowsAFailure() {
        let defaults = scratchDefaults()
        let registrar = FakeHotKeys()
        let center = HotKeyCenter(defaults: defaults, registrar: registrar)
        center.start()
        XCTAssertNil(registrar.registered, "no shortcut, nothing registered")
        let combo = KeyCombo(keyCode: F, modifiers: controlOption)
        center.set(combo)
        XCTAssertEqual(registrar.registered, combo)
        XCTAssertEqual(KeyCombo.saved(in: defaults), combo)
        var pressed = 0
        center.onPressed = { pressed += 1 }
        registrar.press()
        XCTAssertEqual(pressed, 1)

        center.beginRecording()
        XCTAssertNil(registrar.registered, "recording leaves the current shortcut free to be pressed")
        center.endRecording()
        XCTAssertEqual(registrar.registered, combo)

        let other = KeyCombo(keyCode: 2, modifiers: KeyCombo.command | KeyCombo.control)
        registrar.refuse = "Another app already uses ⌃⌘D."
        center.set(other)
        XCTAssertEqual(center.failure, "Another app already uses ⌃⌘D.")
        XCTAssertNil(registrar.registered)

        registrar.refuse = nil
        center.set(nil)                                                                 // Reset
        XCTAssertNil(center.failure)
        XCTAssertNil(registrar.registered)
        XCTAssertNil(KeyCombo.saved(in: defaults))

        // A saved shortcut is registered at the next launch.
        KeyCombo.save(combo, in: defaults)
        let relaunched = HotKeyCenter(defaults: defaults, registrar: registrar)
        relaunched.start()
        XCTAssertEqual(registrar.registered, combo)
        relaunched.stop()
        XCTAssertNil(registrar.registered)
    }

    // decision: APP-741, APP-709
    func testTheHotKeyTakesTheDoubleTapPathIncludingFromAYield() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        KeyCombo.save(KeyCombo(keyCode: F, modifiers: controlOption), in: defaults)
        let registrar = FakeHotKeys()
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults, hotKeyRegistrar: registrar)
        defer { coordinator.tearDown() }
        coordinator.updateScreen()
        coordinator.hotKeys.start()
        var seen: [IntroControl] = []
        coordinator.introControls.onControl = { seen.append($0) }

        registrar.press()
        XCTAssertTrue(model.bodyVisible, "the shortcut didn't open FERB")
        XCTAssertTrue(model.staysOpenOnHoverOut, "a shortcut-opened chat stays open like a double-tap-opened one")
        XCTAssertEqual(seen, [.toggle])
        model.beginYield(.foreignOverlay)
        registrar.press()
        XCTAssertFalse(model.isYielded, "the shortcut didn't bring FERB back from a yield")
        XCTAssertTrue(model.expanded)
        registrar.press()
        XCTAssertFalse(model.expanded, "the shortcut didn't close FERB")
    }

    // decision: APP-740, APP-552
    func testWithDoubleTapOffTheMonitorIgnoresItAndHoldToTalkStillWorks() {
        var state = NotchShortcutState()
        state.doubleTapEnabled = false
        XCTAssertEqual(state.update([.rightOption], at: 0, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 0.1, keyCount: 0), [])
        XCTAssertEqual(state.update([.rightOption], at: 0.2, keyCount: 0), [])
        XCTAssertEqual(state.update([], at: 0.3, keyCount: 0), [], "double-tap is off")
        XCTAssertEqual(state.update([.rightOption, .rightCommand], at: 1, keyCount: 0), [.startVoice])
        XCTAssertEqual(state.update([], at: 2, keyCount: 0), [.finishVoice])
        _ = state.reset()
        XCTAssertFalse(state.doubleTapEnabled, "a reset (sleep, session switch) keeps the owner's choice")
    }

    // decision: APP-740, APP-728
    func testTheGuideCarriesTheShortcutAndWhetherDoubleTapIsOn() {
        let defaults = scratchDefaults()
        XCTAssertEqual(AppGuide.keys(AppGuide.openAction, defaults), "Double-tap right Option")
        KeyCombo.save(KeyCombo(keyCode: F, modifiers: controlOption), in: defaults)
        XCTAssertEqual(AppGuide.keys(AppGuide.openAction, defaults), "Double-tap right Option, or ⌃⌥F")
        defaults.set(false, forKey: AppSettings.doubleTap.key)
        let payload = AppGuide.payload(defaults)
        let shortcuts = payload["shortcuts"] as? [[String: String]] ?? []
        XCTAssertEqual(shortcuts.first { $0["action"] == AppGuide.openAction }?["keys"], "⌃⌥F")
        let rows = payload["app_settings"] as? [[String: String]] ?? []
        XCTAssertEqual(rows.first { $0["key"] == AppSettings.openShortcut.key }?["value"], "⌃⌥F")
        XCTAssertEqual(rows.first { $0["key"] == AppSettings.doubleTap.key }?["value"], "off")
        KeyCombo.save(nil, in: defaults)
        XCTAssertEqual(rows.count, AppSettings.all.count)
        XCTAssertTrue(AppGuide.keys(AppGuide.openAction, defaults).contains("click"))
    }
}

/// A registrar that records what FERB registered and can press it, or refuse it as another app would.
@MainActor
final class FakeHotKeys: HotKeyRegistering {
    private(set) var registered: KeyCombo?
    private var pressed: (() -> Void)?
    var refuse: String?

    func register(_ combo: KeyCombo, pressed: @escaping () -> Void) -> String? {
        if let refuse { registered = nil; return refuse }
        registered = combo
        self.pressed = pressed
        return nil
    }

    func unregister() {
        registered = nil
        pressed = nil
    }

    func press() { pressed?() }
}
