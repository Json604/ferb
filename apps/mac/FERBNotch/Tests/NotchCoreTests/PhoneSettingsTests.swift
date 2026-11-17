import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Settings → Phone (stage 3 S5, kb T-1205): the page's model over the daemon's phone area (`phone.*`, journal 106) and
/// the phone link's helper (`link.*`, the seam S4 provides), both stood in by scripted transports; time is injected.
@MainActor
final class PhoneSettingsTests: XCTestCase {
    private var date = Date(timeIntervalSince1970: 1_800_000_000)
    private let daemon = ScriptedRPC()
    private let link = ScriptedRPC()

    private func model(link hasLink: Bool = true) -> PhoneSettingsModel {
        PhoneSettingsModel(service: RPCPhoneService(daemon: daemon, link: hasLink ? link : nil), now: { [unowned self] in date })
    }

    private func phone(_ id: String, _ name: String, unused: Bool = false) -> [String: Any] {
        ["id": id, "name": name, "app": "android", "paired_at": "2026-11-10T09:00:00+05:30",
         "last_seen": "2026-11-16T09:00:00.250000+05:30", "unused_warning": unused]
    }

    private func list(_ phones: [[String: Any]], pairing: Any = NSNull()) -> [String: Any] {
        ["phones": phones, "pairing": pairing, "max": 5, "unused_days": 30]
    }

    private let invite: [String: Any] = ["code": "FERB1-ABCDEF", "url": "ferb://pair?x", "qr": ["1010", "0101", "1010", "0101"],
                                         "qr_old": ["1"], "seconds": 120]
    private let asking: [String: Any] = ["request_id": "r1", "phone": "ep-new", "name": "Pixel 8", "app": "android",
                                         "code": "482 913", "seconds_left": 100]

    private func showCode(_ page: PhoneSettingsModel) async {
        daemon.replies["phone.list"] = list([])
        link.replies["phone.pair_start"] = invite
        await page.load()
        await page.startPairing()
    }

    // decision: PHN-142
    func testPairingShowsTheLinksCodeCountsDownAndClosesWhenItsTimeIsUp() async throws {
        let page = model()
        await showCode(page)
        guard case let .showing(shown, _) = page.pairing else { return XCTFail("\(page.pairing)") }
        XCTAssertEqual(shown.code, "FERB1-ABCDEF")
        XCTAssertEqual(shown.rows, ["1010", "0101", "1010", "0101"])
        XCTAssertEqual(page.secondsLeft, 120)
        date += 59.5
        await page.tick()
        XCTAssertEqual(page.secondsLeft, 61)
        date += 61
        await page.tick()
        XCTAssertEqual(page.pairing, .ended(PhoneWords.expired))
        XCTAssertEqual(link.methods().filter { $0 == "phone.pair_cancel" }.count, 1)
        // PHN-142: whole points per module, a four-module margin, about 320 points wide
        XCTAssertEqual(PairingCodeView.module(25), 9)
        XCTAssertEqual(PairingCodeView.module(57), 4)
    }

    // decision: PHN-419
    // decision: PHN-480
    func testAPhoneThatAsksIsConfirmedOnlyByTheOwnersYes() async throws {
        let page = model()
        await showCode(page)
        daemon.replies["phone.list"] = list([], pairing: asking)
        await page.tick()
        await page.tick()                                   // the list is read every other second while a code shows
        guard case let .asking(request, _) = page.pairing else { return XCTFail("\(page.pairing)") }
        XCTAssertEqual(request.name, "Pixel 8")
        XCTAssertEqual(request.code, "482 913")
        XCTAssertEqual(page.secondsLeft, 100)
        XCTAssertFalse(daemon.methods().contains("phone.pair_decide"))   // nothing answers for the owner
        daemon.replies["phone.pair_decide"] = ["phone": "ep-new", "name": "Pixel 8", "paired": true]
        daemon.replies["phone.list"] = list([phone("ep-new", "Pixel 8")])
        await page.decide(yes: true)
        let decided = try XCTUnwrap(daemon.params(of: "phone.pair_decide").last)
        XCTAssertEqual(decided["yes"] as? Bool, true)
        XCTAssertEqual(decided["request_id"] as? String, "r1")
        XCTAssertEqual(page.pairing, .ended(PhoneWords.paired("Pixel 8")))
        XCTAssertEqual(page.phones.map(\.name), ["Pixel 8"])
    }

