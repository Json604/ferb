import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase G: after a new build's setup settles, the notch says "Updated to <version>" once; the menu offers an
/// update only while one waits for a click; Settings → General shows the switch and what the updater is doing.
@MainActor
final class UpdateWiringTests: XCTestCase {
    private func model() -> NotchModel {
        NotchModel(rpc: ScriptedRPC(), preferences: UserDefaults(suiteName: "ferb-update-\(UUID().uuidString)")!)
    }

    func testTheNotchSaysUpdatedToOncePerNewBuild() {
        let model = model()
        model.preferences.set("202610250900", forKey: NotchModel.lastBuildKey)
        let info: [String: Any] = ["CFBundleVersion": "202611010900", "CFBundleShortVersionString": "0.28.0"]
        model.noteUpdate(info: info)
        XCTAssertEqual(model.conversation.notices.map(\.text), ["Updated to 0.28.0"])
        XCTAssertEqual(model.conversation.notices.first?.tag, "FERB")
        model.noteUpdate(info: info)                                       // the same build again
        XCTAssertEqual(model.conversation.notices.count, 1)
        XCTAssertEqual(model.preferences.string(forKey: NotchModel.lastBuildKey), "202611010900")
    }

    func testAFirstLaunchSaysNothing() {
        let model = model()
        model.noteUpdate(info: ["CFBundleVersion": "202611010900", "CFBundleShortVersionString": "0.28.0"])
        XCTAssertTrue(model.conversation.notices.isEmpty)
        XCTAssertEqual(model.preferences.string(forKey: NotchModel.lastBuildKey), "202611010900")
    }

    func testTheMenuOffersAnUpdateOnlyWhileOneWaits() {
        func titles(_ offer: String?) -> [String] {
            StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: false,
                               hasWindow: true, updateOffer: offer).compactMap {
                if case .item(let title, _, _) = $0 { return title }
                return nil
            }
        }
        XCTAssertFalse(titles(nil).contains { $0.hasPrefix("Update available") })
        XCTAssertTrue(titles("0.28.0").contains("Update available (FERB 0.28.0) — Install"))
        XCTAssertTrue(StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: false,
                                         hasWindow: true, updateOffer: "0.28.0")
            .contains(.item("Update available (FERB 0.28.0) — Install", .installUpdate, key: "")))
    }

    func testTheUpdatesSectionLaysOut() {
        let updates = UpdateCoordinator(rpc: ScriptedRPC())
        updates.found(version: "0.28.0")
        let page = GeneralSettingsPage(feedback: SoundFeedback(), cursor: CursorController(makePanels: false),
                                       updates: updates)
        let hosting = NSHostingView(rootView: page.frame(width: 700, height: 500))
        hosting.frame = NSRect(x: 0, y: 0, width: 700, height: 500)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }
}
