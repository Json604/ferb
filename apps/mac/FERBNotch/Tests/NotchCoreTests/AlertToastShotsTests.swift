import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The alert toast drawn offscreen (APP-1109), collapsed and opened, as PNGs for the owner's review. By hand only:
/// runs when FERB_TOAST_SHOTS names a folder (never his screen).
@MainActor
final class AlertToastShotsTests: XCTestCase {
    // decision: APP-1109
    func testTheToastDrawsCollapsedAndOpen() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("ferb-toast-shots"))
        model.receiveNotices([
            ["id": "alert:set-aside-ferb.db", "text": "FERB couldn't read its database, so it moved the file aside and started a fresh one.", "alert": true],
            ["id": "alert:insomnia-warning-w1", "text": "Your Mac is getting hot. FERB keeps working, but macOS may slow it down, or put it to sleep if it gets too hot.", "alert": true],
        ])
        model.showOwnAlert(id: "update-0.28.1", text: "Updated to 0.28.1")
        func draw(_ name: String) throws -> NSSize {
            let scene = ZStack(alignment: .bottom) {
                Color.black
                VStack(alignment: .leading, spacing: 8) {
                    Text("Done. I moved fun_facts.txt to the Trash.").font(.system(size: 13)).foregroundStyle(.white)
                        .padding(10).background(.white.opacity(0.1), in: RoundedRectangle(cornerRadius: 14))
                    Spacer()
                }
                .padding(12).frame(maxWidth: .infinity, alignment: .leading)
                AlertToastView(model: model).padding(.horizontal, 12)
            }
            .frame(width: 440, height: model.alertsExpanded ? 300 : 200)
            .environment(\.colorScheme, .dark)
            let host = NSHostingView(rootView: scene)
            host.frame = CGRect(origin: .zero, size: host.fittingSize)
            host.layoutSubtreeIfNeeded()
            guard let dir = ProcessInfo.processInfo.environment["FERB_TOAST_SHOTS"], !dir.isEmpty else { return host.fittingSize }
            let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
            try XCTUnwrap(rep.representation(using: .png, properties: [:]))
                .write(to: URL(fileURLWithPath: dir).appendingPathComponent(name))
            return host.fittingSize
        }
        XCTAssertGreaterThan(try draw("mac-toast-collapsed.png").height, 0)
        model.alertsExpanded = true
        XCTAssertGreaterThan(try draw("mac-toast-expanded.png").height, 0)
    }
}
