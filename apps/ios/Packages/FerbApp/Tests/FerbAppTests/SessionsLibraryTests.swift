import FerbMobile
import Foundation
import XCTest

/// A Mac that answers the `sessions.*` calls with the daemon's shapes (the contract: engine/crates/ferb-daemon
/// sessions/rpc.rs), for the real phone library on the simulator (the chat's FixtureMac holds no sessions).
final class SessionsMac: Transport, @unchecked Sendable {
    private let lock = NSLock()
    private var _calls: [(method: String, params: [String: Any], proof: String?)] = []
    /// A method the Mac refuses, with the link's code and the Mac's words.
    var refuse: [String: (String, String)] = [:]

    var calls: [(method: String, params: [String: Any], proof: String?)] { lock.lock(); defer { lock.unlock() }; return _calls }

    static let session: [String: Any] = [
        "id": "claude:abc", "agent": "claude_code", "cwd": "/Users/owner/code/garden-app/", "title": "", "status": "waiting_for_you",
        "waiting": "permission", "host": ["bundle_id": "com.apple.Terminal", "name": "Terminal", "pid": 42, "via": "tty", "tty": "/dev/ttys003"],
        "send_paths": ["foreground", "terminal_script"], "last_at": "2026-01-02T10:00:00Z", "last_text": "Run npm test?",
        "control": "phone:p-1", "delegate": "t-9", "yours": true, "ferb_note": "FERB is on it: “Fix the test”",
    ]

    func call(method: String, paramsJson: String, proofJson: String?) throws -> String {
        let p = (try? JSONSerialization.jsonObject(with: Data(paramsJson.utf8))) as? [String: Any] ?? [:]
        lock.lock()
        _calls.append((method, p, proofJson))
        lock.unlock()
        if let (code, words) = refuse[method] { throw TransportError.Failed(code: code, detail: words, allow: nil, retryAfterS: nil) }
        let v: Any
        switch method {
        case "link.challenge": v = ["challenge": "C-1"]
        case "sessions.list":
            var codex = Self.session
            codex["id"] = "codex:t-2"
            codex["agent"] = "codex"
            codex["title"] = "Tidy the README"
            codex["status"] = "idle"
            codex["waiting"] = "none"
            codex["host"] = NSNull()
            codex["send_paths"] = [String]()
            codex["no_send"] = "No app on your Mac shows this session."
            codex["yours"] = false
            codex.removeValue(forKey: "delegate")
            codex.removeValue(forKey: "ferb_note")
            codex.removeValue(forKey: "control")
            // HAND-710: an app's own window, which FERB reads no words of
            let window: [String: Any] = [
                "id": "app:cursor:9", "agent": "app_chat", "cwd": "", "title": "Fix the login form — garden-app",
                "window": "Fix the login form — garden-app", "program": "", "status": "idle", "waiting": "none",
                "host": ["bundle_id": "com.todesktop.230313mzl4w4u92", "name": "Cursor", "pid": 77], "send_paths": ["window"],
                "watch": true, "yours": false,
            ]
            v = ["enabled": true, "sessions": [Self.session, codex, window]]
        case "sessions.timeline":
            v = ["session": Self.session, "more": true,
                 "items": [["seq": 7, "at": "2026-01-02T10:00:00Z", "kind": "user", "text": "Fix the test"],
                           ["seq": 8, "at": "2026-01-02T10:00:01Z", "kind": "tool", "text": "Bash: npm test", "tool": "Bash"],
                           ["seq": 9, "at": "2026-01-02T10:00:02Z", "kind": "assistant", "text": "**Done.**",
                            "parts": [["kind": "thinking", "label": "Thought 2s", "text": ""], ["kind": "text", "text": "**Done.**"],
                                      ["kind": "tool", "label": "Run the tests", "detail": "npm test", "text": "ok"],
                                      ["kind": "meta", "text": "Just now"]]]],
                 "cards": [["id": "card-1", "session": "claude:abc", "kind": "permission", "tool": "Bash", "detail": "npm test",
                            "choices": [["id": "allow", "label": "Yes"], ["id": "deny", "label": "No"]], "asked_at": "2026-01-02T10:00:02Z"]]]
        case "sessions.send":
            v = ["needs_yes": ["question": "Type this into Claude Code in Terminal? FERB will bring Terminal to the front to do it.",
                               "detail": p["text"] ?? "", "covers": "Yes lets you send to this session from here until you stop controlling it."]]
        case "sessions.control": v = ["sent": true, "path": "foreground", "delivered": false, "seconds": 1.5, "control": "phone:p-1"]
        case "sessions.answer": v = ["answered": true]
        case "sessions.interrupt": v = ["interrupted": true]
        case "sessions.release": v = ["released": 1]
        case "sessions.prepare":
            var r: [String: Any] = ["folders": ["/Users/owner/code/garden-app", ""], "host": "Terminal"]
            if let c = p["command"] as? String, !c.isEmpty {
                r["needs_yes"] = ["question": "Open a new Terminal window in x and type this?", "detail": c, "covers": ""]
            }
            v = r
        case "sessions.start": v = ["started": true, "command": p["command"] ?? "", "session": "term:300", "host": "Terminal"]
        case "sessions.delegate": v = ["handed_over": true, "message": "FERB is on it."]
        default: throw TransportError.Failed(code: "method_not_allowed", detail: "\(method) is not allowed", allow: nil, retryAfterS: nil)
        }
        return String(decoding: try JSONSerialization.data(withJSONObject: v), as: UTF8.self)
    }

