import Foundation
import NotchCore

private typealias Pick = IntroCopy.Pick

/// The tour's setup beats (kb area app steps 2–5), through SetupModel's own actions: the same sign-in,
/// grants, pairing and identity paths as the setup window.
extension IntroController {
    // MARK: 2. Sign in

    /// Codex or Claude first, an API key under "Other ways to connect". Once connected, the Mac scan starts in the
    /// background (`setup.persona {}`), so it is ready by Try.
    func signIn() async {
        await setup.refreshAI()
        var line = IntroCopy.signIn
        while live && !Self.connected(setup.ai) {
            guard let picked = await ask(line) else { return }
            let tried = await connect(picked)
            line = tried ? IntroCopy.signInFailed(setup.aiMessage) : IntroCopy.signIn
        }
        guard live else { return }
        await working(IntroCopy.checking(nil)) { await setup.refreshPersona() }
        guard live else { return }
        say(IntroCopy.connected(AISnapshot.label(setup.ai?.current ?? "")))
        await dwell(Timing.line * 2)
    }

    /// One way of connecting, as picked; false when the owner went back without trying.
    private func connect(_ picked: IntroAnswer) async -> Bool {
        guard case .choice(let id) = picked else { return false }
        switch id {
        case Pick.codex:
            setup.signInWithChatGPT()
            let since = Date()
            let answer = await waitForSignIn({ [setup] in IntroCopy.codexWaiting(setup.aiMessage).waiting(since: since) },
                                             active: { [setup] in setup.signingIn })
            if case .choice(Pick.cancel)? = answer { setup.cancelSignIn() }
            return true
        case Pick.claude:
            let since = Date()
            await working(IntroCopy.openingClaude) { await setup.signInWithClaude() }
            while live {
                let answer = await waitForSignIn({ [setup] in
                    IntroCopy.claudeWaiting(setup.aiMessage).waiting(since: since)
                }, active: { [setup] in setup.claudeSigningIn })
                switch answer {
                case .choice(Pick.claudePage)?:
                    setup.openClaudeSignInPage()
                case .choice(Pick.claudeCode)?:
                    if case .entry(let code)? = await ask(IntroCopy.claudeCode) {
                        await working(IntroCopy.checking(nil)) { await setup.sendClaudeCode(code) }
                    }
                case .choice(Pick.cancel)?:
                    await setup.cancelClaudeSignIn()
                    return true
                default:
                    return true
                }
            }
            return true
        case Pick.otherWays:
            guard case .choice(let key)? = await ask(IntroCopy.otherWays),
                  let provider = APIProvider.allCases.first(where: { Pick.key($0) == key }) else { return false }
            // The key goes straight to the Keychain (SetupModel.saveAPIKey); no line ever holds it.
            guard case .entry(let secret)? = await ask(IntroCopy.pasteKey(provider)) else { return false }
            await working(IntroCopy.checking(nil)) { await setup.saveAPIKey(provider, key: secret) }
            return true
        default:
            return false
        }
    }

    /// A sign-in in the browser: polled until connected, or until it has been over for a few polls (its result is in
    /// `aiMessage`), or the owner picks one of the line's choices.
    private func waitForSignIn(_ line: @escaping () -> IntroLine, active: @escaping () -> Bool) async -> IntroAnswer? {
        var idle = 0
        return await wait(showing: line) { [weak self] in
            guard let self else { return true }
            await self.setup.refreshAI()
            if Self.connected(self.setup.ai) { return true }
            idle = active() ? 0 : idle + 1
            return idle > 2
        }
    }

    // MARK: 4. Pair browsers

