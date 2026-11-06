import AppKit
import SwiftUI
import NotchCore

extension NSAttributedString.Key {
    static let messageBlockFill = NSAttributedString.Key("FERBMessageBlockFill")
}

/// One native selection surface per message, using FERB's owned glyph renderer.
struct NativeMessageText: NSViewRepresentable {
    let text: NSAttributedString
    @Environment(\.selectableTextGeneration) private var generation

    init(_ text: NSAttributedString) { self.text = text }

    init(_ source: String, size: CGFloat = 12, secondary: Bool = false,
         markdown: Bool = false, bold: Bool = false, monospaced: Bool = false,
         alignment: NSTextAlignment = .left) {
        text = MessageTextAttributes.make(source, size: size, secondary: secondary,
                                         markdown: markdown, bold: bold, monospaced: monospaced,
                                         alignment: alignment)
    }

    func makeNSView(context: Context) -> MessageTextView { MessageTextView() }
    func updateNSView(_ view: MessageTextView, context: Context) {
        view.update(text, generation: generation)
    }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: MessageTextView, context: Context) -> CGSize? {
        nsView.measuredSize(width: proposal.width)
    }
}

enum MessageTextAttributes {
    static func joined(_ parts: [NSAttributedString]) -> NSAttributedString {
        let result = NSMutableAttributedString(string: "")
        for part in parts where part.length > 0 {
            if result.length > 0 { result.append(NSAttributedString(string: "\n\n", attributes: [.font: NSFont.systemFont(ofSize: 7)])) }
            result.append(part)
        }
        return result
    }

    static func make(_ source: String, size: CGFloat, secondary: Bool, markdown: Bool,
                     bold: Bool, monospaced: Bool, alignment: NSTextAlignment) -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        let color = (secondary ? NSColor.secondaryLabelColor : NSColor.labelColor)
        func attributes(_ intent: InlinePresentationIntent? = nil) -> [NSAttributedString.Key: Any] {
            let code = monospaced || intent?.contains(.code) == true
            var font = code ? NSFont.monospacedSystemFont(ofSize: monospaced ? size : 11, weight: .regular)
                : NSFont.systemFont(ofSize: size, weight: bold || intent?.contains(.stronglyEmphasized) == true ? .semibold : .regular)
            if intent?.contains(.emphasized) == true { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
            var result: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: paragraph]
            if intent?.contains(.strikethrough) == true { result[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if intent?.contains(.code) == true { result[.backgroundColor] = NSColor.white.withAlphaComponent(0.06) }
            return result
        }
        guard markdown else { return NSAttributedString(string: source, attributes: attributes()) }
        let parsed = MarkdownDocument.inline(source)
        let result = NSMutableAttributedString(string: "")
        for run in parsed.runs {
            var style = attributes(run.inlinePresentationIntent)
            if let url = run.link, MarkdownDocument.safeLink(url) {
                style[.link] = url
                style[.foregroundColor] = NSColor.linkColor     // the link's colour lives in the text: hover lifts it
            }
            result.append(NSAttributedString(string: String(parsed[run.range].characters), attributes: style))
        }
        return result
    }
}

final class MessageTextView: NSTextView, NSTextViewDelegate {
    private var revealGeneration: Int?
    /// The link under the pointer, drawn lifted (APP-753, `HoverLook`): a temporary attribute, never the text.
    private(set) var hoveredLink: NSRange?
    private var hoverArea: NSTrackingArea?
    private var linkAreas: [NSTrackingArea] = []
    private var measuredWidths: [CGFloat: CGSize] = [:]
    private(set) var displayRefreshCount = 0

