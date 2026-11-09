import AppKit
import SwiftUI
import NotchCore

extension NSAttributedString.Key {
    static let messageBlockFill = NSAttributedString.Key("FERBMessageBlockFill")
}

private struct MessageTextShownHeightKey: EnvironmentKey {
    static let defaultValue: CGFloat? = nil
}
/// CollapsibleText's fold (APP-719, APP-809): the text view folds itself to it in one layout pass.
struct MessageFold {
    let kind: CollapsePolicy.Kind
    let open: Bool
    let more: String
    /// Lines shown under the fade only while collapsed (an answer's "Couldn't confirm" and code's lines).
    let tail: NSAttributedString?
    let lineHeight: CGFloat
    let toggle: () -> Void

    static let spacing: CGFloat = 3
}

private struct MessageFoldKey: EnvironmentKey {
    static let defaultValue: MessageFold? = nil
}
extension EnvironmentValues {
    var messageTextFold: MessageFold? {
        get { self[MessageFoldKey.self] }
        set { self[MessageFoldKey.self] = newValue }
    }
    /// While CollapsibleText is collapsed: how much of its text shows, faded at the bottom (APP-721). The text view
    /// clips and fades itself, so no SwiftUI clip or mask wraps chat text (APP-788).
    var messageTextShownHeight: CGFloat? {
        get { self[MessageTextShownHeightKey.self] }
        set { self[MessageTextShownHeightKey.self] = newValue }
    }
}

/// One native selection surface per message, using FERB's owned glyph renderer. All chat text draws through it, never
/// SwiftUI's selectable `Text`, so the notch's reveal refresh reaches every glyph (APP-788).
struct NativeMessageText: NSViewRepresentable {
    let text: NSAttributedString
    @Environment(\.selectableTextGeneration) private var generation
    @Environment(\.messageTextShownHeight) private var shownHeight

    init(_ text: NSAttributedString) { self.text = text }

    init(_ source: String, size: CGFloat = 12, secondary: Bool = false,
         markdown: Bool = false, bold: Bool = false, monospaced: Bool = false,
         alignment: NSTextAlignment = .left, color: NSColor? = nil) {
        text = MessageTextAttributes.make(source, size: size, secondary: secondary,
                                         markdown: markdown, bold: bold, monospaced: monospaced,
                                         alignment: alignment, color: color)
    }