    /// Pair all (`setup.browsers_pairing`): every profile at once, on the owner's click, shown live in the notch: which
    /// profiles are pairing, each result as it lands (HAND-412). A profile FERB's store page can fix offers Add to
    /// <browser> in the result; the beat then waits for the extension there and offers to pair it (HAND-413).
    func browsers() async {
        var line = IntroCopy.browsers
        var progress = PairProgress()
        while live {
            let picked = await ask(line)
            if case .choice(let id)? = picked, let rowID = Pick.addedRow(id),
               let row = progress.rows.first(where: { $0.id == rowID }) {
                await addAndPair(row, into: &progress)
                guard live else { return }
                line = progress.rows.allSatisfy({ $0.state == .paired }) ? IntroCopy.paired
                    : IntroCopy.notPaired(progress, message: setup.message ?? "")
                if line == IntroCopy.paired { say(line); await dwell(Timing.line); return }
                continue
            }
            guard case .choice(Pick.pairAll)? = picked else { return }
            let since = Date()
            await working(IntroCopy.lookingAtBrowsers) { await setup.refreshBrowsers() }
            guard live else { return }
            say(IntroCopy.pairing(PairProgress()).waiting(since: since))
            // The orb waits at a paired browser's front window, where its own "turn FERB on" prompt appears.
            let labels = setup.browsers?.browsers.map(\.label) ?? []
            let point = labels.isEmpty ? nil : apps.frontWindow(of: labels)
            let trip = point.map { point in
                Task { [orb] in await orb.launch(to: point, instruction: IntroCopy.pairingOrb) }
            }
            await setup.pairAll { [weak self] live in
                guard let self, self.live else { return }
                self.say(IntroCopy.pairing(live).waiting(since: since))
            }
            progress = setup.pairProgress ?? PairProgress()
            if let trip {
                await trip.value
                await orb.flyHome()
            }
            guard live else { return }
            let unsupported = setup.browsers?.unsupported ?? []      // HAND-415: said, never passed over
            if let why = setup.message, !why.isEmpty {     // HAND-411: "Paired." only when every profile is
                line = IntroCopy.noting(unsupported, on: why == SetupModel.noBrowserProfiles ? IntroCopy.noProfiles(why)
                    : IntroCopy.notPaired(progress, message: why))
                continue
            }
            say(IntroCopy.noting(unsupported, on: IntroCopy.paired))
            await dwell(Timing.line)
            return
        }
    }

    /// Add to <browser> on one profile of Pair all's result (HAND-413): FERB's Web Store page opens in that profile
    /// (the owner adds it there), the notch shows the wait live until the extension is there and on, then offers to
    /// pair it; pairing happens only on the owner's Pair (rule 5). Stop waiting or Not now goes back to the result.
    private func addAndPair(_ row: PairProgress.Row, into progress: inout PairProgress) async {
        let profile = BrowserProfileRow(browser: row.browser, folder: row.folder, name: row.name, email: row.email,
                                        state: .notInstalled, ferbIdentity: false, connected: false)
        let opened = await working(IntroCopy.openingStore(row)) { await setup.addExtension(profile, watch: false) }
        guard live, opened else { return }
        let since = Date()
        var added = false
        let answer = await wait(showing: { IntroCopy.waitingForAdd(row, added: added).waiting(since: since) }) {
            [weak self] in
            guard let self else { return true }
            await self.setup.refreshBrowsers()
            let state = self.setup.browserProfile(row.id)?.1.state
            added = state == .installed
            return state == .on || state == .paired
        }
        guard live, answer == nil else { return }                  // Stop waiting, or the tour ended
        if setup.browserProfile(row.id)?.1.state == .paired {
            progress.set(row.id, state: .paired)
            return
        }
        guard case .choice(Pick.pairOne)? = await ask(IntroCopy.readyToPair(row)) else {
            progress.set(row.id, state: .skipped, why: "FERB is on there: pair it with Try again.")
            return
        }
        let on = BrowserProfileRow(browser: row.browser, folder: row.folder, name: row.name, email: row.email,
                                   state: .on, ferbIdentity: false, connected: false)
        await working(IntroCopy.pairingOne(row)) { await setup.pairProfile(on) }
        guard live else { return }
        if let why = setup.message, !why.isEmpty { progress.set(row.id, state: .failed, why: why) }
        else { progress.set(row.id, state: .paired) }
    }

    // MARK: 5. FERB's own email (optional)

    /// One line: set it up now (its address, then the browser profile holding only that account), or later at no
    /// cost; it stays in Settings and FERB asks again when a task first needs it.
    func identity() async {
        guard case .choice(Pick.identityNow)? = await ask(IntroCopy.identity) else { return }
        var line = IntroCopy.identityAddress
        while live {
            switch await ask(line) {
            case .entry(let address)?:
                setup.identityAddress = address
                await working(IntroCopy.identityLooking) { await setup.refreshIdentity() }
                guard live else { return }
                let found = setup.identity?.candidates.filter(\.only) ?? []
                guard !found.isEmpty else { line = IntroCopy.identityNotFound; continue }
                guard case .choice(let picked)? = await ask(IntroCopy.identityProfiles(found)),
                      let index = found.indices.first(where: { Pick.profile($0) == picked }) else { return }
                await working(IntroCopy.identityPairing) { await setup.useIdentity(found[index]) }
                guard live else { return }
                say(IntroCopy.identityResult(setup.identityLine))
                await dwell(Timing.line * 2)
                return
            case .choice(Pick.retry)?:
                line = IntroCopy.identityAddress
            default:
                return
            }
        }
    }
}
