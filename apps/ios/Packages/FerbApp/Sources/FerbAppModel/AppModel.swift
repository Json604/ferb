import FerbCore
import FerbDesign
import FerbMobile
import Foundation
import Observation

/// What fills the screen: one at a time, nothing private before `main` (PHN-8).
public enum Gate: Sendable { case starting, failed, noScreenLock, locked, pair, codes, removed, main }

/// Why the pairing screen shows again (or nil on a first run).
public enum PairReason: Sendable { case wiped, removed, keys }

/// What the phone can't do yet (nothing on the Mac's side carries it): the screen says so plainly and fakes nothing.
public enum Feature: Sendable {
    case voice
    public var wordsKey: String { "feature_voice" }
    public var textKey: String { "not_yet_voice" }
}

/// What slides up over the page. Only ids are kept (nothing saved holds question words, and never a private value).
public enum AppSheet: Equatable, Sendable {
    case asking(approvalId: String)
    case relay, profile, forget, typeCode, scan, ownRelay, notifications, help, stopAll, settings
    case days(key: String)
    case live(taskId: String)
    case notYet(Feature)
    /// The Mac asks for this phone's exact location once, for a task (GATE-613); the owner says yes or no here.
    case location(requestId: String)
    /// The words before iOS's own question, when the owner turns on sharing the city (SENSE-420).
    case city
    /// What reaches the phone while FERB is closed (iOS has no Stay connected, kb PHN-566): plain words.
    case background
}

/// A short line for the owner: a catalog key (Android's string name) and its arguments (the Mac's own words, when
/// the key is `err_mac_said` or `err_other`). Lines that say "your fingerprint" on Android name this phone's own kind
/// ([L.bio]).
public struct Note: Equatable, Sendable {
    public let key: String
    public let args: [String]

    public init(_ key: String, _ args: String...) {
        self.key = key
        self.args = args
    }

    /// The words as the screen shows them.
    public var text: String {
        args.isEmpty ? L.bio(key) : String(format: L.s(key), locale: Locale.current, arguments: args)
    }
}

/// This phone's own switches, as the profile sheet shows them (no Stay connected on iPhone, kb PHN-566).
public struct PhoneSettings: Equatable, Sendable {
    public var blockScreenshots: Bool
    public var hideChecks: Bool
    /// Epoch ms (0: not paired yet).
    public var pairedSince: Int64
    public var shareCity: Bool

    public init(blockScreenshots: Bool, hideChecks: Bool, pairedSince: Int64, shareCity: Bool) {
        self.blockScreenshots = blockScreenshots
        self.hideChecks = hideChecks
        self.pairedSince = pairedSince
        self.shareCity = shareCity
    }
}

/// The prompts only the screen can show: the system's Face ID / Touch ID / passcode prompt, with a key or without
/// one. [PhonePrompts] is the real one over FerbCore.
@MainActor
public protocol Prompts: AnyObject {
    /// A signer that shows the prompt for `kind` inside each signature (nil: no prompt can be shown now).
    func signer(_ kind: KeyKind, why: Why) -> Signer?
    /// The prompt alone (the app opens while the Mac can't be reached): true when the owner passed it.
    func local() async -> Bool
}

/// The real prompts: FerbCore's KeySigner (the key's own prompt) and Prompt.evaluate (no key).
@MainActor
public final class PhonePrompts: Prompts {
    private let phone: FerbPhone
    public init(_ phone: FerbPhone) { self.phone = phone }
    public func signer(_ kind: KeyKind, why: Why) -> Signer? { phone.signer(kind, reason: why.reason) }
    public func local() async -> Bool { await Prompt.evaluate(reason: Why.open.reason) }
}

/// One exact fix for the Mac's request (GATE-613; the shell's CoreLocation side).
@MainActor
public protocol Locator: AnyObject {
    /// iOS allows this app a location now (approximate or precise).
    func allowed() -> Bool
    /// One fresh fix, bounded by `timeout` seconds; nil when none came.
    func exact(timeout: Double) async -> LocationFix?
}

/// The Files tab: the places' counts, the folder open, the Mac's recent files and a search.
public struct FilesUi: Equatable, Sendable {
    public var folder: Folder? = nil
    public var loading: String? = nil
    public var problem: Note? = nil
    public var counts: [String: Int64] = [:]
    /// The Mac's recent files for this phone, or nil before (or when they couldn't be read: `recentNote`).
    public var recent: RecentFiles? = nil
    public var recentNote: Note? = nil
    public var search: SearchUi? = nil
    public init() {}
}

/// A search of names on the Mac: what was asked, where, and what came back (or why nothing did).
public struct SearchUi: Equatable, Sendable {
    public let query: String
    public let path: String?
    public var loading = true
    public var found: FoundFiles? = nil
    public var problem: Note? = nil

    public init(query: String, path: String?, loading: Bool = true, found: FoundFiles? = nil, problem: Note? = nil) {
        self.query = query
        self.path = path
        self.loading = loading
        self.found = found
        self.problem = problem
    }
}

/// What the screens draw (Android's UiState, field for field).
public struct UiState: Equatable {
    public var gate: Gate = .starting
    public var failure: String? = nil
    public var pairReason: PairReason? = nil
    public var pairing = false
    public var pairError: Note? = nil
    public var link: LinkStatus? = nil
    public var keys: KeyHealth? = nil
    public var conversation: String? = nil
    public var rows: [ChatRow] = []
    /// What the chat holds (the Tasks tab falls back to it when the Mac's own list can't be read).
    public var chatWork: Work = .empty
    public var work: Work = .empty
    /// What finished or failed since the app was last open (the home screen's list and the corpus's counts).
    public var away: [WorkItem] = []
    public var questions: [Question] = []
    public var older: OlderState = .unknown
    public var needsMacUnlock = false
    public var busy: Set<String> = []
    public var note: Note? = nil
    /// What FERB has waiting, as the Mac last said it (PHN-125): its questions' needs, Stop everything.
    public var status: PhoneStatus? = nil
    /// The Mac's own name and this phone's pairing date (kept by the library).
    public var mac: MacInfo? = nil
    /// The Mac's task list (`tasks.list`), or nil when it couldn't be read (the chat's list is used then).
    public var macTasks: [TaskRow]? = nil
    public var files = FilesUi()
    /// FERB's settings as read from the Mac, or nil before (or when they couldn't be read).
    public var settings: [MacSettingRow]? = nil
    /// A Face ID / Touch ID key made after pairing that the Mac doesn't know yet (PHN-133).
    public var newKeys = false
    public init() {}
}

