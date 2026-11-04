import Foundation
import NotchCore

/// Everything FERB says in the first-run tour (kb area app, "the journey"): warm, plain, short, as text in the
/// notch or in the bubble beside the orb, never read aloud. One place, so the words can be read and changed together.
/// The choice ids are what IntroController acts on.
enum IntroCopy {
    enum Pick {
        static let codex = "codex", claude = "claude", otherWays = "otherWays", back = "back", cancel = "cancel"
        static let claudePage = "claudePage", claudeCode = "claudeCode", retry = "retry"
        static func key(_ provider: APIProvider) -> String { "key." + provider.rawValue }
        static let permissionsNext = "permissions.next"
        static func permission(_ id: String) -> String { "permission." + id }
        static let openLoginItems = "openLoginItems"
        static let pairAll = "pairAll", browsersLater = "browsersLater", next = "next"
        static let identityNow = "identityNow", identityLater = "identityLater"
        static func profile(_ index: Int) -> String { "profile.\(index)" }
        static func suggestion(_ index: Int) -> String { "suggestion.\(index)" }
        static let otherSuggestion = "otherSuggestion", notNow = "notNow", goOn = "goOn"
        static let stopTask = "stopTask", letFinish = "letFinish", stopWaiting = "stopWaiting", pairOne = "pairOne"
        /// Add to <browser> on one profile of Pair all's result (HAND-413): its row's id is "browser/folder".
        static func add(_ id: String) -> String { "add." + id }
        static func addedRow(_ pick: String) -> String? { pick.hasPrefix("add.") ? String(pick.dropFirst(4)) : nil }
    }

    // MARK: 1. Meet

    static let hello = IntroLine(text: "Hi, I'm FERB.",
                                 detail: "I work on your Mac in the background while you keep working. Let me show you around.")
    static let gettingReady = IntroLine(text: "Getting my background part ready…")
    static let loginItems = IntroLine(text: "macOS wants your OK before I can run in the background.",
                                      detail: "In Login Items, switch FERB on under Allow in the Background.",
                                      choices: [IntroChoice(Pick.openLoginItems, "Open Login Items")])
    static let loginItemsOrb = IntroLine(text: "Switch FERB on here", detail: "Login Items · Allow in the Background")
    static func serviceFailed(_ why: String?) -> IntroLine {
        IntroLine(text: "I couldn't start my background part.", detail: why.flatMap(lastLine),
                  choices: [IntroChoice(Pick.retry, "Try again")])
    }

    // MARK: 2. Sign in

    static let signIn = IntroLine(text: "First, connect me to an AI service.",
                                  detail: "Sign in with ChatGPT or Claude. Your passwords, codes and card numbers never go to the AI: I keep them on this Mac and fill them in myself.",
                                  choices: [IntroChoice(Pick.codex, "ChatGPT"), IntroChoice(Pick.claude, "Claude"),
                                            IntroChoice(Pick.otherWays, "Other ways to connect")])
    static func signInFailed(_ why: String?) -> IntroLine {
        IntroLine(text: "That didn't connect.", detail: why, choices: signIn.choices)
    }
    static func codexWaiting(_ status: String?) -> IntroLine {
        IntroLine(text: "Finish signing in in the browser window that opened.", detail: status,
                  choices: [IntroChoice(Pick.cancel, "Cancel")])
    }
    static func claudeWaiting(_ status: String?) -> IntroLine {
        IntroLine(text: "Finish signing in to Claude in your browser.", detail: status,
                  choices: [IntroChoice(Pick.claudePage, "Open the page again"),
                            IntroChoice(Pick.claudeCode, "Paste a code"), IntroChoice(Pick.cancel, "Cancel")])
    }
    static let claudeCode = IntroLine(text: "Paste the code the sign-in page shows.",
                                      choices: [IntroChoice(Pick.back, "Back")],
                                      entry: IntroEntry(placeholder: "Code from the page", secure: true))
    static let otherWays = IntroLine(text: "Or use an API key.", detail: "Your key is kept in your Keychain, on this Mac.",
                                     choices: APIProvider.allCases.map { IntroChoice(Pick.key($0), $0.label) }
                                        + [IntroChoice(Pick.back, "Back")])
    static func pasteKey(_ provider: APIProvider) -> IntroLine {
        IntroLine(text: "Paste your \(provider.label) API key.",
                  choices: [IntroChoice(Pick.back, "Back")],
                  entry: IntroEntry(placeholder: "\(provider.label) API key", secure: true))
    }
    static func checking(_ status: String?) -> IntroLine { IntroLine(text: "Checking that it answers…", detail: status) }
    static let openingClaude = IntroLine(text: "Opening Claude's sign-in…")
    static func connected(_ name: String) -> IntroLine {
        IntroLine(text: "Connected. I'll use \(name).",
                  detail: "While we go on, I'll take a quick look at the apps you use, so I can suggest something useful.")
    }

