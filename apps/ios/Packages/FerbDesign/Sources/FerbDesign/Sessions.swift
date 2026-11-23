import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// NOT FROM THE LAB. The owner's coding sessions on his Mac (Claude Code, Codex; kb HAND-651): the Sessions tab and one
// session's screen, the twins of Android's design (components/Sessions.kt, screens/SessionScreens.kt). Made only of the
// lab's own pieces and numbers (TopBar, Card, RowIcon, Opens, Pill, SkyLabel, OwnerBubble, ExactCommand, SheetIcon,
// Sheet, SheetTitle, CloseButton, LampDot) until the lab draws them. Stateless: data and callbacks are parameters, the
// words are the app's (its catalog). The agent's words are data: plain text only, never a link or markup; a tool row
// and the exact detail of a question are fixed-width.

/// What a session is doing (`status`): working, waiting for the owner, or idle.
public enum SessionState: Sendable { case working, waiting, idle }

/// One session as its row shows it: its title (the agent's own, else the folder's name), "<folder> · in <app>", its
/// state's words, and "FERB is on it" while FERB keeps it going.
public struct SessionRowData: Equatable, Sendable {
    public var title: String
    public var subtitle: String
    public var state: SessionState
    public var stateText: String
    public var ferb: String?
    public init(title: String, subtitle: String, state: SessionState, stateText: String, ferb: String? = nil) {
        self.title = title
        self.subtitle = subtitle
        self.state = state
        self.stateText = stateText
        self.ferb = ferb
    }
}

/// The state as a small chip: a turning arc while it works, amber while it waits for the owner, quiet when idle (the
/// words always say it: colour is never the only sign).
public struct SessionChip: View {
    let state: SessionState
    let text: String
    @Environment(\.onFerb) private var onFerb
    public init(_ state: SessionState, _ text: String) { self.state = state; self.text = text }
    public var body: some View {
        let card = CardInk(onFerb)
        let ink: Color = switch state {
        case .waiting: Ink.black
        case .working: card.primary
        case .idle: card.secondary
        }
        HStack(spacing: 6) {
            switch state {
            case .working: SessionArc(color: ink).frame(width: 12, height: 12)
            case .waiting: Circle().fill(Ink.black).frame(width: 7, height: 7)
            case .idle: Circle().fill(LabColors.waitingRing).frame(width: 7, height: 7)
            }
            Text(text).type(12.5, .semibold, ink).lineLimit(1).fixedSize()
        }
        .padding(.leading, 8).padding(.trailing, 10)
        .frame(minHeight: 24)
        .background(state == .waiting ? Ink.amber : card.fill, in: Capsule())
    }
}

/// Mark's running arc, small: it turns while the session works.
struct SessionArc: View {
    let color: Color
    var body: some View {
        MotionClock { t in
            let turn = t.truncatingRemainder(dividingBy: 0.8) / 0.8
            Circle().trim(from: 0, to: 0.3)
                .stroke(color, style: StrokeStyle(lineWidth: 1.8, lineCap: .round))
                .rotationEffect(.degrees(turn * 360))
                .padding(1)
        }
        .accessibilityHidden(true)
    }
}

/// "FERB is on it": FERB keeps this session going for the owner (a hand-over).
public struct FerbBadge: View {
    let text: String
    @Environment(\.onFerb) private var onFerb
    public init(_ text: String) { self.text = text }
    public var body: some View {
        HStack(spacing: 6) {
            LampDot(color: Ink.amber, size: 7)
            Text(text).type(12.5, .semibold, onFerb ? Ink.black : Ink.white).lineLimit(1).fixedSize()
        }
        .padding(.leading, 8).padding(.trailing, 10)
        .frame(minHeight: 24)
        .background(onFerb ? Ink.white : Ink.black, in: Capsule())
    }
}

/// A session's row in the Sessions tab's card: the terminal's mark, its title and place, its state and FERB's badge,
/// and it opens.
public struct SessionRow: View {
    let row: SessionRowData
    var onClick: (() -> Void)?
    @Environment(\.onFerb) private var onFerb
    public init(_ row: SessionRowData, onClick: (() -> Void)? = nil) { self.row = row; self.onClick = onClick }
    public var body: some View {
        let ink = CardInk(onFerb)
        HStack(spacing: 14) {
            RowIcon(symbol: "terminal").frame(width: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(row.title).type(16.5, .semibold, ink.primary).lineLimit(2)
                Text(row.subtitle).type(14, .regular, ink.secondary).lineLimit(1)
                HStack(spacing: 6) {
                    SessionChip(row.state, row.stateText)
                    if let f = row.ferb { FerbBadge(f) }
                }
                .padding(.top, 6)
            }
            Spacer(minLength: 8)
            Opens()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 13)
        .pressable(onClick)
        .accessibilityElement(children: .combine)
    }
}

/// Sessions are off on the Mac: what FERB would do, and the one button that turns them on.
public struct SessionsOff: Equatable, Sendable {
    public var title: String
    public var text: String
    public var turnOn: String
    public init(title: String, text: String, turnOn: String) { self.title = title; self.text = text; self.turnOn = turnOn }
}

/// The Sessions tab (Android's SessionsPage): its label on the sky, the sessions in one white card, and New session at
/// the foot. Off: what turning it on does and Turn on. None: `empty`.
public struct SessionsPage: View {
    var label: String
    var rows: [SessionRowData]
    var newLabel: String
    var insets: PhoneInsets
    var off: SessionsOff?
    var empty: String?
    /// A new terminal started but not in the list yet: one quiet line until it shows.
    var soon: String?
    var waiting: PhoneTab?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onRow: ((Int) -> Void)?
    var onNew: (() -> Void)?
    var onTurnOn: (() -> Void)?

    public init(label: String, rows: [SessionRowData], newLabel: String, insets: PhoneInsets = .lab, off: SessionsOff? = nil,
                empty: String? = nil, soon: String? = nil, waiting: PhoneTab? = nil, onTab: ((PhoneTab) -> Void)? = nil,
                onProfile: (() -> Void)? = nil, onRow: ((Int) -> Void)? = nil, onNew: (() -> Void)? = nil,
                onTurnOn: (() -> Void)? = nil) {
        self.label = label
        self.rows = rows
        self.newLabel = newLabel
        self.insets = insets
        self.off = off
        self.empty = empty
        self.soon = soon
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.onRow = onRow
        self.onNew = onNew
        self.onTurnOn = onTurnOn
    }

