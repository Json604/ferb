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
    var cutoutHeight: CGFloat = 32
    var size: CGSize { CGSize(width: geometry.first.first, height: geometry.first.second) }
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

    func present(size: CGSize, expanded: Bool, bodyVisible: Bool, animate: Bool, completion: @escaping () -> Void) {
        displayLink?.invalidate()
        displayLink = nil
        self.completion = nil
        target = Vector(AnimatablePair(size.width, size.height), AnimatablePair(expanded ? 1 : 0, bodyVisible ? 1 : 0))
        guard animate, let screen = backdrop.window?.screen ?? NSScreen.main else {
            velocity = .zero
            apply(target)
            completion()
            return
        }
        origin = geometry
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
        apply(geometry)
    }

    @objc private func tick(_ link: CADisplayLink) {
        let elapsed = max(0, link.targetTimestamp - startedAt)
        guard elapsed < duration else {
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
        apply(origin + spring.value(target: delta, initialVelocity: initialVelocity, time: elapsed))
    }

    private func apply(_ value: Vector) {
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) {
            geometry = value
            backdrop.updateSurface(size: size, expansion: expansion, cutoutHeight: cutoutHeight)
            backdrop.layoutSubtreeIfNeeded()
        }
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
