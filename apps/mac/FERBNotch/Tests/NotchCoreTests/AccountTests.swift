import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB's account in FERB.app (APP-1127, APP-1138): required; the tour's account step has no skip and blocks until
/// signed in; the tokens go only to the daemon's `account.save` (the Keychain through the signed helper); a refresh
/// that can't reach the server keeps the owner signed in, and only the server's No signs out.
@MainActor
final class AccountTests: XCTestCase {
    nonisolated static let grant = AccountGrant(tokens: AccountTokens(access: "fa1_access-value", accessExpiresAt: 2_000_000_000,
                                                          refresh: "fr1_refresh-value"),
                                    account: AccountInfo(id: "acct-1", email: "ada@example.com", name: "Ada",
                                                         providers: ["google"]))

    final class FakeServer: AccountServing, @unchecked Sendable {
        var exchanges: [(code: String, verifier: String)] = []
        var refreshes: [String] = []
        var signOuts: [String] = []
        var deletes: [String] = []
        var exchangeReply: AccountReply<AccountGrant> = .ok(AccountTests.grant)
        var refreshReply: AccountReply<AccountGrant> = .ok(AccountTests.grant)
        var deleteReply: AccountReply<Void> = .ok(())
        var refreshDelay: UInt64 = 0
        var readyReply: AccountReply<Bool> = .ok(true)
        /// Answered first, in order, before `refreshReply` / `deleteReply`.
        var refreshReplies: [AccountReply<AccountGrant>] = []
        var deleteReplies: [AccountReply<Void>] = []
        var meReply: AccountReply<AccountInfo> = .ok(AccountTests.grant.account)
        var mes: [String] = []

        func ready(_ provider: AccountProvider) async -> AccountReply<Bool> { readyReply }

        func exchange(code: String, verifier: String, device: AccountDevice) async -> AccountReply<AccountGrant> {
            exchanges.append((code, verifier))
            return exchangeReply
        }
        func refresh(_ refreshToken: String, device: AccountDevice) async -> AccountReply<AccountGrant> {
            refreshes.append(refreshToken)
            if refreshDelay > 0 { try? await Task.sleep(nanoseconds: refreshDelay) }
            return refreshReplies.isEmpty ? refreshReply : refreshReplies.removeFirst()
        }
        func me(accessToken: String) async -> AccountReply<AccountInfo> {
            mes.append(accessToken)
            return meReply
        }
        func signOut(refreshToken: String) async { signOuts.append(refreshToken) }
        func delete(accessToken: String) async -> AccountReply<Void> {
            deletes.append(accessToken)
            return deleteReplies.isEmpty ? deleteReply : deleteReplies.removeFirst()
        }
    }

