import XCTest
@testable import NotchCore
@testable import FERBNotch

final class CompletionGlowTests: XCTestCase {
    // decision: APP-564
    func testCompletionHasExactlyTwoGlowsSeparatedByDarkness() {
        var wasLit = false
        var pulses = 0
        for time in stride(from: 0.0, through: CompletionGlow.duration + 2, by: 0.01) {
            let opacity = CompletionGlow.opacity(at: time)
            XCTAssertTrue((0...1).contains(opacity))
            let lit = opacity > 0.001
            if lit && !wasLit { pulses += 1 }
            wasLit = lit
        }
        XCTAssertEqual(pulses, 2)
        XCTAssertEqual(CompletionGlow.opacity(at: CompletionGlow.duration), 0)
        XCTAssertEqual(CompletionGlow.opacity(at: 100), 0)
    }

    // decision: APP-564
    func testReducedMotionUsesOneQuietGlowThenTurnsOff() {
        XCTAssertGreaterThan(CompletionGlow.opacity(at: 0.7, reduceMotion: true), 0)
        XCTAssertGreaterThan(CompletionGlow.opacity(at: 1.7, reduceMotion: true), 0)
        XCTAssertEqual(CompletionGlow.opacity(at: CompletionGlow.duration, reduceMotion: true), 0)
    }

    @MainActor
    // decision: APP-558
    func testPinBlocksAutomaticDismissalButExplicitCloseStillWorks() {
        let model = NotchModel()
        model.expand()
        model.togglePin()
        model.dismissAutomatically()
        XCTAssertTrue(model.expanded)
        XCTAssertTrue(model.isPinned)
        model.collapse()
        XCTAssertFalse(model.expanded)
        XCTAssertFalse(model.isPinned)
        model.expand()
        model.dismissAutomatically()
        XCTAssertFalse(model.expanded)
    }
}
