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
        static let otherSuggestion = "otherSuggestion", doItForReal = "doItForReal", notNow = "notNow"
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
    static func connected(_ name: String) -> IntroLine {
        IntroLine(text: "Connected. I'll use \(name).",
                  detail: "While we go on, I'll take a quick look at the apps you use, so I can suggest something useful.")
    }

    // MARK: 3. Permissions

    static let dragMe = IntroLine(text: "Drag me into this list", detail: "or switch FERB on if it's there")
    /// Full Disk Access lists two "FERB" rows (FERB.app and its background part): both go on (APP-646)
    static let dragBoth = IntroLine(text: "Drag me into this list", detail: "then turn both FERB switches on")
    static let allAllowed = IntroLine(text: "That's everything. Thank you.")

    @MainActor static func permissionOverview(rows: [PermissionRow], selected: String?, requesting: Bool,
                                   added: Bool, busy: Bool) -> IntroLine {
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
        var line = IntroLine(text: "A few permissions, one step at a time.",
                             detail: "\(rows.filter { $0.status == .granted }.count) of \(rows.count) allowed. Pick any permission, or follow along below.",
                             choices: choices)
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
            if !row.blocked.isEmpty { detail = row.blocked; status = "Needs attention" }
            return IntroChecklistItem(id: row.id, title: PermissionCopy.label(row.id), detail: detail,
                                      complete: row.status == .granted,
                                      actionID: !requesting && row.status != .granted && row.blocked.isEmpty
                                        ? IntroCopy.Pick.permission(row.id) : nil,
                                      active: active, status: row.status == .granted ? "Allowed" : status)
        }
        return line
    }

    // MARK: 4. Browsers

    static let browsers = IntroLine(text: "Now your browsers.",
                                    detail: "I'll pair with each one so I can help on the web. A browser may ask you to turn FERB on.",
                                    choices: [IntroChoice(Pick.pairAll, "Pair all"),
                                              IntroChoice(Pick.browsersLater, "Later")])
    static let pairing = IntroLine(text: "Pairing your browsers…")
    static let pairingOrb = IntroLine(text: "If your browser asks, turn FERB on here")
    static let paired = IntroLine(text: "Paired.")
    static func notPaired(_ why: String) -> IntroLine {
        IntroLine(text: "Not all of them paired.", detail: why,
                  choices: [IntroChoice(Pick.pairAll, "Try again"), IntroChoice(Pick.next, "Continue")])
    }

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
                  detail: "Pick one and I'll show you how I'd do it. I won't do anything yet.",
                  choices: requests.enumerated().map { IntroChoice(Pick.suggestion($0.offset), $0.element) }
                      + [IntroChoice(Pick.notNow, "Not now")])
    }
    static let previewing = IntroLine(text: "Here's how I'd do it…")
    /// The orb's bubble at the app the previewed task would use: it only points.
    static func wouldUse(_ target: IntroAppTarget, browser: Bool) -> IntroLine {
        IntroLine(text: browser || target.name.isEmpty ? "I'd use your browser for this" : "I'd use \(target.name) for this",
                  iconPath: target.iconPath)
    }
    static func previewed(otherShown: Bool) -> IntroLine {
        IntroLine(text: "That's how I'd do it. Nothing was done.",
                  detail: "Want me to do it for real once the tour ends?",
                  choices: [IntroChoice(Pick.doItForReal, "Do it for real")]
                      + (otherShown ? [] : [IntroChoice(Pick.otherSuggestion, "Show the other one")])
                      + [IntroChoice(Pick.notNow, "Not now")])
    }
    static func previewFailed(_ why: String) -> IntroLine {
        IntroLine(text: "I couldn't show that plan right now.", detail: why,
                  choices: [IntroChoice(Pick.notNow, "Continue")])
    }
    static let willDoIt = IntroLine(text: "I'll start it when the tour ends.", detail: "It runs as a normal task, with its usual questions.")

    // MARK: 7. Control

    static func control(_ control: IntroControl) -> IntroLine {
        switch control {
        case .toggle:
            return IntroLine(text: "Double-tap the right ⌥ Option key to open me. Do it again to close me.")
        case .talk:
            return IntroLine(text: "Hold right ⌘ Command and ⌥ Option together and talk to me. Let go when you're done.",
                             detail: "Nothing you say now starts a task.")
        case .summon:
            return IntroLine(text: "Control-Option-Space opens me too. Try it.")
        case .stop:
            return IntroLine(text: "Here's that plan again, running as a preview.",
                             detail: "Press Stop this task. That's how you stop anything I'm doing.")
        case .pause:
            return IntroLine(text: "Last one: click my icon in the menu bar and choose Pause FERB.",
                             detail: "Paused, I don't see or touch anything. I'll switch myself back on right away.")
        }
    }
    static let alreadyPaused = IntroLine(text: "I'm paused right now, as you left me.",
                                         detail: "That's Pause FERB in my menu-bar icon: I don't see or touch anything until you choose Resume. I'll stay paused.")
    static let controlsDone = IntroLine(text: "You've got it.")

    // MARK: 8. Home

    static let home = IntroLine(text: "That's it. I'll be up here in the notch.",
                                detail: "Double-tap right ⌥, or press Control-Option-Space, whenever you need me. You can replay this tour in Settings.")

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
