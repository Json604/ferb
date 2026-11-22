import SwiftUI

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
    var waiting: PhoneTab?
    var onTab: ((PhoneTab) -> Void)?
    var onProfile: (() -> Void)?
    var onRow: ((Int) -> Void)?
    var onNew: (() -> Void)?
    var onTurnOn: (() -> Void)?

    public init(label: String, rows: [SessionRowData], newLabel: String, insets: PhoneInsets = .lab, off: SessionsOff? = nil,
                empty: String? = nil, waiting: PhoneTab? = nil, onTab: ((PhoneTab) -> Void)? = nil, onProfile: (() -> Void)? = nil,
                onRow: ((Int) -> Void)? = nil, onNew: (() -> Void)? = nil, onTurnOn: (() -> Void)? = nil) {
        self.label = label
        self.rows = rows
        self.newLabel = newLabel
        self.insets = insets
        self.off = off
        self.empty = empty
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

/// What a line of a session's conversation is: the owner's words, the agent's, one tool it ran, a stop, an error.
public enum SessionLineKind: Sendable { case owner, agent, tool, interrupted, error }

/// One line of the conversation (`key` stays the same while it is on screen: its sequence number).
public struct SessionLine: Equatable, Sendable {
    public var key: String
    public var kind: SessionLineKind
    public var text: String
    public init(_ key: String, _ kind: SessionLineKind, _ text: String) { self.key = key; self.kind = kind; self.text = text }
}

/// One line on the session screen's black page: the owner's words on white at the right, the agent's on the dark fill
/// at the left, a tool it ran as one small fixed-width line, its turn stopped and an error as quiet lines.
public struct SessionLineView: View {
    let line: SessionLine
    public init(_ line: SessionLine) { self.line = line }
    public var body: some View {
        switch line.kind {
        case .owner: OwnerBubble(line.text)
        case .agent:
            Text(line.text).type(15.5, .regular, Ink.white).lineSpacing(2)
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

/// One of the session screen's actions, in its menu (Watch, Hand to FERB, Stop its turn, Stop controlling).
public struct SessionAction {
    public let label: String
    public let symbol: String?
    public let onClick: (() -> Void)?
    public init(_ label: String, symbol: String?, onClick: (() -> Void)?) { self.label = label; self.symbol = symbol; self.onClick = onClick }
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
                    ZStack { if let s = a.symbol { Image(systemName: s).font(.system(size: 16, weight: .medium)) } }
                        .frame(width: 20)
                    Text(a.label).type(16, .medium, ink).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .foregroundStyle(ink)
                .padding(.horizontal, 14)
                .frame(minHeight: 48)
                .contentShape(Rectangle())
                .pressable(a.onClick.map { f in { onPicked(); f() } }, label: a.label)
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
    public init(value: String, placeholder: String, sendLabel: String, text: Binding<String>? = nil, onSend: (() -> Void)? = nil) {
        self.value = value
        self.placeholder = placeholder
        self.sendLabel = sendLabel
        self.text = text
        self.onSend = onSend
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
            Image(systemName: "arrow.up").font(.system(size: 17, weight: .semibold)).foregroundStyle(Ink.black)
                .frame(width: 40, height: 40).background(ready ? Ink.white : Ink.onDark, in: Circle())
                .pressable(ready ? onSend : nil, label: sendLabel)
        }
        .padding(.leading, 18).padding(.trailing, 8).padding(.vertical, 8)
        .frame(minHeight: 56)
        .background(Ink.darkFill, in: RoundedRectangle(cornerRadius: 28, style: .continuous))
    }
}

/// One session, full screen on black: its title and state on one header with its menu (where it runs and its actions,
/// PHN-800) and Close; the conversation with "Show earlier" at its top, a refused message (`unsent`) after its last
/// line and the agent's open questions at its end; the message box (`foot`, or why nothing can be sent). The app follows
/// the owner's screenshot switch here (PHN-800); only Watch is kept out of captures always.
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
    var moreLabel: String
    var menuOpen: Bool
    var onMenu: ((Bool) -> Void)?
    /// False draws the conversation's end without a scroll view (a still picture: the screenshots cannot draw one).
    var scrolls: Bool
    var foot: Foot

    public init(header: SessionRowData, lines: [SessionLine], asks: [SessionAsk], actions: [SessionAction], insets: PhoneInsets = .lab,
                older: String? = nil, onOlder: (() -> Void)? = nil, onClose: (() -> Void)? = nil, onChoice: ((Int, Int) -> Void)? = nil,
                unsent: SessionUnsent? = nil, onRetry: (() -> Void)? = nil, moreLabel: String = "More", menuOpen: Bool = false,
                onMenu: ((Bool) -> Void)? = nil, scrolls: Bool = true, @ViewBuilder foot: () -> Foot) {
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
        self.moreLabel = moreLabel
        self.menuOpen = menuOpen
        self.onMenu = onMenu
        self.scrolls = scrolls
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
            BottomFitOrScroll(end: (lines.last?.key ?? "") + ":\(asks.count):\(unsent?.text ?? "")", scrolls: scrolls) {
                VStack(spacing: 10) {
                    if let older {
                        Pill(title: older, kind: .grey, height: 36, fill: false, onClick: onOlder).ferbSurface()
                    }
                    ForEach(Array(lines.enumerated()), id: \.offset) { _, l in SessionLineView(l) }
                    if let unsent { SessionUnsentView(unsent, onRetry: onRetry) }
                    ForEach(Array(asks.enumerated()), id: \.offset) { a, ask in
                        SessionAskCard(ask, onChoice: onChoice.map { f in { f(a, $0) } })
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 14)
                .padding(.bottom, 12)
            }
            foot.padding(.horizontal, 16)
        }
        .padding(.top, insets.top + 8)
        .padding(.bottom, insets.bottom + 8)
        .background(Ink.black)
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
}

/// The conversation: drawn as it is when it fits (the screenshots), else a scroll view at its end that follows a new
/// line while the owner is at the end.
struct BottomFitOrScroll<Content: View>: View {
    let end: String
    var scrolls = true
    @ViewBuilder var content: Content
    var body: some View {
        if !scrolls {
            // its own height (every line whole), its end at the bottom, what is above cut off
            content.fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, minHeight: 0, maxHeight: .infinity, alignment: .bottom).clipped()
        } else {
            fitOrScroll
        }
    }
    private var fitOrScroll: some View {
        ViewThatFits(in: .vertical) {
            content.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
            ScrollViewReader { reader in
                ScrollView {
                    content
                    Color.clear.frame(height: 1).id("end")
                }
                .defaultScrollAnchor(.bottom)
                .scrollBounceBehavior(.basedOnSize)
                .onChange(of: end) { reader.scrollTo("end", anchor: .bottom) }
            }
        }
    }
}

/// The Mac's question before FERB types for the owner (GATE-622) or starts a session (HAND-665): its question as the title,
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
    public static let label = "Your coding sessions"
    public static let rows: [SessionRowData] = [
        SessionRowData(title: "Fix the flaky login test", subtitle: "web-app · in Terminal", state: .working, stateText: "Working"),
        SessionRowData(title: "Move the blog to the new theme", subtitle: "blog · in Visual Studio Code", state: .waiting, stateText: "Waiting for you"),
        SessionRowData(title: "Add dark mode to settings", subtitle: "notes-app · in Terminal", state: .working, stateText: "Working",
                       ferb: "FERB is on it"),
        SessionRowData(title: "thesis-tools", subtitle: "thesis-tools · in the background", state: .idle, stateText: "Idle"),
    ]
    public static let off = SessionsOff(
        title: "FERB doesn't see your Claude Code and Codex sessions yet.",
        text: "Turn it on and you can follow and answer them from here, start new ones and hand one to FERB. FERB adds a small hook to Claude Code's settings so it hears when a session finishes or needs you. Codex needs nothing added.",
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

    public static func session(insets: PhoneInsets = .lab, typed: String = "", scrolls: Bool = true, menuOpen: Bool = false,
                               unsent: SessionUnsent? = nil) -> some View {
        SessionPage(header: rows[1], lines: lines, asks: unsent == nil ? [ask] : [],
                    actions: [SessionAction("Watch", symbol: "eye", onClick: {}), SessionAction("Hand to FERB", symbol: "hand.raised", onClick: {}),
                              SessionAction("Stop its turn", symbol: "stop.fill", onClick: {})],
                    insets: insets, older: "Show earlier", onClose: {}, unsent: unsent, onRetry: {}, menuOpen: menuOpen, onMenu: { _ in },
                    scrolls: scrolls) {
            SessionComposer(value: typed, placeholder: "Message Claude Code…", sendLabel: "Send")
        }
    }
}
