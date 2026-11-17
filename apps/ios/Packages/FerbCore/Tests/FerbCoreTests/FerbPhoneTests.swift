import Foundation
import FerbMobile
import XCTest
@testable import FerbCore

/// A Mac in this test's own words: one conversation, acknowledgements, a challenge.
final class FakeMac: Transport, @unchecked Sendable {
    private let l = NSLock()
    private var _calls: [String] = []
    var calls: [String] { l.lock(); defer { l.unlock() }; return _calls }
    static let entry = #"{"id":"m-1","conversation_id":"c1","pos":4294967296,"seq":1,"rev":1,"kind":"message","state":"done","open":false,"request_id":"r-1","client_id":null,"submission_id":null,"created_at":"2026-01-02T10:00:00+00:00","updated_at":"2026-01-02T10:00:00+00:00","day":"2026-01-02","body":{"text":"Invented question","attachments":[],"hidden":[]},"deleted":false}"#

    func call(method: String, paramsJson: String, proofJson: String?) throws -> String {
        l.lock(); _calls.append(method + (proofJson == nil ? "" : " signed")); l.unlock()
        switch method {
        case "chat.entries.page": return #"{"conversation":"c1","epoch":"E1","head":1,"entries":["# + Self.entry + #"],"more":false,"before_pos":null}"#
        case "chat.entries.changes": return #"{"conversation":"c1","epoch":"E1","head":1,"entries":[],"more":false,"next_seq":1}"#
        case "link.challenge": return #"{"challenge":"C-1"}"#
        case "link.unlock", "link.lock", "stop.engage": return "{}"
        default: throw TransportError.Failed(code: "method_not_allowed", detail: method, allow: nil, retryAfterS: nil)
        }
    }
    func send(paramsJson: String) throws -> String {
        #"{"type":"ack","request_id":"r-2","entry_id":"pm-2","seq":2,"epoch":"E1","submission_id":"sub-1"}"#
    }
    func watch(paramsJson: String, sink: WatchSink) throws -> UInt64 {
        sink.frame(frameJson: #"{"type":"caught_up","conversation":"c1","epoch":"E1","head":1}"#)
        return 1
    }
    func closeWatch(watchId: UInt64) {}
    func get(id: String, paramsJson: String, saveTo: String?, append: Bool, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "unsupported", detail: "", allow: nil, retryAfterS: nil)
    }
    func put(id: String, path: String, optsJson: String, progress: TransferProgress) throws -> String {
        throw TransportError.Failed(code: "unsupported", detail: "", allow: nil, retryAfterS: nil)
    }
    func cancel(id: String) {}
}

/// The link stood in by the fake Mac: what a start hands over, what pairing and forget do.
final class FakeUplinks: UplinkFactory, @unchecked Sendable {
    let mac = FakeMac()
    private let l = NSLock()
    private var _starts: [UplinkStart] = []
    var starts: [UplinkStart] { l.lock(); defer { l.unlock() }; return _starts }
    var refuseState = false

    func start(_ start: UplinkStart) throws -> Uplink {
        if refuseState, start.state != nil { throw FerbError.BadArgument(detail: "not the link's state", linkCode: nil) }
        l.lock(); _starts.append(start); l.unlock()
        start.listener.persist(state: start.state ?? Data("{\"invented\":\"link state\"}".utf8))
        let u = FakeUplink(mac: mac, listener: start.listener)
        l.lock(); _made.append(u); l.unlock()
        return u
    }
    private var _made: [FakeUplink] = []
    var made: [FakeUplink] { l.lock(); defer { l.unlock() }; return _made }
}

final class FakeUplink: Uplink, @unchecked Sendable {
    let mac: FakeMac
    let listener: LinkListener
    var paired = false
    init(mac: FakeMac, listener: LinkListener) { self.mac = mac; self.listener = listener }
    func attach(_ mobile: Mobile) throws {
        try mobile.connectTransport(transport: mac)
        try mobile.linkEvent(eventJson: #"{"type":"state","phase":"online","tier":"direct"}"#)
    }
    func pair(_ code: String) throws {
        if code != "FERB-good" { throw FerbError.Refused(detail: "That code didn't work.", linkCode: "pair_secret_wrong") }
        paired = true
    }
    func status() -> LinkStatus? { nil }
    /// What the app told the link, in the order it arrived.
    private let l = NSLock()
    private var _said: [String] = []
    var said: [String] { l.lock(); defer { l.unlock() }; return _said }
    private func say(_ s: String) { l.lock(); _said.append(s); l.unlock() }
    func setActive(_ on: Bool) { say("active=\(on)") }
    func setOpen(_ on: Bool) { say("open=\(on)") }
    func networkChanged(_ kind: String?) { say("network=\(kind ?? "nil")") }
    func retry() { say("retry") }
    func forget() throws { paired = false }
    func shutdown() {}
}

/// What `MainGuard` named, from whichever thread.
final class Named: @unchecked Sendable {
    private let l = NSLock()
    private var v: [String] = []
    func add(_ s: String) { l.lock(); v.append(s); l.unlock() }
    var all: [String] { l.lock(); defer { l.unlock() }; return v }
}

@MainActor
final class FerbPhoneTests: XCTestCase {
    private var dir: URL!

    override func setUp() async throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-phone-\(UUID().uuidString)/ferb")
    }

    override func tearDown() async throws { try? FileManager.default.removeItem(at: dir.deletingLastPathComponent()) }

