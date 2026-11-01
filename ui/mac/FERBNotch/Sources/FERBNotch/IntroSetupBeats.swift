import Foundation
import NotchCore

private typealias Pick = IntroCopy.Pick

/// The tour's setup beats (notes/setup-intro.md steps 2–5), through SetupModel's own actions: the same sign-in,
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
        await setup.refreshPersona()
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
            let answer = await waitForSignIn({ [setup] in IntroCopy.codexWaiting(setup.aiMessage) },
                                             active: { [setup] in setup.signingIn })
            if case .choice(Pick.cancel)? = answer { setup.cancelSignIn() }
            return true
        case Pick.claude:
            await setup.signInWithClaude()
            while live {
                let answer = await waitForSignIn({ [setup] in IntroCopy.claudeWaiting(setup.aiMessage) },
                                                 active: { [setup] in setup.claudeSigningIn })
                switch answer {
                case .choice(Pick.claudePage)?:
                    setup.openClaudeSignInPage()
                case .choice(Pick.claudeCode)?:
                    if case .entry(let code)? = await ask(IntroCopy.claudeCode) { await setup.sendClaudeCode(code) }
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
            say(IntroCopy.checking(nil))
            await setup.saveAPIKey(provider, key: secret)
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

    // MARK: 3. Allow all, guided

    /// Every required permission in Allow all's order (SetupModel.allowAllSequence), each confirmed by the system:
    /// list panes with the orb docked beside System Settings, prompts with one line before macOS's own question.
    /// A denied prompt keeps the beat open, offering its pane. The beat is the resume point, so a grant that needs a
    /// restart ("Quit & Reopen") comes back here.
    func permissions() async {
        // Nothing is "everything" before the daemon has said which grants it has: wait for its answer.
        await setup.refreshPermissions()
        while live && setup.permissions == nil {
            say(IntroCopy.gettingReady)
            await dwell(Timing.poll)
            await setup.refreshPermissions()
        }
        guard live else { return }
        if !SetupModel.allowAllOrder(setup.setupRows).isEmpty {
            guard await ask(IntroCopy.permissions) != nil else { return }
            var banner: Set<String> = []            // asked, its macOS banner not answered yet: nothing waits on it
            while live, let id = SetupModel.allowAllOrder(setup.setupRows).first(where: { !banner.contains($0) })
                    ?? SetupModel.allowAllOrder(setup.setupRows).first,
                  let row = setup.allRows.first(where: { $0.id == id }) {
                if banner.contains(id) {
                    await waitForBanner(row)
                    banner.remove(id)
                } else if SetupModel.answeredInPopup.contains(id) {
                    if !(await promptRow(row)) { banner.insert(id) }
                } else {
                    await listRow(row)
                }
            }
        }
        guard live else { return }
        say(IntroCopy.allAllowed)
        await dwell(Timing.line)
    }

    private func listRow(_ row: PermissionRow) async {
        sideChoices = [Pick.showInFinder: { [weak self] in self?.setup.showAgentApp() },
                       Pick.restartToCheck: { [weak self] in Task { await self?.setup.restartService() } }]
        defer { sideChoices = [:] }
        say(IntroCopy.listPane(row))
        let target = setup.permissions?.grantTarget ?? ""
        let granted = await setup.grantAndConfirm(row) { [weak self] in
            guard let self, self.live else { return }
            self.orb.dockBesideSettings(dragging: target.isEmpty ? nil : URL(fileURLWithPath: target),
                                        instruction: IntroCopy.dragMe)
        }
        guard granted, live else { return }
        orb.hop()
        await orb.flyHome()
    }

    /// macOS's own question, then its answer: true once on. A denial offers the row's pane and waits there for the
    /// switch. False only for a banner still unanswered (notifications), which the beat comes back to last.
    private func promptRow(_ row: PermissionRow) async -> Bool {
        say(IntroCopy.prompt(row))
        let banner = row.id == "notifications" && row.status == .notDetermined
        let granted = await setup.grantAndConfirm(row, polls: banner ? 2 : nil)
        if !granted && banner && current(row).status == .notDetermined { return false }
        if !granted { await untilOn(row) }
        return true
    }

    /// Off after macOS asked: its pane, and a wait there until the system reports it on.
    private func untilOn(_ row: PermissionRow) async {
        var granted = false
        while live && !granted {
            guard await ask(IntroCopy.denied(row)) != nil else { return }
            say(IntroCopy.waitingInSettings(row))
            granted = await setup.grantAndConfirm(current(row))
        }
    }

    private func waitForBanner(_ row: PermissionRow) async {
        say(IntroCopy.banner(row))
        while live && current(row).status == .notDetermined {
            await dwell(Timing.poll)
            await setup.refreshPermissions()
        }
        if live && current(row).status != .granted { await untilOn(row) }
    }

    private func current(_ row: PermissionRow) -> PermissionRow {
        setup.allRows.first { $0.id == row.id } ?? row
    }

    // MARK: 4. Pair browsers

    /// Pair all (`setup.browsers_pair_all`): every profile at once, on the owner's click.
    func browsers() async {
        var line = IntroCopy.browsers
        while live {
            guard case .choice(Pick.pairAll)? = await ask(line) else { return }
            say(IntroCopy.pairing)
            // The orb waits at a paired browser's front window, where its own "turn FERB on" prompt appears.
            await setup.refreshBrowsers()
            let labels = setup.browsers?.browsers.map(\.label) ?? []
            let point = labels.isEmpty ? nil : apps.frontWindow(of: labels)
            let trip = point.map { point in
                Task { [orb] in await orb.launch(to: point, instruction: IntroCopy.pairingOrb) }
            }
            await setup.pairAll()
            if let trip {
                await trip.value
                await orb.flyHome()
            }
            guard live else { return }
            if let why = setup.message, !why.isEmpty {
                line = IntroCopy.notPaired(why)
                continue
            }
            say(IntroCopy.paired)
            await dwell(Timing.line)
            return
        }
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
                await setup.refreshIdentity()
                guard live else { return }
                let found = setup.identity?.candidates.filter(\.only) ?? []
                guard !found.isEmpty else { line = IntroCopy.identityNotFound; continue }
                guard case .choice(let picked)? = await ask(IntroCopy.identityProfiles(found)),
                      let index = found.indices.first(where: { Pick.profile($0) == picked }) else { return }
                say(IntroCopy.identityPairing)
                await setup.useIdentity(found[index])
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
