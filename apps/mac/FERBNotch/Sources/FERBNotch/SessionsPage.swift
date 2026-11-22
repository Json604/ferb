import AppKit
import SwiftUI
import NotchCore

/// Sessions: the owner's Claude Code and Codex sessions on this Mac, in any app. The list, one session's conversation
/// (the agent's own record, shown as plain text: nothing in it is a link or runs), its open questions with their
/// buttons, a composer, and Watch, Hand to FERB, Stop its turn, Stop controlling and New session. Everything that would
/// type or run something shows the daemon's question with the exact text first.
struct SessionsPage: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var sessions: SessionsModel
    /// The FERB window is on screen: only then does the page ask again (list ~3 s, the open session ~2 s).
    @State private var visible = true
    @State private var sheet: SessionsSheet?

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(WindowVisibilityReader { visible = $0 })
            .task(id: visible) {
                guard visible else { return }
                var tick = 0
                while !Task.isCancelled {
                    do { try await Task.sleep(for: .seconds(1)) } catch { return }
                    tick += 1
                    if sessions.openID != nil, tick % 2 == 0 { await sessions.poll() }
                    if tick % 3 == 0 { await sessions.load(quiet: true) }
                }
            }
            .onReceive(NotificationCenter.default.publisher(for: .ferbSessionsChanged)) { event in
                let named = event.object as? String
                Task {
                    await sessions.load(quiet: true)
                    if named == nil || named == sessions.openID { await sessions.poll() }
                }
            }
            .sheet(isPresented: Binding(get: { sheet != nil || sessions.confirm != nil },
                                        set: { if !$0 { sheet = nil; sessions.cancelConfirm() } })) {
                SessionsSheetView(sessions: sessions, previewAccess: store.previewAccess, sheet: $sheet)
            }
    }

    @ViewBuilder private var content: some View {
        if sessions.openID != nil, let row = sessions.openSession {
            SessionDetailView(sessions: sessions, row: row, sheet: $sheet)
        } else {
            list
        }
    }

    private var list: some View {
        WindowPage {
            if let problem = sessions.problem { ProblemLine(text: problem) }
            if let note = sessions.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
            if sessions.enabled == false {
                WindowCard {
                    VStack(alignment: .leading, spacing: 12) {
                        Text("FERB doesn't see your Claude Code and Codex sessions yet.")
                            .font(.system(size: 15, weight: .semibold))
                        Text("Turn it on and you can follow and answer them from here, start new ones and hand one to FERB. FERB adds a small hook to Claude Code's settings so it hears when a session finishes or needs you. Codex needs nothing added.")
                            .font(.system(size: 12)).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        Button("Turn on") { Task { await store.turnOnSessions() } }
                            .buttonStyle(SetupActionStyle(primary: true))
                    }
                }
            } else if sessions.enabled == true {
                HStack {
                    Spacer()
                    Button("New session") { sheet = .start; Task { await sessions.prepareNew() } }
                    Button { Task { await sessions.load() } } label: { Image(systemName: "arrow.clockwise") }
                        .help("Refresh sessions").accessibilityLabel("Refresh sessions")
                }.padding(.bottom, 4)
                if sessions.sessions.isEmpty {
                    Text("No Claude Code or Codex sessions are running right now.").foregroundStyle(.secondary)
                }
                ForEach(sessions.sessions) { row in
                    Button { Task { await sessions.open(row) } } label: {
                        WindowCard { SessionRowView(row: row) }.contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

/// What the page's one sheet shows (the daemon's question takes it over whenever there is one).
enum SessionsSheet: Equatable {
    case handOver, start, watch(AgentSessionRow)
}

private struct ProblemLine: View {
    let text: String
    var body: some View {
        Label(text, systemImage: "exclamationmark.circle").foregroundStyle(.red).textSelection(.enabled)
    }
}

/// A session's state: Working, Waiting for you or Idle.
struct SessionStateChip: View {
    let row: AgentSessionRow
    var body: some View {
        Text(row.stateWords).font(.system(size: 10, weight: .medium))
            .foregroundStyle(row.status == "waiting_for_you" ? Color.orange : .white.opacity(row.status == "working" ? 0.85 : 0.5))
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(.white.opacity(0.06), in: Capsule())
    }
}

private struct FerbBadge: View {
    var body: some View {
        Text("FERB is on it").font(.system(size: 10, weight: .semibold)).foregroundStyle(.black)
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(Color.orange.opacity(0.9), in: Capsule())
    }
}

/// One row of the list: which agent, its title (or folder), where it runs, its last line, its state.
struct SessionRowView: View {
    let row: AgentSessionRow

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            VStack(alignment: .leading, spacing: 6) {
                Text(row.agentName.uppercased()).font(.system(size: 9, weight: .semibold)).tracking(1.2)
                    .foregroundStyle(.white.opacity(0.5))
                Text(row.heading).font(.system(size: 15, weight: .semibold)).lineLimit(2)
                Text(row.whereLine).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1)
                if !row.lastText.isEmpty {
                    Text(row.lastText).font(.system(size: 12)).foregroundStyle(.white.opacity(0.6)).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 6) {
                HStack(spacing: 6) {
                    if row.withFerb { FerbBadge() }
                    SessionStateChip(row: row)
                }
                if let at = row.lastAt {
                    Text(at.formatted(date: .omitted, time: .shortened)).font(.system(size: 11)).foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// One session open: its header and actions, its conversation, its open cards, and the composer.
struct SessionDetailView: View {
    @ObservedObject var sessions: SessionsModel
    let row: AgentSessionRow
    @Binding var sheet: SessionsSheet?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if let problem = sessions.problem { ProblemLine(text: problem) }
            if let note = sessions.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
            timeline
            composer
        }
        .padding(.horizontal, 30).padding(.bottom, 24)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Button { sessions.close() } label: { Label("Sessions", systemImage: "chevron.left") }
                Spacer()
                if row.host != nil { Button("Watch") { sheet = .watch(row) } }
                if !row.withFerb { Button("Hand to FERB") { sheet = .handOver } }
                if row.status == "working" { Button("Stop its turn") { Task { await sessions.interrupt() } } }
                if row.yours { Button("Stop controlling") { Task { await sessions.release() } } }
            }
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(row.heading).font(.system(size: 17, weight: .semibold)).lineLimit(2).textSelection(.enabled)
                    Text(row.agentName + " · " + row.whereLine).font(.system(size: 12)).foregroundStyle(.secondary)
                    if let goal = row.goal { Text(goal).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(2) }
                }
                Spacer(minLength: 8)
                if row.withFerb { FerbBadge() }
                SessionStateChip(row: row)
            }
        }
    }

    private var timeline: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if sessions.more {
                        Button("Show earlier") { Task { await sessions.showEarlier() } }
                            .frame(maxWidth: .infinity)
                    }
                    ForEach(sessions.items) { item in SessionItemView(item: item).id(item.seq) }
                    ForEach(sessions.cards) { card in
                        SessionCardView(card: card, agentName: row.agentName) { choice in
                            Task { await sessions.answer(card, choice) }
                        }
                    }
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(.vertical, 6)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .onChange(of: sessions.items.last?.seq) { _, _ in proxy.scrollTo("end", anchor: .bottom) }
            .onChange(of: sessions.cards.count) { _, _ in proxy.scrollTo("end", anchor: .bottom) }
            .onAppear { proxy.scrollTo("end", anchor: .bottom) }
        }
    }

    @ViewBuilder private var composer: some View {
        if row.canSend {
            HStack(alignment: .bottom, spacing: 10) {
                TextField("Message \(row.agentName)…", text: $sessions.draft, axis: .vertical)
                    .textFieldStyle(.plain).lineLimit(1...8)
                    .padding(.horizontal, 14).padding(.vertical, 11)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .onSubmit { Task { await sessions.send() } }
                Button("Send") { Task { await sessions.send() } }
                    .buttonStyle(SetupActionStyle(primary: true))
                    .disabled(sessions.busy || sessions.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        } else if let why = row.noSend {
            Text(why).font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// One line of the conversation: the owner's words on the right, the agent's on the left, a tool call as one small
/// monospace line, an interrupt or an error as a quiet line. Plain text only.
struct SessionItemView: View {
    let item: SessionItem

    var body: some View {
        switch item.kind {
        case "user":
            HStack {
                Spacer(minLength: 80)
                Text(item.text).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 14).padding(.vertical, 9)
                    .background(.white.opacity(0.12), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            }
        case "tool":
            Text(item.line).font(.system(size: 11, design: .monospaced)).foregroundStyle(.white.opacity(0.55))
                .lineLimit(2).textSelection(.enabled)
        case "interrupted":
            Text(item.text.isEmpty ? "You stopped its turn." : item.text).font(.system(size: 12)).italic()
                .foregroundStyle(.secondary)
        case "error":
            Label(item.text, systemImage: "exclamationmark.triangle").font(.system(size: 12))
                .foregroundStyle(.red.opacity(0.85)).textSelection(.enabled)
        default:
            HStack {
                Text(item.text).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 80)
            }
        }
    }
}

/// The agent's open permission prompt or question, with its buttons; one only the Mac's own dialog can answer says so.
struct SessionCardView: View {
    let card: SessionCard
    let agentName: String
    let choose: (SessionChoice) -> Void

    var body: some View {
        WindowCard {
            VStack(alignment: .leading, spacing: 10) {
                Text(card.heading(agentName: agentName)).font(.system(size: 14, weight: .semibold))
                if let question = card.question {
                    Text(question).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                }
                if !card.detail.isEmpty {
                    Text(card.detail).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                        .background(.black.opacity(0.35), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                if card.choices.isEmpty {
                    Text("Answer this one on your Mac.").font(.system(size: 12)).foregroundStyle(.secondary)
                } else {
                    FlowButtons(choices: card.choices, choose: choose)
                }
            }
        }
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color.orange.opacity(0.45)))
    }
}

private struct FlowButtons: View {
    let choices: [SessionChoice]
    let choose: (SessionChoice) -> Void
    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) { buttons }
            VStack(alignment: .leading, spacing: 8) { buttons }
        }
    }
    @ViewBuilder private var buttons: some View {
        ForEach(choices) { choice in Button(choice.label) { choose(choice) } }
    }
}

/// The page's one sheet: the daemon's question before anything is sent or run, Hand to FERB, New session, or Watch.
struct SessionsSheetView: View {
    @ObservedObject var sessions: SessionsModel
    let previewAccess: PreviewAccess
    @Binding var sheet: SessionsSheet?

    var body: some View {
        Group {
            if let confirm = sessions.confirm {
                SessionConfirmView(confirm: confirm,
                                   yes: { Task { sheet = nil; await sessions.confirmYes() } },
                                   cancel: { sessions.cancelConfirm() })
            } else {
                switch sheet {
                case .handOver: HandOverView(sessions: sessions, close: { sheet = nil })
                case .start: NewSessionView(sessions: sessions, close: { sheet = nil })
                case .watch(let row): SessionWatchView(row: row, rpc: sessions.rpc, access: previewAccess,
                                                       close: { sheet = nil })
                case nil: EmptyView()
                }
            }
        }
        .buttonStyle(SetupActionStyle())
        .foregroundStyle(.white.opacity(0.92))
        .font(.system(size: 13))
        .background(Color(white: 0.09))
        .preferredColorScheme(.dark)
    }
}

/// The daemon's question: its title, the exact text in monospace, what the Yes covers; Yes or Cancel.
struct SessionConfirmView: View {
    let confirm: SessionsModel.Confirm
    let yes: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(confirm.yes.question).font(.system(size: 15, weight: .semibold))
                .fixedSize(horizontal: false, vertical: true)
            if !confirm.yes.detail.isEmpty {
                ScrollView {
                    Text(confirm.yes.detail).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 240).fixedSize(horizontal: false, vertical: true)
                .padding(10)
                .background(.black.opacity(0.4), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            if !confirm.yes.covers.isEmpty {
                Text(confirm.yes.covers).font(.system(size: 11)).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel", action: cancel).keyboardShortcut(.cancelAction)
                Button(isStart ? "Start" : "Yes, send it", action: yes).buttonStyle(SetupActionStyle(primary: true))
            }
        }
        .padding(24).frame(width: 480)
    }

    private var isStart: Bool {
        if case .start = confirm.action { return true }
        return false
    }
}

/// Hand to FERB: the owner's goal; FERB works the session toward it in the chat.
struct HandOverView: View {
    @ObservedObject var sessions: SessionsModel
    let close: () -> Void
    @State private var goal = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Hand this session to FERB").font(.system(size: 17, weight: .semibold))
            Text("What should it get done?").foregroundStyle(.secondary)
            TextField("Keep going until the tests pass", text: $goal, axis: .vertical)
                .textFieldStyle(.plain).lineLimit(2...6)
                .padding(10).background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            Text("FERB answers the agent for you and asks you before anything risky. You can stop it any time.")
                .font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Hand it over") { Task { if await sessions.delegate(goal: goal) { close() } } }
                    .buttonStyle(SetupActionStyle(primary: true))
                    .disabled(goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(24).frame(width: 460)
    }
}

/// New session: which agent, which folder, an optional first message; Start shows the exact command first.
struct NewSessionView: View {
    @ObservedObject var sessions: SessionsModel
    let close: () -> Void
    @State private var agent = "claude_code"
    @State private var folder = ""
    @State private var first = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New session").font(.system(size: 17, weight: .semibold))
            Text("Which agent?").foregroundStyle(.secondary)
            Picker("Which agent?", selection: $agent) {
                ForEach(sessions.agents, id: \.self) { Text(AgentSessionRow.agentName($0)).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden()
            Text("Folder").foregroundStyle(.secondary)
            Menu(folder.isEmpty ? "Choose…" : (folder as NSString).abbreviatingWithTildeInPath) {
                ForEach(sessions.folders, id: \.self) { path in
                    Button((path as NSString).abbreviatingWithTildeInPath) { folder = path }
                }
                if !sessions.folders.isEmpty { Divider() }
                Button("Choose…") { choose() }
            }
            .fixedSize()
            Text("First message (optional)").foregroundStyle(.secondary)
            TextField("", text: $first, axis: .vertical)
                .textFieldStyle(.plain).lineLimit(2...6)
                .padding(10).background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            if let problem = sessions.problem { ProblemLine(text: problem) }
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Start") { Task { await sessions.planStart(agent: agent, folder: folder, text: first) } }
                    .buttonStyle(SetupActionStyle(primary: true))
                    .disabled(folder.isEmpty)
            }
        }
        .padding(24).frame(width: 480)
        .onAppear { if let named = sessions.agents.first, !sessions.agents.contains(agent) { agent = named } }
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        guard AppWindowPresentation.runModal(panel) == .OK, let url = panel.url else { return }
        folder = url.path
    }
}

