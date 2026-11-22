import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI
import UIKit

// The owner's coding sessions on his Mac (kb HAND-651; Android's ui/Sessions.kt): the Sessions tab, one session, the Yes
// before FERB types for him, Hand to FERB, a new session and the session's window. The words are the app's (the
// catalog, the same English as FERB.app); the agent's own words are shown as plain text. The polls run only while these
// show (the list every 3 s, the open session every 2 s); the model also checks the app is unlocked and in front.

/// The Sessions tab: the off card with Turn on, the list (or its empty line, or why it couldn't be read), New session.
struct SessionsView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let waiting: PhoneTab?

    var body: some View {
        SessionsTabPage(model: model, insets: insets, waiting: waiting)
            .task {
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: UInt64(CodingSessions.listEvery * 1e9))
                    if Task.isCancelled { break }
                    await model.sessionsTick()
                }
            }
    }
}

/// The tab's page from the model (the screenshots draw it too).
struct SessionsTabPage: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let waiting: PhoneTab?

    var body: some View {
        let c = model.coding
        let sessions = c.list?.sessions ?? []
        let loaded = c.list != nil
        SessionsPage(label: L.s("sessions_title"), rows: sessions.map(SessionWords.row), newLabel: L.s("sessions_new"), insets: insets,
                     off: loaded && c.list?.enabled == false
                         ? SessionsOff(title: L.s("sessions_off"), text: L.s("sessions_off_more"), turnOn: L.s("sessions_turn_on")) : nil,
                     empty: c.problem?.text ?? (loaded ? L.s("sessions_none") : nil), waiting: waiting,
                     onTab: { model.choose($0) }, onProfile: { model.show(.profile) },
                     onRow: { i in if i < sessions.count { model.openSession(sessions[i].id) } },
                     onNew: { model.newSession() },
                     onTurnOn: model.ui.busy.contains("sessions-on") ? nil : { model.turnOnSessions() })
    }
}

/// One session's screen (captures as the owner's switch says, PHN-800), with its poll.
struct SessionScreen: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        if let o = model.coding.open {
            SessionScreenPage(model: model, open: o, insets: insets)
                // opened over the page, it rises and fades in, and fades out (kb PHN-623)
                .presenceFade()
                .task(id: o.id) {
                    while !Task.isCancelled {
                        try? await Task.sleep(nanoseconds: UInt64(CodingSessions.openEvery * 1e9))
                        if Task.isCancelled { break }
                        await model.sessionsTick()
                    }
                }
        }
    }
}

/// The session's page from the model (the screenshots draw it too): Watch and Hand to FERB always, Stop its turn while
/// it isn't idle, Stop controlling while this phone controls it; the box while something can deliver to it.
struct SessionScreenPage: View {
    @Bindable var model: AppModel
    let open: OpenSession
    let insets: PhoneInsets
    /// A still picture (the screenshots): no scroll view and no text field, which a still picture cannot draw.
    var still = false

