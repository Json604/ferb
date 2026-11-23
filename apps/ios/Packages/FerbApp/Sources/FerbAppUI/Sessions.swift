import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI
import UIKit

// The owner's sessions on his Mac (kb HAND-691, HAND-651; Android's ui/Sessions.kt): the Sessions tab, one session, the
// Yes before FERB types for him, Hand to FERB, a new terminal and the session's window. The words are the app's (the
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
                     empty: c.problem?.text ?? (loaded ? L.s("sessions_none") : nil),
                     soon: c.soon ? L.s("sessions_coming") : nil, waiting: waiting,
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

/// The session's page from the model (the screenshots draw it too): Watch and Hand to FERB but on a session FERB reads
/// no words of (HAND-710), Stop its turn while it isn't idle, Stop controlling while this phone controls it; the box while
/// something can deliver to it. A session FERB reads no words of shows its window live in place of the conversation,
/// the note on what a send does, and "Type here" (HAND-710).
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
        let actions = Self.actions(model: model, open: open)
        let cards = open.cards
        // a day's start between days, as the main chat's earlier days (PHN-813)
        let today = ChatModel.chatDay(Date(), Calendar.current)
        let lines = SessionWords.lines(open.items, today: today, markdown: SessionWords.markdown(s)) { dayLabel($0, today: today) }
        let canSend = s == nil || s?.canSend == true
        let note = s?.ferbNote ?? ""
        // his words from the moment he tapped Send: sending, then his bubble until the conversation has it (PHN-812)
        let sending: SessionSending? = open.outgoing.flatMap { o in
            switch o.state {
            case .sending: SessionSending(text: o.text, status: L.s("outbox_sending"))
            case .sent: SessionSending(text: o.text, status: nil)
            case .notSent: nil
            }
        }
        let window = open.watch ? Self.window(model: model) : nil
        return SessionPage(header: header, lines: lines, asks: cards.map { SessionWords.ask($0, agent: agent) },
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
                           sending: sending,
                           moreLabel: L.s("sessions_more"), menuOpen: open.menu, onMenu: { model.sessionMenu($0) },
                           newMessage: L.s("chat_new_message"), toLatest: L.s("chat_to_latest"),
                           // the first page on its way: the main chat's waiting look, not an empty page (PHN-816)
                           scrolls: !still, waiting: open.loaded ? nil : L.s("sessions_loading"),
                           // FERB's word on his hand-over, verbatim (HAND-702): above the box, or under the header without one
                           note: note.isEmpty ? nil : note, noteOnTop: !canSend, window: window) {
            if canSend {
                let sendingNow = sending?.status != nil
                SessionComposer(value: model.coding.draft,
                                placeholder: open.watch ? L.s("sessions_type_here") : L.s("sessions_message_hint", agent.isEmpty ? header.title : agent),
                                sendLabel: L.s("sessions_send"),
                                text: still ? nil : Binding(get: { model.coding.draft }, set: { model.sessionTyped($0) }),
                                onSend: busy.contains("session-send") || sendingNow ? nil : { model.sendToSession() },
                                sending: sendingNow)
            } else if let s {
                Text(s.noSend).type(14.5, .regular, Ink.onDark).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6).padding(.vertical, 10)
            }
        }
    }
}

extension SessionScreenPage {
    /// The menu: Watch and Hand to FERB (or Take it back), but not on a session FERB reads no words of (HAND-710: its
    /// screen is its Watch already, and only Claude Code and Codex are handed to FERB); Stop its turn while it isn't
    /// idle; Stop controlling while this phone controls it.
    static func actions(model: AppModel, open: OpenSession) -> [SessionAction] {
        let s = open.session
        let busy = model.ui.busy
        var actions: [SessionAction] = []
        if !open.watch {
            actions.append(SessionAction(L.s("sessions_watch"), symbol: "eye", onClick: { model.watchCodingSession() }))
            actions.append(handAction(model: model, open: open))
        }
        if let s, s.status != "idle" {
            actions.append(SessionAction(L.s("sessions_stop_turn"), symbol: "stop.fill",
                                         onClick: busy.contains("session-stop") ? nil : { model.stopSessionTurn() }))
        }
        if s?.controlledHere == true {
            actions.append(SessionAction(L.s("sessions_let_go"), symbol: "rectangle.portrait.and.arrow.right",
                                         onClick: busy.contains("session-release") ? nil : { model.releaseSession() }))
        }
        return actions
    }

