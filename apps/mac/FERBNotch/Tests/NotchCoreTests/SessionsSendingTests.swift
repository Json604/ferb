import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// A send to a coding session shows at once (PHN-812), and the conversation keeps the chat's list rules (PHN-813).
@MainActor
final class SessionsSendingTests: XCTestCase {
    /// A daemon whose calls named in `held` wait until `release()`: what the page shows meanwhile can be read.
    final class HoldingRPC: DaemonRPCTransport, @unchecked Sendable {
        let scripted = ScriptedRPC()
        var held: Set<String> = []
        private var waiting: [CheckedContinuation<Void, Never>] = []
        private let lock = NSLock()

        func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
            if held.contains(method) {
                await withCheckedContinuation { c in
                    lock.lock()
                    waiting.append(c)
                    lock.unlock()
                }
            }
            return try await scripted.call(method, params: params)
        }

        func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
            scripted.stream(method, params: params)
        }

        var arrived: Int {
            lock.lock()
            defer { lock.unlock() }
            return waiting.count
        }

        func release() {
            lock.lock()
            let all = waiting
            waiting = []
            lock.unlock()
            all.forEach { $0.resume() }
        }

        func waitForHeldCall() async throws {
            for _ in 0..<400 where arrived == 0 { try await Task.sleep(for: .milliseconds(5)) }
            XCTAssertEqual(arrived, 1, "the held call arrived")
        }
    }

    private func opened(_ rpc: HoldingRPC) async throws -> SessionsModel {
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude]]
        rpc.scripted.replies["sessions.timeline"] = ["session": SessionsTests.claude, "more": false, "cards": [] as [Any],
                                                     "items": [["seq": 1, "kind": "user", "text": "hi"]]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        await model.open(try XCTUnwrap(model.sessions.first))
        return model
    }

    // decision: PHN-812
    // decision: APP-1120
    func testHisWordsShowAtOnceAsSendingThenSentAndNeverTwice() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc)
        rpc.held = ["sessions.send"]
        rpc.scripted.replies["sessions.send"] = ["sent": true, "path": "terminal_script", "delivered": true, "seconds": 3]
        // the poll right after the send: the record doesn't have his words yet
        rpc.scripted.replies["sessions.timeline"] = ["items": [] as [Any], "more": false, "cards": [] as [Any]]
        model.draft = "run the tests\n"
        let sending = Task { await model.send() }
        try await rpc.waitForHeldCall()
        // while FERB types into the terminal: the box is empty, his words show as sending, Send shows it is busy
        XCTAssertEqual(model.draft, "")
        XCTAssertEqual(model.outgoing?.text, "run the tests")
        XCTAssertEqual(model.outgoing?.state, .sending)
        XCTAssertTrue(model.busy)
        rpc.release()
        await sending.value
        XCTAssertEqual(model.outgoing?.state, .sent)
        XCTAssertFalse(model.busy)
        // the session's record has them: the sent bubble goes, so they show once
        rpc.scripted.replies["sessions.timeline"] = ["items": [["seq": 2, "kind": "user", "text": "run the tests"]],
                                                     "more": false, "cards": [] as [Any]]
        await model.poll()
        XCTAssertNil(model.outgoing)
        XCTAssertEqual(model.items.filter { $0.kind == "user" && $0.text == "run the tests" }.count, 1)
        let rows = SessionTranscript.rows(items: model.items, cards: model.cards, outgoing: model.outgoing, loadingEarlier: false)
        XCTAssertFalse(rows.contains { $0.key == "session-outgoing" })
    }

    // decision: PHN-812
    func testARefusedSendKeepsHisWordsAsNotSentAndTryAgainSendsThem() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc)
        let why = "FERB couldn't find this session's window on your Mac."
        rpc.scripted.failures["sessions.send"] = DaemonRPCError.remote(why, code: -32045)
        model.draft = "go on"
        await model.send()
        XCTAssertEqual(model.outgoing, SessionsModel.Outgoing(session: "claude:abc", text: "go on", state: .notSent(why), after: 1))
        XCTAssertEqual(model.draft, "", "the words are in the not-sent bubble, not lost")
        XCTAssertNil(model.problem, "said once, under his words")
        // Try again: the same words
        rpc.scripted.failures["sessions.send"] = nil
        rpc.scripted.replies["sessions.send"] = ["sent": true, "delivered": true]
        rpc.held = ["sessions.send"]
        let again = Task { await model.tryAgain() }
        try await rpc.waitForHeldCall()
        XCTAssertEqual(model.outgoing?.state, .sending)
        rpc.release()
        await again.value
        XCTAssertEqual(rpc.scripted.params(of: "sessions.send").last?["text"] as? String, "go on")
        XCTAssertEqual(model.outgoing?.state, .sent)
    }

    // decision: PHN-812
    // decision: GATE-622
    func testTheMacsYesKeepsThemSendingUntilTheTerminalHasThem() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc)
        rpc.scripted.replies["sessions.send"] = ["needs_yes": ["question": "Send this to Claude Code in Terminal?",
                                                               "detail": "lint it", "covers": ""]]
        model.draft = "lint it"
        await model.send()
        XCTAssertNotNil(model.confirm)
        XCTAssertEqual(model.outgoing?.state, .sending, "behind the question, his words wait as sending")
        rpc.scripted.failures["sessions.control"] = DaemonRPCError.remote("The terminal closed.", code: -32045)
        await model.confirmYes()
        XCTAssertEqual(model.outgoing?.state, .notSent("The terminal closed."))
        XCTAssertEqual(model.outgoing?.text, "lint it")
    }

    // MARK: The list (PHN-813)

    // decision: PHN-813
    // decision: APP-1120
    func testNewItemsFollowOnlyAtTheBottomElseNewMessageShows() {
        let place = SessionTranscriptPlace()
        let t0 = Date(timeIntervalSince1970: 1_000)
        // laid out at the bottom (viewport 400 of 1,000)
        _ = place.scrolled(offset: 600, height: 1000, viewport: 400, more: false, loading: false, at: t0)
        XCTAssertTrue(place.arrived(), "at the bottom: a new item keeps the view on the newest")
        XCTAssertFalse(place.unreadBelow)
        // he scrolls up
        _ = place.scrolled(offset: 200, height: 1000, viewport: 400, more: false, loading: false, at: t0 + 1)
        XCTAssertFalse(place.arrived(), "scrolled up: the view stays where he is")
        XCTAssertTrue(place.unreadBelow, "and New message shows")
        // the item grew the list; he scrolls back down himself: the control goes
        _ = place.scrolled(offset: 200, height: 1100, viewport: 400, more: false, loading: false, at: t0 + 2)
        _ = place.scrolled(offset: 700, height: 1100, viewport: 400, more: false, loading: false, at: t0 + 3)
        XCTAssertFalse(place.unreadBelow)
        XCTAssertTrue(place.arrived(), "back at the bottom it follows again")
        // tapping New message follows from wherever he was
        _ = place.scrolled(offset: 100, height: 1100, viewport: 400, more: false, loading: false, at: t0 + 4)
        XCTAssertFalse(place.arrived())
        place.jumped()
        XCTAssertFalse(place.unreadBelow)
        XCTAssertTrue(place.arrived())
    }

    // decision: PHN-813
    func testEarlierItemsComeAsHeScrollsNearTheTopNeverAsItOpens() {
        let place = SessionTranscriptPlace()
        let t0 = Date(timeIntervalSince1970: 2_000)
        place.movedByFERB(at: t0, animated: false)                     // opened at the newest
        _ = place.scrolled(offset: 2600, height: 3000, viewport: 400, more: true, loading: false, at: t0 + 1)
        XCTAssertFalse(place.scrolled(offset: 2600, height: 3000, viewport: 400, more: true, loading: false, at: t0 + 1.1).loadEarlier)
        // he scrolls to near the top
        XCTAssertTrue(place.scrolled(offset: 150, height: 3000, viewport: 400, more: true, loading: false, at: t0 + 2).loadEarlier)
        XCTAssertFalse(place.scrolled(offset: 120, height: 3000, viewport: 400, more: true, loading: true, at: t0 + 2.1).loadEarlier,
                       "one page at a time")
        XCTAssertFalse(place.scrolled(offset: 100, height: 3000, viewport: 400, more: false, loading: false, at: t0 + 3).loadEarlier,
                       "nothing earlier")
    }

    // decision: PHN-813
    func testEarlierPagesLoadOnceAtATime() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude]]
        rpc.queue("sessions.timeline", [["items": [["seq": 51, "kind": "user", "text": "b"]], "more": true, "cards": [] as [Any]]])
        rpc.replies["sessions.timeline"] = ["items": [["seq": 50, "kind": "assistant", "text": "a"]], "more": false, "cards": [] as [Any]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        await model.open(try XCTUnwrap(model.sessions.first))
        async let first: Void = model.showEarlier()
        async let second: Void = model.showEarlier()
        _ = await (first, second)
        XCTAssertEqual(rpc.params(of: "sessions.timeline").filter { $0["before"] != nil }.count, 1)
        XCTAssertEqual(model.items.map(\.seq), [50, 51])
        XCTAssertFalse(model.loadingEarlier)
    }

    // decision: PHN-813
    func testTheRowsCarryTheLoaderItemsCardsAndHisWordsAndChangeWithTheirState() throws {
        let items = [SessionItem(["seq": 1, "kind": "user", "text": "hi"]), SessionItem(["seq": 2, "kind": "assistant", "text": "yo"])]
            .compactMap { $0 }
        let cards = SessionTimeline(["cards": [SessionsTests.card]]).cards
        let sending = SessionsModel.Outgoing(session: "s", text: "go", state: .sending, after: 2)
        let rows = SessionTranscript.rows(items: items, cards: cards, outgoing: sending, loadingEarlier: true)
        XCTAssertEqual(rows.map(\.key), ["transcript-top", "transcript-earlier", "session-item-1", "session-item-2",
                                         "session-card-card-1", "session-outgoing", "transcript-bottom"])
        var notSent = sending
        notSent.state = .notSent("No.")
        let after = SessionTranscript.rows(items: items, cards: cards, outgoing: notSent, loadingEarlier: false)
        XCTAssertNotEqual(rows.last { $0.key == "session-outgoing" }, after.last { $0.key == "session-outgoing" },
                          "a changed state redraws its row")
    }

    // decision: PHN-812
    // decision: PHN-813
    func testThePageLaysOutWithHisWordsInEachState() async throws {
        let rpc = ScriptedRPC()
        let store = FERBWindowStore(rpc: rpc)
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude]]
        rpc.replies["sessions.timeline"] = ["session": SessionsTests.claude, "more": true, "cards": [SessionsTests.card],
                                            "items": [["seq": 1, "kind": "user", "text": "Add tests"],
                                                      ["seq": 2, "kind": "assistant", "text": "On it."]]]
        await store.sessions.load()
        await store.sessions.open(try XCTUnwrap(store.sessions.sessions.first))
        for state in [SessionsModel.Outgoing.State.sending, .sent, .notSent("The terminal closed.")] {
            let view = SessionOutgoingView(outgoing: .init(session: "claude:abc", text: "run it", state: state, after: 2),
                                           tryAgain: {})
            let hosting = NSHostingView(rootView: view.frame(width: 600))
            hosting.layout()
            XCTAssertGreaterThan(hosting.fittingSize.height, 20)
        }
        let page = NSHostingView(rootView: SessionsPage(store: store, sessions: store.sessions).frame(width: 700, height: 600))
        page.frame = NSRect(x: 0, y: 0, width: 700, height: 600)
        page.layout()
        XCTAssertGreaterThan(page.fittingSize.height, 0)
    }
}

