import Foundation
import FerbMobile
import Observation
import UIKit

/// Where the phone is.
public enum PhoneState: Equatable, Sendable {
    case closed
    case starting
    /// The store and the link run. `keys`: which signing keys work now (`pairAgain`: the Mac can't be unlocked from
    /// here any more). `wiped`: the kept pairing was given up at this start; the app pairs again.
    case open(keys: KeyHealth, wiped: Lost?)
    /// The start failed (rare: the Keychain not readable yet, the store's folder can't be written).
    case failed(code: String, detail: String)

    public var keys: KeyHealth? {
        if case .open(let k, _) = self { return k }
        return nil
    }

    public var isOpen: Bool { keys != nil }
}

/// The names the Mac's phone list shows.
public struct PhoneIdentity: Sendable, Equatable {
    public let name: String
    public let app: String

    public init(name: String, app: String) {
        self.name = name; self.app = app
    }

    /// The phone's name as iOS gives it to apps (since iOS 16 the model's name, e.g. "iPhone", unless Apple grants
    /// the user-assigned-name entitlement), and the app's id and version.
    @MainActor public static func current() -> PhoneIdentity {
        let b = Bundle.main
        let version = b.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        return PhoneIdentity(name: UIDevice.current.name, app: "\(b.bundleIdentifier ?? "com.ferb.phone") \(version)")
    }
}

/// The time as the engine writes it: `2026-11-16T09:30:00+05:30` (an offset, never `Z`).
public final class SystemClockIso: Clock, @unchecked Sendable {
    public init() {}
    public func nowIso() -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = .current
        f.dateFormat = "yyyy-MM-dd'T'HH:mm:ssxxx"
        return f.string(from: Date())
    }
}

public final class RandomIds: Ids, @unchecked Sendable {
    public init() {}
    public func newId() -> String { UUID().uuidString.lowercased() }
}

/// The device key signs for the link with no prompt (`link.present`, PHN-493).
final class DeviceKeySigner: DeviceSigner, @unchecked Sendable {
    private let keys: PhoneKeys
    init(_ keys: PhoneKeys) { self.keys = keys }
    func sign(payload: String) -> String? { keys.deviceSign(payload) }
}

/// A signer that shows the system prompt for `kind` with `reason` (Face ID, Touch ID or the passcode) inside each
/// signature, on the library's calling thread. `signed` tells whether the owner passed it.
public final class KeySigner: Signer, @unchecked Sendable {
    public let kind: KeyKind
    private let keys: PhoneKeys
    private let reason: String
    private let lock = NSLock()
    private var _signed = false

    public init(keys: PhoneKeys, kind: KeyKind, reason: String) {
        self.keys = keys; self.kind = kind; self.reason = reason
    }

    public var signed: Bool { lock.lock(); defer { lock.unlock() }; return _signed }

    public func sign(payload: String) -> String? {
        let s = keys.sign(kind, payload, reason: reason)
        if s != nil { lock.lock(); _signed = true; lock.unlock() }
        return s
    }
}

/// The phone's one object for the app (kb PHN-498's twin, PHN-563): the encrypted store (ferb-mobile `Mobile`) and
/// the link (`Link`), started from the [Vault], with the library's callbacks delivered on the main actor. One per
/// process, owned by the app; [close] ends both. Every blocking call runs off the main thread.
@MainActor
@Observable
public final class FerbPhone {
    /// Where the phone is (on the main actor).
    public private(set) var state: PhoneState = .closed
    /// The link's status as it last said it (nil before it started).
    public private(set) var link: LinkStatus?

    @ObservationIgnored private var subscribers: [UUID: AsyncStream<Change>.Continuation] = [:]
    @ObservationIgnored private let engine: Engine
    public var vault: Vault { engine.vault }

    public init(vault: Vault, uplinks: UplinkFactory, identity: PhoneIdentity, clock: Clock = SystemClockIso(),
                ids: Ids = RandomIds(), utcOffsetS: @escaping @Sendable () -> Int32 = { Int32(TimeZone.current.secondsFromGMT()) }) {
        engine = Engine(vault: vault, uplinks: uplinks, identity: identity, clock: clock, ids: ids, utcOffsetS: utcOffsetS)
        engine.onChange = { [weak self] change in Task { @MainActor in self?.emit(change) } }
        engine.onLink = { [weak self] status in Task { @MainActor in self?.link = status } }
    }

    /// The real phone: the vault in Application Support and the Keychain, the real link, this phone's names.
    public static func create() -> FerbPhone {
        FerbPhone(vault: .forPhone(), uplinks: LinkUplinks(), identity: .current())
    }

