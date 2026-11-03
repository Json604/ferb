import AppKit
import SwiftUI
import UniformTypeIdentifiers
import NotchCore

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
    @Published var isNotchDocked = false
    var workSessions: [UUID: NotchSession] = [:] { didSet { taskListCache.invalidate() } }
    var taskListCache = TaskListCache()
    var submissions: [UUID: Task<Void, Never>] = [:]
    @Published var pendingStops = Set<UUID>()
    @Published var conversation = ConversationArchive() { didSet { taskListCache.invalidate() } }
    @Published var followsToday = true
    @Published var selectedDay = Calendar.current.startOfDay(for: Date())
    @Published var handleWidth: CGFloat = 180
    @Published var cutoutHeight: CGFloat = 40
    @Published var hasHardwareNotch = true
    @Published var isDropTarget = false
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
    @Published var voiceStartedAt = Date()
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
    @Published private(set) var isPinned = false
    @Published private(set) var isHeaderOnly = false

    let feedback: SoundFeedback
    var cursor: CursorController?
    let microphone: VoiceCapturing
    @Published var voiceTranscript = ""
    @Published var voiceFinishing = false
    private var voiceDraft = ""
    /// The words the last dictation added: a draft still holding them is sent as a voice turn, answered aloud.
    private var lastDictation: String?
    /// Settings → Notch & Shortcuts: a held dictation sends its draft when the keys are released.
    static let sendOnReleaseKey = "voice.sendOnRelease"
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

    var menuBarState: MenuBarState {
        MenuBarState.derive(link: daemonLink.state, paused: daemonPaused, stopped: daemonStopped,
                            questions: pendingApprovals.count, running: runningTasks.count)
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
    /// Opens a report file the owner asked to view; tests replace it.
    var openReportFile: (URL) -> Void = { NSWorkspace.shared.open($0) }

    init(rpc: DaemonRPCTransport = DaemonRPC(), preferences: UserDefaults = .standard, microphone: VoiceCapturing? = nil) {
        self.microphone = microphone ?? Microphone()
        self.rpc = rpc
        self.clickThrough = ClickThroughReport(rpc: rpc)
        self.preferences = preferences
        feedback = SoundFeedback(defaults: preferences)
        preferredPanelWidth = max(360, CGFloat((preferences.object(forKey: "notch.panelWidth") as? NSNumber)?.doubleValue ?? 440))
        preferredTranscriptHeight = max(260, CGFloat((preferences.object(forKey: "notch.transcriptHeight") as? NSNumber)?.doubleValue ?? 300))
        if let data = preferences.data(forKey: "FERBConversationArchive") {
            if let saved = try? JSONDecoder().decode(ConversationArchive.self, from: data) {
                conversation = saved
                conversation.interruptRestoredDrafts()
                if conversation != saved, let repaired = try? JSONEncoder().encode(conversation.forStorage()) {
                    preferences.set(repaired, forKey: "FERBConversationArchive")
                }
            } else {
                // History this build can't read is never saved over: it is kept aside, unchanged, and a new
                // conversation archive starts.
                let aside = "FERBConversationArchive.unreadable-" + ISO8601DateFormatter().string(from: Date())
                preferences.set(data, forKey: aside)
                if preferences.data(forKey: aside) == data { preferences.removeObject(forKey: "FERBConversationArchive") }
                else { archiveBlocked = true }
            }
        }
        conversationID = preferences.string(forKey: "FERBConversationID")
        announcedAttentionIDs = Set(preferences.stringArray(forKey: "FERBAnnouncedAttentionIDs") ?? [])
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
    var onSettings: (() -> Void)?
    var onOpenWindow: (() -> Void)?
    var onStatusChange: (() -> Void)?
    var onInteractionEnded: (() -> Void)?
    var onInteraction: (() -> Void)?
    var onPinChange: ((Bool) -> Void)?
    var onDetachTaskActivity: ((String) -> Void)?
    private var completionTask: Task<Void, Never>?
    private var attentionTask: Task<Void, Never>?
    private var pendingPinnedFold = false
    private var profileMenuOpen = false
    private(set) var filePickerOpen = false
    /// Why FERB's windows are currently stepping aside for another surface (its own file picker, another app's
    /// menu over the notch, an app the owner is working in under the pinned chat). They live in an overlay space
    /// above every ordinary window, so nothing can be raised over them by window level.
    private(set) var yielding = NotchYield()
    /// Called when FERB starts (true) or stops (false) yielding; the coordinator makes its windows invisible and
    /// click-through for that duration. Draft, attachments, pin and conversation are untouched.
    var onYieldChange: ((Bool) -> Void)?
    private var heldVoiceSnapshot: (session: NotchSession, sources: Bool, history: Bool, activity: Bool, headerOnly: Bool)?

    @Published private(set) var compactPlanText: String?
    /// Retain the compact content while the same native surface closes; never expose transcript mid-collapse.
    @Published private(set) var compactPlanDisplayText: String?
    @Published private(set) var compactReminderCard: ResultCard?
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
    /// What the owner did on an intro line (a choice, a typed entry, Skip, Stop on the replayed preview); set by the
    /// intro controller while the tour runs. Set, the line also offers Skip.
    var onIntroAction: ((IntroAction) -> Void)?
    /// While the tour runs, a request typed or spoken stays in the composer instead of starting a task: the tour runs
    /// no real work (kb area app); it is sent after the tour.
    var introHoldsWork = false
    /// The running tour is at the permissions beat or later: its line offers Skip tour (APP-638).
    @Published var introSkippable = false
    /// The tour wants System Settings in front of it: the notch steps behind Settings' window when it lies under the
    /// tour's surface, as when Settings comes forward (APP-642).
    var onStepBehindSettings: (() -> Void)?
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
    var bodyVisible: Bool { expanded && !isHeaderOnly }
    var isYielded: Bool { self.yielding.isActive }
    @Published var availableSurfaceSize = CGSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    var bodyWidth: CGFloat { min(availableSurfaceSize.width, max(360, preferredPanelWidth)) }
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
        min(max(260, preferredTranscriptHeight), max(0, availableSurfaceSize.height - cutoutHeight - conversationChromeHeight))
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
        if showingTasks { return 320 }
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
        cancelPinnedFold()
        isHeaderOnly = false
        session.expand()
        acknowledgement.dismiss()
        if focus { onPresentationChange?(!keyboard) } else { onBackgroundChange?(true) }
    }

    func collapse(keyboard: Bool = false) {
        acknowledgement.dismiss()
        dismissPreviewPopovers()
        cancelPinnedFold()
        isPinned = false
        isHeaderOnly = false
        heldVoiceSnapshot = nil
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
        endYield(.activeAppUnderPinnedChat)
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
    // its actual window bounds; merely being presented must never pin chat open.
    var blocksAutomaticDismissal: Bool {
        isPinned || profileMenuOpen || filePickerOpen || session.phase == .listening || introLine != nil
    }

    func dismissAutomatically() {
        guard !blocksAutomaticDismissal else { return }
        collapse()
    }

    func togglePin() {
        isPinned.toggle()
        if isPinned { foldPinnedBody() }
        else { revealPinnedBody() }
        onPinChange?(isPinned)
        if !isPinned { endYield(.activeAppUnderPinnedChat) }
    }

    /// Starts yielding for a reason. Yielding to an app under the chat applies only to a pinned chat, and
    /// another app's windows never hide FERB while the owner is dictating.
    func beginYield(_ reason: NotchYieldReason) {
        if reason == .activeAppUnderPinnedChat && !(isPinned && expanded) && introLine == nil { return }
        if yielding.begin(reason, listening: session.phase == .listening) { onYieldChange?(true) }
    }

    func endYield(_ reason: NotchYieldReason) {
        if yielding.end(reason) { onYieldChange?(false) }
    }

    func cancelPinnedFold() {
        pendingPinnedFold = false
    }

    func foldPinnedBody() {
        guard isPinned, bodyVisible else { return }
        // Start immediately unless an active interaction needs the body. Hover
        // can cancel this request before the menu, picker or voice mode finishes.
        pendingPinnedFold = true
        guard !profileMenuOpen, !filePickerOpen, session.phase != .listening else { return }
        cancelPinnedFold()
        isHeaderOnly = true
        onPresentationChange?(true)
    }

    private func resumePendingPinnedFold() {
        if pendingPinnedFold { foldPinnedBody() }
    }

    func revealPinnedBody() {
        cancelPinnedFold()
        guard isHeaderOnly else { return }
        isHeaderOnly = false
        acknowledgement.dismiss()
        onPresentationChange?(true)
    }

    func setProfileMenuOpen(_ open: Bool) {
        profileMenuOpen = open
        if open { onInteraction?() }
        else { resumePendingPinnedFold(); onInteractionEnded?() }
    }

    func toggle() {
        // The interaction callback can consume a compact arrival by opening chat. Decide before that callback.
        let close = expanded && compactPlanText == nil
        onInteraction?()
        close ? collapse() : expand()
    }

    func edit(keyboard: Bool = false) {
        microphone.cancel()
        guard session.phase != .working else { expand(keyboard: keyboard); return }
        voiceIsHeld = false
        heldVoiceSnapshot = nil
        session.edit()
        isHeaderOnly = false
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
        heldVoiceSnapshot = (session, showingSources, showingHistory, showingActivity, isHeaderOnly)
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
        isHeaderOnly = snapshot.headerOnly
        onPresentationChange?(false)
        resumePendingPinnedFold()
    }

    private func beginVoice(held: Bool) {
        guard canCompose else { expand(keyboard: held); return }
        cancelPinnedFold()
        isHeaderOnly = false
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
        voiceStartedAt = Date()
        feedback.play(.listeningStart)
        if !held { onInteraction?() }
        onPresentationChange?(!held)
        voiceDraft = session.draft
        voiceTranscript = ""
        voiceFinishing = false
        promptError = nil
        microphone.start(update: { [weak self] text in
            self?.voiceTranscript = text
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
                    self.lastDictation = text
                    send = held && self.preferences.bool(forKey: Self.sendOnReleaseKey)
                }
            case .failure(let error): self.promptError = error.localizedDescription
            }
            self.onPresentationChange?(true)
            self.focusRequest = UUID()
            self.resumePendingPinnedFold()
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
        guard !session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        if handleLocalNotchCommand(session.draft) { return }
        let outgoing = PastedRequest(text: session.draft, provenance: pasteProvenance).markedText
        let draft = session.draft
        let spoken = lastDictation.map { draft.contains($0) } ?? false
        lastDictation = nil
        begin(request: draft, outgoing: outgoing, sources: session.attachments, spoken: spoken, keepComposer: false)
    }

    /// A message of the owner's that isn't the composer's (the intro's "Do it for real", which they tapped): sent the
    /// same way, while whatever they typed stays in the composer. Opened without taking focus (rule 10). While
    /// dictation holds the composer it waits, and is sent once the composer is free.
    func send(_ text: String) {
        guard !introHoldsWork, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        whenComposerFree { [weak self] in
            guard let self, !self.introHoldsWork else { return }
            self.begin(request: text, outgoing: text, sources: [], spoken: false, keepComposer: true)
        }
    }

    /// Text of the owner's for them to send themselves (the intro's "Do it for real" after a repair skip, while the
    /// service is down): put in the composer, never over what they typed (it goes after it, on a new line).
    func addToDraft(_ text: String) {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        whenComposerFree { [weak self] in
            guard let self else { return }
            let draft = self.session.draft
            self.session.draft = draft.isEmpty ? text : draft + (draft.hasSuffix("\n") ? "" : "\n") + text
            self.preservePasteProvenance()
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

    private func begin(request: String, outgoing: String, sources: [URL], spoken: Bool, keepComposer: Bool) {
        localSettingReply = nil
        let held = (draft: session.draft, sources: session.attachments)
        if let active = session.activeTaskID { workSessions[active] = session }
        var next = NotchSession()
        next.draft = request
        next.attach(sources)
        next.expand()
        session = next
        guard let id = session.beginWork() else { return }
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
        submitToDaemon(localID: id, text: outgoing, sources: sources, speak: spoken)
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
        isHeaderOnly = false
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
        cancelPinnedFold()
        let picker = NSOpenPanel()
        picker.allowsMultipleSelection = true
        picker.canChooseDirectories = true
        picker.prompt = "Hand to FERB"
        picker.message = "Selected paths are handed to FERB with your request. FERB may read these files."
        onInteraction?()
        picker.begin { [weak self] response in
            Task { @MainActor in
                guard let self else { return }
                self.setFilePickerOpen(false)
                if response == .OK { self.attach(picker.urls) }
                self.resumePendingPinnedFold()
                self.onInteractionEnded?()
            }
        }
    }

    func acceptDrop(_ providers: [NSItemProvider]) -> Bool {
        guard canCompose else { return false }
        for provider in providers where provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { [weak self] item, _ in
                let url: URL?
                if let data = item as? Data { url = URL(dataRepresentation: data, relativeTo: nil) }
                else { url = item as? URL }
                guard let url, url.isFileURL else { return }
                Task { @MainActor in self?.attach([url]) }
            }
        }
        return true
    }

    func toggleSources() {
        showingActivity = false
        showingSources.toggle()
        onInteraction?()
        onPresentationChange?(true)
    }

    func history() {
        isHeaderOnly = false
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
        isHeaderOnly = false
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
        withDocument { NSWorkspace.shared.activateFileViewerSelecting([$0]) }
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
        announceAttention(key, item: approval.id)
    }

    /// Something waits on the owner (a question, an approval, FERB's own question): the notch opens and needs you
    /// plays. `item` is what a banner of it would name (an approval's id), so the two sound once.
    func announceAttention(_ identity: String, item: String? = nil) {
        guard announcedAttentionIDs.insert(identity).inserted else { return }
        preferences.set(Array(announcedAttentionIDs), forKey: "FERBAnnouncedAttentionIDs")
        cancelPinnedFold()
        isHeaderOnly = false
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
        cancelPinnedFold()
        isHeaderOnly = false
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
        cancelPinnedFold()
        isHeaderOnly = false
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
