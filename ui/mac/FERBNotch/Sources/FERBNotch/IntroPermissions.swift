import Combine
import Foundation
import NotchCore

/// One permission visit, with one request in flight. The checklist stays in place while macOS answers.
@MainActor
private final class PermissionVisit {
    var active: String?
    var requesting = false
    var added = false
    var result: Bool?
    var queue: [String] = []
    var task: Task<Void, Never>?
    var guided = false
    /// The grant macOS last confirmed, said until the next one starts.
    var confirmed: String?
    /// When macOS's notification banner came up (the line stays alive while it is up).
    var bannerSince: Date?
}

extension IntroController {
    /// Every wait here shows what FERB is doing now, alive, with its time (APP-665, APP-679): the first read, each
    /// grant's steps as SetupModel.grantWait publishes them (macOS's question, Settings opening, the owner's switch,
    /// each check, a restart, a silent service started again), and the grant macOS confirmed.
    func permissions() async {
        let since = Date()
        say(IntroCopy.checkingPermissions.waiting(since: since))
        await setup.refreshPermissions()
        while live && setup.permissions == nil {
            say(IntroCopy.gettingReady.waiting(since: since))
            await dwell(Timing.poll)
            await setup.refreshPermissions()
        }
        guard live else { return }
        let visit = PermissionVisit()
        // Back from macOS's Quit & Reopen: the grant FERB was waiting on when it quit, confirmed now, is acknowledged
        // in front of System Settings like any other (APP-687).
        if let waited = progress.pendingGrant {
            progress.pendingGrant = nil
            if setup.setupRows.contains(where: { $0.id == waited && $0.status == .granted }) {
                visit.confirmed = waited
                say(permissionOverview(visit))
                notch.onComeForward?()
            }
        }
        // Each step of a grant's wait shows the moment it starts, not at the next poll (a check or a restart can be
        // shorter than one).
        let steps = setup.$grantWait.dropFirst().removeDuplicates().sink { [weak self] wait in
            guard let self, self.live else { return }
            self.say(self.permissionOverview(visit, wait: wait))
        }
        defer {
            steps.cancel()
            visit.task?.cancel()
            orb.onDropAccepted = nil
            sideChoices = [:]
        }
        orb.onDropAccepted = { [weak self] in
            guard let self, let id = visit.active,
                  let row = self.setup.allRows.first(where: { $0.id == id }) else { return }
            visit.added = true
            self.say(self.permissionOverview(visit))
            self.dockForPermission(row, added: true)
        }
        while live && !permissionsConfirmed {
            if visit.task == nil, !visit.queue.isEmpty {
                let next = visit.queue.removeFirst()
                if let row = setup.allRows.first(where: { $0.id == next && $0.status != .granted }) {
                    requestPermission(row, visit: visit)
                }
            }
            let answer = await wait(showing: { [self] in permissionOverview(visit) }) { [self] in
                if visit.task == nil { await setup.refreshPermissions() }
                return visit.result != nil || permissionsConfirmed
            }
            guard live else { return }
            if let granted = visit.result {
                await visit.task?.value
                visit.task = nil
                visit.result = nil
                if granted && visit.guided { orb.hop() }
                if granted { visit.active = nil; visit.added = false }
            }
            guard case .choice(let id)? = answer else { continue }
            // A system prompt owns the current request until it answers. Buttons are disabled meanwhile.
            guard !visit.requesting else { continue }
            visit.task?.cancel()
            await visit.task?.value
            visit.task = nil
            visit.result = nil
            visit.queue = []
            if id == IntroCopy.Pick.permissionsNext {
                let rows = setup.setupRows
                let pending = SetupModel.allowAllOrder(rows)
                let quick = SetupModel.quickPermissionOrder(rows)
                visit.queue = quick.isEmpty ? Array(pending.prefix(1)) : quick
            } else if let row = setup.setupRows.first(where: { IntroCopy.Pick.permission($0.id) == id }),
                      row.status != .granted && row.blocked.isEmpty {
                visit.queue = [row.id]
            }
        }
        guard live else { return }
        // The last grant's rows can read granted a moment before its own wait returns: let that wait finish, so its
        // confirmation (the line, coming forward) and the hop aren't lost; a wait for anything else is cancelled.
        if visit.result == nil, let active = visit.active,
           setup.setupRows.contains(where: { $0.id == active && $0.status == .granted }) {
            await visit.task?.value
            if visit.result == true && visit.guided { orb.hop() }
        }
        visit.task?.cancel()
        await visit.task?.value
        if visit.guided { await orb.flyHome() }
        guard live else { return }
        say(IntroCopy.allAllowed)
        sound.play(.allAllowed, at: nil)
        await dwell(Timing.line)
    }