    func send(paramsJson: String) throws -> String { throw TransportError.Failed(code: "not_found", detail: "no chat", allow: nil, retryAfterS: nil) }
    func watch(paramsJson: String, sink: WatchSink) throws -> UInt64 { 1 }
    func closeWatch(watchId: UInt64) {}
    func get(id: String, paramsJson: String, saveTo: String?, append: Bool, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "not_found", detail: "no files", allow: nil, retryAfterS: nil)
    }
    func put(id: String, path: String, optsJson: String, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "not_found", detail: "no files", allow: nil, retryAfterS: nil)
    }
    func cancel(id: String) {}
}

/// The sessions' calls through the real phone library (kb HAND-651): the Mac's answers read into the records the screens
/// use, the parameters and proofs each call carries, and a refusal as the app sees it.
final class SessionsLibraryTests: XCTestCase {
    private final class FixedClock: Clock, @unchecked Sendable { func nowIso() -> String { "2026-01-02T10:00:00+00:00" } }
    private final class Counted: Ids, @unchecked Sendable {
        private var n = 0
        private let l = NSLock()
        func newId() -> String { l.lock(); defer { l.unlock() }; n += 1; return "id-\(n)" }
    }
    private final class Deaf: Listener, @unchecked Sendable { func changed(change: Change) {} }
    private final class Signs: Signer, @unchecked Sendable {
        var payloads: [String] = []
        func sign(payload: String) -> String? { payloads.append(payload); return "sig" }
    }

    private var dir: URL!
    private var mobile: Mobile!
    private let mac = SessionsMac()

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-sessions-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        mobile = try Mobile.open(path: dir.appendingPathComponent("phone.db").path, key: Data(repeating: 7, count: 32),
                                 config: MobileConfig(clientId: "client-test", utcOffsetS: 0, pageLimit: nil, maxBytes: nil, pieceChars: nil),
                                 clock: FixedClock(), ids: Counted(), listener: Deaf())
        try mobile.connectTransport(transport: mac)
        try mobile.linkEvent(eventJson: #"{"type":"state","phase":"online","tier":"direct"}"#)
    }

    override func tearDownWithError() throws {
        mobile?.shutdown()
        try? FileManager.default.removeItem(at: dir)
    }

