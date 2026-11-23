import FerbMobile
import Foundation

/// One conversation of the engine's chat fixtures (engine/fixtures/chat-entries), as the Mac stores it.
struct FixtureConversation {
    let file: String
    let scenario: String
    let conversation: String
    let entries: [[String: Any]]
}

extension FixtureConversation {
    /// `question_left_open` as the Mac stores it, its "Which colour?" (`ap-1`) asked loose (no card row) and the
    /// questions in `answered` answered: its two open questions are both drawn at the end of the chat by FERB.app.
    static func looseQuestions(answered: Set<String> = []) throws -> FixtureConversation {
        let c = try Fixtures.conversations().first { $0.scenario == "question_left_open" }!
        let entries = c.entries.map { e -> [String: Any] in
            var x = e
            if x["approval_id"] as? String == "ap-1", var body = x["body"] as? [String: Any] {
                body["part"] = NSNull()
                x["body"] = body
            }
            if let id = x["approval_id"] as? String, answered.contains(id) {
                x["state"] = "answered"; x["open"] = false; x["rev"] = 9; x["seq"] = 20
            }
            return x
        }
        return FixtureConversation(file: c.file, scenario: c.scenario, conversation: c.conversation, entries: entries)
    }
}

enum Fixtures {
    /// engine/fixtures/chat-entries in this checkout (the simulator reads the Mac's files).
    static let dir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent() // FerbApp
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent() // repo
        .appendingPathComponent("engine/fixtures/chat-entries", isDirectory: true)

    static func conversations() throws -> [FixtureConversation] {
        var out: [FixtureConversation] = []
        for file in ["scenarios.json", "engine_chat.json", "engine_more.json"] {
            let data = try Data(contentsOf: dir.appendingPathComponent(file))
            let root = try JSONSerialization.jsonObject(with: data) as! [String: Any]
            for s in root["scenarios"] as! [[String: Any]] {
                let entries = s["entries"] as! [[String: Any]]
                var order: [String] = []
                var by: [String: [[String: Any]]] = [:]
                for e in entries {
                    let c = e["conversation_id"] as! String
                    if by[c] == nil { order.append(c) }
                    by[c, default: []].append(e)
                }
                for c in order { out.append(FixtureConversation(file: file, scenario: s["name"] as! String, conversation: c, entries: by[c]!)) }
            }
        }
        return out
    }

    static func write(_ v: Any) -> String {
        let data = try! JSONSerialization.data(withJSONObject: v, options: [.fragmentsAllowed])
        return String(decoding: data, as: UTF8.self)
    }
}

/// A Mac that serves one stored conversation by ENG-14's rules (pages newest first, changes after a cursor, the watch
/// caught up), as ferb-mobile's tests/common/mac.rs does, and answers questions and Stop (Android's FixtureMac.kt).
/// `refuse` makes a method answer with a link code.
final class FixtureMac: Transport, @unchecked Sendable {
    private let c: FixtureConversation
    private let lock = NSLock()
    private var _calls: [String] = []
    private var _refuse: [String: String] = [:]
    /// Under the lock: the library calls `call` from its own threads (the sync a link event starts) while a test may
    /// set this. Thread Sanitizer caught that race in another fake built the same way: a torn dictionary read there
    /// raised an Objective-C exception through the library's Rust frames ("Rust cannot catch foreign exceptions").
    var refuse: [String: String] {
        get { lock.lock(); defer { lock.unlock() }; return _refuse }
        set { lock.lock(); defer { lock.unlock() }; _refuse = newValue }
    }
    private let byPos: [[String: Any]]
    private let head: Int64

    var calls: [String] { lock.lock(); defer { lock.unlock() }; return _calls }

    init(_ c: FixtureConversation) {
        self.c = c
        byPos = c.entries.sorted { num($0["pos"]) < num($1["pos"]) }
        head = c.entries.map { Int64(num($0["seq"])) }.max() ?? 0
    }

