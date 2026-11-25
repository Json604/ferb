import AppKit
import SwiftUI
import UniformTypeIdentifiers
import NotchCore
import os

/// A running loop's latest `loop.status` (L31): what it keeps, its state (acting, watching, backing_off, paused), how
/// many rounds fixed something and when the last one ended.
struct StandingWork: Equatable {
    var goal: String
    var state: String
    var rounds: Int
    var lastRoundEnd: Date?
    var gapSeconds: Double
}

extension StandingWork {
    /// From a `loop.status` event or a `chat.status` loop row; nil once the loop finished or stopped. Its numbers
    /// arrive as JSON whole or decimal numbers, and `last_round_end` (epoch seconds) is null before any round ended.
    init?(loopStatus row: [String: Any]) {
        let state = row["state"] as? String ?? "watching"
        guard !["finished", "stopped"].contains(state) else { return nil }
        self.init(goal: row["goal"] as? String ?? "", state: state,
                  rounds: (row["rounds"] as? NSNumber)?.intValue ?? 0,
                  lastRoundEnd: (row["last_round_end"] as? NSNumber).map { Date(timeIntervalSince1970: $0.doubleValue) },
                  gapSeconds: (row["gap_seconds"] as? NSNumber)?.doubleValue ?? 0)
    }
}

/// The owner asked to answer this question: its field has keyboard focus until it loses it, the question is
/// resolved or the body hides. Only an explicit owner action creates one.
struct QuestionFocus: Equatable {
    let id: String
    let token = UUID()
}

@MainActor
final class NotchModel: ObservableObject {
    /// How the saved chat was read back at launch (APP-801): counts and keys, never chat text.
    nonisolated static let historyLog = Logger(subsystem: "com.ferb.FERB", category: "history")
    // The task list's inputs (TaskList.swift): each change drops the list built from them, so it is built again once.
    @Published var session = NotchSession() {
        didSet {
            taskListSessionChanged(from: oldValue)
            if !waitingForComposer.isEmpty && canCompose { scheduleWaitingForComposer() }
        }
    }
    @Published private(set) var textRenderGeneration = 0
    private var textPresentationExpanded = false
    private var textRevealPending = false

    /// Repeated presentation requests can replace an opening animation. Keep the
    /// pending glyph refresh until the winning completion; already-open updates do none.
    func noteTextPresentation(expanded: Bool) {
        if expanded && !textPresentationExpanded { textRevealPending = true }
        if !expanded { textRevealPending = false }
        textPresentationExpanded = expanded
    }

    func completeTextReveal() {
        guard textPresentationExpanded, textRevealPending else { return }
        textRevealPending = false
        textRenderGeneration += 1
    }

    @Published var preferredPanelWidth: CGFloat = 440
    @Published var preferredTranscriptHeight: CGFloat = 300
    @Published var localSettingReply: String?
    @Published var indicatorEdge = "top"
    /// The sticky window holds the chat (Where FERB opens: Window, outside the tour; APP-745): `expanded` then means the
    /// window shows, the notch mounts no chat body and nothing dismisses it automatically. Set by PanelCoordinator.
    @Published var chatInWindow = false
    /// The chat body's size in the window (its width, and its height below the window's header): the transcript and
    /// composer measure against it while the window holds the chat.
    @Published var windowChatSize: CGSize?
    /// Where the transcript was when its host let go of it, for the host that shows it next (APP-745).
    var transcriptResume: TranscriptResume?
    @Published var isNotchDocked = false
    var workSessions: [UUID: NotchSession] = [:] { didSet { taskListCache.invalidate() } }
    var taskListCache = TaskListCache()

    /// Sets a published value only when it differs: setting one, even to the same value, tells every view of the model
    /// to redraw (the whole chat), so a periodic read that finds nothing new must not set anything.
    func changeIfNeeded<Value: Equatable>(_ path: ReferenceWritableKeyPath<NotchModel, Value>, _ value: Value) {
        if self[keyPath: path] != value { self[keyPath: path] = value }
    }

    /// Changes the conversation through `change` and publishes it only when it really changed (a mutating call on the
    /// published archive publishes even when it changes nothing). Returns what `change` returned.
    @discardableResult
    func changeConversation<Result>(_ change: (inout ConversationArchive) -> Result) -> Result {
        var archive = conversation
        let result = change(&archive)
        if archive != conversation { conversation = archive }
        return result
    }
    var submissions: [UUID: Task<Void, Never>] = [:]
    @Published var pendingStops = Set<UUID>()
    /// The saved chat: kept in the chat history (APP-805) and the archive.
    @Published var savedConversation = ConversationArchive() {
        didSet { taskListCache.invalidate(); shownTranscript = nil }
    }
    /// The temporary chat while it is on (owner 2026-11-23; TemporaryChatModel.swift): what the chat shows instead of
    /// the saved one, held in memory only and read from the daemon. Its `archive` is written by the same pipeline.
    @Published var temporaryChat: TemporaryChat? { didSet { shownTranscript = nil; taskListCache.invalidate() } }
    /// Which chat the chat pipeline reads and writes now: the saved one, or the temporary one inside `inChat`.
    private(set) var chatSource = ChatSource.saved
    /// The conversation the chat pipeline reads and writes (a message sent, its stream's frames, a task's outcome, a
    /// question's place, an entry another device made): the saved chat's, or the temporary chat's inside
    /// `inChat(.temporary)`. Every turn goes through the same code whichever chat holds it (owner 2026-11-24: "i told
    /// you to match normal chat and temp chat UI, why are they even diff???").
    var conversation: ConversationArchive {
        get { chatSource == .temporary ? temporaryChat?.archive ?? ConversationArchive() : savedConversation }
        set {
            if chatSource == .temporary { temporaryChat?.archive = newValue } else { savedConversation = newValue }
        }
    }
    /// Runs `body` with the chat pipeline on `source`'s conversation (and its entries' state, `entryWatch`), as
    /// `withWorkSession` runs a stream's frame on its session. Synchronous only: nothing can come between. A chat
    /// that ended takes nothing more: `body` doesn't run (its turns' late frames reach no chat, the saved one least).
    @discardableResult
    func inChat<Result>(_ source: ChatSource, otherwise: Result, _ body: () -> Result) -> Result {
        if source == .temporary && temporaryChat == nil { return otherwise }
        let before = chatSource
        chatSource = source
        defer { chatSource = before }
        return body()
    }
    func inChat(_ source: ChatSource, _ body: () -> Void) { inChat(source, otherwise: (), body) }
    /// The temporary chat's icon was tapped and FERB hasn't answered yet.
    @Published var temporaryToggling = false
    let temporary = TemporaryChatState()
    /// The shown day's entries as last built (`visibleEntries`): the chat, the notch's size on every pointer move and
    /// frame, and the questions all read them, so they are built once per change of the conversation or the day.
    var shownTranscript: (day: Date, entries: [TranscriptEntry])?
    /// Where the chat is kept and what the daemon holds of it (APP-805).
    let chatStore = HistoryState()
    /// The engine's shared conversation as this app follows it (ENG-10): turns other devices sent (`EntryWatch.swift`).
    let savedEntryWatch = EntryWatchState()
    /// The entries' state of the chat the pipeline is on (`chatSource`): the saved chat's watch, or the temporary
    /// chat's (`temporary.entries`), read by the same entry path.
    var entryWatch: EntryWatchState { chatSource == .temporary ? temporary.entries : savedEntryWatch }
    /// What is shown of each day (APP-806); changed only through `updateWindow`.
    var dayWindows: [Date: DayWindow] = [:]
    /// The days the daemon has chat for (History), read when History opens.
    @Published var historyDays: [Date] = []
    @Published var followsToday = true
    @Published var selectedDay = Calendar.current.startOfDay(for: Date())
    @Published var handleWidth: CGFloat = 180
    @Published var cutoutHeight: CGFloat = 40
    @Published var hasHardwareNotch = true
    @Published var isDropTarget = false
    /// Files and folders dragged onto FERB: every drop target reports here (NotchFileDrop.swift).
    let drops = FileDropSession()
    @Published var showingSources = false
    @Published var showingHistory = false
    @Published var showingActivity = false
    @Published var showingTasks = false
    @Published var inspectedTaskID: String?
    @Published var taskInspection: String?
    @Published var taskRecordURL: URL?
    var taskInspectionRequest = UUID()
    var taskInspectionJob: Task<Void, Never>?
    let taskRecordFiles = TaskRecordFiles()
    @Published var stoppingTaskIDs = Set<String>()
    @Published var focusRequest = UUID()
    /// The microphone's loudness while dictating (APP-674), observed by the meter alone.
    let voiceMeter = VoiceMeter()
    @Published var voiceIsHeld = false
    @Published private(set) var completionTraceVisible = false
    @Published private(set) var completionStartedAt: Date?
    @Published private(set) var attentionTraceVisible = false
    @Published private(set) var attentionStartedAt: Date?
    var announcedAttentionIDs = Set<String>()
    var refreshingStatus = false
    @Published var acknowledgedIndicatorSignal: String?
    var taskStateRevision = 0
    var reconciledMissingTasks = Set<String>()
    /// The owner opened this chat with a press on the closed notch or pill (APP-705). Kept until the chat closes.
    private(set) var openedByPress = false
    /// A chat the owner opened with a press stays open when the pointer leaves it, whatever the hover setting, until
    /// Escape, the shortcut, Hide or a click outside FERB (APP-707). A hover-opened chat folds on hover-out (APP-557).
    var staysOpenOnHoverOut: Bool { openedByPress && expanded }

