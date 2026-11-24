import Combine
import CryptoKit
import Foundation

// FERB's account in FERB.app (REL-449, APP-1127, APP-1138): signing in is required, with Google or Apple, through the
// site's sign-in (site/api/app) in the person's own browser, back to FERB.app over the loopback (REL-452,
// LoopbackReturn), and the same user list as the site's download sign-in. The app holds a short-lived
// access token and a rotating refresh token, both opaque. They live only in the Keychain, through the daemon's
// `account.save` and FERB's signed secret helper: never a file, a preference, a log, a trace or anything a model sees.
// Here: the pieces every part of the flow shares (the request, the callback, the account and its tokens) and the
// session that runs it with its seams (the server, the browser, the Keychain) injected.

public enum AccountProvider: String, CaseIterable, Sendable {
    case google, apple

    public var title: String { self == .google ? "Google" : "Apple" }
}

/// The site the apps sign in through. Fixed in the build: a setting a program could change would let it send the
/// sign-in elsewhere.
public enum AccountSite {
    public static let base = URL(string: "https://ferb.kartikey.xyz")!
    public static let platform = "mac"
    /// The loopback return (RFC 8252 §7.3, REL-452): the site sends the browser to http://127.0.0.1:<port>/callback,
    /// the port FERB.app listens on for this sign-in. Another program that saw the code couldn't use it: the site
    /// trades it only with this sign-in's PKCE verifier.
    public static let loopbackHost = "127.0.0.1"
    public static let callbackPath = "/callback"
    /// How long FERB.app waits for the browser.
    public static let signInTimeout: TimeInterval = 10 * 60

    public static func returnAddress(port: UInt16) -> URL {
        URL(string: "http://\(loopbackHost):\(port)\(callbackPath)")!
    }
}

/// Who is signed in: never a secret.
public struct AccountInfo: Equatable, Sendable {
    public var id: String
    public var email: String
    public var name: String
    public var providers: [String]

    public init(id: String, email: String, name: String = "", providers: [String] = []) {
        self.id = id
        self.email = email
        self.name = name
        self.providers = providers
    }

    public init?(json: [String: Any]?) {
        guard let json, let id = json["id"] as? String, !id.isEmpty, let email = json["email"] as? String,
              !email.isEmpty else { return nil }
        self.init(id: id, email: email, name: json["name"] as? String ?? "",
                  providers: (json["providers"] as? [String]) ?? [])
    }

    public var json: [String: Any] { ["id": id, "email": email, "name": name, "providers": providers] }

    /// "with Google", "with Apple", "with Google and Apple".
    public var providerWords: String {
        let names = AccountProvider.allCases.filter { providers.contains($0.rawValue) }.map(\.title)
        return names.isEmpty ? "" : "with " + names.joined(separator: " and ")
    }
}

/// The two tokens. Their values never print: `description` names the type only, so a log or a `dump` can't carry them.
public struct AccountTokens: Equatable, Sendable, CustomStringConvertible, CustomDebugStringConvertible {
    public var access: String
    /// Unix seconds.
    public var accessExpiresAt: Double
    public var refresh: String

    public init(access: String, accessExpiresAt: Double, refresh: String) {
        self.access = access
        self.accessExpiresAt = accessExpiresAt
        self.refresh = refresh
    }

    public init?(json: [String: Any]?) {
        guard let json, let access = json["access_token"] as? String, let refresh = json["refresh_token"] as? String,
              !refresh.isEmpty else { return nil }
        let expires = (json["access_expires_at"] as? Double) ?? (json["access_expires_at"] as? Int).map(Double.init) ?? 0
        self.init(access: access, accessExpiresAt: expires, refresh: refresh)
    }

    public var json: [String: Any] {
        ["access_token": access, "access_expires_at": accessExpiresAt, "refresh_token": refresh]
    }

    /// Under a minute left (or gone): refresh before using it.
    public func expiresSoon(now: Date) -> Bool { accessExpiresAt - now.timeIntervalSince1970 < 60 }

    public var description: String { "AccountTokens(hidden)" }
    public var debugDescription: String { description }
}

/// This install, as the server records it: a random id made once, the platform and FERB's version. Nothing personal.
public struct AccountDevice: Equatable, Sendable {
    public static let idKey = "FERBAccountDevice"
    public var id: String
    public var version: String

    public init(id: String, version: String) {
        self.id = id
        self.version = version
    }