    // MARK: 3. Permissions

    static let dragMe = IntroLine(text: "Drag me into this list", detail: "or switch FERB on if it's there")
    /// Full Disk Access lists two "FERB" rows (FERB.app and its background part): both go on (APP-646)
    static let dragBoth = IntroLine(text: "Drag me into this list", detail: "then turn both FERB switches on")
    static let allAllowed = IntroLine(text: "That's everything. Thank you.")

    static let checkingPermissions = IntroLine(text: "Checking your permissions with macOS…")
    static let resuming = IntroLine(text: "Picking up where we left off…",
                                    detail: "Waiting for my background part to answer.")
    static let startingService = IntroLine(
        text: "Starting my background part again…",
        detail: "It stopped answering (macOS's Quit & Reopen can end it). Your place is kept.")

    /// The permission beat's line: what FERB is doing for the grant in flight (`wait`), live (APP-665, APP-679), or the
    /// checklist at rest; `confirmed` names a grant macOS just confirmed; `bannerSince`: Notifications' banner is up.
    /// A row ticks only when the system reports it on (APP-510), whatever the wait says.
    @MainActor static func permissionOverview(rows: [PermissionRow], selected: String?, requesting: Bool,
                                              added: Bool, busy: Bool, wait: GrantWait? = nil,
                                              confirmed: String? = nil, bannerSince: Date? = nil) -> IntroLine {
        let pending = SetupModel.allowAllOrder(rows)
        let quick = SetupModel.quickPermissionOrder(rows)
        let choices: [IntroChoice]
        if busy || requesting {
            choices = []
        } else if let active = selected, rows.contains(where: { $0.id == active && $0.status != .granted }) {
            choices = [IntroChoice(IntroCopy.Pick.permission(active), "Open \(PermissionCopy.label(active))")]
        } else if !quick.isEmpty {
            choices = [IntroChoice(IntroCopy.Pick.permissionsNext, "Set up quick permissions")]
        } else if let next = pending.first {
            choices = [IntroChoice(IntroCopy.Pick.permissionsNext, "Set up \(PermissionCopy.label(next))")]
        } else { choices = [] }
        let count = "\(rows.filter { $0.status == .granted }.count) of \(rows.count) allowed."
        let waited = wait.flatMap { wait in rows.first { $0.id == wait.id && $0.status != .granted } }
        var line: IntroLine
        if let wait, let waited {
            line = grantWaitLine(wait, row: waited, added: added)
            line.choices = choices
            line.since = wait.shownSince
        } else if let confirmed, rows.contains(where: { $0.id == confirmed && $0.status == .granted }) {
            line = IntroLine(text: "\(PermissionCopy.label(confirmed)) is on. macOS confirmed it.",
                             detail: "\(count) Pick any permission, or follow along below.", choices: choices)
        } else {
            line = IntroLine(text: "A few permissions, one step at a time.",
                             detail: "\(count) Pick any permission, or follow along below.", choices: choices)
            line.since = bannerSince                                     // macOS's banner is up: alive while it is
        }
        line.checklist = rows.map { row in
            let active = selected == row.id
            var detail = PermissionCopy.reason(row.id)
            var status = row.restartToCheck ? "Restart needed" : "Set up"
            if active {
                if requesting {
                    detail = "Choose Allow in macOS's question. " + detail
                    status = "Waiting for macOS"
                } else if row.restartToCheck {
                    detail = (added ? "Added to the list. " : "Drag FERB into the list if it's missing. ")
                        + "Turn it on, then choose Quit & Reopen if macOS asks. Your progress is saved."
                    status = added ? "Added · restart next" : "In System Settings"
                } else if row.id == "notifications" && row.status == .notDetermined {
                    detail = "Choose Allow in macOS's notification banner. You can set up the other permissions meanwhile."
                    status = "Answer the banner"
                } else {
                    detail = "Switch FERB on in System Settings. " + detail
                    status = row.status == .denied ? "Not allowed yet" : "In System Settings"
                }
            }
            if let wait, wait.id == row.id { status = grantWaitStatus(wait, row: row, added: added) }
            if !row.blocked.isEmpty { detail = row.blocked; status = "Needs attention" }
            return IntroChecklistItem(id: row.id, title: PermissionCopy.label(row.id), detail: detail,
                                      complete: row.status == .granted,
                                      actionID: !requesting && row.status != .granted && row.blocked.isEmpty
                                        ? IntroCopy.Pick.permission(row.id) : nil,
                                      active: active, status: row.status == .granted ? "Allowed" : status)
        }
        return line
    }

