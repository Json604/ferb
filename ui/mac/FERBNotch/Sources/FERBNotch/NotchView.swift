import SwiftUI
import UniformTypeIdentifiers
import NotchCore

struct NotchView: View {
    @ObservedObject var model: NotchModel
    @ObservedObject var presentation: NotchPresentation
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState private var composerFocused: Bool

    var body: some View {
        ZStack(alignment: .top) {
            NotchSurfaceShape(expansion: presentation.expansion, docked: model.isNotchDocked)
                .fill(Palette.green.opacity(model.isDropTarget ? 0.1 : 0))
                .frame(width: presentation.size.width, height: presentation.size.height)
                .offset(x: presentation.surfaceOffset.x, y: presentation.surfaceOffset.y)
            VStack(spacing: 0) {
                if case .intro(let line)? = model.compactContent {
                    IntroNarrationView(line: line, cameraSpace: model.isNotchDocked ? model.cutoutHeight : 0,
                                       act: model.introActions, typed: $model.introTyped,
                                       canSkip: model.introSkippable)
                        .frame(height: presentation.size.height, alignment: .top)
                } else if model.compactContent == .pulse {
                    Color.clear.frame(height: presentation.size.height)
                } else if case .reminder(let card, let text)? = model.compactContent {
                    ScrollView { ReminderArrivalView(card: card, text: text).allowsHitTesting(false) }
                        .padding(.top, model.isNotchDocked ? model.cutoutHeight : 0)
                        .frame(height: presentation.size.height, alignment: .top)
                        .contentShape(Rectangle())
                        .onTapGesture { model.openReminderChat() }
                        .accessibilityElement(children: .combine)
                        .accessibilityAddTraits(.isButton)
                        .accessibilityHint("Open reminder in chat")
                        .accessibilityAction { model.openReminderChat() }
                } else if case .plan(let text)? = model.compactContent {
                    ScrollView {
                        Text(text)
                            .font(.system(size: 13))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 25)
                            .padding(.vertical, 17)
                    }
                    .padding(.top, model.isNotchDocked ? model.cutoutHeight : 0)
                    .frame(height: presentation.size.height)
                    .contentShape(Rectangle())
                    .onTapGesture { model.expand() }
                    .accessibilityLabel("FERB is planning. Open chat.")
                } else if model.indicatorHeaderAtBottom {
                    bodyContent
                    header
                } else {
                    header
                    bodyContent
                }
            }
            // Keep the full body mounted without letting its minimum height
            // recenter the header when AppKit trims the canvas after a fold.
            .fixedSize(horizontal: false, vertical: true)
            .frame(width: model.bodyWidth, height: presentation.size.height, alignment: .top)
            .opacity(presentation.expansion)
            .mask(alignment: .top) {
                NotchSurfaceShape(expansion: presentation.expansion, docked: model.isNotchDocked)
                    .frame(width: presentation.size.width, height: presentation.size.height)
            }
            .offset(x: presentation.surfaceOffset.x, y: presentation.surfaceOffset.y)
            .allowsHitTesting(presentation.expansion > 0.001 && model.surfaceVisible)
            .accessibilityHidden(!model.surfaceVisible)
            if presentation.expansion < 0.001 {
                Button { model.toggle() } label: {
                    Color.clear.frame(width: model.handleWidth, height: model.cutoutHeight)
                        .contentShape(Rectangle())
                }
                .offset(x: presentation.surfaceOffset.x, y: presentation.surfaceOffset.y)
                .buttonStyle(.plain)
                .accessibilityLabel(model.session.phase == .working && !model.isStanding(model.session)
                                    ? "FERB is working. Open activity." : "Open FERB")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .environment(\.selectableTextGeneration, model.textRenderGeneration)
        .foregroundStyle(Palette.ink)
        .preferredColorScheme(.dark)
        .ignoresSafeArea()
        .onDrop(of: [UTType.fileURL], isTargeted: $model.isDropTarget, perform: model.acceptDrop)
        .onChange(of: model.isDropTarget) { _, targeted in if targeted { model.expand() } }
        .onReceive(NotificationCenter.default.publisher(for: .NSCalendarDayChanged)) { _ in
            if model.followsToday {
                model.selectedDay = Calendar.current.startOfDay(for: Date())
                model.onBackgroundChange?(true)
            }
        }
        .onChange(of: model.focusRequest) { _, _ in focusInput() }
        .onChange(of: model.session.draft) { _, _ in
            model.preservePasteProvenance()
            if model.expanded && composerFocused && NSApp.keyWindow is NotchPanel { model.onInteraction?() }
        }
        .onChange(of: model.composerHeight) { _, _ in model.composerChanged() }
        .onChange(of: model.bodyVisible) { _, visible in if !visible { composerFocused = false } }
        .onExitCommand { model.collapse(keyboard: true) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("FERB notch")
    }

    private func focusInput() {
        composerFocused = model.bodyVisible && !model.showingSources && model.viewingToday && !model.showingActivity
            && model.canCompose && model.questionFocus == nil
    }

    private var bodyContent: some View {
        content
            .id(contentID)
            .transition(contentTransition)
            .padding(.horizontal, 25)
            .padding(.top, 9)
            .padding(.bottom, 17)
            .frame(height: model.bodyHeight)
            .animation(presentation.animates ? .easeOut(duration: 0.16) : nil, value: contentID)
            .scaleEffect(reduceMotion ? 1 : 0.975 + 0.025 * presentation.expansion, anchor: .top)
            .blur(radius: reduceMotion ? 0 : 2 * (1 - presentation.expansion))
            .offset(y: reduceMotion ? 0 : -model.bodyHeight * (1 - presentation.bodyExpansion))
            .opacity(presentation.bodyExpansion)
            .frame(height: model.bodyHeight, alignment: .top)
            .clipped()
            .allowsHitTesting(model.bodyVisible)
            .accessibilityHidden(!model.bodyVisible)
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

    private var header: some View {
        HStack(spacing: 0) {
            Text("ferb\(Text(".").foregroundColor(model.indicatorPhase == .ready ? .white : Palette.taskColor(model.indicatorPhase)))")
                .font(.system(size: 12, weight: .semibold)).tracking(-0.4)
                .accessibilityLabel("FERB")
                .accessibilityValue(statusLabel)
                .help(statusLabel)
                .frame(width: model.isNotchDocked ? max(0, (model.bodyWidth - model.handleWidth) / 2 - 18) : nil,
                       alignment: .leading)
            Spacer(minLength: 0)
            if model.isNotchDocked {
                // Keep the original camera safe area clear; title and account live in its wings.
                Color.clear.frame(width: model.handleWidth).accessibilityHidden(true)
                Spacer(minLength: 0)
            }
            ProfileMenuButton(model: model).frame(width: 26, height: 26)
                .frame(width: model.isNotchDocked ? max(0, (model.bodyWidth - model.handleWidth) / 2 - 18) : nil,
                       alignment: .trailing)
        }
        .padding(.horizontal, 18)
        .frame(height: max(38, model.cutoutHeight))
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
                    model.onPresentationChange?(true)
                } label: { Text("History").font(.system(size: 11)) }
                    .buttonStyle(.plain).foregroundStyle(Palette.secondary)
            }
            if model.showingHistory || !model.viewingToday {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 12) {
                        ForEach(model.conversation.days, id: \.self) { day in
                            Button { model.selectDay(day) } label: {
                                Text(day, format: .dateTime.month(.abbreviated).day())
                                    .font(.system(size: 11))
                                    .padding(.horizontal, 8).padding(.vertical, 5)
                                    .background(.white.opacity(0.08), in: Capsule())
                            }.buttonStyle(.plain)
                        }
                        if model.conversation.days.isEmpty { Text("Your conversations will appear here.").font(.system(size: 11)) }
                    }
                }.frame(height: 28)
            }
            if let localSettingReply = model.localSettingReply {
                Label(localSettingReply, systemImage: "checkmark.circle.fill")
                    .font(.system(size: 11)).foregroundStyle(Palette.green)
                    .textSelection(.enabled)
            }
            if !model.visibleEntries.isEmpty || !model.inlineQuestions.isEmpty {
                ConversationTranscript(entries: model.visibleEntries, questions: model.inlineQuestions, model: model)
            } else if !model.viewingToday {
                Text("No messages on this day.").font(.system(size: 12)).foregroundStyle(Palette.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            if let error = model.promptError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.amber).textSelection(.enabled)
            }
            if let line = model.introLine {
                // The tour goes on in the open chat (its preview, its controls): FERB's line above the composer.
                IntroNarrationView(line: line, cameraSpace: 0, act: model.introActions,
                                   inset: IntroNarrationView.chatInset,
                                   typed: $model.introTyped, canSkip: model.introSkippable)
                    .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 13))
            }
            if model.showsWorkRow {
                HStack(spacing: 8) {
                    // Turns only while work runs; a loop kept between its rounds is standing work (L31).
                    if model.hasRunningWork { ProgressView().controlSize(.mini) }
                    Text(model.workRowText)
                        .font(.system(size: 11)).foregroundStyle(Palette.secondary)
                    if let since = model.workSince {
                        Text("· \(since, style: .timer)")
                            .font(.system(size: 11).monospacedDigit()).foregroundStyle(Palette.secondary)
                    }
                    if model.hasRunningWork { TaskActivityBubble(model: model) }
                    Spacer()
                    Button("View tasks", action: model.toggleTasks)
                        .buttonStyle(.plain).font(.system(size: 11))
                }
            }
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
                if !model.session.attachments.isEmpty { attachedContext }
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
                    if !model.session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
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
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .fill(.white.opacity(composerFocused ? 0.065 : 0.04))
                    .allowsHitTesting(false)
            }
        }
    }

    private var attachedContext: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(model.session.attachments, id: \.self) { url in
                    ComposerAttachmentCard(url: url, model: model)
                }
            }.padding(8)
        }
        .scrollIndicators(.hidden)
        .frame(height: model.attachmentStripHeight)
    }

    private var listening: some View {
        VStack(spacing: 9) {
            VoiceLight(listening: true, startedAt: model.voiceStartedAt, active: model.expanded)
            Text(model.voiceFinishing ? "Finishing…" : (model.voiceTranscript.isEmpty ? "Listening…" : model.voiceTranscript))
                .font(.system(size: 12)).foregroundStyle(Palette.secondary).lineLimit(3)
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