    /// The install's id from `defaults`, made on first use (not a secret: it only names this install to the server).
    public static func current(_ defaults: UserDefaults, version: String) -> AccountDevice {
        if let id = defaults.string(forKey: idKey), id.count >= 16 { return AccountDevice(id: id, version: version) }
        let id = AccountRequest.base64url(AccountRequest.secureRandom(16))
        defaults.set(id, forKey: idKey)
        return AccountDevice(id: id, version: version)
    }

    public var json: [String: Any] { ["id": id, "platform": AccountSite.platform, "version": String(version.prefix(32))] }
}

/// One sign-in's secrets, kept in memory only for its length: the PKCE verifier and the app's state (RFC 7636, RFC
/// 8252). The start address carries only the verifier's SHA-256 and the state.
public struct AccountRequest: Equatable, Sendable {
    public let verifier: String
    public let state: String

    public init(verifier: String, state: String) {
        self.verifier = verifier
        self.state = state
    }

    public static func make(random: (Int) -> Data = AccountRequest.secureRandom) -> AccountRequest {
        AccountRequest(verifier: base64url(random(32)), state: base64url(random(16)))
    }

    /// S256: base64url(SHA-256(verifier)).
    public var challenge: String { Self.base64url(Data(SHA256.hash(data: Data(verifier.utf8)))) }

    /// `redirect`: the loopback address FERB.app listens on for this sign-in.
    public func startURL(_ provider: AccountProvider, redirect: URL, base: URL = AccountSite.base) -> URL {
        var parts = URLComponents(url: base.appendingPathComponent("api/app/start"), resolvingAgainstBaseURL: false)!
        parts.queryItems = [
            URLQueryItem(name: "provider", value: provider.rawValue),
            URLQueryItem(name: "platform", value: AccountSite.platform),
            URLQueryItem(name: "redirect", value: redirect.absoluteString),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
        ]
        return parts.url!
    }

    public static func base64url(_ data: Data) -> String {
        data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    public static func secureRandom(_ count: Int) -> Data {
        var bytes = [UInt8](repeating: 0, count: count)
        precondition(SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess, "no system random source")
        return Data(bytes)
    }
}

/// What came back to the loopback address.
public enum AccountCallback: Equatable, Sendable {
    case code(String)
    case cancelled
    case failed

    /// Nil when the address isn't this sign-in's (another scheme, host, port or path, or another state: CSRF, RFC 6749
    /// §10.12).
    public static func read(_ url: URL, state: String, returnAddress: URL) -> AccountCallback? {
        guard url.scheme == "http", url.host == AccountSite.loopbackHost, url.port != nil,
              url.port == returnAddress.port, url.path == AccountSite.callbackPath, url.user == nil,
              url.fragment == nil,
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              let given = items.first(where: { $0.name == "state" })?.value,
              Self.same(given, state) else { return nil }
        if let error = items.first(where: { $0.name == "error" })?.value {
            return error == "cancelled" ? .cancelled : .failed
        }
        guard let code = items.first(where: { $0.name == "code" })?.value, !code.isEmpty, code.count <= 256 else {
            return .failed
        }
        return .code(code)
    }

    private static func same(_ a: String, _ b: String) -> Bool {
        let (x, y) = (Array(a.utf8), Array(b.utf8))
        guard x.count == y.count else { return false }
        return zip(x, y).reduce(UInt8(0)) { $0 | ($1.0 ^ $1.1) } == 0
    }
}

/// A server answer: the thing asked for, the server's definite No (the session is over), no answer (offline, the
/// server down), or another failure.
public enum AccountReply<Value> {
    case ok(Value)
    case rejected
    case unreachable
    case failed
}

public struct AccountGrant: Sendable {
    public var tokens: AccountTokens
    public var account: AccountInfo
    public init(tokens: AccountTokens, account: AccountInfo) {
        self.tokens = tokens
        self.account = account
    }
}

/// The site's app endpoints (site/api/app).
public protocol AccountServing: Sendable {
    /// Whether sign-in with `provider` is set up on the site (`/api/app/ready`), asked before a browser opens.
    func ready(_ provider: AccountProvider) async -> AccountReply<Bool>
    func exchange(code: String, verifier: String, device: AccountDevice) async -> AccountReply<AccountGrant>
    func refresh(_ refreshToken: String, device: AccountDevice) async -> AccountReply<AccountGrant>
    func signOut(refreshToken: String) async
    func delete(accessToken: String) async -> AccountReply<Void>
}

/// The site over HTTPS. Ephemeral: no cookies or cache kept for these calls.
public final class HTTPAccountServer: AccountServing, @unchecked Sendable {
    private let base: URL
    private let session: URLSession
    private let now: @Sendable () -> Date

