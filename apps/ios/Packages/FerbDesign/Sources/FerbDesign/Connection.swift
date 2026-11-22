import SwiftUI

// The connection control in the top bar and its sheet (kb PHN-751, PHN-752; Android's TopBar.kt LinkMark, Connection,
// ConnectionButton and SheetScreens.kt ConnectionSheet).

/// How the link to the Mac stands, as the top bar's connection control shows it (kb PHN-751): connecting (the design's
/// turning arc, as FERB.app shows "Connecting to FERB…"), connected by its path (a quiet solid dot: white directly,
/// white with a ring through his own relay, grey through the shared relay), or can't reach (an amber lamp, only after
/// connecting has really failed).
public enum LinkMark: Sendable { case connecting, direct, ownRelay, sharedRelay, unreachable }

/// The connection control: its mark, what VoiceOver says (`label`) and its tap (the connection sheet).
public struct Connection {
    public let mark: LinkMark
    public let label: String
    public let onClick: (() -> Void)?
    public init(mark: LinkMark, label: String, onClick: (() -> Void)?) {
        self.mark = mark
        self.label = label
        self.onClick = onClick
    }
}

private struct ConnectionKey: EnvironmentKey { static let defaultValue: Connection? = nil }

public extension EnvironmentValues {
    /// The connection control every page's top bar shows beside the profile button (kb PHN-751); nil: no control.
    var connection: Connection? {
        get { self[ConnectionKey.self] }
        set { self[ConnectionKey.self] = newValue }
    }
}

/// The connection control (kb PHN-751): the profile button's round, with the link's mark in it.
public struct ConnectionButton: View {
    let c: Connection
    public init(_ c: Connection) { self.c = c }
    public var body: some View {
        LinkMarkView(c.mark, size: 9)
            .frame(width: 42, height: 42)
            .background(Ink.darkFill, in: Circle())
            .pressable(c.onClick, label: c.label)
            .accessibilityLabel(Text(c.label))
    }
}

/// The connection's row on the profile page (PHN-801): the link's mark in a black round, the state, the path, and the
/// connection sheet on a tap (Try again, the relay, the Mac tab). Android's ConnectionRow.
public struct ConnectionRow: View {
    let mark: LinkMark
    let state: String
    let path: String?
    let onClick: (() -> Void)?
    public init(mark: LinkMark, state: String, path: String?, onClick: (() -> Void)?) {
        self.mark = mark
        self.state = state
        self.path = path
        self.onClick = onClick
    }
    public var body: some View {
        CardRow(title: state, detail: path, onClick: onClick) {
            LinkMarkView(mark, size: 9).frame(width: 28, height: 28).background(Ink.black, in: Circle())
        } trailing: { Opens() }
    }
}

/// The link's mark at `size` (the dot's size): the arc, a dot, a dot in a ring, or the amber lamp.
public struct LinkMarkView: View {
    let mark: LinkMark
    let size: CGFloat
    public init(_ mark: LinkMark, size: CGFloat) { self.mark = mark; self.size = size }
    public var body: some View {
        switch mark {
        case .connecting: Spinner(size: size * 1.8)
        case .direct: Circle().fill(Ink.white).frame(width: size, height: size)
        case .ownRelay:
            Circle().fill(Ink.white).frame(width: size * 0.9, height: size * 0.9)
                .frame(width: size * 1.9, height: size * 1.9)
                .overlay(Circle().strokeBorder(Ink.white.opacity(0.55), lineWidth: 1.2))
        case .sharedRelay: Circle().fill(Ink.onDark).frame(width: size, height: size)
        case .unreachable: LampDot(color: Ink.amber, size: size)
        }
    }
}

/// The connection sheet (kb PHN-752): which Mac, how the link stands in plain words with its mark, the path and what it
/// means for him (on the shared relay, the relay sheet's own list of what works there), the link's own words for why
/// it can't connect, Try again while it isn't connected, and the way to the Mac tab for the rest.
public struct ConnectionSheet: View {
    let mac: String
    let mark: LinkMark
    let state: String
    let path: String?
    let reason: String?
    let relay: RelayCopy?
    let tryAgain: String?
    let onTryAgain: (() -> Void)?
    let more: String
    let onMore: () -> Void
    let insets: PhoneInsets
    let onClose: (() -> Void)?

    public init(mac: String, mark: LinkMark, state: String, path: String?, reason: String?, relay: RelayCopy?, tryAgain: String?,
                onTryAgain: (() -> Void)?, more: String, onMore: @escaping () -> Void, insets: PhoneInsets = .lab, onClose: (() -> Void)? = nil) {
        self.mac = mac
        self.mark = mark
        self.state = state
        self.path = path
        self.reason = reason
        self.relay = relay
        self.tryAgain = tryAgain
        self.onTryAgain = onTryAgain
        self.more = more
        self.onMore = onMore
        self.insets = insets
        self.onClose = onClose
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onClose) {
            SheetTitle(mac, onClose: onClose)
            HStack(spacing: 10) {
                LinkMarkView(mark, size: 9).frame(width: 28, height: 28).background(Ink.black, in: Circle())
                Text(state).type(17, .semibold)
            }
            .accessibilityElement(children: .combine)
            .padding(.top, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            if let path { line(path) }
            if let reason { line(reason) }
            if let relay {
                VStack(spacing: 0) {
                    ForEach(Array(relay.works.enumerated()), id: \.offset) { i, w in
                        if i > 0 { Rectangle().fill(Ink.line).frame(height: 1) }
                        HStack {
                            Text(w.what).type(15)
                            Spacer()
                            Text(w.how).type(15, .regular, Ink.detail)
                        }
                        .padding(.horizontal, 14).padding(.vertical, 12)
                        .accessibilityElement(children: .combine)
                    }
                }
                .background(Ink.fill, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                .padding(.top, 14)
            }
            VStack(spacing: 10) {
                if let tryAgain { Pill(title: tryAgain, kind: .black, onClick: onTryAgain) }
                Pill(title: more, kind: .grey, onClick: onMore)
            }
            .padding(.top, 18)
        }
    }

    private func line(_ text: String) -> some View {
        Text(text).type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 10)
    }
}