    /// HAND-710: the session's window as Watch shows it (its newest picture, else its waiting words or why it ended, with
    /// Watch again), in the owner's screenshot shield (PHN-810, PHN-811), and the note on what a send does.
    static func window(model: AppModel) -> SessionWindow {
        let v = model.live.view
        let image = v?.frame.flatMap { UIImage(data: $0) }.map { Image(uiImage: $0) }
        let placeholder = v?.ended?.text ?? v?.empty ?? L.s("live_waiting")
        let line = v.flatMap { v in image != nil ? (v.ended?.text ?? v.line) : (v.ended == nil ? v.line : nil) }
        return SessionWindow(frame: image, placeholder: placeholder, line: line,
                             again: v?.ended != nil ? L.s("live_again") : nil, onAgain: { model.watchSessionAgain() },
                             note: L.s("sessions_type_note"), shieldFrame: { AnyView($0.screenshotShielded()) })
    }

    /// Hand to FERB while FERB doesn't have it; Take it back while it does (its task's id: the task stop, owner
    /// 2026-11-23), "Taking it back…" while that is on its way; "FERB is starting…" (no tap) between his hand-over and
    /// FERB's task.
    static func handAction(model: AppModel, open: OpenSession) -> SessionAction {
        let s = open.session
        if let s, !s.ferbTask.isEmpty {
            if open.takingBack {
                return SessionAction(L.s("sessions_taking_back"), symbol: "arrow.uturn.backward", busy: true, stays: true, onClick: nil)
            }
            return SessionAction(L.s("sessions_take_back"), symbol: "arrow.uturn.backward", stays: true, onClick: { model.takeSessionBack() })
        }
        if s?.withFerb == true { return SessionAction(L.s("sessions_ferb_starting"), symbol: nil, busy: true, onClick: nil) }
        return SessionAction(L.s("sessions_hand_over"), symbol: "hand.raised", onClick: { model.askHandToFerb() })
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

/// Hand this session to FERB (ORCH-687): the owner's goal, what FERB will do, Hand it over (signed). At the tap the button
/// says "Handing it to FERB…" until the Mac answers; refused, the Mac's reason under it and his words kept.
struct SessionHandSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    @State private var goal = ""

    var body: some View {
        let back = { model.backToSession() }
        let handing = model.coding.handing
        let ready = !goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !model.ui.busy.contains("session-hand")
        Sheet(bottomInset: insets.bottom, onDismiss: handing ? nil : back) {
            SheetTitle(L.s("sessions_hand_title"), onClose: handing ? nil : back)
            Text(L.s("sessions_hand_label")).type(15.5, .semibold, Ink.black).padding(.top, 12)
            Field(text: $goal, hint: L.s("sessions_hand_hint")).padding(.top, 8).disabled(handing).opacity(handing ? 0.55 : 1)
            Text(L.s("sessions_hand_note")).type(14, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 12)
            if handing {
                BusyPill(title: L.s("sessions_hand_going")).padding(.top, 18)
            } else {
                Pill(title: L.s("sessions_hand_go"), kind: .black, onClick: ready ? { model.handToFerb(goal: goal) } : nil).padding(.top, 18)
            }
            if let why = model.coding.handRefused, !handing { SheetRefusal(why.text).padding(.top, 10) }
        }
    }
}

/// New session (HAND-691): a terminal on the Mac. Only where (the Mac's list, or "Another folder…": the Mac's Files
/// browser for folders, PHN-821) and an optional command; with a command, the Mac's exact command and the owner's Yes.
/// No agent to pick: he types the one he wants. Start shows "Opening a terminal…" at once (owner 2026-11-23, PHN-822).
struct NewSessionSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    /// A still picture (the screenshots): no scroll view and no text field.
    var still = false