    var body: some View {
        let s = open.session
        let busy = model.ui.busy
        let header = s.map(SessionWords.row) ?? SessionRowData(title: "", subtitle: "", state: .idle, stateText: "")
        let agent = s?.agentName ?? ""
        var actions = [SessionAction(L.s("sessions_watch"), symbol: "eye", onClick: { model.watchCodingSession() }),
                       SessionAction(L.s("sessions_hand_over"), symbol: "hand.raised", onClick: { model.askHandToFerb() })]
        if let s, s.status != "idle" {
            actions.append(SessionAction(L.s("sessions_stop_turn"), symbol: "stop.fill",
                                         onClick: busy.contains("session-stop") ? nil : { model.stopSessionTurn() }))
        }
        if s?.controlledHere == true {
            actions.append(SessionAction(L.s("sessions_let_go"), symbol: "rectangle.portrait.and.arrow.right",
                                         onClick: busy.contains("session-release") ? nil : { model.releaseSession() }))
        }
        let cards = open.cards
        return SessionPage(header: header, lines: open.items.map(SessionWords.line), asks: cards.map { SessionWords.ask($0, agent: agent) },
                           actions: actions, insets: insets, older: open.more ? L.s("sessions_older") : nil,
                           onOlder: busy.contains("session-earlier") ? nil : { model.sessionEarlier() },
                           onClose: { model.closeSession() },
                           onChoice: { a, c in
                               guard a < cards.count, c < cards[a].choices.count, !busy.contains("session-answer:\(cards[a].id)") else { return }
                               model.answerSession(card: cards[a].id, choice: cards[a].choices[c].id)
                           },
                           unsent: open.unsent.map {
                               SessionUnsent(text: $0.text, reason: $0.reason.text, retry: L.s("sessions_try_again"), notSent: L.s("sessions_not_sent"))
                           },
                           onRetry: busy.contains("session-send") ? nil : { model.retrySessionSend() },
                           moreLabel: L.s("sessions_more"), menuOpen: open.menu, onMenu: { model.sessionMenu($0) },
                           scrolls: !still) {
            if s == nil || s?.canSend == true {
                SessionComposer(value: model.coding.draft, placeholder: L.s("sessions_message_hint", agent.isEmpty ? header.title : agent),
                                sendLabel: L.s("sessions_send"),
                                text: still ? nil : Binding(get: { model.coding.draft }, set: { model.sessionTyped($0) }),
                                onSend: busy.contains("session-send") ? nil : { model.sendToSession() })
            } else if let s {
                Text(s.noSend).type(14.5, .regular, Ink.onDark).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6).padding(.vertical, 10)
            }
        }
    }
}

/// The Mac's question before FERB types for the owner or starts a session: Yes, send it (Start for a new session).
struct SessionYesView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        if let ask = model.coding.asking {
            let starting = if case .start = ask { true } else { false }
            SessionYesSheet(question: ask.yes.question, detail: ask.yes.detail, covers: ask.yes.covers,
                            yes: L.s(starting ? "sessions_start" : "sessions_yes_send"), cancel: L.s("sessions_cancel"), yesSymbol: Words.bioSymbol,
                            insets: insets, onYes: model.ui.busy.contains("session-yes") ? nil : { model.sessionYes() },
                            onCancel: { model.sessionNo() })
        }
    }
}

/// HAND-683: FERB can't tell which terminal is the session's: the Mac's reason, the terminals it can see in the app by
/// the app's own words (the one in front now first), each a choice signed as control is, and Don't send.
struct PickTerminalView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        if let pick = model.coding.open?.pick {
            PickTerminalSheet(title: L.s("sessions_pick_title"), why: pick.why, terminals: pick.terminals.map { ($0.id, $0.label) },
                              note: L.s("sessions_pick_note"), no: L.s("sessions_pick_no"), yesSymbol: Words.bioSymbol, insets: insets,
                              onPick: model.ui.busy.contains("session-pick") ? nil : { model.pickTerminal($0) },
                              onNo: { model.pickNoTerminal() })
        }
    }
}

/// Hand this session to FERB (ORCH-687): the owner's goal, what FERB will do, Hand it over (signed).
struct SessionHandSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    @State private var goal = ""

    var body: some View {
        let back = { model.backToSession() }
        let ready = !goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !model.ui.busy.contains("session-hand")
        Sheet(bottomInset: insets.bottom, onDismiss: back) {
            SheetTitle(L.s("sessions_hand_title"), onClose: back)
            Text(L.s("sessions_hand_label")).type(15.5, .semibold, Ink.black).padding(.top, 12)
            Field(text: $goal, hint: L.s("sessions_hand_hint")).padding(.top, 8)
            Text(L.s("sessions_hand_note")).type(14, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 12)
            Pill(title: L.s("sessions_hand_go"), kind: .black, onClick: ready ? { model.handToFerb(goal: goal) } : nil).padding(.top, 18)
        }
    }
}

