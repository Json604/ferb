import SwiftUI

// NOT FROM THE LAB. The sessions' live fixes (owner 2026-11-23; kb HAND-702, HAND-703, PHN-821, PHN-822), the twins of Android's
// Sessions pieces: an agent's reply drawn as one block of its parts (its Markdown through the chat's rich blocks, a
// thought, a step it ran, the turn's meta line; Cursor's look on the black page), FERB's word on a hand-over, a button
// that is on its way, the new session's sheet and its folder picker over the Files tab's browser. Stateless but for
// what a row shows open; the words are the app's.

/// One part of an agent's reply as the session screen draws it (HAND-703): its words (Markdown, as the chat's blocks),
/// a thought (`label` like "Thought 1s"; opens only when it has words), one step it ran (`label` then `detail` dimmer;
/// opens to its output in a fixed-width block when it has some), the turn's small meta line ("Just now", "Working…").
public enum SessionPartShown: Equatable, Sendable {
    case text([RichBlock])
    case thinking(label: String, text: String)
    case tool(label: String, detail: String, text: String)
    case meta(String)
}

extension SessionPage {
    /// The session's words on its black page: the agent line's size, white, the quiet ink for markers.
    static var look: RichLook { RichLook(size: 15.5, ink: Ink.white, dim: Ink.onDark, link: RichLook.onBlack.link, codeFill: Ink.darkFill) }
}

/// An agent's reply: its parts in order in one block, full width (a table needs the room), no bubble per line. Steps and
/// thoughts are closed until tapped.
public struct SessionReplyView: View {
    let key: String
    let parts: [SessionPartShown]
    @State private var open: Set<Int> = []
    public init(key: String, parts: [SessionPartShown]) { self.key = key; self.parts = parts }

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(parts.enumerated()), id: \.offset) { i, p in
                switch p {
                case .text(let blocks): SessionRichText(blocks, key: "session:\(key):\(i)")
                case .thinking(let label, let text): thought(i, label, text)
                case .tool(let label, let detail, let text): step(i, label, detail, text)
                case .meta(let words): Text(words).type(12.5, .medium, Ink.onDark).padding(.top, 2)
                }
            }
        }
        .padding(.horizontal, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func chevron(_ i: Int) -> some View {
        Image(systemName: open.contains(i) ? "chevron.down" : "chevron.right").font(.system(size: 11, weight: .semibold))
            .foregroundStyle(Ink.onDark).frame(width: 12)
    }

    private func toggle(_ i: Int) { if open.contains(i) { open.remove(i) } else { open.insert(i) } }

    @ViewBuilder private func thought(_ i: Int, _ label: String, _ text: String) -> some View {
        let opens = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text(label).type(14, .medium, Ink.onDark).lineLimit(1)
                if opens { chevron(i) }
            }
            .contentShape(Rectangle())
            .pressable(opens ? { toggle(i) } : nil, label: label)
            if opens, open.contains(i) {
                Text(text).type(14, .regular, Ink.onDark).lineSpacing(2).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 10)
            }
        }
    }

    @ViewBuilder private func step(_ i: Int, _ label: String, _ detail: String, _ text: String) -> some View {
        let opens = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                if opens { chevron(i) }
                Text(label).type(14, .medium, Ink.white).lineLimit(1).layoutPriority(1)
                if !detail.isEmpty { Text(detail).type(14, .regular, Ink.onDark).lineLimit(1) }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .contentShape(Rectangle())
            .pressable(opens ? { toggle(i) } : nil, label: detail.isEmpty ? label : "\(label), \(detail)")
            if opens, open.contains(i) {
                Rectangle().fill(Color.white.opacity(0.1)).frame(height: 1)
                Text(text).font(.system(size: 12.5, design: .monospaced)).foregroundStyle(Ink.onDark).lineSpacing(2)
                    .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 12).padding(.vertical, 10)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .background(Ink.darkFill.opacity(0.5), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Color.white.opacity(0.1), lineWidth: 1))
    }
}

/// Markdown blocks on the black page: words as one selectable text (the chat's rich text, kept per part), a code block
/// fixed-width in its own box, a table as a real table (rows and lines; one too wide for the screen scrolls sideways in
/// place, never the page).
public struct SessionRichText: View {
    let blocks: [RichBlock]
    let key: String
    public init(_ blocks: [RichBlock], key: String) { self.blocks = blocks; self.key = key }

    private enum Segment { case words([RichBlock]), code(String, String), table([[RichRun]], [[[RichRun]]]) }

