import SwiftUI
import UniformTypeIdentifiers
import NotchCore

/// FERB's one chat body: the transcript with its cards and questions, the work row, the composer with its
/// attachments, dictation, the attached-context list, Tasks and the activity feed. The notch and the sticky window
/// (APP-745) both show this view over the same NotchModel, so there is one conversation, never two; only the host that
/// holds the chat now mounts it (`NotchModel.chatInWindow`), so a draft, attachments and the transcript's place move
/// with it.
struct ChatBodyView: View {
    @ObservedObject var model: NotchModel
    /// The notch animates content changes with its spring; the window cross-fades only when FERB is animating.
    var animates = true
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState private var composerFocused: Bool

    var body: some View {
        content
            .id(contentID)
            .transition(contentTransition)
            .animation(animates ? .easeOut(duration: 0.16) : nil, value: contentID)
            .onReceive(NotificationCenter.default.publisher(for: .NSCalendarDayChanged)) { _ in
                if model.followsToday {
                    model.selectedDay = Calendar.current.startOfDay(for: Date())
                    model.onBackgroundChange?(true)
                }
            }
            .onAppear { focusInput() }
            .onChange(of: model.focusRequest) { _, _ in focusInput() }
            .onChange(of: model.session.draft) { _, _ in
                model.preservePasteProvenance()
                if model.expanded && composerFocused && (NSApp.keyWindow is NotchPanel || NSApp.keyWindow is ChatWindowPanel) {
                    model.onInteraction?()
                }
            }
            .onChange(of: model.composerHeight) { _, _ in model.composerChanged() }
            .onChange(of: model.bodyVisible) { _, visible in if !visible { composerFocused = false } }
    }

    private func focusInput() {
        composerFocused = model.bodyVisible && !model.showingSources && model.viewingToday && !model.showingActivity
            && model.canCompose && model.questionFocus == nil
    }

    private var contentID: String {
        if model.showingTasks { return "tasks" }
        if model.showingSources { return "sources" }
        if model.showingActivity { return "activity" }
        return model.session.phase == .listening ? "listening" : "conversation"
    }

    private var contentTransition: AnyTransition {
        reduceMotion ? .opacity : .modifier(active: NotchContentTransition(progress: 0),
                                            identity: NotchContentTransition(progress: 1))
    }


    @ViewBuilder private var content: some View {
        if model.showingTasks { TaskListView(model: model) }
        else if model.showingSources { sources }
        else if model.showingActivity {
            VStack(spacing: 10) {
                ActivityFeed(session: model.session)
                HStack {
                    Button("Back", action: model.toggleActivity).buttonStyle(FerbButtonStyle(quiet: true))
                    Spacer()
                    if model.session.phase == .working {
                        Button("Stop this task", action: model.stopWork).buttonStyle(FerbButtonStyle())
                    }
                }
            }
        } else {
            if model.session.phase == .listening { listening }
            else { conversation }
        }
    }

