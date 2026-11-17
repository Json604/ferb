import SwiftUI

// NOT FROM THE LAB. The glass lab (23a0fde) draws none of these; the app needs them, and Android built the same ones
// (design components/Pairing.kt QrFrame; app ui/Files.kt FolderView, Live.kt LivePage, Settings.kt SettingsSheet,
// Root.kt Message). Each is made only of the lab's own pieces and numbers (Card, CardRow, Divide, RowIcon, Opens,
// Pill, Sheet, SheetTitle, CloseButton, PhoneSwitch, FileIcon, FerbMark, the page margins) until the lab draws one.
// Their words are parameters: the app's strings live in the app, not here.

/// The QR scanner's frame (Android's QrFrame): a white card of 28 corners, an Ink.fill well of 24 corners holding the
/// camera's picture, corner marks, and a hint under it. The well clips the picture to its own rounded square, so a
/// picture of any size or aspect stays inside (kb PHN-618). With `live` (a camera picture under them) the corner
/// marks are white on a soft dark edge, readable on a dark room and on the Mac's white screen alike.
public struct QrFrame<Preview: View>: View {
    let hint: String
    let live: Bool
    let preview: Preview

    public init(hint: String, live: Bool = false, @ViewBuilder preview: () -> Preview) {
        self.hint = hint
        self.live = live
        self.preview = preview()
    }

    public var body: some View {
        Card(padding: 16) {
            ZStack {
                // the picture is laid over the well, never sizes it: a picture of its own size can't push the page
                RoundedRectangle(cornerRadius: 24, style: .continuous).fill(Ink.fill)
                    .overlay { preview.frame(maxWidth: .infinity, maxHeight: .infinity) }
                    .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
                Canvas { g, size in
                    let len = min(size.width, size.height) * 0.18
                    let r: CGFloat = 10
                    func corner(_ x: CGFloat, _ y: CGFloat, _ dx: CGFloat, _ dy: CGFloat) {
                        var p = Path()
                        p.move(to: CGPoint(x: x, y: y + dy * len))
                        p.addLine(to: CGPoint(x: x, y: y + dy * r))
                        p.addQuadCurve(to: CGPoint(x: x + dx * r, y: y), control: CGPoint(x: x, y: y))
                        p.addLine(to: CGPoint(x: x + dx * len, y: y))
                        if live {
                            g.stroke(p, with: .color(.black.opacity(0.35)), style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
                            g.stroke(p, with: .color(Ink.white), style: StrokeStyle(lineWidth: 4, lineCap: .round, lineJoin: .round))
                        } else {
                            g.stroke(p, with: .color(Ink.black), style: StrokeStyle(lineWidth: 4, lineCap: .round, lineJoin: .round))
                        }
                    }
                    corner(0, 0, 1, 1)
                    corner(size.width, 0, -1, 1)
                    corner(0, size.height, 1, -1)
                    corner(size.width, size.height, -1, -1)
                }
                .padding(28)
                .accessibilityHidden(true)
            }
            .aspectRatio(1, contentMode: .fit)
            .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
            Text(hint).type(14, .regular, Ink.detail).multilineTextAlignment(.center).frame(maxWidth: .infinity)
                .padding(.horizontal, 4).padding(.top, 12)
        }
    }
}

public extension QrFrame where Preview == EmptyView {
    init(hint: String) { self.init(hint: hint) { EmptyView() } }
}

/// One entry of a folder on the Mac.
public struct FolderEntry: Equatable, Sendable {
    public var name: String
    public var detail: String?
    public var isFolder: Bool
    /// A key place's entry: it shows its lock and opens only after a fresh Face ID (kb PHN-121).
    public var locked: Bool
    public init(name: String, detail: String? = nil, isFolder: Bool = false, locked: Bool = false) {
        self.name = name
        self.detail = detail
        self.isFolder = isFolder
        self.locked = locked
    }
}

/// An open folder on the Files page (Android's FolderView): its name with a way back, then its entries in one card
/// of the lab's rows (a folder's symbol or the file's icon, a lock for a key place).
public struct FolderList: View {
    @Environment(\.skyInk) private var ink
    let title: String
    let entries: [FolderEntry]
    var locked = false
    /// Shown instead of the card when the folder is empty.
    var emptyText: String?
    /// Shown under the card when the list was cut short.
    var footnote: String?
    var onBack: (() -> Void)?
    var onEntry: ((Int) -> Void)?

