import CryptoKit
import FerbAppModel
import FerbCore
import FerbMobile
import Foundation
import XCTest

/// FERB's site for the account's tests: each request recorded (path, JSON body, bearer), answered by `answer` (by
/// path) or by the scripts queued per path. `offline` throws as URLSession does with no network. Never the network.
final class FakeHttp: HttpClient, @unchecked Sendable {
    struct Sent {
        let path: String
        let body: [String: Any]
        let bearer: String?
    }

    private let l = NSLock()
    private var _sent: [Sent] = []
    private var scripts: [String: [(Int, String)]] = [:]
    var offline = false
    /// Holds the answers until let go (one refresh at a time).
    var held = false

    var sent: [Sent] { l.lock(); defer { l.unlock() }; return _sent }
    func paths() -> [String] { sent.map(\.path) }

    /// The next answers for `path`, in turn (then 503; the assertion: an invented one).
    func script(_ path: String, _ answers: (Int, String)...) {
        l.lock(); scripts[path, default: []] += answers; l.unlock()
    }

    func send(_ request: URLRequest) async throws -> (status: Int, body: Data) {
        let path = request.url?.path ?? ""
        let body = request.httpBody.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        let auth = request.value(forHTTPHeaderField: "authorization")
        l.lock(); _sent.append(Sent(path: path, body: body, bearer: auth)); l.unlock()
        while held { try await Task.sleep(nanoseconds: 5_000_000) }
        if offline { throw URLError(.notConnectedToInternet) }
        l.lock(); defer { l.unlock() }
        guard var queue = scripts[path], !queue.isEmpty else {
            // the pairing's assertion works unless a test says otherwise; everything else unscripted is 503
            if path == "/api/app/assertion" { return (200, Data(#"{"assertion":"fpa1.invented.default","expires_in":300}"#.utf8)) }
            return (503, Data(#"{"error":"unavailable"}"#.utf8))
        }
        let (status, text) = queue.removeFirst()
        scripts[path] = queue
        return (status, Data(text.utf8))
    }

    static func tokens(_ access: String, _ refresh: String, id: String = Rig.someone.id, email: String = "someone@example.com",
                       providers: String = #"["google"]"#) -> String {
        #"{"access_token":"\#(access)","token_type":"Bearer","expires_in":3600,"refresh_token":"\#(refresh)","account":{"id":"\#(id)","email":"\#(email)","name":null,"providers":\#(providers)}}"#
    }
}

/// The web sign-in sheet for the tests: what it was opened on, and how it ends (`end` sees the start page's URL).
@MainActor
final class FakeWeb: WebSignIn {
    var opened: [URL] = []
    var end: (URL) -> WebSignInResult = { _ in .cancelled }

    func open(_ url: URL, callbackScheme: String) async -> WebSignInResult {
        opened.append(url)
        return end(url)
    }

    /// Ends with the site's callback for this sign-in (its own state) and `code`.
    func finishes(code: String) {
        end = { url in
            let state = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "state" }?.value ?? ""
            return .callback(URL(string: "ferb-auth://callback?code=\(code)&state=\(state)")!)
        }
    }
}

/// Apple's sheet held open until the test lets it go.
@MainActor
final class HeldApple: AppleSignIn {
    private var held: CheckedContinuation<AppleSignInResult, Never>?
    func authorize(nonceHash: String) async -> AppleSignInResult { await withCheckedContinuation { held = $0 } }
    func release(_ result: AppleSignInResult) {
        held?.resume(returning: result)
        held = nil
    }
}

/// The system's Sign in with Apple for the tests: the nonce hashes it was asked with, and how it ends.
@MainActor
final class FakeApple: AppleSignIn {
    var asked: [String] = []
    var end: AppleSignInResult = .cancelled
    func authorize(nonceHash: String) async -> AppleSignInResult {
        asked.append(nonceHash)
        return end
    }
}

/// FERB's account on the phone (owner 2026-11-24: "required. google and apple."): the gate, the sign-in, the tokens'
/// place and refresh, phone first, the pairing's account and signing out.
@MainActor
final class AccountTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }
    private var http: FakeHttp { rig.http }

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func query(_ url: URL) -> [String: String] {
        Dictionary(uniqueKeysWithValues: (URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []).map { ($0.name, $0.value ?? "") })
    }

    // decision: PHN-839, PHN-8
    func testSignedOutTheSignInShowsAfterTheLockAndNothingElse() async {
        rig.account.clearLocal()
        phone.state = .starting
        let vm = rig.model()
        XCTAssertEqual(vm.ui.gate, .locked, "the lock comes first")
        rig.session.unlocked()
        await eventually("the sign-in while the phone starts") { vm.ui.gate == .signIn }
        // whatever the phone and the link say, nothing of FERB's shows while signed out
        phone.state = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: nil)
        for link in [FakePhone.status(), FakePhone.status(.idle, paired: false), FakePhone.status(.pairingWaitForMac, paired: false, code: "482 719")] {
            phone.link = link
            await settle()
            XCTAssertEqual(vm.ui.gate, .signIn)
        }
        phone.state = .failed(code: "store", detail: "invented")
        await settle()
        XCTAssertEqual(vm.ui.gate, .signIn)
        phone.state = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: nil)
        phone.link = FakePhone.status()
        rig.session.lockNow()
        await eventually("locked again") { vm.ui.gate == .locked }

        // signing in opens the app as it is
        rig.session.unlocked()
        await eventually("sign-in") { vm.ui.gate == .signIn }
        rig.web.finishes(code: "invented-code")
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_a", "fr1_a")))
        await rig.account.signIn(.google).value
        await eventually("main") { vm.ui.gate == .main }
        XCTAssertEqual(vm.ui.note?.key, "signin_signed_in_as")
        XCTAssertEqual(vm.ui.note?.text, "You're signed in as someone@example.com.")
    }

