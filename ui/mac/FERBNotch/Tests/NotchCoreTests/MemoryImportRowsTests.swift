import XCTest
@testable import NotchCore

/// Memory import on the Memory page (notes/memory-import.md): the daemon's status and estimate read as one line each.
final class MemoryImportRowsTests: XCTestCase {
    func testARunningImportReadsItsProgress() throws {
        let status = try XCTUnwrap(MemoryImportStatus(["run": "r1", "name": "ChatGPT", "status": "running",
                                                       "counts": ["chunks_done": 2, "chunks_left": 3, "facts_saved": 7]]))
        XCTAssertTrue(status.running)
        XCTAssertEqual(status.chunks, 5)
        XCTAssertTrue(status.line.contains("2 of 5"))
    }

    func testNoRunIsNoStatus() {
        XCTAssertNil(MemoryImportStatus(nil))
        XCTAssertNil(MemoryImportStatus(["status": "done"]))
    }

    func testOnlyAccountExportsNeedAFile() {
        XCTAssertEqual(MemoryImportSource.allCases.filter(\.needsFile), [.chatgpt, .claude, .gemini])
    }
}
