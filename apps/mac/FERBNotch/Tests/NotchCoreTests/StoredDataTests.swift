import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Settings → Stored data (owner STORE-422; agent STORE-423, STORE-424, STORE-425): the page's models read the
/// daemon's `data.*` replies; the page's model calls them with the right parameters, asks before a delete with what goes
/// and what stays, and drops a deleted turn from the chat this app holds; the playground answers them with invented data.
@MainActor
final class StoredDataTests: XCTestCase {
    private let tablesReply: [String: Any] = [
        "groups": [["id": "conversation", "title": "Conversation"], ["id": "model_input", "title": "What the AI was given"],
                   ["id": "other", "title": "Everything else"]],
        "tables": [
            ["name": "chat_history", "group": "conversation", "rows": 3, "columns": ["id", "body_json"], "internal": false,
             "action": "turn", "note": "A turn's rows can be deleted"],
            ["name": "spans", "group": "model_input", "rows": 9, "columns": ["id", "input_json"], "internal": false,
             "action": NSNull(), "note": ""],
            ["name": "chat_fts_data", "group": "other", "rows": 4, "columns": ["block"], "internal": true,
             "action": NSNull(), "note": ""]]]

    // decision: STORE-423
    func testTheTablesAreGroupedForAPersonWithInternalOnesHidden() throws {
        let tables = try XCTUnwrap(StoredTables(tablesReply))
        XCTAssertEqual(tables.groups.map(\.title), ["Conversation", "What the AI was given", "Everything else"])
        XCTAssertEqual(tables.tables(in: "conversation", internal: false).map(\.name), ["chat_history"])
        XCTAssertEqual(tables.tables(in: "other", internal: false).map(\.name), [])
        XCTAssertEqual(tables.tables(in: "other", internal: true).map(\.name), ["chat_fts_data"])
        XCTAssertEqual(tables.hiddenCount, 1)
        XCTAssertEqual(tables.tables.first?.action, "turn")
        XCTAssertNil(tables.tables[1].action)
    }

    // decision: STORE-423
    func testAPageAndARowReadTheirCellsAndTheCutOnesInPieces() throws {
        var page = try XCTUnwrap(StoredPage([
            "table": "spans", "columns": ["id", "input_json", "cost"], "total": 3, "next": 7,
            "rows": [["rowid": 9, "cells": ["s9", "{\"context\": \"Lay", 0.5], "cut": ["input_json": 40000],
                      "action": ["do": "turn", "id": "r1"]],
                     ["rowid": 8, "cells": ["s8", NSNull(), ["bytes": 12]]]]]))
        XCTAssertEqual(page.rows.map(\.rowid), [9, 8])
        XCTAssertEqual(page.rows[0].cut["input_json"], 40000)
        XCTAssertEqual(page.rows[0].action, StoredAction(["do": "turn", "id": "r1"]))
        XCTAssertEqual(page.rows[1].cells, [.text("s8"), .null, .bytes(12)])
        XCTAssertEqual(page.summary(page.rows[1]), "s8  ·  —  ·  12 bytes")
        page.append(try XCTUnwrap(StoredPage(["table": "spans", "columns": ["id"], "rows": [["rowid": 7, "cells": ["s7"]]]])))
        XCTAssertEqual(page.rows.map(\.rowid), [9, 8, 7])
        XCTAssertEqual(Set(page.rows.map(\.id)).count, 3)
        XCTAssertNil(page.next)

        var row = try XCTUnwrap(StoredFullRow(["table": "spans", "rowid": 9, "columns": ["id", "input_json"],
                                               "cells": ["s9", "abc"], "cut": ["input_json": 6]]))
        XCTAssertEqual(row.shown("input_json")?.left, 3)
        row.add(piece: ["column": "input_json", "offset": 2, "text": "zzz", "end": true])   // not where the text ends
        XCTAssertEqual(row.cells[1], .text("abc"))
        row.add(piece: ["column": "input_json", "offset": 3, "text": "def", "end": true])
        XCTAssertEqual(row.cells[1], .text("abcdef"))
        XCTAssertNil(row.cut["input_json"])
        XCTAssertEqual(row.shown("input_json")?.left, 0)
    }

    // decision: STORE-423
    func testWhatTheAIWasGivenReadsAsWritten() {
        let stored = "{\"role\": \"supervisor\", \"context\": \"Layer 1\\nthe owner: hi\\nFERB: hello\"}"
        let shown = StoredData.readable(stored)
        XCTAssertTrue(shown.contains("── context ──\nLayer 1\nthe owner: hi\nFERB: hello"), shown)
        XCTAssertTrue(shown.contains("\"role\" : \"supervisor\""), shown)
        XCTAssertEqual(StoredData.readable("plain words"), "plain words")
        XCTAssertEqual(StoredData.readable("{\"cut"), "{\"cut")            // a cut cell is shown as it is
    }