    // decision: PHN-419
    func testNoAndAnUnansweredRequestLeaveThePhoneUnpaired() async throws {
        let page = model()
        await showCode(page)
        await page.receive(topic: "phone.pair_requested", data: asking)
        daemon.replies["phone.pair_decide"] = ["phone": "ep-new", "name": "Pixel 8", "paired": false]
        await page.decide(yes: false)
        XCTAssertEqual(daemon.params(of: "phone.pair_decide").last?["yes"] as? Bool, false)
        XCTAssertEqual(page.pairing, .ended(PhoneWords.notPaired))
        XCTAssertTrue(link.methods().contains("phone.pair_cancel"))

        await page.receive(topic: "phone.pair_requested", data: asking)
        date += 121
        await page.tick()
        XCTAssertEqual(page.pairing, .ended(PhoneWords.noAnswer))

        // the daemon let go of it first (the phone left): said, never answered
        await page.receive(topic: "phone.pair_requested", data: asking)
        daemon.replies["phone.list"] = list([])
        await page.tick(); await page.tick()
        XCTAssertEqual(page.pairing, .ended(PhoneWords.requestEnded))
        XCTAssertEqual(daemon.methods().filter { $0 == "phone.pair_decide" }.count, 1)
    }

    // decision: PHN-419
    func testASixthPhoneIsRefusedBeforeAnyCodeOpens() async throws {
        let page = model()
        daemon.replies["phone.list"] = list((1...5).map { phone("ep\($0)", "Phone \($0)") })
        await page.load()
        XCTAssertTrue(page.isFull)
        await page.startPairing()
        XCTAssertEqual(page.pairing, .ended(PhoneWords.full(5)))
        XCTAssertFalse(link.methods().contains("phone.pair_start"))
        // and when the daemon's own count refuses the Yes (another phone was added meanwhile)
        daemon.replies["phone.list"] = list([phone("ep1", "Phone 1")])
        await page.load()
        await page.receive(topic: "phone.pair_requested", data: asking)
        daemon.failures["phone.pair_decide"] = DaemonRPCError.remote("FERB already has 5 paired phones", code: -32023)
        await page.decide(yes: true)
        XCTAssertEqual(page.pairing, .ended(PhoneWords.full(5)))
    }

    // decision: PHN-419
    func testTheListFollowsEventsWithItsWarningAndEachPhonesConnection() async throws {
        let page = model()
        daemon.replies["phone.list"] = list([phone("ep1", "Pixel 8"), phone("ep2", "Old tablet", unused: true)])
        link.replies["phone.link_status"] = ["relay": ["state": "own"], "own_relay": ["url": "https://relay.example.com", "token_set": true],
                                      "own_relay_always": false,
                                      "devices": [["id": "ep1", "online": true, "tier": "own_relay"], ["id": "ep2", "online": false]]]
        await page.load()
        XCTAssertEqual(page.phones.map(\.unusedWarning), [false, true])
        XCTAssertEqual(page.phones.map(\.connection), [.ownRelay, PhoneConnection.none])
        XCTAssertNotNil(page.phones[1].lastSeen)               // Python-style microseconds are read
        XCTAssertEqual(page.link?.line, PhoneWords.statusOwn)
        XCTAssertEqual(page.link?.ownRelay, "https://relay.example.com")
        daemon.replies["phone.list"] = list([phone("ep1", "Pixel 8")])
        await page.receive(topic: "phone.removed", data: ["phone": "ep2", "name": "Old tablet"])
        XCTAssertEqual(page.phones.map(\.id), ["ep1"])
        XCTAssertEqual(page.phones.first?.connection, .ownRelay)
        let reads = daemon.methods().filter { $0 == "phone.list" }.count
        await page.receive(topic: "phone.unused", data: ["phone": "ep1"])
        XCTAssertEqual(daemon.methods().filter { $0 == "phone.list" }.count, reads + 1)
    }

