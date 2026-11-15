import Foundation
import NotchCore

/// A daemon with the shared conversation (ENG-10, ENG-14) for the entry watch tests: `chat.attach` with its
/// capabilities, `chat.status`, the chat history kept by a `HistoryRPC` (shared across a relaunch), streams scripted per
/// call (`chat.send`'s frames or a failure before any frame; the watch's first frames), streams held open
/// (`logs.subscribe`, `chat.entries.watch`) that a test feeds with `push`, and every call recorded with its params.
final class EntriesRPC: DaemonRPCTransport, @unchecked Sendable {
    enum Script { case frames([[String: Any]]), fail(Error) }

    private let lock = NSLock()
    private var recorded: [(String, [String: Any])] = []
    let history: HistoryRPC
    var capabilities: [String]
    var status: [String: Any] = ["results": [], "approvals": [], "loops": [], "notices": [], "running": [],
                                 "paused": false, "stopped": false]
    /// Each stream call of a method takes the next script; with none left, `defaults`.
    private var scripts: [String: [Script]] = [:]
    var defaults: [String: [[String: Any]]] = [
        "chat.entries.watch": [["type": "caught_up", "conversation": "S1", "epoch": "E1", "head": 0]],
    ]
    /// Streams that stay open after their frames until `end`.
    var held: Set<String> = ["logs.subscribe", "chat.entries.watch"]
    private var open: [String: [AsyncThrowingStream<[String: Any], Error>.Continuation]] = [:]

    init(history: HistoryRPC = HistoryRPC(), capabilities: [String] = ["input_paths", "entries"]) {
        self.history = history
        self.capabilities = capabilities
    }

    var calls: [(String, [String: Any])] { lock.lock(); defer { lock.unlock() }; return recorded }
    func params(of method: String) -> [[String: Any]] { calls.filter { $0.0 == method }.map(\.1) }
    func methods() -> [String] { calls.map(\.0) }

    func script(_ method: String, _ items: Script...) {
        lock.lock(); defer { lock.unlock() }
        scripts[method, default: []] += items
    }

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        let (caps, status) = lock.withLock { () -> ([String], [String: Any]) in
            recorded.append((method, params))
            return (capabilities, self.status)
        }
        switch method {
        case "chat.attach": return ["session_id": "S1", "capabilities": caps]
        case "chat.status": return status
        case "chat.entries.mark": return ["marks": params]
        default:
            if method.hasPrefix("chat.history.") { return try await history.call(method, params: params) }
            return ["ok": true]
        }
    }

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        lock.lock()
        recorded.append((method, params))
        var next: Script = .frames(defaults[method] ?? [])
        if var queue = scripts[method], !queue.isEmpty {
            next = queue.removeFirst()
            scripts[method] = queue
        }
        let keepOpen = held.contains(method)
        lock.unlock()
        return AsyncThrowingStream { continuation in
            switch next {
            case let .fail(error):
                continuation.finish(throwing: error)
            case let .frames(frames):
                for frame in frames { continuation.yield(frame) }
                // a reset, a cursor ahead or a watcher behind: the daemon ends the stream there
                let ends = frames.contains { ["reset_required", "cursor_ahead", "behind"].contains($0["type"] as? String ?? "") }
                if keepOpen && !ends {
                    self.lock.lock()
                    self.open[method, default: []].append(continuation)
                    self.lock.unlock()
                } else {
                    continuation.finish()
                }
            }
        }
    }

    /// Yields `events` on every open `method` stream.
    func push(_ method: String, _ events: [String: Any]...) {
        lock.lock()
        let streams = open[method] ?? []
        lock.unlock()
        for stream in streams { for event in events { stream.yield(event) } }
    }

    /// Ends every open `method` stream.
    func end(_ method: String) {
        lock.lock()
        let streams = open.removeValue(forKey: method) ?? []
        lock.unlock()
        for stream in streams { stream.finish() }
    }

    func endAll() { for method in held { end(method) } }
}