    init() {
        let storage = NSTextStorage()
        let manager = NSLayoutManager()
        let container = NSTextContainer(size: CGSize(width: 1, height: CGFloat.greatestFiniteMagnitude))
        storage.addLayoutManager(manager)
        manager.addTextContainer(container)
        container.lineFragmentPadding = 0
        container.widthTracksTextView = false
        container.heightTracksTextView = false
        super.init(frame: .zero, textContainer: container)
        isEditable = false
        isSelectable = true
        drawsBackground = false
        textContainerInset = .zero
        isHorizontallyResizable = false
        isVerticallyResizable = false
        minSize = .zero
        maxSize = CGSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        delegate = self
        // No colour here, so a hovered link's temporary colour shows; the text carries the link colour itself.
        linkTextAttributes = [.cursor: NSCursor.pointingHand]
        setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func update(_ value: NSAttributedString, generation: Int) {
        if !attributedString().isEqual(to: value) {
            let selection = selectedRange()
            textStorage?.setAttributedString(value)
            let location = min(selection.location, value.length)
            setSelectedRange(NSRange(location: location, length: min(selection.length, value.length - location)))
            measuredWidths.removeAll(keepingCapacity: true)
            invalidateIntrinsicContentSize()
            refreshGlyphDisplay()
        }
        if revealGeneration != generation {
            revealGeneration = generation
            refreshGlyphDisplay()
        }
    }

    func measuredSize(width proposedWidth: CGFloat?) -> CGSize {
        let width = max(1, proposedWidth.flatMap { $0.isFinite ? $0 : nil } ?? 1_000_000)
        if let cached = measuredWidths[width] { return cached }
        guard let container = textContainer, let manager = layoutManager else { return .zero }
        let previousSize = container.containerSize
        container.containerSize = CGSize(width: width, height: CGFloat.greatestFiniteMagnitude)
        manager.ensureLayout(for: container)
        let used = manager.boundingRect(forGlyphRange: NSRange(location: 0, length: manager.numberOfGlyphs), in: container)
        let size = CGSize(width: min(width, ceil(used.maxX)), height: ceil(max(used.maxY, manager.usedRect(for: container).maxY, manager.extraLineFragmentUsedRect.maxY)))
        container.containerSize = previousSize
        manager.ensureLayout(for: container)
        measuredWidths[width] = size
        return size
    }

    override func layout() {
        super.layout()
        defer { if window != nil { updateTrackingAreas() } }   // the links' areas follow the laid-out text
        guard bounds.width > 0, let container = textContainer, container.containerSize.width != bounds.width else { return }
        container.containerSize = CGSize(width: bounds.width, height: CGFloat.greatestFiniteMagnitude)
        layoutManager?.ensureLayout(for: container)
        refreshGlyphDisplay()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        refreshGlyphDisplay()
    }

    override func draw(_ dirtyRect: NSRect) {
        // Rounded decorations stay behind the native glyphs and selection; they
        // never split a code block, quote or table into another selection surface.
        if let storage = textStorage, let manager = layoutManager, let container = textContainer {
            storage.enumerateAttribute(.messageBlockFill, in: NSRange(location: 0, length: storage.length)) { fill, range, _ in
                guard let fill = fill as? NSColor else { return }
                let glyphs = manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
                let rect = manager.boundingRect(forGlyphRange: glyphs, in: container)
                    .offsetBy(dx: textContainerOrigin.x, dy: textContainerOrigin.y)
                    .insetBy(dx: -6, dy: -4).intersection(bounds)
                fill.setFill()
                NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6).fill()
            }
        }
        super.draw(dirtyRect)
    }

    private func refreshGlyphDisplay() {
        guard let storage = textStorage else { return }
        layoutManager?.invalidateDisplay(forCharacterRange: NSRange(location: 0, length: storage.length))
        needsDisplay = true
        displayRefreshCount += 1
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverArea { removeTrackingArea(hoverArea) }
        for area in linkAreas { removeTrackingArea(area) }
        // Always active: FERB never activates (APP-756). One area for the whole text (leaving it settles any hover) and
        // one per visible piece of each link, whose entering and leaving AppKit reports even while another app is in
        // front, where moves inside a view may never arrive.
        // owned by a small responder of its own: NSTextView keeps its own pointer tracking, and the live check showed
        // areas it owns never hearing the pointer while another app was in front
        let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                  owner: linkOwner, userInfo: nil)
        addTrackingArea(area)
        hoverArea = area
        linkAreas = linkRects().map { rect, range in
            let piece = NSTrackingArea(rect: rect, options: [.mouseEnteredAndExited, .cursorUpdate, .activeAlways],
                                       owner: linkOwner, userInfo: ["location": range.location, "length": range.length])
            addTrackingArea(piece)
            return piece
        }
        logLinkAreas()
    }

    /// Each link's visible rectangles in this view's coordinates, with its character range.
    private func linkRects() -> [(CGRect, NSRange)] {
        guard let storage = textStorage, let manager = layoutManager, let container = textContainer,
              storage.length > 0 else { return [] }
        var out: [(CGRect, NSRange)] = []
        storage.enumerateAttribute(.link, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
            guard value != nil else { return }
            let glyphs = manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
            manager.enumerateEnclosingRects(forGlyphRange: glyphs, withinSelectedGlyphRange: NSRange(location: NSNotFound,
                                            length: 0), in: container) { rect, _ in
                let shown = rect.offsetBy(dx: self.textContainerOrigin.x, dy: self.textContainerOrigin.y)
                    .intersection(self.visibleRect)
                if !shown.isEmpty { out.append((shown, range)) }
            }
        }
        return out
    }

