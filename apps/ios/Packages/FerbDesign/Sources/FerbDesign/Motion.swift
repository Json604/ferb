import SwiftUI

// Not from the lab: one clock for the motion nobody asked for (the running mark, the typing dots, the season's
// layer), so a picture can be still. The lab runs them on TimelineView(.animation) and draws the season's layer at
// t = 1000 when animations are off (Weather.swift); here the same still moment is used under Reduce Motion, when a
// view asks for it (`stillMotion()`, the screenshots and previews), and the numbers inside each piece are the lab's.

/// The lab's still moment, in seconds: what Weather.swift draws when animations are off.
public enum MotionStill {
    public static let time = 1000.0
}

private struct StillMotionKey: EnvironmentKey { static let defaultValue: Double? = nil }

public extension EnvironmentValues {
    /// When set, every moving piece is drawn at this moment (seconds) and does not move.
    var stillMotion: Double? {
        get { self[StillMotionKey.self] }
        set { self[StillMotionKey.self] = newValue }
    }
}

public extension View {
    /// Draws every moving piece inside at one fixed moment.
    func stillMotion(at time: Double = MotionStill.time) -> some View { environment(\.stillMotion, time) }
}

/// Gives its content the time in seconds: the frame clock, or the still moment.
struct MotionClock<Content: View>: View {
    @Environment(\.stillMotion) private var still
    @Environment(\.accessibilityReduceMotion) private var reduce
    @ViewBuilder var content: (Double) -> Content

    var body: some View {
        if let still {
            content(still)
        } else if reduce {
            content(MotionStill.time)
        } else {
            TimelineView(.animation) { context in content(context.date.timeIntervalSinceReferenceDate) }
        }
    }
}

// Not from the lab: its still screens never hold more than fits. A list longer than the screen scrolls; one that
// fits is drawn as the lab draws it, top-aligned with no scroll view (so a picture made with ImageRenderer, which
// leaves scroll views out, shows it too).
struct FitOrScroll<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        ViewThatFits(in: .vertical) {
            content.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            ScrollView { content }.scrollBounceBehavior(.basedOnSize)
        }
    }
}

// MARK: FERB's one motion system (kb PHN-621..624; the owner wants PairIt's motion, PHN-620)

// The iPhone twin of Android's Motion.kt and MotionHosts.kt, with the same numbers: one table for both phones,
// apps/android/design/src/test/resources/motion/motion.json (MotionTests here, MotionTableTest there). Every screen
// moves through these, never with numbers of its own. Only opacity and offsets (no layout per frame), never a blur.

/// The numbers every screen moves with.
public enum FerbMotion {
    /// A spring by its response (one undamped swing, seconds) and damping ratio: Compose's spring(dampingRatio,
    /// stiffness = (2 pi / response)^2) is the same equation.
    public struct Spring: Sendable {
        public let response: Double
        public let damping: Double
        public var animation: Animation { .spring(response: response, dampingFraction: damping, blendDuration: 0) }
    }

    /// A timed curve: its length and cubic Bézier.
    public struct Curve: Sendable {
        public let ms: Int
        public let bezier: (Double, Double, Double, Double)
        public var seconds: Double { Double(ms) / 1000 }
        public var animation: Animation {
            bezier == (0, 0, 1, 1) ? .linear(duration: seconds)
                : .timingCurve(bezier.0, bezier.1, bezier.2, bezier.3, duration: seconds)
        }
    }

    /// The tab pill: PairIt's 650 ms curve 1 - e^(-7.5t)(cos 10t + 0.75 sin 10t), which is this spring's step.
    public static let tab = Spring(response: 0.326726, damping: 0.6)
    /// A press at touch-down and its release: the same curve over 220 ms and 550 ms (PairIt's GlassUI.pressFeedback).
    public static let pressDown = Spring(response: 0.110584, damping: 0.6)
    public static let pressUp = Spring(response: 0.276460, damping: 0.6)
    /// A sheet up from below and back down (the lab's panel: 0.30 s, bounce 0.08, interruptible).
    public static let sheet = Spring(response: 0.30, damping: 0.92)

