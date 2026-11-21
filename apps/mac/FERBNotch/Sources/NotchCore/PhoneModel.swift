import Combine
import Foundation

// Settings → Phone (stage 3 slice S5, kb T-1205): pairing a phone, the paired phones, and the owner's own relay.
// Two sides answer it. FERB's daemon holds the paired phones, the owner's Yes to a pairing and what phones did
// (`phone.list`, `phone.pair_decide`, `phone.rename`, `phone.remove`, `phone.audit`; bus events `phone.pair_requested`,
// `phone.unused`, `phone.removed`; journal 106). The phone link's helper (S4) draws the pairing code, knows how each
// phone is connected right now and keeps the relay setting (`link.*`, as the lab's admin socket speaks them). Until S4
// hands FERB.app a way to reach the helper, `PhoneService.hasLink` is false and the page says so.
//
// The relay token is a secret (rule 7): it lives in this model's memory only while the owner types it, goes only to the
// helper (which keeps it in the Keychain, PHN-143), and is never logged, stored in preferences or shown back.

// MARK: - What the two sides answer

/// How a phone is connected right now (the helper's `tier`).
public enum PhoneConnection: String, Equatable {
    case direct
    case ownRelay = "own_relay"
    case sharedRelay = "shared_relay"
    case none

    public var words: String {
        switch self {
        case .direct: return PhoneWords.connectedDirect
        case .ownRelay: return PhoneWords.connectedOwnRelay
        case .sharedRelay: return PhoneWords.connectedSharedRelay
        case .none: return PhoneWords.notConnected
        }
    }
}

/// One paired phone (`phone.list`), with its connection when the helper said.
public struct PhoneEntry: Equatable, Identifiable {
    public let id: String
    public var name: String
    public let app: String
    public let pairedAt: Date?
    public let lastSeen: Date?
    /// Unused for the warning's days (PHN-419): said on its row until it is seen again; never removed by itself.
    public let unusedWarning: Bool
    public var connection: PhoneConnection?

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        name = (value["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? PhoneWords.unnamedPhone
        app = value["app"] as? String ?? ""
        pairedAt = WindowTime.parse(value["paired_at"])
        lastSeen = WindowTime.parse(value["last_seen"])
        unusedWarning = value["unused_warning"] as? Bool ?? false
    }
}

/// A phone that proved it saw the code and waits for the owner's Yes, with the six digits both screens show.
public struct PairRequest: Equatable {
    public let requestID: String?
    public let phone: String
    public let name: String
    public let code: String