    /// The daemon's side of `account.*` as FERB.app sees it: what was asked, and one stored item.
    final class AccountRPC: DaemonRPCTransport, @unchecked Sendable {
        var calls: [(String, [String: Any])] = []
        var stored: [String: Any]?
        var answers = true
        /// The next this many `account.save`s fail (the Keychain or the daemon not taking it).
        var failSaves = 0
        func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
            calls.append((method, params))
            guard answers else { throw DaemonRPCError.unavailable }
            if method == "account.save" && failSaves > 0 {
                failSaves -= 1
                throw DaemonRPCError.unavailable
            }
            switch method {
            case "account.get":
                guard let stored else { return ["signed_in": false, "account": NSNull(), "tokens": NSNull()] }
                return ["signed_in": true, "account": stored["account"] as Any, "tokens": stored["tokens"] as Any]
            case "account.save":
                stored = params
                return ["saved": true, "removed_phones": [["id": "p1", "name": "Pixel"]]]
            case "account.clear":
                stored = nil
                return ["cleared": true]
            default: return [:]
            }
        }
        func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
            AsyncThrowingStream { $0.finish() }
        }
    }

    /// The browser step: each run answers with the next scripted result; `.callback` answers come to the loopback
    /// address the start page named, with the state it asked for unless `wrongState`. `endings` is what each browser
    /// tab was told.
    final class FakeWeb: WebSignIn {
        enum Step { case cancel, timeout, fail, code(String), wrongState, wrongPort, error(String) }
        static let back = AccountSite.returnAddress(port: 52345)
        var steps: [Step]
        var opened: [URL] = []
        var endings: [LoopbackReturn.Ending] = []
        var cancels = 0
        init(_ steps: [Step]) { self.steps = steps }
        func run(state: String, start: (URL) -> URL) async -> WebSignInResult {
            let url = start(Self.back)
            opened.append(url)
            let state = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?
                .first { $0.name == "state" }?.value ?? ""
            switch steps.isEmpty ? .cancel : steps.removeFirst() {
            case .cancel: return .cancelled
            case .timeout: return .timedOut
            case .fail: return .failed
            case .code(let code): return .callback(URL(string: "\(Self.back)?code=\(code)&state=\(state)")!)
            case .wrongState: return .callback(URL(string: "\(Self.back)?code=stolen&state=someone-else")!)
            case .wrongPort: return .callback(URL(string: "http://127.0.0.1:1234/callback?code=c&state=\(state)")!)
            case .error(let kind): return .callback(URL(string: "\(Self.back)?error=\(kind)&state=\(state)")!)
            }
        }
        func finish(_ ending: LoopbackReturn.Ending) { endings.append(ending) }
        func cancel() { cancels += 1 }
    }

    func session(_ rpc: AccountRPC, _ server: FakeServer, _ web: FakeWeb? = nil,
                 now: Date = Date(timeIntervalSince1970: 1_900_000_000)) -> AccountSession {
        AccountSession(vault: DaemonAccountVault(rpc: rpc), server: server, web: web,
                       device: AccountDevice(id: "device-0123456789ab", version: "0.28.0"), now: { now },
                       pause: { _ in })
    }

    /// A clock a test moves.
    final class Clock { var now = Date(timeIntervalSince1970: 1_900_000_000) }

    func signedIn(_ rpc: AccountRPC, expires: Double = 0) {
        rpc.stored = ["account": Self.grant.account.json,
                      "tokens": AccountTokens(access: "fa1_old", accessExpiresAt: expires, refresh: "fr1_old").json]
    }

    // MARK: The request and the callback

    // decision: APP-1138
    func testTheStartAddressCarriesOnlyTheChallengeAndTheState() {
        // RFC 7636's own example: the verifier and its S256 challenge
        let request = AccountRequest(verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", state: "s1")
        XCTAssertEqual(request.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        let url = request.startURL(.apple, redirect: AccountSite.returnAddress(port: 52345))
        XCTAssertEqual(url.host, "ferb.kartikey.xyz")
        XCTAssertEqual(url.path, "/api/app/start")
        let query = Dictionary(uniqueKeysWithValues: URLComponents(url: url, resolvingAgainstBaseURL: false)!
            .queryItems!.map { ($0.name, $0.value ?? "") })
        XCTAssertEqual(query, ["provider": "apple", "platform": "mac", "redirect": "http://127.0.0.1:52345/callback",
                               "state": "s1",
                               "code_challenge": "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
                               "code_challenge_method": "S256"])
        XCTAssertFalse(url.absoluteString.contains(request.verifier))
        let made = AccountRequest.make()
        XCTAssertEqual(made.verifier.count, 43)
        XCTAssertNotEqual(made.verifier, AccountRequest.make().verifier)
    }

    // decision: APP-1137, APP-1138
    func testACallbackCountsOnlyAtThisSignInsLoopbackAddressWithItsState() {
        let back = AccountSite.returnAddress(port: 52345)
        XCTAssertEqual(back.absoluteString, "http://127.0.0.1:52345/callback")
        let read = { (text: String, state: String) in AccountCallback.read(URL(string: text)!, state: state, returnAddress: back) }
        XCTAssertEqual(read("http://127.0.0.1:52345/callback?code=abc&state=s1", "s1"), .code("abc"))
        XCTAssertNil(read("http://127.0.0.1:52345/callback?code=abc&state=s1", "s2"))
        XCTAssertNil(read("http://127.0.0.1:52345/callback?code=abc", "s1"))
        XCTAssertNil(read("http://127.0.0.1:52346/callback?code=abc&state=s1", "s1"))
        XCTAssertNil(read("http://localhost:52345/callback?code=abc&state=s1", "s1"))
        XCTAssertNil(read("https://127.0.0.1:52345/callback?code=abc&state=s1", "s1"))
        XCTAssertNil(read("http://127.0.0.1:52345/elsewhere?code=abc&state=s1", "s1"))
        XCTAssertNil(read("http://127.0.0.1:52345/callback/x?code=abc&state=s1", "s1"))
        XCTAssertNil(read("ferb-auth://callback?code=abc&state=s1", "s1"))
        XCTAssertEqual(read("http://127.0.0.1:52345/callback?error=cancelled&state=s1", "s1"), .cancelled)
        XCTAssertEqual(read("http://127.0.0.1:52345/callback?error=failed&state=s1", "s1"), .failed("failed"))
        // decision: REL-453 — the site's own errors keep their word; anything else is failed
        for error in ["no_email", "account_conflict", "provider", "not_ready", "slow_down", "unavailable"] {
            XCTAssertEqual(read("http://127.0.0.1:52345/callback?error=\(error)&state=s1", "s1"), .failed(error))
        }
        XCTAssertEqual(read("http://127.0.0.1:52345/callback?error=whatever&state=s1", "s1"), .failed("failed"))
        XCTAssertEqual(read("http://127.0.0.1:52345/callback?state=s1", "s1"), .failed("failed"))
    }

    // MARK: Where the tokens go

    // decision: APP-1138
    func testTokensGoOnlyToTheDaemonsKeychainPath() async {
        let rpc = AccountRPC()
        let server = FakeServer()
        let web = FakeWeb([.code("one-time")])
        let defaults = scratchDefaults("Account")
        let account = AccountSession(vault: DaemonAccountVault(rpc: rpc), server: server, web: web,
                                     device: AccountDevice.current(defaults, version: "0.28.0"))
        let signedIn = await account.signIn(.google)
        XCTAssertTrue(signedIn)
        XCTAssertEqual(account.state, .signedIn(Self.grant.account))
        XCTAssertEqual(server.exchanges.first?.code, "one-time")
        XCTAssertEqual(server.exchanges.first?.verifier.count, 43)
        // the one place the tokens went: account.save, as one item for the Keychain
        let saves = rpc.calls.filter { $0.0 == "account.save" }
        XCTAssertEqual(saves.count, 1)
        let tokens = saves.first?.1["tokens"] as? [String: Any]
        XCTAssertEqual(tokens?["access_token"] as? String, "fa1_access-value")
        XCTAssertEqual(tokens?["refresh_token"] as? String, "fr1_refresh-value")
        XCTAssertEqual(account.removedPhones, ["Pixel"])
        // nothing secret in the preferences: only the install's id
        let kept = defaults.dictionaryRepresentation().filter { $0.key.hasPrefix("FERB") }
        XCTAssertEqual(Array(kept.keys), [AccountDevice.idKey])
        let everything = "\(defaults.dictionaryRepresentation())"
        XCTAssertFalse(everything.contains("fa1_") || everything.contains("fr1_"))
        // and a token never prints
        XCTAssertFalse("\(Self.grant.tokens)".contains("fa1_"))
        XCTAssertFalse(String(reflecting: Self.grant.tokens).contains("fr1_"))
    }

    // decision: APP-1137, APP-1138
    func testACallbackWithAnotherStateOrPortIsNeverExchanged() async {
        let server = FakeServer()
        let web = FakeWeb([.wrongState, .wrongPort, .cancel, .error("failed"), .error("cancelled"), .timeout, .fail])
        let account = session(AccountRPC(), server, web)
        let first = await account.signIn(.google)
        XCTAssertFalse(first)
        let second = await account.signIn(.google)
        XCTAssertFalse(second)
        XCTAssertTrue(server.exchanges.isEmpty)
        let third = await account.signIn(.google)
        XCTAssertFalse(third)
        XCTAssertEqual(account.line, AccountWords.cancelled)
        let fourth = await account.signIn(.apple)
        XCTAssertFalse(fourth)
        XCTAssertEqual(account.line, AccountWords.signInFailed)
        let fifth = await account.signIn(.apple)
        XCTAssertFalse(fifth)
        XCTAssertEqual(account.line, AccountWords.cancelled)
        let sixth = await account.signIn(.google)
        XCTAssertFalse(sixth)
        XCTAssertEqual(account.line, AccountWords.timedOut)
        let seventh = await account.signIn(.google)
        XCTAssertFalse(seventh)
        XCTAssertEqual(account.line, AccountWords.browserFailed)
        XCTAssertTrue(server.exchanges.isEmpty)
        // every tab that brought a callback was told it didn't work; a cancel, timeout or failure has no tab to tell
        XCTAssertEqual(web.endings, [.failed, .failed, .failed, .cancelled])
    }

    /// The browser's tab says signed in only once FERB has kept the sign-in.
    // decision: APP-1137, APP-1138
    func testTheBrowsersTabIsToldSignedInOnlyOnceTheSignInIsKept() async {
        let server = FakeServer()
        let rpc = AccountRPC()
        let web = FakeWeb([.code("one-time"), .code("one-time"), .code("one-time")])
        let account = session(rpc, server, web)
        server.exchangeReply = .unreachable
        let offline = await account.signIn(.google)
        XCTAssertFalse(offline)
        rpc.answers = false
        server.exchangeReply = .ok(Self.grant)
        let notKept = await account.signIn(.google)
        XCTAssertFalse(notKept)
        XCTAssertEqual(account.line, AccountWords.notKept)
        rpc.answers = true
        let done = await account.signIn(.google)
        XCTAssertTrue(done)
        XCTAssertEqual(web.endings, [.failed, .failed, .signedIn])
        XCTAssertTrue(web.opened.allSatisfy { $0.absoluteString.contains("redirect=http://127.0.0.1:52345/callback") })
    }

    /// The site isn't set up yet (the owner hasn't finished the guide) or can't be reached: FERB says so plainly and
    /// opens no browser; the step stays and the owner can try again.
    // decision: APP-1138
    func testASiteNotSetUpOrUnreachableIsSaidPlainlyAndOpensNoBrowser() async {
        let server = FakeServer()
        let web = FakeWeb([.code("one-time")])
        let account = session(AccountRPC(), server, web)
        server.readyReply = .ok(false)
        let notReady = await account.signIn(.apple)
        XCTAssertFalse(notReady)
        XCTAssertEqual(account.line, "Signing in with Apple isn't ready on FERB's site yet. Try again later.")
        server.readyReply = .unreachable
        let offline = await account.signIn(.google)
        XCTAssertFalse(offline)
        XCTAssertEqual(account.line, AccountWords.offline)
        XCTAssertTrue(web.opened.isEmpty)
        XCTAssertEqual(account.state, .unknown)
        server.readyReply = .ok(true)
        let done = await account.signIn(.google)
        XCTAssertTrue(done)
    }

    // MARK: Refresh

    // decision: APP-1138
    func testARefreshThatCantReachTheServerKeepsTheOwnerSignedIn() async {
        let rpc = AccountRPC()
        rpc.stored = ["account": Self.grant.account.json,
                      "tokens": AccountTokens(access: "fa1_old", accessExpiresAt: 0, refresh: "fr1_old").json]
        let server = FakeServer()
        server.refreshReply = .unreachable
        let account = session(rpc, server)
        var signedOut = 0
        account.onSignedOut = { signedOut += 1 }
        await account.load()
        await account.refresh()
        XCTAssertEqual(server.refreshes, ["fr1_old", "fr1_old"])        // no answer: asked again at once (REL-453)
        XCTAssertTrue(account.signedIn)
        XCTAssertTrue(account.checkTrouble)
        XCTAssertEqual(signedOut, 0)
        XCTAssertNotNil(rpc.stored)                                     // nothing cleared
        server.refreshReply = .failed                                   // a 4xx other than the server's No: the same
        await account.refresh()
        XCTAssertTrue(account.signedIn)

        server.refreshReply = .ok(Self.grant)                           // later it answers: rotated and kept
        await account.refresh()
        XCTAssertFalse(account.checkTrouble)
        XCTAssertEqual((rpc.stored?["tokens"] as? [String: Any])?["refresh_token"] as? String, "fr1_refresh-value")
    }

    // decision: APP-1138
    func testTheServersNoSignsOutAndBringsTheAccountStepBack() async {
        let rpc = AccountRPC()
        rpc.stored = ["account": Self.grant.account.json,
                      "tokens": AccountTokens(access: "fa1_old", accessExpiresAt: 0, refresh: "fr1_old").json]
        let server = FakeServer()
        server.refreshReply = .rejected()
        let account = session(rpc, server)
        var signedOut = 0
        account.onSignedOut = { signedOut += 1 }
        await account.load()
        await account.refresh()
        XCTAssertEqual(account.state, .signedOut)
        XCTAssertEqual(signedOut, 1)
        XCTAssertNil(rpc.stored)
    }

    // decision: APP-1138
    func testOneRefreshAtATime() async {
        let rpc = AccountRPC()
        rpc.stored = ["account": Self.grant.account.json,
                      "tokens": AccountTokens(access: "fa1_old", accessExpiresAt: 0, refresh: "fr1_old").json]
        let server = FakeServer()
        server.refreshDelay = 50_000_000
        let account = session(rpc, server)
        await account.load()
        async let a: Void = account.refresh()
        async let b: Void = account.refresh()
        _ = await (a, b)
        XCTAssertEqual(server.refreshes, ["fr1_old"])                   // a refresh token is never sent twice
    }

    // decision: APP-1138
    func testTheServersAnswersAreClassified() {
        XCTAssertTrue({ if case .ok = HTTPAccountServer.classify(status: 200, body: [:]) { return true }; return false }())
        for status in [401] {
            XCTAssertTrue({ if case .rejected = HTTPAccountServer.classify(status: status, body: [:]) { return true }
                return false }())
        }
        XCTAssertTrue({ if case .rejected = HTTPAccountServer.classify(status: 400, body: ["error": "invalid_grant"]) {
            return true }; return false }())
        // decision: REL-453 — 429 and 5xx each have their own words, never "offline"
        XCTAssertTrue({ if case .slowDown = HTTPAccountServer.classify(status: 429, body: [:]) { return true }
            return false }())
        for status in [500, 503] {
            XCTAssertTrue({ if case .unavailable = HTTPAccountServer.classify(status: status, body: [:]) { return true }
                return false }())
        }
        XCTAssertTrue({ if case .rejected(reason: "account_deleted") = HTTPAccountServer.classify(
            status: 401, body: ["error": "invalid_grant", "reason": "account_deleted"]) { return true }
            return false }())
        XCTAssertTrue({ if case .failed = HTTPAccountServer.classify(status: 400, body: [:]) { return true }
            return false }())
    }

    // MARK: Sign out and delete

    // decision: APP-1138
    func testSignOutAndDelete() async {
        let rpc = AccountRPC()
        rpc.stored = ["account": Self.grant.account.json, "tokens": Self.grant.tokens.json]
        let server = FakeServer()
        let account = session(rpc, server)
        await account.load()
        server.deleteReply = .unreachable
        let failed = await account.deleteAccount()
        XCTAssertFalse(failed)
        XCTAssertEqual(account.line, AccountWords.deleteFailed)
        XCTAssertTrue(account.signedIn)                                 // nothing cleared until the server deleted it
        server.deleteReply = .ok(())
        let deleted = await account.deleteAccount()
        XCTAssertTrue(deleted)
        XCTAssertEqual(server.deletes, ["fa1_access-value", "fa1_access-value"])
        XCTAssertEqual(account.state, .signedOut)
        XCTAssertNil(rpc.stored)

        rpc.stored = ["account": Self.grant.account.json, "tokens": Self.grant.tokens.json]
        await account.load()
        await account.signOut()
        XCTAssertEqual(server.signOuts, ["fr1_refresh-value"])
        XCTAssertEqual(account.state, .signedOut)
        XCTAssertNil(rpc.stored)
    }

    // decision: APP-1138
    func testADaemonThatDoesntAnswerSaysNothingEitherWay() async {
        let rpc = AccountRPC()
        rpc.answers = false
        let account = session(rpc, FakeServer())
        let state = await account.load()
        XCTAssertEqual(state, .unknown)
    }

    // MARK: Each failure in its own words (REL-453)

    // decision: REL-453, APP-1139
    func testEachSignInFailureIsSaidInItsOwnWords() async {
        let server = FakeServer()
        let web = FakeWeb([.error("no_email"), .error("account_conflict"), .error("provider"), .error("not_ready"),
                           .error("slow_down"), .error("unavailable"), .code("c"), .code("c"), .code("c"), .code("c")])
        let account = session(AccountRPC(), server, web)
        for (provider, words) in [(AccountProvider.apple, AccountWords.noEmail(.apple)),
                                  (.google, AccountWords.accountConflict(.google)),
                                  (.google, AccountWords.providerFailed(.google)),
                                  (.apple, AccountWords.notSetUp(.apple)),
                                  (.google, AccountWords.slowDown),
                                  (.google, AccountWords.serverDown)] {
            let done = await account.signIn(provider)
            XCTAssertFalse(done)
            XCTAssertEqual(account.line, words)
        }
        for (reply, words) in [(AccountReply<AccountGrant>.rejected(), AccountWords.codeExpired),
                               (.unavailable, AccountWords.serverDown), (.slowDown, AccountWords.slowDown),
                               (.failed, AccountWords.signInFailed)] {
            server.exchangeReply = reply
            let done = await account.signIn(.google)
            XCTAssertFalse(done)
            XCTAssertEqual(account.line, words)
        }
        XCTAssertEqual(web.endings, Array(repeating: .failed, count: 10))
        for (reply, words) in [(AccountReply<Bool>.unavailable, AccountWords.serverDown),
                               (.slowDown, AccountWords.slowDown), (.unreachable, AccountWords.offline)] {
            server.readyReply = reply
            let done = await account.signIn(.google)
            XCTAssertFalse(done)
            XCTAssertEqual(account.line, words)
        }
        // every distinct case has distinct words
        let all = [AccountWords.noEmail(.apple), AccountWords.accountConflict(.google), AccountWords.providerFailed(.google),
                   AccountWords.notSetUp(.apple), AccountWords.slowDown, AccountWords.serverDown, AccountWords.offline,
                   AccountWords.codeExpired, AccountWords.signInFailed]
        XCTAssertEqual(Set(all).count, all.count)
    }

    // MARK: The site's No: why, and nothing else lost (REL-453)

    /// The site signing this Mac out says why; only the Keychain item goes (the phones stay paired: account.clear
    /// removes none, as the daemon's tests check), and the account step shows the reason.
    // decision: REL-453, APP-1139
    func testTheSitesNoSaysWhy() async {
        for (reason, ended) in [("account_deleted", AccountEnded.deleted), ("expired", .expired),
                                ("signed_out", .ended), (nil, .ended)] as [(String?, AccountEnded)] {
            let rpc = AccountRPC()
            signedIn(rpc)
            let server = FakeServer()
            server.refreshReply = .rejected(reason: reason)
            let account = session(rpc, server)
            await account.load()
            await account.refresh()
            XCTAssertEqual(account.state, .signedOut)
            XCTAssertEqual(account.ended, ended)
            XCTAssertEqual(rpc.calls.map(\.0), ["account.get", "account.clear"], "nothing but the Keychain item")
        }
        XCTAssertEqual(Set([AccountEnded.deleted, .expired, .ended].map(\.words)).count, 3)
        // signing out here says nothing more
        let rpc = AccountRPC()
        signedIn(rpc)
        let account = session(rpc, FakeServer())
        await account.load()
        await account.signOut()
        XCTAssertNil(account.ended)
    }

    /// A refresh with no answer is asked again at once with the same token, inside the site's grace: the same new pair
    /// comes back and nothing signs out.
    // decision: REL-453, APP-1139
    func testARefreshWithNoAnswerIsAskedAgainAtOnce() async {
        let rpc = AccountRPC()
        signedIn(rpc)
        let server = FakeServer()
        server.refreshReplies = [.unreachable, .ok(Self.grant)]
        let account = session(rpc, server)
        await account.load()
        await account.refresh()
        XCTAssertEqual(server.refreshes, ["fr1_old", "fr1_old"])
        XCTAssertFalse(account.checkTrouble)
        XCTAssertEqual((rpc.stored?["tokens"] as? [String: Any])?["refresh_token"] as? String, "fr1_refresh-value")
    }

    /// Rotated tokens the Keychain didn't take stay in memory, win over the spent ones a load reads, and are saved at
    /// the next refresh; the spent refresh token is never sent again.
    // decision: REL-453, APP-1139
    func testRotatedTokensTheKeychainDidntTakeAreKeptAndSavedAgain() async {
        let rpc = AccountRPC()
        signedIn(rpc)
        let server = FakeServer()
        let account = session(rpc, server)
        await account.load()
        rpc.failSaves = AccountSession.saveTries + 1
        await account.refresh()
        XCTAssertTrue(account.checkTrouble)
        XCTAssertEqual((rpc.stored?["tokens"] as? [String: Any])?["refresh_token"] as? String, "fr1_old")
        await account.load()                                            // the Keychain's spent token doesn't win
        XCTAssertEqual((rpc.stored?["tokens"] as? [String: Any])?["refresh_token"] as? String, "fr1_refresh-value")
        XCTAssertFalse(account.checkTrouble)
        await account.refresh(force: true)
        XCTAssertEqual(server.refreshes, ["fr1_old", "fr1_refresh-value"])
    }

    /// Delete account: a refused access token is refreshed and the delete asked once more; success only when the site
    /// confirmed (its delete, or its refresh saying the account is already deleted).
    // decision: REL-453, APP-1139
    func testDeleteNeverReportsSuccessTheSiteDidntConfirm() async {
        func signedInSession(_ server: FakeServer) async -> (AccountSession, AccountRPC) {
            let rpc = AccountRPC()
            rpc.stored = ["account": Self.grant.account.json, "tokens": AccountTokens(
                access: "fa1_old", accessExpiresAt: 2_000_000_000, refresh: "fr1_old").json]
            let account = session(rpc, server)
            await account.load()
            return (account, rpc)
        }
        // the access token had ended: refreshed, then deleted
        let a = FakeServer()
        a.deleteReplies = [.rejected()]
        let (first, _) = await signedInSession(a)
        let deleted = await first.deleteAccount()
        XCTAssertTrue(deleted)
        XCTAssertEqual(a.deletes, ["fa1_old", "fa1_access-value"])
        XCTAssertEqual(a.refreshes, ["fr1_old"])
        // refused twice: not deleted, still signed in
        let b = FakeServer()
        b.deleteReply = .rejected()
        let (second, _) = await signedInSession(b)
        let refused = await second.deleteAccount()
        XCTAssertFalse(refused)
        XCTAssertEqual(second.line, AccountWords.deleteFailed)
        XCTAssertTrue(second.signedIn)
        // the refresh says the account is already deleted: that is the site's confirmation
        let c = FakeServer()
        c.deleteReply = .rejected()
        c.refreshReply = .rejected(reason: "account_deleted")
        let (third, _) = await signedInSession(c)
        let gone = await third.deleteAccount()
        XCTAssertTrue(gone)
        XCTAssertEqual(third.state, .signedOut)
        // the refresh ends the sign-in otherwise: never "deleted"; sign in again first
        let d = FakeServer()
        d.deleteReply = .rejected()
        d.refreshReply = .rejected(reason: "expired")
        let (fourth, _) = await signedInSession(d)
        let notDeleted = await fourth.deleteAccount()
        XCTAssertFalse(notDeleted)
        XCTAssertEqual(fourth.line, AccountWords.deleteNeedsSignIn)
        XCTAssertEqual(fourth.ended, .expired)
    }

    /// FERB.app asks the site at most every five minutes whether its sign-in still stands; a refused access token is
    /// refreshed, so a deleted account elsewhere signs this Mac out with its reason.
    // decision: REL-453, APP-1139
    func testCheckFindsASignOutElsewherePromptly() async {
        let rpc = AccountRPC()
        signedIn(rpc, expires: 2_000_000_000)
        let server = FakeServer()
        let clock = Clock()
        let account = AccountSession(vault: DaemonAccountVault(rpc: rpc), server: server, web: nil,
                                     device: AccountDevice(id: "device-0123456789ab", version: "0.28.0"),
                                     now: { clock.now }, pause: { _ in })
        await account.load()
        await account.check()
        XCTAssertEqual(server.mes, ["fa1_old"])
        XCTAssertTrue(server.refreshes.isEmpty, "a live access token isn't rotated")
        clock.now += AccountSession.checkEvery - 1
        await account.check()
        XCTAssertEqual(server.mes.count, 1, "not again within five minutes")
        clock.now += 1
        server.meReply = .rejected()
        server.refreshReply = .rejected(reason: "account_deleted")
        var signedOut = 0
        account.onSignedOut = { signedOut += 1 }
        await account.check()
        XCTAssertEqual(server.mes.count, 2)
        XCTAssertEqual(account.state, .signedOut)
        XCTAssertEqual(account.ended, .deleted)
        XCTAssertEqual(signedOut, 1)
        // offline: nothing changes
        let quiet = FakeServer()
        quiet.meReply = .unreachable
        let other = AccountRPC()
        signedIn(other, expires: 2_000_000_000)
        let kept = session(other, quiet)
        await kept.load()
        await kept.check()
        XCTAssertTrue(kept.signedIn)
    }

    // MARK: The plan

    // decision: APP-1127, APP-1138
    func testSignedOutAlwaysGetsTheAccountStepFirst() {
        let done = IntroNeeds(setupDone: true, signedIn: true, missingPermissions: [], unpairedBrowsers: false,
                              firstRun: false, account: false)
        XCTAssertEqual(IntroPlan.beats(done, replay: false), [.account])               // a short step, not the tour
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: done), .intro([.account]))
        var grants = done
        grants.missingPermissions = ["accessibility"]
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: grants), .intro([.account, .permissions, .home]))
        var unfinished = done
        unfinished.setupDone = false
        unfinished.signedIn = false
        XCTAssertEqual(IntroPlan.beats(unfinished, replay: false), [.account, .signIn, .home])
        var signedIn = done
        signedIn.account = true
        XCTAssertEqual(IntroPlan.beats(signedIn, replay: false), [])
        XCTAssertEqual(IntroBeat.allCases.prefix(3), [.meet, .account, .signIn])      // right after Meet
        // a quit tour's saved beats keep their meaning: the raw values never moved
        XCTAssertEqual(IntroBeat.signIn.rawValue, 1)
        XCTAssertEqual(IntroBeat.home.rawValue, 7)
        XCTAssertEqual(IntroBeat(rawValue: 8), .account)
        XCTAssertEqual(IntroPlan.next(after: .meet, in: IntroBeat.allCases), .account)
    }
}

