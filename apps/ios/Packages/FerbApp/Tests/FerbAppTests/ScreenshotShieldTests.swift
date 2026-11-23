#if canImport(UIKit)
import SwiftUI
import UIKit
import XCTest
@testable import FerbAppUI

/// The screenshot shield (kb PHN-597): on this iOS the secure field's canvas is found, taken out of the field, carries
/// FERB's content, and is the layer UIKit marks to leave out of captures. The simulator marks the layer but does not
/// honour the mark in its own screenshots, so whether a screenshot comes out blank is checked on an iPhone (the live
/// check); this test proves the parts iOS lets a test see, and catches an iOS that builds the field differently.
@MainActor
final class ScreenshotShieldTests: XCTestCase {
    // decision: PHN-811
    func testTheSecureCanvasIsFoundAndHostsTheContent() throws {
        XCTAssertTrue(ScreenshotShield.available, "this iOS no longer builds a secure field's canvas as the shield needs")
        let host = ShieldHost(rootView: Text("secret").accessibilityIdentifier("inside"))
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 402, height: 874))
        window.rootViewController = host
        window.makeKeyAndVisible()
        host.view.layoutIfNeeded()
        let shield = host.shield
        XCTAssertTrue(shield.shielded)
        XCTAssertTrue(NSStringFromClass(type(of: shield.container)).hasSuffix("CanvasView"))
        XCTAssertTrue(host.hosting.view.isDescendant(of: shield.container), "the content is inside the canvas")
        XCTAssertNil(shield.container.superview.flatMap { $0 as? UITextField }, "the field itself never enters the window")
        XCTAssertEqual(host.hosting.view.frame.size, window.bounds.size, "the content fills the shield")
        XCTAssertTrue(shield.container.isUserInteractionEnabled, "touches reach the content")
        // let the mark happen (the next main-queue turn once attached), then read it the way only a test may: the
        // private CALayer property that tells the render server to leave the layer out of captures (never in the app)
        let marked = expectation(description: "marked")
        DispatchQueue.main.async { DispatchQueue.main.async { marked.fulfill() } }
        wait(for: [marked], timeout: 2)
        let key = "disableUpdateMask"
        if shield.container.layer.responds(to: NSSelectorFromString(key)) {
            let mask = (shield.container.layer.value(forKey: key) as? NSNumber)?.intValue ?? 0
            XCTAssertNotEqual(mask, 0, "the canvas's layer is not marked: captures would show the content")
        }
        window.isHidden = true
    }

    // decision: PHN-811
    func testWithoutACanvasTheContentIsStillShownInAPlainView() {
        // an iOS that builds the field differently: the content shows as before (the capture cover still hides it
        // while recorded or mirrored), and This phone says screenshots can show FERB (ScreenshotShield.available)
        let host = ShieldHost(rootView: Text("secret"), canvas: false)
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 402, height: 874))
        window.rootViewController = host
        window.makeKeyAndVisible()
        host.view.layoutIfNeeded()
        XCTAssertFalse(host.shield.shielded)
        XCTAssertFalse(NSStringFromClass(type(of: host.shield.container)).hasSuffix("CanvasView"))
        XCTAssertTrue(host.hosting.view.isDescendant(of: host.shield.container))
        XCTAssertEqual(host.hosting.view.frame.size, window.bounds.size)
        window.isHidden = true
    }
}
#endif