    // decision: PHN-419
    func testRenameAndRemoveGoToTheDaemonAndTheListIsReadAgain() async throws {
        let page = model()
        daemon.replies["phone.list"] = list([phone("ep1", "Pixel 8")])
        await page.load()
        daemon.replies["phone.rename"] = ["id": "ep1", "name": "Work phone"]
        await page.rename("ep1", to: "  Work phone ")
        XCTAssertEqual(daemon.params(of: "phone.rename").last?["name"] as? String, "Work phone")
        XCTAssertEqual(page.phones.first?.name, "Work phone")
        await page.rename("ep1", to: "   ")                    // an empty name is never sent
        XCTAssertEqual(daemon.params(of: "phone.rename").count, 1)
        daemon.replies["phone.list"] = list([])
        await page.remove("ep1")
        XCTAssertEqual(daemon.params(of: "phone.remove").last?["id"] as? String, "ep1")
        XCTAssertTrue(page.phones.isEmpty)
        XCTAssertEqual(page.note, PhoneWords.removed("Work phone"))
    }

    // decision: PHN-205
    // decision: PHN-453
    // decision: PHN-479
    func testTheRelayIsTestedSavedAndSwitchedThroughTheHelper() async throws {
        let page = model()
        daemon.replies["phone.list"] = list([])
        link.replies["phone.link_status"] = ["relay": ["state": "shared", "connected": true], "own_relay": NSNull(), "devices": []]
        await page.load()
        XCTAssertEqual(page.link?.line, PhoneWords.statusShared)
        XCTAssertEqual(page.link?.alwaysUseRelay, false)          // today's helper says it in its limits
        page.relayLink = "ferbrelay://relay.example.com:7443?tok=abc\n&ca=xyz"
        link.replies["phone.relay_test"] = ["url": "https://relay.example.com:7443", "ms": 40,
                                          "steps": ["reached the relay"], "warnings": ["discovery is off"]]
        await page.testRelay()
        XCTAssertEqual(link.params(of: "phone.relay_test").last?["url"] as? String, "ferbrelay://relay.example.com:7443?tok=abc&ca=xyz")
        XCTAssertEqual(page.relayResult, .reachable(["reached the relay", "Note: discovery is off"]))
        link.replies["phone.relay_set"] = ["saved": true, "test": ["steps": ["reached the relay"], "warnings": []]]
        link.replies["phone.link_status"] = ["relay": ["state": "own"], "own_relay": ["url": "https://relay.example.com:7443"],
                                      "own_relay_always": true, "devices": []]
        await page.useRelay()
        XCTAssertEqual(page.relayResult, .saved(["reached the relay"]))
        XCTAssertEqual(page.relayLink, "")                          // the typed secret is forgotten once saved
        XCTAssertEqual(page.link?.line, PhoneWords.statusOwnAlways)
        await page.setAlwaysUseRelay(false)
        XCTAssertEqual(link.params(of: "phone.relay_always").last?["on"] as? Bool, false)
        await page.stopUsingRelay()
        XCTAssertEqual(link.params(of: "phone.relay_set").last?["off"] as? Bool, true)
        // an https address takes its token beside it
        page.relayAddress = " https://relay.example.com "
        page.relayToken = " t0k "
        await page.testRelay()
        XCTAssertEqual(link.params(of: "phone.relay_test").last?["url"] as? String, "https://relay.example.com")
        XCTAssertEqual(link.params(of: "phone.relay_test").last?["token"] as? String, "t0k")
        XCTAssertTrue(daemon.calls.allSatisfy { !$0.0.hasPrefix("link.") })   // the relay is the helper's alone
    }

    /// Rule 7: the token reaches the helper and nothing else: no daemon call, no word on the page, no description, no
    /// preference (the page's sources write none).
    // decision: PHN-481
    func testTheRelayTokenNeverLeavesForAnythingButTheHelper() async throws {
        let marker = "MARKER-relay-token-7Q2"
        let page = model()
        daemon.replies["phone.list"] = list([])
        link.replies["phone.link_status"] = ["relay": ["state": "shared", "connected": true], "own_relay_always": false]
        await page.load()
        page.relayAddress = "https://relay.example.com"
        page.relayToken = marker
        link.failures["phone.relay_test"] = DaemonRPCError.remote("own_relay_access_denied: the relay refused \(marker) (Check the token)")
        await page.testRelay()
        guard case let .unreachable(words) = page.relayResult else { return XCTFail("\(String(describing: page.relayResult))") }
        XCTAssertFalse(words.contains(marker))
        XCTAssertEqual(words, "The relay refused •••. Check the token.")
        page.relayAddress = ""; page.relayToken = ""
        page.relayLink = "ferbrelay://relay.example.com?tok=\(marker)"
        link.failures["phone.relay_set"] = DaemonRPCError.remote("own_relay_bad_url: cannot read ferbrelay://relay.example.com?tok=\(marker)")
        await page.useRelay()
        guard case let .unreachable(again) = page.relayResult else { return XCTFail() }
        XCTAssertFalse(again.contains(marker))
        XCTAssertFalse(String(describing: page.relayEntry).contains(marker))
        XCTAssertFalse(String(reflecting: page.relayEntry).contains(marker))
        for text in [page.problem, page.note, page.linkProblem] { XCTAssertFalse((text ?? "").contains(marker)) }
        XCTAssertFalse(daemon.calls.contains { "\($0.1)".contains(marker) })
        XCTAssertEqual(Set(link.calls.filter { "\($0.1)".contains(marker) }.map(\.0)), ["phone.relay_test", "phone.relay_set"])
        XCTAssertFalse(SourceSettingsKeys.found.values.contains { ["PhoneModel.swift", "PhonePage.swift"].contains($0) })
    }

