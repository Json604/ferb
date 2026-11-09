import Foundation
import NotchCore

/// A daemon with the chat history RPCs (APP-805) kept in memory, as ferb/store/chat_history.py keeps them: put replaces,
/// import keeps what it holds, a day reads newest first in pages with the turns its replies answer. Other methods
/// answer as ScriptedRPC does (`chat.status` empty). `refuse`: an older daemon, which has no such RPC.
final class HistoryRPC: DaemonRPCTransport, @unchecked Sendable {
    private let lock = NSLock()
    private(set) var rows: [String: [String: Any]] = [:]
    private(set) var calls: [(String, [String: Any])] = []
    var refuse = false
    var failPuts = false

    func held(_ id: UUID) -> [String: Any]? { lock.lock(); defer { lock.unlock() }; return rows[id.uuidString] }
    func count(_ method: String) -> Int { lock.lock(); defer { lock.unlock() }; return calls.filter { $0.0 == method }.count }
    func last(_ method: String) -> [String: Any]? { lock.lock(); defer { lock.unlock() }; return calls.last { $0.0 == method }?.1 }

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        calls.append((method, params))
        if method.hasPrefix("chat.history.") && refuse { throw DaemonRPCError.remote("no such method", code: -32601) }
        switch method {
        case "chat.history.put", "chat.history.import":
            if method == "chat.history.put" && failPuts { throw DaemonRPCError.unavailable }
            let entries = params["entries"] as? [[String: Any]] ?? []
            var changed = 0
            for entry in entries {
                guard let id = (entry["id"] as? String)?.uppercased() else { continue }
                if method == "chat.history.import" && rows[id] != nil { continue }
                rows[id] = entry
                changed += 1
            }
            return ["changed": changed, "held": entries.filter { rows[($0["id"] as? String ?? "").uppercased()] != nil }.count]
        case "chat.history.page":
            let day = params["day"] as? String ?? ""
            let limit = params["limit"] as? Int ?? 30
            let key: ([String: Any]) -> String = { "\($0["at"] as? String ?? "")|\($0["id"] as? String ?? "")" }
            var all = rows.values.filter { $0["day"] as? String == day }.sorted { key($0) < key($1) }
            if let before = params["before"] as? String { all = all.filter { key($0) < before } }
            let page = Array(all.suffix(limit))
            let more = all.count > limit
            let shown = Set(page.compactMap { $0["id"] as? String })
            let answered = Set(page.compactMap { ($0["body"] as? [String: Any])?["turnID"] as? String })
            var out: [String: Any] = ["entries": page, "more": more, "cursor": more ? key(page[0]) : NSNull(),
                                      "context": answered.subtracting(shown).compactMap { rows[$0] }]
            if params["open"] as? Bool == true { out["open"] = rows.values.filter { $0["open"] as? Bool == true } }
            return out
        case "chat.history.days":
            return ["days": Array(Set(rows.values.compactMap { $0["day"] as? String })).sorted(by: >)]
        case "chat.status":
            return ["results": [], "approvals": [], "loops": [], "notices": [], "running": [], "paused": false,
                    "stopped": false]
        case "chat.attach":
            return ["session_id": "S1", "capabilities": []]
        default:
            return ["ok": true]
        }
    }

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
