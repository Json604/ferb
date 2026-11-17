import SwiftUI
import XCTest
@testable import FerbDesign

/// A message's words are built once and found again (kb PHN-716; Android's ScrollWorkTest): a body run again, or the
/// row scrolling back into the list, builds nothing; changed words (a streamed one) build anew.
final class RichTextCacheTests: XCTestCase {
    // decision: PHN-716
    func testAMessageSeenAgainBuildsNothingAndChangedWordsBuildAnew() {
        let look = RichLook(size: 16, ink: .white, dim: .gray, link: .blue, codeFill: .black)
        let blocks: [RichBlock] = [.paragraph([RichRun("FERB looked through the folder.")])]
        let before = RichTextCache.builds
        let first = RichTextCache.text("cache-test", blocks, look)
        for _ in 0..<5 { XCTAssertEqual(RichTextCache.text("cache-test", blocks, look), first) }
        XCTAssertEqual(RichTextCache.builds, before + 1, "seen again: built again")
        _ = RichTextCache.text("cache-test", [.paragraph([RichRun("FERB looked through the folder. And")])], look)
        XCTAssertEqual(RichTextCache.builds, before + 2, "a streamed word: built anew")
    }
}
