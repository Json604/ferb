#if FERB_PLAYGROUND
import Foundation

/// Settings → Stored data in the playground (STORE-423): FERB's `data.*` calls answered from a small invented database
/// (a conversation turn, its task and trace, a memory, an audit row and a safety record), changed only here.
@MainActor
final class PlaygroundStoredData {
    private(set) var deleted = false

    private static let groups: [[String: Any]] = [
        ["id": "conversation", "title": "Conversation"], ["id": "tasks", "title": "Tasks and results"],
        ["id": "model_input", "title": "What FERB sent for thinking"], ["id": "memory", "title": "Memory"],
        ["id": "activity", "title": "Activity log"], ["id": "safety", "title": "Safety records"],
        ["id": "other", "title": "Everything else"]]

    private static let context = "Layer 3 — the conversation (oldest first):\n[Sat 15 Nov 10:02] owner: Plan my " +
        "afternoon around the studio review.\n[Sat 15 Nov 10:03] FERB: Here is a sample plan for the afternoon."

    /// table → (group, columns, rows newest first, action, note)
    private var tables: [(String, String, [String], [[Any]], String?, String)] {
        let turn: [[Any]] = deleted ? [] : [[1, "sample-turn", "turn", PlaygroundSamples.time(-600), "{\"requestID\": \"sample-request\", \"request\": \"Plan my afternoon around the studio review.\"}"]]
        return [
            ("chat_history", "conversation", ["rowid", "id", "kind", "at", "body_json"], turn, "turn",
             "A turn's rows can be deleted: its message, plan, answers, tasks, results and traces."),
            ("tasks", "tasks", ["rowid", "id", "request_id", "goal", "status"],
             deleted ? [] : [[1, "sample-brief", "sample-request", "Prepare the afternoon brief", "done"]], "turn",
             "A turn's rows can be deleted: its message, plan, answers, tasks, results and traces."),
            ("spans", "model_input", ["rowid", "id", "kind", "input_json"],
             deleted ? [] : [[1, "sample-span", "model.turn", "{\"role\": \"supervisor\", \"request_id\": \"sample-request\", \"context\": \(Self.quoted(Self.context))}"]],
             nil, "Traces: deleting a turn deletes its own; later turns' copies of its words are blanked where found."),
            ("memory_items", "memory", ["rowid", "id", "content"], [[1, "sample-memory", "Prefers focused work after lunch"]],
             "forget", "A memory can be forgotten (the Memory page's Forget)."),
            ("journal", "activity", ["rowid", "id", "kind", "data_json"],
             [[2, "sample-audit", "task.started", "{\"task_id\": \"sample-brief\"}"]] + (deleted ? [] :
                [[1, "sample-message", "chat.message", "{\"request_id\": \"sample-request\", \"text\": \"Plan my afternoon around the studio review.\"}"]]),
             "turn", "The audit log is not deletable here. Deleting a turn removes that turn's conversation rows from it."),
            ("confirmations", "safety", ["rowid", "id", "tree_id", "answer"], [[1, "sample-confirmation", "sample-brief", "yes"]],
             nil, "Not deletable here: FERB's safety rules read these records. Uninstall → delete everything removes them."),
        ]
    }

    private static func quoted(_ text: String) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: [text], options: [.withoutEscapingSlashes])) ?? Data()
        let list = String(data: data, encoding: .utf8) ?? "[\"\"]"
        return String(list.dropFirst().dropLast())
    }

    private func rowReply(_ name: String, _ columns: [String], _ row: [Any], _ action: String?) -> [String: Any] {
        var out: [String: Any] = ["rowid": row[0], "cells": Array(row.dropFirst())]
        if let action {
            let id: String? = action == "forget" ? row[1] as? String
                : ((row.last as? String)?.contains("sample-request") == true || name == "tasks" ? "sample-request" : nil)
            if let id { out["action"] = ["do": action, "id": id] }
        }
        return out
    }

    func answer(_ method: String, _ params: [String: Any]) -> [String: Any] {
        let table = params["table"] as? String ?? ""
        let found = tables.first { $0.0 == table }
        switch method {
        case "data.tables":
            return ["groups": Self.groups, "tables": tables.map { t in
                ["name": t.0, "group": t.1, "rows": t.3.count, "columns": Array(t.2.dropFirst()), "internal": false,
                 "action": t.4 as Any, "note": t.5] }
                + [["name": "chat_fts", "group": "other", "rows": deleted ? 0 : 1, "columns": ["text"], "internal": true,
                    "action": NSNull(), "note": ""]]]
        case "data.rows":
            guard let t = found else { return ["table": table, "columns": [], "rows": [], "total": 0] }
            var rows = t.3
            if let filter = params["filter"] as? [String: Any], let prefix = filter["starts_with"] as? String {
                rows = rows.filter { ($0[2] as? String)?.hasPrefix(prefix) == true }
            }
            return ["table": t.0, "columns": Array(t.2.dropFirst()), "total": rows.count, "note": t.5,
                    "rows": rows.map { rowReply(t.0, t.2, $0, t.4) }]
        case "data.row":
            guard let t = found, let row = t.3.first(where: { $0[0] as? Int == params["rowid"] as? Int }) else {
                return ["table": table, "rowid": params["rowid"] ?? 0, "columns": [], "cells": []]
            }
            return rowReply(t.0, t.2, row, t.4).merging(["table": t.0, "columns": Array(t.2.dropFirst()), "note": t.5]) { $1 }
        case "data.cell":
            return ["table": table, "rowid": params["rowid"] ?? 0, "column": params["column"] ?? "", "offset": 0, "chars": 0,
                    "text": "", "end": true]
        case "data.search":
            let text = (params["text"] as? String ?? "").lowercased()
            var hits: [[String: Any]] = []
            for t in tables {
                for row in t.3 {
                    for (index, cell) in row.dropFirst().enumerated() {
                        if let s = cell as? String, !text.isEmpty, s.lowercased().contains(text) {
                            hits.append(["table": t.0, "group": t.1, "rowid": row[0], "column": t.2[index + 1], "snippet": s])
                        }
                    }
                }
            }
            return ["text": params["text"] ?? "", "hits": hits, "tables_read": tables.count, "stopped": false]
        case "data.context":
            return ["conversation": "playground", "blocks": deleted ? [] : [["text": Self.context, "origins": ["owner", "ferb"]]]]
        case "data.delete":
            let learned: [[String: Any]] = [["id": "sample-learned", "text": "Wants the studio review first on Fridays"]]
            let shared: [[String: Any]] = [["id": "sample-memory", "text": "Prefers focused work after lunch"]]
            var preview: [String: Any] = ["tasks": 1, "conversation_rows": 2, "summaries": 0, "chat_entries": 2,
                                          "chat_history": 1, "traces": 3, "screenshots": 1, "episodes": 1,
                                          "memories": learned, "learned": [[String: Any]]()]
            let keeps: [String: Any] = ["audit_log": 1, "confirmations": 1, "action_ledger": 0, "memories_shared": 1]
            if params["confirm"] as? Bool == true {
                deleted = true
                preview["memories"] = 1
                preview["learned"] = 0
                return ["confirmed": true, "request_id": params["request_id"] ?? "", "deleted": preview, "keeps": keeps]
            }
            return ["confirmed": false, "request_id": params["request_id"] ?? "", "will_delete": preview, "keeps": keeps,
                    "keeps_memories": shared, "keeps_learned": [[String: Any]]()]
        default:
            return [:]
        }
    }
}
#endif