    func call(method: String, paramsJson: String, proofJson: String?) throws -> String {
        lock.lock()
        _calls.append(method)
        let n = _calls.count
        let refused = _refuse[method]
        lock.unlock()
        if let code = refused { throw TransportError.Failed(code: code, detail: "refused by the test", allow: nil, retryAfterS: nil) }
        let p = (try? JSONSerialization.jsonObject(with: Data(paramsJson.utf8))) as? [String: Any] ?? [:]
        switch method {
        case "chat.entries.page":
            let before = p["before_pos"].flatMap { $0 is NSNull ? nil : num($0) }
            let limit = p["limit"].map { Int(num($0)) } ?? 50
            let all = byPos.filter { before == nil || num($0["pos"]) < before! }
            let from = max(0, all.count - limit)
            let more = from > 0
            return Fixtures.write(["conversation": c.conversation, "epoch": "E1", "head": head, "entries": Array(all[from...]),
                                   "more": more, "before_pos": more ? all[from]["pos"]! : NSNull()])
        case "chat.entries.changes":
            let after = p["after_seq"].map(num) ?? 0
            let es = c.entries.filter { num($0["seq"]) > after }.sorted { num($0["seq"]) < num($1["seq"]) }
            return Fixtures.write(["conversation": c.conversation, "epoch": "E1", "head": head, "entries": es, "more": false,
                                   "next_seq": es.last?["seq"] ?? after])
        case "chat.entries.mark":
            return #"{"marks":{}}"#
        case "link.challenge", "phone.challenge":
            return #"{"challenge":"C-\#(n)"}"#
        case "link.unlock", "task.stop", "chat.approve", "chat.answer", "chat.correct":
            return #"{"ok":true}"#
        default:
            throw TransportError.Failed(code: "method_not_allowed", detail: "\(method) is not allowed", allow: nil, retryAfterS: nil)
        }
    }

    func send(paramsJson: String) throws -> String {
        let n = calls.count
        return #"{"type":"ack","request_id":"r-\#(n)","entry_id":"pm-\#(n)","seq":\#(head + Int64(n)),"epoch":"E1","submission_id":"sub-\#(n)"}"#
    }

    func watch(paramsJson: String, sink: WatchSink) throws -> UInt64 {
        sink.frame(frameJson: #"{"type":"caught_up","conversation":"\#(c.conversation)","epoch":"E1","head":\#(head)}"#)
        return 1
    }

    func closeWatch(watchId: UInt64) {}

    func get(id: String, paramsJson: String, saveTo: String?, append: Bool, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "not_found", detail: "The fixture Mac holds no files.", allow: nil, retryAfterS: nil)
    }

    func put(id: String, path: String, optsJson: String, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "not_found", detail: "The fixture Mac takes no files.", allow: nil, retryAfterS: nil)
    }

    func cancel(id: String) {}

    private final class FixedClock: Clock, @unchecked Sendable { func nowIso() -> String { "2026-01-02T10:00:00+00:00" } }
    private final class Counted: Ids, @unchecked Sendable {
        private var n = 0
        private let l = NSLock()
        func newId() -> String { l.lock(); defer { l.unlock() }; n += 1; return "id-\(n)" }
    }
    private final class Deaf: Listener, @unchecked Sendable { func changed(change: Change) {} }

    /// A phone core (the real library, on the simulator) synced from `mac`: its transcript of the conversation.
    static func transcript(_ mac: FixtureMac, utcOffsetS: Int32 = 0) throws -> TranscriptView {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-app-test-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let m = try Mobile.open(path: dir.appendingPathComponent("phone.db").path, key: Data(repeating: 7, count: 32),
                                config: MobileConfig(clientId: "client-test", utcOffsetS: utcOffsetS, pageLimit: nil, maxBytes: nil, pieceChars: nil),
                                clock: FixedClock(), ids: Counted(), listener: Deaf())
        defer { m.shutdown() }
        try m.connectTransport(transport: mac)
        try m.linkEvent(eventJson: #"{"type":"state","phase":"online","tier":"direct"}"#)
        try m.sync()
        guard let conv = try m.currentConversation() else { throw NSError(domain: "fixture", code: 1, userInfo: [NSLocalizedDescriptionKey: "no conversation after sync"]) }
        return try m.transcript(conversation: conv)
    }
}

private func num(_ v: Any?) -> Double { (v as? NSNumber)?.doubleValue ?? 0 }
