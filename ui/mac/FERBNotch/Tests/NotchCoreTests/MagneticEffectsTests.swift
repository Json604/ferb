import XCTest
@testable import FERBNotch

/// The magnetic docking visuals (its sound was removed, owner 2026-11-01).
final class MagneticEffectsTests: XCTestCase {
    func testDirectionalStrainPreservesOppositeEdgeAndReducedMotion() {
        var sample = MagneticDocking.Sample()
        sample.pill = CGRect(x: 100, y: 100, width: 58, height: 30)
        sample.direction = CGPoint(x: 0, y: 1)
        sample.strain = 0.018
        let strained = MagneticFieldGeometry.strainedPill(sample)
        XCTAssertEqual(strained.minY, sample.pill.minY)
        XCTAssertGreaterThan(strained.maxY, sample.pill.maxY)
        sample.reduceMotion = true
        XCTAssertEqual(MagneticFieldGeometry.strainedPill(sample), sample.pill)
    }

    @MainActor func testNativeFieldRestrictsResponseAndReducedMotionHasNoOscillation() {
        let view = MagneticFieldView(frame: CGRect(x: 0, y: 0, width: 400, height: 400))
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging
        sample.pill = CGRect(x: 171, y: 200, width: 58, height: 30)
        sample.target = CGRect(x: 140, y: 300, width: 120, height: 30)
        sample.proximity = 0.8
        sample.direction = CGPoint(x: 0, y: 1)
        sample.reduceMotion = true
        view.update(sample, canvas: view.bounds)
        let layers = view.layer!.sublayers!.compactMap { $0 as? CAShapeLayer }
        XCTAssertEqual(layers.count, 6)
        let paths = layers.map { $0.path! }
        XCTAssertTrue(paths.prefix(3).allSatisfy { $0.boundingBox.minY > sample.pill.midY })
        XCTAssertTrue(paths.suffix(3).allSatisfy { $0.boundingBox.maxY < sample.target!.minY })
        sample.time = 7
        view.update(sample, canvas: view.bounds)
        for (before, after) in zip(paths, layers) { XCTAssertEqual(before, after.path) }
        sample.fusionProgress = 1
        view.update(sample, canvas: view.bounds)
        XCTAssertTrue(layers.allSatisfy { $0.opacity == 0 })
    }
}
