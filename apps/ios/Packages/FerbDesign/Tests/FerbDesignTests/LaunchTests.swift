#if canImport(UIKit)
import SwiftUI
import UIKit
import XCTest
@testable import FerbDesign

/// The launch (kb PHN-630..634), the twin of Android's LaunchTest and LaunchShotsTest: the shared table's numbers and
/// samples (apps/android/design/src/test/resources/motion/motion.json `launch`), its state on a fake clock (ready early
/// cuts cleanly, slow loading loops, reduced motion is still), its first frame (the launch screen's image) and its
/// last frame, which rests on the lock page's own golden.
@MainActor
final class LaunchTests: XCTestCase {
    private func launch() throws -> [String: Any] {
        let url = LabNumbersTests.repo.appendingPathComponent("apps/android/design/src/test/resources/motion/motion.json")
        let t = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        return try XCTUnwrap(t["launch"] as? [String: Any])
    }

    private func num(_ m: Any?, _ k: String) throws -> Double { try XCTUnwrap(((m as? [String: Any])?[k] as? NSNumber)?.doubleValue, k) }

    // decision: PHN-631
    func testTheNumbersAreTheSharedTables() throws {
        let l = try launch()
        XCTAssertEqual(try num(l, "size"), Double(FerbLaunch.size))
        XCTAssertEqual(try num(l["breath"], "after_ms"), FerbLaunch.breathAfterMs)
        XCTAssertEqual(try num(l["breath"], "period_ms"), FerbLaunch.breathPeriodMs)
        XCTAssertEqual(try num(l["breath"], "scale"), FerbLaunch.breathScale)
        XCTAssertEqual(try num(l["breath"], "alpha"), FerbLaunch.breathAlpha)
        XCTAssertEqual(Int(try num(l["settle"], "ms")), FerbLaunch.settle.ms)
        XCTAssertEqual(try num(l["settle"], "prompt_ms"), FerbLaunch.settlePromptMs)
        let b = try XCTUnwrap((l["settle"] as? [String: Any])?["bezier"] as? [NSNumber]).map(\.doubleValue)
        XCTAssertEqual(b, [FerbLaunch.settle.bezier.0, FerbLaunch.settle.bezier.1, FerbLaunch.settle.bezier.2, FerbLaunch.settle.bezier.3])
        XCTAssertEqual(try num(l["reduced"], "ms"), FerbLaunch.reducedMs)
        XCTAssertEqual(try num(l, "ios_launch_screen_ms"), FerbLaunch.launchScreenMs)
        XCTAssertEqual(l["background"] as? String, "ink0")
    }

    /// The same values at the same moments as Android (and as tools/launch_samples.py).
    // decision: PHN-631
    func testEverySampleIsTheSameOnThisPhone() throws {
        let samples = try XCTUnwrap(try launch()["samples"] as? [[String: Any]])
        XCTAssertGreaterThanOrEqual(samples.count, 20)
        for s in samples {
            let t = try num(s, "t")
            let ready = (s["ready"] as? NSNumber)?.doubleValue
            let reduced = try XCTUnwrap(s["reduced"] as? Bool)
            let f = FerbLaunch.at(t, readyAt: ready, reduced: reduced)
            let at = "t=\(t) ready=\(String(describing: ready)) reduced=\(reduced)"
            XCTAssertEqual(f.dotY, try num(s, "dotY"), accuracy: 2e-4, at)
            XCTAssertEqual(f.dotAlpha, try num(s, "dotAlpha"), accuracy: 2e-4, at)
            XCTAssertEqual(f.dotScale, try num(s, "dotScale"), accuracy: 2e-4, at)
            XCTAssertEqual(f.fScale, try num(s, "fScale"), accuracy: 2e-4, at)
            XCTAssertEqual(f.handover, try num(s, "handover"), accuracy: 2e-3, at)
            XCTAssertEqual(f.promptOk, s["promptOk"] as? Bool, at)
        }
    }

