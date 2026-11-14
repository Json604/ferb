import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Past-bug guard (TEST-503): FERB.app hung for minutes (2026-11-14, its sample) because a chat text's update changed
/// the text view's storage inside SwiftUI's update pass; the text view then moved its selection and posted
/// Accessibility's selected-text notification, and with an outside Accessibility client observing FERB, AppKit asked
/// the hosting view for its children, which re-entered the SwiftUI graph mid-update (an attribute-graph cycle). The text
/// from SwiftUI goes into the storage on a main-queue turn of its own and is measured as it will show meanwhile.
/// The windows here are never ordered front.
@MainActor
final class MessageTextStagingTests: XCTestCase {
    private func turn() async {
        await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
    }

    /// A text view shown in a window (never ordered front), as the chat's are, with `text` in it.
    private var windows: [NSWindow] = []
    private func shown(_ text: String) -> MessageTextView {
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 400, height: 300),
                              styleMask: .borderless, backing: .buffered, defer: true)
        let view = MessageTextView()
        view.update(NativeMessageText(text).text, generation: 0)
        view.frame = CGRect(x: 0, y: 0, width: 400, height: 300)
        window.contentView?.addSubview(view)
        windows.append(window)
        return view
    }

    // decision: APP-788
    func testSwiftUIsUpdatePassLeavesTheTextStorageAlone() async throws {
        let source = StagingSource()
        let host = NSHostingView(rootView: StagingHost(source: source))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 360, height: 300),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                   // never ordered front or captured
        host.layoutSubtreeIfNeeded()
        await turn()
        host.layoutSubtreeIfNeeded()
        let view = try XCTUnwrap(textViews(in: host).first)
        XCTAssertEqual(view.string, "Working on it")

        let edits = EditCounter()
        view.textStorage?.delegate = edits
        source.text = "Working on it · 12 s\n\nA second paragraph that wraps across the bubble's width, more than once."
        host.layoutSubtreeIfNeeded()                               // SwiftUI's update and layout pass, as the display link runs it
        XCTAssertEqual(edits.count, 0, "nothing changed the text storage inside the pass")
        XCTAssertEqual(view.string, "Working on it")
        XCTAssertEqual(view.pendingText?.string, source.text)
        XCTAssertEqual(view.frame.height, view.measuredSize(width: view.frame.width).height, accuracy: 0.5,
                       "the row is already the new text's height")

        await turn()
        XCTAssertEqual(view.string, source.text, "the text goes in on the next main-queue turn")
        XCTAssertNil(view.pendingText)
        XCTAssertEqual(edits.count, 1)
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(view.frame.height, view.measuredSize(width: view.frame.width).height, accuracy: 0.5)
        withExtendedLifetime(window) {}
    }

    // decision: APP-788
    func testOnlyTheLatestStagedTextGoesIn() async {
        let view = shown("one")
        let edits = EditCounter()
        view.textStorage?.delegate = edits
        view.stage(NativeMessageText("two").text, generation: 0)
        view.stage(NativeMessageText("three").text, generation: 0)
        XCTAssertEqual(view.string, "one")
        await turn()
        XCTAssertEqual(view.string, "three", "the newer text replaces the staged one before it goes in")
        XCTAssertEqual(edits.count, 1, "a text replaced before its turn never goes in")

        view.stage(NativeMessageText("four").text, generation: 0)
        view.stage(NativeMessageText("three").text, generation: 0)   // back to what shows: nothing waits
        XCTAssertNil(view.pendingText)
        await turn()
        XCTAssertEqual(view.string, "three")
        XCTAssertEqual(edits.count, 1)

        view.stage(NativeMessageText("five").text, generation: 0)
        view.update(NativeMessageText("six").text, generation: 0)     // put in at once: newer than the staged text
        await turn()
        XCTAssertEqual(view.string, "six")
    }

    // decision: APP-788
    func testTheSelectionStaysWithinTheTextAsBefore() async {
        let view = shown("A longer first text to select")
        view.setSelectedRange(NSRange(location: 2, length: 20))
        view.stage(NativeMessageText("Short").text, generation: 0)
        XCTAssertEqual(view.selectedRange(), NSRange(location: 2, length: 20), "untouched inside the pass")
        await turn()
        XCTAssertEqual(view.selectedRange(), NSRange(location: 2, length: 3))
    }

    // decision: APP-815
    func testAViewInNoWindowYetTakesTheTextAtOnce() {
        let view = MessageTextView()                               // SwiftUI fills a new view before placing it
        view.stage(NativeMessageText("Shown in the first frame").text, generation: 0)
        XCTAssertEqual(view.string, "Shown in the first frame")
        XCTAssertNil(view.pendingText)
    }

    // decision: APP-809, APP-788
    func testAStagedTextMeasuresAsItWillShow() async {
        let long = (1...30).map { "- 7.\($0) a long bullet about the visa rules and what to bring along" }.joined(separator: "\n")
        let texts: [NSAttributedString] = [
            NativeMessageText("Short").text,
            NativeMessageText("Two lines\nand a newline at the end\n").text,
            NativeMessageText("A sentence long enough to wrap at the narrower widths measured below, twice over.",
                              size: 13.5, bold: true).text,
            NativeMessageText("let x = 1\nlet y = 2", size: 11.5, monospaced: true).text,
            NativeMessageText("Some **bold**, *italic* and `code` with [a link](https://example.com).", markdown: true).text,
            MarkdownTextAttributes.make("# Result\n\n- one\n- two\n\n| Item | Value |\n| --- | ---: |\n| One | 42 |", size: 12),
            NativeMessageText(long).text,
        ]
        for text in texts {
            for width: CGFloat? in [80, 240, 413, nil] {
                let staged = shown("what showed before")
                staged.stage(text, generation: 0)
                let early = staged.measuredSize(width: width)
                await turn()
                XCTAssertNil(staged.pendingText)
                let settled = staged.measuredSize(width: width)
                let direct = MessageTextView()
                direct.update(text, generation: 0)
                XCTAssertEqual(early, settled, "\(text.string.prefix(20)) at \(String(describing: width))")
                XCTAssertEqual(early, direct.measuredSize(width: width))
            }
        }
    }

    // decision: APP-809
    func testAFoldDecidedFromAStagedTextIsTheSame() async {
        let long = NativeMessageText((1...40).map { "Line \($0) of a long answer." }.joined(separator: "\n")).text
        let fold = MessageFold(kind: .answer(viewport: 300), open: false, more: "Show all", tail: nil,
                               lineHeight: 15, toggle: {})
        let staged = shown("Working")
        staged.fold = fold
        staged.stage(long, generation: 0)
        let early = staged.foldedSize(width: 320)
        await turn()
        let direct = MessageTextView()
        direct.fold = fold
        direct.update(long, generation: 0)
        XCTAssertEqual(early, staged.foldedSize(width: 320))
        XCTAssertEqual(early, direct.foldedSize(width: 320))
        XCTAssertLessThan(early.height, direct.measuredSize(width: 320).height, "the long text folds")
    }

    private func textViews(in view: NSView) -> [MessageTextView] {
        if let field = view as? MessageTextView { return [field] }
        return view.subviews.flatMap { textViews(in: $0) }
    }
}

@MainActor
private final class StagingSource: ObservableObject {
    @Published var text = "Working on it"
}

private struct StagingHost: View {
    @ObservedObject var source: StagingSource
    var body: some View {
        NativeMessageText(source.text).frame(width: 300, alignment: .topLeading).fixedSize(horizontal: false, vertical: true)
    }
}

private final class EditCounter: NSObject, NSTextStorageDelegate {
    var count = 0
    func textStorage(_ textStorage: NSTextStorage, didProcessEditing editedMask: NSTextStorageEditActions,
                     range editedRange: NSRange, changeInLength delta: Int) {
        if editedMask.contains(.editedCharacters) { count += 1 }
    }
}
