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
