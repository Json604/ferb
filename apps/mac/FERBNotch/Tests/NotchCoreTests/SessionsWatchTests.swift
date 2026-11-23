import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// HAND-710: a watched session (any terminal, an agent app's window) has no conversation: its screen is its window,
/// live, the box types into it, and a send is done at the Mac's answer.
@MainActor
final class SessionsWatchTests: XCTestCase {
    typealias HoldingRPC = SessionsSendingTests.HoldingRPC

    static let terminal: [String: Any] = [
        "id": "term:110", "agent": "terminal", "program": "agy", "cwd": "/Users/o/Desktop", "title": "",
        "window": "Desktop — agy — 120×30", "status": "idle", "waiting": "none", "watch": true,
        "host": ["bundle_id": "com.apple.Terminal", "name": "Terminal", "pid": 9, "via": "direct", "tty": "/dev/ttys002"],
        "send_paths": ["terminal_script"]]
    static let cursor: [String: Any] = [
        "id": "app:cursor:1", "agent": "app_chat", "cwd": "", "program": "", "title": "site — studio.css",
        "window": "site — studio.css", "status": "idle", "waiting": "none", "watch": true,
        "host": ["bundle_id": "com.todesktop.230313mzl4w4u92", "name": "Cursor", "pid": 7, "via": "direct", "tty": ""],
        "send_paths": ["window"]]

    private func opened(_ rpc: HoldingRPC, _ row: [String: Any] = terminal) async throws -> SessionsModel {
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [row]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        await model.open(try XCTUnwrap(model.sessions.first))
        return model
    }

    // decision: HAND-710
    func testWatchIsReadAndAbsentIsFalse() throws {
        XCTAssertTrue(try XCTUnwrap(AgentSessionRow(Self.terminal)).watch)
        XCTAssertFalse(try XCTUnwrap(AgentSessionRow(SessionsTests.claude)).watch)
        XCTAssertFalse(try XCTUnwrap(AgentSessionRow(["id": "x", "watch": false])).watch)
    }

    // decision: HAND-710
    func testAnAppsWindowIsNamedByItsTitleAndItsAppAlone() throws {
        let row = try XCTUnwrap(AgentSessionRow(Self.cursor))
        XCTAssertEqual(row.heading, "site — studio.css")
        XCTAssertEqual(row.whereLine, "Cursor")
        XCTAssertEqual(row.detailLine, "Cursor")
        // no title: the app's name
        var untitled = Self.cursor
        untitled["title"] = ""
        untitled["window"] = ""
        XCTAssertEqual(try XCTUnwrap(AgentSessionRow(untitled)).heading, "Cursor")
        // a terminal keeps "<program> · <folder> · in <app>"
        XCTAssertEqual(try XCTUnwrap(AgentSessionRow(Self.terminal)).whereLine, "agy · Desktop · in Terminal")
        XCTAssertEqual(try XCTUnwrap(AgentSessionRow(SessionsTests.claude)).detailLine, "Claude Code · site · in Terminal")
    }

    // decision: HAND-710
    func testAWatchedSessionOffersNoHandOverAndItsBoxSaysTypeHere() throws {
        let row = try XCTUnwrap(AgentSessionRow(Self.terminal))
        XCTAssertFalse(row.offersHandOver)
        XCTAssertEqual(row.composerPlaceholder, "Type here")
        XCTAssertEqual(AgentSessionRow.typeNote, "What you send is typed into this window on your Mac, then Return.")
        let claude = try XCTUnwrap(AgentSessionRow(SessionsTests.claude))
        XCTAssertTrue(claude.offersHandOver)
        XCTAssertEqual(claude.composerPlaceholder, "Message Claude Code…")
    }

