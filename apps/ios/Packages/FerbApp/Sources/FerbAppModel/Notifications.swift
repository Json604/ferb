import FerbCore
import FerbDesign
import FerbMobile
import Foundation

// FERB's notifications as plain data (PHN-415, PHN-417, PHN-421, PHN-422, PHN-125, PHN-126; kb PHN-504, PHN-519;
// Android: Notifier.kt and Background.kt's AnswerReceiver). The planner decides WHAT to post, replace or remove; the
// shell turns each [NotificationAction] into UNUserNotificationCenter calls (the id is the request identifier, so a
// post with the same id replaces the one shown).
//
// What a question's notification carries follows the Mac's needs for it, never the app's judgement:
// - routine (the Mac takes the answer with no proof): its words and up to three answer buttons, or a typed answer;
// - needs the phone's proof: its words and no buttons, "Needs Face ID. Tap to answer it in FERB.";
// - only on the Mac (can_answer false): its words and that it is answered on the Mac;
// - a hidden input: never worded (PHN-422), "Open FERB to answer.";
// - words held back (a locked session that didn't prove the device key, PHN-126): "FERB has a question", wordless;
// - one of FERB's own checks while the phone is locked and the owner's setting is on: wordless, no buttons (PHN-422).
// Unknown needs (no status yet) are treated as needing the app: no buttons. Every notification carries a public text
// (FERB's name and "FERB has a question" or "FERB has news") for a lock screen that hides previews; no picture is
// ever put in one.

/// Which kind of notification (Android's channels): the shell maps it to a category, a thread and how it interrupts
/// (question, check and link: time-sensitive in Android's HIGH sense; done: ordinary).
public enum NotificationKind: String, Sendable {
    case question, check, done, link
}

/// What a tap on the notification opens.
public enum NotificationTap: Equatable, Sendable {
    /// The app as it is.
    case app
    /// The question's sheet ([AppModel.openQuestion]) once the app is unlocked.
    case question(String)
    /// The chat where the task is (Android passes the task id as its entry).
    case task(String)
}

/// One answer button. `choice` is the index in [Question.choices]; nil: a typed answer (the shell shows a text field
/// with `placeholder`).
public struct NotificationButton: Equatable, Sendable {
    /// The "Yes, until I'm back at my Mac" button's `choice` (not an index into the question's choices, PHN-611).
    public static let untilBack = -2

    public let label: String
    public let choice: Int?
    public let placeholder: String?

    public init(label: String, choice: Int?, placeholder: String?) {
        self.label = label
        self.choice = choice
        self.placeholder = placeholder
    }
}

/// One notification to show, replacing any with the same `id`.
public struct PlannedNotification: Equatable, Sendable {
    /// `q:<approval id>`, `t:<task id>` or `link`.
    public let id: String
    public let kind: NotificationKind
    public let title: String
    public let body: String
    /// What a lock screen that hides previews shows instead (the public version).
    public let publicText: String
    /// The words were left out (a private answer, held words, or one of FERB's checks while the phone is locked).
    public let wordless: Bool
    /// Answer buttons in the order shown (Yes first where there is one).
    public let buttons: [NotificationButton]
    public let tap: NotificationTap
    /// A re-post of one already shown (its words or needs changed): no sound, no banner again.
    public let quietly: Bool

    public init(id: String, kind: NotificationKind, title: String, body: String, publicText: String, wordless: Bool,
                buttons: [NotificationButton], tap: NotificationTap, quietly: Bool) {
        self.id = id
        self.kind = kind
        self.title = title
        self.body = body
        self.publicText = publicText
        self.wordless = wordless
        self.buttons = buttons
        self.tap = tap
        self.quietly = quietly
    }
}

public enum NotificationAction: Equatable, Sendable {
    case post(PlannedNotification)
    case remove(id: String)
}

/// The decisions, pure over what the Mac's status and the transcript say: tested without iOS. What was announced
/// lives in [Prefs.announced] (ids only); what is on show now is kept here.
@MainActor
public final class NotificationPlanner {
    private let prefs: Prefs
    /// Questions on show now (re-planned when the phone locks or unlocks: FERB's checks change their words then).
    public private(set) var posted: [String: Question] = [:]
    private var primedTranscript = false
    private var primedStatus = false
    private var lastPhase: LinkPhase?