    @Environment(\.skyInk) private var ink

    public var body: some View {
        VStack(spacing: 0) {
            TopBar(chosen: .sessions, waiting: waiting, onTab: onTab, onProfile: onProfile)
            FitOrScroll {
                VStack(spacing: 0) {
                    SkyLabel(text: label)
                    if off == nil, let soon {
                        HStack(spacing: 8) {
                            SessionArc(color: ink.low).frame(width: 12, height: 12)
                            Text(soon).type(14.5, .medium, ink.low).fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 8).padding(.bottom, 10)
                        .accessibilityElement(children: .combine)
                        .accessibilityAddTraits(.updatesFrequently)
                    }
                    if let off {
                        Card(padding: 18) {
                            VStack(alignment: .leading, spacing: 10) {
                                Text(off.title).type(17, .semibold, Ink.black).fixedSize(horizontal: false, vertical: true)
                                Text(off.text).type(15, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                                Pill(title: off.turnOn, kind: .black, onClick: onTurnOn).padding(.top, 6)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    } else if rows.isEmpty {
                        if let empty {
                            Text(empty).type(15.5, .medium, ink.low).fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 8).padding(.top, 4)
                        }
                    } else {
                        Card {
                            ForEach(Array(rows.enumerated()), id: \.offset) { i, row in
                                if i > 0 { Divide() }
                                SessionRow(row, onClick: onRow.map { f in { f(i) } })
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, -6)
                .padding(.bottom, 16)
            }
            if off == nil {
                Pill(title: newLabel, kind: .black, symbol: "plus", onClick: onNew)
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
                    .padding(.bottom, insets.bottom + 8)
            }
        }
    }
}

// MARK: One session

/// What a line of a session's conversation is: the owner's words, the agent's (plain), the agent's reply drawn as one
/// block of parts (HAND-703: its Markdown, thoughts, steps and meta line), one tool it ran, a stop, an error, a day's
/// start (PHN-813).
public enum SessionLineKind: Sendable { case owner, agent, reply, tool, interrupted, error, day }

/// One line of the conversation (`key` stays the same while it is on screen: its sequence number). A reply's `parts`
/// are drawn in order in one block; `text` is then what a screen reader and Copy fall back to.
public struct SessionLine: Equatable, Sendable {
    public var key: String
    public var kind: SessionLineKind
    public var text: String
    public var parts: [SessionPartShown]
    /// HAND-709: an owner's-side line FERB sent while it kept the session going: its small label ("Sent by FERB").
    public var by: String?
    public init(_ key: String, _ kind: SessionLineKind, _ text: String, parts: [SessionPartShown] = [], by: String? = nil) {
        self.key = key; self.kind = kind; self.text = text; self.parts = parts; self.by = by
    }
}

/// One line on the session screen's black page: the owner's words on white at the right, the agent's on the dark fill
/// at the left, a tool it ran as one small fixed-width line, its turn stopped and an error as quiet lines.
public struct SessionLineView: View {
    let line: SessionLine
    public init(_ line: SessionLine) { self.line = line }
    public var body: some View {
        switch line.kind {
        case .owner:
            if let by = line.by {
                // HAND-709: what FERB sent for the owner, on his side, marked as FERB's
                VStack(alignment: .trailing, spacing: 3) {
                    Text(by).type(12, .medium, Ink.onDark).padding(.trailing, 6)
                    OwnerBubble(line.text)
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
                .accessibilityElement(children: .combine)
            } else {
                OwnerBubble(line.text)
            }
        // HAND-703: one reply block, the main chat's FERB-reply look on the black page (no bubble per line)
        case .reply: SessionReplyView(key: line.key, parts: line.parts)
        case .agent:
            // selectable, as FERB's words in the main chat are (PHN-813)
            Text(line.text).type(15.5, .regular, Ink.white).lineSpacing(2).textSelection(.enabled)
                .padding(.horizontal, 16).padding(.vertical, 11)
                .background(Ink.darkFill, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 6, bottomTrailingRadius: 22,
                                                                      topTrailingRadius: 22, style: .continuous))
                .frame(maxWidth: 320, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .tool:
            Text(line.text).font(.system(size: 13, design: .monospaced)).foregroundStyle(Ink.onDark).lineLimit(3)
                .padding(.horizontal, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .interrupted:
            Text(line.text).type(13.5, .medium, Ink.onDark).multilineTextAlignment(.center)
                .padding(.horizontal, 6)
                .frame(maxWidth: .infinity, alignment: .center)
        case .error:
            HStack(spacing: 8) {
                LampDot(color: Lamp.red, size: 7)
                Text(line.text).type(14, .regular, Lamp.red).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 6)
        // a day's start, as the main chat's DayDivider, in the black page's quiet ink (PHN-813)
        case .day:
            Text(line.text).type(13, .semibold, Ink.onDark)
                .frame(maxWidth: .infinity)
                .padding(.top, 6).padding(.bottom, 2)
                .accessibilityAddTraits(.isHeader)
        }
    }
}

/// The agent waits for the owner (a permission prompt or a question): what it asks, the exact thing in a fixed-width
/// face, one button per answer it offers. No answers: `onMac` says to answer it on the Mac.
public struct SessionAsk: Equatable, Sendable {
    public var title: String
    public var question: String
    public var detail: String
    public var choices: [String]
    public var onMac: String
    public var key: String
    public init(title: String, question: String, detail: String, choices: [String], onMac: String, key: String? = nil) {
        self.title = title
        self.question = question
        self.detail = detail
        self.choices = choices
        self.onMac = onMac
        self.key = key ?? title
    }
}

public struct SessionAskCard: View {
    let ask: SessionAsk
    var onChoice: ((Int) -> Void)?
    public init(_ ask: SessionAsk, onChoice: ((Int) -> Void)? = nil) { self.ask = ask; self.onChoice = onChoice }
    public var body: some View {
        Card(padding: 16) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 12) {
                    SheetIcon(symbol: "terminal")
                    Text(ask.title).type(17, .semibold, Ink.black).fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                if !ask.question.trimmingCharacters(in: .whitespaces).isEmpty {
                    Text(ask.question).type(15.5, .regular, Ink.black).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                }
                if !ask.detail.trimmingCharacters(in: .whitespaces).isEmpty { ExactCommand(ask.detail) }
                if ask.choices.isEmpty {
                    Text(ask.onMac).type(14.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                } else {
                    ForEach(Array(ask.choices.enumerated()), id: \.offset) { i, label in
                        Pill(title: label, kind: i == 0 ? .black : .grey, height: 46, onClick: onChoice.map { f in { f(i) } })
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// One of the session screen's actions, in its menu (Watch, Hand to FERB or Take it back, Stop its turn, Stop
/// controlling). `busy`: a small turning arc in place of its icon while what it started is on its way (its words say
/// so, and it takes no tap); `stays`: the menu stays open on its tap, for the result to show in its row.
public struct SessionAction {
    public let label: String
    public let symbol: String?
    public let onClick: (() -> Void)?
    public let busy: Bool
    public let stays: Bool
    public init(_ label: String, symbol: String?, busy: Bool = false, stays: Bool = false, onClick: (() -> Void)?) {
        self.label = label
        self.symbol = symbol
        self.busy = busy
        self.stays = stays
        self.onClick = onClick
    }
}

/// The session screen's buttons: small dark pills in one row, in two when they don't fit (Android's row scrolls
/// sideways; here they all stay in view, and a still picture draws them).
public struct SessionActions: View {
    let actions: [SessionAction]
    public init(_ actions: [SessionAction]) { self.actions = actions }
    public var body: some View {
        let half = (actions.count + 1) / 2
        ViewThatFits(in: .horizontal) {
            row(actions[...])
            VStack(alignment: .leading, spacing: 8) {
                row(actions[..<half])
                row(actions[half...])
            }
        }
        .padding(.horizontal, 16)
        .ferbSurface()
    }
    private func row(_ list: ArraySlice<SessionAction>) -> some View {
        HStack(spacing: 8) {
            ForEach(Array(list.enumerated()), id: \.offset) { _, a in
                Pill(title: a.label, kind: .grey, symbol: a.symbol, height: 40, fill: false, onClick: a.onClick)
            }
            Spacer(minLength: 0)
        }
    }
}

/// A message the Mac refused (PHN-800): the owner's words, dimmed, never as sent; why, in the Mac's plain words; and
/// `retry` to send them again.
public struct SessionUnsent: Equatable, Sendable {
    public var text: String
    public var reason: String
    public var retry: String
    public var notSent: String
    public init(text: String, reason: String, retry: String, notSent: String) {
        self.text = text
        self.reason = reason
        self.retry = retry
        self.notSent = notSent
    }
}

public struct SessionUnsentView: View {
    let unsent: SessionUnsent
    var onRetry: (() -> Void)?
    public init(_ unsent: SessionUnsent, onRetry: (() -> Void)? = nil) { self.unsent = unsent; self.onRetry = onRetry }
    public var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            OwnerBubble(unsent.text).opacity(0.5)
            HStack(alignment: .top, spacing: 8) {
                LampDot(color: Lamp.red, size: 7).padding(.top, 6)
                Text("\(unsent.notSent) \(unsent.reason)").type(14, .regular, Ink.onDark).lineSpacing(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.leading, 40).padding(.trailing, 6)
            Pill(title: unsent.retry, kind: .grey, height: 36, fill: false, onClick: onRetry).ferbSurface().padding(.trailing, 2)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// His words on their way into the session (owner 2026-11-23, kb PHN-812): his bubble, dimmed, the moment he taps Send,
/// with a small spinner and "Sending…" under it, as the main chat's outbox row says it (PHN-401); once sent, his bubble.
public struct SessionSending: Equatable, Sendable {
    public var text: String
    /// "Sending…" while on its way; nil once sent (his plain bubble until the conversation carries it).
    public var status: String?
    public init(text: String, status: String?) { self.text = text; self.status = status }
}

public struct SessionSendingView: View {
    let sending: SessionSending
    public init(_ sending: SessionSending) { self.sending = sending }
    public var body: some View {
        if let status = sending.status {
            VStack(alignment: .trailing, spacing: 4) {
                OwnerBubble(sending.text).opacity(0.7)
                HStack(spacing: 8) {
                    SessionArc(color: Ink.onDark).frame(width: 12, height: 12)
                    Text(status).type(13.5, .medium, Ink.onDark)
                }
                .padding(.horizontal, 6)
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.updatesFrequently)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        } else {
            OwnerBubble(sending.text)
        }
    }
}

/// The session's menu (PHN-800): where it runs, then its actions, one per row, in a white card.
public struct SessionMenu: View {
    let place: String
    let actions: [SessionAction]
    var onPicked: () -> Void
    public init(place: String, actions: [SessionAction], onPicked: @escaping () -> Void = {}) {
        self.place = place
        self.actions = actions
        self.onPicked = onPicked
    }
    public var body: some View {
        Card(padding: 6) {
            if !place.trimmingCharacters(in: .whitespaces).isEmpty {
                Text(place).type(13.5, .regular, Ink.detail).lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 6)
            }
            ForEach(Array(actions.enumerated()), id: \.offset) { i, a in
                if i > 0 || !place.trimmingCharacters(in: .whitespaces).isEmpty { Divide() }
                let ink = a.onClick == nil ? Ink.detail : Ink.black
                HStack(spacing: 12) {
                    ZStack {
                        if a.busy {
                            SessionArc(color: Ink.detail).frame(width: 16, height: 16)
                        } else if let s = a.symbol {
                            Image(systemName: s).font(.system(size: 16, weight: .medium))
                        }
                    }
                    .frame(width: 20)
                    Text(a.label).type(16, .medium, ink).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .foregroundStyle(ink)
                .padding(.horizontal, 14)
                .frame(minHeight: 48)
                .contentShape(Rectangle())
                .pressable(a.busy ? nil : a.onClick.map { f in { if !a.stays { onPicked() }; f() } }, label: a.label)
                .accessibilityAddTraits(a.busy ? .updatesFrequently : [])
            }
        }
        .frame(minWidth: 220, maxWidth: 280)
    }
}

/// The session header's menu button: an ellipsis in Close's grey round, white while its menu is open.
public struct MoreButton: View {
    let label: String
    let open: Bool
    var onClick: (() -> Void)?
    public init(label: String, open: Bool, onClick: (() -> Void)?) { self.label = label; self.open = open; self.onClick = onClick }
    public var body: some View {
        Image(systemName: "ellipsis").font(.system(size: 16, weight: .bold)).foregroundStyle(.black)
            .frame(width: 36, height: 36).background(open ? Ink.white : Ink.fill, in: Circle())
            .pressable(onClick, label: label)
    }
}

/// The session screen's message box (kb HAND-651): the owner's words to the agent, several lines, and Send. Never a
/// terminal's keyboard: what is sent is what the box holds. Without `text` it only shows `value` (screenshots).
public struct SessionComposer: View {
    let value: String
    let placeholder: String
    let sendLabel: String
    var text: Binding<String>?
    var onSend: (() -> Void)?
    /// A message is on its way (PHN-812): Send shows a small spinner and waits; the box stays free to type in.
    var sending: Bool
    public init(value: String, placeholder: String, sendLabel: String, text: Binding<String>? = nil, onSend: (() -> Void)? = nil,
                sending: Bool = false) {
        self.value = value
        self.placeholder = placeholder
        self.sendLabel = sendLabel
        self.text = text
        self.onSend = onSend
        self.sending = sending
    }
    public var body: some View {
        let shown = text?.wrappedValue ?? value
        let ready = !shown.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        HStack(alignment: .bottom, spacing: 10) {
            ZStack(alignment: .leading) {
                if let text {
                    TextField("", text: text, axis: .vertical).lineLimit(1...6).type(16.5, .regular, Ink.white).tint(Ink.onDark)
                        .accessibilityLabel(Text(placeholder))
                } else if !value.isEmpty {
                    Text(value).type(16.5, .regular, Ink.white).lineLimit(6)
                }
                if shown.isEmpty { Text(placeholder).type(16.5, .regular, Ink.onDark).lineLimit(1).allowsHitTesting(false) }
            }
            .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
            ZStack {
                if sending {
                    SessionArc(color: Ink.black).frame(width: 18, height: 18)
                } else {
                    Image(systemName: "arrow.up").font(.system(size: 17, weight: .semibold)).foregroundStyle(Ink.black)
                }
            }
            .frame(width: 40, height: 40).background(ready && !sending ? Ink.white : Ink.onDark, in: Circle())
            .pressable(ready && !sending ? onSend : nil, label: sendLabel)
        }
        .padding(.leading, 18).padding(.trailing, 8).padding(.vertical, 8)
        .frame(minHeight: 56)
        .background(Ink.darkFill, in: RoundedRectangle(cornerRadius: 28, style: .continuous))
    }
}

/// A session FERB reads none of the words of (HAND-710: a terminal, an app's window): its window, live, in place of the
/// conversation. The picture as Watch draws it (the newest frame, else its waiting words or why it ended), the line
/// over it, Watch again once it ended, and one quiet note on what a send does. `shieldFrame` is the app's screenshot
/// shield around the picture (PHN-810, PHN-811); nil draws it plainly.
public struct SessionWindow {
    public var frame: Image?
    public var placeholder: String
    public var line: String?
    public var again: String?
    public var onAgain: (() -> Void)?
    public var note: String
    public var shieldFrame: ((AnyView) -> AnyView)?
    public init(frame: Image? = nil, placeholder: String, line: String? = nil, again: String? = nil, onAgain: (() -> Void)? = nil,
                note: String, shieldFrame: ((AnyView) -> AnyView)? = nil) {
        self.frame = frame
        self.placeholder = placeholder
        self.line = line
        self.again = again
        self.onAgain = onAgain
        self.note = note
        self.shieldFrame = shieldFrame
    }
}

/// The window's picture sized to the room between the header and the message box, his words on their way or refused
/// under it (PHN-812, PHN-800), and the quiet note last, just above the box.
struct SessionWindowView: View {
    let window: SessionWindow
    let unsent: SessionUnsent?
    let onRetry: (() -> Void)?
    let sending: SessionSending?

    var body: some View {
        VStack(spacing: 10) {
            ZStack {
                if let frame = window.frame {
                    let picture = frame.resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    if let shield = window.shieldFrame { shield(AnyView(picture)) } else { picture }
                } else {
                    Text(window.placeholder).type(16, .regular, Ink.onDark).multilineTextAlignment(.center).padding(.horizontal, 16)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityIdentifier("session-window")
            if let line = window.line {
                Text(line).type(14.5, .medium, Ink.onDark).lineLimit(2).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
            }
            if let again = window.again {
                Pill(title: again, kind: .grey, symbol: "eye", height: 40, fill: false, onClick: window.onAgain).ferbSurface()
            }
            if let unsent { SessionUnsentView(unsent, onRetry: onRetry) }
            if let sending { SessionSendingView(sending).transition(.motionItem) }
            Text(window.note).type(13.5, .medium, Ink.onDark).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6)
        }
        .padding(.horizontal, 16).padding(.top, 4).padding(.bottom, 10)
    }
}

/// One session, full screen on black: its title and state on one header with its menu (where it runs and its actions,
/// PHN-800) and Close; the conversation with "Show earlier" at its top, a refused message (`unsent`) after its last
/// line and the agent's open questions at its end, or, for a session FERB reads no words of, its window live
/// (`window`, HAND-710); the message box (`foot`, or why nothing can be sent). The app follows the owner's screenshot
/// switch here, as everywhere (PHN-800, PHN-810).
public struct SessionPage<Foot: View>: View {
    var header: SessionRowData
    var lines: [SessionLine]
    var asks: [SessionAsk]
    var actions: [SessionAction]
    var insets: PhoneInsets
    var older: String?
    var onOlder: (() -> Void)?
    var onClose: (() -> Void)?
    var onChoice: ((Int, Int) -> Void)?
    var unsent: SessionUnsent?
    var onRetry: (() -> Void)?
    /// His words on their way, or just sent (PHN-812).
    var sending: SessionSending?
    var moreLabel: String
    var menuOpen: Bool
    var onMenu: ((Bool) -> Void)?
    /// The jump control's words (the main chat's: "New message", "Go to the latest messages"; PHN-813).
    var newMessage: String
    var toLatest: String
    /// False draws the conversation's end without a scroll view (a still picture: the screenshots cannot draw one).
    var scrolls: Bool
    /// A still picture as the owner sees it scrolled up (the jump control drawn; the screenshots).
    var stillJump: ListScroll?
    /// The first read is on its way (`waiting`'s words): the main chat's waiting look, never an empty page or an error
    /// before the Mac answered (owner 2026-11-23, PHN-816).
    var waiting: String?
    /// FERB's latest word on his hand-over (HAND-702), verbatim: one quiet line with FERB's mark at the conversation's
    /// end just above the message box, or under the header when `noteOnTop` (no box).
    var note: String?
    var noteOnTop: Bool
    /// HAND-710: the session's window, live, in place of the conversation (its lines, asks and Show earlier are not drawn).
    var window: SessionWindow?
    var foot: Foot

    public init(header: SessionRowData, lines: [SessionLine], asks: [SessionAsk], actions: [SessionAction], insets: PhoneInsets = .lab,
                older: String? = nil, onOlder: (() -> Void)? = nil, onClose: (() -> Void)? = nil, onChoice: ((Int, Int) -> Void)? = nil,
                unsent: SessionUnsent? = nil, onRetry: (() -> Void)? = nil, sending: SessionSending? = nil, moreLabel: String = "More",
                menuOpen: Bool = false, onMenu: ((Bool) -> Void)? = nil, newMessage: String = "New message",
                toLatest: String = "Go to the latest messages", scrolls: Bool = true, stillJump: ListScroll? = nil,
                waiting: String? = nil, note: String? = nil, noteOnTop: Bool = false, window: SessionWindow? = nil,
                @ViewBuilder foot: () -> Foot) {
        self.header = header
        self.lines = lines
        self.asks = asks
        self.actions = actions
        self.insets = insets
        self.older = older
        self.onOlder = onOlder
        self.onClose = onClose
        self.onChoice = onChoice
        self.unsent = unsent
        self.onRetry = onRetry
        self.sending = sending
        self.moreLabel = moreLabel
        self.menuOpen = menuOpen
        self.onMenu = onMenu
        self.newMessage = newMessage
        self.toLatest = toLatest
        self.scrolls = scrolls
        self.stillJump = stillJump
        self.waiting = waiting
        self.note = note
        self.noteOnTop = noteOnTop
        self.window = window
        self.foot = foot()
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center, spacing: 8) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(header.title).type(19, .semibold, Ink.white).lineLimit(1).accessibilityAddTraits(.isHeader)
                    HStack(spacing: 6) {
                        SessionChip(header.state, header.stateText)
                        if let f = header.ferb { FerbBadge(f) }
                    }
                    .ferbSurface()
                }
                Spacer(minLength: 0)
                if !actions.isEmpty { MoreButton(label: moreLabel, open: menuOpen, onClick: onMenu.map { f in { f(!menuOpen) } }) }
                CloseButton(onClick: onClose)
            }
            .padding(.horizontal, 16)
            // the header's own room under it: the conversation never touches it (owner 2026-11-23, PHN-815)
            .padding(.bottom, Self.headerGap)
            if noteOnTop, let note, !note.isEmpty { SessionFerbNote(note).padding(.horizontal, 16) }
            // the main chat's list (PHN-813): followed at the end, the jump control when scrolled up ("New message" when
            // something arrived), the keyboard down on a drag, his sent words always in view; rows dissolve under the
            // header instead of being cut at its edge (PHN-815)
            if let window {
                SessionWindowView(window: window, unsent: unsent, onRetry: onRetry, sending: sending)
            } else {
                conversation
            }
            if !noteOnTop, let note, !note.isEmpty { SessionFerbNote(note).padding(.horizontal, 16).padding(.bottom, 6) }
            foot.padding(.horizontal, 16)
        }
        .padding(.top, insets.top + 8)
        .padding(.bottom, insets.bottom + 8)
        // the whole screen takes every touch: nothing reaches the Sessions tab under it (its New session at the foot;
        // owner 2026-11-23, PHN-815)
        .background(Ink.black)
        .contentShape(Rectangle())
        .overlay(alignment: .topTrailing) {
            if menuOpen, !actions.isEmpty {
                ZStack(alignment: .topTrailing) {
                    // a tap anywhere else closes the menu
                    Color.black.opacity(0.001).contentShape(Rectangle()).onTapGesture { onMenu?(false) }
                    SessionMenu(place: header.subtitle, actions: actions, onPicked: { onMenu?(false) })
                        .padding(.top, insets.top + 8 + 52).padding(.trailing, 16)
                }
            }
        }
        .ignoresSafeArea()
    }

    /// The conversation as the main chat's list draws it.
    @ViewBuilder private var conversation: some View {
        let endKey = "\(lines.last?.key ?? ""):\(asks.count):\(unsent?.text ?? ""):\(sending?.text ?? ""):\(sending?.status ?? "")"
        ZStack(alignment: .bottom) {
            // the first read on its way: the main chat's waiting look (shapes of messages, its spinner and words)
            if let waiting, lines.isEmpty {
                ChatWaiting(waiting, textInk: Ink.onDark).frame(maxHeight: .infinity, alignment: .top)
                    .transition(.opacity)
                    .accessibilityIdentifier("session-waiting")
            }
            FollowingList(end: ListEnd(key: endKey, mine: sending != nil, value: endKey),
                          keys: lines.map(\.key), spacing: 10,
                          padding: EdgeInsets(top: Self.listTop, leading: 16, bottom: 12, trailing: 16),
                          fade: Self.fade, still: !scrolls,
                          jump: { ListJump.plain($0, newMessage: newMessage, toLatest: toLatest, light: true) }, over: { _ in EmptyView() }) {
                if let older {
                    Pill(title: older, kind: .grey, height: 36, fill: false, onClick: onOlder).ferbSurface()
                }
                ForEach(lines, id: \.key) { l in SessionLineView(l).id(l.key).transition(.motionItem) }
                if let unsent { SessionUnsentView(unsent, onRetry: onRetry) }
                if let sending { SessionSendingView(sending).id("sending").transition(.motionItem) }
                ForEach(Array(asks.enumerated()), id: \.offset) { a, ask in
                    SessionAskCard(ask, onChoice: onChoice.map { f in { f(a, $0) } })
                }
            }
            // a still picture of him scrolled up: the jump control as the live list draws it
            if !scrolls, let s = stillJump, let j = ListJump.plain(s, newMessage: newMessage, toLatest: toLatest, light: true) {
                JumpPill(j.text, label: j.label, light: true) {}.padding(.bottom, 10)
            }
        }
    }
}

extension SessionPage {
    /// The room between the header and the conversation, the conversation's own top inset and the fade that rows dissolve
    /// in under the header: rows at rest start below the fade, so nothing at rest is dimmed (PHN-815).
    static var headerGap: CGFloat { 10 }
    static var listTop: CGFloat { 20 }
    static var fade: CGFloat { 20 }
}

/// The Mac's question before FERB types for the owner (GATE-622) or starts a session (HAND-695): its question as the title,
/// the exact words or command in a fixed-width face, what the Yes covers, and Yes / Cancel. Nothing was sent before
/// his Yes.
public struct SessionYesSheet: View {
    let question: String
    let detail: String
    let covers: String
    let yes: String
    let cancel: String
    var yesSymbol: String?
    var insets: PhoneInsets
    var onYes: (() -> Void)?
    var onCancel: (() -> Void)?
    public init(question: String, detail: String, covers: String, yes: String, cancel: String, yesSymbol: String? = nil,
                insets: PhoneInsets = .lab, onYes: (() -> Void)? = nil, onCancel: (() -> Void)? = nil) {
        self.question = question
        self.detail = detail
        self.covers = covers
        self.yes = yes
        self.cancel = cancel
        self.yesSymbol = yesSymbol
        self.insets = insets
        self.onYes = onYes
        self.onCancel = onCancel
    }
    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onCancel) {
            SheetTitle(question, onClose: onCancel)
            if !detail.isEmpty {
                ViewThatFits(in: .vertical) {
                    ExactCommand(detail)
                    ScrollView { ExactCommand(detail) }
                }
                .frame(maxHeight: 260)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 14)
            }
            if !covers.trimmingCharacters(in: .whitespaces).isEmpty {
                Text(covers).type(14, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true).padding(.top, 12)
            }
            Pill(title: yes, kind: .black, symbol: yesSymbol, onClick: onYes).padding(.top, 18)
            Pill(title: cancel, kind: .plain, height: 44, onClick: onCancel).padding(.top, 4)
        }
    }
}

/// HAND-683: FERB can't tell which terminal is the session's: the Mac's reason, the terminals it can see (id, the
/// app's own words), each a choice with the phone's own prompt mark, the note, and Don't send. Android's PickTerminalSheet.
public struct PickTerminalSheet: View {
    let title: String
    let why: String
    let terminals: [(id: String, label: String)]
    let note: String
    let no: String
    let yesSymbol: String
    let insets: PhoneInsets
    let onPick: ((String) -> Void)?
    let onNo: () -> Void
    public init(title: String, why: String, terminals: [(id: String, label: String)], note: String, no: String, yesSymbol: String,
                insets: PhoneInsets = .lab, onPick: ((String) -> Void)?, onNo: @escaping () -> Void) {
        self.title = title
        self.why = why
        self.terminals = terminals
        self.note = note
        self.no = no
        self.yesSymbol = yesSymbol
        self.insets = insets
        self.onPick = onPick
        self.onNo = onNo
    }
    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onNo) {
            SheetTitle(title, onClose: onNo)
            if !why.isEmpty {
                Text(why).type(14.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true).padding(.top, 10)
            }
            VStack(spacing: 0) {
                ForEach(Array(terminals.enumerated()), id: \.offset) { i, t in
                    if i > 0 { Divide() }
                    CardRow(title: t.label, onClick: onPick.map { f in { f(t.id) } }) { RowIcon(symbol: "terminal") } trailing: {
                        Image(systemName: yesSymbol).font(.system(size: 16)).foregroundStyle(Ink.detail)
                    }
                }
            }
            .background(Ink.fill, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .padding(.top, 14)
            Text(note).type(14, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true).padding(.top, 12)
            Pill(title: no, kind: .plain, height: 44, onClick: onNo).padding(.top, 12)
        }
    }
}

/// Invented sessions for the gallery and the screenshots (no real folder, title or command; Android's labSessions).
public enum SessionSample {
    public static let label = "Your sessions"
    public static let rows: [SessionRowData] = [
        SessionRowData(title: "Fix the flaky login test", subtitle: "web-app · in Terminal", state: .working, stateText: "Working"),
        SessionRowData(title: "Move the blog to the new theme", subtitle: "blog · in Visual Studio Code", state: .waiting, stateText: "Waiting for you"),
        SessionRowData(title: "Add dark mode to settings", subtitle: "notes-app · in Terminal", state: .working, stateText: "Working",
                       ferb: "FERB is on it"),
        SessionRowData(title: "thesis-tools", subtitle: "thesis-tools · in the background", state: .idle, stateText: "Idle"),
    ]
    public static let off = SessionsOff(
        title: "FERB doesn't see your terminal sessions yet.",
        text: "Turn it on and you can follow your terminals and type into them from here, Claude Code and Codex too, open new ones and hand one to FERB. FERB adds a small hook to Claude Code's settings so it hears when a session finishes or needs you.",
        turnOn: "Turn on")
    public static let lines: [SessionLine] = [
        SessionLine("1", .owner, "Move the blog to the new theme and keep the old URLs working"),
        SessionLine("2", .agent, "I'll switch the theme in the config, then add redirects for the old post URLs."),
        SessionLine("3", .tool, "Read: config/site.toml"),
        SessionLine("4", .tool, "Edit: config/site.toml"),
        SessionLine("5", .agent, "The theme builds. Now I need to run the build to check the redirects."),
    ]
    public static let ask = SessionAsk(title: "Claude Code wants to use Bash", question: "", detail: "npm run build",
                                       choices: ["Yes", "Yes, and don't ask again", "No"], onMac: "Answer this one on your Mac.")

    public static func page(insets: PhoneInsets = .lab, off: Bool = false) -> SessionsPage {
        SessionsPage(label: label, rows: rows, newLabel: "New session", insets: insets, off: off ? Self.off : nil)
    }

    /// `scrolls` false for a still picture (the screenshots).
    public static let unsent = SessionUnsent(
        text: "Then deploy it to staging",
        reason: "FERB couldn't tell that the terminal in front in Visual Studio Code is this session's, so it typed nothing. Click into the session's terminal once, then send again.",
        retry: "Try again", notSent: "Not sent.")

    /// HAND-683: the terminals a session's app shows, when FERB can't tell which is the session's.
    public static let terminals: [(id: String, label: String)] = [
        ("front", "The terminal in front in Visual Studio Code now (Terminal 1, 2.1.330)"),
        ("w7:0:Terminal 1, 2.1.330", "config.toml — blog · Terminal 1, 2.1.330"),
        ("w7:1:Terminal 2, zsh", "config.toml — blog · Terminal 2, zsh"),
    ]

    /// PHN-812: his words the moment he tapped Send.
    public static let sending = SessionSending(text: "Then deploy it to staging", status: "Sending…")

    /// A conversation longer than the screen, the newest at the end (PHN-813, PHN-815: the header's room, the fade, the
    /// jump control).
    public static let long: [SessionLine] = lines + [
        SessionLine("6", .tool, "Bash: npm run build"),
        SessionLine("7", .agent, "The build passed and the redirects answer with 301. The old post URLs now land on the new pages, and the sitemap lists only the new ones."),
        SessionLine("8", .owner, "Great. Check the RSS feed keeps its old address too"),
        SessionLine("9", .agent, "The feed keeps /index.xml, so readers' subscriptions keep working. Nothing else links to the old theme's assets."),
        SessionLine("10", .owner, "Now open a pull request with a short summary of what changed"),
        SessionLine("11", .tool, "Bash: git push -u origin new-theme"),
        SessionLine("12", .agent, "Pushed the branch new-theme and opened the pull request with the summary: the new theme, the redirects for the old post URLs, and the feed kept at its address."),
    ]

    public static func session(insets: PhoneInsets = .lab, typed: String = "", scrolls: Bool = true, menuOpen: Bool = false,
                               unsent: SessionUnsent? = nil, sending: SessionSending? = nil, lines: [SessionLine] = lines,
                               asks: Bool = true, stillJump: ListScroll? = nil, waiting: String? = nil) -> some View {
        SessionPage(header: rows[1], lines: lines, asks: unsent == nil && sending == nil && asks && waiting == nil ? [ask] : [],
                    actions: [SessionAction("Watch", symbol: "eye", onClick: {}), SessionAction("Hand to FERB", symbol: "hand.raised", onClick: {}),
                              SessionAction("Stop its turn", symbol: "stop.fill", onClick: {})],
                    insets: insets, older: waiting == nil ? "Show earlier" : nil, onClose: {}, unsent: unsent, onRetry: {}, sending: sending,
                    menuOpen: menuOpen, onMenu: { _ in }, scrolls: scrolls, stillJump: stillJump, waiting: waiting) {
            SessionComposer(value: typed, placeholder: "Message Claude Code…", sendLabel: "Send", sending: sending?.status != nil)
        }
    }

    // MARK: the live fixes (owner 2026-11-23; HAND-702, HAND-703, PHN-821, PHN-822)

    /// An agent's reply drawn from its parts, modelled on the owner's Cursor reference: a thought, a paragraph, a step
    /// closed with its label and detail, a two-column table, a list, inline code and the turn's "Just now".
    public static let richMarkdown = """
        This is a **photo backup**: four archives from an old laptop, and nothing here is unpacked yet.

        | File | What's inside |
        | --- | --- |
        | `photos-2019.zip` (2 GB) | Holiday photos from 2019, about 1,400 pictures. |
        | `photos-2020.zip` (3 GB) | The year's photos and a few short videos. |
        | `notes.zip` | Plain text notes from the same laptop. |

        - The largest is `photos-2020.zip`.
        - Nothing needs unzipping to tell what it is.
        """
    public static let richText: [RichBlock] = [
        .paragraph([RichRun("This is a "), RichRun("photo backup", bold: true),
                    RichRun(": four archives from an old laptop, and nothing here is unpacked yet.")]),
        .table(header: [[RichRun("File")], [RichRun("What's inside")]], rows: [
            [[RichRun("photos-2019.zip", code: true), RichRun(" (2 GB)")], [RichRun("Holiday photos from 2019, about 1,400 pictures.")]],
            [[RichRun("photos-2020.zip", code: true), RichRun(" (3 GB)")], [RichRun("The year's photos and a few short videos.")]],
            [[RichRun("notes.zip", code: true)], [RichRun("Plain text notes from the same laptop.")]],
        ]),
        .item(marker: "•", level: 0, [RichRun("The largest is "), RichRun("photos-2020.zip", code: true), RichRun(".")]),
        .item(marker: "•", level: 0, [RichRun("Nothing needs unzipping to tell what it is.")]),
    ]
    public static let richParts: [SessionPartShown] = [
        .thinking(label: "Thought 1s", text: ""),
        .text([.paragraph([RichRun("I'll look through this folder and tell you what it is.")])]),
        .tool(label: "List the archives and their sizes", detail: "ls, unzip, head",
              text: "  Length      Date    Time    Name\n---------  ---------- -----   ----\n     2048  01-01-2020 00:00   photos/\n"),
        .thinking(label: "Thought 4s", text: "Four zips with photo folders inside; one holds text files."),
        .text(richText),
        .text([.paragraph([RichRun("So it's your old photo backup, sitting in four zips.")])]),
        .meta("Just now"),
    ]
    public static let rich: [SessionLine] = [
        SessionLine("1", .owner, "What's in this folder? I forgot"),
        SessionLine("2", .reply, "", parts: richParts),
    ]
    /// A session FERB keeps going: its row, FERB's latest word on it.
    public static let withFerb = rows[2]
    public static let ferbNote = "FERB is on it: “Keep going until the tests pass”"

    /// The session FERB has: Take it back in place of Hand to FERB (`taking`: on its way), FERB's word above the box.
    public static func ferbSession(insets: PhoneInsets = .lab, menuOpen: Bool = false, taking: Bool = false, starting: Bool = false,
                                   note: String? = ferbNote, lines: [SessionLine] = lines, asks: [SessionAsk] = []) -> some View {
        let hand: SessionAction = starting
            ? SessionAction("FERB is starting…", symbol: nil, busy: true, onClick: nil)
            : taking ? SessionAction("Taking it back…", symbol: "arrow.uturn.backward", busy: true, stays: true, onClick: nil)
            : SessionAction("Take it back", symbol: "arrow.uturn.backward", stays: true, onClick: {})
        return SessionPage(header: withFerb, lines: lines, asks: asks,
                           actions: [SessionAction("Watch", symbol: "eye", onClick: {}), hand, SessionAction("Stop its turn", symbol: "stop.fill", onClick: {})],
                           insets: insets, older: nil, onClose: {}, menuOpen: menuOpen, onMenu: { _ in }, scrolls: false, note: note) {
            SessionComposer(value: "", placeholder: "Message Claude Code…", sendLabel: "Send")
        }
    }

    /// HAND-708, HAND-709: after 20 rounds FERB asks in the session whether to keep going (the owner's words); what it
    /// sent while it kept the session going is on his side, marked as FERB's.
    public static let keepGoingQuestion = "I've gone 20 rounds on “Keep going until the tests pass”. Keep going?"
    public static let keepGoingNote = "FERB is waiting for your answer."
    public static let keepGoingLines: [SessionLine] = [
        SessionLine("1", .agent, "The tests build. Two of them still fail on the login form."),
        SessionLine("2", .owner, "Fix those two and run the tests again", by: "Sent by FERB"),
        SessionLine("3", .agent, "Done: both pass now, and the whole suite is green."),
    ]
    public static let keepGoingAsk = SessionAsk(title: "FERB asks", question: keepGoingQuestion, detail: "",
                                                choices: ["Keep going", "Stop"], onMac: "Answer this one on your Mac.")
    public static func keepGoing(insets: PhoneInsets = .lab) -> some View {
        ferbSession(insets: insets, note: keepGoingNote, lines: keepGoingLines, asks: [keepGoingAsk])
    }

    // MARK: a session FERB reads no words of (HAND-710)

    /// A terminal FERB reads none of the words of: its row (the program, its folder, its app) and its window as the Mac
    /// sends it, an invented one.
    public static let watchRow = SessionRowData(title: "web-app — zsh", subtitle: "zsh · web-app · in Terminal", state: .idle, stateText: "Idle")
    public static let typeNote = "What you send is typed into this window on your Mac, then Return."

    #if canImport(UIKit)
    /// The invented window's picture as the Mac's preview sends it (PNG bytes, drawn here): a terminal that ran the tests.
    @MainActor public static func windowPicture() -> Data {
        let r = ImageRenderer(content: WindowPicture().environment(\.colorScheme, .light))
        r.scale = 1
        return r.uiImage?.pngData() ?? Data()
    }

    /// The watch session's screen: the header, its window live, the note, the box saying "Type here".
    @MainActor public static func watch(insets: PhoneInsets = .lab, picture: Data? = nil) -> some View {
        let frame = UIImage(data: picture ?? windowPicture()).map { Image(uiImage: $0) }
        return SessionPage(header: watchRow, lines: [], asks: [],
                           actions: [], insets: insets, onClose: {}, scrolls: false,
                           window: SessionWindow(frame: frame, placeholder: "Waiting for the first picture…", note: typeNote)) {
            SessionComposer(value: "", placeholder: "Type here", sendLabel: "Send")
        }
    }
    #endif

    public static let newWords = NewSessionWords(title: "New session", folder: "Folder", other: "Another folder…", command: "Command (optional)",
                                                 commandHint: "Command to run (optional)", start: "Start", starting: "Opening a terminal…")
    public static let folders: [SessionFolderRow] = [
        SessionFolderRow(title: "web-app", detail: "/Users/you/code/web-app", checked: true),
        SessionFolderRow(title: "blog", detail: "/Users/you/code/blog"),
        SessionFolderRow(title: "notes-app", detail: "/Users/you/code/notes-app"),
    ]

    public static func newSession(insets: PhoneInsets = .lab, starting: Bool = false, refused: String? = nil) -> some View {
        NewSessionSheetView(words: newWords, folders: folders, command: "claude", starting: starting, refused: refused, insets: insets,
                            scrolls: false, onFolder: { _ in }, onOther: {}, onStart: {}, onClose: {})
    }

    /// The folder picker inside ~/code: its folders, the path, Use this folder.
    public static func folderPicker(insets: PhoneInsets = .lab, top: Bool = false) -> some View {
        SessionFolderPickerView(
            title: "Choose a folder", folder: top ? nil : "code", backLabel: "Back",
            rows: top
                ? [SessionFolderRow(title: "Home folder", symbol: "house", opens: true),
                   SessionFolderRow(title: "Desktop", symbol: "menubar.dock.rectangle", opens: true),
                   SessionFolderRow(title: "Documents", symbol: "doc", opens: true),
                   SessionFolderRow(title: "Downloads", symbol: "arrow.down.circle", opens: true)]
                : [SessionFolderRow(title: "blog", opens: true), SessionFolderRow(title: "notes-app", opens: true),
                   SessionFolderRow(title: "thesis-tools", opens: true), SessionFolderRow(title: "web-app", opens: true)],
            recentLabel: top ? "Recent" : nil,
            recent: top ? [SessionFolderRow(title: "recipes", detail: "/Users/you/code/recipes", opens: true)] : [],
            path: top ? "" : "/Users/you/code", use: "Use this folder", insets: insets, scrolls: false,
            onUp: {}, onRow: { _ in }, onRecent: { _ in }, onUse: top ? nil : {}, onClose: {})
    }
}


/// The invented terminal window the watch session's sample shows (no real folder or command of the owner's).
struct WindowPicture: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 7) {
                ForEach([Color(red: 1, green: 0.37, blue: 0.34), Color(red: 1, green: 0.74, blue: 0.18), Color(red: 0.16, green: 0.79, blue: 0.25)],
                        id: \.self) { c in Circle().fill(c).frame(width: 11, height: 11) }
                Spacer(minLength: 0)
                Text("web-app — zsh — 80×24").font(.system(size: 12, weight: .medium)).foregroundStyle(Color(white: 0.55))
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12).frame(height: 30)
            .background(Color(white: 0.17))
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(["~/code/web-app $ npm test", "", " PASS  src/login.test.ts", " PASS  src/settings.test.ts",
                         " PASS  src/theme.test.ts", "", "Tests:  42 passed, 42 total", "Time:   3.1 s", "", "~/code/web-app $ "]
                            .enumerated()), id: \.offset) { _, line in
                    Text(line.isEmpty ? " " : line).font(.system(size: 13, design: .monospaced))
                        .foregroundStyle(line.contains("PASS") ? Color(red: 0.4, green: 0.85, blue: 0.45) : Color(white: 0.9))
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Color(white: 0.1))
        }
        .frame(width: 520, height: 330)
    }
}
