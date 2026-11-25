import FerbCore
import FerbDesign
import FerbMobile
import Foundation
import Observation

/// What fills the screen: one at a time, nothing private before `main` (PHN-835). `signIn`: after the lock, before
/// anything else, while no FERB account is signed in on this phone (kb PHN-827).
public enum Gate: Sendable { case starting, failed, noScreenLock, locked, signIn, pair, codes, removed, main }

/// Why the pairing screen shows again (or nil on a first run).
public enum PairReason: Sendable { case wiped, removed, keys, unknown }

/// Why the Mac's side needs the owner's fingerprint again while the app is open (kb PHN-649's card).
public enum MacNeed: Sendable { case lapsed, cancelled, old }

/// What pressing the microphone needs from the screen: nothing more, or iOS's microphone and speech questions.
public enum VoicePress: Sendable { case done, askMicrophone }

/// What slides up over the page. Only ids are kept (nothing saved holds question words, and never a private value).
public enum AppSheet: Equatable, Sendable {
    case relay, profile, forget, typeCode, scan, ownRelay, notifications, help, stopAll, settings
    /// The connection control's sheet (kb PHN-752): which Mac, how the link stands, its path, Try again.
    case connection
    case days(key: String)
    case live(taskId: String)
    /// A running task's short view (kb PHN-725; FERB.app's TaskShortView): its picture, title, line, Watch live, Stop.
    case task(taskId: String, worker: String?)
    /// The sites a task contacted (FERB.app's "Sites visited"), by the chat row that has them.
    case sites(row: String)
    /// The words before the owner's first use of speaking to FERB, for this phone's kind of recognition (PHN-604).
    case voice(SpeechKind)
    /// The Mac asks for this phone's exact location once, for a task (GATE-613); the owner says yes or no here.
    case location(requestId: String)
    /// The words before iOS's own question, when the owner turns on sharing the city (SENSE-420).
    case city
    /// What reaches the phone while FERB is closed (iOS has no Stay connected, kb PHN-566): plain words.
    case background
    /// Insomnia mode's warning before the phone turns it on (APP-980): the two ways, the one Yes, Keep it working.
    case insomnia
    /// Show me (APP-983): the Mac's camera while this shows, out of screenshots and captures.
    case camera
    /// FERB's account on this phone (kb PHN-827): who is signed in, Sign out, Delete account.
    case account
    /// Sign out's confirmation (kb PHN-829: the Mac is forgotten with it).
    case signOut
    /// Delete account's confirmation.
    case deleteAccount
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
    /// "Face ID to open" (kb PHN-835).
    public var lockOn: Bool