    private var permissionsConfirmed: Bool {
        setup.permissions != nil && setup.setupRows.allSatisfy { $0.status == .granted }
    }

    private func requestPermission(_ row: PermissionRow, visit: PermissionVisit) {
        visit.active = row.id
        visit.added = false
        visit.requesting = true
        visit.result = nil
        visit.confirmed = nil
        progress.pendingGrant = row.id                             // a Quit & Reopen may end FERB before it's confirmed
        visit.task = Task { [weak self] in
            guard let self else { return }
            let banner = row.id == "notifications" && row.status == .notDetermined
            let granted = await self.setup.grantAndConfirm(row, polls: banner ? 2 : nil) { [weak self] in
                guard let self, self.live else { return }
                visit.requesting = false
                if !SetupModel.answeredInPopup.contains(row.id) || row.status != .notDetermined {
                    visit.guided = true
                    self.dockForPermission(row, added: false)
                }
            }
            visit.requesting = false
            visit.result = granted
            guard self.live else { return }
            self.progress.pendingGrant = nil
            if granted { visit.confirmed = row.id }                // the system confirmed it (APP-510): said at once
            self.say(self.permissionOverview(visit))
            // ...and seen: the tour comes forward over System Settings to acknowledge it, without the keyboard focus
            // (APP-687); the hop follows in the beat's loop, and the next grant's dock steps behind Settings again.
            if granted { self.notch.onComeForward?() }
        }
    }

    private func dockForPermission(_ row: PermissionRow, added: Bool) {
        let target = setup.permissions?.grantTarget ?? ""
        let isList = !SetupModel.answeredInPopup.contains(row.id)
        let both = Self.bothFERBLists.contains(row.id)
        let switchOn = both ? "Turn both FERB switches on" : "Turn FERB on"
        let instruction = added
            ? IntroLine(text: "Added to the list", detail: row.restartToCheck
                ? "\(switchOn), then choose Quit & Reopen if macOS asks. I'll pick up here."
                : "\(switchOn). I'll check when macOS confirms it.")
            : isList ? (both ? IntroCopy.dragBoth : IntroCopy.dragMe) : IntroLine(text: "Switch FERB on here")
        orb.dockBesideSettings(dragging: isList && !target.isEmpty ? Self.dragItems(target, both: both) : [],
                               instruction: instruction, showWay: !added)
        notch.onStepBehindSettings?()            // Settings, not the notch, is what the owner works in now (APP-642)
    }

    /// The privacy lists that show FERB.app as a second "FERB" beside its agent, where people switched on the wrong one
    /// and the step stuck: the logo carries both apps there and the lines say to turn both switches on (APP-646 for
    /// Full Disk Access, APP-659 for Screen & System Audio Recording). FERB's check still reads the agent's grant.
    static let bothFERBLists: Set<String> = ["full_disk_access", "screen_recording"]

    /// What the logo carries into a privacy list: FERB's agent app (`target`, the daemon's grant target), and in
    /// `bothFERBLists` FERB.app too, the app holding the agent.
    static func dragItems(_ target: String, both: Bool) -> [URL] {
        let agent = URL(fileURLWithPath: target)
        let helpers = agent.deletingLastPathComponent()
        guard both, helpers.lastPathComponent == "Helpers",
              helpers.deletingLastPathComponent().lastPathComponent == "Contents" else { return [agent] }
        return [agent, helpers.deletingLastPathComponent().deletingLastPathComponent()]
    }

    private func permissionOverview(_ visit: PermissionVisit) -> IntroLine {
        permissionOverview(visit, wait: setup.grantWait)
    }

    /// The beat's line with `wait` as the grant's current step (the publisher hands the new value before it is set).
    private func permissionOverview(_ visit: PermissionVisit, wait: GrantWait?) -> IntroLine {
        if setup.notificationHint { visit.bannerSince = visit.bannerSince ?? Date() } else { visit.bannerSince = nil }
        return IntroCopy.permissionOverview(rows: setup.setupRows, selected: visit.active, requesting: visit.requesting,
                                            added: visit.added, busy: visit.task != nil, wait: wait,
                                            confirmed: visit.confirmed, bannerSince: visit.bannerSince,
                                            signerChanged: setup.signerChanged)
    }
}