/// What the screen keeps across a relaunch by the system (the shell stores it in scene storage): the tab, whether the
/// chat tab shows the home screen, the draft, and the questions already shown. Never a question's words or a private
/// value.
public struct SavedState: Codable, Equatable, Sendable {
    public var tab: String = "chat"
    public var home: Bool = true
    public var draft: String = ""
    public var presented: [String] = []
    public init() {}
}

/// The app's state over [PhoneApi]: the gate, the chat, tasks and questions, files, Watch live and FERB's settings,
/// and every action (Android's FerbViewModel, feature for feature). The shell makes one per scene and tells it the
/// scene's phase through [Session.foreground]; a relaunch reads it from the phone's store and [saved].
@MainActor
@Observable
public final class AppModel {
    public static let coldStartWait: Double = 6
    public static let tasksPage = 30
    /// How many recent files the Files tab asks for.
    public static let recentFilesCount = 20
    /// The exact fix's bound: the Mac waits 60 s for the answer; the owner's yes and the fix share that.
    public static let exactWait: Double = 20
    /// The lab's three places (PhoneFilesPage), as the Mac's paths.
    public static let places = ["~/Desktop", "~/Documents", "~/Downloads"]
    /// The Mac no longer knows this phone: pair again.
    public static let removedCodes: Set<String> = ["device_revoked", "not_paired", "unknown_phone"]

    public private(set) var ui = UiState()
    /// The chosen tab and whether the chat tab shows the home screen.
    public private(set) var tab: PhoneTab
    public private(set) var home: Bool
    public private(set) var draft: String
    public private(set) var settings: PhoneSettings
    public private(set) var sheet: AppSheet?
    /// Questions shown once already (presented or dismissed): a question slides up by itself only the first time.
    private var presented: Set<String>

    public let transfers: Transfers
    public let live: LiveWatch

    @ObservationIgnored private let api: PhoneApi
    @ObservationIgnored private let session: Session
    @ObservationIgnored private let prefs: Prefs
    @ObservationIgnored private let local: LocalFiles
    @ObservationIgnored private let screenLock: () -> Bool
    @ObservationIgnored private let corpus: Corpus?
    @ObservationIgnored private let now: () -> Date
    @ObservationIgnored private let locator: Locator?
    @ObservationIgnored private let coarse: () -> Coarse?
    @ObservationIgnored private let locationAnswered: () -> Void
    @ObservationIgnored private let box = PromptBox()
    @ObservationIgnored private let unlocker: Unlocker
    @ObservationIgnored private var follows: [AnyObject] = []
    @ObservationIgnored private var changesTask: Task<Void, Never>?
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var refreshing = false
    @ObservationIgnored private var dirty = false
    @ObservationIgnored private var tasksLoading = false
    @ObservationIgnored private var lastView: TranscriptView?
    @ObservationIgnored private var pending: String?
    @ObservationIgnored private var pendingLocation: String?
    /// Key places' files the owner opened in another app: they go when he is back in FERB (PHN-11).
    @ObservationIgnored private var openedKeyFiles: Set<String> = []
    /// The open before this one (the corpus's awayHours, lastOpen, firstToday, newPlace).
    @ObservationIgnored private var previousOpen: Double?
    @ObservationIgnored private var previousZone: String?

    /// The prompts the screen shows (set by the shell once its window exists).
    public var prompts: Prompts? {
        get { box.prompts }
        set { box.prompts = newValue }
    }

    /// A screen that must stay out of screen captures whatever the owner's switch says: a private answer being typed
    /// (PHN-110) and Watch live (PHN-454). The shell hides the app's content while the screen is captured then.
    public var mustSecure: Bool {
        switch sheet {
        case .live: return true
        case .asking(let id): return ui.questions.first { $0.approvalId == id }?.hidden == true
        default: return false
        }
    }

    /// What the shell keeps in scene storage (and passes back at the next launch).
    public var saved: SavedState {
        var s = SavedState()
        s.tab = Self.name(of: tab)
        s.home = home
        s.draft = draft
        s.presented = presented.sorted()
        return s
    }

    public init(api: PhoneApi, session: Session, prefs: Prefs, local: LocalFiles,
                screenLock: @escaping () -> Bool = { Biometry.screenLockSet }, corpus: Corpus? = .bundled,
                saved: SavedState = SavedState(), now: @escaping () -> Date = Date.init, locator: Locator? = nil,
                coarse: @escaping () -> Coarse? = { nil }, locationAnswered: @escaping () -> Void = {}) {
        self.api = api
        self.session = session
        self.prefs = prefs
        self.local = local
        self.screenLock = screenLock
        self.corpus = corpus
        self.now = now
        self.locator = locator
        self.coarse = coarse
        self.locationAnswered = locationAnswered
        tab = Self.tab(named: saved.tab)
        home = saved.home
        draft = saved.draft
        presented = Set(saved.presented)
        settings = PhoneSettings(blockScreenshots: prefs.blockScreenshots, hideChecks: prefs.hideChecksOnLockScreen,
                                 pairedSince: prefs.pairedSince, shareCity: prefs.shareCity)
        let box = self.box
        unlocker = Unlocker(api: api, keys: { api.state.keys }, signer: { kind, why in box.prompts?.signer(kind, why: why) })
        transfers = Transfers(api: api, local: local, unlocker: unlocker)
        live = LiveWatch(api: api)
        start()
    }

    private struct GateInput: Equatable {
        let state: PhoneState
        let link: LinkStatus?
        let locked: Bool
    }

