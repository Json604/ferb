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
            if let url = run.link, MarkdownDocument.safeLink(url) { style[.link] = url }
            result.append(NSAttributedString(string: String(parsed[run.range].characters), attributes: style))
        }
        return result
    }
}

final class MessageTextView: NSTextView, NSTextViewDelegate {
    private var revealGeneration: Int?
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
        linkTextAttributes = [.foregroundColor: NSColor.linkColor, .cursor: NSCursor.pointingHand]
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

    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        let url = (link as? URL) ?? (link as? String).flatMap(URL.init(string:))
        if let url, MarkdownDocument.safeLink(url) { NSWorkspace.shared.open(url) }
        return true // Never delegate unsupported URLs to AppKit's default opener.
    }
}