    // decision: PHN-797, HAND-710
    func testTheListReadsEachSessionAsTheScreensShowIt() throws {
        let l = try mobile.sessionsList()
        XCTAssertTrue(l.enabled)
        XCTAssertEqual(l.sessions.count, 3)
        let a = l.sessions[0]
        XCTAssertEqual(a.agentName, "Claude Code")
        XCTAssertEqual(a.folderName, "garden-app")
        XCTAssertEqual(a.host, "Terminal")
        XCTAssertTrue(a.canSend)
        XCTAssertTrue(a.foreground, "the first path brings the app forward")
        XCTAssertTrue(a.controlledHere, "`yours`")
        XCTAssertTrue(a.withFerb, "a delegate: FERB is on it")
        XCTAssertEqual(a.ferbTask, "t-9", "the delegate: FERB's task, which Take it back stops")
        XCTAssertEqual(a.ferbNote, "FERB is on it: “Fix the test”", "FERB's word, verbatim")
        let b = l.sessions[1]
        XCTAssertEqual(b.agentName, "Codex")
        XCTAssertEqual(b.host, "")
        XCTAssertFalse(b.canSend)
        XCTAssertEqual(b.noSend, "No app on your Mac shows this session.")
        XCTAssertFalse(b.withFerb)
        XCTAssertEqual(b.ferbTask, "")
        XCTAssertEqual(b.ferbNote, "")
        XCTAssertFalse(a.watch, "absent: FERB reads its words")
        XCTAssertFalse(b.watch)
        // HAND-710: an app's window: its words aren't read, it is typed into through the window
        let w = l.sessions[2]
        XCTAssertTrue(w.watch)
        XCTAssertEqual(w.agentName, "Cursor")
        XCTAssertEqual(w.host, "Cursor")
        XCTAssertEqual(w.window, "Fix the login form — garden-app")
        XCTAssertEqual(w.folder, "")
        XCTAssertTrue(w.canSend)
        XCTAssertTrue(w.foreground, "the window path brings its app forward")
    }

    // decision: PHN-797, HAND-663
    func testTheTimelineAsksAfterOrBeforeAndReadsItsCards() throws {
        let first = try mobile.sessionTimeline(session: "claude:abc", after: 0, before: 0, limit: 50)
        XCTAssertEqual(first.items.map(\.seq), [7, 8, 9])
        // HAND-703: a reply's parts, in order, as the Mac sent them
        XCTAssertEqual(first.items[2].parts.map(\.kind), ["thinking", "text", "tool", "meta"])
        XCTAssertEqual(first.items[2].parts[0].label, "Thought 2s")
        XCTAssertEqual(first.items[2].parts[2].detail, "npm test")
        XCTAssertEqual(first.items[2].parts[3].text, "Just now")
        XCTAssertTrue(first.items[0].parts.isEmpty)
        XCTAssertEqual(first.items[1].kind, "tool")
        XCTAssertEqual(first.items[1].text, "Bash: npm test")
        XCTAssertTrue(first.more)
        XCTAssertEqual(first.cards.first?.choices.map(\.id), ["allow", "deny"])
        XCTAssertEqual(first.session?.id, "claude:abc")
        _ = try mobile.sessionTimeline(session: "claude:abc", after: 8, before: 0, limit: 100)
        _ = try mobile.sessionTimeline(session: "claude:abc", after: 0, before: 7, limit: 50)
        let asked = mac.calls.filter { $0.method == "sessions.timeline" }.map(\.params)
        XCTAssertNil(asked[0]["after"])
        XCTAssertNil(asked[0]["before"])
        XCTAssertEqual(asked[1]["after"] as? Int, 8)
        XCTAssertEqual(asked[2]["before"] as? Int, 7)
    }