    private func start() {
        follows.append(Follow({ [weak self] () -> GateInput? in guard let self else { return nil }; return GateInput(state: api.state, link: api.link, locked: session.locked) }) { [weak self] (g: GateInput) in
            guard let self else { return }
            ui.link = g.link
            ui.keys = g.state.keys
            gateOf(g.state, g.link, g.locked)
            showPending()
            if g.state.isOpen && g.link?.paired == true && prefs.pairedSince == 0 {
                prefs.pairedSince = Int64(now().timeIntervalSince1970 * 1000)
                prefs.justPaired = true
                refreshProfile()
            }
        })
        let stream = api.changes()
        changesTask = Task { [weak self] in
            for await c in stream {
                guard let self else { return }
                switch c {
                case .closed: break
                case .status: apply(lastView)
                case .mac: readMac()
                case .locationWanted(let id):
                    pendingLocation = id
                    showPending()
                default: refresh()
                }
            }
        }
        // the link came online while the app is open and unlocked: fetch what changed (one call, no timer)
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return api.link.map { $0.phase == .online && $0.paired } ?? false }) { [weak self] (online: Bool) in
            guard let self else { return }
            if online && !session.locked { Task { await self.syncNow() } }
        })
        // files cross the path again (PHN-444): what waited for it is tried once more, by itself
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return api.link.map { $0.bulkAllowed && $0.phase == .online } ?? false }) { [weak self] (bulk: Bool) in
            guard let self else { return }
            if bulk { transfers.bulkAllowed() }
        })
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return session.locked }) { [weak self] (locked: Bool) in
            guard let self else { return }
            if !locked {
                refresh()
                home = true
                // a request for the exact location that came while the app was closed or locked
                Task {
                    if case .ok(let id?) = await self.api.locationWanted() {
                        self.pendingLocation = id
                        self.showPending()
                    }
                }
            } else {
                // PHN-15: nothing private stays open; the preview stops, the fetched files go
                live.stop()
                switch sheet {
                case .live, .settings, .days, .location: sheet = nil
                default: break
                }
                transfers.locked()
                ui.files = FilesUi()
                ui.settings = nil
                ui.macTasks = nil
            }
        })
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return session.front }) { [weak self] (front: Bool) in
            guard let self else { return }
            if !front {
                // leaving the app: what finished so far has been seen; the preview stops (PHN-454)
                markSeen()
                if live.showing { closeLive() }
            } else {
                transfers.dropKeyPlaceFiles(openedKeyFiles)
                openedKeyFiles = []
                let local = self.local, at = now()
                Task.detached { local.dropOld(now: at) }
                markRead()
                showPending()
            }
        })
        readMac()
        refresh()
    }

    // ---- the gate -------------------------------------------------------------------------------------------------

    private func gateOf(_ s: PhoneState, _ l: LinkStatus?, _ locked: Bool) {
        var wiped: Lost?
        if case .open(_, let w) = s { wiped = w }
        let keys = s.keys
        let removed = l?.phase == .stopped && Self.removedCodes.contains(l?.problem?.code ?? "")
        let gate: Gate
        switch s {
        case .closed, .starting: gate = .starting
        case .failed: gate = .failed
        case .open:
            if !screenLock() { gate = .noScreenLock }
            else if locked { gate = .locked }
            else if let l {
                if l.phase == .pairingWaitForMac { gate = .codes }
                else if removed { gate = .removed }
                else if !l.paired { gate = .pair }
                else if keys?.pairAgain == true { gate = .removed }
                else { gate = .main }
            } else { gate = .starting }
        }
        let reason: PairReason?
        if removed { reason = .removed }
        else if keys?.pairAgain == true && l?.paired == true { reason = .keys }
        else if wiped != nil { reason = .wiped }
        else { reason = nil }
        if ui.pairing, let l {
            if l.paired && l.phase == .online { ui.pairing = false }
            else if l.phase == .stopped || l.phase == .waiting || l.phase == .idle, let p = l.problem {
                ui.pairing = false
                ui.pairError = Self.pairNote(p.code, p.detail)
            }
        }
        ui.gate = gate
        ui.pairReason = reason
        if case .failed(let code, let detail) = s { ui.failure = "\(code): \(detail)" } else { ui.failure = nil }
    }

    /// Reads the gate again (a passcode may have been set in Settings meanwhile).
    public func recheck() {
        gateOf(api.state, api.link, session.locked)
        showPending()
    }

    /// A notification's tap: the question opens as soon as the app is unlocked.
    public func openQuestion(_ approvalId: String) {
        pending = approvalId
        tab = .chat
        showPending()
    }

    /// A notification's tap: the Mac's request for the exact location opens as soon as the app is unlocked.
    public func openLocation(_ requestId: String) {
        pendingLocation = requestId
        showPending()
    }

    private func showPending() {
        if ui.gate != .main || session.locked { return }
        if let id = pending {
            pending = nil
            show(.asking(approvalId: id))
            return
        }
        guard let loc = pendingLocation else { return }
        // in front: the sheet; in the background the notification posts the request instead (no answer without the app)
        if !session.front { return }
        if let s = sheet, !s.isLocation { return }
        pendingLocation = nil
        show(.location(requestId: loc))
    }

    // ---- reading ---------------------------------------------------------------------------------------------------

    /// Reads the transcript again (coalesced: a burst of changes is one read after the one under way).
    @discardableResult
    public func refresh() -> Task<Void, Never>? {
        if refreshing {
            dirty = true
            return refreshTask
        }
        refreshing = true
        refreshTask = Task {
            repeat {
                dirty = false
                var view: TranscriptView?
                if case .ok(let v) = await api.transcript() { view = v }
                apply(view)
            } while dirty
            refreshing = false
            markRead()
            if tab == .tasks { loadTasks() }
        }
        return refreshTask
    }

    private func markSeen() {
        let done = ui.chatWork.earlier.compactMap(\.entry)
        if !done.isEmpty { prefs.seen = prefs.seen.union(done) }
    }

    /// The transcript and the Mac's last status (each question's needs, Stop everything, PHN-125) as the screens draw them.
    private func apply(_ view: TranscriptView?) {
        lastView = view
        let status = api.lastStatus()
        let questions = Questions.merged(view.map { Questions.open($0.questions) } ?? [], status)
        let chatWork = WorkModel.of(view, asking: "")
        if prefs.lastOpen == 0 && prefs.seen.isEmpty { prefs.seen = Set(chatWork.earlier.compactMap(\.entry)) }
        let seen = prefs.seen
        ui.conversation = view?.conversation
        ui.rows = ChatModel.rows(view)
        ui.chatWork = chatWork
        ui.away = chatWork.earlier.filter { w in w.entry.map { !seen.contains($0) } ?? true }
        ui.questions = questions
        ui.older = view?.older ?? .unknown
        ui.status = status
        ui.needsMacUnlock = ui.needsMacUnlock || (view?.outbox.contains { $0.waitingFor == "unlock" } ?? false)
        rebuildWork()
        // a question slides up over whatever is open, once (MobileScreens.swift QuestionSheet)
        if let fresh = questions.first(where: { !presented.contains($0.approvalId) && !$0.held }),
           ui.gate == .main, session.front, sheet == nil {
            show(.asking(approvalId: fresh.approvalId))
        }
        if case .asking(let id) = sheet, !questions.contains(where: { $0.approvalId == id }) {
            // answered on the Mac or elsewhere, or expired: the sheet goes and says so
            sheet = nil
            note(Note("question_gone"))
        }
    }

    /// The Tasks tab: the Mac's own list where it answers, else the chat's (offline, or the Mac's window closed).
    private func rebuildWork() {
        let waiting = waitingOf(ui.questions, ui.chatWork)
        if let mac = ui.macTasks {
            ui.work = WorkModel.fromMac(mac, waiting: waiting, chat: ui.chatWork)
        } else {
            var w = ui.chatWork
            w.waiting = waiting
            ui.work = w
        }
    }

    private func waitingOf(_ questions: [Question], _ chat: Work) -> [WorkItem] {
        var fromChat: [String: WorkItem] = [:]
        for w in chat.waiting { if let id = w.approvalId { fromChat[id] = w } }
        return questions.map { q in
            fromChat[q.approvalId] ?? WorkItem(kind: .asking, title: q.title ?? "", detail: WorkModel.oneLine(q.reason),
                                               approvalId: q.approvalId, taskId: q.taskId)
        }
    }

    /// Reads the Mac's task list (no prompt: while the Mac's window is closed the chat's list stays).
    @discardableResult
    public func loadTasks() -> Task<Void, Never>? {
        if tasksLoading { return nil }
        tasksLoading = true
        return Task {
            defer { tasksLoading = false }
            if session.locked || api.link?.phase != .online { return }
            switch await api.tasks(limit: Self.tasksPage, before: nil) {
            case .ok(let page): ui.macTasks = page.tasks
            case .failed(let e): if case .NeedsUnlock = e { ui.needsMacUnlock = true }
            }
            rebuildWork()
        }
    }

    private func readMac() {
        Task { if case .ok(let m?) = await api.cachedMacInfo() { ui.mac = m } }
    }

    /// After the Mac's unlock: its name and this phone's pairing date, and whether a key waits to be registered.
    private func afterMacUnlock() async {
        if case .ok(let m) = await api.macInfo() { ui.mac = m }
        let fresh = await api.unregisteredKeys()
        ui.newKeys = fresh?.strong != nil
    }

    private func syncNow() async {
        switch await api.sync() {
        case .failed(let e): if case .NeedsUnlock = e { ui.needsMacUnlock = true }
        case .ok: ui.needsMacUnlock = false
        }
    }

    /// The chat on screen, the app open and unlocked: the Mac learns how far the owner has read (one mark, latest).
    private func markRead() {
        guard let conv = ui.conversation else { return }
        if ui.gate != .main || session.locked || !session.front { return }
        if tab != .chat || home { return }
        Task { _ = await api.markReadLatest(conversation: conv) }
    }

    // ---- the app lock (PHN-8, PHN-13, PHN-125) ---------------------------------------------------------------------

    /// One prompt opens the app and unlocks the Mac (PHN-13): with the Mac reachable the lock key signs the Mac's
    /// challenge inside the prompt; otherwise the prompt alone opens the app (the library is told: `unlocked`) and
    /// the Mac is unlocked on the next action that needs it. A cold start waits a few seconds for the link so it
    /// stays one prompt.
    @discardableResult
    public func unlockApp() -> Task<Void, Never> {
        busy("unlock") { [self] in
            guard let p = prompts else { return }
            if api.link?.paired == true && api.link?.phase != .online {
                _ = await waitFor(Self.coldStartWait) { api.link?.phase == .online }
            }
            let online = api.link.map { $0.paired && $0.phase == .online } ?? false
            if online && api.state.keys?.lock == .ok, let inner = p.signer(.lock, why: .open) {
                let signed = Recording(inner)
                let r = await api.unlock(signed)
                if signed.signed {
                    session.unlocked()
                    opened()
                    if r.error != nil { _ = await api.unlocked() }
                    ui.needsMacUnlock = r.error != nil
                    if r.error == nil { await afterMacUnlock() }
                    return
                }
                if case .failed(let e) = r, case .NotSigned = e { return } // cancelled
            }
            if await p.local() {
                session.unlocked()
                opened()
                _ = await api.unlocked()
                if api.link?.paired == true { ui.needsMacUnlock = true }
            }
        }
    }

    /// The Mac's unlock alone (its window closed while the app stayed open).
    @discardableResult
    public func unlockMac() -> Task<Void, Never> {
        busy("unlock-mac") { [self] in
            guard let p = prompts, let s = p.signer(.lock, why: .open) else { return }
            switch await api.unlock(s) {
            case .ok:
                ui.needsMacUnlock = false
                refresh()
                await afterMacUnlock()
            case .failed(let e):
                if case .NotSigned = e { return }
                failed(e)
            }
        }
    }

    private func opened() {
        // the first open after pairing is the one in which the pairing happened; the next open is an ordinary one
        prefs.justPaired = false
        previousOpen = prefs.lastOpen > 0 ? Double(prefs.lastOpen) : nil
        previousZone = prefs.lastZone
        prefs.lastOpen = Int64(now().timeIntervalSince1970)
        prefs.lastZone = TimeZone.current.identifier
    }

    /// PHN-133: registers the Face ID / Touch ID key made after pairing (one prompt), so hard Yeses can use it.
    @discardableResult
    public func registerKeys() -> Task<Void, Never> {
        busy("keys") { [self] in
            guard let p = prompts else { return }
            let lock = p.signer(.lock, why: .keys) ?? Declined()
            let strong = p.signer(.strong, why: .keys) ?? Declined()
            switch await api.registerKeys({ kind in kind == .strong ? strong : lock }) {
            case .ok(let did):
                ui.newKeys = false
                if did { note(Note("keys_registered")) }
            case .failed(let e):
                if case .NotSigned = e { return }
                failed(e)
            }
        }
    }

    // ---- pairing (PHN-442, PHN-447) ---------------------------------------------------------------------------------

    @discardableResult
    public func pair(_ code: String) -> Task<Void, Never> {
        busy("pair") { [self] in
            let text = code.trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty { return }
            ui.pairing = true
            ui.pairError = nil
            sheet = nil
            if case .failed(let e) = await api.pair(text) {
                ui.pairing = false
                ui.pairError = Self.pairNote(e.linkCode ?? e.code, e.detail)
            }
        }
    }

    /// Cancel on the codes screen: the half-made pairing is forgotten, ready to start again.
    @discardableResult
    public func cancelPairing() -> Task<Void, Never> {
        busy("pair") { [self] in
            ui.pairing = false
            _ = await api.forget()
        }
    }

    public func clearPairError() { ui.pairError = nil }

    // ---- chat -----------------------------------------------------------------------------------------------------

    public func typed(_ text: String) {
        draft = text
        if !text.isEmpty { home = false }
    }

    /// Sends through the outbox (PHN-401): with the Mac off it waits there, visible and cancellable. Files go with it.
    @discardableResult
    public func send() -> Task<Void, Never> {
        busy("send") { [self] in
            let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
            let files = transfers.ready
            if text.isEmpty && files.isEmpty { return }
            draft = ""
            home = false
            let r = files.isEmpty ? await api.send(text) : await api.sendWith(text, attachments: files)
            switch r {
            case .ok(let item):
                if !files.isEmpty { transfers.sent() }
                if item.waitingFor == "unlock" { ui.needsMacUnlock = true }
            case .failed(let e):
                draft = text
                failed(e)
            }
            await refresh()?.value
        }
    }

    @discardableResult
    public func cancelOutbox(_ id: String) -> Task<Void, Never> {
        busy("cancel:\(id)") { [self] in
            switch await api.cancel(outboxId: id) {
            case .ok(let c): if c.mayHaveArrived { note(Note("outbox_may_have_arrived")) }
            case .failed(let e): failed(e)
            }
            await refresh()?.value
        }
    }

    /// Stop needs no unlock (PHN-125: a locked connection can stop FERB) and is never queued.
    @discardableResult
    public func stop(_ taskId: String?) -> Task<Void, Never> {
        busy("stop:\(taskId ?? "")") { [self] in
            guard let taskId else { return }
            switch await api.stop(taskIds: [taskId]) {
            case .ok: note(Note("stopped"))
            case .failed(let e): failed(e)
            }
        }
    }

    /// "Stop everything on the Mac" (DMN-1, PHN-125): after the owner's confirmation; no prompt, never queued.
    @discardableResult
    public func stopEverything() -> Task<Void, Never> {
        busy("stop-all") { [self] in
            sheet = nil
            switch await api.stopEverything() {
            case .ok:
                note(Note("stop_all_done"))
                _ = await api.status()
                apply(lastView)
            case .failed(let e):
                failed(e)
            }
        }
    }

    @discardableResult
    public func loadOlder() -> Task<Void, Never> {
        busy("older") { [self] in
            guard let conv = ui.conversation, ui.older == .available else { return }
            if case .failed(let e) = await api.loadOlder(conversation: conv), case .NeedsUnlock = e { ui.needsMacUnlock = true }
            await refresh()?.value
        }
    }

    // ---- questions (PHN-415, PHN-417, PHN-470) ----------------------------------------------------------------------

    /// Sends an answer. Where the Mac's status says the answer needs the phone's proof, Face ID / Touch ID (on a phone
    /// without it, the lock key's prompt) is asked up front; a plain No and an empty answer never ask. A refusal still
    /// asks once, as before (AnswerFlow). Kb PHN-520.
    @discardableResult
    public func answer(_ approvalId: String, _ input: AnswerInput) -> Task<Void, Never> {
        busy("answer:\(approvalId)") { [self] in
            guard let p = prompts else { return }
            let q = ui.questions.first { $0.approvalId == approvalId }
            let declines: Bool
            switch input {
            case .approve(let granted, _): declines = !granted
            case .words(let text): declines = text.isEmpty
            default: declines = false
            }
            var upfront: KeyKind?
            if let q, q.needsProof, !declines { upfront = q.needs?.fingerprint == true ? .strong : .lock }
            let flow = AnswerFlow(api: api, keys: { [api] in api.state.keys }, signer: { kind in p.signer(kind, why: .yes) })
            answered(approvalId, await flow.answer(approvalId, input, upfront: upfront))
        }
    }

    private func answered(_ approvalId: String, _ r: Answered) {
        switch r {
        case .sent:
            if case .asking(let id) = sheet, id == approvalId { sheet = nil }
            refresh()
        case .notSent: break
        case .needsPairingAgain: note(Note("answer_needs_pairing"))
        case .failed(let e): failed(e)
        }
    }

    /// A hidden answer (PHN-109, PHN-110; kb PHN-536): the typed UTF-16 units go into the library's [SecretInput] one
    /// at a time (each a plain number, so the secret never becomes a String here), `units` is overwritten with zeros
    /// at once, before anything else happens, and the SecretInput is emptied when the answer ends, whether it was
    /// sent, refused or the prompt was cancelled (the owner types it again then). Nothing the model keeps holds it.
    @discardableResult
    public func answerHidden(_ approvalId: String, units: inout [UInt16]) -> Task<Void, Never> {
        let secret = SecretInput()
        for u in units { secret.push(unit: u) }
        for i in units.indices { units[i] = 0 }
        let what = "answer:\(approvalId)"
        return Task { [self] in
            defer { secret.clear() }
            guard let p = prompts, !ui.busy.contains(what) else { return }
            ui.busy.insert(what)
            defer { ui.busy.remove(what) }
            let flow = AnswerFlow(api: api, keys: { [api] in api.state.keys }, signer: { kind in p.signer(kind, why: .yes) })
            answered(approvalId, await flow.answerHidden(approvalId, secret: secret))
        }
    }

    // ---- files (PHN-113, PHN-115, PHN-121, PHN-127) ----------------------------------------------------------------

    /// The Files tab opened: the Mac's recent files for this phone (the Mac's unlock: asked once if its window closed)
    /// and the places' counts, read only when cheap (no prompt; left out when they can't be read).
    @discardableResult
    public func filesOpened() -> Task<Void, Never>? {
        if session.locked || api.link?.phase != .online { return nil }
        let recent = loadRecent()
        if !ui.files.counts.isEmpty { return recent }
        return Task {
            await recent.value
            var counts: [String: Int64] = [:]
            for place in Self.places {
                guard case .ok(let f) = await api.files(path: place, signer: nil) else { break }
                counts[place] = Int64(f.total)
            }
            ui.files.counts = counts
        }
    }

    /// The Mac's recent files: made by FERB, fetched or sent by this phone (T-1211).
    @discardableResult
    public func loadRecent() -> Task<Void, Never> {
        busy("recent") { [self] in
            switch await unlocker.run({ await api.recentFiles(limit: Self.recentFilesCount) }) {
            case .ok(let r):
                ui.files.recent = r
                ui.files.recentNote = nil
            case .failed(let e):
                if case .NotSigned = e { return }
                ui.files.recentNote = Self.errorNote(e)
            }
        }
    }

    /// Searches names on the Mac, inside the folder open (or the whole home folder). Never by contents.
    @discardableResult
    public func search(_ query: String) -> Task<Void, Never>? {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if q.isEmpty { return nil }
        let path = ui.files.folder?.path
        ui.files.search = SearchUi(query: q, path: path)
        return busy("search") { [self] in
            let r = await unlocker.run { await api.searchFiles(query: q, path: path) }
            guard var s = ui.files.search, s.query == q, s.path == path else { return }
            s.loading = false
            switch r {
            case .ok(let found): s.found = found
            case .failed(let e):
                if case .NotSigned = e { s.problem = Note("files_not_opened") } else { s.problem = Self.errorNote(e) }
            }
            ui.files.search = s
        }
    }

    public func closeSearch() { ui.files.search = nil }

    /// A found or recent file: a folder opens in the browser (the search closes), a file is fetched as from a folder.
    public func openFound(_ f: FoundFile) {
        if f.dir {
            closeSearch()
            openFolder(f.path)
        } else {
            transfers.fetch(f.path, name: f.name, size: Int64(f.size), keyPlace: false)
        }
    }

    /// Gets the open folder whole (PHN-13): the Mac lists every file under it, then each is fetched into this phone's
    /// fetched files. A key place's folder is not fetched whole (each of its files asks for Face ID by itself).
    @discardableResult
    public func fetchFolder() -> Task<Void, Never> {
        busy("tree") { [self] in
            guard let f = ui.files.folder, !f.locked else { return }
            switch await unlocker.run({ await api.tree(path: f.path, signer: nil) }) {
            case .ok(let tree):
                if tree.files.isEmpty { note(Note("files_tree_empty")) } else { transfers.fetchFolder(tree) }
            case .failed(let e):
                if case .NotSigned = e { return }
                ui.files.problem = filesNote(e)
            }
        }
    }

    /// A folder the owner picked in the system's folder picker: sent whole to the Mac.
    @discardableResult
    public func sendFolder(_ url: URL, forChat: Bool) -> Task<Void, Never> {
        busy("stage") { [self] in
            let local = self.local
            let folder = await Task.detached { local.folder(url) }.value
            guard let folder else { return note(Note("files_folder_unreadable")) }
            if folder.files.isEmpty {
                local.release(folder)
                return note(Note("files_folder_empty"))
            }
            transfers.sendFolder(folder, forChat: forChat)
        }
    }

    /// A file on a finished task's result card (kb PHN-535): fetched by the task and its place; opened on a tap.
    public func fetchTaskFile(_ taskId: String, index: Int, name: String) {
        transfers.fetchTaskFile(taskId, index: UInt64(index), name: name)
    }

    /// Opens a folder of the Mac; a key place asks for Face ID / Touch ID for that very path first (PHN-121).
    @discardableResult
    public func openFolder(_ path: String, keyPlace: Bool = false) -> Task<Void, Never> {
        busy("folder") { [self] in
            ui.files.loading = path
            ui.files.problem = nil
            let r = await unlocker.withProof(keyPlace, .file) { signer in await api.files(path: path, signer: signer) }
            ui.files.loading = nil
            switch r {
            case .ok(let f): ui.files.folder = f
            case .failed(let e): ui.files.problem = filesNote(e)
            }
        }
    }

    /// Up one folder; from a place, back to the places.
    public func folderUp() {
        guard let f = ui.files.folder else { return }
        if let parent = f.parent, !Self.places.contains(f.path), f.path != "~" {
            openFolder(parent)
        } else {
            ui.files.folder = nil
            ui.files.problem = nil
        }
    }

    /// A folder opens (its task returned), a file is fetched.
    @discardableResult
    public func openEntry(_ e: FileEntry) -> Task<Void, Never>? {
        if e.dir { return openFolder(e.path, keyPlace: e.locked) }
        transfers.fetch(e.path, name: e.name, size: Int64(e.size), keyPlace: e.locked)
        return nil
    }

    /// The owner opened or shared a fetched file in another app (his tap: Quick Look's or the share sheet's): a key
    /// place's copy goes when he is back (kb PHN-516).
    public func handedOut(_ key: String) {
        if transfers.list.first(where: { $0.key == key })?.keyPlace == true { openedKeyFiles.insert(key) }
    }

    /// Files the owner picked in the system's picker (their URLs): copied here, then sent to the Mac.
    @discardableResult
    public func sendFiles(_ urls: [URL], forChat: Bool) -> Task<Void, Never> {
        busy("stage") { [self] in
            if urls.isEmpty { return }
            let local = self.local
            let staged = await Task.detached { urls.compactMap(local.stage) }.value
            if staged.count < urls.count { note(Note("files_unreadable")) }
            if !staged.isEmpty { transfers.send(staged, forChat: forChat) }
        }
    }

    /// The files of a finished batch go to the composer, to send with a message.
    public func attachToChat(_ batch: String) {
        transfers.attach(batch)
        openChat()
    }

    /// Pauses a file, or the file under way in a batch.
    public func pause(_ key: String) {
        guard let t = transfers.list.first(where: { $0.key == key }) else { return }
        let running = t.batch.map { b in transfers.list.first { $0.batch == b && $0.phase == .running } } ?? t
        if let running { transfers.pause(running.key) }
    }

    private func filesNote(_ e: FerbError) -> Note {
        if case .NotSigned = e { return Note("files_not_opened") }
        return Self.errorNote(e)
    }

    // ---- Watch live (PHN-454) ----------------------------------------------------------------------------------------

    /// Watch live: never asked for on the public relay (its rule is said instead); elsewhere a full screen of its own.
    public func watchLive(_ taskId: String?) {
        guard let taskId else { return }
        let l = api.link
        if l?.path == .publicRelay || (l?.phase == .online && l?.bulkAllowed == false) {
            note(Note("not_on_shared_relay"))
            return
        }
        sheet = .live(taskId: taskId)
        live.start(taskId)
    }

    /// The Mac's window closed under the preview: unlock (one prompt) and watch again.
    @discardableResult
    public func watchAgain() -> Task<Void, Never> {
        busy("live") { [self] in
            guard case .live(let taskId) = sheet, let p = prompts else { return }
            if live.view?.needsUnlock == true {
                guard let s = p.signer(.lock, why: .open) else { return }
                switch await api.unlock(s) {
                case .ok: ui.needsMacUnlock = false
                case .failed(let e):
                    if case .NotSigned = e {} else { failed(e) }
                    return
                }
            }
            live.start(taskId)
        }
    }

    public func closeLive() {
        live.stop()
        if case .live = sheet { sheet = nil }
    }

    // ---- FERB's settings (PHN-404, PHN-112, PHN-133) ----------------------------------------------------------------

    @discardableResult
    public func openSettings() -> Task<Void, Never> {
        busy("settings") { [self] in
            show(.settings)
            switch await unlocker.run({ await api.settings() }) {
            case .ok(let values): ui.settings = MacSettings.rows(values)
            case .failed(let e):
                if case .NotSigned = e {} else { failed(e) }
                if ui.settings == nil && sheet == .settings { sheet = nil }
            }
        }
    }

    /// One change, one prompt: the Face ID / Touch ID key where the phone has one, else the lock key (PHN-133).
    @discardableResult
    public func setSetting(_ key: String, valueJson: String) -> Task<Void, Never> {
        busy("setting:\(key)") { [self] in
            guard let spec = MacSettings.spec(key) else { return }
            guard let kind = unlocker.freshKind() else { return note(Note("needs_pairing_again")) }
            guard let p = prompts, let signer = p.signer(kind, why: .setting) else { return }
            switch await unlocker.run({ await api.setSetting(key: key, valueJson: valueJson, signer: signer) }) {
            case .ok(let stored):
                let row = MacSettings.row(spec, stored)
                ui.settings = ui.settings?.map { $0.spec.key == key ? row : $0 }
                if case .days = sheet { sheet = .settings }
            case .failed(let e):
                if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    @discardableResult
    public func setSwitch(_ key: String, on: Bool) -> Task<Void, Never> { setSetting(key, valueJson: on ? "true" : "false") }

    @discardableResult
    public func setDays(_ key: String, text: String) -> Task<Void, Never>? {
        guard let days = Int(text.trimmingCharacters(in: .whitespacesAndNewlines)), days >= 1 else {
            note(Note("set_days_invalid"))
            return nil
        }
        return setSetting(key, valueJson: String(days))
    }

    // ---- navigation and sheets ------------------------------------------------------------------------------------

    public func choose(_ t: PhoneTab) {
        if t == .chat && tab == .chat { home.toggle() }
        tab = t
        switch t {
        case .tasks: loadTasks()
        case .files: filesOpened()
        case .chat: markRead()
        default: break
        }
    }

    public func openChat() {
        tab = .chat
        home = false
        markRead()
    }

    public func show(_ s: AppSheet?) {
        if case .asking(let id) = s { presented.insert(id) }
        if case .live = sheet {
            if case .live = s {} else { live.stop() }
        }
        sheet = s
        if s == nil && pendingLocation != nil { showPending() }
    }

    public func note(_ n: Note?) { ui.note = n }

    /// The profile's switches changed outside the model: read them again.
    public func refreshProfile() {
        settings = PhoneSettings(blockScreenshots: prefs.blockScreenshots, hideChecks: prefs.hideChecksOnLockScreen,
                                 pairedSince: prefs.pairedSince, shareCity: prefs.shareCity)
    }

    public func hideChecks(_ on: Bool) {
        prefs.hideChecksOnLockScreen = on
        refreshProfile()
    }

    /// PHN-420 on iOS (kb PHN-565): hide FERB while the screen is recorded or mirrored.
    public func blockScreenshots(_ on: Bool) {
        prefs.blockScreenshots = on
        refreshProfile()
    }

    /// Forget this Mac (PHN-408): the fetched chat, the Mac, the keys and the files here go; the pairing screen comes.
    @discardableResult
    public func forget() -> Task<Void, Never> {
        busy("forget") { [self] in
            sheet = nil
            live.stop()
            switch await api.forget() {
            case .ok:
                prefs.forgetPairing()
                tab = .chat
                home = true
                presented = []
                transfers.clear()
                ui.mac = nil
                ui.macTasks = nil
                ui.files = FilesUi()
                ui.settings = nil
                ui.status = nil
                refreshProfile()
            case .failed(let e):
                failed(e)
            }
        }
    }

    /// The home screen's words for this moment (the lab's corpus, FerbDesign's words engine; kb PHN-507).
    public func homeWords(_ sky: SkyMoment) -> HomeTexts? {
        guard let corpus else { return nil }
        let link: String
        switch ui.link?.path {
        case .direct: link = "direct"
        case .ownRelay: link = "own"
        case .publicRelay: link = "shared"
        default: link = "offline"
        }
        let m = Moment(sky: sky, waiting: ui.work.waiting.count, done: ui.away.filter { $0.kind == .done }.count,
                       failed: ui.away.filter { $0.kind == .failed }.count, running: ui.work.running.count, link: link,
                       firstRun: ui.link?.paired != true, justPaired: prefs.justPaired,
                       lastOpen: previousOpen.map { Date(timeIntervalSince1970: $0) }, lastZone: previousZone)
        return HomeTexts(m, corpus: corpus)
    }

    /// The sky's place (PHN-420): the last coarse fix, rounded to 2 decimals, while the owner shares the city; else
    /// the time zone's principal place.
    public func skyPlace() -> SkyPlace {
        let zone = TimeZone.current.identifier
        if let c = coarse() { return SkyPlace(name: zone, latitude: c.latitude, longitude: c.longitude, zone: zone) }
        return SkyPlace.fromZone()
    }

    // ---- the exact location for a task (GATE-613, SENSE-420; kb PHN-537) -----------------------------------------------

    /// The owner said yes on the sheet: iOS's permission was asked by the screen first (`granted`); then one fix (at
    /// most [exactWait]) goes to the Mac, or the reason none does.
    @discardableResult
    public func shareLocation(_ requestId: String, granted: Bool) -> Task<Void, Never> {
        busy("location") { [self] in
            let l = locator
            let can = granted && (l?.allowed() ?? false)
            let fix = can ? await l?.exact(timeout: Self.exactWait) : nil
            let why: String? = fix != nil ? nil : (can ? L.s("loc_why_none") : L.s("loc_why_denied"))
            await answerLocation(requestId, fix, why)
        }
    }

    /// The owner said no: the Mac is told at once, plainly.
    @discardableResult
    public func declineLocation(_ requestId: String) -> Task<Void, Never> {
        busy("location") { [self] in await answerLocation(requestId, nil, L.s("loc_why_no")) }
    }

    private func answerLocation(_ requestId: String, _ fix: LocationFix?, _ why: String?) async {
        if case .location(let id) = sheet, id == requestId { sheet = nil }
        locationAnswered()
        switch await api.answerLocation(requestId: requestId, fix: fix, whyNot: why) {
        case .ok: note(Note(fix != nil ? "exact_sent" : "exact_not_sent"))
        case .failed(let e): failed(e)
        }
    }

    /// The owner's switch for sharing the city changed (the screen asked iOS first): read the profile again.
    public func cityChanged(granted: Bool) {
        if !granted { note(Note("city_denied")) }
        refreshProfile()
    }

    // ---- helpers ---------------------------------------------------------------------------------------------------

    private func failed(_ e: FerbError) { note(Self.errorNote(e)) }

    /// Runs `block` once at a time per `what` (the screen greys the control while `ui.busy` holds it).
    @discardableResult
    private func busy(_ what: String, _ block: @escaping @MainActor () async -> Void) -> Task<Void, Never> {
        Task { [self] in
            if ui.busy.contains(what) { return }
            ui.busy.insert(what)
            await block()
            ui.busy.remove(what)
        }
    }

    public static func errorNote(_ e: FerbError) -> Note {
        if e.linkCode == Unlocker.pairAgain { return Note("needs_pairing_again") }
        switch e {
        case .Offline: return Note("err_offline")
        case .NeedsBetterPath: return Note("err_needs_better_path")
        case .NeedsUnlock: return Note("err_needs_unlock")
        case .Refused, .NotFound: return Note("err_mac_said", e.detail)
        default: return Note("err_other", e.detail)
        }
    }

    public static func pairNote(_ code: String?, _ detail: String?) -> Note {
        switch code {
        case "pair_window_closed": return Note("pair_expired")
        case "pair_secret_wrong", "bad_request", "refused": return Note("pair_wrong_code")
        case "device_limit": return Note("pair_limit")
        case "pair_denied": return Note("pair_denied")
        case "pair_busy": return Note("pair_busy")
        case "already_paired": return Note("pair_already")
        case "mac_unreachable", "offline": return Note("pair_unreachable")
        default: return Note("err_other", detail ?? "")
        }
    }

    static func name(of tab: PhoneTab) -> String {
        switch tab {
        case .chat: "chat"
        case .tasks: "tasks"
        case .files: "files"
        case .mac: "mac"
        }
    }

    static func tab(named name: String) -> PhoneTab { PhoneTab.allCases.first { Self.name(of: $0) == name } ?? .chat }
}

extension AppSheet {
    var isLocation: Bool { if case .location = self { return true } else { return false } }
}

/// Where the model's prompts live, so the unlocker made in init can reach the ones the shell sets later.
@MainActor
private final class PromptBox {
    var prompts: Prompts?
}

/// Records whether the prompt signed: the app opens on the owner's Face ID even if the Mac then says no.
private final class Recording: Signer, @unchecked Sendable {
    private let inner: Signer
    private let lock = NSLock()
    private var _signed = false
    init(_ inner: Signer) { self.inner = inner }
    var signed: Bool { lock.lock(); defer { lock.unlock() }; return _signed }
    func sign(payload: String) -> String? {
        let s = inner.sign(payload: payload)
        if s != nil { lock.lock(); _signed = true; lock.unlock() }
        return s
    }
}

/// No prompt could be shown: signs nothing (the library answers `not_signed`).
private final class Declined: Signer, @unchecked Sendable {
    func sign(payload: String) -> String? { nil }
}