    public init(prefs: Prefs) {
        self.prefs = prefs
    }

    /// The Mac's status: questions with their needs, and what finished (held: wordless). Works while locked.
    /// `chat`: the transcript's open questions (nil while the app is locked: it reads no chat). `quiet`: the owner is
    /// looking at the open app (nothing new is posted then; it is still marked announced).
    public func status(_ status: PhoneStatus, chat: [Question]?, quiet: Bool, lockedNow: Bool) -> [NotificationAction] {
        var out: [NotificationAction] = []
        var announced = prefs.announced
        let ids = Set(status.questions.map(\.approvalId))
        let questions = Questions.merged((chat ?? []).filter { ids.contains($0.approvalId) }, status)
        if !primedStatus {
            primedStatus = true
            announced.formUnion(questions.map { "q:\($0.approvalId)" } + status.results.map { "t:\($0.taskId)" })
            prefs.announced = announced
            return []
        }
        out += questionsChanged(questions, &announced, quiet: quiet, lockedNow: lockedNow)
        // what the Mac no longer has waiting goes from the shade
        for id in posted.keys.sorted() where !ids.contains(id) { out.append(cancel(id)) }
        for r in status.results {
            let key = "t:\(r.taskId)"
            if announced.contains(key) { continue }
            announced.insert(key)
            if quiet { continue }
            if r.held {
                out.append(.post(done(key, title: L.s("n_done_held"), body: L.s("n_held_text"), tap: .app, wordless: true)))
            } else {
                let failed = ChatModel.markOf(r.status) == .failed
                let line = WorkModel.oneLine(r.name.isEmpty ? ChatModel.plain(r.answer) : r.name)
                out.append(.post(done(key, title: L.s(failed ? "n_task_failed" : "n_task_done"), body: line, tap: .app, wordless: false)))
            }
        }
        prefs.announced = announced
        return out
    }

    /// The transcript (unlocked): the questions' full words, with their needs from the last status.
    public func transcript(_ view: TranscriptView, status: PhoneStatus?, quiet: Bool, lockedNow: Bool) -> [NotificationAction] {
        var out: [NotificationAction] = []
        var announced = prefs.announced
        let inView = Set(view.questions.compactMap(\.approvalId))
        let open = Questions.merged(Questions.open(view.questions), status).filter { inView.contains($0.approvalId) }
        let terminal = finished(view)
        if !primedTranscript {
            // what was already there when the app started is not news
            primedTranscript = true
            announced.formUnion(open.map { "q:\($0.approvalId)" } + terminal.map { "t:\($0.task)" })
            prefs.announced = announced
            return []
        }
        out += questionsChanged(open, &announced, quiet: quiet, lockedNow: lockedNow)
        // a question answered elsewhere, or no longer waiting: its notification goes
        let still = Set(open.map(\.approvalId))
        for id in posted.keys.sorted() where !still.contains(id) { out.append(cancel(id)) }
        for t in terminal {
            let key = "t:\(t.task)"
            if announced.contains(key) { continue }
            announced.insert(key)
            if !quiet { out.append(.post(done(key, title: t.title, body: t.line, tap: .task(t.task), wordless: false))) }
        }
        prefs.announced = announced
        return out
    }

    private func questionsChanged(_ questions: [Question], _ announced: inout Set<String>, quiet: Bool, lockedNow: Bool) -> [NotificationAction] {
        var out: [NotificationAction] = []
        for q in questions {
            let key = "q:\(q.approvalId)"
            if announced.contains(key) {
                // known already: its words or needs may have changed (the app unlocked, a status arrived)
                if let was = posted[q.approvalId], was != q { out.append(.post(question(q, lockedNow: lockedNow, quietly: true))) }
                continue
            }
            announced.insert(key)
            if !quiet { out.append(.post(question(q, lockedNow: lockedNow, quietly: false))) }
        }
        return out
    }

