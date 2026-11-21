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
// - the planner's question (kb PHN-762): its words and "Tap to answer in FERB.", no buttons; the tap opens the chat;
// - words held back (a locked session that didn't prove the device key, PHN-126): "FERB has a question", wordless;
// - one of FERB's own checks while the phone is locked and the owner's setting is on: wordless, no buttons (PHN-422).
// Unknown needs (no status yet) are treated as needing the app: no buttons. Every notification carries a public text
// (FERB's name and "FERB has a question" or "FERB has news") for a lock screen that hides previews; no picture is
// ever put in one.

/// Which kind of notification (Android's channels): the shell maps it to a category, a thread and how it interrupts
/// (question, check and link: time-sensitive in Android's HIGH sense; done: ordinary).
public enum NotificationKind: String, Sendable {
    case question, check, done, link
    /// Insomnia mode (APP-981, APP-983): someone at the Mac, its battery, heat and cover (time-sensitive).
    case mac
}

/// What a tap on the notification opens.
public enum NotificationTap: Equatable, Sendable {
    /// The app as it is.
    case app
    /// The question's sheet ([AppModel.openQuestion]) once the app is unlocked.
    case question(String)
    /// The chat where the task is (Android passes the task id as its entry).
    case task(String)
    /// The chat, where the planner's question is answered by a message (kb PHN-762).
    case chat
    /// The Mac's camera (insomnia's Show me, APP-983) once the app is unlocked.
    case camera
}

/// One answer button. `choice` is the index in [Question.choices]; nil: a typed answer (the shell shows a text field
/// with `placeholder`).
public struct NotificationButton: Equatable, Sendable {
    /// The "Yes, until I'm back at my Mac" button's `choice` (not an index into the question's choices, PHN-611).
    public static let untilBack = -2
    /// Insomnia's notice (APP-983): Show me opens the app on the camera; Lock my Mac and It's fine are answered from
    /// the notification, the phone locked or not (not indexes into a question's choices).
    public static let showMe = -10
    public static let lockMac = -11
    public static let fine = -12

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

/// The one rule for FERB's questions in the phone's notifications (the owner, kb PHN-760: "i shoudl get it like nothc
/// comes down for all questions from worker or palnner no matter what. no notifications only when im in the app";
/// Android's QuestionAlerts.kt):
/// - every question that waits (the Mac's status: a worker's question or check, a hidden input, the planner's question)
///   is posted once, at once, unless he is in the app;
/// - in the app, unlocked, he sees it there: nothing is posted, and it is remembered as seen; if he leaves with it still
///   waiting, it is posted quietly (no sound, no banner), so it can still be answered from there (PHN-764);
/// - in the app but on its lock page he has not seen it: nothing yet, and a banner when he leaves without unlocking;
/// - one already posted is updated quietly when its words or needs change, never posted again; a question FERB asks
///   again is a new question (a new id) and is posted again;
/// - what no longer waits is removed (answered on the phone, the Mac or another phone; withdrawn; expired).
public final class QuestionAlerts {
    public struct Post: Equatable, Sendable {
        public let question: Question
        public let quiet: Bool
    }

    public struct Plan: Equatable, Sendable {
        public let post: [Post]
        public let update: [Question]
        public let cancel: [String]
        /// The announced ids to keep (`q:<id>`, and every other kind as it was).
        public let announced: Set<String>
    }

    private var seen = Set<String>()

    public init() {}

