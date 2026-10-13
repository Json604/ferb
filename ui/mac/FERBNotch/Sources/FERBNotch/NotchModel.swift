import AppKit
import SwiftUI
import UniformTypeIdentifiers
import NotchCore

@MainActor
final class NotchModel: ObservableObject {
    @Published var session = NotchSession()
    @Published var measuredTranscriptHeight: CGFloat = 0
    var workSessions: [UUID: NotchSession] = [:]
    var submissions: [UUID: Task<Void, Never>] = [:]
    @Published var pendingStops = Set<UUID>()
    @Published var conversation = ConversationArchive()
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
    let microphone: VoiceCapturing
    @Published var voiceTranscript = ""
    @Published var voiceFinishing = false
    private var voiceDraft = ""
    let rpc: DaemonRPCTransport
    let preferences: UserDefaults
    @Published var connectionStatus = "Connecting to FERB…"
    @Published var pendingApprovals: [DaemonApproval] = []
    @Published var selectedApprovalID: String?
    var presentedApproval: DaemonApproval? {
        pendingApprovals.first(where: { $0.id == selectedApprovalID }) ?? pendingApprovals.first
    }
    @Published var promptError: String?
    @Published var sendingAnswer = false
    @Published var runningTasks: [String: String] = [:]
    var conversationID: String?
    var supportsAttachments = false
    var eventsTask: Task<Void, Never>?
    var reconciliationTask: Task<Void, Never>?
    var submissionTask: Task<Void, Never>?
    var pastedTexts: [String] = []
    var lastProvenanceDraft = ""
    var pasteWasEdited = false
    var knownCompletedTasks = Set<String>()

    init(rpc: DaemonRPCTransport = DaemonRPC(), preferences: UserDefaults = .standard, microphone: VoiceCapturing? = nil) {
        self.microphone = microphone ?? Microphone()
        self.rpc = rpc
        self.preferences = preferences
        if let data = preferences.data(forKey: "FERBConversationArchive"),
           let saved = try? JSONDecoder().decode(ConversationArchive.self, from: data) {
            conversation = saved
            conversation.interruptRestoredDrafts()
            if conversation != saved, let repaired = try? JSONEncoder().encode(conversation) {
                preferences.set(repaired, forKey: "FERBConversationArchive")
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
    private var filePickerOpen = false
    private var heldVoiceSnapshot: (session: NotchSession, sources: Bool, history: Bool, activity: Bool, headerOnly: Bool)?

    var expanded: Bool { session.isExpanded }
    var bodyVisible: Bool { expanded && !isHeaderOnly }
    var bodyWidth: CGFloat { max(360, handleWidth + 164) }
    var showsCompletionGlow: Bool { completionTraceVisible }
    var showsTraceGlow: Bool { attentionTraceVisible || showsCompletionGlow }
    var composerHeight: CGFloat {
        let textWidth = bodyWidth - 152
        let measured = ((session.draft + "\u{200B}") as NSString).boundingRect(
            with: CGSize(width: textWidth, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: NSFont.systemFont(ofSize: 14)])
        return min(72, max(18, ceil(measured.height))) + 18
    }
    var chatTranscriptHeight: CGFloat { min(320, max(240, measuredTranscriptHeight)) }

    var bodyHeight: CGFloat {
        if showingTasks { return 320 }
        if !pendingApprovals.isEmpty { return 260 }
        if showingSources { return 250 }
        if session.phase != .listening && !showingActivity {
            return (visibleEntries.isEmpty ? 31 : chatTranscriptHeight) + composerHeight + 58 + (hasRunningWork ? 22 : 0) + ((showingHistory || !viewingToday) ? 36 : 0) + (promptError == nil ? 0 : 48) + (session.attachments.isEmpty ? 0 : 30)
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

    func startWork() {
        guard canCompose else { return }
        guard pendingApprovals.isEmpty else { expand(); return }
        preservePasteProvenance()
        let pasted = PastedRequest(text: session.draft, pastes: pastedTexts, editedAfterPaste: pasteWasEdited)
        var outgoing = pasted.markedText
        if pasted.isEntirelyPasted {
            let choice = NSAlert()
            choice.messageText = "Is this pasted text your request?"
            choice.informativeText = "Text copied from a page or document is treated as reference material. Choose My request only for instructions you want FERB to follow."
            choice.addButton(withTitle: "My request")
            choice.addButton(withTitle: "Reference material")
            choice.addButton(withTitle: "Cancel")
            switch choice.runModal() {
            case .alertFirstButtonReturn: outgoing = session.draft
            case .alertSecondButtonReturn: break
            default: return
            }
        }
        guard !session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
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
        conversation.begin(id: id, request: session.draft, sources: session.attachments)
        session.clearComposer()
        pastedTexts = []
        pasteWasEdited = false
        lastProvenanceDraft = ""
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

    func preservePasteProvenance() {
        guard !pastedTexts.isEmpty else { return }
        // Native edits can replace/reorder duplicate substrings. Require an explicit whole-draft
        // provenance decision instead of guessing which surviving copy was the pasted source.
        if session.draft != lastProvenanceDraft { pasteWasEdited = true }
        lastProvenanceDraft = session.draft
    }

    func acceptPastedText(_ providers: [NSItemProvider]) {
        guard canCompose else { return }
        let editor = NSApp.keyWindow?.firstResponder as? NSTextView
        for provider in providers {
            provider.loadObject(ofClass: NSString.self) { [weak self] value, _ in
                guard let text = value as? String else { return }
                Task { @MainActor in
                    guard let self, self.canCompose else { return }
                    if let editor {
                        guard NSApp.keyWindow?.firstResponder === editor else { return }
                        self.pastedTexts.append(text)
                        self.lastProvenanceDraft = (self.session.draft as NSString).replacingCharacters(in: editor.selectedRange(), with: text)
                        editor.insertText(text, replacementRange: editor.selectedRange())
                    } else {
                        self.pastedTexts.append(text)
                        self.lastProvenanceDraft = self.session.draft + text
                        self.session.draft += text
                    }
                    self.onInteraction?()
                }
            }
        }
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

    func chooseFiles() {
        filePickerOpen = true
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
                self.filePickerOpen = false
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
        // Update the shared surface geometry when the field wraps or adds a line.
        if expanded && canCompose { onPresentationChange?(true) }
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
        selectedApprovalID = approval.id
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