    /// What comes in: 24 points and a fade over 220 ms on (.22, 1, .36, 1) (PairIt's panel).
    public static let enter = Curve(ms: 220, bezier: (0.22, 1, 0.36, 1))
    public static let enterShift: CGFloat = 24
    /// What goes: a 120 ms linear fade.
    public static let exit = Curve(ms: 120, bezier: (0, 0, 1, 1))
    /// A list item arriving: up 6 points and in over 180 ms on (.23, 1, .32, 1) (PairIt's notice).
    public static let item = Curve(ms: 180, bezier: (0.23, 1, 0.32, 1))
    public static let itemShift: CGFloat = 6
    /// Under Reduce Motion nothing moves; a change is this linear fade (PairIt's RelayMotion.navigation).
    public static let reduced = Curve(ms: 120, bezier: (0, 0, 1, 1))

    /// A press sinks to this and dims to this (the lab's FerbButton).
    public static let pressScale: CGFloat = 0.97
    public static let pressAlpha: Double = 0.85
    /// The dimmed page under a sheet (Mobile.swift Sheet).
    public static let scrim: Double = 0.5
    /// A dismissible sheet let go past this share of its height, or flung down this fast (points a second), closes.
    public static let dismissFraction: CGFloat = 0.25
    public static let dismissVelocity: CGFloat = 900

    /// How far something coming in still is from its place, at `value` (0 to 1) of its coming: none while it goes,
    /// and none at all under Reduce Motion (then it only fades).
    public static func enterOffset(_ value: Double, leaving: Bool, reduce: Bool) -> CGFloat {
        (leaving || reduce) ? 0 : CGFloat(1 - value) * enterShift
    }
}

/// Where something shown by a `MotionSwitch` is: 0 gone, 1 shown; `leaving` while it goes.
public struct MotionPresence: Equatable, Sendable {
    public var value: Double
    public var leaving: Bool
}

private struct MotionPresenceKey: EnvironmentKey { static let defaultValue: MotionPresence? = nil }
private struct TabBarHostedKey: EnvironmentKey { static let defaultValue = false }

public extension EnvironmentValues {
    /// The presence of the `MotionSwitch` entry this is inside (nil: drawn at rest, as in the screenshots).
    var motionPresence: MotionPresence? {
        get { self[MotionPresenceKey.self] }
        set { self[MotionPresenceKey.self] = newValue }
    }

    /// Inside a `TabBarHost`: a page's own TopBar keeps its room and draws nothing; the host draws the one bar.
    var tabBarHosted: Bool {
        get { self[TabBarHostedKey.self] }
        set { self[TabBarHostedKey.self] = newValue }
    }
}

/// How a `MotionSwitch` moves its entries (Android's MotionKind).
public enum MotionKind: Sendable {
    /// A page: rises 24 points and fades in; fades out.
    case screen
    /// A tab's page: slides 24 points from its tab's side and fades in; fades out.
    case tab
    /// A sheet: its dim and panel move (Sheet), on the sheet spring both ways.
    case sheet
}

/// The entry's look at a moment of its coming or going, interpolated frame by frame.
struct PresenceEffect: ViewModifier, Animatable {
    var value: Double
    let leaving: Bool
    let kind: MotionKind
    let direction: CGFloat
    let still: Bool

    var animatableData: Double {
        get { value }
        set { value = newValue }
    }

    func body(content: Content) -> some View {
        let v = min(1, max(0, value))
        let shift = FerbMotion.enterOffset(value, leaving: leaving, reduce: still)
        switch kind {
        case .sheet:
            content.environment(\.motionPresence, MotionPresence(value: value, leaving: leaving))
        case .screen:
            content.opacity(v).offset(y: shift)
        case .tab:
            content.opacity(v).offset(x: shift * direction)
        }
    }
}