    // decision: HAND-710
    func testAWatchedSessionsConversationIsNeverAsked() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc)
        XCTAssertTrue(model.watching)
        XCTAssertFalse(model.opening, "no first read to wait for: no skeleton")
        await model.poll()
        await model.loadTimeline()
        XCTAssertTrue(rpc.scripted.params(of: "sessions.timeline").isEmpty)
        XCTAssertTrue(model.items.isEmpty)
    }

    // decision: HAND-710
    // decision: PHN-812
    func testASendToAWatchedSessionEndsAtTheMacsAnswer() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, Self.cursor)
        rpc.held = ["sessions.send"]
        rpc.scripted.replies["sessions.send"] = ["sent": true, "delivered": true, "path": "window"]
        model.draft = "add a test for the header"
        let sending = Task { await model.send() }
        try await rpc.waitForHeldCall()
        XCTAssertEqual(model.outgoing?.state, .sending)
        XCTAssertTrue(model.busy)
        rpc.release()
        await sending.value
        // typed: nothing to wait for, no bubble left over, the box free again
        XCTAssertNil(model.outgoing)
        XCTAssertFalse(model.busy)
        XCTAssertNil(model.note)
        XCTAssertEqual(rpc.scripted.params(of: "sessions.send").last?["text"] as? String, "add a test for the header")
        XCTAssertTrue(rpc.scripted.params(of: "sessions.timeline").isEmpty)
    }

    // decision: HAND-710
    // decision: GATE-622
    func testTheTakeControlYesFirstThenDoneAtTheAnswer() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc)
        rpc.scripted.replies["sessions.send"] = ["needs_yes": ["question": "Type this into agy in Terminal?",
                                                               "detail": "ls", "covers": ""]]
        rpc.scripted.replies["sessions.control"] = ["sent": true, "delivered": true, "path": "terminal_script",
                                                    "control": "mac"]
        model.draft = "ls"
        await model.send()
        XCTAssertEqual(model.confirm?.yes.detail, "ls")
        XCTAssertEqual(model.outgoing?.state, .sending)
        await model.confirmYes()
        XCTAssertEqual(rpc.scripted.params(of: "sessions.control").last?["text"] as? String, "ls")
        XCTAssertNil(model.outgoing)
    }

    // decision: HAND-710
    // decision: PHN-812
    func testARefusedSendToAWatchedSessionIsNotSentWithTheMacsWords() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc)
        let why = "FERB couldn't find this window on your Mac."
        rpc.scripted.failures["sessions.send"] = DaemonRPCError.remote(why, code: SessionRefusal.sendFailed)
        model.draft = "pwd"
        await model.send()
        XCTAssertEqual(model.outgoing?.state, .notSent(why))
        XCTAssertEqual(model.outgoing?.text, "pwd")
        // Try again, now typed: gone
        rpc.scripted.failures["sessions.send"] = nil
        rpc.scripted.replies["sessions.send"] = ["sent": true, "delivered": true, "path": "terminal_script"]
        await model.tryAgain()
        XCTAssertNil(model.outgoing)
    }

    // decision: HAND-710
    func testAListWithTheOldNoChatsFieldStillReads() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [Self.cursor], "no_chats": ["Zed"]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        XCTAssertEqual(model.sessions.map(\.id), ["app:cursor:1"])
    }

    private func lays(_ view: some View, width: CGFloat = 700, height: CGFloat = 600) {
        let hosting = NSHostingView(rootView: view.frame(width: width, height: height))
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: height)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
        XCTAssertGreaterThan(hosting.fittingSize.height, 0)
    }

    // decision: HAND-710
    func testTheWatchPaneLaysOutSendingAndNotSent() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, Self.cursor)
        let row = try XCTUnwrap(model.openSession)
        lays(SessionDetailView(sessions: model, row: row, sheet: .constant(nil), previewAccess: PreviewAccess()))
        rpc.scripted.failures["sessions.send"] = DaemonRPCError.remote("The window closed.", code: SessionRefusal.sendFailed)
        model.draft = "hello"
        await model.send()
        lays(SessionDetailView(sessions: model, row: row, sheet: .constant(nil), previewAccess: PreviewAccess()))
        lays(SessionRowView(row: row), height: 90)
    }
}