    // decision: STORE-424
    // decision: STORE-426
    func testADeleteSaysWhatGoesAndWhatStays() throws {
        let preview = try XCTUnwrap(StoredDeletion([
            "confirmed": false, "request_id": "r1",
            "will_delete": ["tasks": 1, "conversation_rows": 2, "chat_entries": 2, "chat_history": 1, "traces": 4,
                            "summaries": 1, "screenshots": 3, "episodes": 1,
                            "memories": [["id": "m1", "text": "The owner's code word is teal-giraffe"]],
                            "learned": [["id": "n1", "text": "Checks the dashboard on Mondays"]]],
            "keeps": ["audit_log": 3, "confirmations": 1, "action_ledger": 2, "memories_shared": 1],
            "keeps_memories": [["id": "m2", "text": "Prefers short answers"]], "keeps_learned": []]))
        XCTAssertFalse(preview.confirmed)
        XCTAssertEqual(preview.removes.first, "the message and FERB's replies (5 records)")
        XCTAssertTrue(preview.removes.contains("1 task with its results"))
        XCTAssertTrue(preview.removes.contains("3 screenshots"))
        XCTAssertTrue(preview.removes.contains("1 memory FERB learned from it"))
        XCTAssertEqual(preview.forgets, ["The owner's code word is teal-giraffe", "Checks the dashboard on Mondays"])
        XCTAssertEqual(preview.keepsLearned, ["Prefers short answers"])
        XCTAssertTrue(preview.keeps.contains { $0.hasPrefix("3 activity-log record(s)") })
        XCTAssertTrue(preview.keeps.contains { $0.hasPrefix("3 safety record(s)") })
        XCTAssertTrue(preview.keeps.contains("1 memory FERB also learned elsewhere"))
        XCTAssertTrue(preview.question.contains("Forgotten with it:\n• The owner's code word is teal-giraffe"))
        XCTAssertTrue(preview.question.contains("FERB learned them elsewhere too:\n• Prefers short answers"))
        let done = try XCTUnwrap(StoredDeletion(["confirmed": true, "request_id": "r1",
                                                 "deleted": ["memories": 1, "screenshots": 2, "screenshots_kept": 1]]))
        XCTAssertTrue(done.removes.contains("1 memory FERB learned from it"))
        XCTAssertTrue(done.keeps.contains { $0.hasPrefix("1 screenshot(s) FERB couldn't remove safely") })
    }

    // decision: STORE-424
    func testADeletedTurnLeavesTheChatThisAppHolds() {
        var archive = ConversationArchive()
        var kept = ConversationTurn(id: UUID(), createdAt: Date(), request: "keep me", sources: [])
        kept.requestID = "r-keep"
        var gone = ConversationTurn(id: UUID(), createdAt: Date(), request: "delete me", sources: [])
        gone.requestID = "r-gone"
        archive.turns = [kept, gone]
        archive.replies = [ConversationReply(id: UUID(), turnID: gone.id, createdAt: Date(), afterTurnID: gone.id, part: .answer),
                           ConversationReply(id: UUID(), turnID: kept.id, createdAt: Date(), afterTurnID: kept.id, part: .answer)]
        XCTAssertEqual(archive.removeTurns(requestIDs: ["r-gone"]), 2)
        XCTAssertEqual(archive.turns.map(\.request), ["keep me"])
        XCTAssertEqual(archive.replies.map(\.turnID), [kept.id])
        XCTAssertEqual(archive.removeTurns(requestIDs: ["r-none"]), 0)
    }