    // decision: PHN-839
    func testTheSignInUsesPKCEAndItsOwnStateAndKeepsTokensOnlyInTheSecureStore() async throws {
        rig.account.clearLocal()
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("sign-in") { vm.ui.gate == .signIn }
        rig.web.finishes(code: "invented-code")
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_secret-access", "fr1_secret-refresh")))
        await rig.account.signIn(.apple).value

        let start = try XCTUnwrap(rig.web.opened.first)
        XCTAssertEqual(start.host, "ferb.kartikey.xyz")
        XCTAssertEqual(start.path, "/api/app/start")
        let q = query(start)
        XCTAssertEqual(q["provider"], "apple", "without the system's Sign in with Apple, Apple through the web flow")
        XCTAssertEqual(q["platform"], "ios")
        XCTAssertEqual(q["redirect"], "ferb-auth://callback")
        XCTAssertEqual(q["code_challenge_method"], "S256")
        XCTAssertGreaterThanOrEqual(q["state"]?.count ?? 0, 16)
        let token = try XCTUnwrap(http.sent.first { $0.path == "/api/app/token" })
        XCTAssertEqual(token.body["grant_type"] as? String, "authorization_code")
        XCTAssertEqual(token.body["code"] as? String, "invented-code")
        let verifier = try XCTUnwrap(token.body["code_verifier"] as? String)
        XCTAssertEqual(verifier.count, 43, "32 random bytes, base64url")
        XCTAssertEqual(q["code_challenge"], Pkce.base64url(Data(SHA256.hash(data: Data(verifier.utf8)))))
        let device = try XCTUnwrap(token.body["device"] as? [String: Any])
        XCTAssertEqual(device["platform"] as? String, "ios")
        XCTAssertEqual(device["id"] as? String, "test-device-000001")

        XCTAssertEqual(rig.account.account?.email, "someone@example.com")
        let item = String(data: try XCTUnwrap(rig.accountStore.get("account")), encoding: .utf8) ?? ""
        XCTAssertTrue(item.contains("fa1_secret-access") && item.contains("fr1_secret-refresh"), "the tokens are in the secure store")
        // and nowhere else: not in the app's defaults
        let defaults = UserDefaults(suiteName: rig.suite)!.dictionaryRepresentation()
        let text = defaults.map { "\($0.key)=\($0.value)" }.joined(separator: "\n")
        XCTAssertFalse(text.contains("fa1_secret-access"))
        XCTAssertFalse(text.contains("fr1_secret-refresh"))
        XCTAssertFalse(text.contains("someone@example.com"), "nor the account")
        await eventually("main") { vm.ui.gate == .main }
    }

