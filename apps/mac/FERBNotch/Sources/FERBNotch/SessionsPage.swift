import AppKit
import SwiftUI
import NotchCore

/// Sessions: every terminal on this Mac (HAND-691), the owner's Claude Code and Codex sessions among them, in any app,
/// and the agent apps' windows (HAND-710). The list, one session's conversation (the agent's own record, shown as plain
/// text: nothing in it is a link or runs) or, for a watched one, its window live; its open questions with their
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
            .sheet(isPresented: Binding(get: { sheet != nil || sessions.confirm != nil || sessions.pick != nil },
                                        set: { if !$0 { sheet = nil; sessions.cancelConfirm(); sessions.cancelPick() } })) {
                SessionsSheetView(sessions: sessions, previewAccess: store.previewAccess, sheet: $sheet)
            }
    }

    @ViewBuilder private var content: some View {
        if sessions.openID != nil, let row = sessions.openSession {
            SessionDetailView(sessions: sessions, row: row, sheet: $sheet, previewAccess: store.previewAccess)
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
                        Text("FERB doesn't see your terminal sessions yet.")
                            .font(.system(size: 15, weight: .semibold))
                        Text("Turn it on and you can follow your terminals and type into them from here, Claude Code and Codex too, open new ones and hand one to FERB. FERB adds a small hook to Claude Code's settings so it hears when a session finishes or needs you.")
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
                // a terminal just opened that hasn't shown yet says so, quietly, until it does
                if let waiting = sessions.awaitingNewLine {
                    Text(waiting).font(.system(size: 12)).foregroundStyle(.secondary)
                } else if sessions.sessions.isEmpty {
                    Text("No terminals or agent apps are open on your Mac right now.").foregroundStyle(.secondary)
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

/// One session open: its header and actions, its conversation, its open cards, and the composer. A watched session
/// (HAND-710) shows its window live in place of the conversation, the quiet line that what he sends is typed into it,
/// his words only while they are on their way or weren't sent, and the box.
struct SessionDetailView: View {
    @ObservedObject var sessions: SessionsModel
    let row: AgentSessionRow
    @Binding var sheet: SessionsSheet?
    let previewAccess: PreviewAccess

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if let problem = sessions.problemShown { ProblemLine(text: problem) }
            if let note = sessions.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
            // HAND-702: FERB's word on his hand-over, at the end of the conversation above the box (under the header
            // when there is no box)
            if !row.canSend { ferbNote }
            if row.watch {
                SessionLiveView(session: row.id, rpc: sessions.rpc, access: previewAccess).id(row.id)
                if row.canSend {
                    Text(AgentSessionRow.typeNote).font(.system(size: 12)).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                // no conversation to hold them: his words show only while sending, or not sent with Try again
                if let outgoing = sessions.outgoing, outgoing.session == row.id, outgoing.state != .sent {
                    SessionOutgoingView(outgoing: outgoing) { Task { await sessions.tryAgain() } }
                }
            } else {
                timeline
            }
            if row.canSend { ferbNote }
            composer
        }
        .padding(.horizontal, 30).padding(.bottom, 24)
    }

    @ViewBuilder private var ferbNote: some View {
        if !row.ferbNote.isEmpty { SessionFerbNote(text: row.ferbNote) }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Button { sessions.close() } label: { Label("Sessions", systemImage: "chevron.left") }
                Spacer()
                // a watched session already shows its window here
                if row.host != nil, !row.watch { Button("Watch") { sheet = .watch(row) } }
                handOverButton
                if row.status == "working" { Button("Stop its turn") { Task { await sessions.interrupt() } } }
                if row.yours { Button("Stop controlling") { Task { await sessions.release() } } }
            }
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(row.heading).font(.system(size: 17, weight: .semibold)).lineLimit(2).textSelection(.enabled)
                    Text(row.detailLine).font(.system(size: 12)).foregroundStyle(.secondary)
                    if let goal = row.goal { Text(goal).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(2) }
                }
                Spacer(minLength: 8)
                if row.withFerb { FerbBadge() }
                SessionStateChip(row: row)
            }
        }
    }

    /// Hand to FERB while the session is his; Take it back while FERB's task keeps it going (it stops that task);
    /// "FERB is starting…" between the two, while FERB hasn't started a task on his goal yet.
    @ViewBuilder private var handOverButton: some View {
        if !row.ferbTask.isEmpty {
            let pending = sessions.isTakingBack(row)
            Button { Task { await sessions.takeItBack() } } label: {
                if pending {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Taking it back…")
                    }
                } else {
                    Label("Take it back", systemImage: "arrow.uturn.backward")
                }
            }
            .disabled(pending)
        } else if row.withFerb {
            Button("FERB is starting…") {}.disabled(true)
        } else if row.offersHandOver {
            Button("Hand to FERB") { sessions.handOverProblem = nil; sheet = .handOver }
        }
    }

    /// The conversation, by the chat's own list and rules (PHN-813): earlier items as he scrolls up, "New message"
    /// when something arrives while he is scrolled up, his words on their way (PHN-812).
    private var timeline: some View {
        Group {
            // while the first read is on its way: the loading look, never an error (PHN-816)
            if sessions.opening {
                SessionSkeleton()
            } else {
                SessionTranscript(sessions: sessions, agent: row.agent, agentName: row.agentName)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder private var composer: some View {
        if row.canSend {
            HStack(alignment: .bottom, spacing: 10) {
                TextField(row.composerPlaceholder, text: $sessions.draft, axis: .vertical)
                    .textFieldStyle(.plain).lineLimit(1...8)
                    .padding(.horizontal, 14).padding(.vertical, 11)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .onSubmit { Task { await sessions.send() } }
                // while his words are on their way the button shows it, never just greyed (PHN-812)
                Button { Task { await sessions.send() } } label: {
                    if sending {
                        ProgressView().controlSize(.small).accessibilityLabel("Sending…")
                    } else {
                        Text("Send")
                    }
                }
                .buttonStyle(SetupActionStyle(primary: true))
                .disabled(sessions.busy || sessions.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        } else if let why = row.noSend {
            Text(why).font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private var sending: Bool { sessions.outgoing?.state == .sending }
}

/// One line of the conversation: the owner's words on the right in one bubble, the agent's reply as one block holding
/// its parts in order (HAND-703: Markdown drawn by the chat's reader, a thought or a step as a small row that opens,
/// the turn's meta line under it), a terminal's output as fixed-width text, a tool call as one small monospace line,
/// an interrupt or an error as a quiet line. Nothing in it runs; links are the chat's inert ones.
struct SessionItemView: View {
    let item: SessionItem
    /// The session's agent ("terminal": its output is not Markdown).
    var agent = ""

    var body: some View {
        switch item.look(agent: agent) {
        case .owner(let text):
            SessionBubble(text: text)
        case .ferbSent(let text):
            // HAND-709: what FERB sent while it kept the session going, marked as FERB's
            VStack(alignment: .trailing, spacing: 3) {
                Text("Sent by FERB").font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary)
                    .padding(.trailing, 4)
                SessionBubble(text: text)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .accessibilityElement(children: .combine)
        case .toolLine(let line):
            Text(line).font(.system(size: 11, design: .monospaced)).foregroundStyle(.white.opacity(0.55))
                .lineLimit(2).textSelection(.enabled)
        case .interrupted(let text):
            Text(text.isEmpty ? "You stopped its turn." : text).font(.system(size: 12)).italic()
                .foregroundStyle(.secondary)
        case .error(let text):
            Label(text, systemImage: "exclamationmark.triangle").font(.system(size: 12))
                .foregroundStyle(.red.opacity(0.85)).textSelection(.enabled)
        case .fixedWidth(let text):
            // the chat's text surface: select and copy as in the chat (APP-788), the terminal's text as it came
            HStack {
                NativeMessageText(text, size: 12, monospaced: true)
                Spacer(minLength: 80)
            }
        case .reply(let parts):
            SessionReplyView(parts: parts)
        }
    }
}

/// An agent's reply (HAND-703): the main chat's FERB-reply look, one block for the whole turn, its parts in order;
/// the turn's meta lines ("Just now", "Working…") small and dim under it, never in the block.
struct SessionReplyView: View {
    let parts: [SessionPart]

    var body: some View {
        let body = parts.filter { $0.shown != "meta" }
        let meta = parts.filter { $0.shown == "meta" && !$0.text.isEmpty }.map(\.text)
        VStack(alignment: .leading, spacing: 4) {
            if !body.isEmpty {
                FerbBubble {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(Array(body.enumerated()), id: \.offset) { _, part in
                            SessionPartView(part: part)
                        }
                    }
                }
            }
            if !meta.isEmpty {
                Text(meta.joined(separator: " · ")).font(.system(size: 10)).foregroundStyle(.secondary)
                    .padding(.leading, 11)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// One part of a reply: Markdown text, or a thought's or a step's row (collapsed; it opens only when it has words).
struct SessionPartView: View {
    let part: SessionPart
    @State private var open = false

    var body: some View {
        switch part.shown {
        case "thinking", "tool":
            VStack(alignment: .leading, spacing: 6) {
                Button { if part.expandable { open.toggle() } } label: {
                    HStack(spacing: 6) {
                        if part.shown == "tool" {
                            Image(systemName: "terminal").font(.system(size: 10)).foregroundStyle(.secondary)
                        }
                        Text(part.rowLabel).font(.system(size: 12, weight: .medium))
                            .foregroundStyle(.white.opacity(0.75)).lineLimit(1)
                        if !part.rowDetail.isEmpty {
                            Text(part.rowDetail).font(.system(size: 12)).foregroundStyle(.white.opacity(0.45))
                                .lineLimit(1).truncationMode(.tail)
                        }
                        if part.expandable {
                            Image(systemName: open ? "chevron.down" : "chevron.right")
                                .font(.system(size: 9, weight: .semibold)).foregroundStyle(.secondary)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!part.expandable)
                .accessibilityValue(part.expandable ? (open ? "Expanded" : "Collapsed") : "")
                if open && part.expandable {
                    if part.shown == "tool" {
                        NativeMessageText(part.text, size: 11, monospaced: true)
                            .padding(8).frame(maxWidth: .infinity, alignment: .leading)
                            .background(.black.opacity(0.3), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    } else {
                        NativeMessageText(part.text, size: 12, secondary: true)
                    }
                }
            }
        default:
            MarkdownContent(part.text, size: 13).equatable()
        }
    }
}

/// HAND-702: FERB's latest word on his hand-over, one quiet line with FERB's mark, shown as the daemon says it.
struct SessionFerbNote: View {
    let text: String
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Image(nsImage: MenuBarIcon.image(MenuBarMark(dot: .solid, dimmed: false), description: "FERB"))
                .renderingMode(.template).resizable().frame(width: 13, height: 13)
                .foregroundStyle(.white.opacity(0.7)).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2 }
            Text(text).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(2).textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
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
            if let pick = sessions.pick {
                PickTerminalView(pick: pick, choose: { t in Task { await sessions.choose(t) } }, cancel: { sessions.cancelPick() })
            } else if let confirm = sessions.confirm {
                SessionConfirmView(confirm: confirm,
                                   yes: {
                                       Task {
                                           // a new session's form stays, pending, until its terminal opened
                                           if case .start = confirm.action {
                                               if await sessions.confirmYes() { sheet = nil }
                                           } else {
                                               sheet = nil
                                               await sessions.confirmYes()
                                           }
                                       }
                                   },
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

/// HAND-683: FERB can't tell which terminal is the session's: the daemon's reason, the terminals it can see by the
/// app's own words (the one in front now first), and Don't send.
struct PickTerminalView: View {
    let pick: SessionsModel.Pick
    let choose: (SessionChoice) -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Which terminal is this session in?").font(.system(size: 15, weight: .semibold))
            Text(pick.why).font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 6) {
                ForEach(pick.terminals) { t in
                    Button { choose(t) } label: {
                        Label(t.label, systemImage: "terminal").frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
            Text("FERB types only into the one you choose, and asks again if it changes.")
                .font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Don't send", action: cancel).keyboardShortcut(.cancelAction)
            }
        }
        .padding(24).frame(width: 480)
    }
}

/// Hand to FERB: the owner's goal; FERB works the session toward it in the chat. From the tap the button shows it is
/// handing it over and waits for the Mac; a yes closes the sheet, a refusal stays here under the button with his words.
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
                .disabled(sessions.handingOver)
            Text("FERB answers the agent for you and asks you before anything risky. You can stop it any time.")
                .font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button { Task { if await sessions.delegate(goal: goal) { close() } } } label: {
                    if sessions.handingOver {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Handing it to FERB…")
                        }
                    } else {
                        Text("Hand it over")
                    }
                }
                .buttonStyle(SetupActionStyle(primary: true))
                .disabled(sessions.handingOver || goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            if let why = sessions.handOverProblem { ProblemLine(text: why).font(.system(size: 12)) }
        }
        .padding(24).frame(width: 460)
    }
}

/// New session (HAND-691): a terminal. Only the folder (the daemon's list, or Choose… to browse) and an optional
/// command; Start opens it, and a command is shown exactly and typed only after the Yes. No agent to pick: the owner
/// types the one he wants. From the tap Start shows it is opening a terminal and the form waits; a refusal stays here
/// under Start.
struct NewSessionView: View {
    @ObservedObject var sessions: SessionsModel
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New session").font(.system(size: 17, weight: .semibold))
            Text("Opens a new \(sessions.host) window in the folder.").font(.system(size: 12)).foregroundStyle(.secondary)
            Text("Folder").foregroundStyle(.secondary)
            Menu(sessions.startFolder.isEmpty ? "Choose…" : (sessions.startFolder as NSString).abbreviatingWithTildeInPath) {
                ForEach(sessions.folders, id: \.self) { path in
                    Button((path as NSString).abbreviatingWithTildeInPath) { sessions.startFolder = path }
                }
                if !sessions.folders.isEmpty { Divider() }
                Button("Choose…") { choose() }
            }
            .fixedSize()
            .disabled(sessions.starting)
            Text("Command (optional)").foregroundStyle(.secondary)
            TextField("Command to run (optional)", text: $sessions.startCommand)
                .textFieldStyle(.plain).font(.system(size: 13, design: .monospaced))
                .padding(10).background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .disabled(sessions.starting)
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button {
                    Task {
                        if await sessions.planStart(folder: sessions.startFolder, command: sessions.startCommand) == .opened {
                            close()
                        }
                    }
                } label: {
                    if sessions.starting {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Opening a terminal…")
                        }
                    } else {
                        Text("Start")
                    }
                }
                .buttonStyle(SetupActionStyle(primary: true))
                .disabled(sessions.starting || sessions.startFolder.isEmpty)
            }
            if let why = sessions.startProblem { ProblemLine(text: why).font(.system(size: 12)) }
        }
        .padding(24).frame(width: 480)
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        guard AppWindowPresentation.runModal(panel) == .OK, let url = panel.url else { return }
        sessions.startFolder = url.path
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

/// A watched session's window, live, in place of its conversation (HAND-710): the same picture as Watch, sized to the
/// space, with its own loading look and errors; it captures only while the page can be seen (APP-407).
struct SessionLiveView: View {
    @StateObject private var preview: TaskPreview

    init(session: String, rpc: DaemonRPCTransport, access: PreviewAccess) {
        _preview = StateObject(wrappedValue: TaskPreview(rpc: rpc, session: session, access: access))
    }

    var body: some View {
        TaskVideoView(preview: preview)
            .frame(maxWidth: .infinity, minHeight: 200, maxHeight: .infinity)
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .background(HiddenFromCapture())
    }
}

/// Keeps the window showing this view out of screen sharing and recordings (APP-406's rule for live previews), while
/// the view is in it: the window's own setting comes back when the view leaves (the FERB window shows a watched
/// session's picture inline).
struct HiddenFromCapture: NSViewRepresentable {
    func makeNSView(context: Context) -> Marker { Marker() }
    func updateNSView(_ nsView: Marker, context: Context) {}
    static func dismantleNSView(_ nsView: Marker, coordinator: ()) { nsView.restore() }

    final class Marker: NSView {
        private weak var covered: NSWindow?
        private var before: NSWindow.SharingType = .readOnly

        override func viewWillMove(toWindow newWindow: NSWindow?) {
            super.viewWillMove(toWindow: newWindow)
            if newWindow !== covered { restore() }
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window, window !== covered else { return }
            before = window.sharingType
            covered = window
            window.sharingType = .none
        }

        func restore() {
            covered?.sharingType = before
            covered = nil
        }
    }
}