    let feedback: SoundFeedback
    var cursor: CursorController?
    let microphone: VoiceCapturing
    @Published var voiceTranscript = ""
    @Published var voiceFinishing = false
    private var voiceDraft = ""
    /// Settings → Notch & Shortcuts: a held dictation sends its draft when the keys are released.
    static let sendOnReleaseKey = AppSettings.sendOnRelease.key
    /// FERB's guide (`about.guide`) follows FERB.app's settings: the declared values last sent, a pending re-send after
    /// a change (debounced by `guideDebounce`), and the defaults observer that notices one.
    var guideSent: [[String: String]]?
    var guideResend: Task<Void, Never>?
    var guideDebounce: Duration = .seconds(1)
    private var guideObserver: NSObjectProtocol?
    let rpc: DaemonRPCTransport
    /// This Mac's Location permission, city and exact fixes, told to the daemon (SENSE-420); nil outside the real app.
    var deviceLocation: DeviceLocationReporter?
    /// FERB's click-through windows, told to the daemon (D31 fix round 4)
    let clickThrough: ClickThroughReport
    /// Insomnia mode (APP-980..APP-985): the daemon's status, the cover's states and FERB.app's reports
    /// (InsomniaModel.swift); the cover's windows attach as its surface in the packaged app only.
    let insomnia: InsomniaController
    /// The daemon's insomnia status as last read (nil: none read, or a daemon without it): the notch's line, the menu.
    @Published var insomniaStatus: InsomniaStatus?
    /// Insomnia mode's root helper (APP-986), the packaged FERB.app's only.
    var powerHelper: PowerHelper?
    let preferences: UserDefaults
    @Published var connectionStatus = "Connecting to FERB…"
    /// L34 D: the daemon as the menu bar shows it.
    @Published var daemonLink = DaemonLink()
    @Published var daemonPaused = false
    @Published var daemonStopped = false
    /// L34 D: a question was answered (here or elsewhere), so its banner, if any, is withdrawn.
    var onApprovalResolved: ((String) -> Void)?

    /// FERB.app's setup hasn't set the background service up yet, or is installing, starting or restarting it now
    /// (AppSetupController keeps it): a daemon that doesn't answer meanwhile reads as setting up (APP-698).
    @Published var serviceSettingUp = false

    var menuBarState: MenuBarState {
        MenuBarState.derive(link: daemonLink.state, paused: daemonPaused, stopped: daemonStopped,
                            questions: pendingApprovals.count, running: runningTasks.count,
                            settingUp: serviceSettingUp)
    }

    /// The connection line of the notch's profile menu: what the menu bar says while the service is being set up,
    /// never the refused connection's "isn't running" (APP-698).
    var connectionLine: String {
        menuBarState == .settingUp ? MenuBarState.settingUp.statusLine : connectionStatus
    }
    @Published var pendingApprovals: [DaemonApproval] = [] {
        didSet {
            guard pendingApprovals != oldValue else { return }
            taskListCache.invalidate()
            paymentPictures.keep(Set(pendingApprovals.map(\.id)))    // GATE-579: an answered question's picture goes
        }
    }
    /// GATE-579, APP-828: the payment questions' one-time pictures, in memory only.
    let paymentPictures: PaymentPictures
    /// MEM-460: FERB's review of what an import brought in, one model for the chat's card and the Memory page.
    let memoryReview: MemoryReviewModel
    @Published var questionFocus: QuestionFocus?
    @Published var promptError: String?
    /// FERB's alerts above the composer (APP-1105, APP-1107): shown for a while, never kept in the chat.
    @Published var alerts: [OwnerAlert] = []
    /// The toast's stack is open, every waiting alert shown (APP-1109).
    @Published var alertsExpanded = false
    /// Alerts put away this run: a status read before the daemon took the ack never brings one back.
    var dismissedAlerts = Set<String>()
    /// The questions whose answer is on its way to the daemon, by approval id. Each card waits only on its own answer:
    /// another open question stays answerable meanwhile (several are open at once: a plan's asks, a safety check).
    @Published var sendingAnswers: Set<String> = []
    func isSending(_ approval: DaemonApproval) -> Bool { sendingAnswers.contains(approval.id) }
    /// The running tasks of both chats, by task: their headings. Which chat's work row shows one is the task list's
    /// (`makeTaskList(_:)`, by `source(ofTask:)`).
    @Published var runningTasks: [String: String] = [:] {
        didSet { if runningTasks != oldValue { runningChanged(from: oldValue) } }
    }

    /// Whether a task runs now, in either chat.
    func isRunning(task id: String) -> Bool { runningTasks[id] != nil }

    /// The running tasks changed (`old`: what they were).
    private func runningChanged(from old: [String: String]) {
        taskListCache.invalidate()
        // When each running task started, for the work row's timer (a friend's feedback 2026-10-27): kept for
        // tasks still running; one first seen here starts now unless the daemon's status says otherwise.
        var since = runningSince.filter { isRunning(task: $0.key) }
        for id in runningTasks.keys where since[id] == nil { since[id] = Date() }
        if since != runningSince { runningSince = since }
        // a task that ended is no longer paused
        let ended = pausedTasks.filter { old[$0] != nil && !isRunning(task: $0) }
        if !ended.isEmpty { pausedTasks.subtract(ended) }
    }
    /// Running tasks the owner paused in place (ORCH-308): the daemon's `task.paused` until its `task.resumed` or the
    /// task's end; after a reconnect, read from tasks.list's status (APP-901).
    @Published var pausedTasks: Set<String> = [] {
        didSet { if pausedTasks != oldValue { taskListCache.invalidate() } }
    }
    /// Keyed by task, and by step for a card row's watch button (its step task id).
    private var taskPreviews: [String: TaskPreview] = [:]
    let previewAccess = PreviewAccess()
    func preview(for item: TaskItem) -> TaskPreview { preview(task: item.backendID ?? item.id) }
    func preview(task id: String, worker: String? = nil) -> TaskPreview {
        let key = worker.map { id + " " + $0 } ?? id
        if let existing = taskPreviews[key] { return existing }
        let created = TaskPreview(rpc: rpc, taskID: id, worker: worker, access: previewAccess)
        taskPreviews[key] = created
        return created
    }
    func discardFinishedPreviews() {
        let active = underwayTaskIDs            // both chats': a picture of the chat not shown stays for its return
        for (key, preview) in taskPreviews where !active.contains(preview.taskID) {
            taskPreviews.removeValue(forKey: key)?.stop()
        }
    }

    @Published var taskActivities: [String: [TaskActivity]] = [:] {
        didSet { taskListCache.invalidate() }
    }
    @Published var runningSince: [String: Date] = [:]
    /// Running loops (L31), keyed by root task id: standing work, shown with its counts until the loop or its task ends.
    @Published var standingWork: [String: StandingWork] = [:] {
        didSet { if standingWork != oldValue { taskListCache.invalidate() } }
    }
    /// The memory import's live card (MEM-456): the daemon's words, at the end of today's chat while an import goes on.
    @Published var memoryImport: MemoryImportCard?
    var conversationID: String?
    var supportsAttachments = false
    var eventsTask: Task<Void, Never>?
    var reconciliationTask: Task<Void, Never>?
    var submissionTask: Task<Void, Never>?
    /// Which parts of the composer draft were pasted: marked when sent (the daemon counts them as the owner's words,
    /// ORCH-518) and never run as a local notch command (APP-596).
    var pasteProvenance = PasteProvenance()
    /// The long pastes held as chips in the composer (APP-935), in the order they were pasted.
    var heldPastes: [HeldPaste] { pasteProvenance.held }
    /// The chip whose whole text the composer shows (clicked) in its editor (APP-1143), if any.
    @Published private(set) var shownPaste: UUID?
    /// Pictures pasted or dropped with no file behind them and promised files, received where the background service
    /// can read them (APP-933, APP-936).
    var receivedFiles = ReceivedFiles()
    /// Promised files still on their way into the composer, shown as arriving (APP-936).
    @Published private(set) var receivingFiles: [ReceivingFile] = []
    /// Arrivals the owner took out before they came: what comes is deleted.
    private var cancelledReceives = Set<UUID>()
    /// The time a pasted picture is named after (tests fix it).
    var now: () -> Date = Date.init
    var knownCompletedTasks = Set<String>()
    /// An unreadable archive couldn't be set aside, so nothing may be saved over it.
    var archiveBlocked = false
    /// Send to developer (L35): each answer's report, by turn id (live state, never saved).
    @Published var reports: [UUID: ReportState] = [:]
    /// A runtime switch the owner chose on a failed answer (ORCH-315), by turn: what it says now. Live state only.
    @Published var runtimeChoices: [UUID: String] = [:]
    /// Each card row's live line (APP-718), by its step's task id: what that step is doing now. Live state only.
    @Published var stepLive: [String: LiveProgress] = [:]
    /// The collapsed texts the owner opened (APP-719), by (turn, part or answer). Kept until FERB.app quits.
    @Published var expandedText = ExpansionState()
    /// The latest item FERB asks the transcript to bring forward (APP-720); the transcript decides scroll or pill.
    /// The newest requests (several may arrive in one update; the transcript reads every one it hasn't applied).
    @Published private(set) var scrollRequests: [TranscriptScrollRequest] = []
    var scrollRequest: TranscriptScrollRequest? { scrollRequests.last }
    private var scrollGeneration = 0
    /// The newest request the transcript applied (a transcript shown again never applies an old one twice).
    var scrollHandled = 0

