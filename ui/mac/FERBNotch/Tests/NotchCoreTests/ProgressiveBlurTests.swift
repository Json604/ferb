import AppKit
import XCTest
@testable import FERBNotch

/// FERB's glass blur (APP-785): heavy at the outer edge, still frosted toward the content, in place of macOS's
/// gaussian blur, and macOS's own frost wherever the private filter isn't there.
@MainActor
final class ProgressiveBlurTests: XCTestCase {
    // decision: APP-785
    func testTheBlurIsFullAtTheEdgeAndEasesToAFrostedInside() throws {
        XCTAssertEqual(ProgressiveBlur.strength(at: 0), 1)
        XCTAssertEqual(ProgressiveBlur.strength(at: 1), ProgressiveBlur.innerShare, accuracy: 1e-9)
        XCTAssertGreaterThan(ProgressiveBlur.innerShare, 0)                     // never clear (APP-551)
        let samples = stride(from: 0.0, through: 1.0, by: 0.05).map { ProgressiveBlur.strength(at: $0) }
        XCTAssertEqual(samples, samples.sorted(by: >))                          // only ever lighter going in
        XCTAssertGreaterThan(ProgressiveBlur.edgeRadius, 30)                    // heavier than macOS's frost
        let mask = try XCTUnwrap(ProgressiveBlur.mask())
        XCTAssertEqual(mask.height, 128)
    }

    // decision: APP-785
    func testFERBsGlassTakesTheVariableBlurInPlaceOfMacOSsGaussianOne() throws {
        let window = NSWindow(contentRect: CGRect(x: -4000, y: -4000, width: 200, height: 120), styleMask: [.borderless],
                              backing: .buffered, defer: false)
        let glass = FerbGlassView(frame: window.contentView!.bounds)
        glass.material = .hudWindow
        glass.blendingMode = .behindWindow
        glass.state = .active
        window.contentView!.addSubview(glass)
        window.orderFrontRegardless()
        defer { window.orderOut(nil) }
        glass.layoutSubtreeIfNeeded()
        CATransaction.flush()
        let backdrop = try XCTUnwrap(ProgressiveBlur.backdropLayer(in: glass.layer), "macOS draws no backdrop layer")
        glass.refresh()
        let names = (backdrop.filters ?? []).map { String(describing: $0) }
        if ProgressiveBlur.filter(radius: 1, mask: nil) == nil {                 // a macOS without it keeps its frost
            XCTAssertTrue(names.contains("gaussianBlur"))
            return
        }
        XCTAssertFalse(names.contains("gaussianBlur"))
        XCTAssertTrue(names.contains("variableBlur"))
        XCTAssertTrue(names.contains("colorSaturate"))                          // macOS's other filters stay
        glass.refresh()                                                          // once only
        XCTAssertEqual((backdrop.filters ?? []).map { String(describing: $0) }.filter { $0 == "variableBlur" }.count, 1)
    }
}