    public init(title: String, entries: [FolderEntry], locked: Bool = false, emptyText: String? = nil, footnote: String? = nil,
                onBack: (() -> Void)? = nil, onEntry: ((Int) -> Void)? = nil) {
        self.title = title
        self.entries = entries
        self.locked = locked
        self.emptyText = emptyText
        self.footnote = footnote
        self.onBack = onBack
        self.onEntry = onEntry
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "chevron.left").font(.system(size: 17, weight: .semibold)).foregroundStyle(ink.top)
                    .frame(width: 28, height: 36)
                    .pressable(onBack, label: "Back")
                Text(title).type(15, .semibold, ink.low).lineLimit(1).accessibilityAddTraits(.isHeader)
                Spacer(minLength: 0)
                if locked { Image(systemName: "lock.fill").font(.system(size: 14)).foregroundStyle(ink.low) }
            }
            .padding(.top, 14)
            if entries.isEmpty {
                if let emptyText { Text(emptyText).type(15, .medium, ink.low).padding(.leading, 8).padding(.top, 8) }
            } else {
                Card {
                    ForEach(Array(entries.enumerated()), id: \.offset) { i, e in
                        if i > 0 { Divide() }
                        CardRow(title: e.name, detail: e.detail, onClick: onEntry.map { f in { f(i) } }) {
                            if e.isFolder { RowIcon(symbol: "folder") } else { FileIcon(name: e.name, width: 26) }
                        } trailing: {
                            if e.locked { Image(systemName: "lock.fill").font(.system(size: 15)).foregroundStyle(Ink.faint) } else { Opens() }
                        }
                    }
                }
                .padding(.top, 4)
            }
            if let footnote { Text(footnote).type(14, .medium, ink.low).padding(.leading, 8).padding(.top, 10) }
        }
    }
}

/// Watch live (Android's LivePage): the task's window on the Mac full screen, on black, with FERB's line under it,
/// a way out and Stop. `frame` is the picture from the Mac; without one `placeholder` is said (or a running mark).
public struct LivePage: View {
    var title: String
    var frame: Image?
    var placeholder: String?
    var line: String?
    var closeTitle: String
    var stopTitle: String
    var insets: PhoneInsets
    var onClose: (() -> Void)?
    var onStop: (() -> Void)?
    /// The app's screenshot shield around the frame (kb PHN-597); nil draws it plainly (the default).
    var shieldFrame: ((AnyView) -> AnyView)?

    public init(title: String = "Watch live", frame: Image? = nil, placeholder: String? = nil, line: String? = nil,
                closeTitle: String, stopTitle: String = "Stop", insets: PhoneInsets = .lab,
                onClose: (() -> Void)? = nil, onStop: (() -> Void)? = nil, shieldFrame: ((AnyView) -> AnyView)? = nil) {
        self.title = title
        self.frame = frame
        self.placeholder = placeholder
        self.line = line
        self.closeTitle = closeTitle
        self.stopTitle = stopTitle
        self.insets = insets
        self.onClose = onClose
        self.onStop = onStop
        self.shieldFrame = shieldFrame
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center) {
                Text(title).type(22, .semibold, Ink.white).accessibilityAddTraits(.isHeader)
                Spacer()
                CloseButton(onClick: onClose)
            }
            .padding(.horizontal, 16)
            ZStack {
                if let frame {
                    let picture = frame.resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                    if let shieldFrame { shieldFrame(AnyView(picture)) } else { picture }
                } else if let placeholder {
                    Text(placeholder).type(16, .regular, Ink.onDark).multilineTextAlignment(.center).padding(.horizontal, 16)
                } else {
                    Mark(kind: .running).colorInvert()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(16)
            if let line {
                Text(line).type(15, .medium, Ink.onDark).frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 24).padding(.vertical, 8)
            }
            HStack(spacing: 8) {
                Pill(title: closeTitle, kind: .grey, height: 46, onClick: onClose)
                Pill(title: stopTitle, kind: .grey, symbol: "stop.fill", height: 46, onClick: onStop)
            }
            .padding(.horizontal, 16)
        }
        .padding(.top, insets.top + 8)
        .padding(.bottom, insets.bottom + 8)
        .background(Color.black)
        .ignoresSafeArea()
    }
}