    // decision: PHN-418
    func testEveryRelayStateHasItsOneLine() {
        func line(_ relay: [String: Any], always: Bool = false) -> String {
            LinkStatus(["relay": relay, "own_relay_always": always]).line
        }
        XCTAssertEqual(line(["state": "shared", "connected": false]), PhoneWords.statusNoRelayYet)
        XCTAssertEqual(line(["state": "own"]), PhoneWords.statusOwn)
        XCTAssertEqual(line(["state": "own"], always: true), PhoneWords.statusOwnAlways)
        XCTAssertEqual(line(["state": "own_fallback", "error": ["code": "own_relay_unreachable", "message": "no answer"]]),
                       PhoneWords.statusOwnFallback + " No answer.")
        XCTAssertEqual(line(["state": "own_down"]), PhoneWords.statusOwnDown)
        XCTAssertEqual(PhoneWords.linkMessage("device_limit: FERB already has 5 paired phones. (Remove one first)", allow: nil),
                       "FERB already has 5 paired phones. Remove one first.")
        XCTAssertEqual(PhoneWords.linkMessage("Your relay: down", allow: nil), "Your relay: down.")
    }

    // decision: PHN-479
    func testWithoutTheLinkOrTheServiceThePageSaysSoHonestly() async throws {
        let page = model(link: false)
        daemon.replies["phone.list"] = list([])
        await page.load()
        XCTAssertEqual(page.linkProblem, PhoneWords.noLinkInThisVersion)
        XCTAssertNil(page.link)
        await page.startPairing()
        XCTAssertEqual(page.pairing, .ended(PhoneWords.noLinkInThisVersion))
        XCTAssertTrue(page.phones.isEmpty)
        XCTAssertNil(page.problem)

        let off = model()
        daemon.failures["phone.list"] = DaemonRPCError.unavailable
        link.failures["phone.link_status"] = DaemonRPCError.unavailable
        await off.load()
        XCTAssertEqual(off.problem, DaemonRPCError.unavailable.localizedDescription)
        XCTAssertEqual(off.linkProblem, PhoneWords.linkNotAnswering)
        daemon.failures["phone.list"] = DaemonRPCError.remote("no such method", code: -32601)
        await off.load()
        XCTAssertEqual(off.problem, PhoneWords.serviceTooOld)
        link.replies["phone.pair_start"] = [:]
        link.failures["phone.pair_start"] = DaemonRPCError.remote("relay_unreachable: This Mac is not connected to a relay yet, so a phone could not find it. (Check the Mac's internet connection and try again)")
        daemon.failures["phone.list"] = nil
        await off.startPairing()
        XCTAssertEqual(off.pairing, .ended("This Mac is not connected to a relay yet, so a phone could not find it. Check the Mac's internet connection and try again."))
    }

