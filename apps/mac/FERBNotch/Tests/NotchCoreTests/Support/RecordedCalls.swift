import Foundation

/// The RPC fakes record calls from whichever task makes them (the model's daemon calls run off the main actor) while
/// tests read them on the main actor. The lock keeps those concurrent appends and reads from corrupting the array,
/// which crashed the suite intermittently.
final class RecordedCalls: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [(String, [String: Any])] = []

    func append(_ method: String, _ params: [String: Any]) {
        lock.lock()
        storage.append((method, params))
        lock.unlock()
    }

    var all: [(String, [String: Any])] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }
}
