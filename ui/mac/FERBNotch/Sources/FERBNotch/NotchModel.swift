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

    @Published var measuredTranscriptHeight: CGFloat = 0
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
    @Published var conversation = ConversationArchive() { didSet { taskListCache.invalidate(); shownTranscript = nil } }
    /// The shown day's entries as last built (`visibleEntries`): the chat, the notch's size on every pointer move and
    /// frame, and the questions all read them, so they are built once per change of the conversation or the day.
    var shownTranscript: (day: Date, entries: [TranscriptEntry])?
    /// Where the chat is kept and what the daemon holds of it (APP-805).
    let chatStore = HistoryState()
    /// What is shown of each day (APP-806); changed only through `updateWindow`.
    var dayWindows: [Date: DayWindow] = [:]
    /// The days the daemon has chat for (History), read when History opens.
    @Published var historyDays: [Date] = []
    /// The transcript holds the view still just before earlier entries are inserted above it (APP-806).
    var beforeEarlierInsert: (() -> Void)?
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
    /// FERB's click-through windows, told to the daemon (D31 fix round 4)
    let clickThrough: ClickThroughReport
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
        didSet { if pendingApprovals != oldValue { taskListCache.invalidate() } }
    }
    @Published var questionFocus: QuestionFocus?
    @Published var promptError: String?
    @Published var sendingAnswer = false
    @Published var runningTasks: [String: String] = [:] {
        didSet {
            guard runningTasks != oldValue else { return }
            taskListCache.invalidate()
            // When each running task started, for the work row's timer (a friend's feedback 2026-10-27): kept for
            // tasks still running; one first seen here starts now unless the daemon's status says otherwise.
            var since = runningSince.filter { runningTasks[$0.key] != nil }
            for id in runningTasks.keys where since[id] == nil { since[id] = Date() }
            if since != runningSince { runningSince = since }
        }
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
        let active = Set(taskItems.filter(\.isRunning).map { $0.backendID ?? $0.id })
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
    var conversationID: String?
    var supportsAttachments = false
    var eventsTask: Task<Void, Never>?
    var reconciliationTask: Task<Void, Never>?
    var submissionTask: Task<Void, Never>?
    /// Which parts of the composer draft were pasted: marked when sent (the daemon counts them as the owner's words,
    /// ORCH-518) and never run as a local notch command (APP-596).
    var pasteProvenance = PasteProvenance()
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
        drops.accepts = { [weak self] in self?.acceptsDroppedFiles ?? false }
        drops.isOpen = { [weak self] in self?.bodyVisible ?? false }
        drops.onDrop = { [weak self] urls in self?.dropFiles(urls) ?? false }
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
    }

    var onPresentationChange: ((Bool) -> Void)?
    var onBackgroundChange: ((Bool) -> Void)?
    /// A notice's page card (step 12c): the FERB window's Memory page, filtered to an import's source when it names one.
    var onOpenMemory: ((String?) -> Void)?
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
    /// One horizontal card strip, independent of the number of attachments.
    var attachmentStripHeight: CGFloat { session.attachments.isEmpty ? 0 : 100 }
    var conversationChromeHeight: CGFloat {
        composerHeight + 58 + (showsWorkRow ? 22 : 0) + ((showingHistory || !viewingToday) ? 36 : 0)
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
        guard pasteProvenance.pastes.isEmpty, !pasteProvenance.isUnclear else { return false }
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
            return (visibleEntries.isEmpty && inlineQuestions.isEmpty ? min(31, chatTranscriptHeight) : chatTranscriptHeight) + conversationChromeHeight
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
        if focus { onPresentationChange?(!keyboard) } else { onBackgroundChange?(true) }
    }

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

    func dismissPreviewPopovers() { previewDismissalRevision &+= 1 }

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
        if handleLocalNotchCommand(session.draft) { return }
        let outgoing = PastedRequest(text: session.draft, provenance: pasteProvenance).markedText
        // The bubble shows the words without leaked paste markers until the ack brings the daemon's text (APP-713)
        begin(request: PasteMarkers.removed(from: session.draft), outgoing: outgoing, sources: session.attachments,
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
        guard let turn = conversation.turns.first(where: { $0.id == id }), TaskItem.isRunning(turn.status) else { return }
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

    @discardableResult
    private func begin(request: String, outgoing: String, sources: [URL], keepComposer: Bool) -> UUID? {
        localSettingReply = nil
        let held = (draft: session.draft, sources: session.attachments)
        if let active = session.activeTaskID { workSessions[active] = session }
        var next = NotchSession()
        next.draft = request
        next.attach(sources)
        next.expand()
        session = next
        guard let id = session.beginWork() else { return nil }
        followsToday = true
        selectedDay = Calendar.current.startOfDay(for: Date())
        promptError = nil
        // The sent message joins the saved conversation; an unsent draft is only ever in memory (L20).
        conversation.begin(id: id, request: session.draft, sources: session.attachments, unredacted: true)
        session.clearComposer()
        if keepComposer {
            session.draft = held.draft
            session.attach(held.sources)
        } else {
            pasteProvenance = PasteProvenance()
        }
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
    }

    /// Files and folders dropped on FERB (D17): all of them join the composer at once (deduplicated, folders kept) and
    /// the chat opens as a press opens it (APP-705/707: back from a yield, kept open on hover-out) with the composer
    /// focused. Nothing is sent: they wait until the owner sends, with or without words, whether or not a task runs
    /// (APP-577). One presentation change.
    @discardableResult
    func dropFiles(_ urls: [URL]) -> Bool {
        let files = urls.filter(\.isFileURL)
        guard acceptsDroppedFiles, !files.isEmpty else { return false }
        onInteraction?()                  // the press path: stops a yield, un-rests the pill, makes the open panel key
        acknowledgement.dismiss()
        if !viewingToday {                // the composer is today's
            followsToday = true
            selectedDay = Calendar.current.startOfDay(for: Date())
        }
        session.attach(files)
        session.edit()
        session.expand()
        showingSources = false
        showingHistory = false
        showingActivity = false
        showingTasks = false
        openedByPress = true
        onPresentationChange?(true)
        return true
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
        // The question comes forward: its row when it belongs to one, else its card at the end (APP-720).
        let target = conversation.rowPlacements([approval])[approval.id]
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
