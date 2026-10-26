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
    @Published var session = NotchSession() { didSet { taskListSessionChanged(from: oldValue) } }
    @Published var measuredTranscriptHeight: CGFloat = 0
    var workSessions: [UUID: NotchSession] = [:] { didSet { taskListCache.invalidate() } }
    var taskListCache = TaskListCache()
    var submissions: [UUID: Task<Void, Never>] = [:]
    @Published var pendingStops = Set<UUID>()
    @Published var conversation = ConversationArchive() { didSet { taskListCache.invalidate() } }
    @Published var followsToday = true
    @Published var selectedDay = Calendar.current.startOfDay(for: Date())
    @Published var handleWidth: CGFloat = 180
    @Published var cutoutHeight: CGFloat = 32
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
    var taskStateRevision = 0
    var reconciledMissingTasks = Set<String>()
    @Published private(set) var isPinned = false
    @Published private(set) var isHeaderOnly = false

    let feedback = SoundFeedback()
    var cursor: CursorController?
    let microphone: VoiceCapturing
    @Published var voiceTranscript = ""
    @Published var voiceFinishing = false
    private var voiceDraft = ""
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
    /// Which parts of the composer draft were pasted; they are sent as data, never as the owner's words (MP2).
    var pasteProvenance = PasteProvenance()
    var knownCompletedTasks = Set<String>()
    /// An unreadable archive couldn't be set aside, so nothing may be saved over it.
    var archiveBlocked = false
    /// Send to developer (L35): each answer's report, by turn id (live state, never saved), and the owner's Mail.
    @Published var reports: [UUID: ReportState] = [:]
    var reportMailer: ReportMailing = SystemReportMailer()

    init(rpc: DaemonRPCTransport = DaemonRPC(), preferences: UserDefaults = .standard, microphone: VoiceCapturing? = nil) {
        self.microphone = microphone ?? Microphone()
        self.rpc = rpc
        self.clickThrough = ClickThroughReport(rpc: rpc)
        self.preferences = preferences
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
    }

    var onPresentationChange: ((Bool) -> Void)?
    var onBackgroundChange: ((Bool) -> Void)?
    var onAttentionArrival: (() -> Void)?
    var onOpenDocument: ((URL) -> Void)?
    var onQuickLook: ((URL) -> Void)?
    var onSettings: (() -> Void)?
    var onStatusChange: (() -> Void)?
    var onInteractionEnded: (() -> Void)?
    var onInteraction: (() -> Void)?
    var onPinChange: ((Bool) -> Void)?
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

    var expanded: Bool { session.isExpanded }
    var bodyVisible: Bool { expanded && !isHeaderOnly }
    var isYielded: Bool { yielding.isActive }
    var bodyWidth: CGFloat { max(360, handleWidth + 164) }
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
    var chatTranscriptHeight: CGFloat { min(320, max(240, measuredTranscriptHeight)) }

    var bodyHeight: CGFloat {
        if showingTasks { return 320 }
        if showingSources { return 250 }
        if session.phase != .listening && !showingActivity {
            return (visibleEntries.isEmpty && inlineQuestions.isEmpty ? 31 : chatTranscriptHeight) + composerHeight + 58 + (showsWorkRow ? 22 : 0) + ((showingHistory || !viewingToday) ? 36 : 0) + (promptError == nil ? 0 : 48) + (session.attachments.isEmpty ? 0 : 30)
        }
        if showingActivity { return 204 }
        switch session.phase {
        case .ready, .draft: return composerHeight + 26 + 31 + (session.attachments.isEmpty ? 0 : 30)
        case .listening: return 124
        case .working: return 118
        case .result: return 220
        case .stopped, .attention: return session.result == nil ? 116 : 220
        }
    }
    /// The drawn surface: header, plus the body when it shows. The panel adds a transparent margin around it.
    var surfaceSize: CGSize {
        CGSize(width: expanded ? bodyWidth : handleWidth, height: cutoutHeight + (bodyVisible ? bodyHeight : 0))
    }
    var panelSize: CGSize {
        if expanded { return CGSize(width: bodyWidth + 16, height: cutoutHeight + (bodyVisible ? bodyHeight : 0) + 12) }
        // The independent, mouse-transparent tracer owns its own glow margin.
        return CGSize(width: handleWidth, height: cutoutHeight)
    }

    func expand(keyboard: Bool = false) {
        cancelPinnedFold()
        isHeaderOnly = false
        session.expand()
        onPresentationChange?(!keyboard)
    }

    func collapse(keyboard: Bool = false) {
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
        session.collapse()
        showingSources = false
        showingHistory = false
        showingActivity = false
        showingTasks = false
        onPresentationChange?(!keyboard)
        endYield(.activeAppUnderPinnedChat)
    }

    var blocksAutomaticDismissal: Bool { isPinned || profileMenuOpen || filePickerOpen || session.phase == .listening }

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
        if reason == .activeAppUnderPinnedChat && !(isPinned && expanded) { return }
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
        onPresentationChange?(true)
    }

    func setProfileMenuOpen(_ open: Bool) {
        profileMenuOpen = open
        if open { onInteraction?() }
        else { resumePendingPinnedFold(); onInteractionEnded?() }
    }

    func toggle() {
        onInteraction?()
        expanded ? collapse() : expand()
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
        feedback.play(.listening)
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
            switch result {
            case .success(let text):
                self.session.draft = self.voiceDraft + (self.voiceDraft.isEmpty || text.isEmpty ? "" : " ") + text
            case .failure(let error): self.promptError = error.localizedDescription
            }
            self.onPresentationChange?(true)
            self.focusRequest = UUID()
            self.resumePendingPinnedFold()
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
        feedback.play(.finishedListening)
        microphone.finish()
    }

    /// Sends the draft. An open question never blocks this (spec §4.7): a typed message can answer it or be new
    /// work. Pasted parts go as data and typed words as the owner's, so nothing asks.
    func startWork() {
        guard canCompose else { return }
        guard !session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        let outgoing = PastedRequest(text: session.draft, provenance: pasteProvenance).markedText
        let draft = session.draft
        let sources = session.attachments
        if let active = session.activeTaskID { workSessions[active] = session }
        var next = NotchSession()
        next.draft = draft
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
        pasteProvenance = PasteProvenance()
        workSessions[id] = session
        saveConversation()
        completionTask?.cancel()
        completionTraceVisible = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        onInteraction?()
        onPresentationChange?(true)
        submitToDaemon(localID: id, text: outgoing, sources: sources)
    }

    func stopWork() { requestDaemonStop() }

    func requireAttention(_ message: String) {
        guard session.phase == .working else { return }
        session.requireAttention(message)
        feedback.play(.attention)
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
        announceAttention(key)
    }

    func announceAttention(_ identity: String) {
        guard announcedAttentionIDs.insert(identity).inserted else { return }
        preferences.set(Array(announcedAttentionIDs), forKey: "FERBAnnouncedAttentionIDs")
        cancelPinnedFold()
        isHeaderOnly = false
        showingTasks = false
        showingSources = false
        showingHistory = false
        showingActivity = false
        session.expand()
        attentionStartedAt = Date()
        attentionTraceVisible = true
        attentionTask?.cancel()
        attentionTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(CompletionGlow.duration)) } catch { return }
            self?.attentionTraceVisible = false
            self?.onStatusChange?()
        }
        feedback.play(.attention)
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