    /// `waiting`: every question that waits now, nil when nothing says yet (then nothing is posted or removed).
    public func decide(_ waiting: [Question]?, front: Bool, unlocked: Bool, posted: [String: Question], announced: Set<String>) -> Plan {
        guard let waiting else { return Plan(post: [], update: [], cancel: [], announced: announced) }
        let ids = Set(waiting.map(\.approvalId))
        seen.formIntersection(ids)
        var known = announced.filter { !$0.hasPrefix("q:") || ids.contains(String($0.dropFirst(2))) }
        var post: [Post] = []
        var update: [Question] = []
        for q in waiting {
            let key = "q:\(q.approvalId)"
            if known.contains(key) {
                if let shown = posted[q.approvalId], shown != q { update.append(q) }
                continue
            }
            if front {
                // in the app: shown there, not in the shade; seen only past the lock page
                if unlocked { seen.insert(q.approvalId) }
                continue
            }
            known.insert(key)
            post.append(Post(question: q, quiet: seen.contains(q.approvalId)))
        }
        let cancel = posted.keys.filter { !ids.contains($0) }.sorted()
        return Plan(post: post, update: update, cancel: cancel, announced: known)
    }
}

/// The decisions, pure over what the Mac's status and the transcript say: tested without iOS. What was announced
/// lives in [Prefs.announced] (ids only); what is on show now is kept here.
@MainActor
public final class NotificationPlanner {
    private let prefs: Prefs
    /// Questions on show now (re-planned when the phone locks or unlocks: FERB's checks change their words then).
    public private(set) var posted: [String: Question] = [:]
    /// What had finished when this process started is not news (questions are: each waiting one is posted once).
    private var primedStatus = false
    private var primedTranscript = false
    private var lastPhase: LinkPhase?
    /// The one rule for questions (kb PHN-760..764).
    private let alerts = QuestionAlerts()

    public init(prefs: Prefs) {
        self.prefs = prefs
    }

    /// One look at what waits (kb PHN-760; Android's Notifier.look): the Mac's status (every question, the planner's
    /// too, with its needs; it keeps coming while the app is locked) and, while unlocked, the transcript (`view`: the
    /// questions' full words, finished turns). Every question that waits is posted once unless the owner is in the app
    /// (`front`; `unlocked`: past its lock page, so he saw it there), by [QuestionAlerts]; what finished is posted
    /// once, and what was already finished when this process started is not news.
    public func look(status: PhoneStatus?, view: TranscriptView?, front: Bool, unlocked: Bool, lockedNow: Bool) -> [NotificationAction] {
        var out: [NotificationAction] = []
        let chat = view.map { Questions.open($0.questions) }
        let waiting: [Question]?
        if let status {
            let ids = Set(status.questions.map(\.approvalId))
            waiting = Questions.waiting((chat ?? []).filter { ids.contains($0.approvalId) }, status)
        } else {
            waiting = chat
        }
        let plan = alerts.decide(waiting, front: front, unlocked: unlocked, posted: posted, announced: prefs.announced)
        var announced = plan.announced
        for p in plan.post { out.append(.post(question(p.question, lockedNow: lockedNow, quietly: p.quiet))) }
        for q in plan.update { out.append(.post(question(q, lockedNow: lockedNow, quietly: true))) }
        for id in plan.cancel { out.append(cancel(id)) }
        // insomnia mode's notice, warnings and how it ended (APP-981, APP-983), from the same status
        if let status { out += insomnia(status.insomnia, announced: &announced, front: front) }
        let terminal = view.map(finished) ?? []
        // the first status and the first transcript this process reads: what had finished is not news
        if !primedStatus, let status {
            primedStatus = true
            announced.formUnion(status.results.map { "t:\($0.taskId)" })
        }
        if !primedTranscript, view != nil {
            primedTranscript = true
            announced.formUnion(terminal.map { "t:\($0.task)" })
        }
        for r in status?.results ?? [] {
            let key = "t:\(r.taskId)"
            if announced.contains(key) { continue }
            announced.insert(key)
            if front { continue }
            if r.held {
                out.append(.post(done(key, title: L.s("n_done_held"), body: L.s("n_held_text"), tap: .app, wordless: true)))
            } else {
                let failed = ChatModel.markOf(r.status) == .failed
                let line = WorkModel.oneLine(r.name.isEmpty ? ChatModel.plain(r.answer) : r.name)
                out.append(.post(done(key, title: L.s(failed ? "n_task_failed" : "n_task_done"), body: line, tap: .app, wordless: false)))
            }
        }
        for t in terminal {
            let key = "t:\(t.task)"
            if announced.contains(key) { continue }
            announced.insert(key)
            if !front { out.append(.post(done(key, title: t.title, body: t.line, tap: .task(t.task), wordless: false))) }
        }
        prefs.announced = announced
        return out
    }