    /// What changed in the core: read it again. Each caller gets its own stream; delivered on the main actor.
    public func changes() -> AsyncStream<Change> {
        let id = UUID()
        return AsyncStream(bufferingPolicy: .bufferingNewest(64)) { cont in
            subscribers[id] = cont
            cont.onTermination = { [weak self] _ in Task { @MainActor in self?.subscribers[id] = nil } }
        }
    }

    private func emit(_ change: Change) {
        for c in subscribers.values { c.yield(change) }
    }

    /// Opens the store and starts the link (the link's start takes up to 20 s).
    @discardableResult
    public func start() async -> PhoneState {
        if state.isOpen || state == .starting { return state }
        state = .starting
        let s = await engine.serial { $0.start() }
        state = s
        link = engine.linkStatus()
        return s
    }

    /// Runs `block` with the core off the main thread; the library's error comes back typed.
    public func call<T>(_ block: @escaping @Sendable (Mobile) throws -> T) async -> Outcome<T> {
        await engine.concurrent { e in
            guard let m = e.mobile else { return .failed(.Closed(detail: "FERB is not open.", linkCode: nil)) }
            do { return .ok(try block(m)) } catch let error as FerbError { return .failed(error) } catch {
                return .failed(.Internal(detail: "\(error)", linkCode: nil))
            }
        }
    }

    /// Pairs from a scanned or typed code. Keys iOS deleted are made anew first (and missing ones the phone can hold
    /// now, such as the lock key after a passcode was set), and the link restarts to carry them.
    public func pair(_ code: String) async -> Outcome<Void> {
        let r = await engine.serial { $0.pair(code) }
        publishOpen()
        return r
    }

    /// PHN-125: the signed unlock the app sends on open: one prompt with the lock key.
    public func unlock(_ signer: Signer) async -> Outcome<Void> {
        let r = await call { try $0.unlock(signer: signer) }
        publishOpen()
        return r
    }

    /// PHN-15: the app locked. The Mac is told (its window for this phone ends) and the conversation's watch stops
    /// until the next unlock. ok(false): offline.
    public func lock() async -> Outcome<Bool> { await call { try $0.lock() } }

    /// The app opened by the prompt alone (offline): the conversation is read again once the Mac admits it.
    public func unlocked() async -> Outcome<Void> { await call { try $0.unlocked() } }

    /// "Stop everything on the Mac" (`stop.engage`): works while locked (PHN-125), never queued, no prompt.
    public func stopEverything() async -> Outcome<Void> { await call { try $0.stopEverything() } }

    /// Keys this phone holds that the Mac was never given (a Face ID or Touch ID enrolled after a pairing made
    /// without one); nil when there is nothing to register. A strong key iOS made unusable is replaced only by
    /// pairing again (PHN-133).
    public func unregisteredKeys() async -> PublicKeys? { await engine.serial { $0.unregisteredKeys() } }

    /// Registers [unregisteredKeys] with the Mac (`link.keys`), signed by the strong key where the Mac holds one,
    /// else the lock key. ok(false): nothing to do.
    public func registerKeys(_ signer: @escaping @Sendable (KeyKind) -> Signer) async -> Outcome<Bool> {
        guard let fresh = await unregisteredKeys(), let mac = engine.vault.registeredKeys() else { return .ok(false) }
        let kind: KeyKind = (mac.strong != nil && engine.vault.keys.health().strong == .ok) ? .strong : .lock
        let s = signer(kind)
        let r = await call { try $0.registerKeys(devicePub: fresh.device, strongPub: fresh.strong, signer: s) }
        switch r {
        case .ok:
            engine.vault.saveRegisteredKeys(PublicKeys(device: mac.device ?? fresh.device, lock: mac.lock, strong: mac.strong ?? fresh.strong))
            publishOpen()
            return .ok(true)
        case .failed(let e):
            return .failed(e)
        }
    }

    /// The last status the Mac pushed or a read kept (memory only; safe on the main thread), or nil.
    public func lastStatusNow() -> PhoneStatus? { (try? engine.mobile?.lastStatus()) ?? nil }

    /// A prompt signer for `kind`: Face ID, Touch ID or the passcode (lock), Face ID or Touch ID only (strong).
    public func signer(_ kind: KeyKind, reason: String) -> KeySigner {
        KeySigner(keys: engine.vault.keys, kind: kind, reason: reason)
    }

