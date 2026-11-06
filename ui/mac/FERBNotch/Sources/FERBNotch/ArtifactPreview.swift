import AppKit
import NotchCore
import SwiftUI

// File cards and links in the chat (APP-715, owner 2026-11-05): nothing opens on hover; a click reveals the file in
// Finder or opens the link; the name can be selected and copied. FERB's own views hold no Quick Look view: Quick Look
// is the system's panel (Space, the context menu), so a closing popover can't tear down an unactivated preview.

/// One action a file card or link offers in its context menu, shared by SwiftUI's menu and the name's own.
struct CardAction: Identifiable {
    let title: String
    let run: () -> Void
    var id: String { title }

    @MainActor
    static func file(_ url: URL, model: NotchModel, card: FileCardState) -> [CardAction] {
        [CardAction(title: "Open in default app") { model.onOpenDocument?(url) },
         CardAction(title: "Quick Look") { model.onQuickLook?(url) },
         CardAction(title: "Reveal in Finder") { card.reveal(url, with: model) },
         CardAction(title: "Copy path") { model.copyText(url.path) }]
    }

    @MainActor
    static func link(_ host: String, model: NotchModel) -> [CardAction] {
        guard let link = VisitedSites.link(host) else { return [] }
        return [CardAction(title: "Open in browser") { model.openURL(link) },
                CardAction(title: "Copy link") { model.copyText(link.absoluteString) }]
    }
}

/// Whether a card's file was missing when the owner last clicked it.
@MainActor
final class FileCardState: ObservableObject {
    static let unavailableLine = "This file is no longer available."
    @Published private(set) var unavailable = false
    func reveal(_ url: URL, with model: NotchModel) { unavailable = !model.revealFile(url) }
}

