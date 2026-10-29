import XCTest
import AppKit
import Combine
@testable import FERBNotch

final class FloatingRenderingTests: XCTestCase {
    @MainActor func testUnchangedBackgroundKeepsMaskAndDoesNotFlushPendingPaint() {
        let view = NotchBackdropView(frame: CGRect(x: 0, y: 0, width: 1512, height: 982))
        let window = NSWindow(contentRect: view.frame, styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = view
        view.updateSurface(size: CGSize(width: 440, height: 600), expansion: 1,
                           cutoutHeight: 40, origin: CGPoint(x: 100, y: 100))
        let mask = view.maskImage
        let tint = view.subviews.last!
        tint.needsDisplay = true
        view.updateSurface(size: CGSize(width: 440, height: 600), expansion: 1,
                           cutoutHeight: 40, origin: CGPoint(x: 100, y: 100))
        view.attachmentRect = nil
        view.attachmentEdge = .left
        view.docked = false
        XCTAssertTrue(view.maskImage === mask, "Identical state must reuse the native blur mask")
        XCTAssertTrue(tint.needsDisplay, "Geometry updates must not synchronously flush pending paint")
    }

    @MainActor func testBatchDefersUnionAndTranslationRetainsTintPaint() {
        let view = NotchBackdropView(frame: CGRect(x: 0, y: 0, width: 1512, height: 982))
        let window = NSWindow(contentRect: view.frame, styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = view
        view.updateSurface(size: CGSize(width: 440, height: 600), expansion: 1,
                           cutoutHeight: 40, origin: CGPoint(x: 100, y: 100))
        let tint = view.subviews.last!
        tint.displayIfNeeded()
        tint.needsDisplay = false
        let old = view.maskImage
        view.performUpdates {
            view.updateSurface(size: CGSize(width: 440, height: 600), expansion: 1,
                               cutoutHeight: 40, origin: CGPoint(x: 120, y: 100))
            view.attachmentRect = CGRect(x: 64, y: 300, width: 58, height: 30)
            view.attachmentEdge = .left
            XCTAssertTrue(view.maskImage === old, "No intermediate disconnected mask")
        }
        XCTAssertFalse(view.maskImage === old)
        XCTAssertEqual(tint.layerContentsRedrawPolicy, .onSetNeedsDisplay)
        XCTAssertEqual(tint.frame.origin, CGPoint(x: 120, y: 100))
        let moved = view.maskImage
        view.docked = true
        XCTAssertFalse(view.maskImage === moved)
        XCTAssertTrue(tint.needsDisplay, "Shape changes still redraw the tint")
        tint.needsDisplay = false
        NSWorkspace.shared.notificationCenter.post(
            name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
        XCTAssertTrue(tint.needsDisplay, "Transparency changes invalidate retained pixels")
        view.updateSurface(size: CGSize(width: 440, height: 600), expansion: 1,
                           cutoutHeight: 40, origin: CGPoint(x: 130, y: 100))
        XCTAssertTrue(tint.needsDisplay, "Translation must preserve accessibility invalidation")
    }

    @MainActor func testTranslationPublishesButRepeatedDragSampleDoesNot() {
        let presentation = NotchPresentation()
        presentation.backdrop.frame = CGRect(x: 0, y: 0, width: 1512, height: 982)
        var changes = 0
        let observation = presentation.objectWillChange.sink { changes += 1 }
        let size = CGSize(width: 440, height: 600)
        presentation.placeDuringDrag(size: size, origin: CGPoint(x: 100, y: 100), bodyVisible: true)
        let initial = changes
        presentation.placeDuringDrag(size: size, origin: CGPoint(x: 100, y: 100), bodyVisible: true)
        XCTAssertEqual(changes, initial)
        presentation.placeDuringDrag(size: size, origin: CGPoint(x: 110, y: 100), bodyVisible: true)
        XCTAssertEqual(changes, initial + 1, "Translation must still move SwiftUI content with its mask")
        presentation.backdrop.frame.size.width = 1600
        presentation.placeDuringDrag(size: size, origin: CGPoint(x: 110, y: 100), bodyVisible: true)
        XCTAssertEqual(changes, initial + 2, "Canvas centering changes must reach SwiftUI")
        withExtendedLifetime(observation) {}
    }

    @MainActor func testFloatingRenderWorkMeasurement() {
        let view = NotchBackdropView(frame: CGRect(x: 0, y: 0, width: 1512, height: 982))
        let start = CFAbsoluteTimeGetCurrent()
        var replacements = 0
        for i in 0..<120 {
            let origin = CGPoint(x: CGFloat(i) + 100, y: 100)
            func count(_ update: () -> Void) {
                let old = view.maskImage
                update()
                if view.maskImage !== old { replacements += 1 }
            }
            let before = view.maskImage
            view.performUpdates {
                count { view.updateSurface(size: CGSize(width: 440, height: 600), expansion: 1,
                                           cutoutHeight: 40, origin: origin) }
                count { view.attachmentRect = CGRect(x: origin.x - 56, y: 300, width: 58, height: 30) }
                count { view.attachmentEdge = .left }
                count { view.attachmentEdge = .left }
            }
            // The deferred final union is the only replacement in this frame.
            if view.maskImage !== before { replacements += 1 }
        }
        XCTAssertEqual(replacements, 120)
        print("FLOATING_RENDER_WORK 120 frames mask replacements=\(replacements) elapsed_ms=\((CFAbsoluteTimeGetCurrent()-start)*1000)")
    }
}