extension IntroControllerTests {
    /// The account step blocks: a cancelled sign-in comes back to the same step with its reason, Skip tour is never
    /// offered there, and only a sign-in moves the tour on.
    // decision: APP-1127, APP-1138
    func testTheAccountStepBlocksUntilSignedIn() async {
        let t = tour()
        let rpc = AccountTests.AccountRPC()
        let web = AccountTests.FakeWeb([.cancel, .code("one-time")])
        let account = AccountSession(vault: DaemonAccountVault(rpc: rpc), server: AccountTests.FakeServer(), web: web,
                                     device: AccountDevice(id: "device-0123456789ab", version: "0.28.0"))
        t.controller.account = account
        var finished = 0
        t.controller.onFinished = { finished += 1 }
        t.controller.start(beats: [.account, .home])
        await t.pick(IntroCopy.Pick.accountGoogle)
        await eventually { t.notch.introLine?.text == AccountWords.cancelled }
        XCTAssertEqual(t.controller.beat, .account)
        XCTAssertFalse(t.notch.introSkippable)
        t.notch.introAction(.skipTour)
        await Task.yield()
        XCTAssertTrue(t.controller.running)
        XCTAssertEqual(finished, 0)
        await t.pick(IntroCopy.Pick.accountApple)
        await eventually { finished == 1 }
        XCTAssertTrue(account.signedIn)
        XCTAssertTrue(t.lines.all.contains { $0.text == AccountWords.signedIn("ada@example.com") })
        XCTAssertEqual(web.opened.count, 2)
        XCTAssertTrue(web.opened.last?.absoluteString.contains("provider=apple") == true)
    }