    /// A permission's wait as it happens (APP-679): the headline says what FERB is doing now; while the owner's switch
    /// is awaited it stays put and only the second line follows each check, so nothing jumps every poll.
    @MainActor static func grantWaitLine(_ wait: GrantWait, row: PermissionRow, added: Bool) -> IntroLine {
        let label = PermissionCopy.label(row.id)
        switch wait.step {
        case .asking:
            return IntroLine(text: "Asking macOS for \(label)…",
                             detail: "Answer macOS's question: Allow, or Open System Settings.")
        case .opening:
            return IntroLine(text: "Opening System Settings at \(label)…")
        case .waitingForSwitch, .checking:
            if row.id == "notifications" && row.status == .notDetermined {
                return IntroLine(text: "Waiting for your answer in macOS's banner…",
                                 detail: "Choose Allow at the top right of your screen.")
            }
            let who = IntroController.bothFERBLists.contains(row.id) ? "both FERB switches" : "FERB"
            let then = row.restartToCheck ? "If macOS asks, choose Quit & Reopen: I'll pick up right here."
                : "It ticks once macOS confirms it."
            let check = wait.step == .checking ? "Checking with macOS…"
                : wait.checks == 0 ? "" : "Still off in macOS (checked \(wait.checks)×)."
            return IntroLine(text: (added ? "Added. " : "") + "Waiting for you to turn \(who) on in \(label)…",
                             detail: check.isEmpty ? then : "\(then) \(check)")
        case .restarting(let seenOn):
            return seenOn
                ? IntroLine(text: "\(label) is on. Restarting FERB's background part so it can use it…",
                            detail: "A few seconds; then I check with macOS again.")
                : IntroLine(text: "Restarting FERB's background part to check \(label)…",
                            detail: "macOS shows this switch to it only after a restart.")
        case .startingService:
            return startingService
        }
    }

    /// The waited-on row's status: the same step as the line, in a word or two.
    static func grantWaitStatus(_ wait: GrantWait, row: PermissionRow, added: Bool) -> String {
        switch wait.step {
        case .asking: return "Waiting for macOS"
        case .opening: return "Opening Settings…"
        case .waitingForSwitch:
            return row.id == "notifications" && row.status == .notDetermined ? "Answer the banner"
                : added ? "Added · turn it on" : "Waiting for you"
        case .checking: return "Checking with macOS…"
        case .restarting: return "Restarting FERB…"
        case .startingService: return "Starting FERB…"
        }
    }

    // MARK: 4. Browsers

