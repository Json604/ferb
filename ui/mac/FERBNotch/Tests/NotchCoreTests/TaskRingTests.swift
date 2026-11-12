import AppKit
import NotchCore
import SwiftUI
import XCTest
@testable import FERBNotch

/// The running tasks' round pictures (APP-899, owner 2026-11-12: "for the desing you proposed, lets do A"): the ring is
/// the task's status, and a task with no frame yet is a soft grey disc, never a black one. Drawn offscreen; the window
/// is never ordered front, so no preview is captured.
@MainActor
final class TaskRingTests: XCTestCase {
    // decision: APP-899
    func testTheRingSaysWhetherATaskWorksOrWaitsForTheOwner() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("TaskRing"))
        model.runningTasks["t-pay"] = "Amazon order"
        model.runningTasks["t-code"] = "Fix the failing test"
        model.pendingApprovals = [DaemonApproval(["approval_id": "a", "task_id": "t-pay", "options": ["Pay", "Don't pay"]])!]
        let items = Dictionary(uniqueKeysWithValues: model.taskItems.filter(\.isRunning).map { ($0.id, $0) })
        let waiting = try XCTUnwrap(items["t-pay"]), working = try XCTUnwrap(items["t-code"])
        XCTAssertEqual(TaskRing.label(waiting), "Amazon order, waiting for you")
        XCTAssertEqual(TaskRing.label(working), "Fix the failing test, working")

        // each circle, in the order the row draws them, checked by its own task's state
        let order = model.taskItems.filter(\.isRunning).map(\.id)
        let rep = try await draw(TaskActivityBubble(model: model))
        let size = TaskActivityBubble.size, spacing: CGFloat = 6
        for (index, id) in order.prefix(2).enumerated() {
            let center = CGPoint(x: CGFloat(index) * (size + spacing) + size / 2, y: size / 2)
            let inside = colour(rep, at: center)
            XCTAssertGreaterThan(luminance(inside), 0.15, "\(id): no frame yet is a soft grey disc, never black")
            XCTAssertLessThan(abs(inside.redComponent - inside.blueComponent), 0.03, "\(id): the empty disc is grey")
            let ring = colour(rep, at: CGPoint(x: center.x - size / 2 + TaskRing.width / 2 + 0.5, y: center.y))
            if id == "t-pay" {
                XCTAssertGreaterThan(ring.redComponent - ring.blueComponent, 0.3, "waiting for the owner: an amber ring")
            } else {
                XCTAssertLessThan(abs(ring.redComponent - ring.blueComponent), 0.05, "working: a white arc, no amber")
            }
        }
    }

    private func draw(_ view: some View) async throws -> NSBitmapImageRep {
        let host = NSHostingView(rootView: view.environment(\.colorScheme, .dark).background(Color(white: 0.11)))
        host.frame.size = host.fittingSize
        let window = NSWindow(contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: true)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host                                   // never ordered front
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        withExtendedLifetime(window) {}
        return rep
    }

    /// The colour at a point in the view's top-left coordinates.
    private func colour(_ rep: NSBitmapImageRep, at point: CGPoint) -> NSColor {
        let scale = CGFloat(rep.pixelsWide) / rep.size.width
        return rep.colorAt(x: Int(point.x * scale), y: Int(point.y * scale))?.usingColorSpace(.sRGB) ?? .black
    }

    private func luminance(_ c: NSColor) -> Double {
        0.2126 * c.redComponent + 0.7152 * c.greenComponent + 0.0722 * c.blueComponent
    }
}
