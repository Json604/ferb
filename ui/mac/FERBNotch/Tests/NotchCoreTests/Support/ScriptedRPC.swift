import Foundation
import NotchCore

/// A daemon fake for the setup window and notification tests: canned replies per method, scripted failures, the events
/// a stream yields, and every call recorded with its params.
final class ScriptedRPC: DaemonRPCTransport, @unchecked Sendable {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    var replies: [String: [String: Any]] = [:]
    var failures: [String: Error] = [:]
    /// The first N calls of a method fail (the daemon not answering yet), then it answers; set before the run.
    var failFirst: [String: Int] = [:]
    var events: [String: [[String: Any]]] = [:]
    /// Replies given in order before `replies` answers (L34 E: a page, then the next; a preview, then the delete).
    var queued: [String: [[String: Any]]] = [:]
    /// The wait each caller asked for, by method (`call(_:params:wait:)`).
    private(set) var waits: [String: Int] = [:]

    func queue(_ method: String, _ items: [[String: Any]]) { queued[method, default: []] += items }

    func call(_ method: String, params: [String: Any], wait seconds: Int) async throws -> [String: Any] {
        waits[method] = seconds
        return try await call(method, params: params)
    }

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        if let left = failFirst[method], left > 0 {
            failFirst[method] = left - 1
            throw DaemonRPCError.invalidResponse
        }
        if var next = queued[method], !next.isEmpty {
            let reply = next.removeFirst()
            queued[method] = next
            return reply
        }
        if let failure = failures[method] { throw failure }
        return replies[method] ?? ["ok": true]
    }

    /// Streams that stay open after their `events` until `endStream` (a task still running).
    var heldStreams: Set<String> = []
    private var open: [String: [AsyncThrowingStream<[String: Any], Error>.Continuation]] = [:]
    private let lock = NSLock()

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        recorded.append(method, params)
        let items = events[method] ?? []
        let held = heldStreams.contains(method)
        return AsyncThrowingStream { continuation in
            for item in items { continuation.yield(item) }
            if held {
                self.lock.lock()
                self.open[method, default: []].append(continuation)
                self.lock.unlock()
            } else {
                continuation.finish()
            }
        }
    }

    /// Ends every open `method` stream after yielding `events`.
    func endStream(_ method: String, with events: [[String: Any]] = []) {
        lock.lock()
        let waiting = open.removeValue(forKey: method) ?? []
        lock.unlock()
        for continuation in waiting {
            for event in events { continuation.yield(event) }
            continuation.finish()
        }
    }

    func methods() -> [String] { calls.map(\.0) }
    func params(of method: String) -> [[String: Any]] { calls.filter { $0.0 == method }.map(\.1) }
}
