import XCTest
@testable import NotchCore

/// Memory import on the Memory page (kb area memory): the daemon's status and estimate read as one line each.
final class MemoryImportRowsTests: XCTestCase {
    // decision: MEM-432
    func testOnlyAccountExportsNeedAFile() {
        XCTAssertEqual(MemoryImportSource.allCases.filter(\.needsFile), [.chatgpt, .claude, .gemini])
    }

    // decision: MEM-443, MEM-444
    func testAnImportedLineSaysWhereItAppliesAndItsOffer() {
        let line = ImportedLine(["item_id": "m1", "scope": "kite", "scope_by": "owner",
                                 "offer": "Offer to help with the rent", "offer_answer": "done"])
        XCTAssertEqual(line?.whereLine, "Only in the kite project")
        XCTAssertEqual(line?.scopeByOwner, true)
        XCTAssertEqual(line?.offerLine, "FERB could offer: Offer to help with the rent (you said it's done)")
        let general = ImportedLine(["item_id": "m2", "scope": "", "offer": ""])
        XCTAssertEqual(general?.whereLine, "Applies everywhere")
        XCTAssertEqual(general?.offerLine, "")
        XCTAssertNil(ImportedLine(["scope": "x"]))
    }
}