    // decision: STORE-424
    func testThePageAsksBeforeDeletingAndThenDropsTheTurnFromTheChat() async {
        let rpc = ScriptedRPC()
        rpc.queue("data.delete", [["confirmed": false, "request_id": "r1", "will_delete": ["conversation_rows": 2],
                                   "keeps": ["audit_log": 1]],
                                  ["confirmed": true, "request_id": "r1", "deleted": ["conversation_rows": 2],
                                   "keeps": ["audit_log": 1]]])
        rpc.replies["data.rows"] = ["table": "journal", "columns": ["id"], "rows": [], "total": 0]
        let model = StoredDataModel(rpc: rpc)
        var dropped: [String] = []
        model.turnDeleted = { dropped.append($0) }
        await model.open("spans", modelCalls: true)
        let filter = rpc.params(of: "data.rows").first?["filter"] as? [String: String]
        XCTAssertEqual(filter, ["column": "kind", "starts_with": "model."])
        await model.prepareDelete(StoredAction(["do": "turn", "id": "r1"])!)
        XCTAssertEqual(model.asking?.requestID, "r1")
        XCTAssertNil(rpc.params(of: "data.delete").first?["confirm"], "the preview changes nothing")
        XCTAssertTrue(dropped.isEmpty)
        await model.confirmDelete()
        XCTAssertEqual(rpc.params(of: "data.delete").last?["confirm"] as? Bool, true)
        XCTAssertEqual(dropped, ["r1"])
        XCTAssertNil(model.asking)
        // a memory goes through the existing forget
        await model.prepareDelete(StoredAction(["do": "forget", "id": "m1"])!)
        XCTAssertEqual(model.forgetting, "m1")
        await model.confirmForget()
        XCTAssertEqual(rpc.params(of: "memory.forget").first?["id"] as? String, "m1")
        XCTAssertEqual(rpc.params(of: "memory.forget").first?["confirm"] as? Bool, true)
    }

    // decision: STORE-425, STORE-426, STORE-427
    func testAFreshConversationUsesTheNewConversationFromThenOn() async {
        let rpc = ScriptedRPC()
        rpc.replies["chat.attach"] = ["session_id": "chat-fresh", "capabilities": ["input_paths", "entries"]]
        let defaults = scratchDefaults()
        let model = NotchModel(rpc: rpc, preferences: defaults)
        model.conversationID = "s1"
        let problem = await model.startFreshConversation()
        XCTAssertNil(problem)
        XCTAssertEqual(rpc.params(of: "chat.attach").first?["new"] as? Bool, true)
        XCTAssertEqual(model.conversationID, "chat-fresh")
        XCTAssertEqual(defaults.string(forKey: "FERBConversationID"), "chat-fresh")
        // STORE-427: the chat shows where FERB stopped reading, once
        let marker = model.conversation.notices.filter { $0.text == StoredData.freshMarker }
        XCTAssertEqual(marker.map(\.id), ["conversation:chat-fresh"])
        XCTAssertEqual(StoredData.freshMarker, "New conversation: FERB doesn't read the messages above.")
        XCTAssertNil(marker.first?.tag)
    }

    // decision: STORE-426, STORE-427
    func testTheEntryWatchFollowsTheFreshConversationWithoutAReconnect() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["chat.attach"] = ["session_id": "chat-fresh"]
        rpc.heldStreams = ["chat.entries.watch"]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.conversationID = "s1"
        model.entryWatch.supported = true
        model.startEntryWatch()
        for _ in 0..<200 where rpc.params(of: "chat.entries.watch").isEmpty { try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(rpc.params(of: "chat.entries.watch").last?["conversation"] as? String, "s1")
        _ = await model.startFreshConversation()
        for _ in 0..<200 where rpc.params(of: "chat.entries.watch").count < 2 { try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(rpc.params(of: "chat.entries.watch").last?["conversation"] as? String, "chat-fresh")
        model.stopEntryWatch()
    }

    #if FERB_PLAYGROUND
    // decision: STORE-423
    func testThePlaygroundAnswersTheViewerWithInventedData() async throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-review-\(UUID())")
        addTeardownBlock { try? FileManager.default.removeItem(at: home) }
        let services = PlaygroundServices.make(home: home, defaults: scratchDefaults(), fresh: true)
        let model = StoredDataModel(rpc: services.daemon)
        await model.loadTables()
        let tables = try XCTUnwrap(model.tables)
        XCTAssertFalse(tables.tables(in: "conversation", internal: false).isEmpty)
        await model.openContext()
        guard case let .context(context) = model.shown else { return XCTFail("\(model.shown)") }
        XCTAssertFalse(context.blocks.isEmpty)
        await model.search("studio review")
        guard case let .search(found) = model.shown else { return XCTFail("\(model.shown)") }
        XCTAssertFalse(found.hits.isEmpty)
        await model.prepareDelete(StoredAction(["do": "turn", "id": "sample-request"])!)
        XCTAssertNotNil(model.asking)
        await model.confirmDelete()
        await model.openContext()
        guard case let .context(after) = model.shown else { return XCTFail("\(model.shown)") }
        XCTAssertTrue(after.blocks.isEmpty)
    }
    #endif
}
