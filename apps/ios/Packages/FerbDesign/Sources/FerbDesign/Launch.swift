import Observation
import SwiftUI

// The launch (owner kb PHN-630, PHN-700; PHN-631..634, PHN-701), the twin of Android's Launch.kt and LaunchHost.kt with
// the same numbers (motion.json `launch`, LaunchTests here, LaunchTest there): FERB's whole mark, the f on its dark tile
// with its amber dot in place, at the screen's centre over ink0 from the launch screen's first frame (App/Assets
// LaunchMark); nothing moves in or lands (PHN-700), and while the app loads the dot breathes. The moment the page under it is
// ready the mark settles into the lock page's mark (a shared element, `launchTarget()`) while ink0 lifts off the sky.
// Never longer than the loading: ready at the first frame, it settles at once.

/// The launch's motion: one pure function of time, the same as Android's.
public enum FerbLaunch {
    /// The mark's side at the screen's centre (points): the launch screen's image.
    public static let size: CGFloat = 112
    public static let breathAfterMs = 900.0, breathPeriodMs = 1800.0, breathScale = 0.86, breathAlpha = 0.72
    /// The settle: the mark flies to the page's mark on the standard curve (it starts from rest).
    public static let settle = FerbMotion.Curve(ms: 450, bezier: (0.4, 0, 0.2, 1))
    /// The system prompt may come this long after the settle began (the mark has done 94 % of its way).
    public static let settlePromptMs = 315.0
    public static let reducedMs = 200.0
    /// iOS crossfades its launch screen into the app's first frame: the settle starts after it (motion.json
    /// ios_launch_screen_ms), so the launch screen's still mark never shows beside the moving one.
    public static let launchScreenMs = 250.0

    public enum Phase: Sendable { case arriving, breathing, settling, done }

    /// What the launch draws at one moment. Lengths are fractions of the mark's side; `handover` 0...1.
    public struct Frame: Equatable, Sendable {
        public var dotY: Double, dotAlpha: Double, dotScale: Double, fScale: Double, handover: Double
        public var phase: Phase
        public var promptOk: Bool
    }

    static let rest = Frame(dotY: 0, dotAlpha: 1, dotScale: 1, fScale: 1, handover: 1, phase: .done, promptOk: true)

    /// The launch at `t` ms after its start; `readyAt`: when the page under it became ready (nil: not yet).
    public static func at(_ t: Double, readyAt: Double?, reduced: Bool) -> Frame {
        guard let r = readyAt, t >= r else { return free(t, reduced: reduced) }
        let from = free(r, reduced: reduced)
        let ms = reduced ? reducedMs : Double(settle.ms)
        let u = min(max((t - r) / ms, 0), 1)
        if u >= 1 { return rest }
        let e = reduced ? u : bezierY(settle.bezier, u)
        func lerp(_ a: Double, _ b: Double) -> Double { a + (b - a) * e }
        return Frame(dotY: lerp(from.dotY, 0), dotAlpha: lerp(from.dotAlpha, 1), dotScale: lerp(from.dotScale, 1),
                     fScale: lerp(from.fScale, 1), handover: e, phase: .settling,
                     promptOk: t - r >= (reduced ? reducedMs : settlePromptMs))
    }

    /// Before ready: the whole mark at rest, then the dot breathes. Reduced: the mark at rest, still.
    public static func free(_ t: Double, reduced: Bool) -> Frame {
        if reduced { return Frame(dotY: 0, dotAlpha: 1, dotScale: 1, fScale: 1, handover: 0, phase: .arriving, promptOk: false) }
        let k = t < breathAfterMs ? 0 : (1 - cos(2 * .pi * (t - breathAfterMs) / breathPeriodMs)) / 2
        return Frame(dotY: 0, dotAlpha: 1 - (1 - breathAlpha) * k, dotScale: 1 - (1 - breathScale) * k, fScale: 1,
                     handover: 0, phase: t < breathAfterMs ? .arriving : .breathing, promptOk: false)
    }

    /// y at x on the cubic Bézier (0,0) (x1,y1) (x2,y2) (1,1), as Compose's CubicBezierEasing solves it.
    static func bezierY(_ b: (Double, Double, Double, Double), _ x: Double) -> Double {
        var lo = 0.0, hi = 1.0
        for _ in 0..<40 {
            let s = (lo + hi) / 2
            let bx = 3 * (1 - s) * (1 - s) * s * b.0 + 3 * (1 - s) * s * s * b.2 + s * s * s
            if bx < x { lo = s } else { hi = s }
        }
        let s = (lo + hi) / 2
        return 3 * (1 - s) * (1 - s) * s * b.1 + 3 * (1 - s) * s * s * b.3 + s * s * s
    }
}

/// The launch over time: when it began, when the page became ready, whether motion is reduced, and its two moments
/// (the prompt may come; it ended). The time is passed in, so the same state runs in the app, the tests and previews.
@MainActor
@Observable
public final class LaunchState {
    public let reduced: Bool
    @ObservationIgnored private var start: Date?
    public private(set) var readyAt: Double?
    /// The mark has settled enough for the system prompt (PHN-633).
    public private(set) var promptOk = false
    /// The launch has ended: nothing of it is drawn and the page's own mark shows.
    public private(set) var done = false
    @ObservationIgnored public var onPromptOk: (() -> Void)?

    public init(reduced: Bool) { self.reduced = reduced }

    /// Ms since the launch's first frame (that frame is 0).
    public func elapsed(_ now: Date) -> Double {
        let s = start ?? now
        if start == nil { start = now }
        return now.timeIntervalSince(s) * 1000
    }

    public func frame(_ now: Date) -> FerbLaunch.Frame { FerbLaunch.at(elapsed(now), readyAt: readyAt, reduced: reduced) }