    // decision: GATE-622
    func testASendAsksAndTheYesIsSignedOverTheSameWords() throws {
        let sent = try mobile.sessionSend(session: "claude:abc", text: "Run the tests")
        XCTAssertFalse(sent.sent, "nothing was sent")
        XCTAssertEqual(sent.needsYes?.detail, "Run the tests")
        let signer = Signs()
        let r = try mobile.sessionControl(session: "claude:abc", text: "Run the tests", terminal: "", signer: signer)
        XCTAssertTrue(r.sent)
        XCTAssertFalse(r.delivered, "said: not seen arriving yet")
        let control = try XCTUnwrap(mac.calls.last { $0.method == "sessions.control" })
        XCTAssertEqual(control.params["text"] as? String, "Run the tests")
        XCTAssertNotNil(control.proof, "signed")
        XCTAssertTrue(signer.payloads.first?.contains("Run the tests") ?? false, "the signature covers the exact words")
    }

    // decision: HAND-663, HAND-695, ORCH-687
    func testAnswersStartsAndHandOversAreSignedEachTime() throws {
        XCTAssertTrue(try mobile.sessionAnswer(session: "claude:abc", card: "card-1", choice: "deny", signer: Signs()))
        let answer = try XCTUnwrap(mac.calls.last { $0.method == "sessions.answer" })
        XCTAssertEqual(answer.params["choice"] as? String, "deny")
        XCTAssertNotNil(answer.proof)
        let plan = try mobile.sessionPrepare(agent: "", folder: "", text: "")
        XCTAssertEqual(plan.folders, ["/Users/owner/code/garden-app"], "empty folders dropped")
        XCTAssertNil(plan.needsYes)
        XCTAssertTrue(mac.calls.last { $0.method == "sessions.prepare" }!.params.isEmpty, "the folder list asks with nothing")
        XCTAssertNil(try mobile.sessionPrepare(agent: "terminal", folder: "/x", text: "").needsYes, "no command: nothing to ask")
        let withFolder = try mobile.sessionPrepare(agent: "terminal", folder: "/x", text: "agy")
        XCTAssertEqual(withFolder.needsYes?.detail, "agy")
        let asked = try XCTUnwrap(mac.calls.last { $0.method == "sessions.prepare" })
        XCTAssertEqual(asked.params["command"] as? String, "agy")
        XCTAssertNil(asked.params["agent"], "no agent: a new session is a terminal")
        let started = try mobile.sessionStart(agent: "terminal", folder: "/x", text: "agy", signer: Signs())
        XCTAssertEqual(started.session, "term:300", "the new session's id, to open it")
        XCTAssertEqual(started.host, "Terminal")
        XCTAssertEqual(started.command, "agy")
        XCTAssertNotNil(mac.calls.last { $0.method == "sessions.start" }?.proof)
        XCTAssertEqual(try mobile.sessionDelegate(session: "claude:abc", goal: "Keep going", signer: Signs()), "FERB is on it.")
        XCTAssertNotNil(mac.calls.last { $0.method == "sessions.delegate" }?.proof)
        XCTAssertEqual(try mobile.sessionRelease(session: "claude:abc"), 1)
        XCTAssertNil(mac.calls.last { $0.method == "sessions.release" }?.proof, "letting go is plain")
    }

    /// The Mac's refusal of a stop on a session this phone doesn't control comes as a refusal with the Mac's words (the
    /// app then asks the Yes); the link carries the daemon's -32045 as its generic `ferb_error`.
    // decision: GATE-622, PHN-797
    func testARefusalComesWithTheMacsWords() {
        mac.refuse["sessions.interrupt"] = ("ferb_error", "Say Yes to sending to this session first.")
        XCTAssertThrowsError(try mobile.sessionInterrupt(session: "claude:abc")) { e in
            guard case FerbError.Refused(let detail, _) = e else { return XCTFail("refused, got \(e)") }
            XCTAssertEqual(detail, "Say Yes to sending to this session first.")
        }
    }
}
