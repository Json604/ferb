import Foundation
import XCTest
import FerbMobile

// The phone library on the iOS simulator (stage 4, slice I1): the static archive and the generated Swift load, a store
// opens with a key, syncs from a fake Mac written here in Swift, sends through the outbox, errors arrive typed, and
// shutdown closes every call. The Swift twin of the scratch check journal 112 ran on the Mac host, now on iOS. The
// panic hooks need the library's `testing` feature, which the phone build leaves out (ferb-mobile-ffi's own tests
// cover them on the host).

private final class FixedClock: Clock, @unchecked Sendable { func nowIso() -> String { "2026-01-02T10:00:00+00:00" } }

private final class Counted: Ids, @unchecked Sendable {
    private var n = 0
    private let l = NSLock()
    func newId() -> String { l.lock(); defer { l.unlock() }; n += 1; return "sub-\(n)" }
}

private final class Heard: Listener, @unchecked Sendable {
    private var _changes: [Change] = []
    private let l = NSLock()
    /// Read under the lock too: the library calls `changed` from its own threads.
    var changes: [Change] { l.lock(); defer { l.unlock() }; return _changes }
    func changed(change: Change) { l.lock(); _changes.append(change); l.unlock() }
}

private let invented = #"{"id":"m-1","conversation_id":"c1","pos":4294967296,"seq":1,"rev":1,"kind":"message","state":"done","open":false,"request_id":"r-1","client_id":null,"submission_id":null,"created_at":"2026-01-02T10:00:00+00:00","updated_at":"2026-01-02T10:00:00+00:00","day":"2026-01-02","body":{"text":"Invented question","attachments":[],"hidden":[]},"deleted":false}"#

/// A Mac that answers the calls a first sync makes, in this test's own words.
private final class FakeMac: Transport, @unchecked Sendable {
    private let l = NSLock()
    private var _sends: [String] = []
    var sends: [String] { l.lock(); defer { l.unlock() }; return _sends }
    var offline = false

    func call(method: String, paramsJson: String, proofJson: String?) throws -> String {
        if offline { throw TransportError.Failed(code: "mac_unreachable", detail: "The Mac can't be reached.", allow: nil, retryAfterS: nil) }
        switch method {
        case "chat.entries.page":
            return #"{"conversation":"c1","epoch":"E1","head":1,"entries":["# + invented + #"],"more":false,"before_pos":null}"#
        case "chat.entries.changes":
            return #"{"conversation":"c1","epoch":"E1","head":1,"entries":[],"more":false,"next_seq":1}"#
        case "link.challenge":
            return #"{"challenge":"C-1"}"#
        default:
            throw TransportError.Failed(code: "method_not_allowed", detail: method, allow: nil, retryAfterS: nil)
        }
    }

    func send(paramsJson: String) throws -> String {
        if offline { throw TransportError.Failed(code: "mac_unreachable", detail: "The Mac can't be reached.", allow: nil, retryAfterS: nil) }
        l.lock(); _sends.append(paramsJson); l.unlock()
        return #"{"type":"ack","request_id":"r-2","entry_id":"pm-2","seq":2,"epoch":"E1","submission_id":"sub-1"}"#
    }

    func watch(paramsJson: String, sink: WatchSink) throws -> UInt64 {
        sink.frame(frameJson: #"{"type":"caught_up","conversation":"c1","epoch":"E1","head":1}"#)
        return 1
    }

    func closeWatch(watchId: UInt64) {}
    func get(id: String, paramsJson: String, saveTo: String?, append: Bool, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "unsupported", detail: "no files here", allow: nil, retryAfterS: nil)
    }
    func put(id: String, path: String, optsJson: String, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "unsupported", detail: "no files here", allow: nil, retryAfterS: nil)
    }
    func cancel(id: String) {}
}

private final class KeptState: LinkListener, @unchecked Sendable {
    private let l = NSLock()
    private var _state: Data?
    private var _status: LinkStatus?
    var state: Data? { l.lock(); defer { l.unlock() }; return _state }
    var status: LinkStatus? { l.lock(); defer { l.unlock() }; return _status }
    func statusChanged(status: LinkStatus) { l.lock(); _status = status; l.unlock() }
    func persist(state: Data) { l.lock(); _state = state; l.unlock() }
}

