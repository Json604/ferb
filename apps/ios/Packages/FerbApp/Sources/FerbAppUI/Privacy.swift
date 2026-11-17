import FerbAppModel
import FerbDesign
import SwiftUI
import UIKit

/// What FERB does about screenshots on iPhone (kb PHN-565, PHN-597, PHN-598). Android's FLAG_SECURE blanks the app in
/// screenshots, screen recordings and the recent-apps list; iOS has no such public switch. What FERB does instead:
/// - the app switcher: whenever the scene is not active (the switcher, Control Center, a call, the background), a
///   cover hides everything, always (Android: recents blank on 13+ whatever the switch says);
/// - recording and mirroring: while the scene is captured (iOS 17's `isSceneCaptured`: a recording, AirPlay, QuickTime,
///   a remote control; not iPhone Mirroring, which iOS doesn't report) the content is hidden when the owner's switch is
///   on, and always for Watch live and a private answer (PHN-454, PHN-110);
/// - screenshots: Watch live's picture of the Mac is drawn inside a secure text field's canvas (ScreenshotShield.swift),
///   which iOS leaves out of screenshots; where this iOS doesn't build that canvas, a screenshot shows it, and This
///   phone says so. A private answer never shows its value (dots, or iOS's SecureField). The rest of the app shows in
///   a screenshot (PHN-597).
struct PrivacyCover: ViewModifier {
    @Environment(\.scenePhase) private var phase
    @Environment(\.isSceneCaptured) private var sceneCaptured
    /// The owner's switch: hide while the screen is captured.
    let hideWhenCaptured: Bool
    /// Watch live or a private answer on screen: hidden while captured whatever the switch says.
    let mustHide: Bool
    /// Tests: the capture state as given, not as the screen says.
    var capturedForTest: Bool? = nil

    func body(content: Content) -> some View {
        let recording = (capturedForTest ?? sceneCaptured) && (hideWhenCaptured || mustHide)
        content
            .overlay {
                if phase != .active || recording {
                    Cover(recording: recording && phase == .active)
                        .transition(.identity)
                        .accessibilityIdentifier("privacy-cover")
                }
            }
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

extension View {
    func ferbPrivacy(hideWhenCaptured: Bool, mustHide: Bool, capturedForTest: Bool? = nil) -> some View {
        modifier(PrivacyCover(hideWhenCaptured: hideWhenCaptured, mustHide: mustHide, capturedForTest: capturedForTest))
    }
}