    static let browsers = IntroLine(text: "Now your browsers.",
                                    detail: "I'll pair with each one so I can help on the web. A browser may ask you to turn FERB on.",
                                    choices: [IntroChoice(Pick.pairAll, "Pair all"),
                                              IntroChoice(Pick.browsersLater, "Later")])
    static let lookingAtBrowsers = IntroLine(text: "Looking at your browsers…")
    static let pairingOrb = IntroLine(text: "If your browser asks, turn FERB on here")
    static let paired = IntroLine(text: "Paired.")
    /// `line` with each installed Chrome-family browser FERB doesn't support yet named under it (HAND-415).
    static func noting(_ unsupported: [UnsupportedBrowser], on line: IntroLine) -> IntroLine {
        guard !unsupported.isEmpty else { return line }
        var noted = line
        noted.detail = ([line.detail].compactMap { $0 } + unsupported.map { $0.message + "." }).joined(separator: "\n")
        return noted
    }

    /// Pair all while it runs (HAND-412): which profiles are pairing, each result as it lands, the skipped ones.
    static func pairing(_ progress: PairProgress) -> IntroLine {
        let open = progress.pairing
        let text = open.count == 1 ? "Pairing \(progress.landed + 1) of \(progress.tried): \(open[0].title)…"
            : open.count > 1 ? "Pairing \(progress.tried) profiles…" : "Pairing your browsers…"
        var line = IntroLine(text: text, detail: progress.tried == 0 ? nil
                             : "\(progress.landed) of \(progress.tried) done. If a browser asks, turn FERB on there.")
        line.checklist = pairChecklist(progress)
        line.checklistHint = "A browser may open a page for a moment while it pairs."
        return line
    }

    /// Pair all's result when not every profile paired (HAND-411): one row per profile with its own step; the ones
    /// FERB's store page fixes offer Add to <browser> (HAND-413).
    static func notPaired(_ progress: PairProgress, message: String) -> IntroLine {
        let paired = progress.rows.filter { $0.state == .paired }.count
        var line = IntroLine(text: progress.rows.isEmpty ? "Not all of them paired."
                             : "\(paired) of \(progress.rows.count) paired.",
                             detail: progress.rows.isEmpty ? message
                                : progress.rows.contains(where: \.canAdd)
                                    ? "The others need a step from you. Add opens FERB's store page in that profile."
                                    : "The others need a step from you.",
                             choices: [IntroChoice(Pick.pairAll, "Try again"), IntroChoice(Pick.next, "Continue")])
        line.checklist = pairChecklist(progress)
        line.checklistHint = "Point at a profile to see what it needs."
        return line
    }

    private static func pairChecklist(_ progress: PairProgress) -> [IntroChecklistItem] {
        progress.rows.map { row in
            let status: String
            switch row.state {
            case .pairing: status = "Pairing…"
            case .paired: status = "Paired"
            case .failed: status = row.canAdd ? "Add to \(row.label)" : "Didn't pair"
            case .skipped: status = row.canAdd ? "Add to \(row.label)" : "Needs a step"
            }
            return IntroChecklistItem(id: row.id, title: row.title,
                                      detail: row.why.isEmpty ? status : row.why, complete: row.state == .paired,
                                      actionID: row.canAdd && row.state != .paired ? Pick.add(row.id) : nil,
                                      active: row.state == .pairing, status: status)
        }
    }