/// A session opening shows its loading look, never an error, until the daemon answers (PHN-816).
@MainActor
final class SessionsOpeningTests: XCTestCase {
    // decision: PHN-816
    // decision: APP-1120
    func testWhileTheFirstReadIsOnItsWayThePageLoadsAndShowsNoError() async throws {
        let rpc = SessionsSendingTests.HoldingRPC()
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude]]
        rpc.scripted.replies["sessions.timeline"] = ["session": SessionsTests.claude, "more": false, "cards": [] as [Any],
                                                     "items": [["seq": 1, "kind": "assistant", "text": "Done."]]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        rpc.held = ["sessions.timeline"]
        let opening = Task { await model.open(try! XCTUnwrap(model.sessions.first)) }
        try await rpc.waitForHeldCall()
        XCTAssertTrue(model.opening, "the skeleton shows")
        // the page's own ticks meanwhile: a list read that fails, and a poll that must not read again
        rpc.scripted.failures["sessions.list"] = DaemonRPCError.remote("FERB can't find this chat.", code: -32040)
        await model.load(quiet: true)
        XCTAssertNotNil(model.problem)
        XCTAssertNil(model.problemShown, "no error while it opens")
        rpc.held = []
        await model.poll()
        XCTAssertEqual(rpc.scripted.params(of: "sessions.timeline").count, 0, "no second read while the first is on its way")
        rpc.scripted.failures["sessions.list"] = nil
        rpc.release()
        await opening.value
        XCTAssertFalse(model.opening)
        XCTAssertEqual(model.items.map(\.seq), [1])
    }

    // decision: PHN-816
    func testTheErrorShowsOnceTheDaemonAnswersNotFound() async throws {
        let rpc = SessionsSendingTests.HoldingRPC()
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude]]
        rpc.scripted.failures["sessions.timeline"] = DaemonRPCError.remote("FERB can't find this chat.", code: -32040)
        let model = SessionsModel(rpc: rpc)
        await model.load()
        rpc.held = ["sessions.timeline"]
        let opening = Task { await model.open(try! XCTUnwrap(model.sessions.first)) }
        try await rpc.waitForHeldCall()
        XCTAssertNil(model.problemShown)
        rpc.release()
        await opening.value
        XCTAssertFalse(model.opening)
        XCTAssertEqual(model.problemShown, "FERB can't find this chat.")
    }

    // decision: PHN-816
    func testTheSkeletonLaysOut() {
        let hosting = NSHostingView(rootView: SessionSkeleton().frame(width: 600, height: 400))
        hosting.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.height, 0)
        XCTAssertEqual(SessionSkeleton.bubbles.filter(\.mine).count, 2)
    }
}
