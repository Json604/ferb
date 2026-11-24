import CryptoKit
import FerbCore
import Foundation
import Observation
import Security

// FERB's account on the phone (owner 2026-11-24: "required. google and apple."; kb PHN-828, PHN-839, PHN-841): the
// sign-in, the tokens and the account in the Keychain only, their refresh, and the calls the phone makes with them (the
// pairing assertion, sign out and delete). The site's API is the shared spec's; this is the iPhone's
// client of it. Google signs in through the site's web flow in the system's sign-in sheet, which shares Safari's
// sign-ins (owner, PHN-841: show the accounts already signed in). Apple signs in with the system's own Sign in with
// Apple (Face ID, no web page; /api/app/apple checks Apple's identity token for this app); when the system can't show
// it (no Apple account on the phone, the capability missing from a build), the web flow is the way.

/// The signed-in account as the server says it.
public struct AccountInfo: Codable, Equatable, Sendable {
    public let id: String
    public let email: String
    public let name: String?
    /// "google", "apple" or both.
    public let providers: [String]

    public init(id: String, email: String, name: String?, providers: [String]) {
        self.id = id
        self.email = email
        self.name = name
        self.providers = providers
    }
}

/// What the Keychain item holds: the account and its tokens (never in UserDefaults, a file or a log).
struct SavedAccount: Codable, Equatable {
    var account: AccountInfo
    var accessToken: String
    /// Epoch seconds.
    var accessExpiresAt: Double
    var refreshToken: String
}

/// HTTP for the account's calls: URLSession on the phone, a fake in tests (no real network in tests). Answers the
/// status and the body; throws only when nothing came back (no network, a timeout).
public protocol HttpClient: Sendable {
    func send(_ request: URLRequest) async throws -> (status: Int, body: Data)
}

/// The real one: an ephemeral session (no cookies, no cache kept on disk), 20 s a request.
public final class URLSessionHttp: HttpClient {
    private let session: URLSession

    public init() {
        let c = URLSessionConfiguration.ephemeral
        c.timeoutIntervalForRequest = 20
        c.urlCache = nil
        c.httpCookieStorage = nil
        c.requestCachePolicy = .reloadIgnoringLocalCacheData
        session = URLSession(configuration: c)
    }

    public func send(_ request: URLRequest) async throws -> (status: Int, body: Data) {
        let (data, response) = try await session.data(for: request)
        return ((response as? HTTPURLResponse)?.statusCode ?? 0, data)
    }
}

/// How the sign-in's web page ended.
public enum WebSignInResult: Equatable, Sendable {
    /// The site sent the browser to the callback (its query holds a code or an error, and the state).
    case callback(URL)
    /// The person closed the sheet.
    case cancelled
    /// The system couldn't show it.
    case failed
}

/// The system's web sign-in sheet (ASWebAuthenticationSession in FerbAppUI), a fake in tests.
@MainActor
public protocol WebSignIn: AnyObject {
    func open(_ url: URL, callbackScheme: String) async -> WebSignInResult
}

/// How the system's own Sign in with Apple ended (PHN-841).
public enum AppleSignInResult: Equatable, Sendable {
    /// Apple's identity token for this app, and the name Apple shares (only on the first authorization).
    case credential(identityToken: String, name: String?)
    /// The person closed it.
    case cancelled
    /// The system couldn't show it: the web flow is used instead.
    case failed
}

/// The system's Sign in with Apple (ASAuthorizationAppleIDProvider in FerbAppUI), a fake in tests. `nonceHash` goes
/// into the request; Apple puts it in the identity token, and the site checks it against the raw nonce.
@MainActor
public protocol AppleSignIn: AnyObject {
    func authorize(nonceHash: String) async -> AppleSignInResult
}

/// Where the account's site is: a constant, never configurable at run time (the shared spec).
public enum AccountSite {
    public static let url = URL(string: "https://ferb.kartikey.xyz")!
    /// The callback ASWebAuthenticationSession catches (the server's allowlist for `ios`).
    public static let callbackScheme = "ferb-auth"
    public static let redirect = "ferb-auth://callback"
}

public enum SignInProvider: String, Sendable { case google, apple }

/// Where the sign-in stands: nothing under way, the browser sheet open, the system's Sign in with Apple open, or the
/// tokens being fetched.
public enum SignInPhase: Equatable, Sendable { case idle, browser, apple, finishing }