    init?(_ value: Any?) {
        guard let value = value as? [String: Any], let phone = value["phone"] as? String, !phone.isEmpty else { return nil }
        requestID = value["request_id"] as? String
        self.phone = phone
        name = (value["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? PhoneWords.unnamedPhone
        code = value["code"] as? String ?? ""
    }

    private init(requestID: String?, phone: String, name: String, code: String) {
        self.requestID = requestID; self.phone = phone; self.name = name; self.code = code
    }

    /// Six digits as "123 456", or nothing: a code the bus masked is never shown as if it were the code.
    var withoutMaskedCode: PairRequest {
        let digits = code.filter(\.isNumber)
        return digits.count == 6 ? self : PairRequest(requestID: requestID, phone: phone, name: name, code: "")
    }
}

/// `phone.list`.
public struct PhoneList: Equatable {
    public let phones: [PhoneEntry]
    public let pairing: PairRequest?
    /// Seconds left for the pairing's Yes, as the daemon counts them.
    public let pairingSecondsLeft: Double?
    public let max: Int
    public let unusedDays: Int
    /// Where files sent from a phone land (PHN-580); nil from a daemon that doesn't say.
    public let uploadFolder: UploadFolder?

    public init(_ reply: [String: Any]) {
        phones = (reply["phones"] as? [[String: Any]] ?? []).compactMap(PhoneEntry.init)
        pairing = PairRequest(reply["pairing"])
        pairingSecondsLeft = ((reply["pairing"] as? [String: Any])?["seconds_left"] as? NSNumber)?.doubleValue
        max = (reply["max"] as? NSNumber)?.intValue ?? 5
        unusedDays = (reply["unused_days"] as? NSNumber)?.intValue ?? 30
        uploadFolder = UploadFolder(reply["upload_folder"])
    }
}

/// PHN-580, PHN-581: the folder files sent from a phone land in (`phone.list` / `phone.upload_folder`).
public struct UploadFolder: Equatable {
    public let path: String
    /// Said plainly: "Downloads › FERB from phone".
    public let shown: String
    public let isDefault: Bool

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let path = value["path"] as? String, !path.isEmpty else { return nil }
        self.path = path
        shown = (value["shown"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? path
        isDefault = value["default"] as? Bool ?? true
    }
}

/// GATE-615: "until I'm back at my Mac" as the daemon has it (`screen_use.status`).
public struct ScreenUse: Equatable {
    public let active: Bool
    public let phoneName: String?
    public let since: Date?

    public init(_ reply: [String: Any]) {
        active = reply["active"] as? Bool ?? false
        phoneName = (reply["phone_name"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        since = WindowTime.parse(reply["since"])
    }
}

/// GATE-618 (owner: "make sure user is shon ferb acknowledgeds them coming back, shodlnt seem it was a spam message"):
/// the quiet "Are you back?" line while "until I'm back" holds screen use. It shows in the chat with no sound and is
/// nothing the phone sees; when the owner is back it turns into the welcome line in place; a hold that passes in
/// silence (FERB's own echo) takes it away with nothing further.
public enum ScreenBackLine {
    static func id(_ data: [String: Any]) -> String {
        "screen-back-\(data["since"] as? String ?? "")-\(data["hold"] as? NSNumber ?? 0)"
    }

    /// Applies one `screen.until_back` event to FERB's alerts above the composer (APP-1107: never kept in the chat):
    /// true when they changed.
    public static func apply(_ data: [String: Any], to alerts: inout [OwnerAlert]) -> Bool {
        let id = id(data)
        switch data["state"] as? String {
        case "checking":
            let title = data["title"] as? String ?? "Are you back?"
            let line = data["line"] as? String ?? ""
            return OwnerAlerts.show(OwnerAlert(id: id, text: line.isEmpty ? title : title + " " + line, fromDaemon: false),
                                    in: &alerts)
        case "resumed":
            let before = alerts.count
            alerts.removeAll { $0.id == id }
            return alerts.count != before
        case "ended":
            // the daemon leaves this one to FERB.app: in place of the question, or (none shown here) as a new line
            guard data["during_hold"] as? Bool == true, let line = data["line"] as? String else { return false }
            return OwnerAlerts.show(OwnerAlert(id: id, text: line, fromDaemon: false), in: &alerts)
        default:
            return false
        }
    }
}

/// PHN-220, PHN-591: phones that haven't connected since the relay setting changed (`link.status` `relay_handover`).
public struct RelayHandover: Equatable {
    public let phonesWaiting: Int
    /// When the shared relays stop being kept open for them.
    public let closesAt: Date?
    /// Past that, with phones still waiting: they can't find this Mac until reconnected or paired again.
    public let closed: Bool

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any] else { return nil }
        phonesWaiting = (value["phones_waiting"] as? NSNumber)?.intValue ?? 0
        closesAt = (value["closes_at"] as? NSNumber).map { Date(timeIntervalSince1970: $0.doubleValue) }
        closed = value["closed"] as? Bool ?? false
    }
}

/// One line of what phones did (`phone.audit`): what, when, which phone. Never a value: the daemon's structural facts
/// (`detail`) are not read here at all.
public struct PhoneAuditRow: Equatable, Identifiable {
    public let at: Date?
    /// The phone's id; empty for what happened on this Mac (a pairing answered, a phone removed).
    public let phone: String
    public let method: String
    public let outcome: String
    public let index: Int
    public var id: Int { index }

    /// "Pixel 8 sent a message", in plain words; a refused call says so.
    public func line(names: [String: String]) -> String {
        let who = phone.isEmpty ? PhoneWords.thisMac : (names[phone] ?? PhoneWords.aRemovedPhone)
        let what = PhoneWords.did(method, fromMac: phone.isEmpty)
        switch outcome {
        case "ok": return "\(who) \(what)"
        case "denied": return phone.isEmpty ? "\(who) \(PhoneWords.saidNoToPairing)" : "\(who) \(what) (\(PhoneWords.refused))"
        default: return "\(who) \(what) (\(PhoneWords.refused))"
        }
    }
}

/// What `link.pair_start` gives: the code drawn as PHN-142 says (`rows`, `1` dark) and the text to type instead.
public struct PairingInvite: Equatable {
    public let code: String
    public let rows: [String]
    public let seconds: Double

    public init?(_ reply: [String: Any]) {
        let code = (reply["code"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? (reply["url"] as? String ?? "")
        guard !code.isEmpty else { return nil }
        self.code = code
        rows = reply["qr"] as? [String] ?? []
        seconds = (reply["seconds"] as? NSNumber)?.doubleValue ?? 120
    }

    public init(code: String, rows: [String], seconds: Double) {
        self.code = code
        self.rows = rows
        self.seconds = seconds
    }
}

/// The relay as the helper uses it now (`link.status` `relay`).
public enum RelayNow: Equatable {
    case shared(connected: Bool)
    case own
    case ownDown(why: String?)
    case ownFallback(why: String?)
}

/// `link.status` as this page reads it.
public struct LinkStatus: Equatable {
    public let relay: RelayNow
    /// The saved relay's address (the helper never sends its token back), nil when none is set.
    public let ownRelay: String?
    public let alwaysUseRelay: Bool
    public let connections: [String: PhoneConnection]
    public let handover: RelayHandover?

    public init(_ reply: [String: Any], always: Bool? = nil) {
        handover = RelayHandover(reply["relay_handover"])
        let relay = reply["relay"] as? [String: Any] ?? [:]
        let why = (relay["error"] as? [String: Any]).flatMap { PhoneWords.linkMessage($0["message"] as? String, allow: $0["allow"] as? String) }
        switch relay["state"] as? String {
        case "own": self.relay = .own
        case "own_down": self.relay = .ownDown(why: why)
        case "own_fallback": self.relay = .ownFallback(why: why)
        default: self.relay = .shared(connected: relay["connected"] as? Bool ?? false)
        }
        ownRelay = ((reply["own_relay"] as? [String: Any])?["url"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        alwaysUseRelay = always ?? (reply["own_relay_always"] as? Bool ?? false)
        var connections: [String: PhoneConnection] = [:]
        for device in reply["devices"] as? [[String: Any]] ?? [] {
            guard let id = device["id"] as? String else { continue }
            let online = device["online"] as? Bool ?? false
            connections[id] = online ? (PhoneConnection(rawValue: device["tier"] as? String ?? "") ?? PhoneConnection.none) : PhoneConnection.none
        }
        self.connections = connections
    }

    /// The one status line (PHN-418, PHN-425, PHN-453): direct preferred; your relay otherwise; the shared relay only as
    /// a fallback, with text only.
    public var line: String {
        switch relay {
        case .shared(true): return PhoneWords.statusShared
        case .shared(false): return PhoneWords.statusNoRelayYet
        case .own: return alwaysUseRelay ? PhoneWords.statusOwnAlways : PhoneWords.statusOwn
        case .ownDown(let why): return PhoneWords.statusOwnDown + (why.map { " " + $0 } ?? "")
        case .ownFallback(let why): return PhoneWords.statusOwnFallback + (why.map { " " + $0 } ?? "")
        }
    }
}

/// What the owner typed for a relay: the one line the relay's setup printed (`ferbrelay://…`, which carries the token),
/// or an https address and its token. Its description never shows either secret.
public struct RelayEntry: Equatable, CustomStringConvertible, CustomDebugStringConvertible {
    public let url: String
    public let token: String?

    public init(link: String, address: String, token: String) {
        let joined = link.components(separatedBy: .whitespacesAndNewlines).joined()
        if !joined.isEmpty {
            url = joined
            self.token = nil
        } else {
            url = address.trimmingCharacters(in: .whitespacesAndNewlines)
            let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
            self.token = token.isEmpty ? nil : token
        }
    }

    public var isEmpty: Bool { url.isEmpty }
    var params: [String: Any] { token.map { ["url": url, "token": $0] } ?? ["url": url] }
    /// The parts that must never be shown or logged: a token, and a pasted link (its query holds the token).
    var secrets: [String] {
        guard url.hasPrefix("ferbrelay://") else { return [token].compactMap { $0 } }
        let tok = URLComponents(string: url)?.queryItems?.first { $0.name == "tok" }?.value
        return [token, url, tok].compactMap { $0 }.filter { !$0.isEmpty }
    }
    public var description: String { "RelayEntry(hidden)" }
    public var debugDescription: String { description }
}

/// What a relay Test or save said: each check that passed and each note (`link.relay_test`).
public struct RelayCheck: Equatable {
    public let steps: [String]
    public let warnings: [String]

    public init(_ reply: [String: Any]) {
        let test = reply["test"] as? [String: Any] ?? reply
        steps = test["steps"] as? [String] ?? []
        warnings = test["warnings"] as? [String] ?? []
    }
}

// MARK: - The seam

public enum PhoneServiceError: LocalizedError, Equatable {
    /// This FERB.app has no way to reach the phone link's helper (S4 provides it).
    case noLink

    public var errorDescription: String? { PhoneWords.noLinkInThisVersion }
}

/// Everything Settings → Phone asks. The daemon's half exists (journal 106); the link's half is the helper's admin
/// methods (`engine/crates/ferb-link/src/admin.rs`), reached once S4 gives FERB.app the helper's socket.
@MainActor
public protocol PhoneService: AnyObject {
    // FERB's daemon
    func list() async throws -> PhoneList
    func rename(id: String, name: String) async throws -> String
    func remove(id: String) async throws
    /// The owner's answer to the phone waiting (`phone.pair_decide`): whether it is paired now.
    func decide(requestID: String?, yes: Bool) async throws -> Bool
    func audit(limit: Int) async throws -> [PhoneAuditRow]
    // the phone link's helper
    var hasLink: Bool { get }
    func linkStatus() async throws -> LinkStatus
    func startPairing() async throws -> PairingInvite
    func cancelPairing() async throws
    func testRelay(_ entry: RelayEntry) async throws -> RelayCheck
    /// Tested by the helper first; saved only when the test passes (PHN-205).
    func useRelay(_ entry: RelayEntry) async throws -> RelayCheck
    func stopUsingRelay() async throws
    func setAlwaysUseRelay(_ on: Bool) async throws
    // PHN-581: where a phone's files land (nil: back to Downloads › FERB from phone)
    func setUploadFolder(_ path: String?) async throws -> UploadFolder?
    // GATE-615: "until I'm back at my Mac"
    func screenUse() async throws -> ScreenUse
    func endScreenUse() async throws
}

public extension PhoneService {
    func setUploadFolder(_ path: String?) async throws -> UploadFolder? { nil }
    func screenUse() async throws -> ScreenUse { ScreenUse([:]) }
    func endScreenUse() async throws {}
}

/// The service over FERB's daemon. The link's helper is reached only through the daemon (S4, kb PHN-441: the daemon
/// relays `phone.link_status`, `phone.pair_start`, `phone.pair_cancel`, `phone.relay_test`, `phone.relay_set` and
/// `phone.relay_always` to it); `link` is that same transport, nil in a FERB.app with no phone link.
@MainActor
public final class RPCPhoneService: PhoneService {
    private let daemon: any DaemonRPCTransport
    private let link: (any DaemonRPCTransport)?

    public init(daemon: any DaemonRPCTransport, link: (any DaemonRPCTransport)?) {
        self.daemon = daemon
        self.link = link
    }

    public var hasLink: Bool { link != nil }

    public func list() async throws -> PhoneList { PhoneList(try await daemon.call("phone.list", params: [:])) }

    public func rename(id: String, name: String) async throws -> String {
        try await daemon.call("phone.rename", params: ["id": id, "name": name])["name"] as? String ?? name
    }

    public func remove(id: String) async throws { _ = try await daemon.call("phone.remove", params: ["id": id]) }

    public func decide(requestID: String?, yes: Bool) async throws -> Bool {
        var params: [String: Any] = ["yes": yes]
        if let requestID { params["request_id"] = requestID }
        return try await daemon.call("phone.pair_decide", params: params)["paired"] as? Bool ?? false
    }

    public func audit(limit: Int) async throws -> [PhoneAuditRow] {
        let rows = try await daemon.call("phone.audit", params: ["limit": limit])["rows"] as? [[String: Any]] ?? []
        return rows.enumerated().map { index, row in
            PhoneAuditRow(at: WindowTime.parse(row["at"]), phone: row["phone"] as? String ?? "",
                          method: row["method"] as? String ?? "", outcome: row["outcome"] as? String ?? "", index: index)
        }
    }

    private func helper(_ method: String, _ params: [String: Any] = [:]) async throws -> [String: Any] {
        guard let link else { throw PhoneServiceError.noLink }
        return try await link.call(method, params: params, wait: 30)
    }

    public func linkStatus() async throws -> LinkStatus { LinkStatus(try await helper("phone.link_status")) }

    public func startPairing() async throws -> PairingInvite {
        guard let invite = PairingInvite(try await helper("phone.pair_start")) else { throw DaemonRPCError.invalidResponse }
        return invite
    }

    public func cancelPairing() async throws { _ = try await helper("phone.pair_cancel") }
    public func testRelay(_ entry: RelayEntry) async throws -> RelayCheck { RelayCheck(try await helper("phone.relay_test", entry.params)) }
    public func useRelay(_ entry: RelayEntry) async throws -> RelayCheck { RelayCheck(try await helper("phone.relay_set", entry.params)) }
    public func stopUsingRelay() async throws { _ = try await helper("phone.relay_set", ["off": true]) }
    public func setAlwaysUseRelay(_ on: Bool) async throws { _ = try await helper("phone.relay_always", ["on": on]) }

    public func setUploadFolder(_ path: String?) async throws -> UploadFolder? {
        let params: [String: Any] = path.map { ["path": $0] } ?? ["reset": true]
        return UploadFolder(try await daemon.call("phone.upload_folder", params: params)["upload_folder"])
    }

    public func screenUse() async throws -> ScreenUse { ScreenUse(try await daemon.call("screen_use.status", params: [:])) }
    public func endScreenUse() async throws { _ = try await daemon.call("screen_use.end", params: [:]) }
}

// MARK: - The page's model

@MainActor
public final class PhoneSettingsModel: ObservableObject {
    public enum Pairing: Equatable {
        case idle
        case starting
        /// The code is shown until `until` (the helper's pairing window, 120 s).
        case showing(PairingInvite, until: Date)
        /// A phone waits for the owner's Yes until `until` (the daemon's 120 s).
        case asking(PairRequest, until: Date)
        case ended(String)
    }

    public enum RelayResult: Equatable {
        case reachable([String])
        case saved([String])
        case unreachable(String)
    }

    @Published public private(set) var phones: [PhoneEntry] = []
    @Published public private(set) var max = 5
    @Published public private(set) var unusedDays = 30
    @Published public private(set) var uploadFolder: UploadFolder?
    @Published public private(set) var screenUse: ScreenUse?
    @Published public private(set) var pairing: Pairing = .idle
    @Published public private(set) var activity: [PhoneAuditRow] = []
    @Published public private(set) var link: LinkStatus?
    /// The daemon didn't answer, or refused: in its words.
    @Published public private(set) var problem: String?
    /// The link's helper isn't in this FERB.app or didn't answer.
    @Published public private(set) var linkProblem: String?
    @Published public private(set) var note: String?
    @Published public private(set) var loaded = false
    @Published public private(set) var clock: Date
    // The relay the owner is typing: in memory only (rule 7), cleared once saved.
    @Published public var relayLink = ""
    @Published public var relayAddress = ""
    @Published public var relayToken = ""
    @Published public private(set) var relayResult: RelayResult?
    @Published public private(set) var relayBusy = false

    public let service: any PhoneService
    private let now: () -> Date
    private var ticks = 0
    /// The daemon's own wait for the owner's Yes when an event names no time (Limits::pair_confirm_s, 120 s).
    static let answerWindow: TimeInterval = 120

    public init(service: any PhoneService, now: @escaping () -> Date = Date.init) {
        self.service = service
        self.now = now
        clock = now()
    }

    public var hasLink: Bool { service.hasLink }
    public var isFull: Bool { phones.count >= max }
    public var names: [String: String] { Dictionary(phones.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a }) }

    /// Whole seconds left in the pairing shown now.
    public var secondsLeft: Int {
        switch pairing {
        case let .showing(_, until), let .asking(_, until): return Swift.max(0, Int((until.timeIntervalSince(clock)).rounded(.up)))
        default: return 0
        }
    }

    // MARK: Reading

    public func load() async {
        await loadScreenUse()
        await loadPhones()
        await loadActivity()
        await loadLink()
        loaded = true
    }

    func loadPhones() async {
        do {
            apply(try await service.list())
            problem = nil
        } catch {
            problem = PhoneWords.daemonProblem(error)
        }
    }

    func loadActivity() async {
        if let rows = try? await service.audit(limit: 30) { activity = rows.reversed() }
    }

    func loadLink() async {
        guard service.hasLink else {
            link = nil
            linkProblem = PhoneWords.noLinkInThisVersion
            return
        }
        do {
            link = try await service.linkStatus()
            linkProblem = nil
            mergeConnections()
        } catch {
            link = nil
            linkProblem = PhoneWords.linkProblem(error)
        }
    }

    private func apply(_ list: PhoneList) {
        phones = list.phones
        max = list.max
        unusedDays = list.unusedDays
        uploadFolder = list.uploadFolder
        mergeConnections()
        let current = now()
        switch pairing {
        case .idle, .starting, .showing, .ended:
            if let request = list.pairing {
                pairing = .asking(request, until: current.addingTimeInterval(list.pairingSecondsLeft ?? Self.answerWindow))
            }
        case .asking(let waiting, _):
            // the daemon no longer holds it (the phone left, another took its place, its time ran out)
            if list.pairing == nil { pairing = .ended(PhoneWords.requestEnded) }
            else if let request = list.pairing, request.requestID != waiting.requestID || request.code != waiting.code {
                // another request, or the same one whose digits the app didn't have yet
                pairing = .asking(request, until: current.addingTimeInterval(list.pairingSecondsLeft ?? Self.answerWindow))
            }
        }
    }

    private func mergeConnections() {
        guard let link else { return }
        phones = phones.map { var phone = $0; phone.connection = link.connections[phone.id] ?? PhoneConnection.none; return phone }
    }

    // MARK: Pairing

    /// Shows a code for one phone (the helper's pairing window). A sixth phone is refused before anything opens.
    public func startPairing() async {
        note = nil
        guard !isFull else { pairing = .ended(PhoneWords.full(max)); return }
        guard service.hasLink else { pairing = .ended(PhoneWords.noLinkInThisVersion); return }
        pairing = .starting
        do {
            let invite = try await service.startPairing()
            ticks = 0
            clock = now()
            pairing = .showing(invite, until: clock.addingTimeInterval(invite.seconds))
        } catch {
            pairing = .ended(PhoneWords.linkProblem(error))
        }
    }

    public func cancelPairing() async {
        if case .asking = pairing { _ = try? await service.decide(requestID: request?.requestID, yes: false) }
        pairing = .idle
        if service.hasLink { try? await service.cancelPairing() }
    }

    var request: PairRequest? { if case let .asking(request, _) = pairing { return request } else { return nil } }

    /// The owner's Yes or No on this Mac (`phone.pair_decide`). Never answered by anything else.
    public func decide(yes: Bool) async {
        guard let request else { return }
        do {
            let paired = try await service.decide(requestID: request.requestID, yes: yes)
            pairing = .ended(paired ? PhoneWords.paired(request.name) : PhoneWords.notPaired)
            if !paired, service.hasLink { try? await service.cancelPairing() }
        } catch let error as DaemonRPCError where error.code == PhoneCodes.deviceLimit {
            pairing = .ended(PhoneWords.full(max))
        } catch let error as DaemonRPCError where error.code == PhoneCodes.notFound {
            pairing = .ended(PhoneWords.requestEnded)
        } catch {
            problem = PhoneWords.daemonProblem(error)
        }
        await loadPhones()
        await loadActivity()
    }

    /// Once a second while the page shows: the countdown, the code's end, and (every other second while a pairing is
    /// open) the daemon's list, so a phone's request shows even when no event reaches this app.
    public func tick() async {
        clock = now()
        switch pairing {
        case let .showing(_, until):
            if clock >= until {
                pairing = .ended(PhoneWords.expired)
                if service.hasLink { try? await service.cancelPairing() }
                return
            }
        case let .asking(_, until):
            if clock >= until { pairing = .ended(PhoneWords.noAnswer); return }
        default: return
        }
        ticks += 1
        if ticks % 2 == 0 { await loadPhones() }
    }

    /// A bus event of the phone area (journal 106), forwarded by the notch's connection.
    public func receive(topic: String, data: [String: Any]) async {
        switch topic {
        case "phone.pair_requested":
            guard let request = PairRequest(data) else { return }
            clock = now()
            // The bus masks the six digits like any one-time code (every event is redacted once for all its
            // consumers), so the event only says a phone is waiting: the digits come from FERB.app's own phone.list.
            // Live, 2026-11-17: the page and the notch showed "[redacted]" where the code belongs.
            if let list = try? await service.list(), let asked = list.pairing {
                apply(list)
                pairing = .asking(asked, until: clock.addingTimeInterval(list.pairingSecondsLeft ?? Self.answerWindow))
            } else {
                pairing = .asking(request.withoutMaskedCode, until: clock.addingTimeInterval(Self.answerWindow))
            }
        case "phone.removed", "phone.unused":
            await loadPhones()
            await loadActivity()
        case "screen.until_back":
            await loadScreenUse()
        default: break
        }
    }

    // MARK: The list

    public func rename(_ id: String, to name: String) async {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        do {
            let saved = try await service.rename(id: id, name: name)
            if let index = phones.firstIndex(where: { $0.id == id }) { phones[index].name = saved }
            problem = nil
        } catch {
            problem = PhoneWords.daemonProblem(error)
        }
    }

    /// Immediate: its connections close and it can't reconnect; it may be paired again as a new phone.
    public func remove(_ id: String) async {
        let name = names[id] ?? PhoneWords.unnamedPhone
        do {
            try await service.remove(id: id)
            note = PhoneWords.removed(name)
            problem = nil
        } catch {
            problem = PhoneWords.daemonProblem(error)
        }
        await loadPhones()
        await loadActivity()
    }

    // MARK: Where files from a phone land (PHN-580, PHN-581)

    /// The owner's chosen folder; the daemon checks it first (it must exist and not be one of FERB's private places).
    public func chooseUploadFolder(_ path: String) async { await saveUploadFolder(path) }

    public func resetUploadFolder() async { await saveUploadFolder(nil) }

    private func saveUploadFolder(_ path: String?) async {
        do {
            if let folder = try await service.setUploadFolder(path) { uploadFolder = folder }
            problem = nil
        } catch {
            problem = PhoneWords.daemonProblem(error)
        }
    }

    // MARK: "Until I'm back at my Mac" (GATE-615)

    func loadScreenUse() async {
        screenUse = try? await service.screenUse()
    }

    public func endScreenUse() async {
        do {
            try await service.endScreenUse()
        } catch {
            problem = PhoneWords.daemonProblem(error)
        }
        await loadScreenUse()
    }

    // MARK: Your own relay

    public var relayEntry: RelayEntry { RelayEntry(link: relayLink, address: relayAddress, token: relayToken) }

    public func testRelay() async {
        await relay { [service] entry in .reachable(PhoneWords.checkLines(try await service.testRelay(entry))) }
    }

    /// Saved only when the helper's test passes; the typed secret is then forgotten here.
    public func useRelay() async {
        await relay { [service] entry in .saved(PhoneWords.checkLines(try await service.useRelay(entry))) }
        if case .saved = relayResult {
            relayLink = ""; relayAddress = ""; relayToken = ""
            await loadLink()
        }
    }

    private func relay(_ run: (RelayEntry) async throws -> RelayResult) async {
        let entry = relayEntry
        guard !entry.isEmpty else { return }
        relayBusy = true
        defer { relayBusy = false }
        do {
            relayResult = try await run(entry)
        } catch {
            relayResult = .unreachable(Self.hiding(entry.secrets, in: PhoneWords.linkProblem(error)))
        }
    }

    public func stopUsingRelay() async {
        do {
            try await service.stopUsingRelay()
            relayResult = nil
            note = PhoneWords.relayStopped
        } catch {
            linkProblem = PhoneWords.linkProblem(error)
        }
        await loadLink()
    }

    public func setAlwaysUseRelay(_ on: Bool) async {
        do {
            try await service.setAlwaysUseRelay(on)
        } catch {
            linkProblem = PhoneWords.linkProblem(error)
        }
        await loadLink()
    }

    /// A message the helper wrote may quote what it was given: the token and the pasted line never show.
    static func hiding(_ secrets: [String], in text: String) -> String {
        // the longest first, so a whole pasted line goes before the token inside it
        secrets.filter { !$0.isEmpty }.sorted { $0.count > $1.count }
            .reduce(text) { $0.replacingOccurrences(of: $1, with: "•••") }
    }
}

/// The daemon's refusal codes this page acts on (ferb-rpc phone_gate.rs).
public enum PhoneCodes {
    public static let notFound = -32018
    public static let deviceLimit = -32023
    public static let noSuchMethod = -32601
}

// MARK: - Every word the page says (the owner reads each one)

public enum PhoneWords {
    // the list
    public static let unnamedPhone = "Phone"
    public static let thisMac = "This Mac"
    public static let aRemovedPhone = "A removed phone"
    public static let noPhones = "No phones yet. Pair one above to use FERB from your phone."
    public static func pairedOn(_ date: Date) -> String { "Paired " + date.formatted(date: .abbreviated, time: .omitted) }
    public static func lastSeen(_ date: Date) -> String { "last used " + date.formatted(.relative(presentation: .named)) }
    public static func unused(_ days: Int) -> String {
        "Not used for \(days) days. It stays paired; remove it if you don't use it anymore."
    }
    public static func full(_ max: Int) -> String { "FERB keeps up to \(max) phones. Remove one to pair another." }

    // files from a phone (PHN-580)
    public static let filesTitle = "Files from your phone"
    public static func filesLandIn(_ shown: String) -> String { "Files you send from your phone are saved in \(shown) on this Mac." }
    public static let chooseFolder = "Choose…"
    public static let useDefaultFolder = "Use Downloads"
    public static let chooseFolderPrompt = "Use This Folder"
    public static let chooseFolderMessage = "Choose where files you send from your phone are saved."

    // "until I'm back at my Mac" (GATE-615)
    public static func screenUntilBack(_ phone: String?) -> String {
        "FERB may use your Mac's screen without asking until you're back" + (phone.map { ", as you allowed from “\($0)”." } ?? ".")
    }
    public static let screenUntilBackEnds = "It ends as soon as you use the keyboard or pointer here, send a message from " +
        "this Mac, or your Mac locks or sleeps."
    public static let askAgain = "Ask me again"
    public static func removed(_ name: String) -> String { "“\(name)” was removed. It can't reach FERB anymore." }
    public static let connectedDirect = "Connected directly"
    public static let connectedOwnRelay = "Connected through your relay"
    public static let connectedSharedRelay = "Connected through the shared relay"
    public static let notConnected = "Not connected"
    public static func removeTitle(_ name: String) -> String { "Remove “\(name)”?" }
    public static let removeMessage = "It's disconnected right away and can't reach FERB again. You can pair it again later as a new phone."

    // pairing
    public static let pairIntro = "Shows a code for your phone to scan. It works once, for two minutes, and you confirm the phone here."
    public static let scanThis = "Open FERB on your phone and scan this code."
    public static let orType = "If the camera doesn't read it, type or paste this in the app instead:"
    public static func validFor(_ seconds: Int) -> String { "Works for \(clock(seconds)) more, for one phone." }
    public static func wantsToPair(_ name: String) -> String { "“\(name)” wants to pair with FERB." }
    public static let checkCode = "Check that your phone shows this code:"
    public static let pairExplain = "Pair it only if the codes match. Your phone can then do what FERB.app does: send tasks, " +
        "answer FERB's questions and see what FERB is doing. Opening or sending files on this Mac and changing FERB's " +
        "settings also ask for its fingerprint or PIN."
    public static func answerWithin(_ seconds: Int) -> String { "Answer within \(clock(seconds))." }
    public static func paired(_ name: String) -> String { "“\(name)” is paired." }
    public static let notPaired = "Not paired."
    public static let expired = "The code expired. Choose Pair a phone for a new one."
    public static let noAnswer = "The phone stopped waiting for your answer, so it wasn't paired."
    public static let requestEnded = "That phone stopped waiting, so it wasn't paired."
    public static let starting = "Making a code…"

    // the notch
    /// The notch's line carries no code: a chat notice is kept and masked like any text; the digits are on the page.
    public static func notchAsks(_ name: String) -> String {
        "“\(name)” wants to pair with FERB. Compare its code and answer in Settings → Phone."
    }
    public static let openPhoneSettings = "Open Settings → Phone"

    // activity
    public static let noActivity = "Nothing yet."
    public static let refused = "not allowed"
    public static let saidNoToPairing = "said no to a phone that asked to pair"
    public static func did(_ method: String, fromMac: Bool) -> String {
        switch method {
        case "chat.send": return "sent a message"
        case "chat.approve", "chat.answer", "chat.correct": return "answered a question"
        case "task.stop": return "stopped a task"
        case "stop.engage": return "stopped everything"
        case "stop.lift": return "let FERB work again"
        case "phone.unlock": return "unlocked FERB"
        case "phone.lock": return "locked FERB"
        case "phone.keys": return "added its keys"
        case "phone.remove": return fromMac ? "removed a phone" : "removed another phone"
        case "phone.pair_decide": return "paired a phone"
        case "phone.list": return "looked at the paired phones"
        case "settings.get": return "read FERB's settings"
        case "settings.set": return "changed a setting"
        case let m where m.hasPrefix("chat.entries"): return "read the chat"
        case let m where m.hasPrefix("tasks."): return "looked at tasks"
        default: return "used \(method)"
        }
    }

    // your own relay
    public static let statusShared = "Phones connect directly when they can. Otherwise they use the free shared relay, which " +
        "carries messages, questions and Stop; pictures, files and the live view wait for a direct connection or your own relay."
    public static let statusNoRelayYet = "This Mac isn't connected to a relay yet. Phones on the same Wi-Fi can still connect directly."
    public static let statusOwn = "Phones connect directly when they can, and through your relay otherwise."
    public static let statusOwnAlways = "Phones always connect through your relay."
    public static let statusOwnDown = "Your relay is set but not connected yet."
    public static let statusOwnFallback = "Your relay can't be reached right now, so phones use the free shared relay until " +
        "it's back: messages, questions and Stop only."
    public static func savedRelay(_ address: String) -> String { "Your relay: \(address)" }
    public static let pasteIntro = "Paste the line your relay's setup printed. It starts with ferbrelay://."
    public static let relayLinkField = "ferbrelay://…"
    public static let useAddressInstead = "Use an https address and access token instead"
    public static let addressField = "https://relay.example.com"
    public static let tokenField = "Access token"
    public static let relayExplain = "FERB tests it before saving. Your phones pick it up from this Mac by themselves; " +
        "nothing is typed on the phone. The token is kept in this Mac's Keychain."
    public static let always = "Always use my relay"
    public static let alwaysExplain = "Phones go through your relay even when a direct connection is possible. If your " +
        "relay stops answering, they connect directly or through the shared relay until it's back."
    public static let reachable = "Your relay answered."
    public static let savedNow = "Saved. Your phones switch to your relay now."
    public static let relayStopped = "FERB stopped using your relay. Phones connect directly or through the shared relay."
    // PHN-220, PHN-590, PHN-591: phones that haven't connected since the relay changed
    public static func phonesToCome(_ count: Int, until: Date?) -> String {
        let phones = count == 1 ? "1 phone hasn't" : "\(count) phones haven't"
        let date = until.map { " until " + $0.formatted(date: .abbreviated, time: .omitted) } ?? ""
        return "\(phones) connected since you changed the relay. FERB keeps the way to this Mac open for them\(date); " +
            "they switch to your relay the next time they connect."
    }
    public static func phonesLeftBehind(_ count: Int) -> String {
        let phones = count == 1 ? "1 phone hasn't" : "\(count) phones haven't"
        return "\(phones) connected since you changed the relay and can't find this Mac now. To bring one back, choose " +
            "Stop using my relay, open FERB on that phone, then set your relay again. Or remove it and pair it again."
    }
    public static func checkLines(_ check: RelayCheck) -> [String] { check.steps + check.warnings.map { "Note: " + $0 } }

    // the how-to (PHN-418: the public relay's message points here)
    public static let howToTitle = "Set up your own relay"
    public static let howToIntro = "Your own relay makes FERB on your phone faster when you're away: pictures, files and " +
        "the live view work at full size wherever the phone is. It's optional; messages and questions work without it."
    public static let howToSteps = [
        "Get a small Linux server with systemd that your phone can reach. 64 MB of memory and 100 MB of disk are enough.",
        "Copy FERB's relay setup script to it: scp setup.sh root@your-server:",
        "On a server that already runs other things, run: ssh root@your-server sh setup.sh --shared. " +
            "It uses TCP port 7443 and UDP port 7842; allow them in your provider's firewall.",
        "On a server of its own, with a DNS name and ports 80 and 443 free, run: " +
            "ssh root@relay.example.com sh setup.sh relay.example.com you@example.com",
        "The script prints one line starting with ferbrelay://. Paste it above, choose Test, then Use this relay.",
    ]
    public static let howToCheck = "To see whether a server is ready without changing anything, run sh setup.sh --check --shared " +
        "there. To remove the relay later, run sh setup.sh --uninstall."
    public static let scriptInSource = "The setup script and its guide are in FERB's source code, in the folder " +
        "engine/crates/ferb-link/relay."
    public static let showScript = "Show the setup script in Finder"

    // when something isn't there
    public static let noLinkInThisVersion = "Pairing a phone isn't available in this version of FERB yet."
    public static let linkNotAnswering = "FERB's phone link isn't answering right now. Try again in a moment."
    public static let serviceTooOld = "FERB's background service doesn't handle phones yet. It will after FERB updates."

    /// A daemon error in plain words: FERB not running says so (its own words), an older service says it can't yet.
    public static func daemonProblem(_ error: Error) -> String {
        if let rpc = error as? DaemonRPCError, rpc.code == PhoneCodes.noSuchMethod { return serviceTooOld }
        return error.localizedDescription
    }

    /// A link error in plain words: the helper's own message without its code, or that it isn't answering.
    public static func linkProblem(_ error: Error) -> String {
        if let service = error as? PhoneServiceError { return service.localizedDescription }
        guard let rpc = error as? DaemonRPCError else { return error.localizedDescription }
        switch rpc {
        case .unavailable, .disconnected: return linkNotAnswering
        case .remote(let message, _): return linkMessage(message, allow: nil) ?? linkNotAnswering
        default: return rpc.localizedDescription
        }
    }

    /// The helper writes "code: message (what to do)"; the owner reads "message. What to do."
    public static func linkMessage(_ message: String?, allow: String?) -> String? {
        guard var text = message?.trimmingCharacters(in: .whitespaces), !text.isEmpty else { return nil }
        if let colon = text.range(of: ": "), text[..<colon.lowerBound].allSatisfy({ $0.isLowercase || $0 == "_" }) {
            text = String(text[colon.upperBound...])
        }
        var advice = allow
        if advice == nil, text.hasSuffix(")"), let open = text.range(of: " (", options: .backwards) {
            advice = String(text[open.upperBound..<text.index(before: text.endIndex)])
            text = String(text[..<open.lowerBound])
        }
        func sentence(_ s: String) -> String {
            guard let first = s.first else { return s }
            let s = first.uppercased() + s.dropFirst()
            return s.hasSuffix(".") || s.hasSuffix("?") || s.hasSuffix("!") ? s : s + "."
        }
        return [text, advice].compactMap { $0 }.filter { !$0.isEmpty }.map(sentence).joined(separator: " ")
    }

    static func clock(_ seconds: Int) -> String { String(format: "%d:%02d", seconds / 60, seconds % 60) }
}