/// New session (HAND-665): which agent, which folder (the Mac's list), the first words; then the Mac's exact command and
/// the owner's Yes.
struct NewSessionSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        let close = { model.closeNewSession() }
        if let n = model.coding.new {
            let agentName = CodingSessions.agents.first { $0.id == n.agent }?.name ?? ""
            Sheet(bottomInset: insets.bottom, onDismiss: close) {
                SheetTitle(L.s("sessions_new"), onClose: close)
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(L.s("sessions_new_agent")).type(15.5, .semibold, Ink.black).padding(.top, 12).padding(.bottom, 8)
                        HStack(spacing: 8) {
                            ForEach(CodingSessions.agents, id: \.id) { a in
                                Pill(title: a.name, kind: n.agent == a.id ? .black : .grey, height: 44) { model.newSessionAgent(a.id) }
                                    .accessibilityAddTraits(n.agent == a.id ? .isSelected : [])
                            }
                        }
                        Text(L.s("sessions_new_folder")).type(15.5, .semibold, Ink.black).padding(.top, 16).padding(.bottom, 8)
                        if !n.folders.isEmpty {
                            // the sheet's grey fill for the folders: a white card would vanish on the white sheet
                            VStack(spacing: 0) {
                                ForEach(Array(n.folders.enumerated()), id: \.offset) { i, f in
                                    if i > 0 { Divide() }
                                    CardRow(title: (f as NSString).lastPathComponent.isEmpty ? f : (f as NSString).lastPathComponent, detail: f,
                                            onClick: { model.newSessionFolder(f) }) {
                                        RowIcon(symbol: "folder")
                                    } trailing: {
                                        if f == n.folder { Image(systemName: "checkmark").font(.system(size: 17, weight: .semibold)).foregroundStyle(Ink.black) }
                                    }
                                    .accessibilityAddTraits(f == n.folder ? .isSelected : [])
                                }
                            }
                            .background(Ink.fill, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                        } else if n.loading {
                            ProgressView().frame(maxWidth: .infinity)
                        }
                        Text(L.s("sessions_new_first")).type(15.5, .semibold, Ink.black).padding(.top, 16).padding(.bottom, 8)
                        Field(text: Binding(get: { model.coding.new?.text ?? "" }, set: { model.newSessionText($0) }),
                              hint: L.s("sessions_message_hint", agentName))
                    }
                }
                .frame(maxHeight: 520)
                .fixedSize(horizontal: false, vertical: true)
                Pill(title: L.s("sessions_start"), kind: .black,
                     onClick: n.folder == nil || model.ui.busy.contains("session-prepare") ? nil : { model.startSession() })
                    .padding(.top, 18)
            }
        }
    }
}

/// Watch: the session's window live (Watch live's page and rules: memory only, out of captures, not on the shared relay).
struct SessionWatchView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        if let v = model.live.view {
            let name = model.coding.open?.session.map(SessionWords.title) ?? ""
            let image = v.frame.flatMap { UIImage(data: $0) }.map { Image(uiImage: $0) }
            let placeholder = v.ended?.text ?? v.empty ?? L.s("live_waiting")
            let line = image != nil ? (v.ended?.text ?? v.line) : (v.ended == nil ? v.line : nil)
            ZStack(alignment: .bottom) {
                LivePage(title: L.s("sessions_watch_title", name), frame: image, placeholder: placeholder, line: line,
                         closeTitle: L.s("live_close"), insets: insets, onClose: { model.closeLive() },
                         // the picture of the Mac out of screenshots where this iOS allows it (PHN-597)
                         shieldFrame: { AnyView($0.screenshotShielded()) }, stops: false)
                if v.ended != nil {
                    Pill(title: L.s("live_again"), kind: .grey, symbol: "eye", height: 46) { model.watchSessionAgain() }
                        .padding(.horizontal, 16).padding(.bottom, insets.bottom + 62)
                }
            }
            .presenceFade()
        }
    }
}