    static func noProfiles(_ why: String) -> IntroLine {
        IntroLine(text: "No browser profiles found.", detail: why,
                  choices: [IntroChoice(Pick.pairAll, "Try again"), IntroChoice(Pick.next, "Continue")])
    }
    static func openingStore(_ row: PairProgress.Row) -> IntroLine {
        IntroLine(text: "Opening FERB's store page in \(row.title)…")
    }
    /// Waiting for the extension to appear in that profile after Add to (HAND-413).
    static func waitingForAdd(_ row: PairProgress.Row, added: Bool) -> IntroLine {
        IntroLine(text: added ? "FERB is in \(row.title). Turn it on there."
                              : "Waiting for FERB to be added in \(row.title)…",
                  detail: added ? "\(row.label) may show “Action required”, or a switch on its extensions page."
                                : "Click Add to \(row.label) on the page that opened.",
                  choices: [IntroChoice(Pick.stopWaiting, "Stop waiting")])
    }
    static func readyToPair(_ row: PairProgress.Row) -> IntroLine {
        IntroLine(text: "FERB is on in \(row.title).", detail: "Pair it now?",
                  choices: [IntroChoice(Pick.pairOne, "Pair"), IntroChoice(Pick.notNow, "Not now")])
    }
    static func pairingOne(_ row: PairProgress.Row) -> IntroLine { IntroLine(text: "Pairing \(row.title)…") }

    // MARK: 5. FERB's own email

    static let identity = IntroLine(text: "Want me to have my own email for sign-ups and searches?",
                                    detail: "Set it up now, or later in Settings. I'll ask again the first time a task needs it.",
                                    choices: [IntroChoice(Pick.identityNow, "Set it up now"),
                                              IntroChoice(Pick.identityLater, "Later")])
    static let identityAddress = IntroLine(text: "Which Google account is mine?",
                                           detail: "Sign into it in a browser profile of its own first.",
                                           choices: [IntroChoice(Pick.identityLater, "Later")],
                                           entry: IntroEntry(placeholder: "Its email address", secure: false))
    static let identityNotFound = IntroLine(text: "I didn't find a browser profile with only that account.",
                                            detail: "Sign into it in a profile of its own, then try again.",
                                            choices: [IntroChoice(Pick.retry, "Try again"),
                                                      IntroChoice(Pick.identityLater, "Later")])
    static func identityProfiles(_ candidates: [IdentityCandidate]) -> IntroLine {
        IntroLine(text: "Which profile is mine?",
                  choices: candidates.enumerated().map { IntroChoice(Pick.profile($0.offset),
                                                                     "\($0.element.name) in \($0.element.browserName)") }
                      + [IntroChoice(Pick.identityLater, "Later")])
    }
    static let identityLooking = IntroLine(text: "Looking for that account in your browsers…")
    static let identityPairing = IntroLine(text: "Setting up my account…",
                                           detail: "Turn FERB's extension on in that profile if the browser asks.")
    static func identityResult(_ line: String?) -> IntroLine { IntroLine(text: line ?? "Done.") }

    // MARK: 6. Try

    /// What the scan saw, in one or two of its own sentences.
    static func saw(_ summary: String) -> IntroLine {
        IntroLine(text: "Here's what I saw on your Mac.", detail: firstSentences(summary, 2))
    }
    static let thinking = IntroLine(text: "Let me think of something useful for you…")
    static let noSuggestions = IntroLine(text: "I couldn't come up with suggestions right now.",
                                         choices: [IntroChoice(Pick.retry, "Try again"),
                                                   IntroChoice(Pick.notNow, "Skip this")])
    static func suggest(_ requests: [String]) -> IntroLine {
        IntroLine(text: "Here are two things I could do for you.",
                  detail: "Pick one and I'll do it now, while you watch. It's a real task: I'll ask you before anything that needs your OK.",
                  choices: requests.enumerated().map { IntroChoice(Pick.suggestion($0.offset), $0.element) }
                      + [IntroChoice(Pick.notNow, "Not now")])
    }
    static let starting = IntroLine(text: "Starting it…")

    /// Where a task the tour started stands, from its turn in the chat (APP-666).
    enum TaskState: Equatable {
        case starting, planned(String), step(Int, Int, String), asking, working
    }
    static func running(_ state: TaskState) -> IntroLine {
        let detail: String
        switch state {
        case .starting: detail = "Planning it…"
        case .planned(let plan): detail = plan
        case .step(let index, let total, let label): detail = "Step \(index) of \(total): \(label)"
        case .asking: detail = "I'm asking you something in the chat."
        case .working: detail = "Watch it in the chat."
        }
        return IntroLine(text: state == .asking ? "Waiting for your answer…" : "On it…", detail: detail,
                         choices: [IntroChoice(Pick.goOn, "Continue the tour")])
    }

