#if canImport(UIKit)
import FerbGallery
import SwiftUI
import XCTest
@testable import FerbDesign

/// The lab's twelve phone screens at dawn, noon and night in Delhi on 2026-11-14 (the moments Android's goldens use:
/// 6.1, 12.5 and 22.5 local), at the lab's iPhone size, 402 by 874 points at one pixel a point, against the committed
/// goldens in Screenshots/screens (Android's are apps/android/design/src/test/screenshots/screens, same names). Motion
/// is still at the lab's t = 1000 and the sky does not glide.
@MainActor
final class ScreenshotTests: XCTestCase {
    static let times: [(String, Double)] = [("dawn", 6.1), ("noon", 12.5), ("night", 22.5)]

    private func shoot(_ screen: LabScreen) {
        for (name, hour) in Self.times {
            let view = LabScreenView(screen: screen, moment: .lab(hour), glide: false)
                .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
                .stillMotion()
            Snapshot.assert(view, size: PhoneInsets.labSize, path: "screens/\(screen.rawValue)-\(name).png")
        }
    }

    // decision: PHN-416, PHN-449, PHN-410
    func testHome() { shoot(.home) }
    // decision: PHN-416
    func testChat() { shoot(.chat) }
    // decision: PHN-416
    func testQuestion() { shoot(.question) }
    // decision: PHN-416
    func testSecret() { shoot(.secret) }
    // decision: PHN-416
    func testTasks() { shoot(.tasks) }
    // decision: PHN-416
    func testFiles() { shoot(.files) }
    // decision: PHN-416
    func testMac() { shoot(.mac) }
    // decision: PHN-416, PHN-410
    func testRelay() { shoot(.relay) }
    // decision: PHN-416
    func testProfile() { shoot(.profile) }
    // decision: PHN-416
    func testUnlock() { shoot(.unlock) }
    // decision: PHN-416
    func testPair() { shoot(.pair) }
    // decision: PHN-416
    func testCodes() { shoot(.codes) }

    /// The Sessions tab at dawn, noon and night, off at noon, and one session with its question over the tab, and the
    /// Yes before FERB types for the owner (not from the lab, kb HAND-651; Android's LabScreen SESSIONS and SESSION).
    // decision: PHN-416, GATE-622, PHN-797
    func testSessions() {
        func shot<P: View, S: View>(_ path: String, hour: Double = 12.5, @ViewBuilder sheet: () -> S, @ViewBuilder page: () -> P) {
            let view = PhoneScreen(moment: .lab(hour), glide: false, sheet: sheet, page: page)
                .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
                .stillMotion()
            Snapshot.assert(view, size: PhoneInsets.labSize, path: path)
        }
        for (name, hour) in Self.times {
            shot("screens/sessions-\(name).png", hour: hour, sheet: { EmptyView() }, page: { SessionSample.page() })
        }
        shot("screens/sessions-off-noon.png", sheet: { EmptyView() }, page: { SessionSample.page(off: true) })
        shot("screens/session-noon.png", sheet: { SessionSample.session(scrolls: false) }, page: { SessionSample.page() })
        shot("screens/session-yes-noon.png", sheet: {
            ZStack {
                SessionSample.session(typed: "Run it once more and tell me", scrolls: false)
                SessionYesSheet(question: "Type this into Claude Code in Terminal? FERB will bring Terminal to the front to do it.",
                                detail: "Run it once more and tell me",
                                covers: "Yes lets you send to this session from here until you stop controlling it.",
                                yes: "Yes, send it", cancel: "Cancel", yesSymbol: "faceid")
            }
        }, page: { SessionSample.page() })
    }

    /// A few of the gallery's showcases, at their own size.
    // decision: PHN-416, PHN-450
    func testComponents() {
        for name in ["tokens", "marks-pills", "rows", "conversation", "sheet-pieces", "keypad", "link-pictures", "seasons", "moons"] {
            guard let show = Showcases.all.first(where: { $0.name == name }) else { XCTFail("no showcase \(name)"); continue }
            Snapshot.assert(show.view().stillMotion(), size: nil, path: "components/\(name).png")
        }
    }

    /// The pages drawn with the app's callbacks attached draw exactly the lab's sample (kb PHN-509): a callback only
    /// adds touch.
    // decision: PHN-509
    func testCallbacksDoNotChangeTheDrawing() throws {
        try XCTSkipIf(Snapshot.recording, "checked against the recorded chat golden, never recorded itself")
        let moment = SkyMoment.lab(12.5)
        let view = PhoneScreen(moment: moment, glide: false) {
            ChatPage(onTab: { _ in }, onProfile: {}, onWatch: { _ in }, onStop: { _ in })
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "screens/chat-noon.png")
    }
}
#endif
