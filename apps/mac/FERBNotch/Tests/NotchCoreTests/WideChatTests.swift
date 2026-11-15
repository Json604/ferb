import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Owner 2026-11-09, shown a wide chat whose answers and timer cards ran across the whole window: "the space between ferb
/// res and my msg shodl increase with reply boxes adn ohther boxes not affected". In a chat wider than its standard
/// 440 pt, every message keeps the width it has at the standard width (ChatSide): FERB's on the left, the owner's on the
/// right, the extra width between them.
@MainActor
final class WideChatTests: XCTestCase {
    // decision: APP-816
    func testAWideChatKeepsEveryMessageAtItsStandardWidth() async throws {
        let probe = TranscriptScrollProbeTests()
        let (model, host, window) = try await probe.mixedDay()
        window.setContentSize(CGSize(width: 1200, height: 900))
        model.selectDay(TranscriptScrollProbeTests.day, focus: false)
        await probe.settle(host)
        let scroll = try XCTUnwrap(probe.scrollView(in: host), "the transcript's scroll view")
        XCTAssertGreaterThan(scroll.frame.width, 1100, "the chat is wide")
        let requests = Set((0..<TranscriptScrollProbeTests.entries).map(TranscriptScrollProbeTests.request))
        var mine = 0, ferbs = 0
        let clip = scroll.contentView
        for step in 0..<40 {
        clip.scroll(to: CGPoint(x: 0, y: max(0, (scroll.documentView?.frame.height ?? 0) - clip.bounds.height - CGFloat(step) * 150)))
        scroll.reflectScrolledClipView(clip)
        await probe.settle(host)
        let shown = clip.convert(clip.bounds, to: scroll)
        for view in probe.textViews(in: host) where !view.isHiddenOrHasHiddenAncestor {
            let frame = view.convert(view.bounds, to: scroll)
            guard frame.intersects(shown), frame.height > 0 else { continue }
            XCTAssertLessThanOrEqual(frame.width, ChatSide.width + 0.5, "a message no wider than at the standard width: \(view.string.prefix(40))")
            if ProcessInfo.processInfo.environment["FERB_PROBE_TRACE"] == "1" { print("WIDE x=\(Int(frame.minX))..\(Int(frame.maxX)) \(view.string.prefix(50))") }
            if requests.contains(view.string) {
                mine += 1
                XCTAssertGreaterThan(frame.maxX, scroll.frame.width - 30, "the owner's message on the right: \(view.string.prefix(40))")
            } else {
                ferbs += 1
                XCTAssertLessThan(frame.minX, 40, "FERB's message on the left: \(view.string.prefix(40))")
            }
        }
        }
        XCTAssertGreaterThan(mine, 0, "the owner's messages in view")
        XCTAssertGreaterThan(ferbs, 0, "FERB's messages in view")
        withExtendedLifetime(window) {}
    }
}