    // decision: PHN-839
    func testACallbackWithAnotherStateIsIgnored() async {
        rig.account.clearLocal()
        let vm = rig.model()
        rig.session.unlocked()
        rig.web.end = { _ in .callback(URL(string: "ferb-auth://callback?code=stolen-code&state=someone-elses-state-000")!) }
        await rig.account.signIn(.google).value
        XCTAssertFalse(http.paths().contains("/api/app/token"), "a code that isn't this sign-in's is never exchanged")
        XCTAssertNil(rig.account.account)
        XCTAssertNil(rig.account.problem)
        XCTAssertEqual(vm.ui.gate, .signIn)

        // the sign-in's own endings say what happened
        rig.web.end = { _ in .cancelled }
        await rig.account.signIn(.google).value
        XCTAssertEqual(rig.account.problem?.text, "Sign-in was cancelled. Try again when you're ready.")
        rig.web.finishes(code: "c")
        http.offline = true
        await rig.account.signIn(.google).value
        XCTAssertEqual(rig.account.problem?.text, "FERB can't reach its sign-in right now. Check your internet connection and try again.")
        http.offline = false
        await rig.account.signIn(.google).value   // nothing scripted: 503
        XCTAssertEqual(rig.account.problem?.text, "FERB's sign-in isn't answering right now. Try again in a minute.")
        XCTAssertNil(rig.account.account)
    }

