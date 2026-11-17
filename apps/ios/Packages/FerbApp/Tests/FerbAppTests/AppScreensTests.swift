#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import UserNotifications
import XCTest
@testable import FerbAppUI

/// The app's own screens (FerbAppUI) drawn from the app model over a fake phone, at noon in Delhi on 2026-11-14 at the
/// lab's iPhone size: where the app shows a lab screen it must draw the design's golden (Android's ScreenShotsTest
/// does the same against :design's); app-only states have goldens of their own (Screenshots/app). Also the privacy
/// cover, the notification requests and the string catalog.
@MainActor
final class AppScreensTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func screen<V: View>(_ page: V) -> some View {
        PhoneScreen(moment: .lab(12.5), glide: false) { page }
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
            .stillMotion()
    }

    private func shot<V: View>(_ page: V, app name: String) {
        AppSnapshot.assert(screen(page), size: PhoneInsets.labSize, path: "app/\(name).png")
    }

    /// Against the design's golden. 0.2 %: a page whose actions have callbacks wraps them in buttons, and a button
    /// places its label on whole points, so a label in a column of fractional width moves by under a point (the
    /// pairing page's "Type code": 450 pixels, 0.13 %); anything more is a real difference.
    private func same<V: View>(_ page: V, asDesign name: String) {
        AppSnapshot.assert(screen(page), size: PhoneInsets.labSize, path: name, at: AppSnapshot.designGolden("screens/\(name).png"),
                           allowed: 0.002)
    }

    // decision: PHN-416, PHN-509
    func testThePairingPageIsTheLabsAndItsProblemsSayWhatHappened() async {
        rig.phone.link = FakePhone.status(.idle, paired: false)
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("pair") { vm.ui.gate == .pair }
        same(PairView(model: vm, insets: .lab), asDesign: "pair-noon")
        vm.note(nil)
        await vm.pair("FERB-wrong").value
        rig.phone.link = FakePhone.status(.waiting, paired: false,
                                          problem: LinkProblem(code: "pair_secret_wrong", detail: "The code is wrong.", allow: nil))
        await eventually("the pairing's problem") { vm.ui.pairError != nil }
        shot(PairView(model: vm, insets: .lab), app: "pair-wrong-code-noon")
    }

    // decision: PHN-416, PHN-8
    func testTheLockIsTheLabsUnlockPage() {
        same(UnlockPage(insets: .lab, bio: "Face ID", symbol: "faceid"), asDesign: "unlock-noon")
        shot(UnlockPage(insets: .lab, bio: L.bioWord(.touchID), symbol: "touchid"), app: "unlock-touch-id-noon")
    }

    // decision: PHN-566, PHN-416
    func testThisPhoneSaysWhatArrivesWhileFERBIsClosed() async {
        let sheet = ProfileSheet(macName: "MacBook Pro", pairedSince: "10 November", insets: .lab, lockOn: true,
                                 lockTitle: L.s("profile_lock").replacingOccurrences(of: "%@", with: "Face ID"),
                                 instead: (title: L.s("background_title"), detail: L.s("background_detail"), onClick: nil))
        AppSnapshot.assert(PhoneScreen(moment: .lab(12.5), glide: false, sheet: { sheet }, page: { HomePage(insets: .lab) })
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height).stillMotion(),
                           size: PhoneInsets.labSize, path: "app/profile-noon.png")
    }

    // decision: PHN-416, PHN-418
    func testTheMacOnTheSharedRelayAndWithNoLink() async {
        rig.phone.link = FakePhone.status(.online, paired: true, path: .publicRelay)
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        shot(MacView(model: vm, waiting: nil), app: "mac-shared-relay-noon")
        rig.phone.link = FakePhone.status(.waiting, paired: true, path: .offline)
        await eventually("offline") { vm.ui.link?.phase == .waiting }
        shot(MacView(model: vm, waiting: nil), app: "mac-offline-noon")
    }

    /// "Until I'm back at my Mac" (PHN-611): the screen-use question with the third choice and its line, and the Mac
    /// tab's line with "Ask me again" while it holds (Android's question-until-back and mac-screen-use).
    // decision: PHN-611
    func testUntilImBackOnTheQuestionAndTheMacTab() async {
        let q = Question(approvalId: "ap-4", taskId: "t-1", title: "Use your screen for this?",
                         reason: "FERB needs to click in Keynote, which works only in front.", exact: nil, kind: "owner_question",
                         capability: "screen.foreground", options: ["Yes", "No"], labels: ["Use the screen", "Not now"], hidden: false,
                         needs: QuestionNeeds(canAnswer: true, needsUnlock: false, hidden: false, fingerprint: false, challenge: "ch-1",
                                              problem: nil, untilBack: true))
        let vm = rig.model()
        AppSnapshot.assert(PhoneScreen(moment: .lab(12.5), glide: false,
                                       sheet: { Asking(model: vm, q: q, insets: .lab, busy: false, close: {}) },
                                       page: { HomePage(insets: .lab) })
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height).stillMotion(),
                           size: PhoneInsets.labSize, path: "app/question-until-back-noon.png")
        rig.phone.link = FakePhone.status(.online, paired: true, path: .direct)
        rig.phone.status = PhoneStatus(locked: false, words: true, stopped: false, questions: [], running: [], results: [],
                                       screenUse: ScreenUse(active: true, since: "2026-11-17T10:00:00+05:30", phone: "phone-1",
                                                            phoneName: "iPhone", canOffer: true))
        rig.phone.view = emptyView()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        await vm.refresh()?.value
        await eventually("screen use") { vm.ui.status?.screenUse?.active == true }
        shot(MacView(model: vm, waiting: nil), app: "mac-screen-use-noon")
    }

    // ---- the privacy cover (PHN-597) ---------------------------------------------------------------------------

    private func centre<V: View>(_ v: V) -> (r: UInt8, g: UInt8, b: UInt8) {
        let image = AppSnapshot.render(v.frame(width: 200, height: 400), size: CGSize(width: 200, height: 400)).cgImage!
        let p = AppSnapshot.pixels(image)
        let i = (300 * p.width + 20) * 4
        return (p.bytes[i], p.bytes[i + 1], p.bytes[i + 2])
    }

    private var white: some View { Color.white }

    /// The app switcher and Control Center never see FERB: whenever the scene isn't active, the cover shows.
    // decision: PHN-597, PHN-15
    func testTheCoverShowsWheneverTheSceneIsNotActive() {
        let active = centre(white.ferbPrivacy(hideWhenCaptured: false, mustHide: false, capturedForTest: false).environment(\.scenePhase, .active))
        XCTAssertEqual(active.r, 255, "active: the app shows")
        for phase in [ScenePhase.inactive, .background] {
            let c = centre(white.ferbPrivacy(hideWhenCaptured: false, mustHide: false, capturedForTest: false).environment(\.scenePhase, phase))
            XCTAssertEqual([c.r, c.g, c.b], [0, 0, 0], "\(phase): covered")
        }
    }

    /// While the screen is recorded or mirrored: hidden by the owner's switch, and always for Watch live and a
    /// private answer (mustHide); never otherwise.
    // decision: PHN-597, PHN-454, PHN-110
    func testACapturedScreenHidesFERBByTheSwitchAndAlwaysForWhatMustStayPrivate() {
        func shown(switchOn: Bool, must: Bool, captured: Bool) -> Bool {
            centre(white.ferbPrivacy(hideWhenCaptured: switchOn, mustHide: must, capturedForTest: captured).environment(\.scenePhase, .active)).r == 255
        }
        XCTAssertFalse(shown(switchOn: true, must: false, captured: true))
        XCTAssertFalse(shown(switchOn: false, must: true, captured: true))
        XCTAssertTrue(shown(switchOn: false, must: false, captured: true))
        XCTAssertTrue(shown(switchOn: true, must: true, captured: false))
    }

    // ---- notifications as iOS gets them (PHN-415, PHN-417, PHN-422, PHN-566) ------------------------------------

    // decision: PHN-417, PHN-519, PHN-566
    func testARoutineQuestionGetsItsButtonsAndAWordlessCheckNone() {
        let routine = PlannedNotification(id: "q:a-1", kind: .question, title: "Send the invoice to Dana?", body: "Invented reason",
                                          publicText: "FERB has a question", wordless: false,
                                          buttons: [NotificationButton(label: "Yes", choice: 1, placeholder: nil),
                                                    NotificationButton(label: "No", choice: 0, placeholder: nil),
                                                    NotificationButton(label: "Answer", choice: nil, placeholder: "Your answer")],
                                          tap: .question("a-1"), quietly: false)
        let r = NotificationPoster.request(routine)
        XCTAssertEqual(r.identifier, "q:a-1")
        XCTAssertEqual(r.content.title, "Send the invoice to Dana?")
        XCTAssertEqual(r.content.categoryIdentifier, "q.q:a-1", "its own buttons")
        XCTAssertNotNil(r.content.sound)
        XCTAssertNil(r.trigger, "now, from the app itself: no server")
        let c = NotificationPoster.category(id: r.content.categoryIdentifier, buttons: routine.buttons, publicText: routine.publicText)
        XCTAssertEqual(c.actions.map(\.identifier), ["choice.1", "choice.0", "typed"])
        XCTAssertTrue(c.actions[2] is UNTextInputNotificationAction)
        XCTAssertEqual(c.hiddenPreviewsBodyPlaceholder, "FERB has a question", "a lock screen hiding previews shows only this")

        let check = PlannedNotification(id: "q:a-2", kind: .check, title: "FERB has a check for you", body: "Unlock your phone to see it.",
                                        publicText: "FERB has a question", wordless: true, buttons: [], tap: .question("a-2"), quietly: true)
        let r2 = NotificationPoster.request(check)
        XCTAssertEqual(r2.content.categoryIdentifier, "check", "no buttons")
        XCTAssertNil(r2.content.sound, "a quiet re-post makes no sound")
        XCTAssertEqual(r2.content.interruptionLevel, .passive)
        XCTAssertTrue(r2.content.attachments.isEmpty, "never a picture")
    }

    // ---- the catalog (PHN-567) --------------------------------------------------------------------------------------

    /// Every key the app's Swift names (L.s, L.bio, L.plural, Note) is in the one catalog.
    // decision: PHN-567
    func testEveryKeyTheAppNamesIsInTheCatalog() throws {
        let sources = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Sources").standardized
        let pattern = try NSRegularExpression(pattern: #"(?:L\.(?:s|bio|plural)|Note)\(\s*"([a-z_0-9]+)""#)
        var keys: Set<String> = []
        let walker = FileManager.default.enumerator(at: sources, includingPropertiesForKeys: nil)!
        for case let url as URL in walker where url.pathExtension == "swift" {
            let text = try String(contentsOf: url, encoding: .utf8)
            for m in pattern.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
                keys.insert(String(text[Range(m.range(at: 1), in: text)!]))
            }
        }
        XCTAssertGreaterThan(keys.count, 100)
        let missing = keys.filter { !L.has($0) }.sorted()
        XCTAssertEqual(missing, [], "keys the app names that the catalog lacks")
        XCTAssertEqual(L.plural("tr_unreadable", 1), "1 file couldn't be read, so it wasn't sent.")
        XCTAssertEqual(L.plural("tr_unreadable", 3), "3 files couldn't be read, so they weren't sent.")
        XCTAssertEqual(L.plural("search_skipped", 2, .faceID), "2 places that need Face ID weren't searched.")
        XCTAssertEqual(L.bio("files_key_place", .touchID), "Asks for Touch ID each time")
        XCTAssertEqual(L.s("outbox_failed", "the Mac said no"), "Not sent: the Mac said no")
    }
}
#endif