struct FileUnavailableLine: View {
    var body: some View {
        Text(FileCardState.unavailableLine).font(.system(size: 10)).foregroundStyle(Palette.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// The shared hover look (APP-753, `HoverLook`): the card's fill brightens under the pointer, a link's text lifts
/// towards white and the pointer becomes the pointing hand; only colour changes, nothing opens (APP-715). Reduce Motion
/// changes it at once.
struct CardHover: ViewModifier {
    var link = false
    var cornerRadius: CGFloat = 9
    var capsule = false
    /// Draws the fill; off when the content draws its own from `hovered`.
    var drawsFill = true
    let hovered: Binding<Bool>
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pushed = false

    func body(content: Content) -> some View {
        content
            .background {
                if drawsFill {
                    if capsule { Capsule().fill(.white.opacity(HoverLook.fill(hovered: hovered.wrappedValue))) }
                    else {
                        RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                            .fill(.white.opacity(HoverLook.fill(hovered: hovered.wrappedValue)))
                    }
                }
            }
            .onHover { inside in
                withAnimation(HoverLook.duration(reduceMotion: reduceMotion).map { .easeOut(duration: $0) }) {
                    hovered.wrappedValue = inside
                }
                guard link else { return }
                if inside && !pushed { NSCursor.pointingHand.push(); pushed = true }
                else if !inside && pushed { NSCursor.pop(); pushed = false }
            }
            .onDisappear { if pushed { NSCursor.pop(); pushed = false } }
    }
}

extension View {
    func cardHover(_ hovered: Binding<Bool>, link: Bool = false, cornerRadius: CGFloat = 9, capsule: Bool = false,
                   drawsFill: Bool = true) -> some View {
        modifier(CardHover(link: link, cornerRadius: cornerRadius, capsule: capsule, drawsFill: drawsFill,
                           hovered: hovered))
    }
}

extension NSColor {
    /// A link's colour under the pointer (`HoverLook.linkLift`).
    func hoverLifted(_ hovered: Bool) -> NSColor {
        let lift = HoverLook.linkLift(hovered: hovered)
        return lift > 0 ? (blended(withFraction: lift, of: .white) ?? self) : self
    }
}

/// The text of a file card or link: selectable and copyable, while a plain click acts (APP-716). A drag or a
/// double-click selects (Command-C copies through NotchPanel's routing); a single click that leaves nothing
/// selected runs `click`. The card around it keeps its own click, drag-out and context menu.
struct SelectableName: NSViewRepresentable {
    let text: String
    var size: CGFloat = 11
    var weight: NSFont.Weight = .regular
    var color: NSColor = .labelColor
    let click: () -> Void
    var actions: [CardAction] = []

    func makeNSView(context: Context) -> SelectableNameView { SelectableNameView() }
    func updateNSView(_ view: SelectableNameView, context: Context) {
        view.click = click
        view.actions = actions
        view.show(text, font: .systemFont(ofSize: size, weight: weight), color: color)
    }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: SelectableNameView, context: Context) -> CGSize? {
        nsView.measuredSize(width: proposal.width)
    }
}

final class SelectableNameView: NSTextView {
    var click: (() -> Void)?
    var actions: [CardAction] = []
    private var shownFont: NSFont = .systemFont(ofSize: 11)

    init() {
        let storage = NSTextStorage()
        let manager = NSLayoutManager()
        let container = NSTextContainer(size: CGSize(width: 1, height: CGFloat.greatestFiniteMagnitude))
        storage.addLayoutManager(manager)
        manager.addTextContainer(container)
        container.lineFragmentPadding = 0
        container.maximumNumberOfLines = 1
        container.lineBreakMode = .byTruncatingMiddle
        container.widthTracksTextView = true
        super.init(frame: .zero, textContainer: container)
        isEditable = false
        isSelectable = true
        isRichText = false
        drawsBackground = false
        textContainerInset = .zero
        isHorizontallyResizable = false
        isVerticallyResizable = false
        setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func show(_ text: String, font: NSFont, color: NSColor) {
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingMiddle
        let value = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color,
                                                                   .paragraphStyle: paragraph])
        guard !attributedString().isEqual(to: value) else { return }
        shownFont = font
        textStorage?.setAttributedString(value)
        invalidateIntrinsicContentSize()
    }

    /// One line: the whole name when it fits, else the offered width (the middle is truncated).
    func measuredSize(width: CGFloat?) -> CGSize {
        let full = ceil(attributedString().size().width) + 1
        let offered = width.flatMap { $0.isFinite ? $0 : nil } ?? full
        let height = ceil(layoutManager?.defaultLineHeight(for: shownFont) ?? shownFont.pointSize * 1.25)
        return CGSize(width: max(1, min(full, offered)), height: height)
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        super.mouseDown(with: event)        // AppKit's selection tracking; returns when the button comes up
        finishPress(clickCount: event.clickCount)
    }

    /// A single click that selected nothing is the card's click; a drag or a double-click only selected.
    func finishPress(clickCount: Int) {
        if clickCount == 1, selectedRange().length == 0 { click?() }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = NSMenu()
        if selectedRange().length > 0 {
            menu.addItem(NSMenuItem(title: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: ""))
            if !actions.isEmpty { menu.addItem(.separator()) }
        }
        for action in actions { menu.addItem(ActionMenuItem(action)) }
        return menu.items.isEmpty ? nil : menu
    }
}

final class ActionMenuItem: NSMenuItem {
    private let run: () -> Void
    init(_ action: CardAction) {
        run = action.run
        super.init(title: action.title, action: #selector(runAction), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func runAction() { run() }
}

/// What every file card has besides its click: dragging the file out and the context menu (APP-573).
struct FileCardMenu: ViewModifier {
    let url: URL
    @ObservedObject var model: NotchModel
    @ObservedObject var card: FileCardState
    func body(content: Content) -> some View {
        content
            .onDrag { NSItemProvider(object: url as NSURL) }
            .contextMenu {
                ForEach(CardAction.file(url, model: model, card: card)) { action in Button(action.title, action: action.run) }
            }
    }
}

/// A file in a reply: a click reveals it in Finder, selected.
struct ArtifactRow: View {
    let url: URL
    @ObservedObject var model: NotchModel
    @StateObject private var card = FileCardState()
    @State private var hovered = false
    func reveal() { card.reveal(url, with: model) }
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Button(action: reveal) {
                HStack(spacing: 6) {
                    Image(systemName: "doc")
                    SelectableName(text: url.lastPathComponent, size: 12, weight: .medium, click: reveal,
                                   actions: CardAction.file(url, model: model, card: card))
                }
                .padding(.horizontal, 9).frame(height: 28)
                .cardHover($hovered)
                .contentShape(RoundedRectangle(cornerRadius: 9))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Show \(url.lastPathComponent) in Finder")
            .modifier(FileCardMenu(url: url, model: model, card: card))
            if card.unavailable { FileUnavailableLine().padding(.leading, 7) }
        }
    }
}

/// A file attached to the composer: the same click, menu and drag as a reply's file.
struct ComposerAttachmentCard: View {
    let url: URL
    @ObservedObject var model: NotchModel
    @StateObject private var card = FileCardState()
    @State private var hovered = false
    func reveal() { card.reveal(url, with: model) }
    var body: some View {
        Button(action: reveal) {
            VStack(alignment: .leading, spacing: 7) {
                if card.unavailable {
                    FileUnavailableLine().frame(maxWidth: .infinity, maxHeight: 32, alignment: .leading)
                } else {
                    Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                        .resizable().scaledToFit().frame(width: 30, height: 32)
                        .frame(maxWidth: .infinity)
                }
                SelectableName(text: url.lastPathComponent, size: 11, click: reveal,
                               actions: CardAction.file(url, model: model, card: card))
            }
            .padding(10).frame(width: 116, height: 84)
            .background(.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 10))
            .cardHover($hovered, cornerRadius: 10)
            .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Show \(url.lastPathComponent) in Finder")
        .modifier(FileCardMenu(url: url, model: model, card: card))
        .overlay(alignment: .topTrailing) {
            Button { model.removeAttachment(url) } label: {
                Image(systemName: "xmark.circle.fill")
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.secondary, .background)
                    .font(.system(size: 14)).frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(url.lastPathComponent)")
        }
    }
}

/// The sites row (APP-700, owner 2026-11-04: "we decided on one site plus rest under +x"): the first site, then
/// "+N" opening the list of all of them. Link cards and the older "Sites contacted" text both show this way.
struct VisitedSites {
    let all: [String]
    init(_ source: String) {
        self.init(hosts: source.split(separator: ",").map(String.init))
    }
    init(hosts: [String]) {
        var seen = Set<String>()
        all = hosts.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty && seen.insert($0).inserted }
    }
    var inline: String { all.first ?? "" }
    var remainingCount: Int { max(0, all.count - 1) }