    private func phone(_ secrets: MemoryStore, _ uplinks: FakeUplinks) -> FerbPhone {
        let keys = PhoneKeys(backend: SoftwareBackend(store: secrets), record: dir.appendingPathComponent("keys.made"))
        return FerbPhone(vault: Vault(dir: dir, secrets: secrets, keys: keys), uplinks: uplinks,
                         identity: PhoneIdentity(name: "Invented iPhone", app: "com.ferb.phone test"))
    }

    // decision: PHN-563, PHN-494
    func testStartSealsTheLinkStateSyncsAndDeliversChangesOnTheMainActor() async throws {
        let secrets = MemoryStore()
        let uplinks = FakeUplinks()
        let p = phone(secrets, uplinks)
        let changes = p.changes()
        let state = await p.start()
        XCTAssertEqual(state, .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: nil))
        XCTAssertNotNil(secrets.get("link.state"), "the link's first persist went to the Keychain")
        let start = try XCTUnwrap(uplinks.starts.first)
        XCTAssertNotNil(start.keys.device)
        XCTAssertNotNil(start.keys.lock)
        XCTAssertEqual(start.name, "Invented iPhone")

        let synced = await p.call { try $0.sync() }
        XCTAssertNotNil(synced.value)
        var heard = false
        for await c in changes {
            dispatchPrecondition(condition: .onQueue(.main))
            if case .transcript = c { heard = true; break }
        }
        XCTAssertTrue(heard)
        let view = await p.call { try $0.transcript(conversation: "c1") }
        XCTAssertEqual(view.value?.items.count, 1)

        let r = await p.unlock(p.signer(.lock, reason: "Unlock FERB"))
        XCTAssertNotNil(r.value, "\(String(describing: r.error))")
        XCTAssertTrue(uplinks.mac.calls.contains("link.unlock signed"), "\(uplinks.mac.calls)")
        await p.close()
        XCTAssertEqual(p.state, .closed)
        let after = await p.call { try $0.sync() }
        XCTAssertEqual(after.error?.code, "closed")
    }

    /// The rule the owner's freeze broke (kb PHN-680, PHN-682): the main thread never enters the phone library. Called
    /// from the main actor, as the app does: the last status, a sync, the unlock, and what the app tells the link,
    /// which the link hears in the order said. `MainGuard` stands at the library's door and names every call that
    /// reaches it on the main thread; there must be none.
    // decision: PHN-680, PHN-682
    func testTheMainActorNeverEntersThePhoneLibrary() async throws {
        let onMain = Named()
        MainGuard.onMain = { what in onMain.add(what) }
        defer { MainGuard.onMain = nil }
        let uplinks = FakeUplinks()
        let p = phone(MemoryStore(), uplinks)
        _ = await p.start()
        let none = await p.lastStatus()
        XCTAssertNil(none, "no status before the Mac said one")
        let synced = await p.call { try $0.sync() }
        XCTAssertNotNil(synced.value)
        _ = await p.unlock(p.signer(.lock, reason: "Unlock FERB"))
        p.setActive(false)
        p.setActive(true)
        p.setOpen(true)
        p.networkChanged("wifi")
        p.retry()
        let link = try XCTUnwrap(uplinks.made.last)
        let want = ["active=false", "active=true", "open=true", "network=wifi", "retry"]
        for _ in 0..<400 where link.said.count < want.count { try await Task.sleep(nanoseconds: 5_000_000) }
        XCTAssertEqual(link.said, want, "the link hears the app in the order said")
        await p.close()
        XCTAssertEqual(onMain.all, [], "the main thread entered the phone library")
    }

    // decision: PHN-563, PHN-561
    func testARestartKeepsThePairingAndARefusedStateWipesItSaid() async throws {
        let secrets = MemoryStore()
        let uplinks = FakeUplinks()
        let p = phone(secrets, uplinks)
        await p.start()
        let pubs = uplinks.starts[0].keys
        await p.close()

        let again = phone(secrets, uplinks)
        let s = await again.start()
        XCTAssertEqual(s.keys, KeyHealth(device: .ok, lock: .ok, strong: .ok))
        XCTAssertEqual(uplinks.starts[1].state, Data("{\"invented\":\"link state\"}".utf8), "the kept state started the link")
        XCTAssertEqual(uplinks.starts[1].keys, pubs, "the same keys")
        await again.close()

        uplinks.refuseState = true
        let third = phone(secrets, uplinks)
        let t = await third.start()
        guard case .open(_, let wiped) = t else { return XCTFail("\(t)") }
        XCTAssertEqual(wiped, .stateRefused)
        XCTAssertNotEqual(uplinks.starts.last?.keys, pubs, "new keys after the wipe")
        await third.close()
    }

    // decision: PHN-408, PHN-563
    func testPairingAndForgettingTheMac() async throws {
        let secrets = MemoryStore()
        let uplinks = FakeUplinks()
        let p = phone(secrets, uplinks)
        await p.start()
        let bad = await p.pair("FERB-bad")
        XCTAssertEqual(bad.error?.linkCode, "pair_secret_wrong")
        let good = await p.pair("FERB-good")
        XCTAssertNotNil(good.value)
        XCTAssertEqual(p.vault.registeredKeys(), uplinks.starts[0].keys, "what the Mac holds is recorded")
        let before = uplinks.starts[0].keys

        _ = await p.call { try $0.sync() }
        let storeKey = try p.vault.storeKey()
        let forgot = await p.forgetThisMac()
        XCTAssertNotNil(forgot.value)
        XCTAssertNotEqual(try p.vault.storeKey(), storeKey, "the fetched chat went with its store and key")
        XCTAssertNotEqual(uplinks.starts.last?.keys.lock, before.lock, "new signing keys for the next pairing")
        XCTAssertTrue(p.state.isOpen)
        await p.close()
    }
}