    private var conversation: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack {
                Button("Today") { model.selectDay(Date()) }
                    .font(.system(size: 11, weight: model.viewingToday ? .semibold : .regular))
                    .buttonStyle(.plain)
                if !model.viewingToday {
                    Text(model.selectedDay, format: .dateTime.month(.abbreviated).day())
                        .font(.system(size: 11, weight: .semibold))
                }
                Spacer()
                Button("Tasks", action: model.toggleTasks)
                    .font(.system(size: 11)).buttonStyle(.plain)
                Button {
                    model.showingHistory.toggle()
                    if model.showingHistory { model.refreshHistoryDays() }
                    model.onPresentationChange?(true)
                } label: { Text("History").font(.system(size: 11)) }
                    .buttonStyle(.plain).foregroundStyle(Palette.secondary)
            }
            if model.showingHistory || !model.viewingToday {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 12) {
                        ForEach(model.listedDays, id: \.self) { day in
                            Button { model.selectDay(day) } label: {
                                Text(day, format: .dateTime.month(.abbreviated).day())
                                    .font(.system(size: 11))
                                    .padding(.horizontal, 8).padding(.vertical, 5)
                                    .background(.white.opacity(0.08), in: Capsule())
                            }.buttonStyle(.plain)
                        }
                        if model.listedDays.isEmpty { Text("Your conversations will appear here.").font(.system(size: 11)) }
                    }
                }.frame(height: 28)
            }
            if let localSettingReply = model.localSettingReply {
                // chat text draws through FERB's own text view (APP-788)
                HStack(alignment: .top, spacing: 5) {
                    Image(systemName: "checkmark.circle.fill").font(.system(size: 11)).foregroundStyle(Palette.green)
                    NativeMessageText(localSettingReply, size: 11, color: Palette.greenText)
                }
            }
            Group {
                if !model.visibleEntries.isEmpty || !model.inlineQuestions.isEmpty || model.inlineMemoryImport != nil {
                    ConversationTranscript(entries: model.visibleEntries, earlier: model.earlierEntries,
                                           questions: model.inlineQuestions, memoryImport: model.inlineMemoryImport,
                                           model: model)
                } else if model.shownDayLoading {
                    // its first page on its way: the loader, in the chat's own height (APP-806)
                    ProgressView().controlSize(.small).accessibilityLabel("Loading messages")
                        .frame(maxWidth: .infinity).frame(height: model.chatTranscriptHeight)
                } else if !model.viewingToday {
                    Text("No messages on this day.").font(.system(size: 12)).foregroundStyle(Palette.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            // FERB's alerts float over the chat's foot as a toast (APP-1109): no room of their own
            .overlay(alignment: .bottom) { AlertToastView(model: model) }
            if let error = model.promptError {
                NativeMessageText(error, size: 11, color: Palette.amberText)
            }
            if let line = model.introLine {
                // The tour goes on in the open chat (its preview, its controls): FERB's line above the composer.
                IntroNarrationView(line: line, cameraSpace: 0, act: model.introActions,
                                   inset: IntroNarrationView.chatInset,
                                   typed: $model.introTyped, canSkip: model.introSkippable)
                    .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 13))
            }
            if model.showsInsomniaLine { InsomniaLineView(model: model) }
            if model.showsWorkRow { WorkRowView(model: model) }
            if model.viewingToday { composer.disabled(!model.canCompose) }
            else {
                Button("Continue in Today") { model.selectDay(Date()) }
                    .buttonStyle(FerbButtonStyle(quiet: true))
            }
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 7) {
            if model.visibleEntries.isEmpty && model.inlineQuestions.isEmpty { Text("Go ahead. I’m here.")
                .font(.system(size: 14, weight: .medium))
                .frame(maxWidth: .infinity)
                .frame(height: 24) }
            VStack(spacing: 0) {
                if !model.session.attachments.isEmpty || !model.heldPastes.isEmpty || !model.receivingFiles.isEmpty {
                    attachedContext
                }
                HStack(alignment: .bottom, spacing: 3) {
                    Button(action: model.chooseFiles) {
                        Image(systemName: "paperclip").frame(width: 24, height: 30)
                    }
                    .buttonStyle(.plain).foregroundStyle(Palette.secondary)
                    .help("Attach files").accessibilityLabel("Attach files")
                    ChatInput(model: model)
                        .frame(maxWidth: .infinity)
                        .frame(height: model.composerHeight - 6)
                        .overlay(alignment: .topLeading) {
                            if model.session.draft.isEmpty {
                                Text("Message FERB…")
                                    .font(.system(size: 14)).foregroundStyle(Palette.secondary)
                                    .padding(.top, 6).allowsHitTesting(false)
                            }
                        }
                    Button(action: model.startDictation) {
                        Image(systemName: "mic").font(.system(size: 15)).frame(width: 27, height: 30)
                    }
                    .buttonStyle(.plain)
                    .help("Dictate · hold right ⌘ + ⌥").accessibilityLabel("Start dictation")
                    if model.canSend {
                        Button(action: model.startWork) {
                            Image(systemName: "arrow.up.circle.fill").font(.system(size: 22)).frame(width: 28, height: 30)
                        }
                        .buttonStyle(.plain)
                        .help("Submit · Return. New line · Shift–Return").accessibilityLabel("Submit request")
                    }
                }
                .padding(.horizontal, 7)
                .frame(minHeight: 36)
            }
            .ferbGlass(in: RoundedRectangle(cornerRadius: 12))          // APP-785: the even heavy smear
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .fill(.white.opacity(composerFocused ? 0.065 : 0.04))
                    .allowsHitTesting(false)
            }
        }
    }

    private var attachedContext: some View {
        VStack(spacing: 0) {
            if let shown = model.shownHeldPaste { ShownPasteView(paste: shown, model: model) }
            ScrollView(.horizontal) {
                HStack(spacing: 8) {
                    ForEach(model.session.attachments, id: \.self) { url in
                        ComposerAttachmentCard(url: url, model: model)
                    }
                    ForEach(model.receivingFiles) { file in ReceivingFileCard(file: file, model: model) }
                    ForEach(model.heldPastes) { paste in HeldPasteCard(paste: paste, model: model) }
                }.padding(8)
            }
            .scrollIndicators(.hidden)
            .frame(height: 100)
        }
        .frame(height: model.attachmentStripHeight)
    }

    private var listening: some View {
        VStack(spacing: 9) {
            VoiceLight(meter: model.voiceMeter)
            // The newest words stay in view: a long dictation drops its start from the preview, never its end.
            Text(model.voiceFinishing ? "Finishing…" : (model.voiceTranscript.isEmpty ? "Listening…" : model.voiceTranscript))
                .font(.system(size: 12)).foregroundStyle(Palette.secondary).lineLimit(3).truncationMode(.head)
            HStack {
                Button { model.edit() } label: { Image(systemName: "keyboard") }
                    .buttonStyle(FerbButtonStyle(quiet: true))
                    .help("Type instead").accessibilityLabel("Type instead")
                Spacer()
                if model.voiceIsHeld {
                    Text("Release ⌘⌥ to finish").font(.system(size: 11)).foregroundStyle(Palette.secondary)
                }
                Button(action: model.finishVoice) { Image(systemName: "stop.fill") }
                    .buttonStyle(FerbButtonStyle(primary: true))
                    .disabled(model.voiceFinishing)
                    .help("Finish dictation").accessibilityLabel("Finish dictation")
            }
        }
    }

    private var sources: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                SectionCaption(text: "Attached context")
                Spacer()
                Button("Done", action: model.toggleSources).buttonStyle(FerbButtonStyle(quiet: true))
            }
            ScrollView {
                VStack(spacing: 8) {
                    ForEach(model.session.attachments, id: \.self) { url in
                        HStack(spacing: 8) {
                            Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                                .resizable().scaledToFit().frame(width: 20, height: 24)
                            Text(url.lastPathComponent).lineLimit(1).truncationMode(.middle)
                            Spacer(minLength: 0)
                            Button { model.removeAttachment(url) } label: {
                                Image(systemName: "xmark").font(.system(size: 10)).frame(width: 26, height: 26)
                            }.buttonStyle(.plain).accessibilityLabel("Remove \(url.lastPathComponent)")
                        }.font(.system(size: 12))
                    }
                }
            }
            HStack {
                Button("Add files…", action: model.chooseFiles).buttonStyle(FerbButtonStyle())
                Spacer()
                Text("Files are shared when you submit").font(.system(size: 10)).foregroundStyle(Palette.secondary)
            }
        }
    }
}