    /// The page under it is ready (the first call counts): the settle starts, and its two moments follow.
    public func ready(_ now: Date = Date(), sleep: @escaping @Sendable (Double) async -> Void = { try? await Task.sleep(for: .milliseconds(Int($0))) }) {
        guard readyAt == nil else { return }
        let now = elapsed(now)
        let r = max(now, FerbLaunch.launchScreenMs)
        readyAt = r
        let prompt = reduced ? FerbLaunch.reducedMs : FerbLaunch.settlePromptMs
        let end = reduced ? FerbLaunch.reducedMs : Double(FerbLaunch.settle.ms)
        Task { @MainActor in
            await sleep(r - now + prompt)
            self.settledForPrompt()
            await sleep(end - prompt)
            self.done = true
        }
    }

    func settledForPrompt() {
        guard !promptOk else { return }
        promptOk = true
        onPromptOk?()
    }
}

/// Where the page's mark is, for the launch to settle into.
public struct LaunchTargetKey: PreferenceKey {
    public static let defaultValue: Anchor<CGRect>? = nil
    public static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) { value = value ?? nextValue() }
}

private struct LaunchHidesTargetKey: EnvironmentKey { static let defaultValue = false }

public extension EnvironmentValues {
    /// A launch is flying to the page's mark: the mark stays hidden until it lands.
    var launchHidesTarget: Bool {
        get { self[LaunchHidesTargetKey.self] }
        set { self[LaunchHidesTargetKey.self] = newValue }
    }
}

private struct LaunchTarget: ViewModifier {
    @Environment(\.launchHidesTarget) private var hides
    func body(content: Content) -> some View {
        content.opacity(hides ? 0 : 1).anchorPreference(key: LaunchTargetKey.self, value: .bounds) { $0 }
    }
}

public extension View {
    /// This is the page's mark the launch settles into (the shared element).
    func launchTarget() -> some View { modifier(LaunchTarget()) }

    /// The launch over this view: ink0 and the mark from the first frame, settling into the `launchTarget()` inside
    /// once `ready`. Nothing is drawn once it is done.
    func launchOverlay(_ state: LaunchState?, ready: Bool) -> some View {
        modifier(LaunchHost(state: state, ready: ready))
    }
}

private struct LaunchHost: ViewModifier {
    let state: LaunchState?
    let ready: Bool

    func body(content: Content) -> some View {
        if let state {
            content
                .environment(\.launchHidesTarget, !state.done && !state.reduced)
                .overlayPreferenceValue(LaunchTargetKey.self) { anchor in
                    GeometryReader { proxy in
                        if !state.done {
                            LaunchOverlay(state: state, target: anchor.map { proxy[$0] })
                        }
                    }
                    .ignoresSafeArea()
                }
                .onAppear { if ready { state.ready() } }
                .onChange(of: ready) { _, r in if r { state.ready() } }
        } else {
            content
        }
    }
}

/// The launch's picture at one moment: ink0 and the mark, moved by layer transforms only.
public struct LaunchOverlay: View {
    let state: LaunchState
    /// The page's mark in this overlay's own coordinates (nil: the page has none; the mark then fades where it is).
    let target: CGRect?
    var ground = true
    /// A fixed moment (ms) instead of the frame clock, with the page ready at `readyAt` (nil: not yet): the screenshots.
    var at: Double?
    var readyAt: Double?

    public init(state: LaunchState, target: CGRect?, ground: Bool = true, at: Double? = nil, readyAt: Double? = nil) {
        self.state = state
        self.target = target
        self.readyAt = readyAt
        self.ground = ground
        self.at = at
    }

    public var body: some View {
        if let at {
            picture(FerbLaunch.at(at, readyAt: readyAt, reduced: state.reduced))
        } else {
            TimelineView(.animation) { context in picture(state.frame(context.date)) }
        }
    }

    private func picture(_ f: FerbLaunch.Frame) -> some View {
        GeometryReader { geo in
            let s = FerbLaunch.size
            let home = CGRect(x: (geo.size.width - s) / 2, y: (geo.size.height - s) / 2, width: s, height: s)
            let to = target
            let h = state.reduced ? 0 : f.handover
            let at = to.map { t in CGRect(x: home.minX + (t.minX - home.minX) * h, y: home.minY + (t.minY - home.minY) * h,
                                          width: s + (t.width - s) * h, height: s + (t.width - s) * h) } ?? home
            ZStack(alignment: .topLeading) {
                if ground { Tone.ink0.opacity(1 - f.handover) }
                LaunchMark(dotY: f.dotY, dotAlpha: f.dotAlpha, dotScale: f.dotScale, fScale: f.fScale)
                    .frame(width: s, height: s)
                    .scaleEffect(at.width / s, anchor: .topLeading)
                    .offset(x: at.minX, y: at.minY)
                    .opacity(to == nil || state.reduced ? 1 - f.handover : 1)
            }
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// FerbMark at the launch's size, its f and dot each moved on their own (the f from its foot, the dot from its centre).
struct LaunchMark: View {
    var dotY: Double, dotAlpha: Double, dotScale: Double, fScale: Double

    var body: some View {
        let size = FerbLaunch.size
        ZStack {
            FerbMark.Tile(size: size)
            FerbMark.Letter().fill(Tone.text)
                .scaleEffect(x: 1, y: fScale, anchor: UnitPoint(x: ((49.969 + 60.477) / 2 - 16) / 96, y: (90 - 16) / 96))
            LampDot(color: Lamp.amber, size: size / 6)
                .scaleEffect(dotScale)
                .opacity(dotAlpha)
                .position(x: size * (82.0 - 16) / 96, y: size * (85.0 - 16) / 96 + dotY * size)
        }
        .frame(width: size, height: size)
    }
}
