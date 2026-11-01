import Foundation
import XCTest
@testable import NotchCore

final class MarkdownRenderingTests: XCTestCase {
    func testInlineFormattingAndSafeLinksWithoutExecutableContent() {
        let text = MarkdownDocument.inline("**Bold** and `code`; [Web](https://example.com), [Bad](javascript:alert), ![photo](https://example.com/image.png)")
        XCTAssertTrue(text.runs.contains { $0.inlinePresentationIntent?.contains(.stronglyEmphasized) == true })
        XCTAssertTrue(text.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true })
        XCTAssertTrue(text.runs.contains { $0.link?.absoluteString == "https://example.com" })
        XCTAssertFalse(text.runs.contains { $0.link?.scheme == "javascript" })
    }
}
