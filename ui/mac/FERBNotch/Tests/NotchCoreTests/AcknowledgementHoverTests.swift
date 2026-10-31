import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class AcknowledgementHoverTests: XCTestCase {
    func testFreshPlanIsImmediateWithoutExpandingAndDoesNotReplay() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Check my task"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Check my task", sources: [])
        model.collapse()
        var focus = 0
        model.onPresentationChange = { _ in focus += 1 }
        model.receiveChatEvent(["type": "ack", "text": "Sure, starting now."], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Checking now"], localID: id)
        XCTAssertEqual(model.acknowledgement.text, "Checking now")
        XCTAssertEqual(model.conversation.turns.first?.plan, "Checking now")
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(focus, 0)
        model.acknowledgement.dismiss()
        model.receiveChatEvent(["type": "ack", "text": "Sure, starting now."], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Checking now"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        model.expand()
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Still checking"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        XCTAssertEqual(model.conversation.turns.first?.plan, "Still checking")
    }

    func testBackgroundPlanUsesForegroundVisibilityAndHistoryNeverReplays() {
        let preferences = scratchDefaults()
        let model = NotchModel(preferences: preferences)
        model.session.draft = "Background task"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Background task", sources: [])
        model.workSessions[id] = model.session
        model.session = NotchSession()
        model.session.collapse()
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Checking background"], localID: id)
        XCTAssertEqual(model.acknowledgement.text, "Checking background")
        XCTAssertFalse(model.expanded)
        model.expand()
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "New acknowledgement"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        XCTAssertTrue(model.expanded)
        let restored = NotchModel(preferences: preferences)
        XCTAssertNil(restored.acknowledgement.text)
        XCTAssertEqual(restored.conversation.turns.first?.plan, "New acknowledgement")
    }

    func testNativeHandCursorRectAndHoverHighlightLifecycle() async throws {
        let state = ArtifactHoverState()
        let view = CursorRecordingHoverView(frame: NSRect(x: 0, y: 0, width: 100, height: 28))
        let window = NSPanel(contentRect: view.frame, styleMask: [.borderless, .nonactivatingPanel],
                             backing: .buffered, defer: false)
        window.contentView = view
        view.pointingHand = true
        view.changed = state.anchorHovered
        view.resetCursorRects()
        XCTAssertEqual(view.registeredCursor, NSCursor.pointingHand)
        XCTAssertEqual(view.registeredRect, view.visibleRect)
        view.updateTrackingAreas()
        XCTAssertTrue(view.trackingAreas.contains { $0.options.contains(.activeAlways) && $0.options.contains(.cursorUpdate) })
        XCTAssertNil(view.hitTest(.zero))
        let event = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered, location: NSPoint(x: 20, y: 20),
            modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
            eventNumber: 1, trackingNumber: 0, userData: nil))
        view.mouseEntered(with: event)
        XCTAssertTrue(state.isHovered)
        view.mouseExited(with: event)
        XCTAssertFalse(state.isHovered)
        view.mouseEntered(with: event)
        view.removeFromSuperview()
        await Task.yield()
        XCTAssertFalse(state.isHovered)
        state.dismiss()
    }

    func testInterruptedOrStoppedRequestDismissesAcknowledgement() {
        for ending in ["abort", "disconnect", "stop"] {
            let model = NotchModel(preferences: scratchDefaults())
            model.session.draft = "Check task"
            let id = model.session.beginWork()!
            model.conversation.begin(id: id, request: "Check task", sources: [])
            model.collapse()
            model.receiveChatEvent(["type": "ack", "text": "Checking"], localID: id)
            model.acknowledgement.append("Checking plan", turn: id)
            XCTAssertNotNil(model.acknowledgement.text)
            switch ending {
            case "abort": model.receiveChatEvent(["type": "reply_abort", "reason": "Connection lost"], localID: id)
            case "disconnect": model.finishSubmission(localID: id)
            default: model.requestDaemonStop()
            }
            XCTAssertNil(model.acknowledgement.text, ending)
            XCTAssertEqual(model.conversation.turns.first?.ack, "Checking")
        }
    }

    func testReplacementTimerCannotDismissNewAckAndDismissalCancels() async throws {
        let notice = AcknowledgementNotice(duration: .milliseconds(100))
        notice.show("First")
        try await Task.sleep(for: .milliseconds(60))
        notice.show("Second")
        try await Task.sleep(for: .milliseconds(60))
        XCTAssertEqual(notice.text, "Second")
        try await Task.sleep(for: .milliseconds(70))
        XCTAssertNil(notice.text)
        notice.show("Third")
        notice.dismiss()
        XCTAssertNil(notice.text)
    }

    func testAllAttentionPathsDismissSmallAck() {
        let model = NotchModel(preferences: scratchDefaults())
        for action in [{ model.expand() }, { model.presentIncomingAnswer() },
                       { model.presentTerminalAnswer() }, { model.announceAttention(UUID().uuidString) }] {
            model.acknowledgement.show("Checking")
            action()
            XCTAssertNil(model.acknowledgement.text)
        }
    }

    func testSitesUnionAllowsCrossingAndScrollingThenDismissesOutsideBoth() async throws {
        let state = SitesHoverState()
        state.anchorHovered(true)
        state.anchorHovered(false)
        XCTAssertTrue(state.isPresented)
        state.previewHovered(true)
        try await Task.sleep(for: .milliseconds(240))
        XCTAssertTrue(state.isPresented)
        state.previewHovered(false)
        try await Task.sleep(for: .milliseconds(240))
        XCTAssertFalse(state.isPresented)
        state.activate(keyboard: true)
        state.anchorHovered(false)
        try await Task.sleep(for: .milliseconds(240))
        XCTAssertTrue(state.isPresented)
        state.dismiss()
        XCTAssertFalse(state.isPresented)
    }

    func testNativePopoverHoverKeepsSitesOpenAndOccludedArtifactCannotTakeOver() async throws {
        let sites = SitesHoverState()
        let artifact = ArtifactHoverState()
        let host = NSHostingView(rootView: NativePreviewHover(changed: sites.previewHovered))
        let window = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        func tracker(in view: NSView) -> PreviewHoverTrackingView? {
            if let found = view as? PreviewHoverTrackingView { return found }
            return view.subviews.lazy.compactMap { tracker(in: $0) }.first
        }
        let popupTracker = try XCTUnwrap(tracker(in: host))
        let artifactTracker = OwnershipHoverView(frame: popupTracker.frame)
        artifactTracker.changed = artifact.anchorHovered
        artifactTracker.ownsPointer = false
        let event = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered, location: NSPoint(x: 20, y: 20),
            modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
            eventNumber: 1, trackingNumber: 0, userData: nil))
        sites.anchorHovered(true)
        sites.anchorHovered(false)
        popupTracker.mouseEntered(with: event)
        artifactTracker.mouseEntered(with: event)
        try await Task.sleep(for: .milliseconds(240))
        XCTAssertTrue(sites.isPresented)
        XCTAssertFalse(artifact.isPresented)
        popupTracker.mouseMoved(with: event)
        XCTAssertTrue(sites.isPresented)
        popupTracker.mouseExited(with: event)
        try await Task.sleep(for: .milliseconds(240))
        XCTAssertFalse(sites.isPresented)
    }

    func testWrongRequestPlanCannotPresentCaption() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Check task"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Check task", sources: [])
        model.collapse()
        model.receiveChatEvent(["type": "ack", "request_id": "correct", "text": "Starting"], localID: id)
        model.receiveChatEvent(["type": "plan", "request_id": "wrong", "task_id": "task", "text": "Wrong"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        XCTAssertNil(model.conversation.turns.first?.plan)
    }

    func testCompactPreviewFitsAvailableSurfaceForEveryEdge() {
        let model = NotchModel(preferences: scratchDefaults())
        model.availableSurfaceSize = CGSize(width: 300, height: 100)
        model.collapse()
        for edge in ["top", "bottom", "left", "right"] {
            model.indicatorEdge = edge
            model.acknowledgement.show(String(repeating: "A long plan. ", count: 100))
            XCTAssertLessThanOrEqual(model.surfaceSize.width, 300)
            XCTAssertLessThanOrEqual(model.surfaceSize.height, 100)
            XCTAssertFalse(model.bodyVisible)
        }
    }

}

@MainActor
private final class CursorRecordingHoverView: PreviewHoverTrackingView {
    var registeredCursor: NSCursor?
    var registeredRect: NSRect?
    override func addCursorRect(_ aRect: NSRect, cursor anObj: NSCursor) {
        registeredCursor = anObj
        registeredRect = aRect
        super.addCursorRect(aRect, cursor: anObj)
    }
}

@MainActor
private final class OwnershipHoverView: PreviewHoverTrackingView {
    var ownsPointer = true
    override func pointerBelongsToWindow(_ event: NSEvent) -> Bool { ownsPointer }
}