    /// What each insomnia notification said as last posted (its count and words): a change updates it quietly.
    private var insomniaShown: [String: String] = [:]
    private var asleepPosted = false

    /// The Mac may be asleep (APP-981): one quiet notification while it can't be reached, gone when it is back.
    public func asleep(_ on: Bool) -> [NotificationAction] {
        if !on {
            defer { asleepPosted = false }
            return asleepPosted ? [.remove(id: "mac_asleep")] : []
        }
        if asleepPosted { return [] }
        asleepPosted = true
        return [.post(PlannedNotification(id: "mac_asleep", kind: .mac, title: L.s("n_insomnia_warning"), body: L.s("mac_asleep"),
                                          publicText: L.s("n_public_mac"), wordless: false, buttons: [], tap: .app, quietly: true))]
    }

    /// Insomnia mode's notifications (APP-981, APP-983; Android's Notifier.insomnia), each from the Mac's status:
    /// - the notice (someone touched the Mac, or an app opened that FERB didn't): posted once, with Show me (opens the
    ///   app on the camera, after its unlock), Lock my Mac and It's fine (answered from here, the phone locked or not);
    ///   a later touch of the same episode updates it quietly. Posted even while the app is on screen: someone at his
    ///   Mac is news wherever he is;
    /// - each warning (battery, heat, the cover): posted once, no buttons, a tap opens the app;
    /// - each is removed when the status no longer has it (answered, gone, the mode ended);
    /// - how the mode ended: said once, quietly (in the app the Mac's alert says it, kb APP-1109); never when the owner
    ///   turned it off himself (kb APP-1108).
    /// No camera frame is ever put in a notification. An older Mac (no `insomnia`) removes what was posted.
    func insomnia(_ i: Insomnia?, announced: inout Set<String>, front: Bool) -> [NotificationAction] {
        var out: [NotificationAction] = []
        var now = Set<String>()
        if let i, i.on, let n = i.notice {
            let key = "in:\(n.id)"
            now.insert(key)
            let known = announced.contains(key)
            let said = "\(n.count)|\(n.text)"
            // after a restart what was posted is taken as it is (never posted again)
            if known && insomniaShown[key] == nil { insomniaShown[key] = said }
            // a later touch of the same episode (its count), or new words from the Mac: updated quietly
            if !known || insomniaShown[key] != said {
                let title = L.s(n.kind == "activation" ? "n_insomnia_activation" : "n_insomnia_touched")
                let buttons = [
                    NotificationButton(label: L.s("insomnia_show_me"), choice: NotificationButton.showMe, placeholder: nil),
                    NotificationButton(label: L.s("insomnia_lock_mac"), choice: NotificationButton.lockMac, placeholder: nil),
                    NotificationButton(label: L.s("insomnia_fine"), choice: NotificationButton.fine, placeholder: nil),
                ]
                out.append(.post(PlannedNotification(id: key, kind: .mac, title: title, body: n.text.isEmpty ? title : n.text,
                                                     publicText: L.s("n_public_mac"), wordless: false, buttons: buttons,
                                                     tap: .app, quietly: known)))
                announced.insert(key)
                insomniaShown[key] = said
            }
        }
        if let i, i.on {
            for w in i.warnings {
                let key = "iw:\(w.id)"
                now.insert(key)
                let known = announced.contains(key)
                if known && insomniaShown[key] == nil { insomniaShown[key] = w.text }
                // the same warning with new words (the lid on battery follows the Mac's helper): updated quietly
                if known && insomniaShown[key] == w.text { continue }
                announced.insert(key)
                insomniaShown[key] = w.text
                out.append(.post(PlannedNotification(id: key, kind: .mac, title: L.s("n_insomnia_warning"), body: w.text,
                                                     publicText: L.s("n_public_mac"), wordless: false, buttons: [], tap: .app,
                                                     quietly: known)))
            }
        }
        // what the status no longer has goes (answered on the Mac or here, gone, the mode ended)
        for key in announced.filter({ ($0.hasPrefix("in:") || $0.hasPrefix("iw:")) && !now.contains($0) }).sorted() {
            announced.remove(key)
            insomniaShown[key] = nil
            out.append(.remove(id: key))
        }
        // how it ended: once, quietly; in the app the Mac's alert says it
        if let i, !i.on, let end = i.lastEnd, !end.text.isEmpty, end.why != "turned_off", !front, prefs.claimInsomniaEnd(end.id) {
            out.append(.post(PlannedNotification(id: "ie:\(end.id)", kind: .done, title: L.s("n_insomnia_warning"), body: end.text,
                                                 publicText: L.s("n_public_mac"), wordless: false, buttons: [], tap: .app,
                                                 quietly: true)))
        }
        return out
    }

