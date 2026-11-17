import Foundation
import SwiftUI
import XCTest
@testable import FerbDesign

/// The motion tokens are one table for both phones (apps/android/design/src/test/resources/motion/motion.json; Android's
/// MotionTableTest reads the same file): every number FerbMotion moves with is the table's, the table's springs are
/// PairIt's curves, and under Reduce Motion nothing moves. At rest the hosts draw the goldens' very pixels.
final class MotionTests: XCTestCase {
    private func table() throws -> [String: Any] {
        let url = LabNumbersTests.repo.appendingPathComponent("apps/android/design/src/test/resources/motion/motion.json")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
    }

    private func obj(_ t: [String: Any], _ path: String...) throws -> [String: Any] {
        var m = t
        for k in path { m = try XCTUnwrap(m[k] as? [String: Any], k) }
        return m
    }

    private func num(_ m: [String: Any], _ k: String) throws -> Double { try XCTUnwrap((m[k] as? NSNumber)?.doubleValue, k) }

    // decision: PHN-620, PHN-621
    func testEveryMotionNumberIsTheSharedTables() throws {
        let t = try table()
        for (name, s) in [("tab", FerbMotion.tab), ("press_down", FerbMotion.pressDown), ("press_up", FerbMotion.pressUp), ("sheet", FerbMotion.sheet)] {
            let row = try obj(t, "springs", name)
            XCTAssertEqual(try num(row, "response"), s.response, accuracy: 1e-6, name)
            XCTAssertEqual(try num(row, "damping"), s.damping, accuracy: 1e-9, name)
        }
        for (name, c) in [("enter", FerbMotion.enter), ("exit", FerbMotion.exit), ("item", FerbMotion.item), ("reduced", FerbMotion.reduced)] {
            let row = try obj(t, "curves", name)
            XCTAssertEqual(Int(try num(row, "ms")), c.ms, name)
            let b = try XCTUnwrap(row["bezier"] as? [NSNumber]).map(\.doubleValue)
            XCTAssertEqual(b, [c.bezier.0, c.bezier.1, c.bezier.2, c.bezier.3], name)
        }
        XCTAssertEqual(try num(obj(t, "curves", "enter"), "shift"), Double(FerbMotion.enterShift))
        XCTAssertEqual(try num(obj(t, "curves", "item"), "shift"), Double(FerbMotion.itemShift))
        XCTAssertEqual(try num(obj(t, "press"), "scale"), Double(FerbMotion.pressScale), accuracy: 1e-9)
        XCTAssertEqual(try num(obj(t, "press"), "alpha"), FerbMotion.pressAlpha, accuracy: 1e-9)
        XCTAssertEqual(try num(obj(t, "scrim"), "alpha"), FerbMotion.scrim, accuracy: 1e-9)
        XCTAssertEqual(try num(obj(t, "dismiss"), "fraction"), Double(FerbMotion.dismissFraction), accuracy: 1e-9)
        XCTAssertEqual(try num(obj(t, "dismiss"), "velocity"), Double(FerbMotion.dismissVelocity), accuracy: 1e-9)
    }

    /// PairIt's 1 - e^(-7.5t)(cos 10t + 0.75 sin 10t) over 650, 220 and 550 ms is exactly the step of a spring with
    /// damping 0.6 and response duration x 2 pi / 12.5, which is what SwiftUI's spring(response:dampingFraction:) plays.
    // decision: PHN-620, PHN-621
    func testTheSpringsArePairItsCurves() {
        for (s, ms) in [(FerbMotion.tab, 650.0), (FerbMotion.pressDown, 220.0), (FerbMotion.pressUp, 550.0)] {
            let omega = 2 * Double.pi / s.response, zeta = s.damping, wd = omega * (1 - zeta * zeta).squareRoot()
            for i in 0...100 {
                let u = Double(i) / 100, t = u * ms / 1000
                let pairIt = 1 - exp(-7.5 * u) * (cos(10 * u) + 0.75 * sin(10 * u))
                let step = 1 - exp(-zeta * omega * t) * (cos(wd * t) + zeta * omega / wd * sin(wd * t))
                XCTAssertEqual(pairIt, step, accuracy: 2e-5, "\(ms) ms at \(u)")
            }
        }
    }

    /// Under Reduce Motion (and for what is going) nothing moves: a change is only a fade.
    // decision: PHN-624
    func testUnderReduceMotionNothingMoves() {
        for v in stride(from: 0.0, through: 1.0, by: 0.1) {
            XCTAssertEqual(FerbMotion.enterOffset(v, leaving: false, reduce: true), 0)
            XCTAssertEqual(FerbMotion.enterOffset(v, leaving: true, reduce: false), 0)
        }
        XCTAssertEqual(FerbMotion.enterOffset(0, leaving: false, reduce: false), FerbMotion.enterShift)
        XCTAssertEqual(FerbMotion.enterOffset(1, leaving: false, reduce: false), 0)
        XCTAssertEqual(FerbMotion.reduced.ms, 120)
    }
}

#if canImport(UIKit)
/// The hosts at rest draw exactly what the lab's screens draw: the tab bar once over a page whose own bar keeps its
/// room, and a sheet shown by the sheet switch.
@MainActor
final class MotionRestTests: XCTestCase {
    // decision: PHN-622
    func testTheTabHostAtRestIsTheTasksGolden() {
        let view = PhoneScreen(moment: .lab(12.5), glide: false) {
            TabBarHost(chosen: .tasks, waiting: .tasks, onTab: nil, onProfile: nil) { TasksPage() }
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "screens/tasks-noon.png")
    }

    // decision: PHN-623
    func testASheetShownByTheSwitchAtRestIsTheQuestionGolden() {
        let view = PhoneScreen(moment: .lab(12.5), glide: false) {
            MotionSwitch("question", kind: .sheet) { _ in QuestionSheet(insets: .lab) }
        } page: {
            ChatPage(insets: .lab)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "screens/question-noon.png")
    }
}
#endif