    /// The link stopped because the Mac no longer knows this phone: the owner must pair again.
    public func link(_ l: LinkStatus?) -> [NotificationAction] {
        defer { lastPhase = l?.phase }
        guard let l, l.phase == .stopped, lastPhase != .stopped, AppModel.removedCodes.contains(l.problem?.code ?? "") else { return [] }
        return [.post(PlannedNotification(id: "link", kind: .link, title: L.s("n_link_removed_title"), body: L.s("n_link_removed_text"),
                                          publicText: L.s("n_public_question"), wordless: false, buttons: [], tap: .app, quietly: false))]
    }

    /// The phone locked (true) or was unlocked (false) while FERB could still act, or FERB is about to be suspended
    /// (call it with true: once suspended FERB can't take a check's words away when the phone locks later): FERB's
    /// own checks on show are planned again, wordless or worded.
    public func lockScreen(locked: Bool) -> [NotificationAction] {
        posted.values.filter(\.ownCheck).sorted { $0.approvalId < $1.approvalId }
            .map { .post(question($0, lockedNow: locked, quietly: true)) }
    }

    /// A question's notification, worded and with buttons as its needs allow (the file's comment). Recorded as on show.
    public func question(_ q: Question, lockedNow: Bool, quietly: Bool) -> PlannedNotification {
        let id = q.approvalId
        let wordless = q.ownCheck && prefs.hideChecksOnLockScreen && lockedNow
        let asking = q.title ?? L.s("n_question_title")
        let reason: String? = q.reason.isEmpty ? nil : q.reason
        let title: String
        let body: String
        if q.hidden {
            (title, body) = (L.s("n_private_title"), L.s("n_private_text"))
        } else if q.held {
            (title, body) = (L.s("n_question_title"), L.s("n_held_text"))
        } else if wordless {
            (title, body) = (L.s("n_check_title"), L.s("n_check_text"))
        } else if q.onlyOnMac {
            (title, body) = (asking, [reason, L.s("n_on_mac")].compactMap { $0 }.joined(separator: "\n"))
        } else if !q.routine {
            let need: String
            if q.needs == nil { need = L.bio("n_needs_fingerprint_text") }
            else if q.needs?.fingerprint == true { need = L.bio("n_needs_fingerprint") }
            else { need = L.s("n_needs_unlock") }
            (title, body) = (asking, [reason, need].compactMap { $0 }.joined(separator: "\n"))
        } else {
            (title, body) = (asking, reason ?? asking)
        }
        var buttons: [NotificationButton] = []
        if q.routine && !wordless && !q.held {
            let choices = q.choices
            if !choices.isEmpty && choices.count <= 3 {
                // Yes first where there is one, as the phone's sheet reads left to right with Yes on the right
                let order = choices.indices.sorted { a, b in choices[a].yes && !choices[b].yes }
                buttons = order.map { NotificationButton(label: choices[$0].label, choice: $0, placeholder: nil) }
                // "until I'm back at my Mac" when the Mac offers it (PHN-611); iOS shows up to four actions
                if q.untilBack {
                    buttons.append(NotificationButton(label: L.s("until_back_yes"), choice: NotificationButton.untilBack, placeholder: nil))
                }
            } else if q.options.isEmpty {
                buttons = [NotificationButton(label: L.s("n_answer"), choice: nil, placeholder: L.s("n_answer_hint"))]
            }
        }
        posted[id] = q
        return PlannedNotification(id: "q:\(id)", kind: q.ownCheck ? .check : .question, title: title, body: body,
                                   publicText: L.s("n_public_question"), wordless: q.hidden || q.held || wordless,
                                   buttons: buttons, tap: .question(id), quietly: quietly)
    }

    /// After an answer from the notification: gone, or replaced by what stopped it (PHN-302).
    public func answered(_ approvalId: String, problem: String?, needsApp: Bool) -> NotificationAction {
        if problem == nil { return cancel(approvalId) }
        return .post(PlannedNotification(id: "q:\(approvalId)", kind: .question,
                                         title: needsApp ? L.bio("n_needs_fingerprint_title") : L.s("n_not_answered_title"),
                                         body: problem ?? "", publicText: L.s("n_public_question"), wordless: false, buttons: [],
                                         tap: .question(approvalId), quietly: false))
    }