    private var segments: [Segment] {
        var out: [Segment] = [], run: [RichBlock] = []
        func flush() { if !run.isEmpty { out.append(.words(run)); run = [] } }
        for b in blocks {
            switch b {
            case .table(let h, let r): flush(); out.append(.table(h, r))
            case .code(let t, let l): flush(); out.append(.code(t, l))
            default: run.append(b)
            }
        }
        flush()
        return out
    }

    public var body: some View {
        let look = SessionPage<EmptyView>.look
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(segments.enumerated()), id: \.offset) { i, seg in
                switch seg {
                case .words(let bs):
                    Text(RichTextCache.text("\(key):\(i)", bs, look)).lineSpacing(2).textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                case .code(let text, let language):
                    VStack(alignment: .leading, spacing: 6) {
                        if !language.isEmpty { Text(language).type(12, .medium, Ink.onDark) }
                        Text(text).font(.system(size: 13, design: .monospaced)).foregroundStyle(Ink.white).lineSpacing(2)
                            .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Ink.darkFill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                case .table(let header, let rows): SessionTable(header: header, rows: rows, look: look)
                }
            }
        }
    }
}

/// A Markdown table: its header row bold, a line between rows, a rounded edge. Two columns or fewer share the width and
/// wrap; more keep their words and scroll sideways when they don't fit.
struct SessionTable: View {
    let header: [[RichRun]]
    let rows: [[[RichRun]]]
    let look: RichLook

    var body: some View {
        let columns = max(header.count, rows.map(\.count).max() ?? 0)
        if columns <= 2 {
            grid(wraps: true)
        } else {
            ViewThatFits(in: .horizontal) {
                grid(wraps: false)
                ScrollView(.horizontal, showsIndicators: false) { grid(wraps: false) }
            }
        }
    }

    private func grid(wraps: Bool) -> some View {
        let all = [header] + rows
        let cellLook = RichLook(size: look.size - 1, ink: look.ink, dim: look.dim, link: look.link, codeFill: look.codeFill)
        return Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
            ForEach(Array(all.enumerated()), id: \.offset) { r, row in
                if r > 0 { Rectangle().fill(Color.white.opacity(0.12)).frame(height: 1).gridCellUnsizedAxes(.horizontal) }
                GridRow {
                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                        Text(richText([.paragraph(cell)], cellLook)).fontWeight(r == 0 ? .semibold : .regular)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.horizontal, 12).padding(.vertical, 9)
                            .frame(minWidth: 64, maxWidth: wraps ? .infinity : 240, alignment: .leading)
                    }
                }
            }
        }
        .background(r0Fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Color.white.opacity(0.14), lineWidth: 1))
    }

    private var r0Fill: Color { Ink.darkFill.opacity(0.35) }
}

/// FERB's latest word on the owner's hand-over (HAND-702), verbatim, with FERB's mark: one quiet line.
public struct SessionFerbNote: View {
    let text: String
    public init(_ text: String) { self.text = text }
    public var body: some View {
        HStack(alignment: .top, spacing: 9) {
            FerbMark(size: 20)
            Text(text).type(13.5, .medium, Ink.onDark).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 4).padding(.vertical, 8)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("session-ferb-note")
    }
}

/// A sheet's one button while what it started is on its way: a small turning arc and what is happening ("Handing it to
/// FERB…", "Opening a terminal…"); it takes no tap.
public struct BusyPill: View {
    let title: String
    public init(title: String) { self.title = title }
    public var body: some View {
        HStack(spacing: 10) {
            SessionArc(color: Ink.white).frame(width: 16, height: 16)
            Text(title).font(.system(size: 16, weight: .semibold))
        }
        .foregroundStyle(Ink.white)
        .frame(maxWidth: .infinity)
        .frame(height: 52)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Ink.black.opacity(0.72)))
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.updatesFrequently)
    }
}

