import XCTest
import AppKit
import SwiftUI
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class LatestSourceHoverTests: XCTestCase {
    private final class Entries: ObservableObject {
        @Published var values: [TranscriptEntry]
        init(_ values: [TranscriptEntry]) { self.values = values }
    }
    private struct Chat: View {
        @ObservedObject var entries: Entries
        let model: NotchModel
        var body: some View {
            VStack(spacing: 8) {
                ConversationTranscript(entries: entries.values, model: model)
                ChatInput(model: model).frame(height: 36)
            }
        }
    }
    private func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
    private func settle(_ host: NSView) async throws {
        for _ in 0..<5 { host.layoutSubtreeIfNeeded(); try await Task.sleep(for: .milliseconds(20)) }
    }

    func testRouterClearsOldCursorBeforeEnteringNextAnchor() {
        final class CursorTracker: PreviewHoverTrackingView {
            var cursors: ((NSCursor) -> Void)?
            override func applyPointerCursor(_ cursor: NSCursor) { cursors?(cursor) }
        }
        let window = NSWindow(contentRect:CGRect(x:0,y:0,width:300,height:100),styleMask:.borderless,backing:.buffered,defer:false)
        let root = NSView(frame:CGRect(x:0,y:0,width:300,height:100))
        window.contentView = root
        let left = CursorTracker(frame:CGRect(x:0,y:0,width:100,height:30))
        let right = CursorTracker(frame:CGRect(x:150,y:0,width:100,height:30))
        var cursors: [NSCursor] = []
        for view in [left,right] { view.pointingHand = true; view.cursors = { cursors.append($0) }; root.addSubview(view) }
        func move(_ x: CGFloat) { PreviewHoverRouter.shared.route(point:window.convertPoint(toScreen:NSPoint(x:x,y:15)), frontmostWindowNumber:window.windowNumber) }
        move(50)
        for x in [200.0,50,200,50] {
            cursors.removeAll()
            move(x)
            XCTAssertEqual(cursors.count,2)
            XCTAssertTrue(cursors.first === NSCursor.arrow)
            XCTAssertTrue(cursors.last === NSCursor.pointingHand)
            move(x)
            XCTAssertEqual(cursors.count,2,"An unchanged anchor must not reset the cursor each sample")
        }
        XCTAssertFalse(window.isVisible)
    }

    func testPointerMonitorReconcilesLastSourceWithoutNativeEnterDelivery() async throws {
        var turn = ConversationTurn(id: UUID(), createdAt: Date(), request: "Explain", sources: [])
        turn.answer = String(repeating: "A long answer paragraph.\n\n", count: 35) + "Sites contacted: in.usembassy.gov, travel.state.gov"
        turn.status = "done"
        let entries = Entries([.turn(turn, answeredBelow: false)])
        let model = NotchModel(preferences: scratchDefaults())
        let host = NSHostingView(rootView: Chat(entries: entries, model: model))
        host.frame = CGRect(x:0,y:0,width:440,height:model.chatTranscriptHeight+44)
        let window = NSWindow(contentRect:host.frame,styleMask:.borderless,backing:.buffered,defer:false)
        window.contentView = host
        try await settle(host)
        let tracker = try XCTUnwrap(descendants(host).compactMap { $0 as? PreviewHoverTrackingView }.first)
        var transitions: [Bool] = []
        tracker.changed = { transitions.append($0) }
        let center = window.convertPoint(toScreen:tracker.convert(NSPoint(x:tracker.bounds.midX,y:tracker.bounds.midY),to:nil))
        // Owner trace: pointer over the actual latest pill and own topmost window,
        // full clipped bounds + native tracking area, but no native enter delivered.
        PreviewHoverRouter.shared.route(point:center, frontmostWindowNumber:window.windowNumber)
        XCTAssertEqual(transitions,[true])
        PreviewHoverRouter.shared.route(point:center, frontmostWindowNumber:window.windowNumber)
        XCTAssertEqual(transitions,[true], "Repeated pointer samples must not reopen the preview")
        PreviewHoverRouter.shared.route(point:center, frontmostWindowNumber:window.windowNumber+1)
        XCTAssertEqual(transitions,[true,false], "A popover covering the source owns the pointer")
        PreviewHoverRouter.shared.route(point:center, frontmostWindowNumber:window.windowNumber)
        let scroll = try XCTUnwrap(descendants(host).compactMap { $0 as? NSScrollView }.first)
        scroll.contentView.scroll(to:.zero)
        scroll.reflectScrolledClipView(scroll.contentView)
        try await settle(host)
        PreviewHoverRouter.shared.route(point:center, frontmostWindowNumber:window.windowNumber)
        XCTAssertEqual(transitions.last,false, "Scrolling the source out clears hover without a native exit")
        XCTAssertFalse(window.isVisible)
    }

    func testTranscriptOffsetStaysFixedWhileCardTranslatesWithinDragCanvas() async throws {
        let model = NotchModel(preferences: scratchDefaults())
        let id = UUID()
        model.conversation.begin(id: id, request: "Explain", sources: [])
        model.conversation.finish(id: id, answer: String(repeating: "Long answer paragraph here. More words make a sentence.\n\n", count: 35) + "Sites contacted: constitution.congress.gov, congress.gov", status: "done", artifacts: [])
        model.resizePanel(width: 427, transcriptHeight: 306.5)
        model.expand()
        let presentation = NotchPresentation()
        let size = model.surfaceSize
        presentation.backdrop.frame = CGRect(origin: .zero, size: size)
        presentation.present(size: size, expanded: true, bodyVisible: true, animate: false) {}
        let host = TrackingHostingView(rootView: NotchView(model: model, presentation: presentation))
        host.sizingOptions = []
        host.frame = CGRect(origin: .zero, size: size)
        let window = NotchPanel(contentRect: host.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.contentView = presentation.backdrop
        host.autoresizingMask = [.width, .height]
        presentation.backdrop.addSubview(host)
        let control = NSView()
        presentation.backdrop.addSubview(control)
        try await settle(host)
        let scroll = try XCTUnwrap(descendants(host).compactMap { $0 as? NSScrollView }.first)
        let tracker = try XCTUnwrap(descendants(host).compactMap { $0 as? PreviewHoverTrackingView }.first)
        let original = scroll.contentView.bounds.origin
        let originalHeight = scroll.documentView!.frame.height
        for index in 0..<60 {
            let y = 100 + sin(Double(index) / 4) * 20.321
            let card = CGRect(x: 350 + cos(Double(index) / 3) * 30.321, y: y, width: size.width, height: size.height)
            model.availableSurfaceSize = card.size
            DragSurfaceCanvas.place(window: window, presentation: presentation, control: control,
                pill: CGRect(x: 500,y: y + size.height-8,width:58,height:30), card: card,
                canvas: CGRect(x:0,y:0,width:1400,height:1000), bodyVisible:true)
            let scale = window.backingScaleFactor
            XCTAssertEqual(presentation.surfaceOrigin.x * scale, (presentation.surfaceOrigin.x * scale).rounded(), accuracy: 0.000001)
            XCTAssertEqual(presentation.surfaceOrigin.y * scale, (presentation.surfaceOrigin.y * scale).rounded(), accuracy: 0.000001)
            host.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(16))
            XCTAssertEqual(scroll.contentView.bounds.width, size.width - 50, accuracy: 0.01)
            XCTAssertEqual(scroll.contentView.bounds.origin.y, original.y, accuracy: 0.5)
            XCTAssertEqual(scroll.documentView!.frame.height, originalHeight, accuracy: 0.5)
            XCTAssertFalse(tracker.hoverRect.isEmpty)
        }
        XCTAssertFalse(window.isVisible)
    }

    func testFullNotchSourceUsesNativeAutomaticClippingAcrossScrollAndReopen() async throws {
        let model = NotchModel(preferences: scratchDefaults())
        let id = UUID()
        model.conversation.begin(id: id, request: "Explain", sources: [])
        model.conversation.finish(id: id, answer: String(repeating: "Long answer paragraph here.\n\n", count: 35) + "Sites contacted: constitution.congress.gov, congress.gov", status: "done", artifacts: [])
        model.expand()
        let presentation = NotchPresentation()
        presentation.backdrop.frame = CGRect(origin: .zero, size: model.panelSize)
        presentation.present(size: model.panelSize, expanded: true, bodyVisible: true, animate: false) {}
        let host = TrackingHostingView(rootView: NotchView(model: model, presentation: presentation))
        host.sizingOptions = []
        host.frame = CGRect(origin: .zero, size: model.panelSize)
        let window = NotchPanel(contentRect: host.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.contentView = presentation.backdrop
        host.autoresizingMask = [.width, .height]
        presentation.backdrop.addSubview(host)
        let resize = PanelResizeOverlay(model: model)
        resize.frame = presentation.backdrop.bounds
        presentation.backdrop.addSubview(resize)
        try await settle(host)
        let tracker = try XCTUnwrap(descendants(host).compactMap { $0 as? PreviewHoverTrackingView }.first)
        XCTAssertEqual(tracker.visibleRect, tracker.bounds,
                       "Native automatic tracking must see the pill, not SwiftUI's entire transcript viewport")
        let area = try XCTUnwrap(tracker.trackingAreas.first)
        XCTAssertTrue(area.options.contains(.inVisibleRect))
        XCTAssertEqual(area.rect, .zero, "AppKit owns the dynamic rectangle, not a stale explicit snapshot")
        tracker.resetCursorRects()
        let scroll = try XCTUnwrap(descendants(host).compactMap { $0 as? NSScrollView }.first)
        let bottom = scroll.contentView.bounds.origin
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
        try await settle(host)
        XCTAssertEqual(tracker.visibleRect, .zero)
        tracker.resetCursorRects() // The offscreen cursor path must stay safe.
        scroll.contentView.scroll(to: bottom)
        scroll.reflectScrolledClipView(scroll.contentView)
        try await settle(host)
        XCTAssertEqual(tracker.visibleRect, tracker.bounds)
        XCTAssertTrue(tracker.trackingAreas.contains { $0.options.contains(.inVisibleRect) })
        // The actual retained hosting hierarchy is compacted and revealed again.
        model.collapse()
        presentation.present(size: model.panelSize, expanded: false, bodyVisible: false, animate: false) {}
        window.setContentSize(model.panelSize)
        try await settle(host)
        model.expand()
        presentation.present(size: model.panelSize, expanded: true, bodyVisible: true, animate: false) {}
        window.setContentSize(model.panelSize)
        try await settle(host)
        let reopened = try XCTUnwrap(descendants(host).compactMap { $0 as? PreviewHoverTrackingView }.first)
        XCTAssertEqual(reopened.visibleRect, reopened.bounds)
        XCTAssertTrue(reopened.trackingAreas.contains { $0.options.contains(.inVisibleRect) })
        reopened.resetCursorRects()
        XCTAssertFalse(window.isVisible)
    }

    func testInvalidHoverRectanglesNeverRegisterNativeTrackingOrCursors() {
        final class ClippedTracker: PreviewHoverTrackingView {
            var clipping = NSRect.zero
            override var visibleRect: NSRect { clipping }
        }
        let tracker = ClippedTracker(frame: NSRect(x: 0, y: 0, width: 120, height: 24))
        tracker.pointingHand = true
        for rect in [NSRect.null, .zero,
                     NSRect(x: 300, y: 300, width: 20, height: 20),
                     NSRect(x: CGFloat.infinity, y: 0, width: 20, height: 20),
                     NSRect(x: 0, y: 0, width: CGFloat.infinity, height: 20)] {
            tracker.clipping = rect
            tracker.updateTrackingAreas()
            XCTAssertTrue(tracker.trackingAreas.isEmpty)
            XCTAssertEqual(tracker.hoverRect, .zero)
            if tracker.hoverRect == .zero { tracker.resetCursorRects() }
        }
        tracker.clipping = tracker.bounds
        tracker.updateTrackingAreas()
        XCTAssertEqual(tracker.trackingAreas.count, 1)
        XCTAssertEqual(tracker.hoverRect, tracker.bounds)
        tracker.resetCursorRects()
    }

    func testLatestSourceTracksOnlyItsPillBeforeAndAfterReplyResizeAndScroll() async throws {
        var turn = ConversationTurn(id: UUID(), createdAt: Date(), request: "Explain this", sources: [])
        turn.answer = String(repeating: "This is a paragraph about the answer.\n\n", count: 18) + "Sites contacted: constitution.congress.gov, congress.gov"
        turn.status = "done"
        let entries = Entries([.turn(turn, answeredBelow: false)])
        let model = NotchModel(preferences: scratchDefaults())
        let host = NSHostingView(rootView: Chat(entries: entries, model: model))
        host.frame = CGRect(x: 0, y: 0, width: 440, height: model.chatTranscriptHeight + 44)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        try await settle(host)
        let tracker = try XCTUnwrap(descendants(host).compactMap { $0 as? PreviewHoverTrackingView }.first)
        let input = try XCTUnwrap(descendants(host).compactMap { $0 as? ChatTextView }.first)
        let pill = tracker.convert(tracker.bounds, to: host)
        XCTAssertLessThan(pill.maxY, input.convert(input.bounds, to: host).minY)
        XCTAssertGreaterThanOrEqual(tracker.visibleRect.height, tracker.bounds.height)

        func checkAnchor() throws {
            tracker.updateTrackingAreas()
            let area = try XCTUnwrap(tracker.trackingAreas.first)
            XCTAssertTrue(area.options.contains(.inVisibleRect))
            XCTAssertTrue(area.options.contains(.activeAlways))
            XCTAssertEqual(area.rect, .zero)
            XCTAssertLessThanOrEqual(tracker.visibleRect.height, tracker.bounds.height)
            XCTAssertFalse(tracker.visibleRect.isEmpty)
            var hovered = false
            tracker.changed = { hovered = $0 }
            let center = tracker.convert(NSPoint(x: tracker.bounds.midX, y: tracker.bounds.midY), to: nil)
            let enter = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered, location: center,
                modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                eventNumber: 1, trackingNumber: 0, userData: nil))
            tracker.mouseEntered(with: enter)
            XCTAssertTrue(hovered)
            // A move over the neighboring composer cannot keep the pill hovered.
            let outside = input.convert(NSPoint(x: input.bounds.midX, y: input.bounds.midY), to: nil)
            let move = try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: outside,
                modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                eventNumber: 2, clickCount: 0, pressure: 0))
            tracker.mouseMoved(with: move)
            XCTAssertFalse(hovered)
        }
        try checkAnchor()
        entries.values.append(.turn(ConversationTurn(id: UUID(), createdAt: Date(), request: "Tell me more", sources: []), answeredBelow: false))
        try await settle(host)
        try checkAnchor()
        window.setContentSize(NSSize(width: 380, height: model.chatTranscriptHeight + 44))
        try await settle(host)
        try checkAnchor()
        let scroll = try XCTUnwrap(descendants(host).compactMap { $0 as? NSScrollView }.first)
        var clippedHover = false
        tracker.changed = { clippedHover = $0 }
        let hoverCenter = tracker.convert(NSPoint(x: tracker.bounds.midX, y: tracker.bounds.midY), to: nil)
        let enteredBeforeClipping = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered,
            location: hoverCenter, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber,
            context: nil, eventNumber: 3, trackingNumber: 0, userData: nil))
        tracker.mouseEntered(with: enteredBeforeClipping)
        XCTAssertTrue(clippedHover)
        let visibleOrigin = scroll.contentView.bounds.origin
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
        try await settle(host)
        tracker.updateTrackingAreas()
        XCTAssertTrue(tracker.hoverRect.isEmpty, "An offscreen source must not track the visible transcript")
        XCTAssertFalse(clippedHover, "Clipping clears an existing hover without needing a mouse exit")
        XCTAssertTrue(tracker.trackingAreas.isEmpty)
        // CoreGraphics returns CGRect.null (infinite origin) for disjoint rectangles.
        // Never pass that to AppKit's cursor API when archived sources are clipped.
        guard tracker.hoverRect.origin.x.isFinite, tracker.hoverRect.origin.y.isFinite else {
            XCTFail("Offscreen hover rectangles must be finite before cursor registration")
            return
        }
        tracker.resetCursorRects()
        scroll.contentView.scroll(to: visibleOrigin)
        scroll.reflectScrolledClipView(scroll.contentView)
        try await settle(host)
        try checkAnchor()
        tracker.resetCursorRects()
        XCTAssertFalse(window.isVisible)
        withExtendedLifetime(window) {}
    }
}