    private func cancel(_ approvalId: String) -> NotificationAction {
        posted[approvalId] = nil
        return .remove(id: "q:\(approvalId)")
    }

    private func done(_ id: String, title: String, body: String, tap: NotificationTap, wordless: Bool) -> PlannedNotification {
        PlannedNotification(id: id, kind: .done, title: title, body: body, publicText: L.s("n_public_done"), wordless: wordless,
                            buttons: [], tap: tap, quietly: false)
    }

    private struct Finished { let task: String; let title: String; let line: String }

    /// The turns that finished, failed or were stopped, by task id, with their title and first line.
    private func finished(_ view: TranscriptView) -> [Finished] {
        view.items.compactMap { item -> Finished? in
            guard case .turn(let t) = item, let task = t.taskId, let answer = t.answer,
                  !["working", "needs_user", "merged"].contains(t.status) else { return nil }
            let failed = ChatModel.markOf(t.status) == .failed
            let title = WorkModel.title(t)
            return Finished(task: task, title: L.s(failed ? "n_task_failed" : "n_task_done"),
                            line: title.isEmpty ? WorkModel.oneLine(ChatModel.plain(answer.text)) : title)
        }
    }
}

/// The planner wired to the phone (Android's Notifier.start): each change the core announces is read (no polling) and
/// what to do goes to `deliver`, which the shell turns into UNUserNotificationCenter calls. `lockedNow`: whether the
/// phone's screen is locked now (the shell's protected-data state). One per process.
@MainActor
public final class Notifier {
    public let planner: NotificationPlanner
    private let api: PhoneApi
    private let session: Session
    private let lockedNow: () -> Bool
    private let deliver: ([NotificationAction]) -> Void
    private var changesTask: Task<Void, Never>?
    private var linkFollow: Follow<LinkStatus?>?
    /// One look at a time, in order (Android's Mutex).
    private var chain: Task<Void, Never>?

    public init(api: PhoneApi, prefs: Prefs, session: Session, lockedNow: @escaping () -> Bool,
                deliver: @escaping ([NotificationAction]) -> Void) {
        planner = NotificationPlanner(prefs: prefs)
        self.api = api
        self.session = session
        self.lockedNow = lockedNow
        self.deliver = deliver
    }

    /// Should a notification show now: not while the owner is looking at the open app.
    private var quiet: Bool { session.front && !session.locked }

    public func start() {
        let stream = api.changes()
        changesTask = Task { [weak self] in
            for await c in stream {
                guard let self else { return }
                switch c {
                case .transcript, .live: look { await $0.lookTranscript() }
                case .status: look { await $0.lookStatus() }
                default: break
                }
            }
        }
        linkFollow = Follow<LinkStatus?>({ [weak self] in
            guard let self else { return nil }
            return .some(api.link)
        }) { [weak self] l in
            guard let self else { return }
            let a = planner.link(l)
            if !a.isEmpty { deliver(a) }
        }
    }

    public func stop() {
        changesTask?.cancel()
        linkFollow?.stop()
    }

    @discardableResult
    private func look(_ work: @escaping @MainActor (Notifier) async -> Void) -> Task<Void, Never> {
        let before = chain
        let t = Task { [weak self] in
            await before?.value
            guard let self else { return }
            await work(self)
        }
        chain = t
        return t
    }

    /// Reads the Mac's status now (a status change, or a test).
    @discardableResult
    public func lookStatus() async -> [NotificationAction] {
        guard let status = api.lastStatus() else { return [] }
        var chat: [Question]?
        if !session.locked, case .ok(let view?) = await api.transcript() { chat = Questions.open(view.questions) }
        let a = planner.status(status, chat: chat, quiet: quiet, lockedNow: lockedNow())
        if !a.isEmpty { deliver(a) }
        return a
    }

