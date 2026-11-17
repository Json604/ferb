#if canImport(UIKit)
import SwiftUI
import XCTest
@testable import FerbDesign

/// The first run's asks (kb PHN-636) over the sky at dawn, noon and night, with the app's words (Android's
/// PermissionsShotsTest, the same page).
@MainActor
final class PermissionsPageTests: XCTestCase {
    static let rows = [
        AskRow(symbol: "bell", title: "Notifications", why: "So FERB can tell you about its questions and finished tasks when you're outside the app."),
        AskRow(symbol: "location", title: "Your city", why: "So local times and places come out right. Only your city goes to your Mac; FERB asks before using your exact spot."),
        AskRow(symbol: "camera", title: "Camera", why: "To scan the code on your Mac when you pair. Nothing is recorded."),
        AskRow(symbol: "mic", title: "Microphone and speech", why: "So you can hold the microphone and speak to FERB. Nothing is recorded."),
    ]

    // decision: PHN-636
    func testThePage() {
        for (name, hour) in ScreenshotTests.times {
            let view = PhoneScreen(moment: .lab(hour), insets: .lab, glide: false) { EmptyView() } page: {
                PermissionsPage(title: "Before you start",
                                intro: "FERB asks your phone for a few things. You can say no to any of them: FERB still works, and says what is missing when you need it.",
                                rows: Self.rows, continueTitle: "Continue", notNowTitle: "Not now", insets: .lab)
            }
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
            .stillMotion()
            Snapshot.assert(view, size: PhoneInsets.labSize, path: "screens/first-run-\(name).png")
        }
    }
}
#endif