/// The pairing assertion's outcome (PHN-828).
public enum AssertionResult: Hashable, Sendable {
    case ok(String)
    /// No network: pairing needs the internet to confirm the account.
    case offline
    /// The site answered with a failure (5xx, a rate limit).
    case down
    /// The site said the sign-in is over: the app shows the sign-in again.
    case signedOut
}

/// PKCE (S256) and the app's state, as the shared spec says: a 32-byte verifier and a 16-byte state, base64url.
public enum Pkce {
    public static func random(_ count: Int) -> String {
        var bytes = [UInt8](repeating: 0, count: count)
        let ok = SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess
        precondition(ok, "the system's random bytes")
        return base64url(Data(bytes))
    }

    public static func challenge(_ verifier: String) -> String { base64url(Data(SHA256.hash(data: Data(verifier.utf8)))) }

    public static func base64url(_ d: Data) -> String {
        d.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

/// The account on this phone (the one object the screens use): signed in or not, the sign-in under way and its line,
/// the refresh, and the calls made with the account. Signed out blocks the app (the gate's `signIn`).
///
/// Where things live: the account and its tokens as ONE generic-password item (`account`) of the Keychain service
/// `com.ferb.phone.account` (kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly, never synchronized; [KeychainStore]),
/// apart from the vault's service so Forget this Mac never signs out; the id of the last account signed in here
/// (`account.last`, the same service; an opaque id, kept across a sign-out to tell a different account); the device id
/// (random per install, not secret) in UserDefaults through [Prefs]. A new install (no device id yet) deletes what an
/// earlier install left in that Keychain service: iOS keeps Keychain items after an app is removed.
@MainActor
@Observable
public final class AccountModel {
    public private(set) var account: AccountInfo?
    public private(set) var phase: SignInPhase = .idle
    /// The sign-in's line (cancelled, can't reach, not answering, didn't finish), or nil.
    public private(set) var problem: Note?
    /// The sign-in page's line: what is under way (Apple's own sheet is no browser), else the last problem.
    public var line: String? {
        switch phase {
        case .idle: problem?.text
        case .apple: L.s("signin_finishing_apple")
        case .browser, .finishing: L.s("signin_finishing")
        }
    }
    /// The last refresh couldn't reach the site: FERB keeps working and tries again later.
    public private(set) var checkFailed = false

    /// A sign-in finished: the account, and whether it is a different one than the last account signed in here (the
    /// app then forgets the Mac paired under the other one: phones pair only within one account, PHN-828).
    @ObservationIgnored public var onSignedIn: ((AccountInfo, Bool) -> Void)?
    /// The site ended this sign-in (`invalid_grant` on a refresh, or a call refused after a refresh): a sign-out, which
    /// the app handles as Sign out (it leaves the Mac too, kb PHN-828). The account here is already cleared.
    @ObservationIgnored public var onSignedOutBySite: (() -> Void)?

    @ObservationIgnored private let store: SecretStore
    @ObservationIgnored private let http: HttpClient
    @ObservationIgnored public var web: WebSignIn?
    @ObservationIgnored public var apple: AppleSignIn?
    @ObservationIgnored private let prefs: Prefs
    @ObservationIgnored private let version: String
    @ObservationIgnored private let deviceName: String?
    @ObservationIgnored private let site: URL
    @ObservationIgnored private let now: () -> Date
    @ObservationIgnored private var saved: SavedAccount?
    @ObservationIgnored private var refreshing: Task<Refresh, Never>?
    /// Counts sign-ins and clears: an answer that arrives after either is dropped.
    @ObservationIgnored private var generation = 0

    static let item = "account"
    static let lastItem = "account.last"
    /// Refresh when the access token has under this many seconds left.
    static let refreshMargin: Double = 60

    public init(store: SecretStore, http: HttpClient, web: WebSignIn? = nil, apple: AppleSignIn? = nil, prefs: Prefs,
                version: String, deviceName: String? = nil, site: URL = AccountSite.url,
                now: @escaping () -> Date = Date.init) {
        self.store = store
        self.http = http
        self.web = web
        self.apple = apple
        self.prefs = prefs
        self.version = String(version.unicodeScalars.filter { $0.value >= 0x20 && $0.value < 0x7F }.prefix(32).map(Character.init))
        self.deviceName = deviceName.map { String($0.prefix(60)) }
        self.site = site
        self.now = now
        if prefs.accountDeviceId == nil {
            // a new install: nothing an earlier install left in this Keychain service is used
            store.deleteAll()
            prefs.accountDeviceId = Pkce.random(16)
        }
        if let data = store.get(Self.item) {
            if let s = try? JSONDecoder().decode(SavedAccount.self, from: data) {
                saved = s
                account = s.account
            } else {
                store.delete(Self.item)
            }
        }
    }

    /// The Keychain item as this model writes it (the tests' and the UI tests' stand-in accounts are made with it).
    public static func savedItem(_ account: AccountInfo, accessToken: String, accessExpiresAt: Date, refreshToken: String) -> Data {
        (try? JSONEncoder().encode(SavedAccount(account: account, accessToken: accessToken,
                                                accessExpiresAt: accessExpiresAt.timeIntervalSince1970, refreshToken: refreshToken))) ?? Data()
    }

    /// The device as the site records it (spec: id 16..64 [A-Za-z0-9_-], platform, version <= 32, name <= 60).
    private var device: [String: Any] {
        var d: [String: Any] = ["id": prefs.accountDeviceId ?? "", "platform": "ios", "version": version]
        if let deviceName, !deviceName.isEmpty { d["name"] = deviceName }
        return d
    }

    // ---- signing in ---------------------------------------------------------------------------------------------------

    /// Continue with Google: the site's sign-in in the system's web sheet, then the tokens. Continue with Apple: the
    /// system's own Sign in with Apple when it can show it, else the same web sheet.
    @discardableResult
    public func signIn(_ provider: SignInProvider) -> Task<Void, Never> {
        Task { [self] in
            guard phase == .idle else { return }
            problem = nil
            if provider == .apple, let apple {
                phase = .apple
                let nonce = Pkce.random(32)
                let result = await apple.authorize(nonceHash: Self.sha256hex(nonce))
                switch result {
                case .cancelled:
                    phase = .idle
                    problem = Note("signin_cancelled")
                    return
                case .credential(let token, let name):
                    phase = .finishing
                    await appleExchange(token: token, nonce: nonce, name: name)
                    phase = .idle
                    return
                case .failed:
                    AppLog.i("sign-in: the system's Sign in with Apple couldn't show; the web sheet instead")
                    phase = .idle
                }
            }
            await webSignIn(provider)
        }
    }

    /// The site's sign-in in the system's web sheet (shares Safari's sign-ins), then the tokens.
    private func webSignIn(_ provider: SignInProvider) async {
        guard phase == .idle, let web else { return }
        let verifier = Pkce.random(32)
        let state = Pkce.random(16)
        var c = URLComponents(url: site.appendingPathComponent("api/app/start"), resolvingAgainstBaseURL: false)!
        c.queryItems = [
            URLQueryItem(name: "provider", value: provider.rawValue),
            URLQueryItem(name: "platform", value: "ios"),
            URLQueryItem(name: "redirect", value: AccountSite.redirect),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: Pkce.challenge(verifier)),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
        ]
        phase = .browser
        let result = await web.open(c.url!, callbackScheme: AccountSite.callbackScheme)
        defer { phase = .idle }
        switch result {
        case .cancelled: problem = Note("signin_cancelled")
        case .failed: problem = Note("signin_failed")
        case .callback(let url):
            let q = Self.query(url)
            // a callback that isn't this sign-in's (another state, another place) is ignored: nothing changes
            guard url.scheme == AccountSite.callbackScheme, url.host == "callback",
                  let got = q["state"], Self.same(got, state) else {
                AppLog.i("sign-in: a callback that isn't this sign-in's was ignored")
                return
            }
            if let e = q["error"] {
                problem = Note(e == "cancelled" ? "signin_cancelled" : e == "no_email" ? "signin_no_email" : "signin_failed")
                return
            }
            guard let code = q["code"], !code.isEmpty else { problem = Note("signin_failed"); return }
            phase = .finishing
            await exchange(code: code, verifier: verifier)
        }
    }

    /// Apple's identity token and the raw nonce to the site, which checks them and answers as the code grant does.
    private func appleExchange(token: String, nonce: String, name: String?) async {
        let gen = generation
        var body: [String: Any] = ["identity_token": token, "nonce": nonce, "device": device]
        if let name, !name.isEmpty { body["name"] = String(name.prefix(200)) }
        let reply = await post("api/app/apple", body, bearer: nil)
        guard gen == generation else { return }
        switch reply {
        case .offline: problem = Note("signin_unreachable")
        case .answered(let status, let data):
            if status == 200, let t = Self.tokens(data) {
                keep(t, fresh: true)
            } else if Self.error(data) == "no_email" {
                problem = Note("signin_no_email")
            } else {
                problem = Note(status >= 500 || status == 429 ? "signin_not_answering" : "signin_failed")
            }
        }
    }

    /// SHA-256 of `text`, lowercase hex: what goes into Apple's request as the nonce (Apple's documented form).
    static func sha256hex(_ text: String) -> String {
        SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    private func exchange(code: String, verifier: String) async {
        let gen = generation
        let reply = await post("api/app/token", ["grant_type": "authorization_code", "code": code, "code_verifier": verifier,
                                                  "device": device], bearer: nil)
        guard gen == generation else { return }
        switch reply {
        case .offline: problem = Note("signin_unreachable")
        case .answered(let status, let body):
            if status == 200, let t = Self.tokens(body) {
                keep(t, fresh: true)
            } else {
                problem = Note(status >= 500 || status == 429 ? "signin_not_answering" : "signin_failed")
            }
        }
    }

    /// Keeps a token answer: the Keychain first (a sign-in that can't be kept didn't happen).
    private func keep(_ t: TokenAnswer, fresh: Bool) {
        let s = SavedAccount(account: t.account, accessToken: t.accessToken,
                             accessExpiresAt: now().timeIntervalSince1970 + Double(t.expiresIn), refreshToken: t.refreshToken)
        do {
            try store.set(Self.item, try JSONEncoder().encode(s))
        } catch {
            AppLog.i("sign-in: the Keychain refused the account")
            problem = Note("signin_failed")
            return
        }
        saved = s
        if account != s.account { account = s.account }
        checkFailed = false
        guard fresh else { return }
        generation += 1
        problem = nil
        let last = store.get(Self.lastItem).flatMap { String(data: $0, encoding: .utf8) }
        try? store.set(Self.lastItem, Data(s.account.id.utf8))
        onSignedIn?(s.account, last != nil && last != s.account.id)
    }

    // ---- the tokens ---------------------------------------------------------------------------------------------------

    enum Refresh: Equatable { case ok, offline, down, signedOut }

    /// The app opened or came back: a refresh when the access token is about to end (none otherwise).
    public func check() async {
        guard let s = saved, s.accessExpiresAt - now().timeIntervalSince1970 < Self.refreshMargin else { return }
        _ = await refresh()
    }

    /// One refresh at a time: a second caller waits for the first one's answer.
    func refresh() async -> Refresh {
        if let r = refreshing { return await r.value }
        let t = Task { await self.refreshNow() }
        refreshing = t
        let r = await t.value
        refreshing = nil
        return r
    }

    private func refreshNow() async -> Refresh {
        guard let s = saved else { return .signedOut }
        let gen = generation
        let reply = await post("api/app/token", ["grant_type": "refresh_token", "refresh_token": s.refreshToken, "device": device],
                               bearer: nil)
        guard gen == generation, saved != nil else { return saved == nil ? .signedOut : .ok }
        switch reply {
        case .offline:
            // never a lockout: FERB keeps the person signed in and tries again later
            checkFailed = true
            return .offline
        case .answered(let status, let body):
            if status == 200, let t = Self.tokens(body) {
                keep(t, fresh: false)
                return .ok
            }
            if status == 401, Self.error(body) == "invalid_grant" {
                AppLog.i("sign-in: the site ended this sign-in")
                signedOutBySite()
                return .signedOut
            }
            checkFailed = true
            return .down
        }
    }

    /// An access token to use now: refreshed when under a minute is left (or `force`), the old one while it still works
    /// when the site can't be reached; nil when there is none.
    private func accessToken(force: Bool) async -> String? {
        guard let s = saved else { return nil }
        let left = s.accessExpiresAt - now().timeIntervalSince1970
        if !force && left >= Self.refreshMargin { return s.accessToken }
        switch await refresh() {
        case .ok: return saved?.accessToken
        case .signedOut: return nil
        case .offline, .down: return !force && left > 0 ? s.accessToken : nil
        }
    }

    enum Bearer { case answered(Int, Data), offline, signedOut }

    /// A call with the account's bearer token; an `invalid_token` answer refreshes once and tries again.
    private func bearer(_ path: String, _ body: [String: Any]) async -> Bearer {
        for attempt in 0..<2 {
            guard let token = await accessToken(force: attempt == 1) else { return saved == nil ? .signedOut : .offline }
            switch await post(path, body, bearer: token) {
            case .offline: return .offline
            case .answered(let status, let data):
                if status == 401 && attempt == 0 { continue }
                if status == 401 {
                    // refused again after a refresh: the site no longer knows this sign-in
                    AppLog.i("sign-in: the site refused the account's token after a refresh")
                    signedOutBySite()
                    return .signedOut
                }
                return .answered(status, data)
            }
        }
        return .offline
    }

    // ---- what the phone asks with the account ---------------------------------------------------------------------------

    /// The server's signed word of this account for this phone's link id (PHN-828): the pairing carries it to the Mac.
    public func assertion(endpoint: String) async -> AssertionResult {
        switch await bearer("api/app/assertion", ["endpoint": endpoint]) {
        case .offline: return .offline
        case .signedOut: return .signedOut
        case .answered(let status, let data):
            if status == 200, let a = Self.json(data)?["assertion"] as? String, !a.isEmpty { return .ok(a) }
            return .down
        }
    }

    /// Sign out (kb PHN-843): the site is told first, so this sign-in's refresh token family is revoked there (bounded
    /// by the request's timeout), then everything here is cleared whatever it answered, so nobody is ever left signed
    /// in. When the site couldn't be told, the sign-in page says so.
    public func signOut() async {
        guard let s = saved else { return }
        let reply = await post("api/app/signout", ["refresh_token": s.refreshToken],
                               bearer: s.accessExpiresAt > now().timeIntervalSince1970 ? s.accessToken : nil)
        clearLocal()
        if case .answered(200, _) = reply { return }
        problem = Note("signout_unreachable")
    }

    /// Delete account: true once the site deleted it (then everything here is cleared); false leaves all as it was.
    public func delete() async -> Bool {
        switch await bearer("api/app/delete", [:]) {
        case .signedOut, .offline: return false
        case .answered(let status, let data):
            guard status == 200, Self.json(data)?["deleted"] as? Bool == true else { return false }
            clearLocal()
            return true
        }
    }

    private func signedOutBySite() {
        clearLocal()
        onSignedOutBySite?()
    }

    /// Signed out here: the account item goes (the last account's id stays, to tell a different one next time).
    public func clearLocal() {
        store.delete(Self.item)
        saved = nil
        account = nil
        generation += 1
        checkFailed = false
        problem = nil
    }

    // ---- the wire -------------------------------------------------------------------------------------------------------

    enum Reply { case answered(Int, Data), offline }

    private func post(_ path: String, _ body: [String: Any], bearer: String?) async -> Reply {
        var r = URLRequest(url: site.appendingPathComponent(path))
        r.httpMethod = "POST"
        r.setValue("application/json", forHTTPHeaderField: "content-type")
        r.setValue("application/json", forHTTPHeaderField: "accept")
        if let bearer { r.setValue("Bearer \(bearer)", forHTTPHeaderField: "authorization") }
        r.httpBody = try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
        do {
            let (status, data) = try await http.send(r)
            return .answered(status, data)
        } catch {
            return .offline
        }
    }

    struct TokenAnswer {
        let accessToken: String
        let expiresIn: Int
        let refreshToken: String
        let account: AccountInfo
    }

    static func json(_ d: Data) -> [String: Any]? { (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] }

    static func error(_ d: Data) -> String? { json(d)?["error"] as? String }

    static func tokens(_ d: Data) -> TokenAnswer? {
        guard let o = json(d), let access = o["access_token"] as? String, let refresh = o["refresh_token"] as? String,
              let expires = o["expires_in"] as? Int, let a = o["account"] as? [String: Any],
              let id = a["id"] as? String, let email = a["email"] as? String, !access.isEmpty, !refresh.isEmpty else { return nil }
        return TokenAnswer(accessToken: access, expiresIn: expires, refreshToken: refresh,
                           account: AccountInfo(id: id, email: email, name: a["name"] as? String,
                                                providers: (a["providers"] as? [String]) ?? []))
    }

    static func query(_ url: URL) -> [String: String] {
        var out: [String: String] = [:]
        for i in URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? [] where out[i.name] == nil {
            out[i.name] = i.value ?? ""
        }
        return out
    }

    /// Compares the states in constant time.
    static func same(_ a: String, _ b: String) -> Bool {
        let x = Array(a.utf8), y = Array(b.utf8)
        guard x.count == y.count else { return false }
        var d: UInt8 = 0
        for i in 0..<x.count { d |= x[i] ^ y[i] }
        return d == 0
    }

    /// "with Google", "with Apple", "with Google and Apple".
    public static func providerWords(_ providers: [String]) -> String {
        let g = providers.contains("google"), a = providers.contains("apple")
        return L.s(g && a ? "account_with_both" : a ? "account_with_apple" : "account_with_google")
    }
}