/// The work row under the chat: "N active tasks", their time, the round pictures and View tasks. The spinner turns
/// only while work runs; a loop kept between its rounds is standing work (L31); a paused task is counted apart, with
/// the pause glyph when nothing runs and no time of its own (APP-901).
struct WorkRowView: View {
    @ObservedObject var model: NotchModel

    var body: some View {
        let words = model.workRowWords
        HStack(spacing: 8) {
            if model.hasRunningWork { ProgressView().controlSize(.mini) }
            else if model.pausedTaskCount > 0 { PausedGlyph(size: 13) }
            Text(words.lead)
                .font(.system(size: 11)).foregroundStyle(Palette.secondary)
            if let since = model.workSince {
                Text("· \(since, style: .timer)")
                    .font(.system(size: 11).monospacedDigit()).foregroundStyle(Palette.secondary)
            }
            if !words.trail.isEmpty {
                Text("· " + words.trail).font(.system(size: 11)).foregroundStyle(Palette.secondary)
            }
            if model.hasRunningWork || model.pausedTaskCount > 0 { TaskActivityBubble(model: model) }
            Spacer()
            Button("View tasks", action: model.toggleTasks)
                .buttonStyle(.plain).font(.system(size: 11))
        }
    }
}

/// Insomnia mode is on (APP-980): the daemon's line and the owner's End, as the work row is drawn.
/// FERB's alerts as a toast over the chat's foot (APP-1105, APP-1109): the newest one rises in, its words, "N alerts"
/// and × on its first line; it leaves by itself after six seconds in front of the owner (put away on every device)
/// unless it needs him, and waits while the pointer is on it or the stack is open. "N alerts" opens the stack: every
/// waiting alert, each with its own ×; clicking it again closes it.
struct AlertToastView: View {
    @ObservedObject var model: NotchModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hovering = false