    /// Signed out by the site, the account step says why first (REL-453).
    // decision: REL-453, APP-1139
    func testTheAccountStepSaysWhyTheSiteSignedOut() async {
        let t = tour()
        let rpc = AccountTests.AccountRPC()
        rpc.stored = ["account": AccountTests.grant.account.json,
                      "tokens": AccountTokens(access: "fa1_old", accessExpiresAt: 0, refresh: "fr1_old").json]
        let server = AccountTests.FakeServer()
        server.refreshReply = .rejected(reason: "expired")
        let account = AccountSession(vault: DaemonAccountVault(rpc: rpc), server: server, web: HeldWeb(),
                                     device: AccountDevice(id: "device-0123456789ab", version: "0.28.0"))
        await account.load()
        await account.refresh()
        t.controller.account = account
        t.controller.start(beats: [.account, .home])
        await eventually { t.notch.introLine?.text == AccountWords.endedExpired }
        XCTAssertEqual(t.notch.introLine?.choices.map(\.id), [IntroCopy.Pick.accountGoogle, IntroCopy.Pick.accountApple])
    }

    /// The waiting step's Copy link puts the sign-in page's address on the clipboard and keeps waiting.
    // decision: REL-453, APP-1139
    func testTheAccountStepsWaitCopiesTheLink() async {
        let t = tour()
        let web = HeldWeb()
        let account = AccountSession(vault: DaemonAccountVault(rpc: AccountTests.AccountRPC()), server: AccountTests.FakeServer(),
                                     web: web, device: AccountDevice(id: "device-0123456789ab", version: "0.28.0"))
        var copied: [String] = []
        t.controller.copyText = { copied.append($0) }
        t.controller.account = account
        t.controller.start(beats: [.account, .home])
        await t.pick(IntroCopy.Pick.accountGoogle)
        await eventually { t.notch.introLine?.text == AccountWords.waiting }
        await t.pick(IntroCopy.Pick.copyLink)
        await eventually { t.notch.introLine?.detail == AccountWords.linkCopied }
        XCTAssertEqual(copied.count, 1)
        XCTAssertTrue(copied.first?.hasPrefix("https://ferb.kartikey.xyz/api/app/start?") == true)
        XCTAssertTrue(copied.first?.contains("redirect=http://127.0.0.1:52345/callback") == true)
        XCTAssertEqual(web.cancels, 0, "still waiting")
        await t.pick(IntroCopy.Pick.cancel)
        await eventually { t.notch.introLine?.text == AccountWords.cancelled }
        XCTAssertEqual(web.cancels, 1)
    }

