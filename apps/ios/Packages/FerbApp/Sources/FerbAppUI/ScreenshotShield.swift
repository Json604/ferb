import FerbAppModel
import SwiftUI
import UIKit

/// Keeping the pictures of the Mac out of screenshots on iPhone while the owner's Block screenshots switch is on (kb
/// PHN-811, PHN-810, PHN-598; the owner's T-1240): Watch live, a task's short view, the Mac's camera, a session's window.
/// With the switch off they show in screenshots like every other page (owner 2026-11-23, PHN-810). A
/// private answer never shows its value (dots for a code; iOS's SecureField, which iOS itself keeps out of screenshots,
/// for a password), so it needs no shield; it is hidden while the screen is recorded or mirrored (Privacy.swift).
///
/// iOS has no public switch like Android's FLAG_SECURE. What banking apps do: a `UITextField` with `isSecureTextEntry`
/// draws its text in an internal canvas view whose layer UIKit marks so the render server leaves it, and everything
/// inside it, out of screenshots, screen recordings and mirroring. FERB takes that canvas out of a field that never
/// enters the window and hosts its own content inside it (the libraries' "move the canvas" pattern; never the riskier
/// one that re-parents the window's layer, which crashed apps on iOS 26). It uses only public calls; it relies on how
/// UIKit builds a secure field, which Apple documents only as "SecureField hides its dots in screenshots" and says may
/// change in any release.
///
/// Fail safe: when the canvas can't be found (a later iOS builds the field differently) the content is hosted in a
/// plain view, `ScreenshotShield.available` is false, and FERB is back to what it did before: the cover in the app
/// switcher and hidden while the screen is recorded or mirrored (Privacy.swift). The app says which (This phone).
/// The simulator marks the layer but does not honour the mark in its own screenshots; only an iPhone shows it working.
public enum ScreenshotShield {
    /// The canvas class names UIKit has used: iOS 15 through 26 `_UITextLayoutCanvasView`, iOS 13-14
    /// `_UITextFieldCanvasView`; any other `…CanvasView` subview is taken as a later name.
    static let knownCanvases: Set<String> = ["_UITextLayoutCanvasView", "_UITextFieldCanvasView"]

    /// Whether this iOS builds a secure field the way the shield needs (probed once, on the main thread).
    @MainActor public static let available: Bool = SecureCanvas.make() != nil
}

/// A secure field that can never take focus, show a keyboard or be read by VoiceOver: it only lends its canvas.
private final class InertSecureField: UITextField {
    override var canBecomeFirstResponder: Bool { false }
    override var canBecomeFocused: Bool { false }
}

/// The secure field's canvas, taken out of the field (which stays out of every window), and the field that marks it.
@MainActor
struct SecureCanvas {
    let field: UITextField
    let canvas: UIView

    static func make() -> SecureCanvas? {
        let field = InertSecureField()
        field.isAccessibilityElement = false
        field.isSecureTextEntry = true
        field.layoutIfNeeded()
        let name = { (v: UIView) in NSStringFromClass(type(of: v)) }
        guard let canvas = field.subviews.first(where: { ScreenshotShield.knownCanvases.contains(name($0)) })
            ?? field.subviews.first(where: { name($0).hasSuffix("CanvasView") }) else { return nil }
        canvas.isUserInteractionEnabled = true
        return SecureCanvas(field: field, canvas: canvas)
    }

    /// The false-to-true change of `isSecureTextEntry` is what marks the canvas's layer; done again once the canvas is
    /// in a window (an earlier mark can be lost when the field is set up before it is attached).
    func mark() {
        field.isSecureTextEntry = false
        field.isSecureTextEntry = true
    }
}

/// The view that holds shielded content: the secure canvas pinned to its edges, or a plain view when there is none.
final class ShieldView: UIView {
    let container: UIView
    let shielded: Bool
    private let secure: SecureCanvas?