    /// Asks the transcript to bring an item forward. Called before FERB opens the notch for it, so a hidden body lands
    /// on the item without animating (`revealing`).
    func requestScroll(_ target: String, _ reason: TranscriptScrollRequest.Reason, label: String = "") {
        revealForScroll(target)                 // an entry the shown day's window hides opens first (APP-806)
        scrollGeneration += 1
        scrollRequests = (scrollRequests + [TranscriptScrollRequest(target: target, reason: reason, label: label,
                                                                     generation: scrollGeneration,
                                                                     revealing: !bodyVisible)]).suffix(8)
    }

    /// Show more / Show less on a collapsed text; collapsing keeps the item's top in view.
    func toggleExpanded(_ key: String, target: String) {
        let opening = !expandedText.isOpen(key)
        expandedText.toggle(key)
        if !opening { requestScroll(target, .collapse) }
    }
    /// Opens a report file the owner asked to view; tests replace it.
    var openReportFile: (URL) -> Void = { HandOff.open($0) }

    init(rpc: DaemonRPCTransport = DaemonRPC(), preferences: UserDefaults = .standard, microphone: VoiceCapturing? = nil) {
        self.microphone = microphone ?? Microphone()
        self.rpc = rpc
        self.clickThrough = ClickThroughReport(rpc: rpc)
        self.insomnia = InsomniaController(rpc: rpc, clickThrough: clickThrough, ownBundle: Bundle.main.bundleIdentifier,
                                           ownPID: ProcessInfo.processInfo.processIdentifier)
        self.paymentPictures = PaymentPictures(rpc: rpc)
        self.memoryReview = MemoryReviewModel(rpc: rpc)
        self.preferences = preferences
        feedback = SoundFeedback(defaults: preferences)
        preferredPanelWidth = max(360, CGFloat((preferences.object(forKey: "notch.panelWidth") as? NSNumber)?.doubleValue ?? 440))
        preferredTranscriptHeight = max(260, CGFloat((preferences.object(forKey: "notch.transcriptHeight") as? NSNumber)?.doubleValue ?? 300))
        // With the chat in FERB's database (APP-805) the preferences archive isn't read: the daemon's pages are.
        if openHistory() {
            // History this build can't read is never saved over: what it can't read is kept aside, unchanged (APP-801).
            let loaded = ArchiveStore.load(preferences)
            archiveBlocked = loaded.blocked
            if let saved = loaded.archive {
                conversation = saved
                conversation.interruptRestoredDrafts()
                if conversation != saved, !archiveBlocked, let repaired = try? JSONEncoder().encode(conversation.forStorage()) {
                    preferences.set(repaired, forKey: ArchiveStore.archiveKey)
                }
            }
            // A copy an earlier build set aside unread comes back once this build can read it (APP-801).
            let recovery = archiveBlocked ? [] : ArchiveStore.recover(into: &conversation, preferences)
            for line in loaded.log + recovery { Self.historyLog.notice("\(line, privacy: .public)") }
        }
        conversationID = preferences.string(forKey: "FERBConversationID")
        guideObserver = NotificationCenter.default.addObserver(forName: UserDefaults.didChangeNotification,
                                                               object: preferences, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.appSettingsChanged() }
        }
        announcedAttentionIDs = Set(preferences.stringArray(forKey: "FERBAnnouncedAttentionIDs") ?? [])
        insomnia.onStatus = { [weak self] status in self?.insomniaChanged(status) }
        drops.accepts = { [weak self] in self?.acceptsDroppedFiles ?? false }
        drops.isOpen = { [weak self] in self?.bodyVisible ?? false }
        drops.onDrop = { [weak self] items in self?.drop(items) ?? false }
        drops.onTargeted = { [weak self] targeted in self?.isDropTarget = targeted }
        acknowledgement.changed = { [weak self] text in
            guard let self else { return }
            let wasVisible = self.surfaceVisible
            let previousSize = self.surfaceSize
            self.compactPlanText = text
            if let text {
                self.compactPlanDisplayText = text
                self.compactReminderCard = self.acknowledgement.reminderCard
            }
            if text != nil && self.introLine == nil { self.introDisplayLine = nil }   // a closed intro line can't show
            if self.bodyVisible { self.compactPlanDisplayText = nil }
            // Text within the same measured lines does not restart the native reveal spring.
            if !self.bodyVisible && (wasVisible != self.surfaceVisible || previousSize != self.surfaceSize) {
                self.onBackgroundChange?(true)
            }
        }
        // MEM-460: the review's question closes when nothing is left to answer (the daemon closes it then): a list read
        // empty takes its card away at once, never a question card with no list
        memoryReview.emptied = { [weak self] in
            guard let self, self.pendingApprovals.contains(where: { $0.capability == MemoryReview.capability }) else { return }
            self.pendingApprovals.removeAll { $0.capability == MemoryReview.capability }
            self.onBackgroundChange?(true)
        }
    }

    var onPresentationChange: ((Bool) -> Void)?
    var onBackgroundChange: ((Bool) -> Void)?
    /// A notice's page card (step 12c): the FERB window's Memory page, filtered to an import's source when it names one.
    var onOpenMemory: ((String?) -> Void)?
    /// A phone asking to pair (T-1205): its notice's button opens Settings → Phone, where the owner answers.
    var onOpenPhone: (() -> Void)?
    let acknowledgement = AcknowledgementNotice()
    var onAttentionArrival: (() -> Void)?
    var onOpenDocument: ((URL) -> Void)?
    var onQuickLook: ((URL) -> Void)?
    /// Where a file card's click and a link's click go (APP-715); tests record them instead.
    var revealInFinder: ([URL]) -> Void = { HandOff.reveal($0) }
    var openURL: (URL) -> Void = { HandOff.open($0) }
    /// Copy path / Copy link write here; tests use a private pasteboard.
    var pasteboard: NSPasteboard = .general
    var onSettings: (() -> Void)?
    var onOpenWindow: (() -> Void)?
    var onStatusChange: (() -> Void)?
    var onInteractionEnded: (() -> Void)?
    var onInteraction: (() -> Void)?
    /// Detach activity: a PreviewSubject's key (a task, or one of its steps, APP-912) to open in a window of its own.
    var onDetachTaskActivity: ((String) -> Void)?
    private var completionTask: Task<Void, Never>?
    private var attentionTask: Task<Void, Never>?
    private var profileMenuOpen = false
    private(set) var filePickerOpen = false
    /// Why FERB's windows are currently stepping aside for another surface (its own file picker, another app's
    /// menu over the notch, an app the owner works in under the tour's line). They live in an overlay space
    /// above every ordinary window, so nothing can be raised over them by window level.
    private(set) var yielding = NotchYield()
    /// Called when FERB starts (true) or stops (false) yielding; the coordinator makes its windows invisible and
    /// click-through for that duration. Draft, attachments and conversation are untouched.
    var onYieldChange: ((Bool) -> Void)?
    private var heldVoiceSnapshot: (session: NotchSession, sources: Bool, history: Bool, activity: Bool)?

    @Published private(set) var compactPlanText: String?
    /// Retain the compact content while the same native surface closes; never expose transcript mid-collapse.
    @Published private(set) var compactPlanDisplayText: String?
    @Published private(set) var compactReminderCard: ResultCard?
    /// The owner's timers still running (`reminders.list`, APP-762): the closed notch's live activity shows the soonest.
    @Published var runningTimers: [RunningTimer] = []
    /// What FERB's rows say happened to each timer or reminder the app shows (DMN-548: `timers.states`, then
    /// `timer.changed`): its cards, the closed notch and the done alert read this, never state of their own.
    @Published var timerRecords: [String: TimerRecord] = [:]
    /// Timers whose done alert the owner stopped (Stop or Repeat) in this session: their Stop and Repeat go.
    @Published var stoppedTimers: Set<String> = []
    /// Timers whose record was asked for (a card showed): connect asks for all of them again.
    var askedTimerStates: Set<String> = []
    private var measuredReminder: (card: ResultCard, text: String, width: CGFloat, minute: Int, height: CGFloat)?
    func finishCompactPreviewDismissal() {
        if compactPlanText == nil { compactPlanDisplayText = nil }
        if introLine == nil { introDisplayLine = nil }
    }
    /// FERB's line in the first-run intro (IntroNarration.swift): set, the notch swells to show it; nil, it shrinks back.
    @Published var introLine: IntroLine? {
        willSet { introSurfaceBefore = (surfaceVisible, surfaceSize) }
        didSet {
            if introLine != oldValue { introTyped = "" }
            if introLine != nil && compactPlanText == nil { compactPlanDisplayText = nil }   // a closed preview can't show
            if introLine == nil && compactPlanText != nil { introDisplayLine = nil }   // the live plan takes over
            introLineChanged(from: oldValue)
        }
    }
    var introSurfaceBefore = (visible: false, size: CGSize.zero)
    /// Kept while the surface closes, like the plan preview's text.
    @Published var introDisplayLine: IntroLine?
    /// The notch's single swell as it takes the orb back (`pulseIntro()`).
    @Published var introPulsing = false
    var introPulseTask: Task<Void, Never>?
    /// What the owner did on an intro line (a choice, a typed entry, Skip); set by the
    /// intro controller while the tour runs. Set, the line also offers Skip.
    var onIntroAction: ((IntroAction) -> Void)? {
        didSet { if (oldValue == nil) != (onIntroAction == nil) { onTourChange?() } }
    }
    /// The tour (or its service step) started or ended: the coordinator moves the chat to the notch for it (APP-745).
    var onTourChange: (() -> Void)?
    /// The tour or its service step runs (its controller sets `onIntroAction` while it does).
    var tourRunning: Bool { onIntroAction != nil }
    /// While the tour runs, a request typed or spoken stays in the composer instead of starting a task, except while
    /// the tour's own real task runs (APP-666: its questions may need a typed answer); it is sent after the tour.
    var introHoldsWork = false
    /// The running tour is at the permissions beat or later: its line offers Skip tour (APP-685).
    @Published var introSkippable = false
    /// The tour wants System Settings in front of it: the notch steps behind Settings' window when it lies under the
    /// tour's surface, as when Settings comes forward (APP-642).
    var onStepBehindSettings: (() -> Void)?
    /// The tour has something to acknowledge (a grant macOS just confirmed): the notch stops yielding and shows above
    /// System Settings, without taking keyboard focus (APP-687, APP-562); the next dock beside Settings steps behind
    /// it again (APP-642).
    var onComeForward: (() -> Void)?
    static let introHeldWork = "It'll wait here until the tour ends."
    /// What the owner is typing into the intro line's field. Emptied whenever the line changes (or goes), so a key
    /// pasted for one line can never show in, or be sent from, the next line's field.
    @Published var introTyped = ""
    var surfaceVisible: Bool { expanded || compactPlanText != nil || introLine != nil || introPulsing }
    var compactPlanHeight: CGFloat {
        guard let compactPlanText else { return 0 }
        if let card = compactReminderCard {
            let minute = Int(Date().timeIntervalSince1970 / 60)
            if measuredReminder?.card != card || measuredReminder?.text != compactPlanText
                || measuredReminder?.width != bodyWidth || measuredReminder?.minute != minute {
                let content = ReminderArrivalView(card: card, text: compactPlanText)
                    .frame(width: bodyWidth).environment(\.colorScheme, .dark)
                let height = NSHostingView(rootView: content).fittingSize.height
                measuredReminder = (card, compactPlanText, bodyWidth, minute, ceil(height))
            }
            return min(availableSurfaceSize.height, (measuredReminder?.height ?? 0)
                + (isNotchDocked ? cutoutHeight : 0))
        }
        let text = (compactPlanText as NSString).boundingRect(
            with: CGSize(width: max(1, bodyWidth - 50), height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: NSFont.systemFont(ofSize: 13)])
        let cameraSpace = isNotchDocked ? cutoutHeight : 0
        return min(availableSurfaceSize.height, ceil(text.height) + 34 + cameraSpace)
    }
    var expanded: Bool { session.isExpanded }
    var bodyVisible: Bool { expanded }
    var isYielded: Bool { self.yielding.isActive }
    @Published var availableSurfaceSize = CGSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    var bodyWidth: CGFloat {
        if chatInWindow, let size = windowChatSize { return size.width }
        return min(availableSurfaceSize.width, max(360, preferredPanelWidth))
    }
    var hoverEnabled: Bool { preferences.object(forKey: "notch.hoverEnabled") as? Bool ?? true }
    /// Hovering opens the chat, except over an intro line: the owner is reading it (or reaching for its choices).
    var hoverOpensChat: Bool { hoverEnabled && introLine == nil }
    var indicatorHeaderAtBottom: Bool { indicatorEdge == "bottom" }
    var showsCompletionGlow: Bool { completionTraceVisible }
    var showsTraceGlow: Bool { attentionTraceVisible || showsCompletionGlow }
    /// Whether the camera-edge light may show. It never takes a click, so while FERB yields it still shows for a
    /// question or approval (whose sound plays as always): a question that arrives then is never hidden silently.
    /// The completion glow yields with the rest of FERB.
    var tracerLightShows: Bool { !isYielded || attentionTraceVisible }
    /// Measured once per draft and width: the notch's size reads it on every pointer move and animation frame.
    var composerHeight: CGFloat {
        let textWidth = bodyWidth - 152
        if let last = composerMeasure, last.width == textWidth, last.draft == session.draft { return last.height }
        let measured = ((session.draft + "\u{200B}") as NSString).boundingRect(
            with: CGSize(width: textWidth, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: NSFont.systemFont(ofSize: 14)])
        let height = min(72, max(18, ceil(measured.height))) + 18
        composerMeasure = (session.draft, textWidth, height)
        return height
    }
    private var composerMeasure: (draft: String, width: CGFloat, height: CGFloat)?
    /// One horizontal card strip, independent of the number of attachments and held pastes, and the shown paste's text.
    var attachmentStripHeight: CGFloat {
        (session.attachments.isEmpty && heldPastes.isEmpty && receivingFiles.isEmpty ? 0 : 100)
            + (shownHeldPaste == nil ? 0 : Self.shownPasteHeight)
    }
    static let shownPasteHeight: CGFloat = 190
    var shownHeldPaste: HeldPaste? { shownPaste.flatMap { id in heldPastes.first { $0.id == id } } }
    var conversationChromeHeight: CGFloat {
        composerHeight + 58 + (showsWorkRow ? 22 : 0) + (showsInsomniaLine ? 22 : 0)
            + ((showingHistory || !viewingToday) && !inTemporaryChat ? 36 : 0)
            + (promptError == nil ? 0 : 48) + attachmentStripHeight + (introLine.map { introContentHeight($0, width: bodyWidth - 50, inset: IntroNarrationView.chatInset) + 9 } ?? 0)
    }
    var chatTranscriptHeight: CGFloat {
        // The window's transcript fills what its body leaves after the chat's own rows (APP-745).
        if chatInWindow, let size = windowChatSize { return max(120, size.height - conversationChromeHeight) }
        return min(max(260, preferredTranscriptHeight), max(0, availableSurfaceSize.height - cutoutHeight - conversationChromeHeight))
    }

    func resizePanel(width: CGFloat, transcriptHeight: CGFloat, persist: Bool = false) {
        preferredPanelWidth = max(360, width)
        preferredTranscriptHeight = max(260, transcriptHeight)
        if persist {
            preferences.set(Double(preferredPanelWidth), forKey: "notch.panelWidth")
            preferences.set(Double(preferredTranscriptHeight), forKey: "notch.transcriptHeight")
        }
        onBackgroundChange?(false)
    }

    func setHoverEnabled(_ enabled: Bool) {
        preferences.set(enabled, forKey: "notch.hoverEnabled")
    }

    func noteTaskActivity(taskID: String, title: String, symbol: String = "ellipsis") {
        guard !taskID.isEmpty else { return }
        var activity = taskActivities[taskID] ?? []
        guard activity.last?.title != title else { return }
        activity.append(TaskActivity(title: title, symbol: symbol))
        taskActivities[taskID] = activity
    }

    /// The notch can handle this one explicit local preference command without sending it to the daemon.
    /// Pasted text is never interpreted as a command.
    func handleLocalNotchCommand(_ text: String) -> Bool {
        guard pasteProvenance.pastes.isEmpty, pasteProvenance.held.isEmpty, !pasteProvenance.isUnclear else { return false }
        let normalized = text.lowercased().split(whereSeparator: { !$0.isLetter && !$0.isNumber }).joined(separator: " ")
        let prefixes = ["", "ferb ", "please ", "can you ", "could you ", "ferb can you ",
                        "ferb could you ", "please ferb "]
        let enabledCommands = ["turn hover back on", "turn hover on", "turn hover to open on", "enable hover opening", "enable hover to open"]
        let disabledCommands = ["turn hover off", "turn hover to open off", "disable hover opening", "disable hover to open"]
        let enabled = prefixes.contains { prefix in enabledCommands.contains { normalized == prefix + $0 } }
        let disabled = prefixes.contains { prefix in disabledCommands.contains { normalized == prefix + $0 } }
        guard enabled || disabled else { return false }
        setHoverEnabled(enabled)
        localSettingReply = enabled ? "Hover opening is on." : "Hover opening is off."
        session.draft = ""
        pasteProvenance = PasteProvenance()
        onInteraction?()
        onBackgroundChange?(true)
        return true
    }

    var bodyHeight: CGFloat { min(requestedBodyHeight, max(0, availableSurfaceSize.height - cutoutHeight)) }
    private var requestedBodyHeight: CGFloat {
        // APP-783: Tasks is exactly the chat's height, so opening it from "View tasks" never shrinks the notch out from
        // under the pointer (a hover-out closed it)
        if showingTasks { return chatTranscriptHeight + conversationChromeHeight }
        if showingSources { return 250 }
        if session.phase != .listening && !showingActivity {
            // A day whose first page is on its way keeps the chat's height and shows the loader in it, so the chat never
            // opens short and grows (owner 2026-11-09: "on opeening a new chat its height get short the first time")
            let empty = !showsTranscript && !shownDayLoading
            return (empty ? min(31, chatTranscriptHeight) : chatTranscriptHeight) + conversationChromeHeight
        }
        if showingActivity { return 204 }
        switch session.phase {
        case .ready, .draft: return composerHeight + 26 + 31 + attachmentStripHeight
        case .listening: return 124
        case .working: return 118
        case .result: return 220
        case .stopped, .attention: return session.result == nil ? 116 : 220
        }
    }
    /// The drawn surface: header, plus the body when it shows. The panel adds a transparent margin around it.
    var surfaceSize: CGSize {
        if !expanded, let height = introSurfaceHeight(introLine) { return CGSize(width: bodyWidth, height: height) }
        if compactPlanText != nil && !bodyVisible { return CGSize(width: bodyWidth, height: compactPlanHeight) }
        if introPulsing && !expanded { return introPulseSize }
        return CGSize(width: expanded ? bodyWidth : handleWidth, height: cutoutHeight + (bodyVisible ? bodyHeight : 0))
    }
    var panelSize: CGSize {
        if !expanded, let height = introSurfaceHeight(introLine) { return CGSize(width: bodyWidth + 16, height: height + 12) }
        if compactPlanText != nil && !bodyVisible { return CGSize(width: bodyWidth + 16, height: compactPlanHeight + 12) }
        if introPulsing && !expanded { return CGSize(width: introPulseSize.width + 16, height: introPulseSize.height + 12) }
        if expanded { return CGSize(width: bodyWidth + 16, height: cutoutHeight + (bodyVisible ? bodyHeight : 0) + 12) }
        // The independent, mouse-transparent tracer owns its own glow margin.
        return CGSize(width: handleWidth, height: cutoutHeight)
    }

    /// `focus: false`: opened by FERB itself (the intro), never the owner: the chat shows without taking keyboard focus
    /// (rule 10); the panel becomes key only on the owner's click or shortcut.
    func expand(keyboard: Bool = false, focus: Bool = true) {
        if !expanded { openedByPress = false }
        session.expand()
        acknowledgement.dismiss()
        if focus { onPresentationChange?(!keyboard); ownerOpened() } else { onBackgroundChange?(true) }
    }

    /// The owner opened FERB himself (his click, shortcut or drag, or History): the daemon's first open of the day shows
    /// what waits for him to check (MEM-485). FERB opening the notch by itself (a question, an answer, the intro) is not
    /// his open.
    func ownerOpened() { memoryReview.opened() }

    /// The day's note's Review (MEM-485): the review list, the Memory page's To check.
    func openReview() { onOpenMemory?(nil) }

    func collapse(keyboard: Bool = false) {
        acknowledgement.dismiss()
        dismissPreviewPopovers()
        openedByPress = false
        heldVoiceSnapshot = nil
        drops.reset()
        microphone.cancel()
        // Hiding cancels microphone capture; committed work keeps running.
        if session.phase == .listening {
            voiceIsHeld = false
            session.finishVoicePreview()
        }
        acknowledgedIndicatorSignal = indicatorSignal
        session.collapse()
        showingSources = false
        showingHistory = false
        showingActivity = false
        showingTasks = false
        onPresentationChange?(!keyboard)
        endYield(.activeAppUnderTourLine)
    }

    var onPreviewWindowChange: ((UUID, NSWindow, Bool) -> Void)?
    private var previewPopoverIDs: Set<UUID> = []
    var previewPopoverCount: Int { previewPopoverIDs.count }
    @Published private(set) var previewDismissalRevision = 0

    func setPreviewPopover(_ id: UUID, presented: Bool) {
        if presented { previewPopoverIDs.insert(id) }
        else if previewPopoverIDs.remove(id) != nil { onInteractionEnded?() }
    }

    func dismissPreviewPopovers() {
        previewDismissalRevision &+= 1
        enlargedPicture = nil
        focusBeforeEnlarging = nil
    }

    /// APP-891 (owner 2026-11-11: "shoudl we show in bothb places???"): a preview is shown once. The running rows of a
    /// multi-part card whose live previews are on screen now, by their root task: while any is, the active-task strip
    /// shows no picture of that task (its count and time stay); scrolled away or not showing, the strip shows it again.
    @Published private(set) var rowPreviewsOnScreen: [String: Set<UUID>] = [:]

    func rowPreview(_ id: UUID, task: String, onScreen: Bool) {
        var shown = rowPreviewsOnScreen[task] ?? []
        guard onScreen ? shown.insert(id).inserted : shown.remove(id) != nil else { return }
        rowPreviewsOnScreen[task] = shown.isEmpty ? nil : shown
    }

    /// Whether a row of this task's card shows its live preview on screen now (APP-891).
    func rowsShowPreview(of task: String) -> Bool { !(rowPreviewsOnScreen[task]?.isEmpty ?? true) }

    /// APP-889: the one picture shown larger now (a payment page, a row's or a task's live preview), by its key. It
    /// closes by its ✕, a click on it, a click outside it, or Esc, which closes it before anything else Esc does; the
    /// keyboard focus goes back to the view that had it (the card's box, the chat field).
    @Published private(set) var enlargedPicture: String?
    private var focusBeforeEnlarging: (window: NSWindow, responder: NSResponder?)?

    func isEnlarged(_ key: String) -> Bool { enlargedPicture == key }

    /// `focus`: the window whose keyboard focus comes back on closing (the one with the keys now).
    func enlargePicture(_ key: String, focus: NSWindow? = NSApp.keyWindow) {
        guard enlargedPicture != key else { return }
        if enlargedPicture == nil, let window = focus {
            focusBeforeEnlarging = (window, window.firstResponder)
        }
        enlargedPicture = key
    }

    func togglePicture(_ key: String) {
        if enlargedPicture == key { closeEnlargedPicture() } else { enlargePicture(key) }
    }

    /// Closes the enlarged picture, if one shows; whether one did (Esc then does nothing else).
    @discardableResult
    func closeEnlargedPicture() -> Bool {
        guard enlargedPicture != nil else { return false }
        enlargedPicture = nil
        let focus = focusBeforeEnlarging
        focusBeforeEnlarging = nil
        if let focus {
            // after the popover has gone, so its window doesn't keep the keys
            DispatchQueue.main.async {
                if focus.window.isVisible, !focus.window.isKeyWindow { focus.window.makeKey() }
                if let responder = focus.responder, focus.window.firstResponder !== responder,
                   (responder as? NSView)?.window === focus.window {
                    focus.window.makeFirstResponder(responder)
                }
            }
        }
        return true
    }

    // A preview is a hover surface, not a modal interaction. The coordinator checks
    // its actual window bounds; merely being presented must never hold the chat open.
    var blocksAutomaticDismissal: Bool {
        chatInWindow || profileMenuOpen || filePickerOpen || session.phase == .listening || introLine != nil
    }

    func dismissAutomatically() {
        guard !blocksAutomaticDismissal else { return }
        collapse()
    }

    /// A press in another FERB window (Settings, Tasks…) closes only what shows (the open notch, a compact line, a
    /// reminder's arrival: APP-624 unchanged). A closed notch has nothing to close: closing it anyway reordered FERB's
    /// notch windows during the press, and the window server then refused that window's title-bar drag (APP-798).
    func pressedInAnotherFERBWindow() {
        if surfaceVisible { dismissAutomatically() }
    }

    /// Starts yielding for a reason. Yielding to an app under FERB's surface applies only while the tour's line shows
    /// (APP-642), and another app's windows never hide FERB while the owner is dictating.
    func beginYield(_ reason: NotchYieldReason) {
        if reason == .activeAppUnderTourLine && introLine == nil { return }
        if yielding.begin(reason, listening: session.phase == .listening) { onYieldChange?(true) }
    }

    func endYield(_ reason: NotchYieldReason) {
        if yielding.end(reason) { onYieldChange?(false) }
    }

    func setProfileMenuOpen(_ open: Bool) {
        profileMenuOpen = open
        if open { onInteraction?() }
        else { onInteractionEnded?() }
    }

    /// `byPress`: the owner pressed the closed notch or pill (APP-705) or double-tapped right Option (APP-709); the chat it
    /// opens stays open on hover-out.
    func toggle(byPress: Bool = false) {
        // The interaction callback can consume a compact arrival by opening chat. Decide before that callback.
        let close = expanded && compactPlanText == nil
        onInteraction?()
        if close { collapse(); return }
        expand()
        if byPress { openedByPress = true }
    }

    func edit(keyboard: Bool = false) {
        microphone.cancel()
        guard session.phase != .working else { expand(keyboard: keyboard); return }
        voiceIsHeld = false
        heldVoiceSnapshot = nil
        session.edit()
        showingSources = false
        showingHistory = false
        showingActivity = false
        focusRequest = UUID()
        onInteraction?()
        onPresentationChange?(!keyboard)
    }

    func startDictation() { beginVoice(held: false) }

    func beginHeldVoice() {
        guard canCompose else { return }
        heldVoiceSnapshot = (session, showingSources, showingHistory, showingActivity)
        beginVoice(held: true)
    }

    func cancelHeldVoice() {
        guard voiceIsHeld, let snapshot = heldVoiceSnapshot else { return }
        microphone.cancel()
        voiceIsHeld = false
        heldVoiceSnapshot = nil
        if let active = snapshot.session.activeTaskID {
            if let updated = workSessions[active] { session = updated }
            else { session = NotchSession() }
            session.draft = snapshot.session.draft
            if snapshot.session.isExpanded { session.expand() } else { session.collapse() }
        } else { session = snapshot.session }
        showingSources = snapshot.sources
        showingHistory = snapshot.history
        showingActivity = snapshot.activity
        onPresentationChange?(false)
    }

    private func beginVoice(held: Bool) {
        guard canCompose else { expand(keyboard: held); return }
        showingSources = false
        showingHistory = false
        showingActivity = false
        voiceIsHeld = held
        // Dictation keeps FERB on screen: other apps' windows stop hiding it. Set after voiceIsHeld so coming back
        // leaves the previous app's keyboard focus alone during the held chord.
        if yielding.endWindowReasons() { onYieldChange?(false) }
        if let active = session.activeTaskID {
            workSessions[active] = session
            var composer = NotchSession()
            composer.draft = session.draft
            composer.attach(session.attachments)
            session = composer
        }
        session.beginVoicePreview()
        voiceMeter.reset()
        feedback.play(.listeningStart)
        if !held { onInteraction?() }
        onPresentationChange?(!held)
        voiceDraft = session.draft
        voiceTranscript = ""
        voiceFinishing = false
        promptError = nil
        microphone.start(update: { [weak self] text in
            self?.voiceTranscript = text
        }, level: { [weak self] rms, duration in
            self?.voiceMeter.add(rms, each: duration)
        }, completion: { [weak self] result in
            guard let self, self.session.phase == .listening else { return }
            self.voiceIsHeld = false
            self.voiceFinishing = false
            self.heldVoiceSnapshot = nil
            self.session.finishVoicePreview()
            var send = false
            switch result {
            case .success(let text):
                self.session.draft = self.voiceDraft + (self.voiceDraft.isEmpty || text.isEmpty ? "" : " ") + text
                if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    send = held && self.preferences.bool(forKey: Self.sendOnReleaseKey)
                }
            case .failure(let error): self.promptError = error.localizedDescription
            }
            self.onPresentationChange?(true)
            self.focusRequest = UUID()
            if send { self.startWork() }
        })
    }

    func finishHeldVoice() {
        guard voiceIsHeld else { return }
        finishVoice()
    }

    func finishVoice() {
        guard session.phase == .listening else { return }
        guard !voiceFinishing else { return }
        voiceFinishing = true
        voiceIsHeld = false
        feedback.play(.listeningEnd)
        microphone.finish()
    }

    /// Sends the draft. An open question never blocks this (spec §4.7): a typed message can answer it or be new
    /// work. Pasted parts go marked; the daemon counts them as the owner's words (ORCH-518).
    func startWork() {
        guard canCompose else { return }
        if introHoldsWork {
            promptError = Self.introHeldWork
            onBackgroundChange?(true)
            return
        }
        guard canSend else { return }
        if !receivingFiles.isEmpty {                    // a dropped file still arriving would miss the message
            promptError = Self.stillReceiving
            onBackgroundChange?(true)
            return
        }
        // Held pastes go back in at their places, marked as pastes like any other (APP-935)
        let whole = pasteProvenance.expanded(session.draft)
        let outgoing = PastedRequest(text: whole.text, provenance: whole.provenance).markedText
        if handleLocalNotchCommand(session.draft) { return }
        // The bubble shows the words without leaked paste markers until the ack brings the daemon's text (APP-713)
        begin(request: PasteMarkers.removed(from: whole.text), outgoing: outgoing, sources: session.attachments,
              keepComposer: false)
    }

    /// A message of the owner's that isn't the composer's (the suggestion they picked in the tour's Try beat): sent
    /// the same way as a typed one, through `chat.send` (APP-666), while whatever they typed stays in the composer.
    /// Opened without taking focus (rule 10). While dictation holds the composer it waits, and is sent once the
    /// composer is free; `started` gets its turn's id then (nil when it couldn't start).
    func send(_ text: String, started: ((UUID?) -> Void)? = nil) {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { started?(nil); return }
        whenComposerFree { [weak self] in
            guard let self else { started?(nil); return }
            let id = self.begin(request: text, outgoing: text, sources: [], keepComposer: true)
            started?(id)                // computed first: an optional call skips evaluating its argument
        }
    }

    /// Stop exactly this turn's task (the tour's Stop on the task it started), through the same `task.stop` as the
    /// task list's Stop task; a turn whose task id hasn't come yet is stopped as soon as it does.
    func stopTurn(_ id: UUID) {
        // A task waiting on the owner's answer is still running and stops the same way (TaskItem.isRunning).
        guard let turn = shownArchive.turns.first(where: { $0.id == id }), TaskItem.isRunning(turn.status) else { return }
        guard let backend = turn.backendTaskID, !backend.isEmpty else { pendingStops.insert(id); return }
        guard !stoppingTaskIDs.contains(backend) else { return }
        stoppingTaskIDs.insert(backend)
        Task {
            do { _ = try await rpc.call("task.stop", params: ["task_ids": [backend]]) }
            catch { promptError = error.localizedDescription }
            stoppingTaskIDs.remove(backend)
            await refreshDaemonStatus()
        }
    }

    /// A selected memory is reference text, including imported memories. Prepare an
    /// editable correction without sending or replacing an existing composer draft.
    func prepareMemoryCorrection(_ memory: String) {
        whenComposerFree { [weak self] in
            guard let self else { return }
            let prefix = (self.session.draft.isEmpty ? "" : "\n\n") + "Help me correct this memory:\n"
            for (text, reference) in [(prefix, false), (memory, true), ("\n\nIt should say: ", false)] {
                let insertion = NSRange(location: self.session.draft.utf16.count, length: 0)
                self.pasteProvenance.edit(insertion, with: text, in: self.session.draft, pasted: reference)
                self.session.draft += text
            }
        }
    }

    /// What waits for dictation to let go of the composer, in order.
    private var waitingForComposer: [() -> Void] = []

    private func whenComposerFree(_ act: @escaping () -> Void) {
        if canCompose { act() } else { waitingForComposer.append(act) }
    }

    /// Run after the change that freed the composer has finished (dictation's own completion sets the draft first).
    private func scheduleWaitingForComposer() {
        Task { @MainActor [weak self] in
            guard let self, self.canCompose else { return }
            let waiting = self.waitingForComposer
            self.waitingForComposer = []
            for act in waiting { act() }
        }
    }

    /// A message of the owner's, sent into the chat that shows (the composer's, `composerSource`): while the temporary
    /// chat is on, it goes there and nothing of it to the saved chat; the same turn, session and stream either way.
    @discardableResult
    private func begin(request: String, outgoing: String, sources: [URL], keepComposer: Bool) -> UUID? {
        let source = composerSource
        return inChat(source, otherwise: nil) {
            begin(request: request, outgoing: outgoing, sources: sources, keepComposer: keepComposer, in: source)
        }
    }

    private func begin(request: String, outgoing: String, sources given: [URL], keepComposer: Bool,
                       in source: ChatSource) -> UUID? {
        // decision: ORCH-717 — FERB's own received files (a pasted picture, a dropped promise) sent in the temporary chat
        // are kept in its own folder, which its end removes; the owner's own files are sent by path, as in the chat
        let sources = source == .temporary ? receivedFiles.sentToTemporary(given) : given
        localSettingReply = nil
        let held = (draft: session.draft, sources: session.attachments)
        if let active = session.activeTaskID { workSessions[active] = session }
        var next = NotchSession()
        next.draft = request
        next.attach(sources)
        next.expand()
        session = next
        guard let id = session.beginWork() else { return nil }
        if source == .temporary { temporary.turns.insert(id) }
        followsToday = true
        selectedDay = Calendar.current.startOfDay(for: Date())
        promptError = nil
        // The sent message joins its conversation (kept only for the saved chat); an unsent draft is only ever in
        // memory (L20).
        conversation.begin(id: id, request: session.draft, sources: session.attachments, unredacted: true)
        session.clearComposer()
        if keepComposer {
            session.draft = held.draft
            session.attach(held.sources)
        } else {
            pasteProvenance = PasteProvenance()
            shownPaste = nil
        }
        receivedFiles.sent(sources)            // a pasted picture sent stays for its task (APP-933)
        workSessions[id] = session
        saveConversation()
        completionTask?.cancel()
        completionTraceVisible = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        if keepComposer {
            onBackgroundChange?(true)
        } else {
            onInteraction?()
            onPresentationChange?(true)
        }
        submitToDaemon(localID: id, text: outgoing, sources: sources)
        return id
    }

    func stopWork() { requestDaemonStop() }

    func requireAttention(_ message: String) {
        guard session.phase == .working else { return }
        session.requireAttention(message)
        feedback.play(.error)
        onPresentationChange?(true)
    }

    /// Follows a draft change the composer didn't report as an edit (dictation, a restored draft).
    func preservePasteProvenance() {
        pasteProvenance.follow(session.draft)
    }

    func attach(_ urls: [URL]) {
        guard canCompose else { return }
        session.attach(urls)
        session.edit()
        showingSources = false
        showingHistory = false
        showingActivity = false
        onInteraction?()
        onPresentationChange?(true)
    }

    func setFilePickerOpen(_ open: Bool) {
        guard filePickerOpen != open else { return }
        filePickerOpen = open
        if open { beginYield(.filePicker) } else { endYield(.filePicker) }
    }

    func chooseFiles() {
        guard !filePickerOpen else { return }        // the panel stays clickable behind its picker: one picker
        setFilePickerOpen(true)
        let picker = NSOpenPanel()
        picker.allowsMultipleSelection = true
        picker.canChooseDirectories = true
        picker.prompt = "Hand to FERB"
        picker.message = "Selected paths are handed to FERB with your request. FERB may read these files."
        onInteraction?()
        AppWindowPresentation.opening("the file picker") { picker.begin { [weak self] response in
            Task { @MainActor in
                guard let self else { return }
                self.setFilePickerOpen(false)
                if response == .OK { self.attach(picker.urls) }
                self.onInteractionEnded?()
            }
        } }
    }

    /// FERB takes dropped files now: not during the tour (D18; its controller sets `onIntroAction` while it runs) or
    /// while dictation holds the composer.
    var acceptsDroppedFiles: Bool { canCompose && onIntroAction == nil }

    /// The composer has something to send: words, or attached files alone (owner, 2026-11-05: files wait "until i send
    /// them manully or attach a message in text and send").
    var canSend: Bool {
        !session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !session.attachments.isEmpty
            || heldPastes.contains { !$0.text.isEmpty }         // a chip emptied in its open editor carries nothing
    }

    /// A picture pasted with no file behind it (a screenshot copied to the clipboard, a browser's Copy Image): written
    /// as a PNG FERB owns, then attached exactly as a dropped file (APP-933). False when FERB can't take files now or
    /// the file couldn't be written.
    @discardableResult
    func pasteImage(_ png: Data) -> Bool {
        guard acceptsDroppedFiles else { return false }
        let url: URL
        do {
            url = try receivedFiles.save(png, named: ComposerPaste.imageName(at: now()))
        } catch {
            promptError = "FERB couldn't attach the pasted picture. Try saving it as a file and attaching that."
            onBackgroundChange?(true)
            return true                                 // the clipboard held a picture: no text goes in instead
        }
        guard dropFiles([url]) else { receivedFiles.discard(url); return false }
        return true
    }

    /// A long paste held as a chip at `location` of the draft (APP-935): the field keeps only what the owner types.
    func holdPaste(_ text: String, at location: Int) {
        guard canCompose else { return }
        objectWillChange.send()
        pasteProvenance.hold(text, at: location, in: session.draft)
        session.edit()
        onInteraction?()
        onBackgroundChange?(true)
    }

    /// The chip's cross: its text is not sent.
    func removeHeldPaste(_ id: UUID) {
        objectWillChange.send()
        pasteProvenance.removeHeld(id)
        if shownPaste == id { show(nil) } else { onBackgroundChange?(true) }
    }

    /// Clicking a chip shows its whole text in the composer, in an editor (APP-1143); clicking it again hides it, as
    /// Done and Escape do (`closeShownPaste`).
    func toggleShownPaste(_ id: UUID) {
        show(shownPaste == id ? nil : id)
    }

    /// Done, or Escape while a chip's editor is open: the editor closes and the edits stay. False when none is open,
    /// so Escape goes on to what it does otherwise.
    @discardableResult
    func closeShownPaste() -> Bool {
        guard shownPaste != nil else { return false }
        show(nil)
        return true
    }

    /// The owner's edit in the open chip's editor: exactly what is sent from now on; its size line follows.
    func editHeldPaste(_ id: UUID, text: String) {
        guard heldPastes.contains(where: { $0.id == id && $0.text != text }) else { return }
        objectWillChange.send()
        pasteProvenance.editHeld(id, to: text)
        onInteraction?()
    }

    /// One way the shown chip changes: a chip left emptied is taken out as its cross would (APP-1144), and the field
    /// the owner types into gets the keyboard (the composer once no editor is open).
    private func show(_ id: UUID?) {
        if let leaving = shownPaste, leaving != id, heldPastes.contains(where: { $0.id == leaving && $0.text.isEmpty }) {
            objectWillChange.send()
            pasteProvenance.removeHeld(leaving)
        }
        shownPaste = id
        focusRequest = UUID()
        onBackgroundChange?(true)
    }

    /// Files and folders dropped on FERB (D17): all of them join the composer at once (deduplicated, folders kept) and
    /// the chat opens as a press opens it (APP-705/707: back from a yield, kept open on hover-out) with the composer
    /// focused. Nothing is sent: they wait until the owner sends, with or without words, whether or not a task runs
    /// (APP-577). One presentation change.
    @discardableResult
    func dropFiles(_ urls: [URL]) -> Bool {
        let files = urls.filter(\.isFileURL)
        guard acceptsDroppedFiles, !files.isEmpty else { return false }
        openForDrop(attaching: files)
        return true
    }

    /// Everything one drop carries (APP-930, APP-936): files attach at once; a picture with no file is written as a PNG
    /// FERB owns and attached; each promised file shows as arriving and is attached when its source has written it. An
    /// item that can't be read or received is said in a plain line, never dropped silently. The chat opens as for files.
    @discardableResult
    func drop(_ items: DroppedItems) -> Bool {
        guard acceptsDroppedFiles, !items.isEmpty else { return false }
        var files = items.files.filter(\.isFileURL)
        var failed = items.unreadable > 0
        for png in items.images {
            if let url = try? receivedFiles.save(png, named: ComposerPaste.imageName(at: now())) {
                files.append(url)
            } else {
                failed = true
            }
        }
        for promise in items.promises { receive(promise) }
        openForDrop(attaching: files)
        if failed { promptError = Self.receiveFailed }
        return true
    }

    private func openForDrop(attaching files: [URL]) {
        onInteraction?()                  // the press path: stops a yield, un-rests the pill, makes the open panel key
        acknowledgement.dismiss()
        if !viewingToday {                // the composer is today's
            followsToday = true
            selectedDay = Calendar.current.startOfDay(for: Date())
        }
        session.attach(files)
        session.edit()
        session.expand()
        ownerOpened()
        showingSources = false
        showingHistory = false
        showingActivity = false
        showingTasks = false
        openedByPress = true
        onPresentationChange?(true)
    }

    static let receiveFailed = "FERB couldn't get a file you dropped. Try saving it first, then drop the saved file."
    static let stillReceiving = "FERB is still getting a file you dropped. Send again in a moment."

    /// A promised file asked of its source into a folder of its own; it shows as arriving until it is written.
    private func receive(_ promise: PromisedFile) {
        let arriving = ReceivingFile(id: UUID(), name: promise.name)
        let folder: URL
        do { folder = try receivedFiles.newFolder() } catch {
            promptError = Self.receiveFailed
            return
        }
        receivingFiles.append(arriving)
        promise.receive(into: folder) { [weak self] result in
            guard let self else { return }
            let cancelled = self.cancelledReceives.contains(arriving.id)
            self.receivingFiles.removeAll { $0.id == arriving.id }
            switch result {
            case .success(let url):
                self.receivedFiles.adopt(url)
                if cancelled {
                    self.receivedFiles.discard(url)
                } else {
                    self.session.attach([url])
                    self.session.edit()
                }
            case .failure:
                self.receivedFiles.removeIfEmpty(folder)
                if !cancelled { self.promptError = Self.receiveFailed }
            }
            self.onBackgroundChange?(true)
        }
    }

    /// The arriving card's cross: the file is not attached, and deleted if it still comes.
    func cancelReceiving(_ id: UUID) {
        guard receivingFiles.contains(where: { $0.id == id }) else { return }
        cancelledReceives.insert(id)
        receivingFiles.removeAll { $0.id == id }
        onBackgroundChange?(true)
    }

    func toggleSources() {
        showingActivity = false
        showingSources.toggle()
        onInteraction?()
        onPresentationChange?(true)
    }

    func history() {
        showingActivity = false
        showingHistory = true
        showingSources = false
        session.expand()
        ownerOpened()
        onInteraction?()
        onPresentationChange?(true)
    }

    func toggleActivity() {
        showingActivity.toggle()
        showingHistory = false
        showingSources = false
        onInteraction?()
        onPresentationChange?(true)
    }

    func recall(_ receipt: WorkReceipt) {
        session.recall(receipt)
        showingHistory = false
        showingActivity = false
        completionTask?.cancel()
        completionTraceVisible = false
        onPresentationChange?(true)
    }

    func composerChanged() {
        // Update the shared surface geometry when the field wraps or adds a line. Geometry only: a presentation that
        // requests focus here would move it while the owner types.
        if expanded && canCompose { onBackgroundChange?(true) }
    }

    /// The owner chose to answer this question (clicked it, or Answer request in Tasks): focus its field.
    func focusQuestion(_ id: String) {
        guard pendingApprovals.contains(where: { $0.id == id }) else { return }
        if !viewingToday { selectDay(Date()) }
        questionFocus = QuestionFocus(id: id)
    }

    /// A notification was clicked (L34 D): the notch opens at its item — the question's field, the task in Tasks, or
    /// today's chat for a reminder.
    func open(_ target: NotificationTarget) {
        if case .handoff(let id) = target {          // the page is the place to act, not the notch
            Task { _ = try? await rpc.call("browser.show_handoff", params: ["handoff_id": id]) }
            return
        }
        if !viewingToday { selectDay(Date()) }
        expand()
        switch target {
        case .question(let id, _):
            focusQuestion(id)
        case .task(let id):
            showingTasks = taskItems.contains { $0.id == id }
            onPresentationChange?(true)
        case .reminder, .handoff, .notch:
            break
        }
    }

    func removeAttachment(_ url: URL) {
        session.removeAttachment(url)
        receivedFiles.discard(url)              // a pasted picture never sent is deleted (APP-933)
        onPresentationChange?(true)
    }

    func openDocument() {
        withDocument { [weak self] url in self?.onOpenDocument?(url) }
    }

    func quickLook() {
        onInteraction?()
        withDocument { [weak self] url in self?.onQuickLook?(url) }
    }

    func revealDocument() {
        withDocument { revealInFinder([$0]) }
    }

    /// A file card's click: the file shown in Finder, selected. False when it is gone (the card says so).
    @discardableResult
    func revealFile(_ url: URL) -> Bool {
        guard url.isFileURL, FileManager.default.fileExists(atPath: url.path) else { return false }
        revealInFinder([url])
        return true
    }

    /// A site's click: its https page in the default browser, only for a plain host (VisitedSites.link).
    @discardableResult
    func openSite(_ host: String) -> Bool {
        guard let link = VisitedSites.link(host) else { return false }
        openURL(link)
        return true
    }

    func copyText(_ text: String) {
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    private func withDocument(_ action: (URL) -> Void) {
        guard let url = session.result?.documentURL else { return }
        guard FileManager.default.fileExists(atPath: url.path) else {
            promptError = "This artifact is no longer available at its recorded path."
            return
        }
        action(url)
    }

    func announceApproval(_ approval: DaemonApproval) {
        let key = "approval:" + approval.id
        guard !announcedAttentionIDs.contains(key) else { return }
        // Questions are shown inline in today's chat; arriving never moves keyboard focus.
        followsToday = true
        selectedDay = Calendar.current.startOfDay(for: Date())
        // The question comes forward: its row when it belongs to one, else its card at the end (APP-720), in the chat
        // it belongs to.
        let source: ChatSource = isTemporaryApproval(approval) ? .temporary : .saved
        let target = inChat(source, otherwise: nil) { conversation.rowPlacements([approval])[approval.id] }
            .map { TranscriptTarget.row(turn: $0.turnID, part: $0.part) } ?? TranscriptTarget.question(approval.id)
        requestScroll(target, .needsYou)
        announceAttention(key, item: approval.id)
    }

    /// Something waits on the owner (a question, an approval, FERB's own question): the notch opens and needs you
    /// plays. `item` is what a banner of it would name (an approval's id), so the two sound once.
    func announceAttention(_ identity: String, item: String? = nil) {
        guard announcedAttentionIDs.insert(identity).inserted else { return }
        preferences.set(Array(announcedAttentionIDs), forKey: "FERBAnnouncedAttentionIDs")
        showingTasks = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        session.expand()
        acknowledgement.dismiss()
        attentionStartedAt = Date()
        attentionTraceVisible = true
        attentionTask?.cancel()
        attentionTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(CompletionGlow.duration)) } catch { return }
            self?.attentionTraceVisible = false
            self?.onStatusChange?()
        }
        feedback.play(.needsYou, for: item ?? identity)
        onAttentionArrival?()
        onBackgroundChange?(true)
        onStatusChange?()
    }

    /// Typed reminder arrivals share the existing compact surface and spring. The full chat stays as the owner left it.
    func presentReminder(card: ResultCard, text: String) {
        // In window mode a reminder brings the window forward at today's chat, without taking focus (D22).
        if chatInWindow, introLine == nil {
            onAttentionArrival?()
            followsToday = true
            selectedDay = Date()
            showingTasks = false
            showingSources = false
            showingHistory = false
            showingActivity = false
            session.expand()
            onBackgroundChange?(true)
            return
        }
        guard !bodyVisible, introLine == nil else { return }
        onAttentionArrival?()
        acknowledgement.show(text, reminder: card)
    }

    func openReminderChat() {
        followsToday = true
        selectedDay = Date()
        showingTasks = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        expand()
    }

    /// A scheduled result has a typed reminder card. Other task results retain their normal full-chat arrival.
    func presentResult(_ event: [String: Any]) {
        if let card = CardLayout.above(ResultCard.list(event["cards"]) ?? []).first {
            presentReminder(card: card, text: event["display_text"] as? String
                ?? event["answer"] as? String ?? event["summary"] as? String ?? event["text"] as? String ?? card.title ?? "Reminder")
        } else {
            presentTerminalAnswer()
        }
    }

    /// Fresh terminal results reveal the chat independently of the temporary glow, without taking focus.
    func presentTerminalAnswer() {
        showingTasks = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        session.expand()
        acknowledgement.dismiss()
        attentionStartedAt = Date()
        followsToday = true
        selectedDay = Date()
        onAttentionArrival?()
        onBackgroundChange?(true)
        onStatusChange?()
    }

    /// The first streamed answer chunk is visible attention, but keeps the owner's active app and keyboard focus.
    func presentIncomingAnswer() {
        showingTasks = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        session.expand()
        acknowledgement.dismiss()
        attentionStartedAt = Date()
        attentionTraceVisible = true
        attentionTask?.cancel()
        attentionTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(CompletionGlow.duration)) } catch { return }
            self?.attentionTraceVisible = false
            self?.onStatusChange?()
        }
        onAttentionArrival?()
        onBackgroundChange?(true)
        onStatusChange?()
    }

    func showCompletionTrace() {
        completionStartedAt = Date()
        completionTraceVisible = true
        completionTask?.cancel()
        completionTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(CompletionGlow.duration)) } catch { return }
            self?.completionTraceVisible = false
            self?.onStatusChange?()
        }
    }

}

/// A promised file on its way into the composer (APP-936).
struct ReceivingFile: Identifiable, Equatable {
    let id: UUID
    /// The source's name for it, when it gave one.
    let name: String?
}
