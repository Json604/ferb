#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// The temporary chat (the owner, 2026-11-23): one chat the Mac holds in memory only, turned on and off only by the
/// chat's dashed bubble. While on, the chat tab shows its banner and only its rows, and the box sends to it; off, the
/// normal chat comes back as it was. A relaunch (or another device) finds it on through the library; nothing of it is
/// kept on the phone.
@MainActor
final class TemporaryChatTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func normalChat() -> TranscriptView {
        TranscriptView(conversation: "c1", items: [FakePhone.ownerTurn("n1", "Book the dentist"),
                                                   .notice(notice: NoticeItem(entry: "n2", at: Date().timeIntervalSinceReferenceDate, legacy: false,
                                                                              reference: "n2", after: nil, text: "Booked for Friday.", quotes: [], cards: nil))],
                       questions: [], live: [], outbox: [], older: .complete)
    }

    private func opened() async -> AppModel {
        phone.view = normalChat()
        let vm = rig.model()
        rig.session.unlocked()
        await vm.refresh()?.value
        await eventually("the normal chat") { vm.rows.contains { $0.key == "n2" } }
        return vm
    }

    // decision: PHN-416
    // decision: PHN-818
    func testTheBubbleShowsOnlyTheTemporaryChatAndTheSameBubbleEndsIt() async {
        let vm = await opened()
        XCTAssertNil(vm.temporary)
        XCTAssertEqual(temporaryToggle(vm).label, "Temporary chat")
        XCTAssertFalse(temporaryToggle(vm).on)
        let normal = vm.rows

        await vm.toggleTemporary().value
        XCTAssertTrue(phone.calls.contains("temporary-set:true"))
        XCTAssertEqual(vm.temporary?.id, "tmp1")
        XCTAssertEqual(vm.temporary?.rows, [], "it starts empty")
        XCTAssertFalse(vm.home, "the chat tab shows the temporary chat, never the home page")
        XCTAssertEqual(temporaryToggle(vm).label, "End temporary chat")
        XCTAssertTrue(temporaryToggle(vm).on)

        // the box sends to it, never through the outbox
        phone.temporaryReply = "Here's a fresh answer."
        vm.typed("What is 2+2?")
        await vm.send().value
        XCTAssertTrue(phone.calls.contains("temporary-send:What is 2+2?"))
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("send:") })
        XCTAssertTrue(phone.outbox.isEmpty)
        XCTAssertEqual(vm.draft, "")
        let shown = vm.temporary?.shown(vm.ui.questions) ?? []
        XCTAssertEqual(shown.map(\.key).filter { !$0.hasPrefix("t-e") }, [], "only the temporary chat's rows")
        XCTAssertTrue(shown.contains { if case .owner(_, let t, _, _) = $0 { return t == "What is 2+2?" } else { return false } })
        XCTAssertEqual(vm.rows, normal, "the normal chat is untouched underneath")

        // the same bubble ends it: the normal chat as it was
        await vm.toggleTemporary().value
        XCTAssertTrue(phone.calls.contains("temporary-set:false"))
        XCTAssertNil(vm.temporary)
        XCTAssertEqual(vm.rows, normal)
        XCTAssertEqual(temporaryToggle(vm).label, "Temporary chat")
    }

    /// Its questions are its own: shown in it, never loose in the normal chat.
    // decision: PHN-770
    // decision: PHN-818
    func testATemporaryChatsQuestionShowsOnlyInIt() async {
        let vm = await opened()
        await vm.toggleTemporary().value
        vm.typed("Pay the bill")
        await vm.send().value
        let task = "tmp-task-0"
        let q = Question(approvalId: "ap-t", taskId: task, title: "Pay $5?", reason: "", exact: nil, kind: "approve", capability: nil,
                         options: ["Yes", "No"], labels: [], hidden: false)
        let other = Question(approvalId: "ap-n", taskId: "t-normal", title: "Send it?", reason: "", exact: nil, kind: "approve",
                             capability: nil, options: ["Yes", "No"], labels: [], hidden: false)
        let shown = vm.temporary!.shown([q, other])
        XCTAssertEqual(shown.last?.key, "q:ap-t")
        XCTAssertFalse(shown.contains { $0.key == "q:ap-n" })
        XCTAssertTrue(vm.temporary!.owns(q))
        XCTAssertFalse(vm.temporary!.owns(other))
    }

    /// The Mac's status marks a temporary chat's questions and running tasks (`temporary`): they show only in it, and
    /// the chat's work row never counts them; its own work row has only them.
    // decision: PHN-770, PHN-725
    // decision: PHN-818
    func testTheStatusRowsOfATemporaryChatShowOnlyInIt() async {
        let vm = await opened()
        await vm.toggleTemporary().value
        rig.phone.status = PhoneStatus(locked: false, words: true, stopped: false,
                                       questions: [StatusQuestion(approvalId: "ap-s", kind: "approve", capability: nil, taskId: "t-x", reason: "Pay $5?",
                                                                  options: ["Yes", "No"], held: false, needs: needs(unlock: false), temporary: true),
                                                   asked("ap-n", needs(unlock: false))],
                                       running: [StatusTask(taskId: "t-x", name: "Pay the bill", goal: "Pay the bill", startedAt: nil, held: false, temporary: true),
                                                 StatusTask(taskId: "t-n", name: "Book a table", goal: "Book a table", startedAt: nil, held: false)],
                                       results: [], screenUse: nil)
        await vm.refresh()?.value
        await eventually("the status") { vm.ui.questions.count == 2 }
        let temp = vm.ui.questions.first { $0.approvalId == "ap-s" }!
        XCTAssertTrue(temp.temporary)
        XCTAssertEqual(vm.temporary!.shown(vm.ui.questions).map(\.key), ["q:ap-s"], "only its own question")
        XCTAssertFalse(vm.temporary!.owns(vm.ui.questions.first { $0.approvalId == "ap-n" }!))
        XCTAssertEqual(vm.ui.running?.tasks.compactMap(\.taskId), ["t-n"], "the chat's work row never counts it")
        await eventually("its own work row") { vm.temporaryRunning?.tasks.compactMap(\.taskId) == ["t-x"] }
    }

    /// The owner's live case (2026-11-23): in the temporary chat FERB's questions sat above his newest message and
    /// stayed after he answered them. Its questions are at its end, after everything said, as FERB.app draws them; once
    /// the Mac's temporary chat says one is answered (its rev moved: the library's Change.temporary), it is gone.
    // decision: PHN-824
    // decision: PHN-818
    func testItsQuestionsAreAtItsEndAndGoOnceAnswered() async throws {
        func chat(_ answered: Set<String>, rev: Int64) throws -> TemporaryChat {
            var t = try FixtureMac.transcript(FixtureMac(try .looseQuestions(answered: answered)))
            t.conversation = "temporary:tmp7"
            return TemporaryChat(on: true, id: "tmp7", startedAt: "2026-11-23T10:00:00+05:30", rev: rev, transcript: t)
        }
        phone.temporaryNow = try chat([], rev: 1)
        let vm = await opened()
        await eventually("the temporary chat") { vm.temporary?.id == "tmp7" }
        let rows = vm.temporary!.shown(vm.ui.questions)
        let tidy = rows.firstIndex { if case .owner(_, let t, _, _) = $0 { return t == "tidy my desktop" } else { return false } }!
        let asks = rows.enumerated().compactMap { i, r -> (String, Int)? in
            if case .asking(_, let id, _, _, _) = r { return (id, i) } else { return nil }
        }
        XCTAssertEqual(asks.map(\.0), ["ap-1", "ap-2"])
        XCTAssertTrue(asks.allSatisfy { $0.1 > tidy }, "below his newer message")

        phone.temporaryNow = try chat(["ap-1"], rev: 2)
        phone.emit(.temporary)
        await eventually("answered: gone") {
            vm.temporary?.shown(vm.ui.questions).compactMap { r -> String? in
                if case .asking(_, let id, _, _, _) = r { return id } else { return nil }
            } == ["ap-2"]
        }
    }

    /// Closing the app or locking the phone never ends it: a new launch reads it from the Mac and shows it; when it
    /// ends elsewhere (the library's Change.temporary), the normal chat comes back.
    // decision: PHN-416
    // decision: PHN-818
    func testARelaunchWithTheTemporaryChatOnShowsIt() async {
        phone.temporaryNow = TemporaryChat(on: true, id: "tmp9", startedAt: "2026-11-23T10:00:00+05:30", rev: 4,
                                           transcript: TranscriptView(conversation: "temporary:tmp9", items: [FakePhone.ownerTurn("t-e0", "Still here?")],
                                                                      questions: [], live: [], outbox: [], older: .complete))
        let vm = await opened()
        await eventually("the temporary chat read at launch") { vm.temporary?.id == "tmp9" }
        XCTAssertEqual(vm.temporary?.rows.map(\.key), ["t-e0"])
        XCTAssertFalse(vm.home)

        // locked and opened again: still on, read again
        rig.session.lockNow()
        await settle()
        XCTAssertEqual(vm.temporary?.id, "tmp9", "locking never ends it")
        let reads = phone.calls.filter { $0 == "temporary" }.count
        rig.session.unlocked()
        await eventually("read again at the unlock") { phone.calls.filter { $0 == "temporary" }.count > reads }
        XCTAssertEqual(vm.temporary?.id, "tmp9")

        // ended on the Mac
        phone.temporaryNow = FakePhone.temporaryOff
        phone.emit(.temporary)
        await eventually("off") { vm.temporary == nil }
        XCTAssertTrue(vm.rows.contains { $0.key == "n2" }, "the normal chat")
    }

    /// Nothing of it is kept on the phone: not the draft in scene storage, not the preferences, not a file, not the
    /// outbox. A failed toggle says so and changes nothing.
    // decision: PHN-416
    // decision: PHN-818
    func testNothingTemporaryIsStoredOnThePhone() async {
        let vm = await opened()
        await vm.toggleTemporary().value
        let secret = "temporary words 7f3a"
        phone.temporaryReply = "reply \(secret)"
        vm.typed(secret)
        XCTAssertEqual(vm.saved.draft, "", "what is typed in it never reaches scene storage")
        await vm.send().value
        vm.typed("half typed \(secret)")
        XCTAssertEqual(vm.saved.draft, "")
        XCTAssertTrue(phone.outbox.isEmpty)

        let defaults = UserDefaults(suiteName: rig.suite)!.persistentDomain(forName: rig.suite) ?? [:]
        XCTAssertFalse(defaults.values.contains { "\($0)".contains(secret) }, "nothing in the preferences")
        var files: [URL] = []
        if let e = FileManager.default.enumerator(at: rig.root, includingPropertiesForKeys: nil) {
            for case let u as URL in e { files.append(u) }
        }
        for f in files {
            if let d = try? Data(contentsOf: f) { XCTAssertNil(d.range(of: Data(secret.utf8)), "nothing in \(f.lastPathComponent)") }
        }

        // a failed end: said, and it stays on
        phone.temporaryRefusal = .Offline(detail: "no", linkCode: nil)
        await vm.toggleTemporary().value
        XCTAssertEqual(vm.ui.note?.key, "temporary_end_failed")
        XCTAssertNotNil(vm.temporary)
        await vm.toggleTemporary().value
        XCTAssertNil(vm.temporary)
        XCTAssertEqual(vm.saved.draft, "half typed \(secret)", "off, the box is the normal chat's again")

        // a failed start: said, and the normal chat stays
        phone.temporaryRefusal = .Offline(detail: "no", linkCode: nil)
        await vm.toggleTemporary().value
        XCTAssertEqual(vm.ui.note?.key, "temporary_start_failed")
        XCTAssertNil(vm.temporary)
    }

    /// His message in the temporary chat shows at once as his outbox row at its end, as the chat shows it (APP-1126:
    /// one chat, another data source), so the list goes to it as for any message of his (the list's `mine`). Once the
    /// send is answered, the Mac's row is there in its place; a send that fails takes it away and puts the words back.
    // decision: APP-1126
    // decision: PHN-818
    func testHisMessageShowsAtOnceAsHisOutboxRowAsInTheChat() async {
        let vm = await opened()
        await vm.toggleTemporary().value
        let gate = Latch()
        phone.temporaryGate = gate
        vm.typed("What is 2+2?")
        let sent = vm.send()
        func outbox() -> [OutboxItem] { vm.temporary?.rows.compactMap { if case .outbox(_, let o) = $0 { o } else { nil } } ?? [] }
        await eventually("on its way") { outbox().map(\.text) == ["What is 2+2?"] }
        XCTAssertEqual(outbox().first?.state, "sending")
        XCTAssertTrue(vm.temporary!.rows.last.map { if case .outbox = $0 { true } else { false } } == true, "at the end")
        XCTAssertTrue(phone.outbox.isEmpty, "never the phone's outbox")
        XCTAssertEqual(vm.draft, "")
        // a change of the chat meanwhile shows it once
        phone.emit(.temporary)
        gate.release()
        await sent.value
        await eventually("the Mac's row in its place") {
            outbox().isEmpty && vm.temporary?.rows.contains { if case .owner(_, let t, _, _) = $0 { t == "What is 2+2?" } else { false } } == true
        }
        XCTAssertEqual(vm.temporary?.rows.filter { if case .owner = $0 { true } else { false } }.count, 1)

        // a send that fails: its row goes, the words come back
        phone.temporaryGate = nil
        phone.temporaryRefusal = .Offline(detail: "FERB's Mac can't be reached now.", linkCode: "mac_unreachable")
        vm.typed("again")
        await vm.send().value
        XCTAssertTrue(outbox().isEmpty)
        XCTAssertEqual(vm.draft, "again")
    }

    // decision: PHN-818
    func testTheStringsAreTheContracts() {
        XCTAssertEqual(L.s("temporary_chat"), "Temporary chat")
        XCTAssertEqual(L.s("temporary_chat_end"), "End temporary chat")
        XCTAssertEqual(L.s("temporary_banner"), "Temporary chat · FERB won't use what it knows or keep this chat")
        XCTAssertEqual(L.s("temporary_empty"), "This chat starts fresh. Tap the dashed bubble again to end it.")
        XCTAssertEqual(L.s("temporary_start_failed"), "Couldn't start a temporary chat. Try again.")
        XCTAssertEqual(L.s("temporary_end_failed"), "Couldn't end the temporary chat. Try again.")
    }

    /// The chat tab while a temporary chat is on and empty (noon): the bar with the bubble on, the banner, the empty
    /// line, the box; and the bar with the bubble off and on (night). Drawn with the lab's still box: the app's own text
    /// field can't draw offscreen.
    // decision: PHN-416
    // decision: PHN-818
    func testGoldenTheTemporaryChat() {
        let on = TemporaryToggle(on: true, label: L.s("temporary_chat_end")) {}
        let off = TemporaryToggle(on: false, label: L.s("temporary_chat")) {}
        let page = PhoneScreen(moment: .lab(12.5), glide: false) {
            VStack(spacing: 0) {
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {}, temporary: on)
                TemporaryBanner(L.s("temporary_banner")).padding(.top, 10)
                TemporaryEmpty(L.s("temporary_empty")).frame(maxHeight: .infinity)
                PhoneComposer(placeholder: L.s("chat_placeholder")).padding(.horizontal, 16).padding(.bottom, PhoneInsets.lab.bottom + 8)
            }
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        AppSnapshot.assert(page, size: PhoneInsets.labSize, path: "app/temporary-chat-noon.png")

        let bars = PhoneScreen(moment: .lab(22.5), glide: false) {
            VStack(spacing: 22) {
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {})
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {}, temporary: off)
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {}, temporary: on)
                TemporaryBanner(L.s("temporary_banner"))
                Spacer()
            }
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        AppSnapshot.assert(bars, size: PhoneInsets.labSize, path: "app/temporary-bubble-night.png")
    }
}
#endif