    private lazy var linkOwner = LinkHoverOwner(view: self)

    func pointerEntered(_ event: NSEvent) {
        if let info = event.trackingArea?.userInfo, let location = info["location"] as? Int,
           let length = info["length"] as? Int {
            lift(NSRange(location: location, length: length))
        }
    }

    func pointerMoved(_ event: NSEvent) {
        hover(at: convert(event.locationInWindow, from: nil))
    }

    func pointerExited(_ event: NSEvent) {
        if let info = event.trackingArea?.userInfo, let location = info["location"] as? Int {
            if hoveredLink?.location == location { lift(nil) }
        } else {
            lift(nil)                                     // left the text: no link stays lifted
        }
    }

    func pointerCursorUpdate() {
        if hoveredLink != nil { NSCursor.pointingHand.set() }
    }

    /// Lifts the link under `point` (none: every link at rest).
    func hover(at point: NSPoint?) {
        var range: NSRange?
        if let point, let manager = layoutManager, let container = textContainer, let storage = textStorage,
           storage.length > 0 {
            let inside = CGPoint(x: point.x - textContainerOrigin.x, y: point.y - textContainerOrigin.y)
            var fraction: CGFloat = 0
            let glyph = manager.glyphIndex(for: inside, in: container, fractionOfDistanceThroughGlyph: &fraction)
            let rect = manager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container)
            let index = manager.characterIndexForGlyph(at: glyph)
            var effective = NSRange(location: 0, length: 0)
            if rect.contains(inside), index < storage.length,
               storage.attribute(.link, at: index, effectiveRange: &effective) != nil {
                range = effective
            }
        }
        lift(range)
    }

    /// The link drawn lifted (a temporary colour, never the text) with the pointing hand over it; nil: none.
    private func lift(_ range: NSRange?) {
        if range != nil {                                 // again on each change: the window server may reset it
            BackgroundCursor.enable()
            NSCursor.pointingHand.set()
        }
        guard range != hoveredLink else { return }
        if let old = hoveredLink, let manager = layoutManager {
            manager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: old)
            if let storage = textStorage, NSMaxRange(old) <= storage.length {
                manager.invalidateDisplay(forCharacterRange: old)
            }
        }
        let had = hoveredLink != nil
        hoveredLink = range
        if let range, let manager = layoutManager {
            manager.addTemporaryAttribute(.foregroundColor, value: NSColor.linkColor.hoverLifted(true),
                                          forCharacterRange: range)
            manager.invalidateDisplay(forCharacterRange: range)
        } else if had {
            NSCursor.arrow.set()                              // off the link: never left as the hand
        }
        HoverSourceView.log.info("hover \(range != nil ? "on" : "off", privacy: .public) link pointing=true")
        needsDisplay = true
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        super.viewWillMove(toWindow: newWindow)
        if hoveredLink != nil { lift(nil) }
    }

    /// Where each link sits on screen, for a live check of the hover (numbers only, debug level).
    private func logLinkAreas() {
        guard let window else { return }
        for (rect, _) in linkRects() {
            let shown = window.convertToScreen(convert(rect, to: nil))
            HoverSourceView.log.debug("hover area link \(Int(shown.minX)) \(Int(shown.minY)) \(Int(shown.width)) \(Int(shown.height))")
        }
    }

    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        let url = (link as? URL) ?? (link as? String).flatMap(URL.init(string:))
        if let url, MarkdownDocument.safeLink(url) { HandOff.open(url) }
        return true // Never delegate unsupported URLs to AppKit's default opener.
    }
}

/// The owner of a message's link tracking areas (APP-757): it hands the pointer's comings and goings to its text view.
final class LinkHoverOwner: NSResponder {
    private weak var view: MessageTextView?
    init(view: MessageTextView) {
        self.view = view
        super.init()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func mouseEntered(with event: NSEvent) { view?.pointerEntered(event) }
    override func mouseMoved(with event: NSEvent) { view?.pointerMoved(event) }
    override func mouseExited(with event: NSEvent) { view?.pointerExited(event) }
    override func cursorUpdate(with event: NSEvent) { view?.pointerCursorUpdate() }
}
