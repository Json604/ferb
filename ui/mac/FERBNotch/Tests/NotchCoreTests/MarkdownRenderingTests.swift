import Foundation
import XCTest
@testable import NotchCore

final class MarkdownRenderingTests: XCTestCase {
    func testConcertAnswerHasRealTableAndSurroundingParagraphs() {
        let source = """
        **Verified.** Tickets to watch:

        | Priority | Event | Link |
        |---|---|---|
        | 1 | Diljit Dosanjh | https://example.com/tickets |
        | 2 | Lollapalooza | [Book](https://example.com/book) |

        Check dates before booking.
        """
        let blocks = MarkdownDocument(source).blocks
        XCTAssertEqual(blocks.count, 3)
        guard case .table(let headers, let rows, _) = blocks[1] else { return XCTFail("Expected native table") }
        XCTAssertEqual(headers, ["Priority", "Event", "Link"])
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0][1], "Diljit Dosanjh")
    }

    func testCodeFencesListsAndEscapedTablePipesRemainLiteral() {
        let source = """
        ## Steps
        1. Open **Settings**
        - Choose `a|b`

        ```swift
        let value = "**literal** | text"
        ```

        | Name | Value |
        | :--- | ---: |
        | A\\|B | `x|y` |
        """
        let blocks = MarkdownDocument(source).blocks
        XCTAssertTrue(blocks.contains { if case .code(let code, _) = $0 { return code.contains("**literal** | text") }; return false })
        guard case .table(_, let rows, let alignments) = blocks.last else { return XCTFail("Expected table") }
        XCTAssertEqual(rows[0], ["A\\|B", "`x|y`"])
        XCTAssertEqual(alignments, [.leading, .trailing])
    }

    func testInlineFormattingAndSafeLinksWithoutExecutableContent() {
        let text = MarkdownDocument.inline("**Bold** and `code`; [Web](https://example.com), [Bad](javascript:alert), ![photo](https://example.com/image.png)")
        XCTAssertTrue(text.runs.contains { $0.inlinePresentationIntent?.contains(.stronglyEmphasized) == true })
        XCTAssertTrue(text.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true })
        XCTAssertTrue(text.runs.contains { $0.link?.absoluteString == "https://example.com" })
        XCTAssertFalse(text.runs.contains { $0.link?.scheme == "javascript" })
    }
}
