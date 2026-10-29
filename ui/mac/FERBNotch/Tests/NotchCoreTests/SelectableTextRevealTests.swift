import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class SelectableTextRevealTests: XCTestCase {
    func testActualMarkdownRetainsNativeStorageAndSelectionAcrossRevealAndGeometry() async throws {
        let state = RevealFixtureState()
        let host = NSHostingView(rootView: RevealFixture(state: state))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 350),
                              styleMask: .borderless, backing: .buffered, defer: false)
        window.contentView = host // Never ordered or captured.
        await settle(host)
        let original = fields(in: host)
        XCTAssertGreaterThanOrEqual(original.count, 3)
        let originalStrings = original.map { $0.attributedString().copy() as! NSAttributedString }
        original.forEach { $0.setSelectedRange(NSRange(location: 0, length: min(3, $0.string.count))) }
        XCTAssertTrue(original.contains { $0.string.contains("temporary") })
        XCTAssertTrue(original.contains { field in
            var linked = false
            field.attributedString().enumerateAttribute(.link, in: NSRange(location: 0, length: field.attributedString().length)) { value, _, _ in
                if value != nil { linked = true }
            }
            return linked
        })
        let refreshCounts = original.compactMap { ($0 as? MessageTextView)?.displayRefreshCount }
        state.width = 58
        state.expansion = 0
        window.setContentSize(CGSize(width: 58, height: 30))
        await settle(host)
        state.width = 400
        state.expansion = 1
        window.setContentSize(CGSize(width: 440, height: 350))
        state.generation += 1
        await settle(host)
        let rebuilt = fields(in: host)
        XCTAssertEqual(rebuilt.count, original.count)
        for (index, field) in rebuilt.enumerated() {
            XCTAssertTrue(original.contains { $0 === field })
            XCTAssertEqual(field.attributedString(), originalStrings[index])
            XCTAssertEqual(field.selectedRange().length, 3)
            let manager = try XCTUnwrap(field.layoutManager)
            let container = try XCTUnwrap(field.textContainer)
            manager.ensureLayout(for: container)
            XCTAssertEqual(manager.numberOfGlyphs, field.string.utf16.count)
            XCTAssertGreaterThan(manager.usedRect(for: container).height, 0)
            XCTAssertGreaterThan(manager.usedRect(for: container).width, 0)
            XCTAssertTrue(field.isSelectable)
            XCTAssertGreaterThan((field as! MessageTextView).displayRefreshCount, refreshCounts[index])
        }
        let movedRefreshCounts = rebuilt.map { ($0 as! MessageTextView).displayRefreshCount }
        for index in 0..<120 { state.offset = CGFloat(index % 20); await settle(host) }
        let moved = fields(in: host)
        XCTAssertEqual(moved.map(ObjectIdentifier.init), rebuilt.map(ObjectIdentifier.init))
        XCTAssertEqual(moved.map { ($0 as! MessageTextView).displayRefreshCount }, movedRefreshCounts)
        withExtendedLifetime(window) {}
    }

    func testProductionTranscriptUsesOwnedRendererForPlanOwnerAndAnswer() async {
        var turn = ConversationTurn(id: UUID(), createdAt: Date(), request: "**Literal owner text**", sources: [])
        turn.plan = "I’ll condense the report into a short update covering all 15 people."
        turn.answer = "**Short version**, as of 29 Oct 2026."
        let model = NotchModel()
        let host = NSHostingView(rootView: ConversationTranscript(entries: [.turn(turn, answeredBelow: false)], model: model))
        host.frame = CGRect(x: 0, y: 0, width: 440, height: 500)
        await settle(host)
        let rendered = fields(in: host)
        XCTAssertTrue(rendered.contains { $0.string == turn.request })
        XCTAssertTrue(rendered.contains { $0.string == turn.plan })
        XCTAssertTrue(rendered.contains { $0.string == "Short version, as of 29 Oct 2026." })
        XCTAssertTrue(rendered.allSatisfy { $0 is MessageTextView })
    }

    func testNativeLayoutAndDisplayInvalidationKeepSelectionWithoutPerDragWork() throws {
        let view = MessageTextView()
        let value = MessageTextAttributes.make("This looks like a temporary portal/session loading issue that may clear with a refresh.",
                                               size: 12, secondary: false, markdown: false, bold: false,
                                               monospaced: false, alignment: .left)
        view.update(value, generation: 0)
        view.frame = CGRect(x: 0, y: 0, width: 300, height: 100)
        view.layout()
        let wide = view.measuredSize(width: 300)
        let narrow = view.measuredSize(width: 120)
        XCTAssertGreaterThan(narrow.height, wide.height)
        XCTAssertEqual(view.textContainer?.containerSize.width, view.bounds.width)
        let storage = view.textStorage
        view.setSelectedRange(NSRange(location: 5, length: 8))
        let before = view.displayRefreshCount
        for index in 0..<120 {
            view.setFrameOrigin(CGPoint(x: index, y: index))
            view.update(value, generation: 0)
            view.layout()
        }
        XCTAssertEqual(view.displayRefreshCount, before)
        view.update(value, generation: 1)
        XCTAssertEqual(view.displayRefreshCount, before + 1)
        XCTAssertTrue(view.textStorage === storage)
        XCTAssertEqual(view.selectedRange(), NSRange(location: 5, length: 8))
        let manager = try XCTUnwrap(view.layoutManager)
        XCTAssertEqual(manager.numberOfGlyphs, value.length)
        XCTAssertGreaterThan(manager.boundingRect(forGlyphRange: NSRange(location: 0, length: manager.numberOfGlyphs),
                                                 in: try XCTUnwrap(view.textContainer)).height, 0)
    }

    func testEmojiDescendersAndTrailingCodeLineFitMeasuredBounds() throws {
        let view = MessageTextView()
        func configure(_ text: String) {
            view.update(MessageTextAttributes.make(text, size: 12, secondary: false, markdown: false,
                                                   bold: false, monospaced: true, alignment: .left), generation: 0)
        }
        configure("gjpq 👋")
        let oneLine = view.measuredSize(width: 200)
        configure("gjpq 👋\n")
        let trailingLine = view.measuredSize(width: 200)
        XCTAssertGreaterThan(trailingLine.height, oneLine.height)
        view.frame = CGRect(origin: .zero, size: trailingLine)
        view.layout()
        let manager = try XCTUnwrap(view.layoutManager)
        let glyphs = manager.boundingRect(forGlyphRange: NSRange(location: 0, length: manager.numberOfGlyphs),
                                           in: try XCTUnwrap(view.textContainer))
        XCTAssertLessThanOrEqual(glyphs.maxY, trailingLine.height)
        XCTAssertEqual(view.string, "gjpq 👋\n")
    }

    func testNativeMarkdownAttributesPreserveFormatsAndDiscardUnsafeLinks() throws {
        let value = MessageTextAttributes.make("**Bold** *italic* ~~gone~~ `code` [Web](https://example.com) [Bad](javascript:alert)",
                                               size: 12, secondary: false, markdown: true, bold: false,
                                               monospaced: false, alignment: .left)
        func attributes(_ word: String) -> [NSAttributedString.Key: Any] {
            value.attributes(at: (value.string as NSString).range(of: word).location, effectiveRange: nil)
        }
        let bold = try XCTUnwrap(attributes("Bold")[.font] as? NSFont)
        XCTAssertTrue(NSFontManager.shared.traits(of: bold).contains(.boldFontMask))
        let italic = try XCTUnwrap(attributes("italic")[.font] as? NSFont)
        XCTAssertTrue(NSFontManager.shared.traits(of: italic).contains(.italicFontMask))
        XCTAssertNotNil(attributes("gone")[.strikethroughStyle])
        XCTAssertNotNil(attributes("code")[.backgroundColor])
        XCTAssertEqual((attributes("code")[.font] as? NSFont)?.pointSize, 11)
        XCTAssertEqual(attributes("Web")[.link] as? URL, URL(string: "https://example.com"))
        XCTAssertNil(attributes("Bad")[.link])
        let view = MessageTextView()
        XCTAssertTrue(view.textView(view, clickedOnLink: URL(string: "javascript:alert")!, at: 0))
    }

    func testRevealRequestSurvivesReplacementButOnlyCompletesOnce() {
        let model = NotchModel()
        model.noteTextPresentation(expanded: true)
        model.noteTextPresentation(expanded: true) // Replaced animation: size/status changed.
        XCTAssertEqual(model.textRenderGeneration, 0)
        model.completeTextReveal()
        XCTAssertEqual(model.textRenderGeneration, 1)
        for _ in 0..<120 { model.noteTextPresentation(expanded: true); model.completeTextReveal() }
        XCTAssertEqual(model.textRenderGeneration, 1)
        model.noteTextPresentation(expanded: false)
        model.completeTextReveal() // A cancelled opening must not publish while closed.
        XCTAssertEqual(model.textRenderGeneration, 1)
        model.noteTextPresentation(expanded: true)
        model.completeTextReveal()
        XCTAssertEqual(model.textRenderGeneration, 2)
    }

    private func fields(in view: NSView) -> [NSTextView] {
        if let field = view as? NSTextView { return [field] }
        return view.subviews.flatMap { fields(in: $0) }
    }
    private func settle(_ host: NSView) async {
        host.layoutSubtreeIfNeeded()
        await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        host.layoutSubtreeIfNeeded()
    }
}

private final class RevealFixtureState: ObservableObject {
    @Published var generation = 0
    @Published var offset: CGFloat = 0
    @Published var width: CGFloat = 400
    @Published var expansion: CGFloat = 1
}
private struct RevealFixture: View {
    @ObservedObject var state: RevealFixtureState
    var body: some View {
        ScrollView {
            MarkdownContent("**The earlier check was incomplete.**\n\nThis looks like a temporary portal/session loading issue that may clear with a refresh.\n\n[Reference](https://example.com)")
                .equatable()
                .font(.system(size: 12))
                .padding(12)
        }
        .frame(width: state.width, height: 300)
        .opacity(state.expansion)
        .scaleEffect(0.975 + 0.025 * state.expansion, anchor: .top)
        .blur(radius: 2 * (1 - state.expansion))
        .mask(Rectangle())
        .offset(x: state.offset)
        .environment(\.selectableTextGeneration, state.generation)
    }
}
