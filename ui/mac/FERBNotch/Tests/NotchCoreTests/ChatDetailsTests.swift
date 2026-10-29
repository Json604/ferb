import XCTest
import AppKit
import SwiftUI
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class ChatDetailsTests: XCTestCase {
    func testSourceChipPreservesFullListAndShowsFirstHost() {
        XCTAssertEqual(VisitedSites("").all, [])
        XCTAssertEqual(VisitedSites("a, b, c").remainingCount, 2)
        let sites = VisitedSites("a, b, c, d, e")
        XCTAssertEqual(sites.inline, "a")
        XCTAssertEqual(sites.remainingCount, 4)
        XCTAssertEqual(sites.all, ["a", "b", "c", "d", "e"])
    }

    func testNativeHoverTracksInactiveWindowWithoutInterceptingClicks() async throws {
        let hover = ArtifactHoverState()
        let host = NSHostingView(rootView: NativePreviewHover(changed: hover.anchorHovered))
        host.frame = NSRect(x: 0, y: 0, width: 120, height: 30)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        func tracker(in view: NSView) -> PreviewHoverTrackingView? {
            if let result = view as? PreviewHoverTrackingView { return result }
            return view.subviews.compactMap { tracker(in: $0) }.first
        }
        let view = try XCTUnwrap(tracker(in: host))
        view.updateTrackingAreas()
        XCTAssertFalse(window.isKeyWindow)
        XCTAssertTrue(view.trackingAreas.contains { $0.options.contains(.activeAlways) && $0.options.contains(.inVisibleRect) && $0.rect == .zero })
        XCTAssertNil(view.hitTest(NSPoint(x: 2, y: 2)))
        let event = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered, location: view.convert(NSPoint(x: view.bounds.midX, y: view.bounds.midY), to: nil), modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 1, trackingNumber: 0, userData: nil))
        view.mouseEntered(with: event)
        XCTAssertTrue(hover.isPresented)
        view.mouseExited(with: event)
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertFalse(hover.isPresented)
        view.mouseEntered(with: event)
        view.removeFromSuperview()
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertFalse(hover.isPresented)
    }

    func testActualArtifactAndComposerViewsInstallPassiveNativeHoverTracking() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        let url = URL(fileURLWithPath: "/tmp/report.md")
        let host = NSHostingView(rootView: VStack {
            ArtifactRow(url: url, model: model)
            ComposerAttachmentCard(url: url, model: model)
            VisitedSitesView(sites: VisitedSites("one.example, two.example"), model: model)
        })
        host.frame = NSRect(x: 0, y: 0, width: 440, height: 240)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        func trackers(in view: NSView) -> [PreviewHoverTrackingView] {
            (view as? PreviewHoverTrackingView).map { [$0] } ?? view.subviews.flatMap { trackers(in: $0) }
        }
        let installed = trackers(in: host)
        XCTAssertEqual(installed.count, 3)
        for tracker in installed {
            tracker.updateTrackingAreas()
            XCTAssertFalse(tracker.bounds.isEmpty)
            XCTAssertNil(tracker.hitTest(.zero))
            XCTAssertTrue(tracker.trackingAreas.contains { $0.options.contains(.activeAlways) })
        }
    }

    func testArtifactPrimaryActionUsesQuickLookNotDefaultApp() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        let url = URL(fileURLWithPath: "/tmp/report.md")
        var previewed: URL?
        var opened = false
        model.onQuickLook = { previewed = $0 }
        model.onOpenDocument = { _ in opened = true }
        ArtifactRow(url: url, model: model).openPreview()
        XCTAssertEqual(previewed, url)
        XCTAssertFalse(opened)
    }

    func testComposerCardsReserveOneStripAndRestoreEmptyHeightOnRemoval() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        let emptyChrome = model.conversationChromeHeight
        let emptyBody = model.bodyHeight
        XCTAssertEqual(model.attachmentStripHeight, 0)
        let first = URL(fileURLWithPath: "/tmp/report.md")
        let second = URL(fileURLWithPath: "/tmp/render.mjs")
        model.attach([first, second])
        XCTAssertEqual(model.session.attachments, [first, second])
        XCTAssertEqual(model.attachmentStripHeight, 100)
        XCTAssertEqual(model.conversationChromeHeight, emptyChrome + 100)
        XCTAssertEqual(model.bodyHeight, emptyBody + 100)
        XCTAssertLessThanOrEqual(model.chatTranscriptHeight + model.conversationChromeHeight + model.cutoutHeight,
                                 model.availableSurfaceSize.height)
        model.removeAttachment(first)
        XCTAssertEqual(model.attachmentStripHeight, 100)
        XCTAssertEqual(model.session.attachments, [second])
        model.removeAttachment(second)
        XCTAssertEqual(model.conversationChromeHeight, emptyChrome)
        XCTAssertEqual(model.bodyHeight, emptyBody)
    }

    func testFreshRepliesAndErrorsRevealWithoutFocusAndDuplicatesStayClosed() {
        for type in ["reply", "error"] {
            let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
            model.session.draft = "Task"
            let id = model.session.beginWork()!
            model.conversation.begin(id: id, request: "Task", sources: [])
            model.collapse()
            var focus = 0
            model.onPresentationChange = { _ in focus += 1 }
            model.receiveChatEvent(["type": type, "text": "Result"], localID: id)
            XCTAssertTrue(model.bodyVisible, type)
            XCTAssertEqual(focus, 0)
            model.collapse()
            model.receiveChatEvent(["type": type, "text": "Result"], localID: id)
            XCTAssertFalse(model.bodyVisible, type)
        }
    }

    func testFreshExternalOutcomesOpenButHistoryAndRepeatedEventsDoNot() {
        for status in ["done", "failed"] {
            let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
            let event: [String: Any] = ["task_id": "task", "status": status, "answer": "Result"]
            model.receiveOutcome(event)
            XCTAssertTrue(model.bodyVisible)
            model.collapse()
            model.receiveOutcome(event)
            XCTAssertFalse(model.bodyVisible)
            model.receiveOutcome(["task_id": "history", "status": status, "answer": "Old"], announce: false)
            XCTAssertFalse(model.bodyVisible)
        }
    }

    func testBackgroundBoundCompletionExpandsForegroundWithoutReplacingDraft() {
        for status in ["done", "failed"] {
            let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
            model.session.draft = "Background task"
            let id = model.session.beginWork()!
            model.session.bindBackendTask("background", localID: id)
            model.conversation.begin(id: id, request: "Background task", sources: [])
            model.conversation.bind(id: id, taskID: "background")
            model.workSessions[id] = model.session
            model.session = NotchSession()
            model.session.draft = "Next request"
            model.receiveOutcome(["task_id": "background", "status": status, "answer": "Result"])
            XCTAssertTrue(model.bodyVisible)
            XCTAssertEqual(model.session.draft, "Next request")
            model.collapse()
            model.receiveOutcome(["task_id": "background", "status": status, "answer": "Result"])
            XCTAssertFalse(model.bodyVisible)
        }
    }

    func testPreviewExitCancellationAndDismissal() async throws {
        let hover = ArtifactHoverState()
        hover.anchorHovered(true)
        XCTAssertTrue(hover.isPresented)
        hover.anchorHovered(false)
        hover.previewHovered(true)
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertTrue(hover.isPresented)
        hover.previewHovered(false)
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertFalse(hover.isPresented)
        hover.anchorHovered(true)
        hover.dismiss()
        XCTAssertFalse(hover.isPresented)
    }
}