    var body: some View {
        let alerts = model.alerts
        VStack(alignment: .leading, spacing: 6) {
            if model.alertsExpanded {
                ForEach(alerts.dropLast()) { alert in row(alert, count: nil) }
            }
            if let newest = OwnerAlerts.toast(alerts) {
                row(newest, count: alerts.count > 1 ? alerts.count : nil)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(white: 0.16).opacity(0.96), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .shadow(color: .black.opacity(0.35), radius: 10, y: 4)
        .padding(.horizontal, 2).padding(.bottom, 6)
        .onHover { hovering = $0 }
        .opacity(alerts.isEmpty ? 0 : 1)
        .offset(y: alerts.isEmpty ? (reduceMotion ? 0 : 14) : 0)
        .allowsHitTesting(!alerts.isEmpty)
        .animation(reduceMotion ? .easeInOut(duration: 0.2) : .spring(duration: 0.4, bounce: 0.15), value: alerts)
        .animation(reduceMotion ? .easeInOut(duration: 0.2) : .spring(duration: 0.35, bounce: 0), value: model.alertsExpanded)
        .task(id: lapseKey) {
            guard let id = OwnerAlerts.toast(alerts)?.id, model.bodyVisible, !model.alertsExpanded, !hovering else { return }
            do { try await Task.sleep(for: .seconds(OwnerAlerts.toastSeconds)) } catch { return }
            model.alertToastLapsed(id)
        }
    }

    private var lapseKey: String {
        "\(OwnerAlerts.toast(model.alerts)?.id ?? "")|\(model.bodyVisible)|\(model.alertsExpanded)|\(hovering)"
    }

    /// One alert: the mark, its words, then (on the newest, with more waiting) the count, and ×, on its first line.
    private func row(_ alert: OwnerAlert, count: Int?) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Image(systemName: "exclamationmark.circle").font(.system(size: 11)).foregroundStyle(Palette.amber)
            VStack(alignment: .leading, spacing: 3) {
                Text(alert.text)
                    .font(.system(size: 12)).foregroundStyle(.white.opacity(0.88))
                    .lineLimit(model.alertsExpanded ? 4 : 2).truncationMode(.tail)
                    .help(alert.text).textSelection(.enabled)
                if alert.card?.page == "phone" {
                    Button(alert.card?.line ?? PhoneWords.openPhoneSettings) { model.onOpenPhone?() }
                        .buttonStyle(.plain).font(.system(size: 11, weight: .medium))
                }
            }
            Spacer(minLength: 4)
            if let count {
                Button { model.alertsExpanded.toggle() } label: {
                    HStack(spacing: 3) {
                        Text("\(count) alerts")
                        Image(systemName: model.alertsExpanded ? "chevron.down" : "chevron.up").font(.system(size: 8, weight: .semibold))
                    }
                    .font(.system(size: 11)).foregroundStyle(Palette.secondary)
                    .padding(.horizontal, 7).padding(.vertical, 2)
                    .background(.white.opacity(0.08), in: Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(model.alertsExpanded ? "Show fewer alerts" : "Show all \(count) alerts")
            }
            Button { model.dismissAlert(alert.id) } label: {
                Image(systemName: "xmark").font(.system(size: 9, weight: .semibold)).frame(width: 16, height: 16)
            }
            .buttonStyle(.plain).foregroundStyle(Palette.secondary)
            .help("Dismiss").accessibilityLabel("Dismiss")
        }
        .transition(reduceMotion ? .opacity : .move(edge: .bottom).combined(with: .opacity))
    }
}

struct InsomniaLineView: View {
    @ObservedObject var model: NotchModel

    var body: some View {
        HStack(spacing: 8) {
            Text(model.insomniaLine)
                .font(.system(size: 11)).foregroundStyle(Palette.secondary)
                .lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 4)
            Button(InsomniaWords.end, action: model.endInsomnia)
                .buttonStyle(.plain).font(.system(size: 11))
        }
    }
}

/// "ferb." with its status dot (APP-564), the header's title in the notch and in the window.
struct ChatTitle: View {
    @ObservedObject var model: NotchModel

    var body: some View {
        Text("ferb\(Text(".").foregroundColor(model.indicatorPhase == .ready ? .white : Palette.taskColor(model.indicatorPhase)))")
            .font(.system(size: 12, weight: .semibold)).tracking(-0.4)
            .accessibilityLabel("FERB")
            .accessibilityValue(statusLabel)
            .help(statusLabel)
    }

    private var statusLabel: String {
        switch model.displayPhase {
        case .ready: return "Ready"
        case .draft: return "Ready to submit"
        case .listening: return "Listening"
        case .working: return "Working"
        case .attention: return "Needs attention"
        case .result: return "Done"
        case .stopped: return "Stopped"
        }
    }
}

private struct ChatInWindowHostKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// The view is shown by the sticky window, not the notch (APP-745).
    var chatInWindowHost: Bool {
        get { self[ChatInWindowHostKey.self] }
        set { self[ChatInWindowHostKey.self] = newValue }
    }
}