extension AnyTransition {
    /// One kind's coming and going; under Reduce Motion a plain fade. The switch changes its key in an animated
    /// transaction (the enter curve, or the sheet spring); the going side is quicker on its own curve.
    static func motion(_ kind: MotionKind, direction: CGFloat, reduce: Bool) -> AnyTransition {
        if reduce { return .opacity }
        switch kind {
        case .sheet:
            return .modifier(active: PresenceEffect(value: 0, leaving: false, kind: .sheet, direction: 0, still: false),
                             identity: PresenceEffect(value: 1, leaving: false, kind: .sheet, direction: 0, still: false))
        case .screen, .tab:
            let shift = AnyTransition.offset(x: kind == .tab ? FerbMotion.enterShift * direction : 0,
                                             y: kind == .screen ? FerbMotion.enterShift : 0)
            return .asymmetric(insertion: shift.combined(with: .opacity),
                               removal: AnyTransition.opacity.animation(FerbMotion.exit.animation))
        }
    }

    /// A row of a list that arrives: up 6 points and in; one removed fades out.
    public static var motionItem: AnyTransition {
        .asymmetric(insertion: .modifier(active: ItemEffect(value: 0), identity: ItemEffect(value: 1)).animation(FerbMotion.item.animation),
                    removal: AnyTransition.opacity.animation(FerbMotion.exit.animation))
    }
}

struct ItemEffect: ViewModifier, Animatable {
    var value: Double
    var animatableData: Double {
        get { value }
        set { value = newValue }
    }
    func body(content: Content) -> some View {
        content.opacity(value).offset(y: (1 - value) * FerbMotion.itemShift)
    }
}

/// Shows `key` (nothing for nil) and moves between keys as `kind` says: what comes in rises (or slides from its tab's
/// side) and fades in, what goes fades out and takes no touch (SwiftUI's), the new one takes touches at once. An
/// `instant` key replaces everything in the same frame (the lock: nothing of the page before may show), and the page
/// after it comes in as any other ("the lock opening into the app"). A going entry keeps what it last drew.
public struct MotionSwitch<Key: Hashable, Content: View>: View {
    let key: Key?
    let kind: MotionKind
    let instant: (Key) -> Bool
    let direction: (Key, Key) -> Int
    @ViewBuilder let content: (Key) -> Content
    /// What is shown: the key, set in one animated transaction with the side it comes from (so the page that comes in
    /// keeps its direction while it moves). An instant key is drawn at once, without waiting for this.
    @State private var shown: Key?
    @State private var dir: CGFloat = 0
    @State private var started = false
    @Environment(\.accessibilityReduceMotion) private var reduce

    public init(_ key: Key?, kind: MotionKind = .screen, instant: @escaping (Key) -> Bool = { _ in false },
                direction: @escaping (Key, Key) -> Int = { _, _ in 0 }, @ViewBuilder content: @escaping (Key) -> Content) {
        self.key = key
        self.kind = kind
        self.instant = instant
        self.direction = direction
        self.content = content
    }

    private var animation: Animation {
        reduce ? FerbMotion.reduced.animation : kind == .sheet ? FerbMotion.sheet.animation : FerbMotion.enter.animation
    }

    public var body: some View {
        if kind == .sheet {
            SheetPresenter(key: key, content: content)
        } else {
            pages
        }
    }

    @ViewBuilder private var pages: some View {
        let current: Key? = {
            if let key, instant(key) { return key }  // the lock: in this very frame
            return started ? shown : key
        }()
        ZStack {
            if let current, instant(current) {
                // nothing comes in with it and nothing of the page before stays; it fades as the next page comes
                content(current).id(current)
                    .transition(.asymmetric(insertion: .identity,
                                            removal: AnyTransition.opacity.animation(reduce ? FerbMotion.reduced.animation : FerbMotion.exit.animation)))
            } else {
                ZStack {
                    if let current { content(current).id(current).transition(.motion(kind, direction: dir, reduce: reduce)) }
                }
                .transition(.asymmetric(insertion: .motion(kind, direction: 0, reduce: reduce), removal: .identity))
            }
        }
        // into an instant key nothing animates: the page before is gone in the same frame
        .transaction { t in
            if let key, instant(key) { t.animation = nil }
        }
        .onChange(of: key, initial: true) { _, new in
            guard started else {
                started = true
                shown = new
                return
            }
            if let new, instant(new) {
                var t = Transaction(animation: nil)
                t.disablesAnimations = true
                withTransaction(t) { shown = new }
                return
            }
            let d: CGFloat = {
                guard let from = shown, let to = new, from != to else { return 0 }
                return CGFloat(direction(from, to))
            }()
            withAnimation(animation) {
                dir = d
                shown = new
            }
        }
    }
}

