import FerbAppModel
import FerbMobile
import Foundation
import XCTest

/// Files picked for the chat wait in the message box and go only with the message (kb PHN-690, PHN-691; Android's
/// SendWithFilesTest): on the fake phone, what is uploaded, what is sent, and how a message with files stands when the
/// Mac is away, the path is the shared relay, he stops it, or FERB closes while its files go.
@MainActor
final class SendWithFilesTests: XCTestCase {
    private var rig: Rig!
    private var picks: URL!

    override func setUp() async throws {
        rig = Rig()
        rig.session.unlocked()
        rig.phone.view = TranscriptView(conversation: "c1", items: [], questions: [], live: [], outbox: [], older: .complete)
        picks = rig.root.deletingLastPathComponent().appendingPathComponent("picks-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: picks, withIntermediateDirectories: true)
        try? Data(repeating: 1, count: 30).write(to: picks.appendingPathComponent("IMG_1.jpg"))
        try? Data(repeating: 2, count: 20).write(to: picks.appendingPathComponent("IMG_2.jpg"))
    }

    override func tearDown() async throws {
        rig.phone.uploadGate?.release()
        try? FileManager.default.removeItem(at: picks)
        rig.tearDown()
    }

    private var phone: FakePhone { rig.phone }
    private var uploads: [String] { phone.calls.filter { $0.hasPrefix("upload") } }

    private func pickTwo(_ vm: AppModel) async {
        await vm.sendFiles([picks.appendingPathComponent("IMG_1.jpg"), picks.appendingPathComponent("IMG_2.jpg")], forChat: true).value
        await eventually("both in the box") { vm.outgoing.chips.count == 2 }
    }

    // decision: PHN-690
    func testPickedFilesWaitInTheBoxAndNothingLeavesThePhone() async {
        let vm = rig.model()
        await pickTwo(vm)
        XCTAssertEqual(vm.outgoing.chips.map(\.name), ["IMG_1.jpg", "IMG_2.jpg"])
        XCTAssertEqual(vm.outgoing.chips.map(\.size), [30, 20])
        XCTAssertTrue(vm.outgoing.chips.allSatisfy { FileManager.default.fileExists(atPath: $0.path) }, "copied here")
        XCTAssertTrue(uploads.isEmpty && phone.sentWith.isEmpty && vm.transfers.list.isEmpty)
    }

    // decision: PHN-690
    func testTheCrossTakesAFileOutOfTheBoxAndItsCopyWithIt() async {
        let vm = rig.model()
        await pickTwo(vm)
        let gone = vm.outgoing.chips[0]
        vm.unpick(gone.key)
        XCTAssertEqual(vm.outgoing.chips.map(\.name), ["IMG_2.jpg"])
        await eventually("its copy is gone") { !FileManager.default.fileExists(atPath: gone.path) }
        XCTAssertTrue(uploads.isEmpty && phone.sentWith.isEmpty)
    }

    // decision: PHN-690, PHN-691
    func testSendUploadsTheFilesThenSendsTheMessageWithTheirIds() async {
        let vm = rig.model()
        await pickTwo(vm)
        vm.typed("Here are the scans")
        await vm.send().value
        XCTAssertTrue(vm.outgoing.chips.isEmpty, "the box is empty at once")
        XCTAssertEqual(vm.draft, "")
        await eventually("sent") { !phone.sentWith.isEmpty }
        XCTAssertEqual(uploads, ["upload:IMG_1.jpg:-", "upload:IMG_2.jpg:-"])
        XCTAssertEqual(phone.sentWith.first?.0, "Here are the scans")
        XCTAssertEqual(phone.sentWith.first?.1.map(\.uploadId), ["up-IMG_1.jpg", "up-IMG_2.jpg"])
        await eventually("the message left the phone's list") { vm.outgoing.messages.isEmpty }
        XCTAssertTrue(vm.transfers.list.isEmpty)
    }

    // decision: PHN-691
    func testWithTheMacAwayItWaitsAndGoesWhenTheMacIsBack() async {
        phone.link = FakePhone.status(.waiting)
        let vm = rig.model()
        await pickTwo(vm)
        await vm.send().value
        await eventually("waiting") { vm.outgoing.messages.first?.phase == .waitingLink }
        XCTAssertTrue(uploads.isEmpty)
        phone.link = FakePhone.status()
        await eventually("sent once the Mac is back") { phone.sentWith.count == 1 }
        XCTAssertEqual(uploads.count, 2)
    }

    /// A reply with files (kb APP-1146): the quote waits with the message while its files wait, is kept across FERB
    /// closing, and goes as its reply_to.
    // decision: APP-1146, PHN-691
    func testAReplyWithFilesKeepsItsQuoteWhileItWaitsAndAcrossAClose() async {
        phone.link = FakePhone.status(.waiting)
        let quote = ReplyQuote(entryId: "a1", kind: "answer", byOwner: false, excerpt: "Booked the 7pm table at Nori.")
        let first = rig.model()
        await pickTwo(first)
        first.reply(to: quote)
        first.typed("Here's the menu")
        await first.send().value
        XCTAssertNil(first.shownQuote, "it went with the message")
        await eventually("waiting") { first.outgoing.messages.first?.phase == .waitingLink }
        XCTAssertEqual(first.outgoing.messages.first?.replyTo, quote, "drawn with it meanwhile")
        let kept = rig.local.root.appendingPathComponent("messages.json")
        await eventually("kept") { ((try? String(contentsOf: kept, encoding: .utf8)) ?? "").contains("Booked the 7pm table") }
        // FERB closes here: a new process reads what was kept
        let vm = rig.model()
        await eventually("the message is back") { !vm.outgoing.messages.isEmpty }
        XCTAssertEqual(vm.outgoing.messages.first?.replyTo, quote)
        phone.link = FakePhone.status()
        await eventually("sent once the Mac is back") { !phone.sentWith.isEmpty }
        XCTAssertEqual(phone.sentQuotes.first, quote)
    }

    // decision: PHN-691, PHN-410
    func testOnTheSharedRelayFilesWaitAndTheWordsCanGoAlone() async {
        phone.link = FakePhone.status(path: .publicRelay)
        let vm = rig.model()
        await pickTwo(vm)
        vm.typed("For later")
        await vm.send().value
        await eventually("waiting for a path") { vm.outgoing.messages.first?.phase == .waitingPath }
        XCTAssertTrue(uploads.isEmpty, "no file on the shared relay")
        let id = vm.outgoing.messages[0].id
        await vm.sendTextOnly(id)?.value
        await eventually("the words went") { phone.calls.contains("send:For later") }
        await eventually("and the message with files is gone") { vm.outgoing.messages.isEmpty }
        XCTAssertTrue(uploads.isEmpty && phone.sentWith.isEmpty)
    }

    // decision: PHN-691, PHN-444
    func testOnTheSharedRelayAMessageWithFilesGoesByItselfOnABetterPath() async {
        phone.link = FakePhone.status(path: .publicRelay)
        let vm = rig.model()
        await pickTwo(vm)
        await vm.send().value
        await eventually("waiting for a path") { vm.outgoing.messages.first?.phase == .waitingPath }
        phone.link = FakePhone.status(path: .ownRelay)
        await eventually("sent") { phone.sentWith.count == 1 }
    }

    // decision: PHN-691
    func testCancelWhileTheFilesGoKeepsTheMessageNotSentAndTryAgainGoesOn() async {
        phone.uploadGate = Latch()
        let vm = rig.model()
        await pickTwo(vm)
        await vm.send().value
        await eventually("half way") { (vm.outgoing.messages.first?.done ?? 0) > 0 }
        let id = vm.outgoing.messages[0].id
        vm.stopSending(id)
        await eventually("stopped") { vm.outgoing.messages.first?.phase == .stopped }
        await settle()
        XCTAssertEqual(vm.outgoing.messages.first?.phase, .stopped)
        XCTAssertTrue(phone.sentWith.isEmpty, "nothing sent")
        phone.uploadGate = nil
        vm.retrySending(id)
        await eventually("sent") { phone.sentWith.count == 1 }
        XCTAssertTrue(uploads.contains("upload:IMG_1.jpg:up-IMG_1.jpg"), "went on from its upload id: \(uploads)")
    }

    // decision: PHN-691
    func testFerbClosedWhileTheFilesWentSaysSoAndTryAgainGoesOnWhereItStopped() async {
        phone.uploadGate = Latch()
        let first = rig.model()
        await pickTwo(first)
        first.typed("Scans")
        await first.send().value
        let kept = rig.local.root.appendingPathComponent("messages.json")
        await eventually("half way, its upload id kept") { ((try? String(contentsOf: kept, encoding: .utf8)) ?? "").contains("up-IMG_1.jpg") }
        // FERB closes here: a new process reads what was kept
        let gate = phone.uploadGate
        phone.uploadGate = nil
        let vm = rig.model()
        await eventually("the message is back") { !vm.outgoing.messages.isEmpty }
        XCTAssertEqual(vm.outgoing.messages.first?.phase, .closed)
        XCTAssertEqual(vm.outgoing.messages.first?.text, "Scans")
        XCTAssertTrue(phone.sentWith.isEmpty)
        vm.retrySending(vm.outgoing.messages[0].id)
        await eventually("sent") { !phone.sentWith.isEmpty }
        XCTAssertTrue(uploads.contains("upload:IMG_1.jpg:up-IMG_1.jpg"), "went on from its upload id: \(uploads)")
        gate?.release()
    }

    // decision: PHN-692
    func testTheFilesTabsSendIsTheSendAndGoesAtOnce() async {
        let vm = rig.model()
        await vm.sendFiles([picks.appendingPathComponent("IMG_1.jpg")], forChat: false).value
        await eventually("on the Mac") { vm.transfers.list.first?.phase == .done }
        XCTAssertEqual(uploads, ["upload:IMG_1.jpg:-"])
        XCTAssertTrue(vm.outgoing.chips.isEmpty, "never in the message box")
    }

    /// The owner (2026-11-24, ORCH-715): the temporary chat is the chat except that nothing is saved. Its box has the
    /// chat's +: picked files upload as the chat's do, then the message goes with their ids, `fresh` (the Mac moves them
    /// into the temporary chat's own folder and deletes them when it ends), never the outbox; the message is never
    /// written on the phone. Before, the iPhone hid + in the temporary chat.
    // decision: ORCH-715
    // decision: PHN-838
    func testInTheTemporaryChatPlusSendsFilesAsTheChatDoes() async {
        let vm = rig.model()
        await vm.toggleTemporary().value
        XCTAssertNotNil(vm.temporary)
        await pickTwo(vm)
        vm.typed("What are these")
        await vm.send().value
        XCTAssertTrue(vm.outgoing.chips.isEmpty, "the box is empty at once")
        await eventually("sent") { phone.calls.contains("temporary-send:What are these") }
        XCTAssertEqual(uploads, ["upload:IMG_1.jpg:-", "upload:IMG_2.jpg:-"])
        XCTAssertEqual(phone.temporaryFiles, [["up-IMG_1.jpg", "up-IMG_2.jpg"]])
        XCTAssertTrue(phone.sentWith.isEmpty && phone.outbox.isEmpty, "never the chat's send or the outbox")
        await eventually("the message left the phone's list") { vm.outgoing.messages.isEmpty }
        let kept = rig.root.appendingPathComponent("messages.json")
        let text = (try? String(contentsOf: kept, encoding: .utf8)) ?? ""
        XCTAssertFalse(text.contains("What are these"), "never written on the phone")
    }

    /// Its message on its way shows only in the temporary chat; the chat ending (on any device) takes it away with its
    /// copies.
    // decision: PHN-838
    func testATemporaryMessageWithFilesShowsOnlyThereAndGoesWhenTheChatEnds() async {
        phone.link = FakePhone.status(.waiting)
        let vm = rig.model()
        await vm.toggleTemporary().value
        await pickTwo(vm)
        await vm.send().value
        await eventually("waiting") { vm.outgoing.messages.first?.phase == .waitingLink }
        XCTAssertTrue(vm.outgoing.messages.first?.temporary == true)
        XCTAssertFalse(vm.rows.contains { if case .withFiles = $0 { true } else { false } }, "not in the normal chat")
        let copies = vm.outgoing.messages.first?.chips.map(\.path) ?? []
        phone.temporaryNow = FakePhone.temporaryOff
        phone.emit(.temporary)
        await eventually("gone with the chat") { vm.outgoing.messages.isEmpty }
        await eventually("its copies too") { copies.allSatisfy { !FileManager.default.fileExists(atPath: $0) } }
        XCTAssertTrue(phone.sentWith.isEmpty && !phone.calls.contains { $0.hasPrefix("temporary-send") })
    }
}