    func testActivitySaysWhatWhenAndWhichPhoneNeverAValue() async throws {
        let page = model()
        daemon.replies["phone.list"] = list([phone("ep1", "Pixel 8")])
        daemon.replies["phone.audit"] = ["rows": [
            ["at": "2026-11-16T09:00:00+05:30", "phone": "ep1", "method": "chat.send", "class": "plain", "outcome": "ok",
             "detail": ["text": "MARKER-value"]],
            ["at": "2026-11-16T09:01:00+05:30", "phone": "ep1", "method": "settings.set", "class": "sensitive",
             "outcome": "bad_proof", "detail": ["value": "MARKER-value"]],
            ["at": "2026-11-16T09:02:00+05:30", "phone": "", "method": "phone.pair_decide", "class": "owner", "outcome": "denied"],
            ["at": "2026-11-16T09:03:00+05:30", "phone": "gone", "method": "task.stop", "class": "plain", "outcome": "ok"]]]
        await page.load()
        let lines = page.activity.map { $0.line(names: page.names) }
        XCTAssertEqual(lines, ["A removed phone stopped a task", "This Mac said no to a phone that asked to pair",
                               "Pixel 8 changed a setting (not allowed)", "Pixel 8 sent a message"])  // newest first
        XCTAssertFalse(lines.joined().contains("MARKER"))
        XCTAssertNotNil(page.activity.first?.at)
    }

    /// A pairing request reaches the owner wherever he is: the notch opens with the phone's name and code and a button
    /// to Settings → Phone; nothing answers it there.
    // decision: PHN-480
    func testAPairingRequestOpensTheNotchWithAWayToSettingsPhone() async throws {
        let model = NotchModel(preferences: scratchDefaults())
        let posted = expectation(forNotification: .ferbPhoneEvent, object: nil) { note in
            note.userInfo?["topic"] as? String == "phone.pair_requested"
        }
        model.receiveBusEvent(["topic": "phone.pair_requested", "data": asking, "at": "2026-11-16T09:00:00+05:30"])
        await fulfillment(of: [posted], timeout: 1)
        let notice = try XCTUnwrap(model.conversation.notices.last)
        XCTAssertEqual(notice.text, PhoneWords.notchAsks("Pixel 8", code: "482 913"))
        XCTAssertEqual(notice.cards?.first?.page, "phone")
        XCTAssertTrue(model.announcedAttentionIDs.contains("phone-pair:r1"))
        model.receiveBusEvent(["topic": "phone.pair_requested", "data": asking])     // once per request
        XCTAssertEqual(model.conversation.notices.filter { $0.id == "phone-pair-r1" }.count, 1)
    }

    // decision: PHN-580
    // decision: PHN-581
    // decision: APP-944
    func testThePageSaysWhereFilesFromThePhoneLandAndChangesItThroughTheDaemon() async throws {
        var listed = list([phone("ep1", "Pixel 8")])
        listed["upload_folder"] = ["path": "/Users/o/Downloads/FERB from phone", "shown": "Downloads › FERB from phone",
                                   "default": true]
        daemon.replies["phone.list"] = listed
        daemon.replies["phone.upload_folder"] = ["upload_folder": ["path": "/Users/o/Inbox", "shown": "Inbox", "default": false]]
        let page = model()
        await page.load()
        XCTAssertEqual(page.uploadFolder?.shown, "Downloads › FERB from phone")
        XCTAssertEqual(PhoneWords.filesLandIn("Downloads › FERB from phone"),
                       "Files you send from your phone are saved in Downloads › FERB from phone on this Mac.")
        await page.chooseUploadFolder("/Users/o/Inbox")
        XCTAssertEqual(page.uploadFolder, UploadFolder(["path": "/Users/o/Inbox", "shown": "Inbox", "default": false]))
        await page.resetUploadFolder()
        let sent = daemon.calls.filter { $0.0 == "phone.upload_folder" }.map { $0.1 }
        XCTAssertEqual(sent.first?["path"] as? String, "/Users/o/Inbox")
        XCTAssertEqual(sent.last?["reset"] as? Bool, true)
        XCTAssertTrue(link.methods().allSatisfy { $0 != "phone.upload_folder" }, "the daemon's, never the helper's")
    }

    // decision: GATE-615
    func testUntilImBackShowsWhileItRunsAndAskAgainEndsIt() async throws {
        daemon.replies["phone.list"] = list([])
        daemon.replies["screen_use.status"] = ["active": true, "phone_name": "Pixel 8", "since": "2026-11-17T10:00:00+05:30",
                                               "can_offer": true]
        let page = model()
        await page.load()
        XCTAssertEqual(page.screenUse?.active, true)
        XCTAssertEqual(page.screenUse?.phoneName, "Pixel 8")
        daemon.replies["screen_use.status"] = ["active": false, "can_offer": true]
        await page.endScreenUse()
        XCTAssertEqual(page.screenUse?.active, false)
        XCTAssertTrue(daemon.calls.contains { $0.0 == "screen_use.end" })
        for line in [PhoneWords.screenUntilBack("Pixel 8"), PhoneWords.screenUntilBackEnds,
                     PhoneWords.phonesToCome(1, until: nil), PhoneWords.phonesLeftBehind(2)] {
            XCTAssertFalse(line.contains("AI"), line)
        }
    }