    var body: some View {
        if let n = model.coding.new {
            if let p = n.picker {
                FolderPickerSheet(model: model, picker: p, insets: insets, still: still)
            } else {
                NewSessionSheetView(
                    words: NewSessionWords(title: L.s("sessions_new"), folder: L.s("sessions_new_folder"), other: L.s("sessions_new_browse"),
                                           command: L.s("sessions_new_command"), commandHint: L.s("sessions_new_command_hint"),
                                           start: L.s("sessions_start"), starting: L.s("sessions_opening")),
                    folders: n.folders.map { f in
                        SessionFolderRow(title: nameOf(f) == L.s("search_mac") ? f : nameOf(f), detail: f, checked: f == n.folder)
                    },
                    loading: n.loading, command: n.text,
                    commandText: still ? nil : Binding(get: { model.coding.new?.text ?? "" }, set: { model.newSessionText($0) }),
                    starting: n.starting, refused: n.refused?.text, insets: insets, scrolls: !still,
                    onFolder: { i in if i < n.folders.count { model.newSessionFolder(n.folders[i]) } },
                    onOther: { model.pickNewSessionFolder() },
                    onStart: n.chosen == nil || model.ui.busy.contains("session-prepare") ? nil : { model.startSession() },
                    onClose: { model.closeNewSession() })
            }
        }
    }
}

/// "Another folder…" (PHN-821): the Files tab's browser over the Mac's files, folders only; Use this folder chooses the
/// folder shown, Close goes back to the form as it was.
struct FolderPickerSheet: View {
    @Bindable var model: AppModel
    let picker: FolderPicker
    let insets: PhoneInsets
    var still = false

    var body: some View {
        // the Files tab's places and the home folder they are in (a project folder is often elsewhere under it)
        let places: [(String, String, String)] = [("~", "sessions_browse_home", "house"),
                                                  (AppModel.places[0], "place_desktop", "menubar.dock.rectangle"),
                                                  (AppModel.places[1], "place_documents", "doc"),
                                                  (AppModel.places[2], "place_downloads", "arrow.down.circle")]
        let f = picker.folder
        let folders = picker.folders
        let rows: [SessionFolderRow] = f == nil
            ? places.map { p in
                SessionFolderRow(title: L.s(p.1), detail: picker.loading == p.0 ? L.s("files_opening") : nil, symbol: p.2, opens: true)
            }
            : folders.map { e in
                SessionFolderRow(title: e.name, detail: picker.loading == e.path ? L.s("files_opening") : (e.locked ? L.bio("files_key_place") : nil),
                                 locked: e.locked, opens: !e.locked)
            }
        let line: String? = picker.problem?.text ?? (f != nil && folders.isEmpty && picker.loading == nil ? L.s("sessions_browse_no_folders") : nil)
        SessionFolderPickerView(
            title: L.s("sessions_browse_title"), folder: f.map { nameOf($0.path) }, backLabel: L.s("files_back"),
            rows: rows, recentLabel: L.s("files_recent"),
            recent: f == nil ? picker.recent.map { SessionFolderRow(title: nameOf($0), detail: $0, opens: true) } : [],
            line: line, path: f?.path ?? "", use: L.s("sessions_browse_use"), insets: insets, scrolls: !still,
            onUp: { model.pickerUp() },
            onRow: { i in
                if f == nil { if i < places.count { model.pickerOpen(places[i].0) } }
                else if i < folders.count { model.pickerOpen(folders[i].path, keyPlace: folders[i].locked) }
            },
            onRecent: { i in if i < picker.recent.count { model.pickerOpen(picker.recent[i]) } },
            onUse: f == nil ? nil : { model.pickerUse() },
            onClose: { model.pickerClose() })
    }
}

/// Watch: the session's window live (Watch live's page and rules: memory only, screenshots as the owner's switch says,
/// not on the shared relay; PHN-810).
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
                         // the picture of the Mac out of screenshots while the owner's switch is on (PHN-810, PHN-811)
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