    /// A notice's answer from here: gone at once on success (the status removes it too), else what stopped it.
    public func insomniaAnswered(_ noticeId: String, problem: String?) -> NotificationAction {
        guard let problem else { return .remove(id: "in:\(noticeId)") }
        return .post(PlannedNotification(id: "in:\(noticeId)", kind: .mac, title: L.s("n_not_answered_title"), body: problem,
                                         publicText: L.s("n_public_mac"), wordless: false, buttons: [], tap: .app, quietly: false))
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
        let asking = Self.headline(q) ?? L.s("n_question_title")
        let reason = Self.detail(q)
        let title: String
        let body: String
        if q.hidden {
            (title, body) = (L.s("n_private_title"), L.s("n_private_text"))
        } else if q.held {
            (title, body) = (L.s("n_question_title"), L.s("n_held_text"))
        } else if wordless {
            (title, body) = (L.s("n_check_title"), L.s("n_check_text"))
        } else if q.clarify {
            // the planner's question (kb PHN-762): his answer is a message in the chat, which the tap opens
            (title, body) = (asking, [reason, L.s("n_clarify_text")].compactMap { $0 }.joined(separator: "\n"))
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
                                   buttons: buttons, tap: q.clarify ? .chat : .question(id), quietly: quietly)
    }

    /// A notification's title and words for a question (Android's Notifier.headline and detail): the Mac's own banner
    /// words (`QuestionShown.bannerTitle`/`bannerText`, ferb-daemon notify.rs `approval_notice`) once the iPhone's
    /// bindings carry them; the planner's question, which has no banner on the Mac, "FERB needs you" and its own words.
    /// The one place a notification reads a question's words; the lock-screen and held-words rules apply on top.
    static func headline(_ q: Question) -> String? { q.clarify ? L.s("n_needs_you") : q.title }
    static func detail(_ q: Question) -> String? { q.reason.isEmpty ? nil : q.reason }

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
    fileprivate let api: PhoneApi
    private let session: Session
    private let lockedNow: () -> Bool
    fileprivate let deliver: ([NotificationAction]) -> Void
    private var changesTask: Task<Void, Never>?
    private var linkFollow: Follow<LinkStatus?>?
    /// One look at a time, in order (Android's Mutex).
    private var chain: Task<Void, Never>?

    /// The can't-reach time (seconds; AppModel's UnlockTiming.linkLost, kb PHN-753): tests pass a short one.
    private let lostAfter: Double
    private var asleepWait: Task<Void, Never>?