    private static let hostCharacters = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: ".-:[]"))

    /// The page a site row opens: https:// and the host only. Anything that isn't a plain host (a scheme, a path, a
    /// user part, spaces) opens nothing, and the result still passes the answers' safe-link rule (APP-589).
    static func link(_ host: String) -> URL? {
        guard !host.isEmpty, host.unicodeScalars.allSatisfy(hostCharacters.contains),
              let url = URL(string: "https://" + host), url.host?.isEmpty == false, url.user == nil,
              url.path.isEmpty, url.query == nil, url.fragment == nil, MarkdownDocument.safeLink(url) else { return nil }
        return url
    }
}

/// One site, as a link: a click opens it in the default browser; the text can be selected and copied.
struct SiteLink: View {
    let host: String
    @ObservedObject var model: NotchModel
    var size: CGFloat = 10
    var color: NSColor = .linkColor
    /// The hover of the chip around it, when it has one; else the link's own.
    var hovered: Bool? = nil
    @State private var own = false
    var body: some View {
        SelectableName(text: host, size: size, color: color.hoverLifted(hovered ?? own), click: { model.openSite(host) },
                       actions: CardAction.link(host, model: model))
            .cardHover($own, link: true, drawsFill: false)
            .accessibilityLabel("Open \(host)")
    }
}

/// The "+N" overflow count in its own small rounded box (APP-753), brighter under the pointer like every link.
struct OverflowBadge: View {
    let count: Int
    @State private var hovered = false
    var body: some View {
        if let text = HoverLook.overflow(count) {
            Text(text).fixedSize()
                .font(.system(size: 9.5, weight: .semibold)).monospacedDigit()
                .foregroundStyle(hovered ? Palette.ink : Palette.secondary)
                .padding(.horizontal, 5).frame(height: 15)
                .background(.white.opacity(HoverLook.badgeFill(hovered: hovered)),
                            in: RoundedRectangle(cornerRadius: 4.5, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 4.5))
                .cardHover($hovered, link: true, drawsFill: false)
        }
    }
}

struct VisitedSitesView: View {
    let sites: VisitedSites
    @ObservedObject var model: NotchModel
    var leading: CGFloat = 10
    @State private var listOpen = false
    @State private var hovered = false
    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: "globe").foregroundStyle(hovered ? Palette.ink : Palette.secondary)
            SiteLink(host: sites.inline, model: model, hovered: hovered)
            if sites.remainingCount > 0 {
                Button { listOpen = true } label: { OverflowBadge(count: sites.remainingCount) }
                .buttonStyle(.plain)
                .accessibilityLabel("Show all \(sites.all.count) visited sites")
                .popover(isPresented: $listOpen) { list }
            }
        }
        .font(.system(size: 10))
        .padding(.horizontal, 8).padding(.vertical, 4)
        .cardHover($hovered, capsule: true)
        .padding(.leading, leading)
        .onChange(of: model.expanded) { _, visible in if !visible { listOpen = false } }
        .onChange(of: model.previewDismissalRevision) { _, _ in listOpen = false }
        .onChange(of: sites.all) { _, _ in listOpen = false }
        .onDisappear { listOpen = false }
    }

    private var list: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Text("Sites visited").font(.headline).padding(.bottom, 12)
                ForEach(Array(sites.all.enumerated()), id: \.offset) { index, host in
                    if index > 0 { Divider().padding(.vertical, 10) }
                    HStack(spacing: 6) {
                        Image(systemName: "globe")
                        SiteLink(host: host, model: model, size: 13)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(16)
        }
        .frame(width: 300, height: min(300, CGFloat(sites.all.count) * 42 + 58))
        .background(TaskPreviewWindow(model: model))
    }
}