    /// Continue with Apple is the system's own Sign in with Apple: the nonce's SHA-256 goes to Apple, the raw nonce and
    /// Apple's identity token to the site, and the answer is kept as the code grant's.
    // decision: PHN-841
    func testAppleUsesTheSystemsOwnSignInWithAppleAndTheSiteChecksItsToken() async throws {
        rig.account.clearLocal()
        let apple = FakeApple()
        rig.account.apple = apple
        apple.end = .credential(identityToken: "header.claims.signature", name: "Ada Lovelace")
        http.script("/api/app/apple", (200, FakeHttp.tokens("fa1_apple", "fr1_apple", providers: #"["apple"]"#)))
        await rig.account.signIn(.apple).value

        XCTAssertTrue(rig.web.opened.isEmpty, "no web page")
        let sent = try XCTUnwrap(http.sent.first { $0.path == "/api/app/apple" })
        let nonce = try XCTUnwrap(sent.body["nonce"] as? String)
        XCTAssertEqual(nonce.count, 43, "32 random bytes, base64url")
        let hash = SHA256.hash(data: Data(nonce.utf8)).map { String(format: "%02x", $0) }.joined()
        XCTAssertEqual(apple.asked, [hash], "Apple gets only the nonce's SHA-256")
        XCTAssertEqual(sent.body["identity_token"] as? String, "header.claims.signature")
        XCTAssertEqual(sent.body["name"] as? String, "Ada Lovelace")
        XCTAssertEqual((sent.body["device"] as? [String: Any])?["platform"] as? String, "ios")
        XCTAssertNil(sent.bearer)
        XCTAssertEqual(rig.account.account?.providers, ["apple"])
        let item = String(data: try XCTUnwrap(rig.accountStore.get("account")), encoding: .utf8) ?? ""
        XCTAssertTrue(item.contains("fr1_apple"))
        XCTAssertEqual(rig.account.phase, .idle)
    }

    /// While Apple's own sheet is up the sign-in page says so, not "in your browser".
    // decision: PHN-841
    func testWhileApplesSheetIsUpThePageSaysSo() async {
        rig.account.clearLocal()
        let apple = HeldApple()
        rig.account.apple = apple
        let running = rig.account.signIn(.apple)
        await eventually("Apple's sheet") { rig.account.phase == .apple }
        XCTAssertEqual(rig.account.line, "Finishing sign-in with Apple…")
        apple.release(.cancelled)
        await running.value
        XCTAssertEqual(rig.account.phase, .idle)
    }

    // decision: PHN-841
    func testAppleCancelledIsSaidAndASystemThatCantShowItFallsBackToTheWebSheet() async {
        rig.account.clearLocal()
        let apple = FakeApple()
        rig.account.apple = apple
        apple.end = .cancelled
        await rig.account.signIn(.apple).value
        XCTAssertEqual(rig.account.problem?.text, "Sign-in was cancelled. Try again when you're ready.")
        XCTAssertTrue(rig.web.opened.isEmpty)

        apple.end = .failed
        rig.web.finishes(code: "invented-code")
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_w", "fr1_w")))
        await rig.account.signIn(.apple).value
        XCTAssertEqual(rig.web.opened.count, 1, "the web sheet instead")
        XCTAssertEqual(query(rig.web.opened[0])["provider"], "apple")
        XCTAssertNotNil(rig.account.account)

        // Google never asks the system's Sign in with Apple
        rig.account.clearLocal()
        apple.asked = []
        await rig.account.signIn(.google).value
        XCTAssertTrue(apple.asked.isEmpty)
    }

    // decision: PHN-841
    func testTheSitesAnswersToAppleAreSaidPlainly() async {
        rig.account.clearLocal()
        let apple = FakeApple()
        rig.account.apple = apple
        apple.end = .credential(identityToken: "t", name: nil)
        http.script("/api/app/apple", (400, #"{"error":"no_email"}"#), (401, #"{"error":"invalid_grant"}"#))
        await rig.account.signIn(.apple).value
        XCTAssertEqual(rig.account.problem?.key, "signin_no_email")
        await rig.account.signIn(.apple).value
        XCTAssertEqual(rig.account.problem?.key, "signin_failed")
        await rig.account.signIn(.apple).value   // nothing scripted: 503
        XCTAssertEqual(rig.account.problem?.key, "signin_not_answering")
        http.offline = true
        await rig.account.signIn(.apple).value
        XCTAssertEqual(rig.account.problem?.key, "signin_unreachable")
        XCTAssertNil(rig.account.account)
        XCTAssertFalse(http.sent.contains { $0.body["name"] != nil }, "no name is sent when Apple shares none")
    }

    // decision: PHN-839
    func testARefreshThatCantReachTheSiteKeepsThePersonSignedInAndInvalidGrantSignsOut() async {
        // an access token about to end
        try? rig.accountStore.set("account", AccountModel.savedItem(Rig.someone, accessToken: "fa1_old", accessExpiresAt: Date().addingTimeInterval(20),
                                                                    refreshToken: "fr1_old"))
        let account = AccountModel(store: rig.accountStore, http: http, web: rig.web, prefs: rig.prefs, version: "1")
        http.offline = true
        await account.check()
        XCTAssertNotNil(account.account, "never a lockout")
        XCTAssertTrue(account.checkFailed)
        XCTAssertNotNil(rig.accountStore.get("account"))
        http.offline = false
        http.script("/api/app/token", (503, #"{"error":"unavailable"}"#))
        await account.check()
        XCTAssertNotNil(account.account, "a 5xx keeps the person signed in too")

        // one refresh at a time, then the new pair is kept
        http.held = true
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_new", "fr1_new")))
        http.script("/api/app/assertion", (200, #"{"assertion":"fpa1.a","expires_in":300}"#), (200, #"{"assertion":"fpa1.b","expires_in":300}"#))
        async let one = account.assertion(endpoint: "e1")
        async let two = account.assertion(endpoint: "e1")
        try? await Task.sleep(nanoseconds: 100_000_000)
        http.held = false
        let answers = await [one, two]
        XCTAssertEqual(Set(answers), [.ok("fpa1.a"), .ok("fpa1.b")])
        XCTAssertEqual(http.sent.filter { $0.path == "/api/app/token" && ($0.body["refresh_token"] as? String) == "fr1_old" }.count, 3,
                       "two failed checks, then one refresh for both callers")
        XCTAssertEqual(http.sent.filter { $0.path == "/api/app/assertion" }.map(\.bearer), ["Bearer fa1_new", "Bearer fa1_new"])
        XCTAssertFalse(account.checkFailed)

        // the site ended the sign-in: signed out, the item gone, the sign-in shows
        try? rig.accountStore.set("account", AccountModel.savedItem(Rig.someone, accessToken: "fa1_x", accessExpiresAt: Date().addingTimeInterval(-5),
                                                                    refreshToken: "fr1_revoked"))
        let again = AccountModel(store: rig.accountStore, http: http, web: rig.web, prefs: rig.prefs, version: "1")
        http.script("/api/app/token", (401, #"{"error":"invalid_grant"}"#))
        let vm = AppModel(api: phone, account: again, session: rig.session, prefs: rig.prefs, local: rig.local, screenLock: { true }, corpus: nil)
        rig.session.unlocked()
        await eventually("signed out") { again.account == nil }
        XCTAssertNil(rig.accountStore.get("account"))
        await eventually("the sign-in") { vm.ui.gate == .signIn }
        // a sign-out like Sign out: this phone left its Mac too (kb PHN-828)
        await eventually("the Mac told and forgotten") { phone.calls.contains("forget") }
    }

    /// A call the site refuses again after a refresh: the site no longer knows this sign-in, so it is Sign out's path.
    // decision: PHN-828, PHN-841
    func testACallRefusedAfterARefreshSignsOutAndLeavesTheMac() async {
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        http.script("/api/app/assertion", (401, #"{"error":"invalid_token"}"#), (401, #"{"error":"invalid_token"}"#))
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_second", "fr1_second")))
        let answer = await rig.account.assertion(endpoint: "invented-endpoint")
        XCTAssertEqual(answer, .signedOut)
        XCTAssertEqual(http.sent.filter { $0.path == "/api/app/assertion" }.map(\.bearer), ["Bearer fa1_first", "Bearer fa1_second"],
                       "one refresh, one more try")
        XCTAssertNil(rig.account.account)
        XCTAssertNil(rig.accountStore.get("account"))
        await eventually("the Mac told and forgotten") { phone.calls.contains("forget") }
        await eventually("the sign-in") { vm.ui.gate == .signIn }
    }

    // decision: PHN-841
    func testPhoneFirstSaysWhereFERBRunsThenGoesToPairing() async {
        phone.link = FakePhone.status(.idle, paired: false)
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("the pairing gate") { vm.ui.gate == .pair }
        XCTAssertTrue(vm.ui.macFirst, "signed in, no Mac paired: FERB runs on your Mac")

        vm.choosePair()
        XCTAssertEqual(vm.ui.gate, .pair)
        XCTAssertFalse(vm.ui.macFirst, "I've installed it: the pairing page")

        // a phone that pairs again (its Mac removed it, keys changed) goes straight to pairing
        let other = Rig()
        defer { other.tearDown() }
        other.phone.state = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: .keychainReset)
        other.phone.link = FakePhone.status(.idle, paired: false)
        let vm2 = other.model()
        other.session.unlocked()
        await eventually("pair") { vm2.ui.gate == .pair }
        XCTAssertFalse(vm2.ui.macFirst)
    }

    // decision: PHN-828
    func testPairingCarriesTheAccountsAssertionAndShowsTheMacsRefusal() async throws {
        phone.link = FakePhone.status(.idle, paired: false)
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("pair") { vm.ui.gate == .pair }
        vm.choosePair()

        http.script("/api/app/assertion", (200, #"{"assertion":"fpa1.eyJ2IjoxfQ.c2ln","expires_in":300}"#))
        await vm.pair("FERB-ABC").value
        let asked = try XCTUnwrap(http.sent.first { $0.path == "/api/app/assertion" })
        XCTAssertEqual(asked.body["endpoint"] as? String, "invented-endpoint-1", "the assertion names this phone's own link id")
        XCTAssertEqual(asked.bearer, "Bearer fa1_first")
        XCTAssertEqual(phone.pairedAccounts, ["fpa1.eyJ2IjoxfQ.c2ln"])
        XCTAssertTrue(vm.ui.pairing)

        // offline: no pairing without the account's confirmation
        vm.cancelPairing()
        await eventually("cancelled") { !vm.ui.pairing }
        http.offline = true
        await vm.pair("FERB-ABC").value
        XCTAssertEqual(vm.ui.pairError?.text, "Pairing needs the internet to confirm your account.")
        XCTAssertEqual(phone.pairedAccounts.count, 1, "nothing sent to the Mac")
        http.offline = false

        // the Mac's refusal: its own words, and another account offered
        let words = "This phone is signed in as other@example.com, but FERB on this Mac is signed in as someone@example.com. Sign in with the same account on both."
        http.script("/api/app/assertion", (200, #"{"assertion":"fpa1.x.y","expires_in":300}"#))
        phone.pairResult = .failed(.Refused(detail: words, linkCode: "account_mismatch"))
        await vm.pair("FERB-ABC").value
        XCTAssertEqual(vm.ui.pairError?.text, words)
        XCTAssertTrue(vm.pairAccountRefused)
        // and the same when the link says it after the pairing started
        vm.clearPairError()
        let signedOut = "FERB on this Mac isn't signed in yet. Sign in on the Mac, then scan a new code."
        XCTAssertEqual(AppModel.pairNote("mac_signed_out", signedOut).text, signedOut)
        XCTAssertEqual(AppModel.pairNote("account_unconfirmed", "FERB couldn't confirm which account this phone is signed in to. Check that the phone is online, then scan a new code on the Mac.").key, "err_mac_said")
        phone.pairResult = .ok(())
        http.script("/api/app/assertion", (200, #"{"assertion":"fpa1.x.y","expires_in":300}"#))
        await vm.pair("FERB-ABC").value
        phone.link = FakePhone.status(.stopped, paired: false, problem: LinkProblem(code: "mac_signed_out", detail: signedOut, allow: nil))
        await eventually("the refusal") { vm.ui.pairError != nil }
        XCTAssertEqual(vm.ui.pairError?.text, signedOut)
        XCTAssertTrue(vm.pairAccountRefused)
    }

    // decision: PHN-841
    func testSignOutLeavesTheMacAndTheAccount() async {
        rig.prefs.pairedSince = 1_700_000_000_000
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        http.script("/api/app/signout", (200, #"{"ok":true}"#))
        vm.show(.signOut)
        await vm.signOut().value
        XCTAssertTrue(phone.calls.contains("forget"), "the Mac is told (phone.forget_self) and forgotten here")
        let out = http.sent.first { $0.path == "/api/app/signout" }
        XCTAssertEqual(out?.body["refresh_token"] as? String, "fr1_first", "the site revokes this sign-in")
        XCTAssertNil(rig.account.account)
        XCTAssertNil(rig.accountStore.get("account"))
        XCTAssertNil(vm.sheet)
        await eventually("the sign-in") { vm.ui.gate == .signIn }
    }

    // decision: PHN-841
    func testDeleteAccountNeedsTheSiteThenClearsLikeSignOut() async {
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        vm.show(.deleteAccount)
        await vm.deleteAccount().value
        XCTAssertNotNil(rig.account.account, "a 503: nothing deleted, nothing cleared")
        XCTAssertEqual(vm.ui.accountProblem?.text, "FERB couldn't delete your account just now. Check your connection and try again.")
        XCTAssertFalse(phone.calls.contains("forget"))
        http.script("/api/app/delete", (200, #"{"deleted":true}"#))
        await vm.deleteAccount().value
        XCTAssertNil(rig.account.account)
        XCTAssertTrue(phone.calls.contains("forget"))
        await eventually("the sign-in") { vm.ui.gate == .signIn }
    }

    // decision: PHN-828, PHN-841
    func testADifferentAccountForgetsTheMacPairedUnderTheOtherOne() async {
        rig.prefs.pairedSince = 1_700_000_000_000
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        rig.account.clearLocal()   // the site ended the sign-in
        await eventually("sign-in") { vm.ui.gate == .signIn }
        XCTAssertFalse(phone.calls.contains("forget"), "the same account signing in again keeps its Mac")
        rig.web.finishes(code: "c1")
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_b", "fr1_b")))
        await rig.account.signIn(.google).value
        XCTAssertFalse(phone.calls.contains("forget"))
        rig.account.clearLocal()
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_c", "fr1_c", id: "22222222-2222-4222-8222-222222222222", email: "other@example.com")))
        await rig.account.signIn(.google).value
        await eventually("the other account's Mac forgotten") { phone.calls.contains("forget") }
    }

    // ---- a pairing made before sign-in existed (kb PHN-828) --------------------------------------------------------

    private func binds() -> Int { phone.calls.filter { $0 == "bind" }.count }

    // decision: PHN-828
    func testAPairingMadeSignedInIsTheAccountsAndNeverBinds() async {
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        await eventually("recorded") { rig.prefs.pairedAccount == Rig.someone.id }
        await settle()
        XCTAssertEqual(binds(), 0)
    }

    // decision: PHN-828
    func testALegacyPairingIsBoundOnSignInWithTheAssertion() async {
        rig.prefs.pairedSince = 1_700_000_000_000   // paired by an earlier FERB, before sign-in existed
        rig.account.clearLocal()
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("sign-in") { vm.ui.gate == .signIn }
        XCTAssertEqual(binds(), 0, "nothing before sign-in")
        rig.web.finishes(code: "c")
        http.script("/api/app/token", (200, FakeHttp.tokens("fa1_b", "fr1_b")))
        http.script("/api/app/assertion", (200, #"{"assertion":"fpa1.bind.me","expires_in":300}"#))
        await rig.account.signIn(.google).value
        await eventually("bound") { rig.prefs.pairedAccount == Rig.someone.id }
        XCTAssertEqual(phone.bound, ["fpa1.bind.me"])
        XCTAssertEqual(http.sent.first { $0.path == "/api/app/assertion" }?.body["endpoint"] as? String, "invented-endpoint-1")
        XCTAssertEqual(vm.ui.gate, .main)
        // bound: never again
        rig.session.lockNow()
        await settle()   // the lock and the unlock are two moments
        rig.session.unlocked()
        await settle()
        XCTAssertEqual(binds(), 1)
    }

    // decision: PHN-828
    func testABindingTheMacRefusesShowsItsWordsAndPairsAgain() async {
        rig.prefs.pairedSince = 1_700_000_000_000
        let words = "This phone is signed in as someone@example.com, but FERB on this Mac is signed in as other@example.com. Sign in with the same account on both."
        phone.bindResult = .failed(.Refused(detail: words, linkCode: "account_mismatch"))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("removed") { vm.ui.gate == .removed }
        XCTAssertEqual(vm.ui.removedWords, words, "the Mac's own words")
        XCTAssertNil(rig.prefs.pairedAccount)
        // Pair again: the existing path forgets the Mac, and its words go with it
        await vm.forget().value
        XCTAssertNil(vm.ui.removedWords)
    }

    // decision: PHN-828
    func testBindingWaitsWhileTheMacIsSignedOutOrThePhoneOfflineAndTriesAgainAtTheNextUnlock() async {
        rig.prefs.pairedSince = 1_700_000_000_000
        phone.bindResult = .failed(.Refused(detail: "FERB on this Mac isn't signed in yet. Sign in on the Mac, then scan a new code.",
                                            linkCode: "mac_signed_out"))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("tried") { binds() == 1 }
        await settle()
        XCTAssertEqual(vm.ui.gate, .main, "never unpaired for this")
        XCTAssertNil(vm.ui.removedWords)
        XCTAssertNil(rig.prefs.pairedAccount)
        // the link going away and coming back within the same unlock: not again
        phone.link = FakePhone.status(.waiting, paired: true, path: .offline)
        await settle()
        phone.link = FakePhone.status()
        await settle()
        XCTAssertEqual(binds(), 1, "at most once per unlock")
        // no internet at the next unlock: no assertion, nothing sent to the Mac, nothing changes
        http.offline = true
        rig.session.lockNow()
        await settle()   // the lock and the unlock are two moments
        rig.session.unlocked()
        await eventually("tried again, offline") { http.sent.filter { $0.path == "/api/app/assertion" }.count == 2 }
        await settle()
        XCTAssertEqual(binds(), 1, "nothing reaches the Mac without the assertion")
        XCTAssertEqual(vm.ui.gate, .main)
        // the next unlock with the internet and the Mac signed in: bound
        http.offline = false
        phone.bindResult = .ok(true)
        rig.session.lockNow()
        await settle()   // the lock and the unlock are two moments
        rig.session.unlocked()
        await eventually("bound") { rig.prefs.pairedAccount == Rig.someone.id }
        XCTAssertEqual(binds(), 2)
    }

    // decision: PHN-839
    func testANewInstallNeverUsesAnAccountAnEarlierInstallLeftInTheKeychain() {
        let defaults = UserDefaults(suiteName: "ferb-account-new-\(UUID().uuidString)")!
        let prefs = Prefs(defaults: defaults)
        let store = MemoryStore()
        try? store.set("account", AccountModel.savedItem(Rig.someone, accessToken: "fa1_left", accessExpiresAt: Date().addingTimeInterval(3600),
                                                         refreshToken: "fr1_left"))
        let a = AccountModel(store: store, http: http, prefs: prefs, version: "1")
        XCTAssertNil(a.account)
        XCTAssertTrue(store.names.isEmpty)
        let id = prefs.accountDeviceId ?? ""
        XCTAssertTrue((16...64).contains(id.count) && id.allSatisfy { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }, id)
        XCTAssertEqual(AccountModel(store: store, http: http, prefs: prefs, version: "1").account, nil)
        XCTAssertEqual(prefs.accountDeviceId, id, "one id per install")
    }
}
