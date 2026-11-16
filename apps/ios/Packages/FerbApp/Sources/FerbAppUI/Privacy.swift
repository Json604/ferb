import FerbAppModel
import FerbDesign
import SwiftUI
import UIKit

/// What iOS lets an app do about screenshots (kb PHN-565). Android's FLAG_SECURE blanks the app in screenshots,
/// screen recordings and the recent-apps list; iOS has no such switch for an app. What FERB does instead:
/// - the app switcher: whenever the scene is not active (the switcher, Control Center, a call, the background), a
///   cover hides everything, always (Android: recents blank on 13+ whatever the switch says);
/// - recording and mirroring: while the screen is captured (`UIScreen.isCaptured`: a recording, AirPlay, QuickTime),
///   the content is hidden when the owner's switch is on, and always for Watch live and a private answer (PHN-454,
///   PHN-110);
/// - a screenshot iOS takes at once; the app learns of it only after (`userDidTakeScreenshotNotification`) and can't
///   blank it. That is the platform's limit, for the owner to rule on.
/// The "hide it in a secure text field's layer" trick is not used: it relies on undocumented behaviour.
struct PrivacyCover: ViewModifier {
    @Environment(\.scenePhase) private var phase
    /// The owner's switch: hide while the screen is captured.
    let hideWhenCaptured: Bool
    /// Watch live or a private answer on screen: hidden while captured whatever the switch says.
    let mustHide: Bool
    /// Tests: the capture state as given, not as the screen says.
    var capturedForTest: Bool? = nil
    @State private var captured = Capture.now

    func body(content: Content) -> some View {
        let recording = (capturedForTest ?? captured) && (hideWhenCaptured || mustHide)
        content
            .overlay {
                if phase != .active || recording {
                    Cover(recording: recording && phase == .active)
                        .transition(.identity)
                        .accessibilityIdentifier("privacy-cover")
                }
            }
            .onReceive(NotificationCenter.default.publisher(for: UIScreen.capturedDidChangeNotification)) { _ in
                captured = Capture.now
            }
            .onAppear { captured = Capture.now }
    }

    private struct Cover: View {
        let recording: Bool
        var body: some View {
            ZStack {
                Color.black.ignoresSafeArea()
                VStack(spacing: 18) {
                    FerbMark(size: 84)
                    if recording {
                        Text(L.s("recording_cover")).type(15, .medium, .white).multilineTextAlignment(.center)
                            .padding(.horizontal, 40)
                    }
                }
            }
        }
    }
}

enum Capture {
    /// Whether any of the app's screens is being recorded or mirrored now.
    @MainActor static var now: Bool {
        UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.screen.isCaptured }.contains(true)
    }
}

extension View {
    func ferbPrivacy(hideWhenCaptured: Bool, mustHide: Bool, capturedForTest: Bool? = nil) -> some View {
        modifier(PrivacyCover(hideWhenCaptured: hideWhenCaptured, mustHide: mustHide, capturedForTest: capturedForTest))
    }
}