    public init(api: PhoneApi, prefs: Prefs, session: Session, lockedNow: @escaping () -> Bool,
                deliver: @escaping ([NotificationAction]) -> Void, lostAfter: Double = 10) {
        planner = NotificationPlanner(prefs: prefs)
        self.api = api
        self.session = session
        self.lockedNow = lockedNow
        self.deliver = deliver
        self.lostAfter = lostAfter
    }

    /// The link changed (APP-981): lost for the can't-reach time while insomnia mode was on, the Mac may be asleep (one
    /// quiet notification); back, or stopped for good, it goes.
    private func linkChanged(_ phase: LinkPhase?) {
        asleepWait?.cancel()
        asleepWait = nil
        guard let phase, phase != .online, phase != .stopped else {
            let a = planner.asleep(false)
            if !a.isEmpty { deliver(a) }
            return
        }
        let after = lostAfter
        asleepWait = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(after * 1_000_000_000))
            guard let self, !Task.isCancelled else { return }
            let status = await api.lastStatus()
            guard !Task.isCancelled else { return }
            let a = planner.asleep(MacAsleep.shown(status, api.link, lost: true))
            if !a.isEmpty { deliver(a) }
        }
    }

    private var sessionFollow: Follow<[Bool]>?
    /// The link's phase as last followed (the asleep notice follows its changes only).
    private var lastPhase: LinkPhase??

    public func start() {
        let stream = api.changes()
        changesTask = Task { [weak self] in
            for await c in stream {
                guard let self else { return }
                switch c {
                case .transcript, .live, .status: look { await $0.lookNow() }
                default: break
                }
            }
        }
        // he leaves the app, comes back, unlocks it: what waits is looked at again (a question seen in the app goes to
        // the shade when he leaves with it still waiting, kb PHN-764)
        sessionFollow = Follow<[Bool]>(initial: false, { [weak self] in
            guard let self else { return nil }
            return [session.front, session.locked]
        }) { [weak self] _ in
            self?.look { await $0.lookNow() }
        }
        linkFollow = Follow<LinkStatus?>({ [weak self] in
            guard let self else { return nil }
            return .some(api.link)
        }) { [weak self] l in
            guard let self else { return }
            let a = planner.link(l)
            if !a.isEmpty { deliver(a) }
            if l?.phase != lastPhase { lastPhase = l?.phase; linkChanged(l?.phase) }
        }
    }

    public func stop() {
        asleepWait?.cancel()
        changesTask?.cancel()
        linkFollow?.stop()
        sessionFollow?.stop()
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

    /// Looks at what waits now (a change of the Mac's status or the chat, the app's own state, or a test).
    @discardableResult
    public func lookNow() async -> [NotificationAction] {
        let status = await api.lastStatus()
        var view: TranscriptView?
        if !session.locked, case .ok(let v?) = await api.transcript() { view = v }
        let a = planner.look(status: status, view: view, front: session.front, unlocked: !session.locked, lockedNow: lockedNow())
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
        var status = await api.lastStatus()
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

extension Notifier {
    /// Lock my Mac (`lock` true) or It's fine from the insomnia notice (APP-983): sent at once, never queued, with no
    /// prompt (both are plain calls the Mac takes while the phone is locked). What stopped one is said (PHN-302). A
    /// launch for it waits for the phone to open and reach the Mac (bounded: iOS gives seconds).
    @discardableResult
    public func insomniaAnswer(_ noticeId: String, lock: Bool) async -> NotificationAction {
        _ = await waitFor(8) { api.state.isOpen && api.link?.phase == .online }
        let problem: String?
        if lock {
            if case .failed(let e) = await api.lockMac() { problem = AppModel.errorNote(e).text } else { problem = nil }
        } else {
            if case .failed(let e) = await api.dismissInsomniaNotice(noticeId) { problem = AppModel.errorNote(e).text } else { problem = nil }
        }
        let a = planner.insomniaAnswered(noticeId, problem: problem)
        deliver([a])
        return a
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