    public init(base: URL = AccountSite.base, session: URLSession? = nil, now: @escaping @Sendable () -> Date = Date.init) {
        self.base = base
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 20
        config.httpCookieStorage = nil
        config.urlCache = nil
        self.session = session ?? URLSession(configuration: config)
        self.now = now
    }

    public func ready(_ provider: AccountProvider) async -> AccountReply<Bool> {
        var parts = URLComponents(url: base.appendingPathComponent("api/app/ready"), resolvingAgainstBaseURL: false)!
        parts.queryItems = [URLQueryItem(name: "provider", value: provider.rawValue)]
        guard let (data, response) = try? await session.data(from: parts.url!),
              let status = (response as? HTTPURLResponse)?.statusCode else { return .unreachable }
        let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        switch Self.classify(status: status, body: body) {
        case .ok(let body): return .ok(body["ready"] as? Bool == true)
        case .unreachable: return .unreachable
        case .rejected, .failed: return .ok(false)       // a site without the apps' sign-in (404) isn't set up
        }
    }

    public func exchange(code: String, verifier: String, device: AccountDevice) async -> AccountReply<AccountGrant> {
        grant(await post("api/app/token", ["grant_type": "authorization_code", "code": code,
                                           "code_verifier": verifier, "device": device.json]))
    }

    public func refresh(_ refreshToken: String, device: AccountDevice) async -> AccountReply<AccountGrant> {
        grant(await post("api/app/token", ["grant_type": "refresh_token", "refresh_token": refreshToken,
                                           "device": device.json]))
    }

    public func signOut(refreshToken: String) async {
        _ = await post("api/app/signout", ["refresh_token": refreshToken])
    }

    public func delete(accessToken: String) async -> AccountReply<Void> {
        switch await post("api/app/delete", [:], bearer: accessToken) {
        case .ok(let reply) where reply["deleted"] as? Bool == true: return .ok(())
        case .ok: return .failed
        case .rejected: return .rejected
        case .unreachable: return .unreachable
        case .failed: return .failed
        }
    }

    private func grant(_ reply: AccountReply<[String: Any]>) -> AccountReply<AccountGrant> {
        switch reply {
        case .ok(let body):
            guard let access = body["access_token"] as? String, let refresh = body["refresh_token"] as? String,
                  let account = AccountInfo(json: body["account"] as? [String: Any]) else { return .failed }
            let seconds = (body["expires_in"] as? Double) ?? (body["expires_in"] as? Int).map(Double.init) ?? 0
            return .ok(AccountGrant(tokens: AccountTokens(access: access,
                                                          accessExpiresAt: now().timeIntervalSince1970 + seconds,
                                                          refresh: refresh), account: account))
        case .rejected: return .rejected
        case .unreachable: return .unreachable
        case .failed: return .failed
        }
    }

    private func post(_ path: String, _ body: [String: Any], bearer: String? = nil) async -> AccountReply<[String: Any]> {
        var request = URLRequest(url: base.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        if let bearer { request.setValue("Bearer " + bearer, forHTTPHeaderField: "authorization") }
        request.httpBody = try? JSONSerialization.data(withJSONObject: body)
        guard let (data, response) = try? await session.data(for: request),
              let status = (response as? HTTPURLResponse)?.statusCode else { return .unreachable }
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        return Self.classify(status: status, body: json)
    }

    /// 200 is the answer; 401 (`invalid_grant`, `invalid_token`) is the server's definite No; 5xx and 429 are "not
    /// now"; anything else failed.
    public static func classify(status: Int, body: [String: Any]) -> AccountReply<[String: Any]> {
        switch status {
        case 200: return .ok(body)
        case 401: return .rejected
        case 400 where body["error"] as? String == "invalid_grant": return .rejected
        case 429, 500...599: return .unreachable
        default: return .failed
        }
    }
}

public enum AccountVaultRead {
    case record(AccountInfo, AccountTokens)
    case none
    /// The daemon didn't answer: nothing is known.
    case unavailable
}

/// Where the account and its tokens are kept: the Keychain, through the daemon and FERB's signed secret helper.
public protocol AccountVault: Sendable {
    func load() async -> AccountVaultRead
    /// The names of the phones the daemon removed because they were paired under another account; nil when not saved.
    func save(_ account: AccountInfo, _ tokens: AccountTokens) async -> [String]?
    func clear() async -> Bool
}

/// `account.get`, `account.save`, `account.clear`: FERB.app's own calls (app_only). The daemon stores one Keychain
/// item through the signed helper and gives the token values to its redactor.
public struct DaemonAccountVault: AccountVault, @unchecked Sendable {
    let rpc: DaemonRPCTransport