    /// How the tour's task ended (APP-666).
    enum Outcome: Equatable { case done, failed, stopped, stillRunning, notStarted }
    static func ran(_ outcome: Outcome, otherLeft: Bool) -> IntroLine {
        let text: String, detail: String
        switch outcome {
        case .done: (text, detail) = ("Done.", "The result is in the chat.")
        case .failed: (text, detail) = ("That didn't work out.", "What happened is in the chat.")
        case .stopped: (text, detail) = ("Stopped.", "What it did before stopping is in the chat.")
        case .stillRunning: (text, detail) = ("It keeps running.", "Its result will arrive in the chat.")
        case .notStarted: (text, detail) = ("I couldn't start it.", "Check that I'm connected, then ask me in the chat.")
        }
        return IntroLine(text: text, detail: detail,
                         choices: (otherLeft ? [IntroChoice(Pick.otherSuggestion, "Try the other one")] : [])
                            + [IntroChoice(Pick.next, "Continue")])
    }

    // MARK: 7. Control

    static func control(_ control: IntroControl) -> IntroLine {
        switch control {
        case .toggle:
            return IntroLine(text: "Double-tap the right ⌥ Option key to open me. Do it again to close me.")
        case .talk:
            return IntroLine(text: "Hold right ⌘ Command and ⌥ Option together and talk to me. Let go when you're done.",
                             detail: "Nothing you say now starts a task.")
        case .stop:
            // Shown when nothing the owner started runs: the tour starts no task of its own (APP-696).
            return IntroLine(text: "To stop me while I work: Tasks, then Stop task.",
                             detail: "Tasks is at the top of the chat, next to History.")
        case .pause:
            return IntroLine(text: "Last one: click my icon in the menu bar and choose Pause FERB.",
                             detail: "Paused, I don't see or touch anything. I'll switch myself back on right away.")
        }
    }
    static let alreadyPaused = IntroLine(text: "I'm paused right now, as you left me.",
                                         detail: "That's Pause FERB in my menu-bar icon: I don't see or touch anything until you choose Resume. I'll stay paused.")
    static let controlsDone = IntroLine(text: "You've got it.")
    /// Stop (APP-696): offered only on the owner's own Try task while it still runs.
    static func stopOffer(_ request: String) -> IntroLine {
        IntroLine(text: "Your task is still running.",
                  detail: "“\(request)”. Stop it now, or let it finish. Stopping works the same on anything I'm doing.",
                  choices: [IntroChoice(Pick.stopTask, "Stop it now"), IntroChoice(Pick.letFinish, "Let it finish")])
    }
    static let stopping = IntroLine(text: "Stopping it…", choices: [IntroChoice(Pick.goOn, "Go on")])
    static let stopped = IntroLine(text: "Stopped.", detail: "That's all it takes. The chat shows it stopped.")
    static let lettingItFinish = IntroLine(text: "It keeps running.", detail: "Its result will arrive in the chat.")
    static let finishedFirst = IntroLine(text: "It finished.", detail: "The result is in the chat.")

    // MARK: 8. Home

    static let home = IntroLine(text: "That's it. I'll be up here in the notch.",
                                detail: "Double-tap right ⌥ whenever you need me. You can replay this tour in Settings.")

    // MARK: Helpers

    private static func lastLine(_ text: String) -> String? {
        text.split(separator: "\n").last.map(String.init)
    }

    static func firstSentences(_ text: String, _ count: Int) -> String {
        var sentences: [String] = []
        text.enumerateSubstrings(in: text.startIndex..., options: .bySentences) { sentence, _, _, stop in
            if let sentence { sentences.append(sentence.trimmingCharacters(in: .whitespaces)) }
            if sentences.count == count { stop = true }
        }
        return sentences.isEmpty ? text : sentences.joined(separator: " ")
    }
}