    /// `secure`: the canvas to host in (nil: a plain view, as on an iOS without one; tests pass nil to see that).
    @MainActor init(secure: SecureCanvas?) {
        self.secure = secure
        shielded = secure != nil
        container = secure?.canvas ?? UIView()
        super.init(frame: .zero)
        backgroundColor = .clear
        container.backgroundColor = .clear
        container.translatesAutoresizingMaskIntoConstraints = false
        addSubview(container)
        NSLayoutConstraint.activate([
            container.topAnchor.constraint(equalTo: topAnchor),
            container.bottomAnchor.constraint(equalTo: bottomAnchor),
            container.leadingAnchor.constraint(equalTo: leadingAnchor),
            container.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    required init?(coder: NSCoder) { nil }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard window != nil, let secure else { return }
        // on the next turn, once attached (setting it during set-up has been seen to leave the layer unmarked)
        DispatchQueue.main.async { secure.mark() }
    }
}

/// Hosts SwiftUI content inside a `ShieldView` (the environment of the place it is used carries over).
final class ShieldHost<Content: View>: UIViewController {
    let hosting: UIHostingController<Content>
    let shield: ShieldView

    /// `canvas`: false hosts in a plain view, as on an iOS without the secure canvas (tests).
    init(rootView: Content, canvas: Bool = true) {
        hosting = UIHostingController(rootView: rootView)
        shield = ShieldView(secure: canvas ? SecureCanvas.make() : nil)
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { nil }

    override func loadView() { view = shield }

    override func viewDidLoad() {
        super.viewDidLoad()
        addChild(hosting)
        hosting.view.backgroundColor = .clear
        // the content takes its place from FERB's own layout, as every page does
        hosting.safeAreaRegions = []
        hosting.view.translatesAutoresizingMaskIntoConstraints = false
        shield.container.addSubview(hosting.view)
        NSLayoutConstraint.activate([
            hosting.view.topAnchor.constraint(equalTo: shield.container.topAnchor),
            hosting.view.bottomAnchor.constraint(equalTo: shield.container.bottomAnchor),
            hosting.view.leadingAnchor.constraint(equalTo: shield.container.leadingAnchor),
            hosting.view.trailingAnchor.constraint(equalTo: shield.container.trailingAnchor),
        ])
        hosting.didMove(toParent: self)
    }
}

/// `content` drawn inside the screenshot shield, filling the space it is given.
struct ScreenshotShielded<Content: View>: UIViewControllerRepresentable {
    let content: Content

    init(@ViewBuilder content: () -> Content) { self.content = content() }

    func makeUIViewController(context: Context) -> ShieldHost<AnyView> {
        ShieldHost(rootView: AnyView(content.environment(\.self, context.environment)))
    }

    func updateUIViewController(_ host: ShieldHost<AnyView>, context: Context) {
        host.hosting.rootView = AnyView(content.environment(\.self, context.environment))
    }
}

/// The owner's Block screenshots switch as every shielded picture reads it (kb PHN-810, PHN-811): set once at the root
/// (RootView, from the model's settings), carried by SwiftUI into every page, sheet and presented window under it. It is
/// on where nothing set it (fail safe: the shield's own check page, a view drawn on its own).
private struct BlockScreenshotsKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    public var ferbBlockScreenshots: Bool {
        get { self[BlockScreenshotsKey.self] }
        set { self[BlockScreenshotsKey.self] = newValue }
    }
}

/// `content` in the shield while the owner's switch is on, drawn plainly while it is off (PHN-810, PHN-811).
struct ShieldedBySwitch<Content: View>: View {
    let content: Content
    @Environment(\.ferbBlockScreenshots) private var block
    var body: some View {
        if block { ScreenshotShielded { content } } else { content }
    }
}

extension View {
    /// Draws this view inside the screenshot shield while the owner's switch is on (the one place: the environment's
    /// `ferbBlockScreenshots`), plainly while it is off. Only for a picture that nothing reads out or taps (a picture
    /// of the Mac, PHN-454): SwiftUI content hosted in there is not reached by VoiceOver (seen on the simulator: the UI
    /// tests' accessibility tree stops at the hosting view, with or without the secure canvas), so controls stay outside.
    func screenshotShielded() -> some View {
        ShieldedBySwitch(content: self)
    }
}