    /// "Forget this Mac" (PHN-408): the fetched chat and its key, the Mac in the link's state and the three signing
    /// keys go; the store and the link start again empty, ready to pair.
    public func forgetThisMac() async -> Outcome<Void> {
        let r = await engine.serial { $0.forget() }
        if case .failed(let e) = r { state = .failed(code: e.code, detail: e.detail) } else { publishOpen() }
        link = engine.linkStatus()
        return r
    }

    /// The app came to the front or went to the back. Non-blocking.
    public func setActive(_ on: Bool) { engine.uplink?.setActive(on) }

    /// The phone's network changed (`wifi`, `cellular`, `none`, …). Non-blocking.
    public func networkChanged(_ kind: String?) { engine.uplink?.networkChanged(kind) }

    public func retry() { engine.uplink?.retry() }

    /// Ends the store and the link (bounded: about 5 s for the link).
    public func close() async {
        await engine.serial { $0.close() }
        state = .closed
        for c in subscribers.values { c.finish() }
        subscribers = [:]
    }

    /// Reads the keys' health again (a passcode or Face ID changed in Settings meanwhile).
    public func publishOpen() {
        guard engine.mobile != nil, state != .closed else { return }
        state = .open(keys: engine.vault.keys.health(), wiped: engine.wiped)
    }
}

/// The blocking side: owns the store and the link; every method runs on its queues, never on the main thread.
final class Engine: @unchecked Sendable {
    let vault: Vault
    private let uplinks: UplinkFactory
    private let identity: PhoneIdentity
    private let clock: Clock
    private let ids: Ids
    private let utcOffsetS: @Sendable () -> Int32
    private let serialQueue = DispatchQueue(label: "com.ferb.phone.serial", qos: .userInitiated)
    private let callQueue = DispatchQueue(label: "com.ferb.phone.calls", qos: .userInitiated, attributes: .concurrent)
    private let lock = NSLock()
    private var _mobile: Mobile?
    private var _uplink: Uplink?
    private var pubs: PublicKeys?
    private(set) var wiped: Lost?
    var onChange: (@Sendable (Change) -> Void)?
    var onLink: (@Sendable (LinkStatus) -> Void)?

    var mobile: Mobile? { lock.lock(); defer { lock.unlock() }; return _mobile }
    var uplink: Uplink? { lock.lock(); defer { lock.unlock() }; return _uplink }

    init(vault: Vault, uplinks: UplinkFactory, identity: PhoneIdentity, clock: Clock, ids: Ids, utcOffsetS: @escaping @Sendable () -> Int32) {
        self.vault = vault
        self.uplinks = uplinks
        self.identity = identity
        self.clock = clock
        self.ids = ids
        self.utcOffsetS = utcOffsetS
    }

    func serial<T>(_ work: @escaping (Engine) -> T) async -> T {
        await withCheckedContinuation { cont in serialQueue.async { cont.resume(returning: work(self)) } }
    }

    func concurrent<T>(_ work: @escaping (Engine) -> T) async -> T {
        await withCheckedContinuation { cont in callQueue.async { cont.resume(returning: work(self)) } }
    }

    func linkStatus() -> LinkStatus? { uplink?.status() }

    private lazy var heard: Listener = Heard(self)
    private lazy var linkHeard: LinkListener = LinkHeard(self)
    private lazy var deviceSigner = DeviceKeySigner(vault.keys)

    fileprivate func changed(_ c: Change) { onChange?(c) }
    fileprivate func statusChanged(_ s: LinkStatus) { onLink?(s) }
    fileprivate func persist(_ state: Data) {
        // on the link's thread, at once: the state holds the phone's link key
        do { try vault.saveLinkState(state) } catch { NSLog("ferb-link: the link state could not be kept: \(error)") }
    }

    func start() -> PhoneState {
        if mobile != nil { return .open(keys: vault.keys.health(), wiped: wiped) }
        do {
            try bringUp()
            return .open(keys: vault.keys.health(), wiped: wiped)
        } catch let e as FerbError {
            NSLog("ferb: the start failed: %@ (%@)", e.code, e.detail)
            return .failed(code: e.code, detail: e.detail)
        } catch is VaultNotReady {
            NSLog("ferb: the start waits: the Keychain can't be read before the phone's first unlock")
            return .failed(code: "not_ready", detail: "Unlock this iPhone, then open FERB again.")
        } catch {
            NSLog("ferb: the start failed: %@", "\(error)")
            return .failed(code: "internal", detail: "\(error)")
        }
    }