final class FerbMobileTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-mobile-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func open(key: Data = Data(repeating: 7, count: 32), heard: Heard = Heard()) throws -> Mobile {
        try Mobile.open(path: dir.appendingPathComponent("phone.db").path, key: key,
                        config: MobileConfig(clientId: "phone-1", utcOffsetS: 0, pageLimit: nil, maxBytes: nil, pieceChars: nil),
                        clock: FixedClock(), ids: Counted(), listener: heard)
    }

    // decision: PHN-446, PHN-492, PHN-570
    func testTheLibrarySyncsFromAFakeMacSendsAndShutsDown() throws {
        XCTAssertFalse(libraryVersion().isEmpty)
        let heard = Heard()
        let mac = FakeMac()
        let m = try open(heard: heard)
        try m.connectTransport(transport: mac)
        try m.linkEvent(eventJson: #"{"type":"state","phase":"online","tier":"direct"}"#)
        try m.sync()
        let view = try m.transcript(conversation: "c1")
        guard case .turn(let turn) = view.items.first else { return XCTFail("a turn first: \(view.items)") }
        XCTAssertEqual(turn.words?.text, "Invented question")
        XCTAssertEqual(try m.currentConversation(), "c1")

        let sent = try m.send(text: "Hello from Swift")
        XCTAssertEqual(sent.text, "Hello from Swift")
        XCTAssertTrue(try m.outbox().isEmpty, "acknowledged, so gone from the outbox")
        XCTAssertEqual(mac.sends.count, 1)
        XCTAssertTrue(mac.sends[0].contains("Hello from Swift"))
        XCTAssertTrue(mac.sends[0].contains("phone-1"), "sent with this phone's client id")
        XCTAssertFalse(heard.changes.isEmpty, "the listener heard the sync")

        m.shutdown(); m.shutdown()
        XCTAssertTrue(m.isClosed())
        XCTAssertThrowsError(try m.sync()) { error in
            guard case FerbError.Closed(let detail, _) = error else { return XCTFail("\(error)") }
            XCTAssertFalse(detail.isEmpty)
        }
        XCTAssertThrowsError(try m.send(text: "after")) { XCTAssertEqual(($0 as? FerbError).map(code), "closed") }
    }

    // decision: PHN-401, PHN-476
    func testWithTheMacOffAMessageWaitsVisiblyAndCanBeCancelled() throws {
        let mac = FakeMac()
        mac.offline = true
        let m = try open()
        try m.connectTransport(transport: mac)
        try m.linkEvent(eventJson: #"{"type":"state","phase":"waiting","tier":"offline"}"#)
        let item = try m.send(text: "Later, please")
        let box = try m.outbox()
        XCTAssertEqual(box.map(\.id), [item.id])
        let cancelled = try m.cancel(outboxId: item.id)
        XCTAssertFalse(cancelled.mayHaveArrived, "never sent, so it can't have arrived")
        XCTAssertTrue(try m.outbox().isEmpty)
        m.shutdown()
    }

    // decision: PHN-448, PHN-474
    func testTheStoreIsEncryptedAndAnotherKeyIsRefusedTyped() throws {
        let m = try open()
        try m.connectTransport(transport: FakeMac())
        try m.linkEvent(eventJson: #"{"type":"state","phase":"online","tier":"direct"}"#)
        try m.sync()
        let path = try m.storePath()
        m.shutdown()
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        XCTAssertNil(bytes.range(of: Data("Invented question".utf8)), "no plain text on disk")
        XCTAssertNil(bytes.range(of: Data("SQLite format 3".utf8)), "not a plain SQLite file")

        XCTAssertThrowsError(try open(key: Data(repeating: 9, count: 32))) { XCTAssertEqual(($0 as? FerbError).map(code), "wrong_key") }
        XCTAssertThrowsError(try open(key: Data(repeating: 7, count: 31))) { XCTAssertEqual(($0 as? FerbError).map(code), "bad_key") }
        let again = try open()
        XCTAssertEqual(try again.currentConversation(), "c1", "the same key opens what was kept")
        try again.forgetThisMac()
        XCTAssertNil(try? again.currentConversation() ?? nil)
        again.shutdown()
    }

    // decision: PHN-494, PHN-440
    func testTheRealLinkStartsOnIOSKeepsItsKeyAsBytesAndRefusesForeignState() throws {
        let kept = KeptState()
        let config = { (state: Data?) in
            LinkConfig(state: state, name: "Invented iPhone", app: "com.ferb.phone test", unlockPub: nil, devicePub: nil,
                       strongPub: nil, segmentationOffload: false,
                       // loopback only (TEST-508): a relay address nothing answers, no port mapping at a gateway
                       devSharedRelays: ["http://127.0.0.1:9"], devForceRelay: false, devPortmapper: false)
        }
        let link = try Link.start(config: config(nil), listener: kept, deviceSigner: nil)
        let id = link.endpointId()
        XCTAssertFalse(id.isEmpty)
        // handed over on the link's own thread right after the start
        let deadline = Date().addingTimeInterval(10)
        while kept.state == nil, Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        let state = try XCTUnwrap(kept.state, "a first start hands its new key over")
        XCTAssertEqual(link.status().phase, .idle, "not paired")
        XCTAssertFalse(link.status().paired)
        XCTAssertThrowsError(try link.pair(code: "FERB-not-a-real-code")) { error in
            XCTAssertEqual((error as? FerbError).map(code), "refused", "\(error)")
        }
        link.shutdown()
        XCTAssertTrue(link.isClosed())

        let again = try Link.start(config: config(state), listener: KeptState(), deviceSigner: nil)
        XCTAssertEqual(again.endpointId(), id, "the kept state is the same phone")
        again.shutdown()
        XCTAssertThrowsError(try Link.start(config: config(Data("{\"not\":\"ours\"}".utf8)), listener: KeptState(), deviceSigner: nil)) {
            XCTAssertEqual(($0 as? FerbError).map(code), "bad_argument", "never silently a new identity")
        }
    }
}

/// The stable code (ferb-mobile-ffi FerbError::code), as the app's core maps it.
private func code(_ e: FerbError) -> String {
    switch e {
    case .Closed: "closed"
    case .BadKey: "bad_key"
    case .WrongKey: "wrong_key"
    case .StoreNewer: "store_newer"
    case .Store: "store"
    case .Offline: "offline"
    case .NeedsBetterPath: "needs_better_path"
    case .NeedsUnlock: "needs_unlock"
    case .Refused: "refused"
    case .NeedsSigner: "needs_signer"
    case .NotSigned: "not_signed"
    case .NotFound: "not_found"
    case .InFlight: "in_flight"
    case .BadReply: "bad_reply"
    case .Cancelled: "cancelled"
    case .BadArgument: "bad_argument"
    case .Internal: "internal"
    }
}