    public init(rpc: DaemonRPCTransport) { self.rpc = rpc }

    public func load() async -> AccountVaultRead {
        // Only the daemon's own "signed_in" counts: no answer, or one without it, says nothing either way.
        guard let reply = try? await rpc.call("account.get", params: [:], wait: 30),
              let signedIn = reply["signed_in"] as? Bool else { return .unavailable }
        guard signedIn, let account = AccountInfo(json: reply["account"] as? [String: Any]),
              let tokens = AccountTokens(json: reply["tokens"] as? [String: Any]) else { return .none }
        return .record(account, tokens)
    }

    public func save(_ account: AccountInfo, _ tokens: AccountTokens) async -> [String]? {
        guard let reply = try? await rpc.call("account.save", params: ["account": account.json, "tokens": tokens.json],
                                              wait: 30),
              reply["saved"] as? Bool == true else { return nil }
        return (reply["removed_phones"] as? [[String: Any]] ?? []).compactMap { $0["name"] as? String }
    }

    public func clear() async -> Bool {
        (try? await rpc.call("account.clear", params: [:], wait: 30))?["cleared"] as? Bool == true
    }
}

public enum WebSignInResult: Equatable, Sendable {
    /// This sign-in's callback came back to the loopback address.
    case callback(URL)
    /// Cancel in the account step.
    case cancelled
    /// Nothing came back in `AccountSite.signInTimeout`.
    case timedOut
    /// Nothing could listen, or no browser opened.
    case failed
}

/// The browser step: the person's default browser and the loopback return (`BrowserSignIn` in FERB.app).
@MainActor
public protocol WebSignIn: AnyObject {
    /// Listens for this sign-in's return, opens `start(returnAddress)` in the browser and waits.
    func run(state: String, start: (URL) -> URL) async -> WebSignInResult
    /// Says how the sign-in ended on the browser's tab that brought the callback, and brings FERB forward.
    func finish(_ ending: LoopbackReturn.Ending)
    /// Stops waiting (Cancel in the account step).
    func cancel()
}

/// The browser step in FERB.app: the person's default browser (`open`, NSWorkspace there) and the loopback return. No
/// system sign-in window and no system alert: the page is the browser's own tab, like other apps' sign-ins.
@MainActor
public final class BrowserSignIn: WebSignIn {
    private let open: (URL) -> Bool
    private let bringForward: () -> Void
    private let port: UInt16
    private var current: LoopbackReturn?

    /// `port` 0: the system picks one (tests may name one).
    public init(port: UInt16 = 0, open: @escaping (URL) -> Bool, bringForward: @escaping () -> Void) {
        self.port = port
        self.open = open
        self.bringForward = bringForward
    }

    public func run(state: String, start: (URL) -> URL) async -> WebSignInResult {
        current?.cancel()
        let loopback = LoopbackReturn(state: state, port: port)
        current = loopback
        guard let address = await loopback.open() else {
            loopback.cancel()
            return .failed
        }
        guard open(start(address)) else {
            loopback.cancel()
            return .failed
        }
        return await loopback.wait()
    }

    public func finish(_ ending: LoopbackReturn.Ending) {
        current?.answer(ending)
        current = nil
        bringForward()
    }