    /// Reads the transcript now (a transcript change, or a test).
    @discardableResult
    public func lookTranscript() async -> [NotificationAction] {
        guard case .ok(let view?) = await api.transcript() else { return [] }
        let a = planner.transcript(view, status: api.lastStatus(), quiet: quiet, lockedNow: lockedNow())
        if !a.isEmpty { deliver(a) }
        return a
    }

    /// The phone locked or unlocked while FERB runs, or FERB is about to be suspended (`locked: true`).
    public func lockScreen(locked: Bool) {
        let a = planner.lockScreen(locked: locked)
        if !a.isEmpty { deliver(a) }
    }

    /// An answer given in a notification (PHN-415, PHN-417; kb PHN-503, PHN-519): `choice` is the button's index in
    /// the question's choices, `typed` the words of a typed answer. Sent at once, never queued, with no prompt (a
    /// routine Yes, an ordinary answer and a plain No need none, PHN-470). When the Mac says this one needs the phone's
    /// own proof, the notification says so and a tap opens the question in the app, where Face ID is asked (PHN-13).
    /// A launch for this answer waits for the phone to open and reach the Mac (bounded: iOS gives seconds).
    @discardableResult
    public func answer(_ approvalId: String, choice: Int?, typed: String?) async -> NotificationAction? {
        _ = await waitFor(8) { api.state.isOpen && api.link?.phase == .online }
        // the question as the chat holds it, else as the Mac's status gives it (a locked app reads no chat)
        var status = api.lastStatus()
        if status == nil, case .ok(let s) = await api.status() { status = s }
        var fromChat: [Question] = []
        if case .ok(let view?) = await api.transcript(),
           let item = view.questions.first(where: { $0.approvalId == approvalId && $0.open }) {
            fromChat = [Questions.parse(item)]
        }
        let q = Questions.merged(fromChat, status).first { $0.approvalId == approvalId }
        let result: NotificationAction?
        if let q, !q.hidden {
            // a button is only on a routine question; one that needs the phone's proof is answered in the app
            if !q.routine {
                result = planner.answered(approvalId, problem: L.s("n_needs_fingerprint_text"), needsApp: true)
            } else {
                let input: AnswerInput?
                let words = typed?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                if let choice, choice == NotificationButton.untilBack { input = q.untilBack ? .untilBack : nil }
                else if let choice { input = choice >= 0 && choice < q.choices.count ? q.choices[choice].input : nil }
                else if !words.isEmpty { input = .words(text: words) }
                else { input = nil }
                if let input {
                    switch await api.answer(approvalId, input, signer: nil) {
                    case .ok:
                        result = planner.answered(approvalId, problem: nil, needsApp: false)
                    case .failed(let e):
                        if case .NeedsUnlock = e, case .untilBack = input {
                            // "until I'm back" needs FERB opened (its unlock), not Face ID: said so
                            result = planner.answered(approvalId, problem: L.s("n_until_back_open"), needsApp: false)
                        } else if case .NeedsUnlock = e {
                            result = planner.answered(approvalId, problem: L.s("n_needs_fingerprint_text"), needsApp: true)
                        } else {
                            result = planner.answered(approvalId, problem: AppModel.errorNote(e).text, needsApp: false)
                        }
                    }
                } else {
                    result = nil
                }
            }
        } else {
            result = planner.answered(approvalId, problem: L.s("question_gone"), needsApp: false)
        }
        if let result { deliver([result]) }
        return result
    }
}

/// When the app locks (five minutes away, or by hand), the Mac is told (`lock`: its unlock window for this phone ends
/// and the conversation's watch stops until the next unlock, PHN-125; kb PHN-510) and the fetched files go. A cold
/// start is locked already and tells nothing. (Android: FerbApplication.kt tellMacOnLock.) Kept alive by the shell.
@MainActor
public final class LockTeller {
    private var follow: Follow<Bool>?
    private var was: Bool

    public init(session: Session, api: PhoneApi, local: LocalFiles) {
        was = session.locked
        follow = Follow({ session.locked }) { [weak self] locked in
            guard let self else { return }
            if locked && !was {
                Task { _ = await api.lock() }
                local.wipeFetched()
            }
            was = locked
        }
    }
}