/// Watch: the session's window, live, while this sheet shows (memory only; hidden from screen sharing, APP-406).
struct SessionWatchView: View {
    let row: AgentSessionRow
    let close: () -> Void
    @StateObject private var preview: TaskPreview

    init(row: AgentSessionRow, rpc: DaemonRPCTransport, access: PreviewAccess, close: @escaping () -> Void) {
        self.row = row
        self.close = close
        _preview = StateObject(wrappedValue: TaskPreview(rpc: rpc, session: row.id, access: access))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Watching \(row.heading)").font(.system(size: 15, weight: .semibold)).lineLimit(1)
                Spacer()
                Button(action: close) { Image(systemName: "xmark") }
                    .keyboardShortcut(.cancelAction).accessibilityLabel("Close")
            }
            TaskVideoView(preview: preview)
                .frame(minWidth: 640, minHeight: 400)
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
        .padding(20)
        .background(HiddenFromCapture())
    }
}

/// Keeps the window showing this view out of screen sharing and recordings (APP-406's rule for live previews).
struct HiddenFromCapture: NSViewRepresentable {
    func makeNSView(context: Context) -> Marker { Marker() }
    func updateNSView(_ nsView: Marker, context: Context) {}

    final class Marker: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            window?.sharingType = .none
        }
    }
}