    public func cancel() { current?.cancel() }
}

/// The owner-facing words of the account (plain, friendly; no "AI": the owner's wording rule).
public enum AccountWords {
    public static let google = "Continue with Google"
    public static let apple = "Continue with Apple"
    public static let kept = "FERB keeps it to know who uses FERB, and doesn't send you emails."
    public static let waiting = "Continue in your browser."
    public static let waitingDetail = "FERB opened the sign-in page in your browser and comes back here when you're done."
    public static let cancelled = "Sign-in was cancelled. Try again when you're ready."
    public static let timedOut = "Sign-in took too long, so FERB stopped waiting. Try again when you're ready."
    public static let browserFailed = "FERB couldn't open the sign-in page in your browser. Try again."
    /// The browser's tab after the sign-in (LoopbackReturn's page).
    public static let pageSignedIn = "You're signed in to FERB. You can close this tab."
    public static let pageCancelled = "Sign-in was cancelled. You can close this tab and try again in FERB."
    public static let pageFailed = "FERB couldn't finish signing you in. You can close this tab and try again in FERB."
    public static let pageUnanswered = "Go back to FERB to finish signing in. You can close this tab."
    public static let offline = "FERB can't reach its sign-in right now. Check your internet connection and try again."
    public static let serverDown = "FERB's sign-in isn't answering right now. Try again in a minute."
    public static func notSetUp(_ provider: AccountProvider) -> String {
        "Signing in with \(provider.title) isn't ready on FERB's site yet. Try again later."
    }
    public static let notKept = "FERB couldn't keep your sign-in on this Mac. Try again."
    public static let tryAgain = "Try again"
    public static func signedIn(_ email: String) -> String { "You're signed in as \(email)." }
    public static func signedInAs(_ email: String) -> String { "Signed in as \(email)" }
    public static let signOut = "Sign out"
    public static let deleteAccount = "Delete account…"
    public static let signOutQuestion = "Sign out of FERB on this Mac? You'll need to sign in again to keep using FERB."
    public static func deleteQuestion(_ email: String) -> String {
        "Delete your FERB account? This deletes \(email)'s account and signs FERB out on all your devices. It can't " +
            "be undone."
    }
    public static let deleteConfirm = "Delete account"
    public static let cancel = "Cancel"
    public static let deleteFailed = "FERB couldn't delete your account just now. Check your connection and try again."
    public static let checkTrouble = "FERB couldn't check your sign-in just now. It keeps working and tries again later."
    public static func phonesRemoved(_ email: String, names: [String]) -> String {
        "You're signed in as \(email) now, so FERB removed the phones paired with your other account: " +
            "\(names.joined(separator: ", ")). Sign in on each phone with \(email) and pair it again."
    }
}

/// FERB.app's account: what the tour's account step and Settings → Account show and do (APP-1138).
/// - Signed out blocks FERB.app (the tour's account step, which has no skip). A definite No from the server (the
///   refresh token revoked, reused or expired, the account deleted) signs out.
/// - A refresh that can't reach the server keeps the owner signed in and working; it is tried again later.
/// - One refresh at a time: the server revokes a whole sign-in when a refresh token is used twice.
@MainActor
public final class AccountSession: ObservableObject {
    public enum State: Equatable {
        case unknown
        case signedOut
        case signedIn(AccountInfo)
    }

    @Published public private(set) var state: State = .unknown
    /// The last outcome worth saying (cancelled, offline, failed), nil when there is none.
    @Published public private(set) var line: String?
    /// The last check couldn't reach the server: still signed in.
    @Published public private(set) var checkTrouble = false
    @Published public private(set) var busy = false
    /// Phones the daemon removed at the last sign-in because they were paired under another account.
    @Published public private(set) var removedPhones: [String] = []
    /// The owner is signed out now (sign-out, a deleted account, the server's No): FERB.app shows the account step.
    public var onSignedOut: (() -> Void)?

    private let vault: AccountVault
    private let server: AccountServing
    private let web: WebSignIn?
    private let device: AccountDevice
    private let now: () -> Date
    private let makeRequest: () -> AccountRequest
    private var tokens: AccountTokens?
    private var refreshing: Task<Void, Never>?

    public init(vault: AccountVault, server: AccountServing, web: WebSignIn?, device: AccountDevice,
                now: @escaping () -> Date = Date.init,
                makeRequest: @escaping () -> AccountRequest = { AccountRequest.make() }) {
        self.vault = vault
        self.server = server
        self.web = web
        self.device = device
        self.now = now
        self.makeRequest = makeRequest
    }

    public var signedIn: Bool { if case .signedIn = state { return true } else { return false } }
    public var account: AccountInfo? { if case .signedIn(let info) = state { return info } else { return nil } }

    /// Reads the Keychain item through the daemon. A daemon that doesn't answer changes nothing (`unknown` stays).
    @discardableResult
    public func load() async -> State {
        switch await vault.load() {
        case .record(let account, let tokens):
            self.tokens = tokens
            state = .signedIn(account)
        case .none:
            tokens = nil
            state = .signedOut
        case .unavailable:
            break
        }
        return state
    }