    public init(blockScreenshots: Bool, hideChecks: Bool, pairedSince: Int64, shareCity: Bool, lockOn: Bool = true) {
        self.blockScreenshots = blockScreenshots
        self.hideChecks = hideChecks
        self.pairedSince = pairedSince
        self.shareCity = shareCity
        self.lockOn = lockOn
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
    /// The pairing gate shows "FERB runs on your Mac" first (kb PHN-829): signed in, no Mac paired yet, until "I've
    /// installed it — pair now" (a re-pairing goes straight to the pairing page).
    public var macFirst = false
    /// The link's code of the last pairing error (the account's refusals offer another account).
    public var pairErrorCode: String? = nil
    /// Delete account's failure line, on its sheet.
    public var accountProblem: Note? = nil
    public var link: LinkStatus? = nil
    public var keys: KeyHealth? = nil
    public var conversation: String? = nil
    /// The work row above the message box (kb PHN-725): what runs, from the chat's live state; nil when nothing does.
    public var running: RunningUi? = nil
    /// The chat's "today" (days since 1970; ChatModel.chatDay, kb PHN-697), held: it moves only when the app opens or
    /// unlocks, comes back to the front, or the Chat tab is chosen, so a day changing never moves the page under him.
    /// 0: not set (screens then ask ChatModel.chatDay themselves).
    public var chatDay: Int = 0
    /// What the chat holds (the Tasks tab falls back to it when the Mac's own list can't be read).
    public var chatWork: Work = .empty
    public var work: Work = .empty
    /// What finished or failed since the app was last open (the home screen's list and the corpus's counts).
    public var away: [WorkItem] = []
    public var questions: [Question] = []
    public var older: OlderState = .unknown
    public var needsMacUnlock = false
    /// Why the Mac's side needs the fingerprint again: the card with its one button (PHN-649); nil: nothing to do.
    public var macNeed: MacNeed? = nil
    /// The Mac doesn't know this phone's keys any more (`unlock_invalid`): it needs pairing again (PHN-649).
    public var macForgotPhone = false
    /// The Mac's own words when it removed this phone over its account (`account_mismatch` while binding, PHN-828):
    /// the removed page says them.
    public var removedWords: String? = nil
    /// The Mac's side while the app is already open (PHN-641): its unlock under way, or slow to answer.
    public var macLine: MacLine? = nil
    /// The Mac has really not been reachable (kb PHN-753): stopped for good, or not online for `UnlockTiming.linkLost`
    /// while the app is open and unlocked. Until then a link that isn't online is connecting, never an error.
    public var linkLost = false
    public var busy: Set<String> = []
    public var note: Note? = nil
    /// What FERB has waiting, as the Mac last said it (PHN-125): its questions' needs, Stop everything.
    public var status: PhoneStatus? = nil
    /// The Mac's own name and this phone's pairing date (kept by the library).
    public var mac: MacInfo? = nil
    /// The Mac's task list (`tasks.list`), or nil when it couldn't be read (the chat's list is used then).
    public var macTasks: [TaskRow]? = nil
    /// FERB's newest alert above the message box (kb APP-1105, PHN-791; FERB.app's alert line): the Mac getting hot,
    /// insomnia's end, a database set aside… never in the chat; nil when none waits. `alertCount`: how many wait.
    public var alert: StatusAlert? = nil
    public var alertCount: Int = 0
    /// Every alert waiting, oldest first (the toast's stack, kb APP-1109); `alertsOpen`: the stack is open.
    public var alerts: [StatusAlert] = []
    public var alertsOpen = false
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

/// The calm line about the Mac's side once the app is open (PHN-641); never a control.
public enum MacLine: Sendable { case opening, slow }

/// How long the unlock waits on the Mac (PHN-649; Android's STATEMENT_AGE_MS, MAC_SLOW_MS, RECHECK_MS, RECHECKS).
public struct UnlockTiming: Sendable {
    /// The opening prompt's statement is handed to the Mac at most this long after it was signed (the Mac's own
    /// bound is 120 s; the phone stops a little sooner so a send in flight still fits).
    public var statementAge: Double = 115
    /// A statement refused because the connection hasn't proved the device key yet is sent again after this.
    public var statementRetry: Double = 1
    public var statementTries = 6
    /// No answer from the Mac's unlock in this long: the line says the Mac is slow (the app is open already).
    public var macSlow: Double = 6
    /// A `locked` refusal is read again `rechecks` times, this far apart, before anything is asked.
    public var recheck: Double = 1.5
    public var rechecks = 2
    /// How long the link may be not online, with the app open and unlocked, before the app says it can't reach the Mac
    /// (kb PHN-753): 10 s, the owner's (kb PHN-757); the connect itself takes about a second after the link fix.
    public var linkLost: Double = 10
    public init(statementAge: Double = 115, statementRetry: Double = 1, statementTries: Int = 6, macSlow: Double = 6,
                recheck: Double = 1.5, rechecks: Int = 2, linkLost: Double = 10) {
        self.statementAge = statementAge
        self.statementRetry = statementRetry
        self.statementTries = statementTries
        self.macSlow = macSlow
        self.recheck = recheck
        self.rechecks = rechecks
        self.linkLost = linkLost
    }
}

/// The app's state over [PhoneApi]: the gate, the chat, tasks and questions, files, Watch live and FERB's settings,
/// and every action (Android's FerbViewModel, feature for feature). The shell makes one per scene and tells it the
/// scene's phase through [Session.foreground]; a relaunch reads it from the phone's store and [saved].
@MainActor
@Observable
public final class AppModel {
    @ObservationIgnored private let timing: UnlockTiming
    /// The phone's steady clock in seconds (the unlock statement's age, PHN-649).
    @ObservationIgnored private let steady: () -> Double
    public static let tasksPage = 30
    /// How many recent files the Files tab asks for.
    public static let recentFilesCount = 20
    /// The exact fix's bound: the Mac waits 60 s for the answer; the owner's yes and the fix share that.
    public static let exactWait: Double = 20
    /// The lab's three places (PhoneFilesPage), as the Mac's paths.
    public static let places = ["~/Desktop", "~/Documents", "~/Downloads"]
    /// The Mac no longer knows this phone: pair again.
    public static let removedCodes: Set<String> = ["device_revoked", "not_paired", "unknown_phone"]

    /// Everything but the chat's rows. Set only when something in it changed: every view that reads it runs again on a
    /// set, so an apply that changes nothing here (a streamed word) runs none of them (kb PHN-714).
    public private(set) var ui: UiState {
        get {
            access(keyPath: \.ui)
            return uiValue
        }
        set {
            guard newValue != uiValue else { return }
            let dayMoved = newValue.chatDay != uiValue.chatDay
            withMutation(keyPath: \.ui) { uiValue = newValue }
            if dayMoved { rebuildPlaces() }
        }
    }
    @ObservationIgnored private var uiValue = UiState()
    /// The chat's rows as the library lays them out (ChatModel.rows), and each entry's calendar day (epoch days, the
    /// phone's zone: the day dividers, kb PHN-627): apart from [ui], so a streamed word changes only these.
    public private(set) var rows: [ChatRow] = []
    public private(set) var days: [String: Int] = [:]
    /// Today's chat and the earlier days as the chat draws them (ChatModel.withDays and split over the rows and the
    /// messages with files, kb PHN-696), made off the main actor for a long chat (kb PHN-714).
    public private(set) var places = ChatModel.Places(today: [], earlier: [])
    /// Something was said on the chat's day (ChatModel.said): the home page's rule, kept as one value.
    public private(set) var saidToday = false
    @ObservationIgnored private var placesGeneration = 0
    /// Above this many rows the chat's places are made off the main actor (Android FerbViewModel.OFF_MAIN_ENTRIES).
    public static let offMainEntries = 100
    #if DEBUG
    /// Long chats whose rows were made on the main thread (tests: must stay 0).
    nonisolated(unsafe) static var longChatsMadeOnMain = 0
    #endif
    /// The temporary chat while it is on (the owner, 2026-11-23): its rows as the chat draws them, read from the Mac and
    /// held in memory only (never in saved state, the store or the outbox); nil while it is off. The chat tab shows it
    /// in place of the chat until the dashed bubble ends it, here or on another device.
    public private(set) var temporary: TemporaryShown?
    /// The work row while the temporary chat shows (kb PHN-725): only its tasks (`ui.running` has only the chat's).
    public private(set) var temporaryRunning: RunningUi?
    /// Reads of the temporary chat: only the newest one's answer is shown, and none lands while a toggle or a send is
    /// on its way (that call's own answer is the chat after it; a read asked meanwhile runs after it).
    @ObservationIgnored private var temporaryReads = 0
    @ObservationIgnored private var temporaryCalling = false
    @ObservationIgnored private var temporaryReadAfter = false
    /// The chosen tab and whether the chat tab shows the home screen.
    public private(set) var tab: PhoneTab
    /// The chat tab shows the earlier days (kb PHN-696): "Earlier messages" opened them; Today and a send come back.
    public private(set) var earlier = false

    /// The chat tab shows today's home page (kb PHN-696): today, with nothing said on it yet (his send, FERB's post, a
    /// message with files on its way). Typing, speaking or picking a file never changes it; something said today does.
    public var home: Bool {
        !earlier && temporary == nil && outgoing.messages.isEmpty && !saidToday
    }
    /// Counts the Chat tab's taps while on Chat: each one takes the chat to its end (kb PHN-662).
    public private(set) var chatToEnd = 0
    public private(set) var draft: String
    public private(set) var settings: PhoneSettings
    public private(set) var sheet: AppSheet?
    /// Questions shown once already: kept in saved state only (questions are boxes in the chat since kb PHN-770).
    private var presented: Set<String>

    public let transfers: Transfers
    /// The message box's files and the messages that carry files (kb PHN-690, PHN-691); kept on the phone, off main.
    public let outgoing: Outgoing
    public let live: LiveWatch
    /// The owner's review of what an import brought in (kb MEM-460): the list on its question's card.
    public let review = ImportReview()
    /// Show me (APP-983): the Mac's camera, pulled only while its view shows.
    public let camera: CameraWatch
    /// The work row's round live pictures (kb PHN-725), only while it shows, unlocked, in front and not on the shared relay.
    public let thumbnails: Thumbnails
    @ObservationIgnored private var workRowIds: [String] = []

    @ObservationIgnored let api: PhoneApi
    /// FERB's account on this phone (kb PHN-827): signed out shows the sign-in page, whatever else holds.
    public let account: AccountModel
    /// "I've installed it — pair now" was chosen (the phone-first page gives way to the pairing page).
    @ObservationIgnored private var pairChosen = false
    /// Binding a legacy pairing was tried since the last unlock (at most once per unlock, kb PHN-828).
    @ObservationIgnored private var bindTried = false
    @ObservationIgnored let session: Session
    @ObservationIgnored private let prefs: Prefs
    @ObservationIgnored private let local: LocalFiles
    @ObservationIgnored private let screenLock: () -> Bool
    @ObservationIgnored private let corpus: Corpus?
    @ObservationIgnored private let now: () -> Date
    @ObservationIgnored private let locator: Locator?
    @ObservationIgnored private let coarse: () -> Coarse?
    @ObservationIgnored private let locationAnswered: () -> Void
    @ObservationIgnored private let voice: VoiceInput?
    @ObservationIgnored private let micAllowed: () -> Bool
    @ObservationIgnored private var dictation: Dictation?
    @ObservationIgnored private var spokenOnto = ""
    @ObservationIgnored private var pressedAt = Date.distantPast
    /// The owner holds the microphone and FERB is listening (PHN-603).
    public private(set) var listening = false
    /// What the microphone shows (kb PHN-643): getting ready, listening, finishing, or nothing.
    public private(set) var mic: MicState = .idle
    /// How loud the phone hears him while it listens, 0...1.
    public private(set) var micLevel: Float = 0
    @ObservationIgnored private var partials = 0
    @ObservationIgnored private var released = false
    @ObservationIgnored private var voiceStarted = Date.distantPast
    @ObservationIgnored private let box = PromptBox()
    @ObservationIgnored let unlocker: Unlocker
    @ObservationIgnored private var follows: [AnyObject] = []
    /// The count towards "can't reach the Mac" (kb PHN-753).
    @ObservationIgnored private var lostTask: Task<Void, Never>?
    @ObservationIgnored private var changesTask: Task<Void, Never>?
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var refreshing = false
    @ObservationIgnored private var dirty = false
    @ObservationIgnored private var tasksLoading = false
    /// Alerts put away on this phone this session: hidden at once, before the Mac's next status drops them (PHN-791).
    @ObservationIgnored private var dismissedAlerts = Set<String>()
    /// The toast's wait before it leaves by itself (kb APP-1109): `AlertToast.seconds`; a test gives its own.
    @ObservationIgnored public var alertDelay: @Sendable (Double) async -> Void = { s in try? await Task.sleep(nanoseconds: UInt64(s * 1_000_000_000)) }
    /// His finger is on the toast: it doesn't leave while held.
    public private(set) var alertHeld = false
    @ObservationIgnored private var alertLapse: Task<Void, Never>?
    @ObservationIgnored private var lastView: TranscriptView?

    /// When this model started, and whether the chat's first entries were logged (`ferb-app`, kb PHN-629).
    @ObservationIgnored private let born = Date()
    @ObservationIgnored private var storedShown = false

    /// Today in the phone's own zone (epoch days): "said today" (kb PHN-626).
    /// The chat's day rule, asked now (kb PHN-697): only ChatModel.chatDay says what "today" is.
    private func today() -> Int { ChatModel.chatDay(now(), Calendar.current) }

    /// The chat's held day catches up with the clock: only at a natural moment (kb PHN-697).
    private func turnTheDay() {
        let d = today()
        if ui.chatDay != d { ui.chatDay = d }
    }
    @ObservationIgnored private var pending: String?
    @ObservationIgnored private var pendingLocation: String?
    @ObservationIgnored private var pendingCamera = false
    /// Key places' files the owner opened in another app: they go when he is back in FERB (PHN-11).
    @ObservationIgnored private var openedKeyFiles: Set<String> = []
    /// The open before this one (the corpus's awayHours, lastOpen, firstToday, newPlace).
    @ObservationIgnored private var previousOpen: Double?
    @ObservationIgnored private var previousZone: String?

    /// The scene is active (no app switcher, system alert or Face ID sheet over it): the prompt by itself is shown only
    /// then (PHN-615). Set by the shell from the scene's phase.
    private var windowReady = false

    /// The shell: the scene became active (true) or inactive or background (false).
    public func setWindowReady(_ ready: Bool) {
        windowReady = ready
    }

    /// The prompts the screen shows (set by the shell once its window exists).
    public var prompts: Prompts? {
        get { box.prompts }
        set { box.prompts = newValue }
    }

    /// A screen that must stay out of screen captures whatever the owner's switch says: only a private answer being
    /// typed (PHN-110). Every other page, sheet and picture follows his Block screenshots switch, Watch live, a task's
    /// short view and the Mac's camera included (owner 2026-11-23, kb PHN-810, PHN-811). The shell
    /// hides the app's content while the screen is captured then.
    public var mustSecure: Bool { !hiddenBoxes.isEmpty }

    /// The question boxes with a hidden input's field on screen now: the screen stays out of captures (PHN-110).
    public private(set) var hiddenBoxes: Set<String> = []
    public func hiddenShown(_ approvalId: String, _ on: Bool) {
        if on { hiddenBoxes.insert(approvalId) } else { hiddenBoxes.remove(approvalId) }
    }

    /// A question's box to bring into view in today's chat (a notification's tap, the Tasks tab's row; kb PHN-770).
    public private(set) var toQuestion: (id: String, n: Int)? = nil

    /// Today's chat with this question's box in view: where it is answered (kb PHN-770).
    public func goToQuestion(_ approvalId: String) {
        tab = .chat
        earlier = false
        sheet = nil
        toQuestion = (approvalId, (toQuestion?.n ?? 0) + 1)
    }

    /// What the shell keeps in scene storage (and passes back at the next launch).
    public var saved: SavedState {
        var s = SavedState()
        s.tab = Self.name(of: tab)
        s.home = home
        // what is typed in a temporary chat is never kept (nothing of it is saved by the phone)
        s.draft = temporary == nil ? draft : ""
        s.presented = presented.sorted()
        return s
    }

    public init(api: PhoneApi, account: AccountModel, session: Session, prefs: Prefs, local: LocalFiles,
                screenLock: @escaping () -> Bool = { Biometry.screenLockSet }, corpus: Corpus? = .bundled,
                saved: SavedState = SavedState(), now: @escaping () -> Date = Date.init, locator: Locator? = nil,
                coarse: @escaping () -> Coarse? = { nil }, locationAnswered: @escaping () -> Void = {},
                voice: VoiceInput? = nil, micAllowed: @escaping () -> Bool = { false }, timing: UnlockTiming = UnlockTiming(),
                steady: @escaping () -> Double = { Session.monotonic() }) {
        self.timing = timing
        self.steady = steady
        self.api = api
        self.account = account
        self.session = session
        self.prefs = prefs
        self.local = local
        self.screenLock = screenLock
        self.corpus = corpus
        self.now = now
        self.locator = locator
        self.coarse = coarse
        self.locationAnswered = locationAnswered
        self.voice = voice
        self.micAllowed = micAllowed
        tab = Self.tab(named: saved.tab)
        draft = saved.draft
        presented = Set(saved.presented)
        settings = PhoneSettings(blockScreenshots: prefs.blockScreenshots, hideChecks: prefs.hideChecksOnLockScreen,
                                 pairedSince: prefs.pairedSince, shareCity: prefs.shareCity, lockOn: prefs.lockOnOpen)
        let box = self.box
        unlocker = Unlocker(api: api, keys: { api.state.keys }, signer: { kind, why in box.prompts?.signer(kind, why: why) })
        transfers = Transfers(api: api, local: local, unlocker: unlocker)
        outgoing = Outgoing(api: api, transfers: transfers, local: local)
        live = LiveWatch(api: api)
        camera = CameraWatch(api: api)
        thumbnails = Thumbnails(api: api)
        outgoing.onSent = { [weak self] item in
            guard let self else { return }
            if item.waitingFor == "unlock" { macLocked() }
            refresh()
        }
        outgoing.onSentTemporary = { [weak self] t in self?.showTemporary(t) }
        ui.chatDay = today()
        followAccount()
        start()
    }

    private struct GateInput: Equatable {
        let state: PhoneState
        let link: LinkStatus?
        let locked: Bool
        let signedIn: Bool
    }

    /// Remakes [places] and [saidToday] from the rows, the days, the messages with files and the chat's day: in place for
    /// a short chat, off the main actor for a long one (only the newest result is kept).
    private func rebuildPlaces() {
        placesGeneration += 1
        let gen = placesGeneration
        let day = ui.chatDay > 0 ? ui.chatDay : today()
        let rows = self.rows
        let days = self.days
        // (a temporary chat's message with files shows only in it)
        let all = rows + outgoing.messages.filter { !$0.temporary }.map { ChatRow.withFiles(key: "send:\($0.id)", item: $0) }
        let make = { (ChatModel.split(ChatModel.withDays(all, days, today: day), day: day), ChatModel.said(rows, days, from: day)) }
        if all.count <= Self.offMainEntries {
            publishPlaces(make())
            return
        }
        Task.detached(priority: .userInitiated) { [weak self] in
            let made = make()
            await MainActor.run { [weak self] in
                guard let self, gen == self.placesGeneration else { return }
                self.publishPlaces(made)
            }
        }
    }

    private func publishPlaces(_ made: (ChatModel.Places, Bool)) {
        if places != made.0 { places = made.0 }
        if saidToday != made.1 { saidToday = made.1 }
    }

    private func start() {
        follows.append(Follow({ [weak self] () -> [Sending]? in self?.outgoing.messages }) { [weak self] (_: [Sending]) in self?.rebuildPlaces() })
        follows.append(Follow({ [weak self] () -> GateInput? in guard let self else { return nil }; return GateInput(state: api.state, link: api.link, locked: session.locked, signedIn: account.account != nil) }) { [weak self] (g: GateInput) in
            guard let self else { return }
            ui.link = g.link
            ui.keys = g.state.keys
            gateOf(g.state, g.link, g.locked)
            showPending()
            if g.state.isOpen && g.link?.paired == true && prefs.pairedSince == 0 {
                // a pairing made signed in carried the account's assertion: it is this account's from the start
                prefs.pairedAccount = account.account?.id
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
                case .status: statusChanged()
                case .mac: readMac()
                // the temporary chat's watch moved it, as transcript/live move the chat: read from memory (ORCH-715)
                case .temporary: showLastTemporary()
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
            // not while the opening prompt's statement is on its way: its unlock reads the chat itself
            if online && !session.locked && statement == nil { Task { await self.syncNow() } }
            if online { bindIfNeeded() }
        })
        // can't reach the Mac (kb PHN-753): only once it has stopped for good, or hasn't been online for
        // timing.linkLost of the app being open and unlocked; every open starts the count again (connecting is normal:
        // on his phone it takes about 25 s tonight, two dials timing out before the third answers)
        follows.append(Follow({ [weak self] () -> Int? in
            guard let self else { return nil }
            let phase = api.link?.phase
            if phase == .stopped { return 2 }
            if phase == .online || phase == nil || session.locked || !session.front { return 0 }
            return 1
        }) { [weak self] (away: Int) in
            guard let self else { return }
            lostTask?.cancel()
            ui.linkLost = away == 2
            if away == 1 {
                let after = timing.linkLost
                lostTask = Task { [weak self] in
                    try? await Task.sleep(nanoseconds: UInt64(after * 1_000_000_000))
                    if !Task.isCancelled { self?.ui.linkLost = true }
                }
            }
        })
        // files cross the path again (PHN-444): what waited for it is tried once more, by itself
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return api.link.map { $0.bulkAllowed && $0.phase == .online } ?? false }) { [weak self] (bulk: Bool) in
            guard let self else { return }
            if bulk { transfers.bulkAllowed() }
        })
        // the box's files and the messages on their way, as FERB left them (read off the main thread); a message with
        // files that waited for the Mac, or for a path that carries files, starts when it can
        let outgoing = self.outgoing
        Task { [weak self] in
            await outgoing.load().value
            guard let self else { return }
            follows.append(Follow({ [weak self] () -> [Bool]? in
                guard let self else { return nil }
                return [api.link?.phase == .online, api.link?.bulkAllowed == true]
            }) { [weak self] (_: [Bool]) in self?.outgoing.linkChanged() })
        }
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return session.locked }) { [weak self] (locked: Bool) in
            guard let self else { return }
            if !locked {
                // the account's tokens refreshed when they are about to end (never a lockout when offline); a pairing
                // made before sign-in existed is bound to the account (once per unlock until it is)
                bindTried = false
                Task {
                    await self.account.check()
                    self.bindIfNeeded()
                }
                // opened or unlocked: today's chat, on the day it is now (kb PHN-696, PHN-697)
                turnTheDay()
                earlier = false
                refresh()
                // a temporary chat started or ended while the app was locked or closed shows as it is now
                readTemporary()
                outgoing.unlocked()
                // a request for the exact location that came while the app was closed or locked
                Task {
                    if case .ok(let id?) = await self.api.locationWanted() {
                        self.pendingLocation = id
                        self.showPending()
                    }
                }
            } else {
                // PHN-15: nothing private stays open; the preview stops, the fetched files go
                AppLog.i("lock shown")
                macTask?.cancel()
                statementTask?.cancel()
                statementTask = nil
                statement = nil
                macPending = false
                ui.macLine = nil
                ui.needsMacUnlock = false
                ui.macNeed = nil
                stopListening()
                live.stop()
                camera.stop()
                thumbnails.show([])
                switch sheet {
                case .live, .task, .settings, .days, .location, .camera, .insomnia: sheet = nil
                default: break
                }
                review.locked()
                transfers.locked()
                outgoing.locked()
                ui.files = FilesUi()
                ui.settings = nil
                ui.macTasks = nil
                // kb PHN-845: with the owner's lock switch off it opens again now, after all of the above
                lockHandled = true
                openUnasked()
            }
        })
        // PHN-615: the lock page that came with opening or returning asks by itself, once, while the scene is active
        follows.append(Follow({ [weak self] () -> Bool? in
            guard let self else { return nil }
            return windowReady && session.promptDue && ui.gate == .locked
        }) { [weak self] (due: Bool) in
            if due { self?.autoUnlock() }
        })
        // kb APP-1109: the toast leaves by itself after its seconds while it may (each change starts its wait again)
        follows.append(Follow({ [weak self] () -> String?? in guard let self else { return nil }; return .some(lapsing) }) { [weak self] (id: String?) in
            self?.lapse(id)
        })
        // PHN-648: while the app is unlocked and in front the device key's presence keeps the Mac's window open
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return !session.locked && session.front }) { [weak self] (open: Bool) in
            self?.api.setOpen(open)
        })
        follows.append(Follow({ [weak self] () -> Bool? in guard let self else { return nil }; return session.front }) { [weak self] (front: Bool) in
            guard let self else { return }
            if !front {
                // leaving the app: what finished so far has been seen; the preview stops (PHN-454)
                markSeen()
                stopListening()
                thumbnails.show([])
                if live.showing { closeLive() }
                // the Mac's camera only while he looks at it (APP-983)
                if camera.showing { closeCamera() }
            } else {
                transfers.dropKeyPlaceFiles(openedKeyFiles)
                openedKeyFiles = []
                let local = self.local, at = now()
                Task.detached { local.dropOld(now: at) }
                // back in front: a natural moment for the chat's day to move (kb PHN-697)
                turnTheDay()
                markRead()
                showPending()
                readTemporary()
                // kb PHN-845: locked while away, with the owner's lock switch off: open without the prompt
                openUnasked()
            }
        })
        readMac()
        refresh()
        readTemporary()
    }

    // ---- the gate -------------------------------------------------------------------------------------------------

    private func gateOf(_ s: PhoneState, _ l: LinkStatus?, _ locked: Bool) {
        let signedIn = account.account != nil
        var wiped: Lost?
        if case .open(_, let w) = s { wiped = w }
        let keys = s.keys
        let removed = l?.phase == .stopped && Self.removedCodes.contains(l?.problem?.code ?? "")
        let gate: Gate
        switch s {
        // the lock page needs neither the library, the store nor the link: it shows at the first frame (PHN-633)
        // signed out (kb PHN-827): the sign-in, right after the lock and before anything else of FERB's
        // with the owner's lock switch off there is no lock page: the start's, the moment before it opens (PHN-845)
        case .closed, .starting: gate = !screenLock() ? .noScreenLock : locked ? (settings.lockOn ? .locked : .starting) : !signedIn ? .signIn : .starting
        case .failed: gate = !signedIn && !locked && screenLock() ? .signIn : .failed
        case .open:
            if !screenLock() { gate = .noScreenLock }
            else if locked { gate = settings.lockOn ? .locked : .starting }
            else if !signedIn { gate = .signIn }
            else if let l {
                if l.phase == .pairingWaitForMac { gate = .codes }
                else if removed { gate = .removed }
                else if ui.macForgotPhone && l.paired { gate = .removed }
                else if !l.paired { gate = .pair }
                else if keys?.pairAgain == true { gate = .removed }
                else { gate = .main }
            } else { gate = .starting }
        }
        let reason: PairReason?
        if removed { reason = .removed }
        else if ui.macForgotPhone && l?.paired == true { reason = .unknown }
        else if keys?.pairAgain == true && l?.paired == true { reason = .keys }
        else if wiped != nil { reason = .wiped }
        else { reason = nil }
        if ui.pairing, let l {
            if l.paired && l.phase == .online { ui.pairing = false }
            else if l.phase == .stopped || l.phase == .waiting || l.phase == .idle, let p = l.problem {
                ui.pairing = false
                ui.pairError = Self.pairNote(p.code, p.detail)
                ui.pairErrorCode = p.code
            }
        }
        ui.gate = gate
        ui.pairReason = reason
        ui.macFirst = gate == .pair && reason == nil && !pairChosen && !ui.pairing
        if case .failed(let code, let detail) = s { ui.failure = "\(code): \(detail)" } else { ui.failure = nil }
    }

    /// Reads the gate again (a passcode may have been set in Settings meanwhile).
    public func recheck() {
        gateOf(api.state, api.link, session.locked)
        openUnasked()
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

    /// The notice's Show me (APP-983): the camera view opens as soon as the app is unlocked.
    public func openCamera() {
        pendingCamera = true
        showPending()
    }

    private func showPending() {
        if ui.gate != .main || session.locked { return }
        if let id = pending {
            pending = nil
            goToQuestion(id)
            return
        }
        if pendingCamera && session.front {
            pendingCamera = false
            tab = .mac
            showCamera()
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
                await apply(view)
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

    @ObservationIgnored private var applyChain: Task<Void, Never>?
    @ObservationIgnored private var statusQueued = false

    /// The Mac's status changed: the screens are drawn again from the transcript as last read. At most one waits.
    private func statusChanged() {
        if statusQueued { return }
        statusQueued = true
        Task { await apply(nil, statusOnly: true) }
    }

    /// The transcript and the Mac's last status as the screens draw them; `statusOnly`: the transcript as last read.
    /// The status is read from the library off the main actor (kb PHN-682), so the applies run one after another, in
    /// the order asked, and the last one has the newest of both.
    private func apply(_ read: TranscriptView?, statusOnly: Bool = false) async {
        let before = applyChain
        let t = Task { [weak self] in
            await before?.value
            guard let self else { return }
            if statusOnly { self.statusQueued = false } else { self.lastView = read }
            let status = await self.api.lastStatus()
            let view = self.lastView
            let mac = self.ui.macTasks
            // made from the whole transcript: off the main actor for a long chat (kb PHN-714), in place for a short one
            let made: Made
            if (view?.items.count ?? 0) > Self.offMainEntries {
                made = await Task.detached(priority: .userInitiated) { Made(view, status, mac) }.value
            } else {
                made = Made(view, status, mac)
            }
            self.applyNow(view, status, made)
        }
        applyChain = t
        await t.value
    }

    /// The transcript and the Mac's last status (each question's needs, Stop everything, PHN-125) as the screens draw them.
    /// What an apply makes from the whole transcript (the rows, their days, the work row, the chat's work).
    private struct Made: @unchecked Sendable {
        let rows: [ChatRow]
        let days: [String: Int]
        let running: RunningUi?
        let chatWork: Work

        init(_ view: TranscriptView?, _ status: PhoneStatus?, _ mac: [TaskRow]?) {
            #if DEBUG
            if Thread.isMainThread && (view?.items.count ?? 0) > AppModel.offMainEntries { AppModel.longChatsMadeOnMain += 1 }
            #endif
            rows = ChatModel.rows(view)
            days = ChatModel.days(view, Calendar.current)
            // the chat's work row never counts a temporary chat's tasks
            running = RunningModel.of(view, status: statusPart(status, temporary: false), mac: mac)
            chatWork = WorkModel.of(view, asking: "")
        }
    }

    private func applyNow(_ view: TranscriptView?, _ status: PhoneStatus?, _ made: Made) {
        let questions = Questions.merged(view.map { Questions.open($0.questions) } ?? [], status)
        let chatWork = made.chatWork
        if prefs.lastOpen == 0 && prefs.seen.isEmpty { prefs.seen = Set(chatWork.earlier.compactMap(\.entry)) }
        let seen = prefs.seen
        ui.conversation = view?.conversation
        // FERB's words come laid out by the library, made off the main actor when the transcript was read and kept per
        // entry (ChatModel.prepare, LibraryPhone.transcript): here only found again (PHN-682)
        rows = made.rows
        ui.running = made.running
        if days != made.days { days = made.days }
        rebuildPlaces()
        if !storedShown && !rows.isEmpty {
            storedShown = true
            AppLog.i("chat entries ready \(Int(Date().timeIntervalSince(born) * 1000)) ms after the app started: \(rows.count) rows")
        }
        ui.chatWork = chatWork
        ui.away = chatWork.earlier.filter { w in w.entry.map { !seen.contains($0) } ?? true }
        ui.questions = questions
        ui.older = view?.older ?? .unknown
        ui.status = status
        showAlerts(status)
        rebuildWork()
        runTemporary()
    }

    // MARK: FERB's alerts (kb APP-1105, APP-1106, PHN-791)

    /// The newest alert the Mac's status lists that wasn't put away here (FERB.app shows the newest the same way).
    private func showAlerts(_ status: PhoneStatus?) {
        let waiting = (status?.alerts ?? []).filter { !dismissedAlerts.contains($0.id) }
        if ui.alert != waiting.last { ui.alert = waiting.last }
        if ui.alertCount != waiting.count { ui.alertCount = waiting.count }
        if ui.alerts != waiting { ui.alerts = waiting }
        if ui.alertsOpen && waiting.count < 2 { ui.alertsOpen = false }
    }

    /// "N alerts" tapped: the stack opens (every waiting alert, each with its ×) or closes (kb APP-1109).
    public func toggleAlerts() { ui.alertsOpen = !ui.alertsOpen && ui.alertCount > 1 }

    /// His finger went down on the toast or came up (kb APP-1109).
    public func holdAlert(_ held: Bool) { if alertHeld != held { alertHeld = held } }

    /// The toast that may leave by itself now: shown on the chat, in front, unlocked, the stack closed, not held.
    private var lapsing: String? {
        guard let id = ui.alert?.id, !ui.alertsOpen, tab == .chat, session.front, !session.locked, !alertHeld else { return nil }
        return id
    }

    /// FERB.app's AlertToastView: the toast shown its six seconds in front of him is seen, so it is put away everywhere.
    private func lapse(_ id: String?) {
        alertLapse?.cancel()
        guard let id else { return }
        let wait = alertDelay
        alertLapse = Task { [weak self] in
            await wait(AlertToast.seconds)
            guard !Task.isCancelled, let self, self.lapsing == id else { return }
            self.dismissAlert(id)
        }
    }

    /// The alert's ×: gone here at once, and on every device once the Mac has it (`attention.ack`).
    public func dismissAlert(_ id: String) {
        dismissedAlerts.insert(id)
        showAlerts(ui.status)
        Task { _ = await api.dismissAlert(id: id) }
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
            case .ok(let page):
                ui.macTasks = page.tasks
                ui.running = RunningModel.of(lastView, status: statusPart(await api.lastStatus(), temporary: false), mac: page.tasks)
            case .failed(let e): if case .NeedsUnlock = e { macLocked() }
            }
            rebuildWork()
        }
    }

    private func readMac() {
        Task { if case .ok(let m?) = await api.cachedMacInfo() { macSays(m) } }
    }

    /// After the Mac's unlock: its name and this phone's pairing date, and whether a key waits to be registered.
    private func afterMacUnlock() async {
        if case .ok(let m) = await api.macInfo() { macSays(m) }
        let fresh = await api.unregisteredKeys()
        ui.newKeys = fresh?.strong != nil
    }

    private func syncNow() async {
        let asked = Date()
        AppLog.i("chat sync asked at \(Int(asked.timeIntervalSince(born) * 1000)) ms")
        switch await api.sync() {
        case .failed(let e):
            AppLog.i("chat sync failed after \(Int(Date().timeIntervalSince(asked) * 1000)) ms")
            if case .NeedsUnlock = e { macLocked() }
        case .ok:
            AppLog.i("chat sync answered after \(Int(Date().timeIntervalSince(asked) * 1000)) ms")
            macPending = false
            ui.needsMacUnlock = false
            readTemporary()
            // opened without the prompt (kb PHN-846): the Mac's word on this phone, which no unlock read
            if macInfoDue { macInfoDue = false; await afterMacUnlock() }
        }
    }

    /// The chat on screen, the app open and unlocked: the Mac learns how far the owner has read (one mark, latest).
    private func markRead() {
        guard let conv = ui.conversation else { return }
        if ui.gate != .main || session.locked || !session.front { return }
        if tab != .chat || home || temporary != nil { return }
        // one mark on its way at a time, and at most one more behind it: every read of the chat asks for one, and on
        // a slow link each started its own call to the Mac before the first had answered (kb PHN-682)
        if marking { markAgain = true; return }
        marking = true
        Task { [weak self] in
            repeat {
                self?.markAgain = false
                _ = await self?.api.markReadLatest(conversation: self?.ui.conversation ?? conv)
            } while self?.markAgain == true
            self?.marking = false
        }
    }
    @ObservationIgnored private var marking = false
    @ObservationIgnored private var markAgain = false

    // ---- the app lock (PHN-835, PHN-845, PHN-13, PHN-125) ----------------------------------------------------------

    /// One prompt opens the app, at once, whatever the link is doing (PHN-649; Android's unlockApp). On a paired phone
    /// the lock key signs, inside that prompt, an unlock statement the phone makes itself; the app opens the moment the
    /// prompt passes and the statement goes to the Mac whenever the link is up (`handStatement`). Not paired: the
    /// prompt alone.
    @discardableResult
    public func unlockApp(auto: Bool = false) -> Task<Void, Never> {
        busy("unlock") { [self] in
            guard let p = prompts else { return }
            _ = session.takePrompt() // the owner's tap takes the due prompt too: after its cancel nothing asks by itself
            if auto && !windowReady { session.promptAgain(); AppLog.i("unlock: screen elsewhere, prompt later"); return }
            AppLog.i("unlock started auto=\(auto)")
            let shown = Date()
            if await api.statementReady(), let inner = p.signer(.lock, why: .open) {
                let payload = UnlockStatement.make(wallMs: Int64(now().timeIntervalSince1970 * 1000))
                AppLog.i("prompt shown (signs the unlock statement)")
                let sig = await Task.detached { inner.sign(payload: payload) }.value
                AppLog.i("prompt result \(sig != nil ? "passed" : "cancelled") after \(ms(since: shown)) ms")
                guard let sig else { return }
                openApp()
                AppLog.i("local unlock done; the statement waits for the link")
                statement = UnlockStatement(payload: payload, sig: sig, signedAt: steady())
                handStatement()
                return
            }
            AppLog.i("prompt shown (alone)")
            let passed = await p.local()
            AppLog.i("prompt result \(passed ? "passed" : "cancelled") after \(ms(since: shown)) ms")
            if passed {
                openApp()
                _ = await api.unlocked()
                AppLog.i("local unlock done")
            }
        }
    }

    @ObservationIgnored private var macTask: Task<Void, Never>?
    /// The Mac couldn't be unlocked yet (offline, or its answer was lost): it is, once the link is up again.
    @ObservationIgnored private var macPending = false
    /// The statement signed in the opening prompt, until the Mac took it (PHN-649).
    @ObservationIgnored private var statement: UnlockStatement?
    @ObservationIgnored private var statementTask: Task<Void, Never>?
    /// The statement grew too old before the link was up: the Mac's side needs the fingerprint once more.
    @ObservationIgnored private var statementExpired = false

    private func ms(since d: Date) -> Int { Int(Date().timeIntervalSince(d) * 1000) }

    /// The app is open: the lock's state, the open's facts (PHN-13). `unasked`: without the prompt (PHN-846).
    private func openApp(unasked: Bool = false) {
        session.unlocked()
        opened()
        statementExpired = false
        macInfoDue = unasked
        ui.macNeed = nil
        ui.needsMacUnlock = false
    }

    /// Opened without the prompt and the Mac's word on this phone's lock not read since (kb PHN-846).
    @ObservationIgnored private var macInfoDue = false
    /// The lock's own clearing (the `session.locked` follow) ran since the last open: the app may open unasked.
    @ObservationIgnored private var lockHandled = false

    /// The owner's lock switch is off (kb PHN-835, PHN-846): the app, locked and in front, opens without the prompt.
    /// Nothing is signed: the Mac gives this phone chat and tasks by its own record of the switch (`phone.lock_off`),
    /// and everything else asks as before (files after the idle minutes, a hard Yes, a private answer, a setting). A
    /// phone with no passcode still can't use the app. (Android's openUnasked.)
    private func openUnasked() {
        guard lockHandled, session.locked, session.front, !settings.lockOn, screenLock(), ui.gate != .failed else { return }
        AppLog.i("opened without the lock: the owner's switch is off")
        lockHandled = false
        openApp(unasked: true)
        Task {
            _ = await api.unlocked()
            // no unlock statement reads the chat this time: the link already up reads it now (else its coming online)
            if api.link?.paired == true && api.link?.phase == .online { await syncNow() }
        }
    }

    /// This phone's "Face ID to open" (kb PHN-835, PHN-846; Android's setAppLock). Off is the Mac's to keep: one prompt
    /// signs `phone.lock_off` (the Face ID key where the phone has one, PHN-129), and only once the Mac took it does the
    /// phone open without asking, so whoever holds the unlocked phone can't quietly take the lock away. On again takes
    /// nothing: the phone asks again at once, and the Mac is told (only ever stricter; again from the Mac's next word if
    /// it can't be told now).
    @discardableResult
    public func setAppLock(_ on: Bool) -> Task<Void, Never> {
        busy("app-lock", tap: true) { [self] in
            if on {
                prefs.lockOnOpen = true
                refreshProfile()
                _ = await api.setLockOff(false, signer: nil)
                return
            }
            guard settings.lockOn, let signer = await freshSigner(.lockOff) else { return }
            switch await api.setLockOff(true, signer: signer) {
            case .ok:
                prefs.lockOnOpen = false
                refreshProfile()
                note(Note("profile_lock_off_note"))
            case .failed(let e):
                if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    /// The Mac's word on this phone (`phone.list`) and its record of the lock switch (kb PHN-846): the Mac has it off
    /// while the phone has it on: the Mac is told on (only ever stricter); the Mac has it on while the phone has it off
    /// (paired again, or removed): the phone asks again too. Not while the switch itself is being changed.
    private func macSays(_ m: MacInfo) {
        ui.mac = m
        if ui.busy.contains("app-lock") { return }
        switch m.lockOff {
        case true?: if prefs.lockOnOpen { Task { _ = await api.setLockOff(false, signer: nil) } }
        case false?:
            if !prefs.lockOnOpen {
                AppLog.i("the Mac has this phone's lock on: on here too")
                prefs.lockOnOpen = true
                refreshProfile()
            }
        case nil: break
        }
    }

    /// Hands the opening prompt's statement to the Mac once the link is up and paired, with its age by the phone's
    /// steady clock: no prompt, no line (Android's handStatement).
    private func handStatement() {
        guard let st = statement, statementTask == nil else { return }
        statementTask = Task { [self] in
            defer { statementTask = nil }
            let up = { (l: LinkStatus?) -> Bool in l.map { $0.paired && $0.phase == .online } ?? false }
            while !up(api.link) {
                if steady() - st.signedAt > timing.statementAge || statement !== st {
                    if statement === st {
                        AppLog.i("statement dropped: the link wasn't up in time")
                        statement = nil
                        statementExpired = true
                    }
                    return
                }
                try? await Task.sleep(nanoseconds: 50_000_000)
            }
            if statement !== st { return }
            if api.link?.macOlder == true {
                AppLog.i("statement: the Mac is older; its own unlock")
                statement = nil
                await unlockMacNow()
                return
            }
            for attempt in 1...timing.statementTries {
                let age = steady() - st.signedAt
                if age > timing.statementAge { break }
                let sent = Date()
                switch await api.unlockWithStatement(payload: st.payload, sig: st.sig, ageMs: UInt64(max(0, age) * 1000)) {
                case .ok:
                    AppLog.i("mac unlock by statement ok in \(ms(since: sent)) ms (age \(Int(age * 1000)) ms)")
                    statement = nil
                    macPending = false
                    ui.needsMacUnlock = false
                    ui.macNeed = nil
                    ui.macLine = nil
                    refresh()
                    await afterMacUnlock()
                    return
                case .failed(let e):
                    let code = e.linkCode ?? AppLog.kind(e)
                    AppLog.i("mac unlock by statement failed \(code) (try \(attempt), age \(Int(age * 1000)) ms)")
                    if code == "locked" {
                        // the connection hasn't proved the device key yet: a moment, then again
                        try? await Task.sleep(nanoseconds: UInt64(timing.statementRetry * 1e9))
                        continue
                    }
                    if code == "unlock_invalid" { statement = nil; macForgot(); return }
                    if code == "unlock_required" { break }
                    if api.link?.phase == .online, case .Offline = e {} else if api.link?.phase == .online { break }
                    // the link dropped under it: once it is back, again
                    try? await Task.sleep(nanoseconds: UInt64(timing.statementRetry * 1e9))
                    while !up(api.link) { try? await Task.sleep(nanoseconds: 50_000_000) }
                    continue
                }
                break
            }
            statement = nil
            statementExpired = true
            needMac(.old)
        }
    }

    /// The Mac doesn't know this phone's keys (`unlock_invalid`): the page that says to pair again (PHN-649).
    private func macForgot() {
        AppLog.i("mac doesn't know this phone's keys: pair again")
        ui.macForgotPhone = true
        ui.macNeed = nil
        ui.needsMacUnlock = false
        recheck()
    }

    /// The card: the Mac's side needs the fingerprint once, and why (PHN-649). Never while a statement is on its way.
    private func needMac(_ why: MacNeed) {
        if statement != nil || session.locked { return }
        AppLog.i("mac needs the fingerprint: \(why)")
        ui.macNeed = why
        ui.needsMacUnlock = true
        ui.macLine = nil
    }

    /// The Mac said its window is closed (a private call came back `locked`). First the same read again
    /// (`timing.rechecks` times): right after a reconnect the phone proves itself on the new connection and the window
    /// is open again; a statement on its way settles it too. Still locked: the card asks once (PHN-649). Nothing asks
    /// by itself.
    func macLocked() {
        if macRunning || ui.busy.contains("unlock-mac") || ui.busy.contains("unlock") { return }
        if session.locked || api.link?.paired != true { return }
        if statement != nil { handStatement(); return }
        macRunning = true
        macTask = Task { [self] in
            defer { macRunning = false }
            if api.link?.phase != .online { macPending = true; return }
            var open = false
            for _ in 0..<timing.rechecks {
                try? await Task.sleep(nanoseconds: UInt64(timing.recheck * 1e9))
                if session.locked { return }
                if api.link?.phase != .online { macPending = true; return }
                if case .ok = await api.sync() { open = true; break }
            }
            if open {
                AppLog.i("mac locked: open again after the recheck")
                macPending = false
                ui.needsMacUnlock = false
                ui.macNeed = nil
                refresh()
                return
            }
            needMac(statementExpired ? .old : .lapsed)
        }
    }

    @ObservationIgnored private var macRunning = false

    /// The due prompt (PHN-615), taken once; none while another unlock is already asking.
    private func autoUnlock() {
        guard prompts != nil else { return }
        let took = session.takePrompt()
        if !took || ui.busy.contains("unlock") { return }
        unlockApp(auto: true)
    }

    /// The card's Unlock: the Mac's own challenge unlock, one prompt (PHN-649).
    @discardableResult
    public func unlockMac() -> Task<Void, Never> {
        busy("unlock-mac", tap: true) { [self] in await unlockMacNow() }
    }

    private func unlockMacNow() async {
        guard let p = prompts, let inner = p.signer(.lock, why: .open) else { return }
        let asked = Date()
        ui.macLine = .opening
        ui.needsMacUnlock = false
        ui.macNeed = nil
        let call = Task { await api.unlock(inner) }
        let slow = Task { [self] in
            try? await Task.sleep(nanoseconds: UInt64(timing.macSlow * 1e9))
            if !Task.isCancelled && ui.macLine == .opening { ui.macLine = .slow }
        }
        let r = await call.value
        slow.cancel()
        ui.macLine = nil
        switch r {
        case .ok:
            AppLog.i("mac unlock answered ok in \(ms(since: asked)) ms")
            statementExpired = false
            macPending = false
            refresh()
            await afterMacUnlock()
        case .failed(let e):
            let code = e.linkCode ?? AppLog.kind(e)
            AppLog.i("mac unlock failed \(code) in \(ms(since: asked)) ms")
            switch e {
            case .NotSigned: needMac(.cancelled)
            case .NeedsSigner: failed(e)
            default:
                if code == "unlock_invalid" { macForgot() } else { macPending = true; needMac(.lapsed) }
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
        busy("keys", tap: true) { [self] in
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

    /// Pairs from a code (PHN-102), carrying the server's signed word of this phone's account for its own link id: the
    /// Mac's daemon pairs only a phone of its own account, before it asks the owner (PHN-828).
    @discardableResult
    public func pair(_ code: String) -> Task<Void, Never> {
        busy("pair") { [self] in
            let text = code.trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty { return }
            ui.pairing = true
            ui.pairError = nil
            ui.pairErrorCode = nil
            sheet = nil
            guard let endpoint = await api.endpointId(), !endpoint.isEmpty else {
                ui.pairing = false
                ui.pairError = Self.errorNote(.Closed(detail: "FERB is not open.", linkCode: nil))
                return
            }
            let assertion: String
            switch await account.assertion(endpoint: endpoint) {
            case .ok(let a): assertion = a
            case .offline:
                ui.pairing = false
                ui.pairError = Note("pair_needs_internet")
                return
            case .down:
                ui.pairing = false
                ui.pairError = Note("signin_not_answering")
                return
            case .signedOut:
                // the gate shows the sign-in now
                ui.pairing = false
                return
            }
            if case .failed(let e) = await api.pair(text, account: assertion) {
                ui.pairing = false
                ui.pairError = Self.pairNote(e.linkCode ?? e.code, e.detail)
                ui.pairErrorCode = e.linkCode ?? e.code
            }
        }
    }

    /// "I've installed it — pair now" on the phone-first page: the pairing page (kb PHN-829).
    public func choosePair() {
        pairChosen = true
        recheck()
    }

    // ---- the account (kb PHN-827..829) -------------------------------------------------------------------------------

    /// Wires the account's sign-ins to the app: a different account than the last one here forgets the Mac paired under
    /// it (a phone pairs only within one account, PHN-828); a sign-in says who is signed in.
    private func followAccount() {
        account.onSignedIn = { [weak self] info, changed in
            guard let self else { return }
            pairChosen = false
            if changed {
                AppLog.i("a different account signed in: the Mac paired under the other one is forgotten")
                Task { await self.forgetHere(quiet: true) }
            }
            note(Note("signin_signed_in_as", info.email))
            recheck()
            bindTried = false
            bindIfNeeded()
        }
        // signed out by the site (kb REL-453): only asked to sign in again, still paired with its Mac (the same account
        // carries on); only a deleted account also leaves the Mac, as Sign out does
        account.onSignedOutBySite = { [weak self] reason in
            guard let self else { return }
            guard reason == "account_deleted" else {
                AppLog.i("the site ended the sign-in: sign in again; still paired")
                self.sheet = nil
                self.recheck()
                return
            }
            AppLog.i("the account was deleted: this phone leaves its Mac as on Sign out")
            busy("account") {
                self.sheet = nil
                await self.forgetHere(quiet: true)
                self.pairChosen = false
                self.recheck()
            }
        }
    }

    /// A pairing made before sign-in existed has no account on the Mac's side: the phone binds it (kb PHN-828) with the
    /// site's assertion for its link id, while signed in, paired, online and unlocked, at most once per unlock until the
    /// Mac says bound. `account_mismatch`: the Mac removed this phone (its words on the removed page); the Mac signed
    /// out, an unconfirmed account, no network: nothing changes, tried again at the next unlock (never unpaired).
    private func bindIfNeeded() {
        // pairedSince 0: a pairing just made (signed in, with the assertion): the gate's follow records its account
        guard !bindTried, !session.locked, let acct = account.account, prefs.pairedAccount == nil, prefs.pairedSince > 0,
              let l = api.link, l.paired, l.phase == .online else { return }
        bindTried = true
        Task { [self] in
            guard let endpoint = await api.endpointId(), !endpoint.isEmpty,
                  case .ok(let assertion) = await account.assertion(endpoint: endpoint) else { return }
            switch await api.bindAccount(assertion: assertion) {
            case .ok(true):
                if account.account?.id == acct.id {
                    prefs.pairedAccount = acct.id
                    AppLog.i("the pairing is bound to the account")
                }
            case .ok(false):
                break
            case .failed(let e):
                AppLog.i("binding the pairing: \(e.linkCode ?? e.code)")
                if e.linkCode == "account_mismatch" {
                    ui.removedWords = e.detail.isEmpty ? nil : e.detail
                    ui.macForgotPhone = true
                    recheck()
                }
            }
        }
    }

    /// Sign out: after its question in Settings, or at once from "Use another account" where the phone isn't paired
    /// (the phone-first page, the Mac's refusal of this account, its removal over it: kb PHN-842). This phone leaves its
    /// Mac first (best effort `phone.forget_self` through the link, then forgotten here), then the account: the site
    /// told first, everything here cleared whatever it said (PHN-843). The pending state is `busy` "account"; the
    /// sign-in shows, with a line when the site couldn't be told.
    @discardableResult
    public func signOut() -> Task<Void, Never> {
        busy("account", tap: true) { [self] in
            sheet = nil
            await forgetHere(quiet: true)
            await account.signOut()
            pairChosen = false
            // the refusal or removal that offered another account is over with this account
            ui.pairError = nil
            ui.pairErrorCode = nil
            ui.removedWords = nil
            ui.macForgotPhone = false
            recheck()
        }
    }

    /// Signing out is under way (Sign out, Use another account): the pages show its pending state (kb PHN-842).
    public var signingOut: Bool { ui.busy.contains("account") }

    /// Delete account (confirmed): the site must delete it; only then the same clearing as Sign out.
    @discardableResult
    public func deleteAccount() -> Task<Void, Never> {
        busy("account", tap: true) { [self] in
            ui.accountProblem = nil
            guard await account.delete() else {
                if account.account != nil { ui.accountProblem = Note("account_delete_failed") }
                return
            }
            sheet = nil
            await forgetHere(quiet: true)
            pairChosen = false
            recheck()
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

    public func clearPairError() {
        ui.pairError = nil
        ui.pairErrorCode = nil
    }

    /// The pairing's error is the Mac's refusal of this phone's account (kb PHN-828): another account fixes it.
    public var pairAccountRefused: Bool { ui.pairErrorCode.map(Self.accountRefusals.contains) ?? false }

    // ---- chat -----------------------------------------------------------------------------------------------------

    public func typed(_ text: String) {
        dictation?.cancel()
        draft = text
    }

    // ---- speaking to FERB (kb PHN-603..PHN-605) ---------------------------------------------------------------------

    /// A press shorter than this that heard nothing is a tap: FERB says to hold the microphone instead.
    public static let tapTime: TimeInterval = 0.35

    /// What was in the message box, then the spoken words (one space between).
    public static func joinSpoken(_ before: String, _ said: String) -> String {
        if said.trimmingCharacters(in: .whitespaces).isEmpty { return before }
        if before.trimmingCharacters(in: .whitespaces).isEmpty { return said }
        return String(before.reversed().drop(while: { $0.isWhitespace }).reversed()) + " " + said
    }

    /// The owner pressed the microphone. Before the first use of this phone's kind of recognition FERB says what happens
    /// to his voice (`.voice`), then iOS asks for the microphone and speech recognition; with both, FERB listens while
    /// he holds, and the words go into the message box after what is there. Nothing is recorded, stored or sent to the
    /// Mac; the words are never sent by themselves.
    @discardableResult
    public func voicePress() -> VoicePress {
        // a new press while the last words are still on their way: those words are kept now and a new one starts
        if let d = dictation, d.finishing { AppLog.i("voice: pressed while finishing; kept and started again"); d.cancel() }
        if dictation != nil || ui.gate != .main || session.locked { return .done }
        guard let input = voice, input.kind() != .none else { note(Note("voice_unavailable")); return .done }
        let kind = input.kind()
        if !prefs.voiceTold.contains(kind.rawValue) { show(.voice(kind)); return .done }
        if !micAllowed() { return .askMicrophone }
        pressedAt = now()
        voiceStarted = Date()
        partials = 0
        released = false
        spokenOnto = draft
        listening = true
        mic = .readying
        AppLog.i("voice start kind=\(kind.rawValue)")
        let d = Dictation(input: input, kind: kind, words: { [weak self] said in
            guard let self else { return }
            self.partials += 1
            self.draft = Self.joinSpoken(self.spokenOnto, said)
        }, ended: { [weak self] end in self?.voiceEnded(end) }, later: { seconds, block in
            let task = Task { @MainActor in
                try? await Task.sleep(for: .seconds(seconds))
                if !Task.isCancelled { block() }
            }
            return { task.cancel() }
        }, ready: { [weak self] in
            guard let self, self.mic == .readying else { return }
            self.mic = .listening
            AppLog.i("voice ready after \(self.ms(since: self.voiceStarted)) ms")
        }, level: { [weak self] l in
            guard let self, self.mic == .listening else { return }
            self.micLevel = l
        })
        dictation = d
        d.start()
        return .done
    }

    /// The owner let go: the last words come, then they stay in the message box. A quick tap says to hold instead.
    public func voiceRelease() {
        guard let d = dictation else { return }
        if now().timeIntervalSince(pressedAt) < Self.tapTime && d.text.isEmpty {
            d.cancel()
            note(Note("voice_hold"))
            return
        }
        mic = .finishing
        micLevel = 0
        released = true
        d.finish()
    }

    /// The owner read the words on `.voice` and goes on: iOS's questions next, if they aren't answered.
    public func voiceTold(_ kind: SpeechKind) {
        prefs.voiceTold.insert(kind.rawValue)
        if case .voice = sheet { sheet = nil }
    }

    /// iOS's answer to the microphone and speech questions.
    public func microphoneAnswered(_ granted: Bool) { note(Note(granted ? "voice_ready" : "voice_denied")) }

    private func voiceEnded(_ end: Dictation.End) {
        let noSpeech = dictation?.lastNoSpeech ?? false
        dictation = nil
        listening = false
        mic = .idle
        micLevel = 0
        switch end {
        case .words(let text):
            AppLog.i("voice end partials=\(partials) after \(ms(since: voiceStarted)) ms")
            draft = Self.joinSpoken(spokenOnto, text)
            // held, and nothing was heard: said, never nothing (kb PHN-643)
            if text.trimmingCharacters(in: .whitespaces).isEmpty && released { note(Note("voice_no_speech")) }
        case .failed(let languageMissing, let code):
            AppLog.i("voice error code=\(code) languageMissing=\(languageMissing) after \(ms(since: voiceStarted)) ms")
            draft = spokenOnto
            // no on-device speech for this language: Apple's service next time, after its words
            if languageMissing, let next = voice?.kind(), next == .service, !prefs.voiceTold.contains(next.rawValue) {
                show(.voice(next))
            } else {
                note(Note(Self.voiceFailure(languageMissing: languageMissing, noSpeech: noSpeech, code: code)))
            }
        }
    }

    /// Each way the recogniser can fail, in plain words (kb PHN-643; Android's voiceFailure); never nothing.
    static func voiceFailure(languageMissing: Bool, noSpeech: Bool, code: Int) -> String {
        if languageMissing { return "voice_language" }
        if noSpeech { return "voice_no_speech" }
        switch code {
        case -1: return "voice_network"        // Apple's recogniser not available now
        case AppModel.micStartFailed: return "voice_mic_busy"
        case 1101, 1107, 203, -1009, -1001: return "voice_network"   // the speech service unreachable or timed out
        default: return "voice_failed"
        }
    }

    /// AppleSpeech's code when the microphone couldn't start (another app holds it).
    public static let micStartFailed = -3

    /// Leaving the app or the lock: the microphone stops at once, and what was said stays in the box.
    private func stopListening() { dictation?.cancel() }

    /// Sends through the outbox (PHN-401): with the Mac off it waits there, visible and cancellable. Files go with it.
    @discardableResult
    public func send() -> Task<Void, Never> {
        busy("send") { [self] in
            let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
            let files = transfers.ready
            // the temporary chat sends as the chat does (the owner, 2026-11-24, ORCH-715), to the Mac's memory only
            // (never the outbox): the box's files go to the Mac first, then the message with them; files already on the
            // Mac go with it now
            if temporary != nil {
                if !outgoing.chips.isEmpty {
                    draft = ""
                    outgoing.send(text, onMac: files, temporary: true)
                    if !files.isEmpty { transfers.sent() }
                    return
                }
                if text.isEmpty && files.isEmpty { return }
                draft = ""
                // his message shows at once as the chat shows his outbox, until the Mac's chat holds it (APP-1126)
                temporarySends += 1
                temporarySending = OutboxItem(id: "temporary-\(temporarySends)", text: text,
                                              createdAt: ISO8601DateFormatter().string(from: now()), state: "sending",
                                              waitingFor: nil, reason: nil, tried: false, attachments: files)
                if let t = lastTemporary { showTemporary(t) }
                var sent = false
                await callTemporary {
                    let r = files.isEmpty ? await self.api.sendTemporary(text) : await self.api.sendTemporary(text, attachments: files)
                    if case .ok = r { sent = true }
                    return r
                } failed: { e in
                    self.draft = text
                    self.failed(e)
                }
                if sent && !files.isEmpty { transfers.sent() }
                return
            }
            // files in the box: the message shows at once and goes when its files are on the Mac (kb PHN-690, PHN-691)
            if !outgoing.chips.isEmpty {
                draft = ""
                // a send from the earlier days goes to today, where his message is (kb PHN-698)
                earlier = false
                outgoing.send(text, onMac: files)
                if !files.isEmpty { transfers.sent() }
                return
            }
            if text.isEmpty && files.isEmpty { return }
            draft = ""
            earlier = false
            let r = files.isEmpty ? await api.send(text) : await api.sendWith(text, attachments: files)
            switch r {
            case .ok(let item):
                if !files.isEmpty { transfers.sent() }
                if item.waitingFor == "unlock" { macLocked() }
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

    /// "Ask me again" (PHN-611): FERB stops using the Mac's screen without asking; the line goes. When it ends on the
    /// Mac (the owner came back, the Mac slept) the status says so too; no notification.
    @discardableResult
    public func endScreenUse() -> Task<Void, Never> {
        busy("screen-use") { [self] in
            switch await api.endScreenUse() {
            case .ok(let end):
                ui.status?.screenUse = end.screenUse
                note(Note("screen_use_ended"))
            case .failed(let e):
                failed(e)
            }
        }
    }

    // ---- insomnia mode (APP-980..APP-985) ----------------------------------------------------------------------

    /// "Keep my Mac working" turned on: its warning first (APP-980), never straight on.
    public func askInsomnia() { show(.insomnia) }

    /// Keep it working (APP-980, APP-984): the way he chose and his one Yes, signed like a setting (a sensitive change
    /// on the Mac, PHN-112). The sheet goes once the Mac took it.
    @discardableResult
    public func startInsomnia(variant: String, screenYes: Bool) -> Task<Void, Never> {
        busy("insomnia", tap: true) { [self] in
            guard let signer = await freshSigner(.insomnia) else { return }
            switch await unlocker.run({ await api.startInsomnia(variant: variant, screenYes: screenYes, signer: signer) }) {
            case .ok(let r):
                ui.status?.insomnia = r.insomnia
                if sheet == .insomnia { sheet = nil }
                if r.already { note(Note("insomnia_already")) }
            case .failed(let e):
                if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    /// End (plain: it only takes something away; no prompt). How it ended is said once from the Mac's status.
    @discardableResult
    public func endInsomnia() -> Task<Void, Never> {
        busy("insomnia-end", tap: true) { [self] in
            switch await api.endInsomnia() {
            case .ok(let r): ui.status?.insomnia = r.insomnia
            case .failed(let e): failed(e)
            }
        }
    }

    /// Lock my Mac (APP-983): the owner's button only; FERB never locks by itself.
    @discardableResult
    public func lockMac() -> Task<Void, Never> {
        busy("lock-mac", tap: true) { [self] in
            switch await api.lockMac() {
            case .ok(let r):
                if let i = r.insomnia { ui.status?.insomnia = i }
                if !r.text.isEmpty { note(Note("err_mac_said", r.text)) }
            case .failed(let e): failed(e)
            }
        }
    }

    /// It's fine: the notice is answered; FERB goes on.
    @discardableResult
    public func insomniaFine(_ noticeId: String) -> Task<Void, Never> {
        busy("insomnia-fine", tap: true) { [self] in
            switch await api.dismissInsomniaNotice(noticeId) {
            case .ok(let i): ui.status?.insomnia = i
            case .failed(let e): failed(e)
            }
        }
    }

    /// Show me (APP-983): the camera view; the library refuses it on the public relay and the view says why.
    public func showCamera() {
        sheet = .camera
        camera.start()
    }

    /// The app locked under the camera: unlock (one prompt) and look again.
    @discardableResult
    public func cameraAgain() -> Task<Void, Never> {
        busy("camera") { [self] in
            guard sheet == .camera, let p = prompts else { return }
            if camera.view?.needsUnlock == true {
                guard let s = p.signer(.lock, why: .open) else { return }
                switch await api.unlock(s) {
                case .ok: ui.needsMacUnlock = false
                case .failed(let e):
                    if case .NotSigned = e {} else { failed(e) }
                    return
                }
            }
            camera.start()
        }
    }

    public func closeCamera() {
        camera.stop()
        if sheet == .camera { sheet = nil }
    }

    /// "Stop everything on the Mac" (DMN-1, PHN-125): after the owner's confirmation; no prompt, never queued.
    @discardableResult
    public func stopEverything() -> Task<Void, Never> {
        busy("stop-all", tap: true) { [self] in
            sheet = nil
            switch await api.stopEverything() {
            case .ok:
                note(Note("stop_all_done"))
                _ = await api.status()
                await apply(nil, statusOnly: true)
            case .failed(let e):
                failed(e)
            }
        }
    }

    @discardableResult
    public func loadOlder() -> Task<Void, Never> {
        busy("older") { [self] in
            guard let conv = ui.conversation, ui.older == .available else { return }
            if case .failed(let e) = await api.loadOlder(conversation: conv), case .NeedsUnlock = e { macLocked() }
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
            // "until I'm back" needs the app's unlock (the locked path unlocks and tries once more), never Face ID
            let untilBack: Bool
            if case .untilBack = input { untilBack = true } else { untilBack = false }
            if let q, q.needsProof, !declines, !untilBack { upfront = q.needs?.fingerprint == true ? .strong : .lock }
            let flow = AnswerFlow(api: api, keys: { [api] in api.state.keys }, signer: { kind in p.signer(kind, why: .yes) })
            answered(approvalId, await flow.answer(approvalId, input, upfront: upfront))
        }
    }

    private func answered(_ approvalId: String, _ r: Answered) {
        switch r {
        case .sent:
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

    /// The next page of matches, added under the ones shown (PHN-609: 500 a page, as many pages as the owner asks for).
    @discardableResult
    public func searchMore() -> Task<Void, Never>? {
        guard let s = ui.files.search, let next = s.found?.next else { return nil }
        return busy("more") { [self] in
            let r = await unlocker.run { await api.searchFiles(query: s.query, path: s.path, after: next) }
            guard var now = ui.files.search, now.query == s.query, now.path == s.path, now.found?.next == next, let had = now.found else { return }
            switch r {
            case .ok(var page):
                page.files = had.files + page.files
                now.found = page
            case .failed(let e): now.problem = Self.errorNote(e)
            }
            ui.files.search = now
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

    /// The open folder's next page, added under the entries shown (PHN-609: 500 a page, as many pages as the owner
    /// asks for); a key place's folder asks for Face ID / Touch ID for its path again, as opening it did (PHN-121).
    @discardableResult
    public func folderMore() -> Task<Void, Never>? {
        guard let f = ui.files.folder, let next = f.next else { return nil }
        return busy("more") { [self] in
            let r = await unlocker.withProof(f.locked, .file) { signer in await api.files(path: f.path, signer: signer, after: next) }
            guard let now = ui.files.folder, now.path == f.path, now.next == next else { return }
            switch r {
            case .ok(var page):
                page.entries = now.entries + page.entries
                ui.files.folder = page
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
            // for the chat they wait in the message box until Send (kb PHN-690); from the Files tab the tap is the send
            if !staged.isEmpty {
                if forChat { outgoing.pick(staged) } else { transfers.send(staged, forChat: false) }
            }
        }
    }

    /// A file's cross in the message box: it goes; nothing was sent.
    public func unpick(_ key: String) { outgoing.unpick(key) }

    /// A message with files: stop it while its files go, try it again, send its words without the files, remove it.
    public func stopSending(_ id: String) { outgoing.cancel(id) }
    public func retrySending(_ id: String) { outgoing.retry(id) }
    @discardableResult
    public func sendTextOnly(_ id: String) -> Task<Void, Never>? { outgoing.textOnly(id) }
    public func removeSending(_ id: String) { outgoing.remove(id) }

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

    /// A running task's short view (kb PHN-725): its larger picture comes from the same live preview as Watch live.
    public func showTask(_ taskId: String?, worker: String? = nil) {
        guard let taskId else { return }
        sheet = .task(taskId: taskId, worker: worker)
        let l = api.link
        if !(l?.path == .publicRelay || (l?.phase == .online && l?.bulkAllowed == false)) { live.start(taskId, worker: worker) }
    }

    /// The card rows on screen with their own pictures, by row (kb PHN-726; FERB.app's RowPreview): their keys.
    @ObservationIgnored private var rowPictureKeys: [String: [String]] = [:]
    /// The tasks whose card rows show their pictures now: the work row leaves their circles out (APP-891).
    public private(set) var tasksWithRowPictures: Set<String> = []

    /// The work row is on screen with these tasks' pictures (empty: it went), kb PHN-725.
    public func workRowShown(_ ids: [String]) {
        workRowIds = ids
        updatePictures()
    }

    /// A card on screen with its rows' pictures (`keys`, Thumbnails.key of task and step); empty: it went (kb PHN-726).
    public func rowPictures(_ row: String, _ keys: [String]) {
        rowPictureKeys[row] = keys.isEmpty ? nil : keys
        updatePictures()
    }

    /// Asks for the pictures on screen: only while the app is unlocked and in front and the path carries pictures.
    private func updatePictures() {
        let rows = rowPictureKeys.values.flatMap { $0 }
        let taken = Set(rows.compactMap { $0.split(separator: "\u{1F}").first.map(String.init) })
        if taken != tasksWithRowPictures { tasksWithRowPictures = taken }
        let l = api.link
        let bulk = l?.phase == .online && l?.bulkAllowed == true && l?.path != .publicRelay
        let want = rows + workRowIds.filter { !taken.contains($0) }
        thumbnails.show(!session.locked && session.front && bulk ? want : [])
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
        busy("settings", tap: true) { [self] in
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
        busy("setting:\(key)", tap: true) { [self] in
            guard let spec = MacSettings.spec(key) else { return note(Note("err_app_older")) }
            guard let signer = await freshSigner(.setting) else { return }
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

    /// The signer for one sensitive call (PHN-112, PHN-133): the Face ID / Touch ID key where the phone has one, else
    /// the lock key. A key made after the pairing that the Mac doesn't know yet is registered first (its one prompt, kb
    /// PHN-656). Nil when there is none (said) or the owner cancelled.
    func freshSigner(_ why: Why) async -> Signer? {
        guard let p = prompts else { note(Note("err_try_again")); return nil }
        if await api.unregisteredKeys()?.strong != nil {
            AppLog.i("\(why): registering the Face ID key first")
            let lock = p.signer(.lock, why: .keys) ?? Declined()
            let strong = p.signer(.strong, why: .keys) ?? Declined()
            switch await api.registerKeys({ kind in kind == .strong ? strong : lock }) {
            case .ok: ui.newKeys = false
            case .failed(let e):
                if case .NotSigned = e {} else { failed(e) }
                return nil
            }
        }
        guard let kind = unlocker.freshKind() else { note(Note("needs_pairing_again")); return nil }
        guard let signer = p.signer(kind, why: why) else { note(Note("err_try_again")); return nil }
        return signer
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
        if t == .chat {
            // choosing the Chat tab is a natural moment for the chat's day to move (kb PHN-697); on the chat already:
            // from the earlier days back to today, in today to its end (kb PHN-662, PHN-698)
            turnTheDay()
            if tab == .chat { if earlier { earlier = false } else { chatToEnd += 1 } }
        }
        tab = t
        switch t {
        case .tasks: loadTasks()
        case .files: filesOpened()
        case .chat: markRead()
        default: break
        }
    }

    /// "Try again" on the connection sheet (kb PHN-752): the link dials now; it never waits on the main thread.
    public func retryLink() {
        // it hands its word to the link's own lane and returns (as setOpen does): it never waits here
        api.retryLink()
    }

    /// "Earlier messages" (the home page's, or the top of today's chat): the earlier days (kb PHN-696).
    public func openChat(focus: Bool = false) {
        tab = .chat
        earlier = true
        markRead()
        if focus { focusAsk = true }
    }

    /// The message box takes the cursor once (a tap on FERB's question's notification: he answers it there, kb PHN-739).
    public private(set) var focusAsk = false
    public func focusTaken() { focusAsk = false }

    /// Back to today from the earlier days (kb PHN-698): its chat, or its home page while nothing was said on it.
    public func backToToday() {
        tab = .chat
        earlier = false
        turnTheDay()
    }

    public func show(_ s: AppSheet?) {
        func watching(_ x: AppSheet?) -> Bool { switch x { case .live, .task: return true; default: return false } }
        if watching(sheet) && !watching(s) { live.stop() }
        if sheet == .camera && s != .camera { camera.stop() }
        sheet = s
        if s == nil && pendingLocation != nil { showPending() }
    }

    public func note(_ n: Note?) { ui.note = n }

    /// The profile's switches changed outside the model: read them again.
    public func refreshProfile() {
        settings = PhoneSettings(blockScreenshots: prefs.blockScreenshots, hideChecks: prefs.hideChecksOnLockScreen,
                                 pairedSince: prefs.pairedSince, shareCity: prefs.shareCity, lockOn: prefs.lockOnOpen)
    }

    public func hideChecks(_ on: Bool) {
        prefs.hideChecksOnLockScreen = on
        refreshProfile()
    }

    /// PHN-420 on iOS (kb PHN-810, PHN-811): every picture of the Mac out of screenshots and FERB hidden while the screen is
    /// recorded or mirrored.
    public func blockScreenshots(_ on: Bool) {
        prefs.blockScreenshots = on
        refreshProfile()
    }

    /// Forget this Mac (PHN-408): the fetched chat, the Mac, the keys and the files here go; the pairing screen comes.
    @discardableResult
    public func forget() -> Task<Void, Never> {
        busy("forget", tap: true) { [self] in
            sheet = nil
            await forgetHere(quiet: false)
        }
    }

    /// Forget this Mac's work: the Mac is told first when it can be (`link.forget`, the daemon's `phone.forget_self`,
    /// PHN-646), then everything of that pairing goes here. `quiet` (signing out, another account): no line about a Mac
    /// that couldn't be told, and none at all when no Mac was paired.
    private func forgetHere(quiet: Bool) async {
        live.stop()
        camera.stop()
        let wasPaired = api.link?.paired == true
        if quiet && !wasPaired && prefs.pairedSince == 0 {
            // nothing paired: the link's identity stays (no Mac knows it)
            return
        }
        switch await api.forget() {
        case .ok(let told):
            statement = nil
            ui.macForgotPhone = false
            ui.removedWords = nil
            ui.macNeed = nil
            ui.needsMacUnlock = false
            pairChosen = false
            // PHN-646: the Mac removed this phone too, or it couldn't be told (then it still lists it until removed)
            AppLog.i("forgot the Mac; the Mac was \(told ? "told" : "not reachable")")
            if !told && !quiet { note(Note("forget_mac_kept")) }
            prefs.forgetPairing()
            temporary = nil
            lastTemporary = nil
            tab = .chat
            earlier = false
            presented = []
            transfers.clear()
            outgoing.clear()
            ui.mac = nil
            ui.macTasks = nil
            ui.files = FilesUi()
            ui.settings = nil
            ui.status = nil
            review.locked()
            refreshProfile()
        case .failed(let e):
            failed(e)
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

    // ---- the temporary chat (the owner, 2026-11-23) -------------------------------------------------------------

    /// The dashed bubble: on starts a temporary chat (or shows the one already on), off ends it; the same bubble does
    /// both, on any device. A failed toggle says so and leaves the chat as it was.
    @discardableResult
    public func toggleTemporary() -> Task<Void, Never> {
        busy("temporary", tap: true) { [self] in
            let on = temporary == nil
            await callTemporary { await self.api.setTemporaryChat(on: on) } failed: { e in
                AppLog.i("temporary chat \(on ? "start" : "end") failed: \(e.linkCode ?? "")")
                self.note(Note(on ? "temporary_start_failed" : "temporary_end_failed"))
            }
            if temporary != nil {
                tab = .chat
                earlier = false
            }
        }
    }

    /// A toggle or a send: its answer is the temporary chat after it; reads asked meanwhile run after it.
    private func callTemporary(_ call: @escaping @MainActor () async -> Outcome<TemporaryChat>,
                               failed: @MainActor (FerbError) -> Void) async {
        temporaryCalling = true
        temporaryReads += 1
        let r = await call()
        temporaryCalling = false
        temporarySending = nil
        switch r {
        case .ok(let t): showTemporary(t)
        case .failed(let e):
            if let t = lastTemporary { showTemporary(t) }
            failed(e)
        }
        if temporaryReadAfter {
            temporaryReadAfter = false
            showLastTemporary()
        }
    }

    /// Each `Change.temporary` (it went on or off on any device, or its watch brought a change or a live frame: streamed
    /// words, the plan line, a step, the answer): the temporary chat as the library holds it, from memory, as the chat's
    /// transcript is read on its changes (the owner, 2026-11-24, ORCH-715). A toggle or a send under way shows its own
    /// answer first; only the newest read is shown.
    @discardableResult
    func showLastTemporary() -> Task<Void, Never>? {
        if temporaryCalling {
            temporaryReadAfter = true
            return nil
        }
        temporaryReads += 1
        let mine = temporaryReads
        return Task { [weak self] in
            guard let self else { return }
            let r = await self.api.lastTemporaryChat()
            guard mine == self.temporaryReads, case .ok(let t) = r else { return }
            if let t { self.showTemporary(t) } else if self.lastTemporary != nil { self.temporaryEnded() }
        }
    }

    /// Reads the temporary chat from the Mac (the app opened, unlocked or back in front, the link up): whether one is on,
    /// and all of it; after that its watch keeps it. Shown while it is on, dropped once it is off. A read that fails
    /// changes nothing.
    @discardableResult
    func readTemporary() -> Task<Void, Never>? {
        if temporaryCalling {
            temporaryReadAfter = true
            return nil
        }
        temporaryReads += 1
        let mine = temporaryReads
        return Task { [weak self] in
            guard let self else { return }
            let r = await self.api.temporaryChat()
            guard mine == self.temporaryReads, case .ok(let t) = r else { return }
            self.showTemporary(t)
        }
    }

    /// The temporary chat as the Mac said it last (memory only), and his message in it on its way (reads of the chat
    /// wait while it goes, so none shows it twice).
    @ObservationIgnored private var lastTemporary: TemporaryChat?
    @ObservationIgnored private var temporarySending: OutboxItem?
    @ObservationIgnored private var temporarySends = 0

    /// The temporary chat as the Mac said it: its rows (FERB's words laid out by the library) while on, nothing off;
    /// his message on its way at its end until the send is answered.
    private func showTemporary(_ t: TemporaryChat) {
        // ended (on any device): its messages with files still on their way go too
        if lastTemporary != nil && !t.on { outgoing.dropTemporary() }
        lastTemporary = t.on ? t : nil
        let shown = t.on ? TemporaryShown(t, sending: temporarySending) : nil
        if temporary != shown { temporary = shown }
        runTemporary()
    }

    /// The temporary chat ended (the library holds none now): the normal chat shows, and its messages with files still
    /// on their way go.
    private func temporaryEnded() {
        outgoing.dropTemporary()
        lastTemporary = nil
        if temporary != nil { temporary = nil }
        runTemporary()
    }

    /// The temporary chat's work row: its turns' live lines and the Mac's running rows that are its.
    private func runTemporary() {
        let r = temporary.flatMap { t in
            RunningModel.of(t.transcript, status: statusPart(ui.status, temporary: true) ?? PhoneStatus(
                locked: false, words: true, stopped: false, questions: [], running: [], results: [], screenUse: nil), mac: nil)
        }
        if temporaryRunning != r { temporaryRunning = r }
    }

    // ---- helpers ---------------------------------------------------------------------------------------------------

    func failed(_ e: FerbError) { note(Self.errorNote(e)) }

    /// Runs `block` once at a time per `what` (the screen greys the control while `ui.busy` holds it).
    @discardableResult
    /// One job of `what` at a time. A tap (`tap`) that comes while it still runs is never a silent no-op: it says FERB
    /// is still on it (kb PHN-656).
    func busy(_ what: String, tap: Bool = false, _ block: @escaping @MainActor () async -> Void) -> Task<Void, Never> {
        Task { [self] in
            if ui.busy.contains(what) {
                if tap { note(Note("still_working")) }
                AppLog.i("tap while \(what) is still working")
                return
            }
            ui.busy.insert(what)
            await block()
            ui.busy.remove(what)
        }
    }

    public static func errorNote(_ e: FerbError) -> Note {
        if e.linkCode == Unlocker.pairAgain { return Note("needs_pairing_again") }
        // a feature the paired Mac's FERB doesn't have yet: said, never a silent failure (kb PHN-656)
        if e.linkCode == "mac_link_older" || e.linkCode == "method_not_allowed" { return Note("err_mac_older") }
        if e.linkCode == "app_older" { return Note("err_app_older") }
        switch e {
        case .Offline: return Note("err_offline")
        case .NeedsBetterPath: return Note("err_needs_better_path")
        case .NeedsUnlock: return Note("err_needs_unlock")
        case .Refused, .NotFound: return Note("err_mac_said", e.detail)
        default: return Note("err_other", e.detail)
        }
    }

    /// The Mac's refusals of the phone's account at pairing (the link's error codes, kb PHN-828).
    public static let accountRefusals: Set<String> = ["account_mismatch", "mac_signed_out", "account_unconfirmed"]

    public static func pairNote(_ code: String?, _ detail: String?) -> Note {
        switch code {
        case "pair_window_closed": return Note("pair_expired")
        case "pair_secret_wrong", "bad_request", "refused": return Note("pair_wrong_code")
        case "device_limit": return Note("pair_limit")
        case "pair_denied": return Note("pair_denied")
        case "pair_busy": return Note("pair_busy")
        case "already_paired": return Note("pair_already")
        case "mac_unreachable", "offline": return Note("pair_unreachable")
        // the Mac's own words for an account it won't pair with (kb PHN-828): they name both accounts
        case let c? where accountRefusals.contains(c) && !(detail ?? "").isEmpty: return Note("err_mac_said", detail ?? "")
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

/// No prompt could be shown: signs nothing (the library answers `not_signed`).
private final class Declined: Signer, @unchecked Sendable {
    func sign(payload: String) -> String? { nil }
}