    func makeNSView(context: Context) -> MessageTextView { MessageTextView() }
    func updateNSView(_ view: MessageTextView, context: Context) {
        view.update(text, generation: generation)
        view.fold = context.environment.messageTextFold
        if view.fold == nil { view.shownHeight = shownHeight }
    }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: MessageTextView, context: Context) -> CGSize? {
        nsView.fold = context.environment.messageTextFold
        return nsView.foldedSize(width: proposal.width)
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

    /// `color` replaces the label colours (a failed row's red, a note's amber); links keep theirs.
    static func make(_ source: String, size: CGFloat, secondary: Bool, markdown: Bool,
                     bold: Bool, monospaced: Bool, alignment: NSTextAlignment,
                     color: NSColor? = nil) -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        let color = color ?? (secondary ? NSColor.secondaryLabelColor : NSColor.labelColor)
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
    /// Collapsed (APP-721): only this much of the text shows, its last part fading out; the rest is neither drawn nor
    /// clickable. Nil: all of it.
    var shownHeight: CGFloat? {
        didSet { if shownHeight != oldValue { refreshGlyphDisplay() } }
    }
    /// Inside CollapsibleText (APP-809): the text measured at the width it is given and folded in the same pass; its
    /// Show more / Show less and, collapsed, the lines kept under the fade are this view's own subviews, below the text.
    var fold: MessageFold? {
        didSet {
            guard fold != nil || oldValue != nil else { return }
            foldControl.isHidden = true
            tailView?.isHidden = true
            needsLayout = true
        }
    }
    private lazy var foldControl: FoldButton = {
        let button = FoldButton(frame: .zero)
        button.isBordered = false
        button.setButtonType(.momentaryChange)
        button.target = button
        button.action = #selector(FoldButton.pressed)
        button.isHidden = true
        addSubview(button)
        return button
    }()
    private var tailView: MessageTextView?

    final class FoldButton: NSButton {
        var run: () -> Void = {}
        @objc func pressed() { run() }
    }

    /// What the fold shows at `width`: the text's height (all of it, or the clamp), then the lines kept under a
    /// collapsed text, then the control; nil parts when the text is too short to fold.
    private func foldPlan(width: CGFloat?) -> (natural: CGSize, text: CGFloat, tail: CGFloat?, control: CGSize?) {
        let natural = measuredSize(width: width)
        guard let fold, let clamp = CollapsePolicy.clamp(fold.kind, natural: natural.height, lineHeight: fold.lineHeight)
        else { return (natural, natural.height, nil, nil) }
        let control = controlTitle(fold).size()
        let tail = !fold.open ? fold.tail.map { tail -> CGFloat in
            let view = tailText(tail)
            return view.measuredSize(width: width).height
        } : nil
        return (natural, fold.open ? natural.height : clamp, tail,
                CGSize(width: ceil(control.width), height: ceil(control.height)))
    }

    func foldedSize(width: CGFloat?) -> CGSize {
        let plan = foldPlan(width: width)
        guard let control = plan.control else { return plan.natural }
        let height = plan.text + (plan.tail.map { $0 + MessageFold.spacing } ?? 0) + MessageFold.spacing + control.height
        let width = width.flatMap { $0.isFinite ? $0 : nil } ?? max(plan.natural.width, control.width)
        return CGSize(width: width, height: height)
    }

    private func controlTitle(_ fold: MessageFold) -> NSAttributedString {
        NSAttributedString(string: fold.open ? "Show less" : fold.more,
                           attributes: [.font: NSFont.systemFont(ofSize: 10.5, weight: .medium),
                                        .foregroundColor: NSColor(Palette.chatBlue)])
    }

    private func tailText(_ tail: NSAttributedString) -> MessageTextView {
        if let tailView {
            if !tailView.attributedString().isEqual(to: tail) { tailView.update(tail, generation: 0) }
            return tailView
        }
        let view = MessageTextView()
        view.update(tail, generation: 0)
        view.isHidden = true
        addSubview(view)
        tailView = view
        return view
    }

    /// Places the fold's parts in this view's bounds: the text shows down to its height (faded when cut), the lines
    /// kept under it and the control below.
    private func layoutFold() {
        guard let fold else { return }
        let plan = foldPlan(width: bounds.width)
        guard let control = plan.control else {
            shownHeight = nil
            foldControl.isHidden = true
            tailView?.isHidden = true
            return
        }
        shownHeight = fold.open ? nil : plan.text
        var y = plan.text
        if let tail = plan.tail, let tailView {
            y += MessageFold.spacing
            tailView.frame = CGRect(x: 0, y: y, width: bounds.width, height: tail)
            tailView.isHidden = false
            y += tail
        } else {
            tailView?.isHidden = true
        }
        foldControl.run = fold.toggle
        let title = controlTitle(fold)
        if foldControl.attributedTitle != title { foldControl.attributedTitle = title }
        foldControl.setAccessibilityHelp(fold.open ? "Shows the first lines" : "Shows the whole text")
        foldControl.frame = CGRect(x: 0, y: y + MessageFold.spacing, width: control.width, height: control.height)
        foldControl.isHidden = false
    }

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
        wantsLayer = true                                  // its own backing: a collapsed fade erases only its pixels
        // Nothing draws outside its frame: since macOS 14 a view doesn't clip unless asked, and a folded text's frame is
        // its folded height while its layout holds all of the text (APP-809; owner 2026-11-09: the folded lines painted
        // over the rows below).
        clipsToBounds = true
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
        // The text stays laid out at the width it was measured at when that is (or will be) its own width: laying it out
        // again at a 1-point width before its first layout typeset every glyph on a line of its own (APP-815)
        if bounds.width > 1, abs(previousSize.width - bounds.width) < 0.5, abs(width - bounds.width) >= 0.5 {
            container.containerSize = previousSize
        }
        measuredWidths[width] = size
        return size
    }

    override func layout() {
        super.layout()
        layoutFold()
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
        if let shownHeight { fadeOut(below: shownHeight) }
    }

    /// The collapsed look (APP-721): the last 38 % of the shown height fades out and nothing below it draws. Erased in
    /// this view's own backing, so no SwiftUI clip or mask wraps the text (APP-788).
    private func fadeOut(below shown: CGFloat) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let start = shown * 0.62
        context.saveGState()
        context.setBlendMode(.destinationOut)
        if let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceGray(),
                                     colors: [CGColor(gray: 0, alpha: 0), CGColor(gray: 0, alpha: 1)] as CFArray,
                                     locations: [0, 1]) {
            context.saveGState()
            context.clip(to: CGRect(x: bounds.minX, y: start, width: bounds.width, height: shown - start))
            // flipped: y grows downwards, so the fade runs from `start` down to `shown`
            context.drawLinearGradient(gradient, start: CGPoint(x: 0, y: start), end: CGPoint(x: 0, y: shown), options: [])
            context.restoreGState()
        }
        context.setFillColor(CGColor(gray: 0, alpha: 1))
        context.fill(CGRect(x: bounds.minX, y: shown, width: bounds.width, height: max(0, bounds.maxY - shown)))
        context.restoreGState()
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        // The fold's control and kept lines are subviews below the shown text (APP-809)
        if let superview, fold != nil {
            let inside = convert(point, from: superview)
            for view in [foldControl as NSView, tailView].compactMap({ $0 }) where !view.isHidden && view.frame.contains(inside) {
                return view.hitTest(convert(inside, to: view.superview)) ?? view
            }
        }
        // The part a collapsed text doesn't show takes no clicks: Show more sits there.
        if let shownHeight, let superview, convert(point, from: superview).y > shownHeight { return nil }
        return super.hitTest(point)
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
        hiddenWatch.watch(newWindow) { [weak self] in self?.restLink() }
    }

    /// Hidden with an ancestor (the notch folding its body) or its window no longer visible (ordered out, occluded):
    /// no link stays lifted and the hand goes back (APP-756), as HoverSourceView does.
    override func viewDidHide() {
        super.viewDidHide()
        restLink()
    }

    private let hiddenWatch = WindowHiddenWatch()

    private func restLink() { if hoveredLink != nil { lift(nil) } }

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