    /// One sign-in through the browser; true when signed in and kept.
    public func signIn(_ provider: AccountProvider) async -> Bool {
        guard let web, !busy else { return false }
        busy = true
        defer { busy = false }
        line = nil
        // The site says first whether this sign-in is set up: never a browser opened on an error page (APP-1138).
        switch await server.ready(provider) {
        case .ok(true): break
        case .ok(false), .rejected, .failed:
            line = AccountWords.notSetUp(provider)
            return false
        case .unreachable:
            line = AccountWords.offline
            return false
        }
        let request = makeRequest()
        var returnAddress: URL?
        let result = await web.run(state: request.state) { address in
            returnAddress = address
            return request.startURL(provider, redirect: address)
        }
        let url: URL
        switch result {
        case .callback(let back): url = back
        case .cancelled: line = AccountWords.cancelled; return false
        case .timedOut: line = AccountWords.timedOut; return false
        case .failed: line = AccountWords.browserFailed; return false
        }
        // The browser's tab waits for this answer, so it says what really happened.
        switch returnAddress.flatMap({ AccountCallback.read(url, state: request.state, returnAddress: $0) }) {
        case .code(let code)?:
            let done = await finish(server.exchange(code: code, verifier: request.verifier, device: device))
            web.finish(done ? .signedIn : .failed)
            return done
        case .cancelled?:
            line = AccountWords.cancelled
            web.finish(.cancelled)
        case .failed?, nil:
            line = AccountWords.serverDown
            web.finish(.failed)
        }
        return false
    }

    /// Cancel while the browser is open: FERB stops waiting for it (the account step's Cancel).
    public func cancelSignIn() { web?.cancel() }

    private func finish(_ reply: AccountReply<AccountGrant>) async -> Bool {
        switch reply {
        case .ok(let grant):
            guard let removed = await vault.save(grant.account, grant.tokens) else {
                line = AccountWords.notKept
                await server.signOut(refreshToken: grant.tokens.refresh)
                return false
            }
            tokens = grant.tokens
            removedPhones = removed
            checkTrouble = false
            state = .signedIn(grant.account)
            return true
        case .unreachable:
            line = AccountWords.offline
        case .rejected, .failed:
            line = AccountWords.serverDown
        }
        return false
    }

    /// Refreshes the tokens when the access token is nearly gone (`force`: whatever its age). Network trouble keeps
    /// the owner signed in (`checkTrouble`); the server's definite No signs out.
    public func refresh(force: Bool = false) async {
        if let refreshing { return await refreshing.value }
        guard signedIn, let current = tokens, force || current.expiresSoon(now: now()) else { return }
        let task = Task { [weak self] in
            guard let self else { return }
            await self.rotate(current)
        }
        refreshing = task
        await task.value
        refreshing = nil
    }

    private func rotate(_ current: AccountTokens) async {
        switch await server.refresh(current.refresh, device: device) {
        case .ok(let grant):
            // The old refresh token is spent now: the new one must be kept, or the next refresh signs out.
            tokens = grant.tokens
            if await vault.save(grant.account, grant.tokens) == nil { checkTrouble = true; return }
            checkTrouble = false
            state = .signedIn(grant.account)
        case .unreachable, .failed:
            checkTrouble = true
        case .rejected:
            await signedOutHere()
        }
    }

    /// Settings → Account → Sign out: the server ends this sign-in (when it can be reached), the Keychain item goes.
    public func signOut() async {
        if let refresh = tokens?.refresh { await server.signOut(refreshToken: refresh) }
        await signedOutHere()
    }

    /// Settings → Account → Delete account: only once the server says it is deleted.
    public func deleteAccount() async -> Bool {
        guard !busy else { return false }
        busy = true
        defer { busy = false }
        await refresh()
        guard let access = tokens?.access, signedIn else { line = AccountWords.deleteFailed; return false }
        switch await server.delete(accessToken: access) {
        case .ok:
            await signedOutHere()
            return true
        case .rejected:
            await signedOutHere()
            return true
        case .unreachable, .failed:
            line = AccountWords.deleteFailed
            return false
        }
    }

    private func signedOutHere() async {
        tokens = nil
        _ = await vault.clear()
        checkTrouble = false
        state = .signedOut
        onSignedOut?()
    }
}