    /// While the browser is open the step says so with Cancel; Cancel stops FERB waiting and the step stays.
    // decision: APP-1137, APP-1138
    func testTheAccountStepsWaitHasCancel() async {
        let t = tour()
        let web = HeldWeb()
        let account = AccountSession(vault: DaemonAccountVault(rpc: AccountTests.AccountRPC()), server: AccountTests.FakeServer(),
                                     web: web, device: AccountDevice(id: "device-0123456789ab", version: "0.28.0"))
        t.controller.account = account
        t.controller.start(beats: [.account, .home])
        await t.pick(IntroCopy.Pick.accountGoogle)
        await eventually { t.notch.introLine?.text == AccountWords.waiting }
        XCTAssertEqual(t.notch.introLine?.choices.map(\.id), [IntroCopy.Pick.copyLink, IntroCopy.Pick.cancel])
        await t.pick(IntroCopy.Pick.cancel)
        await eventually { t.notch.introLine?.text == AccountWords.cancelled }
        XCTAssertEqual(web.cancels, 1)
        XCTAssertEqual(t.controller.beat, .account)
        XCTAssertFalse(account.signedIn)
    }
}

/// A browser that waits until Cancel.
@MainActor
final class HeldWeb: WebSignIn {
    var cancels = 0
    private var held: CheckedContinuation<WebSignInResult, Never>?
    func run(state: String, start: (URL) -> URL) async -> WebSignInResult {
        _ = start(AccountSite.returnAddress(port: 52345))
        return await withCheckedContinuation { held = $0 }
    }
    func finish(_ ending: LoopbackReturn.Ending) {}
    func cancel() {
        cancels += 1
        held?.resume(returning: .cancelled)
        held = nil
    }
}
