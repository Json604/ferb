import XCTest
@testable import NotchCore

/// Memory import on the Memory page (kb area memory): the daemon's status and estimate read as one line each.
final class MemoryImportRowsTests: XCTestCase {
    // decision: MEM-403
    func testOnlyAccountExportsNeedAFile() {
        XCTAssertEqual(MemoryImportSource.allCases.filter(\.needsFile), [.chatgpt, .claude, .gemini])
    }
}
