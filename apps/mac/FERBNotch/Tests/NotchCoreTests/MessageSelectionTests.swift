import AppKit
import XCTest
@testable import FERBNotch

@MainActor
final class MessageSelectionTests: XCTestCase {
    func testWholeFormattedMessageCopiesAcrossParagraphs() {
        let value = MarkdownTextAttributes.make("First paragraph.\n\nA **bold** second paragraph 🪄.\n\nLast paragraph.", size: 12)
        let view = MessageTextView()
        view.update(value, generation: 0)
        view.selectAll(nil)
        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        XCTAssertTrue(view.writeSelection(to: pasteboard, types: view.writablePasteboardTypes))
        XCTAssertEqual(pasteboard.string(forType: .string), "First paragraph.\n\nA bold second paragraph 🪄.\n\nLast paragraph.")
        XCTAssertEqual(view.selectedRange().length, value.length)
        let range = (value.string as NSString).range(of: "bold")
        let font = value.attribute(.font, at: range.location, effectiveRange: nil) as? NSFont
        XCTAssertTrue(font.map { NSFontManager.shared.traits(of: $0).contains(.boldFontMask) } ?? false)
    }

    func testTablesCodeAndSafeLinksRemainSelectableAndFitTheBubble() {
        let value = MarkdownTextAttributes.make("# Result\n\n- [Web](https://example.com)\n- [Unsafe](javascript:alert)\n\n```swift\nlet greeting = \"hello\"\n```\n\n| Item | Value |\n| --- | ---: |\n| One | 42 |", size: 12)
        let view = MessageTextView()
        view.update(value, generation: 0)
        let size = view.measuredSize(width: 340)
        XCTAssertGreaterThan(size.height, 100)
        XCTAssertLessThanOrEqual(size.width, 340)
        view.selectAll(nil)
        XCTAssertTrue(view.string.contains("let greeting"))
        XCTAssertTrue(view.string.contains("One"))
        XCTAssertTrue(view.string.contains("42"))
        var links: [URL] = []
        value.enumerateAttribute(.link, in: NSRange(location: 0, length: value.length)) { link, _, _ in
            if let url = link as? URL { links.append(url) }
        }
        XCTAssertEqual(links.map(\.absoluteString), ["https://example.com"])
    }

    func testSentMessageSelectAllIncludesEveryLine() {
        let view = MessageTextView()
        view.update(NativeMessageText("My first line\n\nMy second line").text, generation: 0)
        view.selectAll(nil)
        XCTAssertEqual(view.selectedRange(), NSRange(location: 0, length: view.string.utf16.count))
        XCTAssertFalse(view.isEditable)
    }
}