    // decision: GATE-618
    // decision: PHN-593
    func testAreYouBackTurnsIntoTheWelcomeInPlaceAndASilentHoldLeavesNothing() {
        let welcome = "Welcome back. FERB will ask before it uses your screen again: a task that needs it waits for your Yes."
        let checking: [String: Any] = ["state": "checking", "title": "Are you back?", "since": "S", "hold": 1,
                                       "line": "FERB stopped using your screen for a moment. If you're here, just move the pointer or press a key."]
        var chat = ConversationArchive()
        XCTAssertTrue(ScreenBackLine.apply(checking, to: &chat))
        XCTAssertEqual(chat.notices.count, 1)
        XCTAssertTrue(chat.notices[0].text.hasPrefix("Are you back? FERB stopped using your screen"))
        // input during the hold: the same line now welcomes him (no second line, no vanishing)
        XCTAssertTrue(ScreenBackLine.apply(["state": "ended", "why": "owner_input", "since": "S", "hold": 1,
                                            "during_hold": true, "line": welcome], to: &chat))
        XCTAssertEqual(chat.notices.map(\.text), [welcome])
        // a clear return with no question: the daemon's own notice says it, this adds nothing
        var clear = ConversationArchive()
        XCTAssertFalse(ScreenBackLine.apply(["state": "ended", "why": "owner_input", "since": "S", "hold": 0,
                                             "during_hold": false, "line": welcome], to: &clear))
        XCTAssertTrue(clear.notices.isEmpty)
        // a hold that passes in silence: the line goes and nothing follows
        var quiet = ConversationArchive()
        XCTAssertTrue(ScreenBackLine.apply(checking, to: &quiet))
        XCTAssertTrue(ScreenBackLine.apply(["state": "resumed", "since": "S", "hold": 1], to: &quiet))
        XCTAssertTrue(quiet.notices.isEmpty)
        for line in [welcome, checking["line"] as! String] { XCTAssertFalse(line.contains("AI"), line) }
    }

    // decision: PHN-591
    func testPhonesStillToComeAfterARelayChangeAreSaidAndWhenTheyAreLeftBehind() {
        let waiting = LinkStatus(["relay": ["state": "own"], "relay_handover": ["phones_waiting": 1, "on_shared_relays": true,
                                                                                 "closes_at": 1_800_000_000, "closed": false]])
        XCTAssertEqual(waiting.handover?.phonesWaiting, 1)
        XCTAssertEqual(waiting.handover?.closesAt, Date(timeIntervalSince1970: 1_800_000_000))
        XCTAssertEqual(waiting.handover?.closed, false)
        let behind = LinkStatus(["relay": ["state": "own"], "relay_handover": ["phones_waiting": 1, "closed": true]])
        XCTAssertEqual(behind.handover?.closed, true)
        XCTAssertNil(LinkStatus(["relay": ["state": "shared"]]).handover)
    }

#if FERB_PLAYGROUND
    // decision: APP-619
    func testThePlaygroundAnswersThePhonePageWithInventedPhones() async throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-review-\(UUID())")
        addTeardownBlock { try? FileManager.default.removeItem(at: home) }
        let services = PlaygroundServices.make(home: home, defaults: scratchDefaults(), fresh: true)
        let page = PhoneSettingsModel(service: RPCPhoneService(daemon: services.daemon, link: services.daemon))
        await page.load()
        XCTAssertEqual(page.phones.count, 2)
        XCTAssertTrue(page.phones.contains { $0.unusedWarning })
        XCTAssertNotNil(page.link)
        await page.startPairing()
        guard case let .showing(invite, _) = page.pairing else { return XCTFail("\(page.pairing)") }
        XCTAssertFalse(invite.rows.isEmpty)
        await page.cancelPairing()
        XCTAssertEqual(page.pairing, .idle)
        await page.remove(page.phones[1].id)
        XCTAssertEqual(page.phones.count, 1)
    }
#endif
}
