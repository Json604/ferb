import Foundation
import Observation

// Small pieces the model shares: following @Observable values (the Swift twin of collecting a StateFlow), reading
// the JSON texts the app reads itself (a question's body, a setting's value), and waiting a bounded time.

/// Follows a value read from @Observable objects (withObservationTracking, armed again after each change, on the main
/// actor): `onChange` gets the current value at once when `initial`, then each distinct value. `read` returns nil
/// when its owner is gone (it captures it weakly): the follow ends then. Kept alive by its owner; `stop` ends it.
@MainActor
final class Follow<T: Equatable> {
    private let read: () -> T?
    private let onChange: (T) -> Void
    private var last: T?
    private var stopped = false

    init(initial: Bool = true, _ read: @escaping () -> T?, _ onChange: @escaping (T) -> Void) {
        self.read = read
        self.onChange = onChange
        arm(deliver: initial)
    }

    private func arm(deliver: Bool) {
        guard !stopped else { return }
        let v = withObservationTracking(read, onChange: { [weak self] in
            Task { @MainActor in self?.arm(deliver: true) }
        })
        guard let v else {
            stopped = true
            return
        }
        let changed = last != v
        last = v
        if deliver && changed { onChange(v) }
    }

    func stop() { stopped = true }
}

/// Polls `cond` on the main actor every 50 ms for at most `seconds`; true when it held.
@MainActor
func waitFor(_ seconds: Double, _ cond: () -> Bool) async -> Bool {
    let end = Date().addingTimeInterval(seconds)
    while !cond() {
        if Date() >= end { return false }
        try? await Task.sleep(nanoseconds: 50_000_000)
    }
    return true
}

/// Typed reads of a JSON text (Foundation's parser; Android carries its own reader only because org.json is a stub
/// on its JVM tests' classpath).
enum Json {
    static func parse(_ text: String) -> Any? {
        guard let data = text.data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
    }

    static func object(_ text: String) -> [String: Any] { parse(text) as? [String: Any] ?? [:] }

    /// A JSON true or false (never a number that happens to be 1 or 0).
    static func bool(_ v: Any?) -> Bool? {
        guard let n = v as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else { return nil }
        return n.boolValue
    }

    /// A JSON number (never true or false).
    static func number(_ v: Any?) -> Double? {
        guard let n = v as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { return nil }
        return n.doubleValue
    }
}

extension Dictionary where Key == String, Value == Any {
    /// A non-empty string, else nil.
    func str(_ key: String) -> String? { (self[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
    func flag(_ key: String) -> Bool { Json.bool(self[key]) ?? false }
    func strings(_ key: String) -> [String] { (self[key] as? [Any])?.compactMap { $0 as? String } ?? [] }
}

extension String {
    /// Kotlin's isBlank: empty or whitespace only.
    var isBlank: Bool { trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// The lines as Kotlin's lineSequence splits them (\n, \r\n and \r).
    var lines: [String] { split(omittingEmptySubsequences: false, whereSeparator: \.isNewline).map(String.init) }
}

/// A process-unique number for the keys of transfers (System.nanoTime in Android's).
func uptimeNanos() -> UInt64 { DispatchTime.now().uptimeNanoseconds }