/// A sheet's coming and going (MotionSwitch's .sheet): its presence (0 to 1) is state animated on the sheet spring and
/// handed to the Sheet, which fades its dim and moves its panel by it; a going sheet is kept, as it last drew, until the
/// spring has it down, and takes no touch, so the page under it takes taps at once. Coming back mid-way reverses it.
struct SheetPresenter<Key: Hashable, Content: View>: View {
    let key: Key?
    let content: (Key) -> Content
    @State private var drawn: Key?
    @State private var progress: Double = 0
    @State private var leaving = false
    @State private var started = false
    @State private var generation = 0
    @Environment(\.accessibilityReduceMotion) private var reduce

    private var animation: Animation { reduce ? FerbMotion.reduced.animation : FerbMotion.sheet.animation }

    var body: some View {
        // before the first change (and in a still picture) the key is drawn at rest
        let shown = started ? drawn : key
        ZStack {
            if let shown {
                content(shown)
                    .id(shown)
                    .modifier(PresenceEffect(value: started ? progress : 1, leaving: leaving, kind: .sheet, direction: 0, still: false))
                    .allowsHitTesting(!leaving)
            }
        }
        .onChange(of: key, initial: true) { _, new in
            guard started else {
                started = true
                drawn = new
                progress = new == nil ? 0 : 1
                return
            }
            generation += 1
            let mine = generation
            if let new {
                if new != drawn || leaving == false && progress == 0 {
                    // another sheet, or one after none: it comes up from below
                    var still = Transaction(animation: nil)
                    still.disablesAnimations = true
                    withTransaction(still) {
                        drawn = new
                        leaving = false
                        progress = 0
                    }
                    Task { @MainActor in
                        guard generation == mine else { return }
                        withAnimation(animation) { progress = 1 }
                    }
                } else {
                    // the same one, called back on its way down
                    leaving = false
                    withAnimation(animation) { progress = 1 }
                }
            } else if drawn != nil {
                leaving = true
                withAnimation(animation, completionCriteria: .removed) {
                    progress = 0
                } completion: {
                    if generation == mine { drawn = nil; leaving = false }
                }
            }
        }
    }
}

/// One tab bar that stays while the pages under it change (kb PHN-622): each page's own TopBar keeps its room and draws
/// nothing; this draws the bar once over them, so its white pill slides from tab to tab.
public struct TabBarHost<Content: View>: View {
    let chosen: PhoneTab
    let waiting: PhoneTab?
    let onTab: ((PhoneTab) -> Void)?
    let onProfile: (() -> Void)?
    let content: Content

    public init(chosen: PhoneTab, waiting: PhoneTab?, onTab: ((PhoneTab) -> Void)?, onProfile: (() -> Void)?,
                @ViewBuilder content: () -> Content) {
        self.chosen = chosen
        self.waiting = waiting
        self.onTab = onTab
        self.onProfile = onProfile
        self.content = content()
    }

    public var body: some View {
        ZStack(alignment: .top) {
            content.environment(\.tabBarHosted, true)
            TopBar(chosen: chosen, waiting: waiting, onTab: onTab, onProfile: onProfile)
                .environment(\.tabBarHosted, false)
        }
    }
}

public extension View {
    /// A page shown over the others by a sheet switch that is not a Sheet (Watch live): it rises and fades in, and
    /// fades out.
    func presenceFade() -> some View { modifier(PresenceFade()) }
}

private struct PresenceFade: ViewModifier {
    @Environment(\.motionPresence) private var presence
    @Environment(\.accessibilityReduceMotion) private var reduce
    func body(content: Content) -> some View {
        if let p = presence {
            content.opacity(min(1, max(0, p.value)))
                .offset(y: FerbMotion.enterOffset(p.value, leaving: p.leaving, reduce: reduce))
        } else {
            content
        }
    }
}