/// Why the Mac refused what a sheet's button asked, in its own words, under the button.
public struct SheetRefusal: View {
    let text: String
    public init(_ text: String) { self.text = text }
    public var body: some View {
        HStack(alignment: .top, spacing: 8) {
            LampDot(color: Lamp.red, size: 7).padding(.top, 6)
            Text(text).type(14, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }
}

/// A sheet's text box (the app's Field), or, without a binding, its words drawn still (a still picture cannot draw a
/// text field).
public struct SheetField: View {
    let value: String
    let hint: String
    var text: Binding<String>?
    public init(value: String, hint: String, text: Binding<String>? = nil) { self.value = value; self.hint = hint; self.text = text }
    public var body: some View {
        ZStack(alignment: .leading) {
            if let text {
                TextField(hint, text: text, axis: .vertical).font(.system(size: 16)).foregroundStyle(Ink.black).lineLimit(1...4)
                    .autocorrectionDisabled().textInputAutocapitalization(.never)
            } else {
                Text(value.isEmpty ? hint : value).font(.system(size: 16)).foregroundStyle(value.isEmpty ? Ink.faint : Ink.black).lineLimit(4)
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 12).frame(maxWidth: .infinity, minHeight: 54, alignment: .leading)
        .background(Ink.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}

/// A sheet's list: scrolled within `height` when live, drawn whole in a still picture.
struct SheetList<Content: View>: View {
    let scrolls: Bool
    let height: CGFloat
    @ViewBuilder var content: Content
    var body: some View {
        if scrolls {
            ScrollView { content }.frame(maxHeight: height).fixedSize(horizontal: false, vertical: true)
        } else {
            content
        }
    }
}

/// One folder a list offers: its name, its place, its icon; `checked` when chosen; `locked` a key place's (it asks
/// for Face ID / Touch ID to open); `opens` when a tap opens something (a chevron) rather than choosing it.
public struct SessionFolderRow: Equatable, Sendable {
    public var title: String
    public var detail: String?
    public var symbol: String
    public var checked: Bool
    public var locked: Bool
    public var opens: Bool
    public init(title: String, detail: String? = nil, symbol: String = "folder", checked: Bool = false, locked: Bool = false,
                opens: Bool = false) {
        self.title = title; self.detail = detail; self.symbol = symbol; self.checked = checked; self.locked = locked; self.opens = opens
    }
}

/// Folders on the sheet's grey fill (a white card would vanish on the white sheet).
struct SessionFolderList: View {
    let rows: [SessionFolderRow]
    var onRow: ((Int) -> Void)?
    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { i, f in
                if i > 0 { Divide() }
                CardRow(title: f.title, detail: f.detail, onClick: onRow.map { g in { g(i) } }) {
                    RowIcon(symbol: f.symbol)
                } trailing: {
                    if f.checked {
                        Image(systemName: "checkmark").font(.system(size: 17, weight: .semibold)).foregroundStyle(Ink.black)
                    } else if f.locked {
                        Image(systemName: "lock.fill").font(.system(size: 15)).foregroundStyle(Ink.faint)
                    } else if f.opens {
                        Opens()
                    }
                }
                .accessibilityAddTraits(f.checked ? .isSelected : [])
            }
        }
        .background(Ink.fill, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
}

/// The new session's words (the app's catalog).
public struct NewSessionWords: Equatable, Sendable {
    public var title, folder, other, command, commandHint, start, starting: String
    public init(title: String, folder: String, other: String, command: String, commandHint: String, start: String, starting: String) {
        self.title = title; self.folder = folder; self.other = other; self.command = command; self.commandHint = commandHint
        self.start = start; self.starting = starting
    }
}

/// New session (HAND-691, PHN-821, PHN-822): where (the Mac's folders, the one chosen checked; "Another folder…" opens the Mac's
/// Files browser for folders) and an optional command, then Start. Starting: Start turns into "Opening a terminal…"
/// with its arc and the form takes no tap; refused: the Mac's reason under Start, the form as it was.
public struct NewSessionSheetView: View {
    let words: NewSessionWords
    let folders: [SessionFolderRow]
    let loading: Bool
    let command: String
    var commandText: Binding<String>?
    let starting: Bool
    let refused: String?
    var insets: PhoneInsets
    var scrolls: Bool
    var onFolder: ((Int) -> Void)?
    var onOther: (() -> Void)?
    var onStart: (() -> Void)?
    var onClose: (() -> Void)?

    public init(words: NewSessionWords, folders: [SessionFolderRow], loading: Bool = false, command: String = "",
                commandText: Binding<String>? = nil, starting: Bool = false, refused: String? = nil, insets: PhoneInsets = .lab,
                scrolls: Bool = true, onFolder: ((Int) -> Void)? = nil, onOther: (() -> Void)? = nil, onStart: (() -> Void)? = nil,
                onClose: (() -> Void)? = nil) {
        self.words = words; self.folders = folders; self.loading = loading; self.command = command; self.commandText = commandText
        self.starting = starting; self.refused = refused; self.insets = insets; self.scrolls = scrolls; self.onFolder = onFolder
        self.onOther = onOther; self.onStart = onStart; self.onClose = onClose
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: starting ? nil : onClose) {
            SheetTitle(words.title, onClose: starting ? nil : onClose)
            SheetList(scrolls: scrolls, height: 520) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(words.folder).type(15.5, .semibold, Ink.black).padding(.top, 12).padding(.bottom, 8)
                    if !folders.isEmpty || !loading {
                        SessionFolderList(rows: folders + [SessionFolderRow(title: words.other, symbol: "folder.badge.plus", opens: true)],
                                          onRow: starting ? nil : { i in i < folders.count ? onFolder?(i) : onOther?() })
                    } else {
                        ProgressView().frame(maxWidth: .infinity).padding(.vertical, 12)
                    }
                    Text(words.command).type(15.5, .semibold, Ink.black).padding(.top, 16).padding(.bottom, 8)
                    SheetField(value: command, hint: words.commandHint, text: starting ? nil : commandText)
                }
            }
            .opacity(starting ? 0.55 : 1)
            .allowsHitTesting(!starting)
            if starting {
                BusyPill(title: words.starting).padding(.top, 18)
            } else {
                Pill(title: words.start, kind: .black, onClick: onStart).padding(.top, 18)
            }
            if let refused, !starting { SheetRefusal(refused).padding(.top, 10) }
        }
    }
}

/// The new session's folder picker (PHN-821): the Mac's Files browser, folders only. At the top the places and the
/// recent folders; in a folder its folders (a tap opens one) and Back, and at the foot the path shown and "Use this
/// folder" (the folder being shown). Close goes back to the form as it was.
public struct SessionFolderPickerView: View {
    let title: String
    /// The folder shown (its name), nil at the top.
    let folder: String?
    let backLabel: String
    let rows: [SessionFolderRow]
    let recentLabel: String?
    let recent: [SessionFolderRow]
    /// A quiet line in place of the rows: none here, opening, why it couldn't be read.
    let line: String?
    let path: String
    let use: String
    var insets: PhoneInsets
    var scrolls: Bool
    var onUp: (() -> Void)?
    var onRow: ((Int) -> Void)?
    var onRecent: ((Int) -> Void)?
    var onUse: (() -> Void)?
    var onClose: (() -> Void)?

    public init(title: String, folder: String?, backLabel: String, rows: [SessionFolderRow], recentLabel: String? = nil,
                recent: [SessionFolderRow] = [], line: String? = nil, path: String, use: String, insets: PhoneInsets = .lab,
                scrolls: Bool = true, onUp: (() -> Void)? = nil, onRow: ((Int) -> Void)? = nil, onRecent: ((Int) -> Void)? = nil,
                onUse: (() -> Void)? = nil, onClose: (() -> Void)? = nil) {
        self.title = title; self.folder = folder; self.backLabel = backLabel; self.rows = rows; self.recentLabel = recentLabel
        self.recent = recent; self.line = line; self.path = path; self.use = use; self.insets = insets; self.scrolls = scrolls
        self.onUp = onUp; self.onRow = onRow; self.onRecent = onRecent; self.onUse = onUse; self.onClose = onClose
    }

    public var body: some View {
        Sheet(bottomInset: insets.bottom, onDismiss: onClose) {
            SheetTitle(title, onClose: onClose)
            if let folder {
                HStack(spacing: 10) {
                    Image(systemName: "chevron.left").font(.system(size: 18, weight: .semibold)).foregroundStyle(Ink.black)
                        .frame(width: 36, height: 36)
                        .pressable(onUp, label: backLabel)
                    Text(folder).type(16.5, .semibold, Ink.black).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.top, 10)
            }
            SheetList(scrolls: scrolls, height: 420) {
                VStack(alignment: .leading, spacing: 0) {
                    if !rows.isEmpty { SessionFolderList(rows: rows, onRow: onRow).padding(.top, 12) }
                    if let line {
                        Text(line).type(14.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 6).padding(.top, 14)
                    }
                    if let recentLabel, !recent.isEmpty {
                        Text(recentLabel).type(15.5, .semibold, Ink.black).padding(.top, 16).padding(.bottom, 8)
                        SessionFolderList(rows: recent, onRow: onRecent)
                    }
                }
            }
            // the folder shown and the one button that chooses it (at the top nothing is shown to choose)
            if folder != nil {
                VStack(alignment: .leading, spacing: 10) {
                    if !path.isEmpty {
                        Text(path).type(13.5, .regular, Ink.detail).lineLimit(1).truncationMode(.middle)
                            .padding(.horizontal, 6)
                    }
                    Pill(title: use, kind: .black, symbol: "checkmark", onClick: onUse)
                }
                .padding(.top, 16)
            }
        }
    }
}