/// One of FERB's settings on the phone (Android's SettingsSheet rows): a switch, a list shown as it is, or a value that
/// opens.
public enum SettingRow {
    case toggle(title: String, symbol: String, on: Bool, onChange: ((Bool) -> Void)?)
    case list(title: String, symbol: String, detail: String)
    case value(title: String, value: String?, onClick: (() -> Void)?)
}

/// FERB's settings from the Mac page (Android's SettingsSheet): ProfileSheet's layout, the rows reaching the sheet's
/// edges, an optional second group under its heading, and a note.
public struct SettingsSheet: View {
    let title: String
    let rows: [SettingRow]
    var groupTitle: String?
    var groupRows: [SettingRow]
    var note: String?
    /// Shown instead of the rows while they load.
    var loading: String?
    var insets: PhoneInsets
    var onClose: (() -> Void)?

    public init(title: String, rows: [SettingRow], groupTitle: String? = nil, groupRows: [SettingRow] = [], note: String? = nil,
                loading: String? = nil, insets: PhoneInsets = .lab, onClose: (() -> Void)? = nil) {
        self.title = title
        self.rows = rows
        self.groupTitle = groupTitle
        self.groupRows = groupRows
        self.note = note
        self.loading = loading
        self.insets = insets
        self.onClose = onClose
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onClose) {
            SheetTitle(title, onClose: onClose)
            if let loading {
                Text(loading).type(15.5, .regular, Ink.detail).padding(.top, 10)
            } else {
                FitOrScroll {
                    VStack(spacing: 0) {
                        lines(rows)
                        if !groupRows.isEmpty {
                            if let groupTitle {
                                Text(groupTitle).type(15, .semibold, Ink.detail).frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, 14).padding(.top, 18).padding(.bottom, 4).accessibilityAddTraits(.isHeader)
                            }
                            lines(groupRows)
                        }
                    }
                }
                .frame(maxHeight: 560)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, -14)
                .padding(.top, 8)
            }
            if let note { Text(note).type(14, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true).padding(.top, 12) }
        }
    }

    @ViewBuilder private func lines(_ list: [SettingRow]) -> some View {
        ForEach(Array(list.enumerated()), id: \.offset) { i, row in
            if i > 0 { Divide() }
            switch row {
            case let .toggle(title, symbol, on, onChange):
                CardRow(title: title) { RowIcon(symbol: symbol) } trailing: { PhoneSwitch(on: on, onChange: onChange, label: title) }
            case let .list(title, symbol, detail):
                CardRow(title: title, detail: detail) { RowIcon(symbol: symbol) }
            case let .value(title, value, onClick):
                CardRow(title: title, onClick: onClick) { EmptyView() } trailing: { Opens(value: value) }
            }
        }
    }
}

/// UnlockPage's layout for the app's other plain messages before anything of FERB's shows (Android's Message): the
/// mark, a title, and a card with the words and one button.
public struct MessagePage: View {
    @Environment(\.skyInk) private var ink
    let title: String
    let text: String
    let action: String
    var insets: PhoneInsets
    var onAction: (() -> Void)?

    public init(title: String, text: String, action: String, insets: PhoneInsets = .lab, onAction: (() -> Void)? = nil) {
        self.title = title
        self.text = text
        self.action = action
        self.insets = insets
        self.onAction = onAction
    }

    public var body: some View {
        VStack(spacing: 14) {
            Spacer().frame(height: 150)
            FerbMark(size: 84)
            Text(title).type(28, .semibold, ink.top).multilineTextAlignment(.center).padding(.top, 6).padding(.horizontal, 24)
                .accessibilityAddTraits(.isHeader)
            Spacer()
            Card(padding: 16) {
                Text(text).type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 4).padding(.bottom, 14)
                Pill(title: action, onClick: onAction)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}
