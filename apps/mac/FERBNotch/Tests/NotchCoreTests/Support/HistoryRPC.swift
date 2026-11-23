import Foundation
import NotchCore

/// A daemon with the chat history RPCs (APP-805) kept in memory, as ferb/store/chat_history.py keeps them: put replaces,
/// import keeps what it holds, a day reads newest first in pages with the turns its replies answer. Other methods
/// answer as ScriptedRPC does (`chat.status` empty). `refuse`: an older daemon, which has no such RPC.
final class HistoryRPC: DaemonRPCTransport, @unchecked Sendable {
    // Every field is read and changed under `lock`, from the test and from the model's own tasks alike (the race that
    // crashed ScriptedRPC, f01e1d44); `call` holds it throughout and uses the stored fields.
    private let lock = NSLock()
    private var _rows: [String: [String: Any]] = [:]
    private var _calls: [(String, [String: Any])] = []
    private var _refuse = false
    private var _failPuts = false
    private var _running: [[String: Any]] = []
    private var _results: [[String: Any]] = []
    private var _traces: [String: [String: Any]] = [:]

    private func locked<T>(_ body: () -> T) -> T { lock.lock(); defer { lock.unlock() }; return body() }
    var rows: [String: [String: Any]] { locked { _rows } }
    var calls: [(String, [String: Any])] { locked { _calls } }
    var refuse: Bool { get { locked { _refuse } } set { locked { _refuse = newValue } } }
    var failPuts: Bool { get { locked { _failPuts } } set { locked { _failPuts = newValue } } }
    /// What `chat.status` says is running and which results it hasn't shown yet; `results.ack` marks them shown.
    var running: [[String: Any]] { get { locked { _running } } set { locked { _running = newValue } } }
    var results: [[String: Any]] { get { locked { _results } } set { locked { _results = newValue } } }
    /// `trace.get`'s answer per task.
    var traces: [String: [String: Any]] { get { locked { _traces } } set { locked { _traces = newValue } } }

    func held(_ id: UUID) -> [String: Any]? { locked { _rows[id.uuidString] } }
    func count(_ method: String) -> Int { locked { _calls.filter { $0.0 == method }.count } }
    func last(_ method: String) -> [String: Any]? { locked { _calls.last { $0.0 == method }?.1 } }

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        _calls.append((method, params))
        if method.hasPrefix("chat.history.") && _refuse { throw DaemonRPCError.remote("no such method", code: -32601) }
        switch method {
        case "chat.history.put", "chat.history.import":
            if method == "chat.history.put" && _failPuts { throw DaemonRPCError.unavailable }
            let entries = params["entries"] as? [[String: Any]] ?? []
            var changed = 0
            for entry in entries {
                guard let id = (entry["id"] as? String)?.uppercased() else { continue }
                if method == "chat.history.import" && _rows[id] != nil { continue }
                _rows[id] = entry
                changed += 1
            }
            return ["changed": changed, "held": entries.filter { _rows[($0["id"] as? String ?? "").uppercased()] != nil }.count]
        case "chat.history.page":
            let day = params["day"] as? String ?? ""
            let limit = params["limit"] as? Int ?? 30
            let key: ([String: Any]) -> String = { "\($0["at"] as? String ?? "")|\($0["id"] as? String ?? "")" }
            var all = _rows.values.filter { $0["day"] as? String == day }.sorted { key($0) < key($1) }
            if let before = params["before"] as? String { all = all.filter { key($0) < before } }
            let page = Array(all.suffix(limit))
            let more = all.count > limit
            let shown = Set(page.compactMap { $0["id"] as? String })
            let answered = Set(page.compactMap { ($0["body"] as? [String: Any])?["turnID"] as? String })
            var out: [String: Any] = ["entries": page, "more": more, "cursor": more ? key(page[0]) : NSNull(),
                                      "context": answered.subtracting(shown).compactMap { _rows[$0] }]
            if params["open"] as? Bool == true { out["open"] = _rows.values.filter { $0["open"] as? Bool == true } }
            return out
        case "chat.history.days":
            return ["days": Array(Set(_rows.values.compactMap { $0["day"] as? String })).sorted(by: >)]
        case "chat.status":
            return ["results": _results, "approvals": [], "loops": [], "notices": [], "running": _running, "paused": false,
                    "stopped": false]
        case "results.ack":
            let shown = Set(params["task_ids"] as? [String] ?? [])
            _results.removeAll { shown.contains($0["task_id"] as? String ?? "") }
            return ["ok": true]
        case "trace.get":
            return _traces[params["task_id"] as? String ?? ""] ?? [:]
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