    /// Ready at the first frame: the settle starts once iOS's crossfade from the launch screen is over (250 ms; the
    /// whole mark stands still meanwhile); the prompt may come 315 ms later and the launch ends 450 ms after its start.
    // decision: PHN-631, PHN-633
    func testReadyEarlyCutsCleanly() async throws {
        let state = LaunchState(reduced: false)
        let start = Date(timeIntervalSinceReferenceDate: 1000)
        _ = state.elapsed(start)
        var slept: [Double] = []
        var prompted = false
        state.onPromptOk = { prompted = true }
        state.ready(start) { slept.append($0) }
        XCTAssertEqual(state.readyAt, FerbLaunch.launchScreenMs)
        var last = -1.0
        for ms in stride(from: 0.0, through: FerbLaunch.launchScreenMs + Double(FerbLaunch.settle.ms) + 20, by: 4) {
            let f = state.frame(start.addingTimeInterval(ms / 1000))
            XCTAssertGreaterThanOrEqual(f.handover, last)
            XCTAssertGreaterThanOrEqual(f.dotScale, 0.999)
            last = f.handover
        }
        for _ in 0..<20 where !state.done { await Task.yield() }
        XCTAssertEqual(slept, [FerbLaunch.launchScreenMs + FerbLaunch.settlePromptMs, Double(FerbLaunch.settle.ms) - FerbLaunch.settlePromptMs])
        XCTAssertTrue(prompted)
        XCTAssertTrue(state.done)
        XCTAssertEqual(FerbLaunch.at(Double(FerbLaunch.settle.ms), readyAt: 0, reduced: false).phase, .done)
    }

    /// The first frame is the whole mark, the dot in its place (owner PHN-700); until the breath nothing moves.
    // decision: PHN-632, PHN-700, PHN-701
    func testTheFirstFrameIsTheWholeMarkAndNothingDrops() {
        for reduced in [false, true] { XCTAssertEqual(FerbLaunch.at(0, readyAt: nil, reduced: reduced).dotAlpha, 1) }
        for ms in stride(from: 0.0, to: FerbLaunch.breathAfterMs, by: 2) {
            let f = FerbLaunch.at(ms, readyAt: nil, reduced: false)
            XCTAssertEqual([f.dotY, f.dotAlpha, f.dotScale, f.fScale], [0, 1, 1, 1], "\(ms)")
        }
        for ms in stride(from: 0.0, through: 6000, by: 10) {
            for ready in [nil, 1200.0] { XCTAssertEqual(FerbLaunch.at(ms, readyAt: ready, reduced: false).dotY, 0) }
        }
    }

    /// Slow loading: the dot breathes for as long as it takes; never done, never a prompt.
    // decision: PHN-631
    func testSlowLoadingLoops() {
        let frames = stride(from: 0.0, through: 20_000, by: 50).map { FerbLaunch.at($0, readyAt: nil, reduced: false) }
        XCTAssertTrue(frames.allSatisfy { $0.phase != .done && !$0.promptOk && $0.handover == 0 })
        let breathing = frames.filter { $0.phase == .breathing }
        XCTAssertEqual(breathing.map(\.dotScale).min()!, FerbLaunch.breathScale, accuracy: 0.005)
        XCTAssertEqual(breathing.map(\.dotScale).max()!, 1, accuracy: 0.005)
        let a = FerbLaunch.at(3000, readyAt: nil, reduced: false), b = FerbLaunch.at(3000 + FerbLaunch.breathPeriodMs, readyAt: nil, reduced: false)
        XCTAssertEqual(a.dotScale, b.dotScale, accuracy: 1e-6)
        XCTAssertEqual(FerbLaunch.at(7777 + Double(FerbLaunch.settle.ms), readyAt: 7777, reduced: false).phase, .done)
    }

    /// Reduce Motion: nothing moves at any moment; the whole mark is still, then the overlay fades off.
    // decision: PHN-634
    func testReducedMotionIsStill() {
        for ms in stride(from: 0.0, through: 5000, by: 25) {
            for ready in [nil, 0.0, 1000.0] {
                let f = FerbLaunch.at(ms, readyAt: ready, reduced: true)
                XCTAssertEqual(f.dotY, 0)
                XCTAssertEqual(f.dotAlpha, 1)
                XCTAssertEqual(f.dotScale, 1)
                XCTAssertEqual(f.fScale, 1)
            }
        }
        XCTAssertFalse(FerbLaunch.at(100, readyAt: 0, reduced: true).promptOk)
        XCTAssertTrue(FerbLaunch.at(200, readyAt: 0, reduced: true).promptOk)
    }

