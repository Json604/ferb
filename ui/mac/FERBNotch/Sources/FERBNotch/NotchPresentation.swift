import AppKit
import SwiftUI
import QuartzCore

/// One display clock drives both the native frost and SwiftUI geometry. Neither
/// surface receives a separate implicit animation that can jump ahead of the other.
@MainActor
final class NotchPresentation: NSObject, ObservableObject {
    private typealias Vector = AnimatablePair<AnimatablePair<CGFloat, CGFloat>, AnimatablePair<CGFloat, CGFloat>>
    let backdrop = NotchBackdropView(frame: .zero)
    @Published private var geometry = Vector(AnimatablePair(180, 32), AnimatablePair(0, 1))
    @Published var animates = false
    private(set) var surfaceOrigin = CGPoint.zero
    private var originStart = CGPoint.zero
    private var originTarget = CGPoint.zero
    var destination = CGPoint.zero
    var compactSize = CGSize(width: 58, height: 30)
    var cutoutHeight: CGFloat = 32
    var size: CGSize { CGSize(width: geometry.first.first, height: geometry.first.second) }
    var canvasSize: CGSize { backdrop.bounds.size }
    var surfaceOffset: CGPoint {
        CGPoint(x: surfaceOrigin.x - (canvasSize.width - size.width) / 2, y: surfaceOrigin.y)
    }
    var expansion: CGFloat { min(1, max(0, geometry.second.first)) }
    var bodyExpansion: CGFloat { min(1, max(0, geometry.second.second)) }

    private var displayLink: CADisplayLink?
    private var origin = Vector.zero
    private var target = Vector.zero
    private var velocity = Vector.zero
    private var initialVelocity = Vector.zero
    private var startedAt: CFTimeInterval = 0
    private var spring = Spring(duration: 0.3, bounce: 0.08)
    private var duration: TimeInterval = 0
    private var completion: (() -> Void)?
    private var publishedOrigin: CGPoint?
    private var publishedCanvas: CGSize?

    func present(size: CGSize, expanded: Bool, bodyVisible: Bool, anchor: CGPoint? = nil, animate: Bool,
                 completion: @escaping () -> Void) {
        let wasAnimating = displayLink != nil
        displayLink?.invalidate()
        displayLink = nil
        self.completion = nil
        target = Vector(AnimatablePair(size.width, size.height), AnimatablePair(expanded ? 1 : 0, bodyVisible ? 1 : 0))
        if !expanded, let anchor {
            originTarget = CGPoint(x: anchor.x - compactSize.width / 2, y: anchor.y - compactSize.height / 2)
        } else {
            originTarget = destination
        }
        guard animate, let screen = backdrop.window?.screen ?? NSScreen.main else {
            velocity = .zero
            surfaceOrigin = originTarget
            apply(target)
            completion()
            return
        }
        if expanded, let anchor, !wasAnimating {
            // The surface reveals stationary content, as the original notch did.
            // Body translation belongs only to an explicit header/body fold.
            origin = Vector(AnimatablePair(compactSize.width, compactSize.height),
                            AnimatablePair(0, bodyVisible ? 1 : 0))
            originStart = CGPoint(x: anchor.x - compactSize.width / 2, y: anchor.y - compactSize.height / 2)
            surfaceOrigin = originStart
        } else {
            origin = geometry
            originStart = surfaceOrigin
        }
        initialVelocity = velocity
        spring = Spring(duration: 0.3, bounce: 0.08)
        duration = spring.settlingDuration
        startedAt = CACurrentMediaTime()
        self.completion = completion
        let link = screen.displayLink(target: self, selector: #selector(tick(_:)))
        displayLink = link
        link.add(to: .main, forMode: .common)
        // Reapply the current shape after the window canvas grows; never expose
        // an unmasked, untinted full-size backdrop for even one frame.
        apply(origin)
    }

    @objc private func tick(_ link: CADisplayLink) {
        let elapsed = max(0, link.targetTimestamp - startedAt)
        guard elapsed < duration else {
            surfaceOrigin = originTarget
            apply(target)
            velocity = .zero
            link.invalidate()
            displayLink = nil
            let finished = completion
            completion = nil
            finished?()
            return
        }
        let delta = target - origin
        velocity = spring.velocity(target: delta, initialVelocity: initialVelocity, time: elapsed)
        let value = origin + spring.value(target: delta, initialVelocity: initialVelocity, time: elapsed)
        let startExpansion = origin.second.first
        let endExpansion = target.second.first
        let expansionDelta = endExpansion - startExpansion
        let progress = abs(expansionDelta) < 0.0001 ? 1 :
            (value.second.first - startExpansion) / expansionDelta
        surfaceOrigin = CGPoint(x: originStart.x + (originTarget.x - originStart.x) * progress,
                                y: originStart.y + (originTarget.y - originStart.y) * progress)
        apply(value)
    }

    private func apply(_ value: Vector) {
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) {
            // Origin/canvas are read by SwiftUI too. A pure translation still
            // publishes, while repeated identical drag samples do not relayout it.
            if geometry != value || publishedOrigin != surfaceOrigin || publishedCanvas != canvasSize {
                geometry = value
                publishedOrigin = surfaceOrigin
                publishedCanvas = canvasSize
            }
            backdrop.updateSurface(size: size, expansion: expansion, cutoutHeight: cutoutHeight,
                                   origin: surfaceOrigin)
            // Native frames and masks are already current. Let AppKit coalesce
            // SwiftUI layout and paint at the next display pass.
        }
    }

    func placeDuringDrag(size: CGSize, origin: CGPoint, bodyVisible: Bool) {
        stop()
        surfaceOrigin = origin
        destination = origin
        apply(Vector(AnimatablePair(size.width, size.height), AnimatablePair(1, bodyVisible ? 1 : 0)))
    }

    func stop() {
        displayLink?.invalidate()
        displayLink = nil
        completion = nil
    }
}

struct NotchContentTransition: ViewModifier {
    let progress: CGFloat

    func body(content: Content) -> some View {
        content
            .opacity(Double(progress))
            .scaleEffect(0.975 + 0.025 * progress, anchor: .top)
            .offset(y: -4 * (1 - progress))
            .blur(radius: 2 * (1 - progress))
    }
}
