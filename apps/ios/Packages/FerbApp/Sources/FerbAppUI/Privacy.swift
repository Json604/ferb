import FerbAppModel
import FerbDesign
import SwiftUI
import UIKit

/// What FERB does about screenshots on iPhone (kb PHN-810, PHN-811, PHN-598, PHN-15). Android's FLAG_SECURE blanks the
/// app in screenshots, screen recordings and the recent-apps list; iOS has no such public switch. What FERB does
/// instead, all from the owner's one Block screenshots switch, set once at the root (`ferbScreens`) and carried into
/// every page and every window FERB presents (`ferbFullScreenCover`, `ferbSheet`):
/// - the app switcher: whenever the scene is not active (the switcher, Control Center, a call, the background), a
///   cover hides everything, always, presented windows included (Android: recents blank on 13+ whatever the switch says);
/// - recording and mirroring: while the scene is captured (iOS 17's `isSceneCaptured`: a recording, AirPlay, QuickTime,
///   a remote control; not iPhone Mirroring, which iOS doesn't report) the content is hidden when the owner's switch is
///   on, and always while a private answer's box shows (PHN-110);
/// - screenshots: with the switch on, the pictures of the Mac (Watch live, a task's short view, the Mac's camera, a
///   session's window) are drawn inside a secure text field's canvas (ScreenshotShield.swift), which iOS leaves out of
///   screenshots; with it off they show like the rest of the app. Where this iOS doesn't build that canvas, a screenshot
///   shows them, and This phone says so. A private answer never shows its value (dots, or iOS's SecureField). The rest
///   of the app shows in a screenshot whatever the switch says: iOS has no way to blank it that keeps VoiceOver and
///   touches working (PHN-811).
struct PrivacyCover: ViewModifier {
    @Environment(\.scenePhase) private var phase
    @Environment(\.isSceneCaptured) private var sceneCaptured
    /// The owner's switch: hide while the screen is captured.
    let hideWhenCaptured: Bool
    /// A private answer's box on screen: hidden while captured whatever the switch says (PHN-110).
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

    /// The one place (kb PHN-811): the owner's switch and whether a private answer shows, for this view and everything
    /// under it: its privacy cover, every shielded picture (`screenshotShielded`) and every window presented from it
    /// through `ferbFullScreenCover` / `ferbSheet`. Set once, at the root.
    func ferbScreens(blockScreenshots: Bool, mustHide: Bool) -> some View {
        ferbPrivacy(hideWhenCaptured: blockScreenshots, mustHide: mustHide)
            .environment(\.ferbBlockScreenshots, blockScreenshots)
            .environment(\.ferbMustHide, mustHide)
    }

    /// A full-screen window FERB presents (a file's preview): the root's privacy cover and switch carried into it, since
    /// a presented window sits above the root's own cover (kb PHN-811, PHN-15).
    func ferbFullScreenCover<Item: Identifiable, Shown: View>(item: Binding<Item?>, @ViewBuilder content: @escaping (Item) -> Shown) -> some View {
        modifier(FerbPresenting(item: item, full: true, shown: content))
    }

    /// A sheet FERB presents (the share sheet): as `ferbFullScreenCover`.
    func ferbSheet<Item: Identifiable, Shown: View>(item: Binding<Item?>, @ViewBuilder content: @escaping (Item) -> Shown) -> some View {
        modifier(FerbPresenting(item: item, full: false, shown: content))
    }
}

/// Whether a private answer's box shows (PHN-110), as the root set it (`ferbScreens`).
private struct MustHideKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    var ferbMustHide: Bool {
        get { self[MustHideKey.self] }
        set { self[MustHideKey.self] = newValue }
    }
}

/// Presents `shown` as a full-screen cover or a sheet with the presenting place's switch and private state, and the
/// same privacy cover the root has (`presented`).
struct FerbPresenting<Item: Identifiable, Shown: View>: ViewModifier {
    @Binding var item: Item?
    let full: Bool
    let shown: (Item) -> Shown
    @Environment(\.ferbBlockScreenshots) private var block
    @Environment(\.ferbMustHide) private var mustHide

    init(item: Binding<Item?>, full: Bool, shown: @escaping (Item) -> Shown) {
        _item = item
        self.full = full
        self.shown = shown
    }

    func body(content: Content) -> some View {
        if full {
            content.fullScreenCover(item: $item) { Self.presented(shown($0), block: block, mustHide: mustHide) }
        } else {
            content.sheet(item: $item) { Self.presented(shown($0), block: block, mustHide: mustHide) }
        }
    }

    /// What a presented window draws: its content under the same switch, private state and cover as the root.
    static func presented<V: View>(_ v: V, block: Bool, mustHide: Bool, capturedForTest: Bool? = nil) -> some View {
        v.ferbPrivacy(hideWhenCaptured: block, mustHide: mustHide, capturedForTest: capturedForTest)
            .environment(\.ferbBlockScreenshots, block)
            .environment(\.ferbMustHide, mustHide)
    }
}