    /// The lock page under the launch at one moment, as RootView draws it (the host with a fixed moment).
    private func lockPage(at ms: Double, ready: Double?) -> some View {
        let state = LaunchState(reduced: false)
        return LabScreenView(screen: .unlock, moment: .lab(12.5), glide: false)
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
            .stillMotion()
            .environment(\.launchHidesTarget, true)
            .overlayPreferenceValue(LaunchTargetKey.self) { anchor in
                GeometryReader { proxy in
                    LaunchOverlay(state: state, target: anchor.map { proxy[$0] }, at: ms, readyAt: ready)
                }
            }
    }

    /// The first frame is the launch screen: the whole mark, its dot in place, at the screen's centre over ink0.
    // decision: PHN-632
    func testTheFirstFrame() {
        Snapshot.assert(lockPage(at: 0, ready: nil), size: PhoneInsets.labSize, path: "launch/first-frame-noon.png")
    }

    // decision: PHN-631
    func testAMomentOfTheSettle() {
        Snapshot.assert(lockPage(at: 200, ready: 0), size: PhoneInsets.labSize, path: "launch/settling-noon.png")
    }

    /// The last frame before the launch hands over draws the lock page as it is: the shared element lands exactly on
    /// the page's mark, at its size, and ink0 is gone. (SwiftUI scales the 112-point mark rather than drawing it at
    /// 84, so edges may differ by a shade: a pixel counts as changed only past 24 of 255 in a channel.)
    // decision: PHN-631, PHN-632
    func testTheSettledFrameRestsOnTheLockPage() {
        let page = LabScreenView(screen: .unlock, moment: .lab(12.5), glide: false)
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height).stillMotion()
        let a = Snapshot.pixels(Snapshot.render(lockPage(at: Double(FerbLaunch.settle.ms) - 0.01, ready: 0), size: PhoneInsets.labSize).cgImage!)
        let b = Snapshot.pixels(Snapshot.render(page, size: PhoneInsets.labSize).cgImage!)
        XCTAssertEqual(a.width, b.width)
        var changed = 0
        for i in stride(from: 0, to: min(a.bytes.count, b.bytes.count), by: 4) {
            if (0..<3).contains(where: { abs(Int(a.bytes[i + $0]) - Int(b.bytes[i + $0])) > 24 }) { changed += 1 }
        }
        XCTAssertLessThan(Double(changed) / Double(a.width * a.height), Snapshot.threshold, "\(changed) pixels differ")
    }

    /// The launch screen's image (App/Assets LaunchMark) is the package's own mark at the launch's size with its dot
    /// at rest (PHN-700), drawn here: the first frame and the launch screen are one picture. FERB_RECORD_SNAPSHOTS=1
    /// writes it.
    // decision: PHN-632
    func testTheLaunchScreenImageIsTheFirstFrame() throws {
        let folder = LabNumbersTests.repo.appendingPathComponent("apps/ios/App/Assets.xcassets/LaunchMark.imageset")
        for scale in [2, 3] {
            let url = folder.appendingPathComponent("launch-mark@\(scale)x.png")
            let r = ImageRenderer(content: LaunchMark(dotY: 0, dotAlpha: 1, dotScale: 1, fScale: 1).environment(\.colorScheme, .light))
            r.scale = CGFloat(scale)
            r.isOpaque = false
            let drawn = try XCTUnwrap(r.cgImage)
            if Snapshot.recording {
                try Snapshot.png(drawn).write(to: url)
                continue
            }
            let stored = try XCTUnwrap(UIImage(data: Data(contentsOf: url))?.cgImage)
            let a = Snapshot.pixels(drawn), b = Snapshot.pixels(stored)
            XCTAssertEqual(a.width, b.width)
            XCTAssertEqual(a.height, b.height)
            var worst = 0
            for i in 0..<min(a.bytes.count, b.bytes.count) { worst = max(worst, abs(Int(a.bytes[i]) - Int(b.bytes[i]))) }
            XCTAssertLessThanOrEqual(worst, 2, "the launch screen's image drifted from FerbMark: record with FERB_RECORD_SNAPSHOTS=1")
        }
    }
}
#endif