    func pair(_ code: String) -> Outcome<Void> {
        do {
            let fresh = try vault.keys.renew()
            if fresh != pubs { try restartLink(fresh) }
            guard let u = uplink else { return .failed(.Closed(detail: "FERB is not open.", linkCode: nil)) }
            try u.pair(code)
            // the keys this pairing's hello carries: what the Mac will hold (registerKeys adds any made later)
            vault.saveRegisteredKeys(fresh)
            wiped = nil
            return .ok(())
        } catch let e as FerbError {
            return .failed(e)
        } catch {
            return .failed(.Internal(detail: "\(error)", linkCode: nil))
        }
    }

    func unregisteredKeys() -> PublicKeys? {
        guard mobile != nil, let mac = vault.registeredKeys(), let now = try? vault.keys.ensure() else { return nil }
        let device = mac.device == nil ? now.device : nil
        let strong = mac.strong == nil ? now.strong : nil
        return device == nil && strong == nil ? nil : PublicKeys(device: device, lock: nil, strong: strong)
    }

    func forget() -> Outcome<Void> {
        do {
            if let m = mobile {
                try? m.forgetThisMac()
                m.shutdown()
            }
            if let u = uplink {
                try? u.forget()   // the Mac-less state reaches persist before the link closes
                u.shutdown()
            }
            lock.lock(); _mobile = nil; _uplink = nil; lock.unlock()
            vault.forget()
            wiped = nil
            try bringUp()
            return .ok(())
        } catch let e as FerbError {
            return .failed(e)
        } catch {
            return .failed(.Internal(detail: "\(error)", linkCode: nil))
        }
    }

    func close() {
        lock.lock()
        let m = _mobile, u = _uplink
        _mobile = nil; _uplink = nil
        lock.unlock()
        m?.shutdown()
        u?.shutdown()
    }

    /// The link first (its kept state decides whether anything else is kept), then the store.
    private func bringUp() throws {
        var state: Data?
        do {
            state = try vault.linkState()
        } catch let lost as VaultLost {
            vault.wipe()
            wiped = lost.why
            state = nil
        }
        // a first start: keys left by an earlier install are never reused
        if state == nil { vault.keys.deleteAll() }
        var keys = try vault.keys.ensure()
        let u: Uplink
        do {
            u = try startLink(state, keys)
        } catch FerbError.BadArgument {
            vault.wipe()
            wiped = .stateRefused
            state = nil
            keys = try vault.keys.ensure()
            u = try startLink(nil, keys)
        }
        lock.lock(); _uplink = u; lock.unlock()
        pubs = keys
        // a phone paired before the record of what the Mac holds existed: what it holds now is what it was given
        if state != nil, vault.registeredKeys() == nil { vault.saveRegisteredKeys(keys) }
        let m = try openStore()
        lock.lock(); _mobile = m; lock.unlock()
        try u.attach(m)
        if let s = u.status() { onLink?(s) }
    }

    private func startLink(_ state: Data?, _ keys: PublicKeys) throws -> Uplink {
        try uplinks.start(UplinkStart(state: state, name: identity.name, app: identity.app, keys: keys, listener: linkHeard,
                                      deviceSigner: deviceSigner))
    }

    private func restartLink(_ keys: PublicKeys) throws {
        uplink?.shutdown()
        lock.lock(); _uplink = nil; lock.unlock()
        let u = try startLink(try vault.linkState(), keys)
        lock.lock(); _uplink = u; lock.unlock()
        pubs = keys
        if let m = mobile { try u.attach(m) }
        if let s = u.status() { onLink?(s) }
    }

    /// The store is a copy of the Mac's (ENG-10): one it can't open is dropped and made again, once.
    private func openStore() throws -> Mobile {
        let config = MobileConfig(clientId: vault.clientId(), utcOffsetS: utcOffsetS(), pageLimit: nil, maxBytes: nil, pieceChars: nil)
        func open() throws -> Mobile {
            try Mobile.open(path: vault.storeFile.path, key: try vault.storeKey(), config: config, clock: clock, ids: ids, listener: heard)
        }
        do {
            return try open()
        } catch let e as FerbError {
            switch e {
            case .WrongKey, .BadKey, .StoreNewer, .Store:
                NSLog("ferb: the store could not be opened (\(e.code)); it is made again")
                vault.dropStore()
                return try open()
            default:
                throw e
            }
        }
    }
}

private final class Heard: Listener, @unchecked Sendable {
    private weak var engine: Engine?
    init(_ e: Engine) { engine = e }
    func changed(change: Change) { engine?.changed(change) }
}

private final class LinkHeard: LinkListener, @unchecked Sendable {
    private weak var engine: Engine?
    init(_ e: Engine) { engine = e }
    func statusChanged(status: LinkStatus) { engine?.statusChanged(status) }
    func persist(state: Data) { engine?.persist(state) }
}
