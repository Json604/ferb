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
        // PHN-800: the session's menu, and a message the Mac refused shown as not sent
        shot("screens/session-menu-noon.png", sheet: { SessionSample.session(scrolls: false, menuOpen: true) }, page: { SessionSample.page() })
        shot("screens/session-not-sent-noon.png", sheet: { SessionSample.session(scrolls: false, unsent: SessionSample.unsent) },
             page: { SessionSample.page() })
        // HAND-683: FERB can't tell which terminal is the session's; he chooses one or doesn't send
        shot("screens/session-pick-terminal-noon.png", sheet: {
            ZStack {
                SessionSample.session(scrolls: false)
                PickTerminalSheet(title: "Which terminal is this session in?",
                                  why: "FERB couldn't tell which terminal in Visual Studio Code is this session's, so it typed nothing. Choose it, and FERB types only there.",
                                  terminals: SessionSample.terminals,
                                  note: "FERB types only into the one you choose, and asks again if it changes.", no: "Don't send",
                                  yesSymbol: "faceid", onPick: { _ in }, onNo: {})
            }
        }, page: { SessionSample.page() })
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

    /// One session's conversation, owner 2026-11-23 (kb PHN-812, PHN-813, PHN-815), at the smallest current iPhone width
    /// (375 points) and the lab's (402): a conversation longer than the screen with room under the header (rows dissolve
    /// in under it) and above the message box; his words the moment he tapped Send (a dimmed bubble, a small turning arc
    /// and "Sending…", Send waiting); him scrolled up, with the main chat's jump control; and opening, its first page on
    /// its way (the main chat's waiting look).
    // decision: PHN-812, PHN-813, PHN-815, PHN-816
    func testASessionsConversation() {
        for width: CGFloat in [375, 402] {
            let size = CGSize(width: width, height: PhoneInsets.labSize.height)
            func shot<S: View>(_ name: String, @ViewBuilder sheet: () -> S) {
                let view = PhoneScreen(moment: .lab(12.5), glide: false, sheet: sheet, page: { SessionSample.page() })
                    .frame(width: size.width, height: size.height)
                    .stillMotion()
                Snapshot.assert(view, size: size, path: "screens/session-\(name)-\(Int(width)).png")
            }
            shot("long") { SessionSample.session(scrolls: false, lines: SessionSample.long, asks: false) }
            shot("sending") { SessionSample.session(scrolls: false, sending: SessionSample.sending) }
            // PHN-816: opening, its first page on its way: the main chat's waiting look, never an error before the Mac answers
            shot("waiting") { SessionSample.session(scrolls: false, lines: [], waiting: "Getting this session from your Mac…") }
            shot("scrolled-up") {
                SessionSample.session(scrolls: false, lines: SessionSample.long, asks: false, stillJump: ListScroll(atEnd: false, unseen: false))
            }
            shot("new-message") {
                SessionSample.session(scrolls: false, lines: SessionSample.long, asks: false, stillJump: ListScroll(atEnd: false, unseen: true))
            }
        }
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

#if canImport(UIKit)
/// The top bar (PHN-801) with every tab chosen in turn, at the smallest current iPhone width (375 points) and the lab's
/// (402): the chosen tab's name fits, the other icons keep their room, the link's state is the profile button's badge.
/// Android's TopBarWidthsShotsTest draws the same.
@MainActor
final class TopBarWidthsTests: XCTestCase {
    private func shoot(width: CGFloat, _ name: String) {
        let size = CGSize(width: width, height: 560)
        let view = PhoneScreen(moment: .lab(12.5), glide: false, sheet: { EmptyView() }, page: {
            VStack(spacing: 18) {
                ForEach(PhoneTab.allCases, id: \.self) { t in
                    TopBar(chosen: t, waiting: t == .tasks ? nil : .tasks, onTab: { _ in }, onProfile: {})
                }
                Spacer(minLength: 0)
            }
            .padding(.top, 20)
            .environment(\.connection, Connection(mark: .direct, label: "Your Mac: Connected", onClick: {}))
        })
        .frame(width: size.width, height: size.height)
        .stillMotion()
        Snapshot.assert(view, size: size, path: "components/\(name).png")
    }

    // decision: PHN-801
    func testEveryTabAt375() { shoot(width: 375, "topbar-375") }
    // decision: PHN-801
    func testEveryTabAt402() { shoot(width: 402, "topbar-402") }
}
#endif
