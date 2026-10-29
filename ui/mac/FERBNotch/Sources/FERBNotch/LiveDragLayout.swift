import AppKit
import SwiftUI

/// Screen-space placement during direct manipulation. The grabbed pill never moves
/// to make room: the card faces inward from the nearest physical screen edge.
enum LiveDragLayout {
    struct Placement { var edge: FERBScreenEdge; var card: CGRect }
    static func resolve(pill: CGRect, screen: CGRect, preferred: CGSize, previous: FERBScreenEdge) -> Placement {
        func candidate(_ edge: FERBScreenEdge) -> CGRect {
            var area = screen
            switch edge {
            case .left: area.origin.x = min(screen.maxX, max(screen.minX, pill.maxX - 8)); area.size.width = screen.maxX - area.minX
            case .right: area.size.width = max(0, min(screen.width, pill.minX + 8 - screen.minX))
            case .bottom: area.origin.y = min(screen.maxY, max(screen.minY, pill.maxY - 8)); area.size.height = screen.maxY - area.minY
            case .top: area.size.height = max(0, min(screen.height, pill.minY + 8 - screen.minY))
            }
            let size = CGSize(width: min(preferred.width, area.width), height: min(preferred.height, area.height))
            var x = min(area.maxX - size.width, max(area.minX, pill.midX - size.width / 2))
            var y = min(area.maxY - size.height, max(area.minY, pill.midY - size.height / 2))
            if edge == .left { x = area.minX }; if edge == .right { x = area.maxX - size.width }
            if edge == .bottom { y = area.minY }; if edge == .top { y = area.maxY - size.height }
            return CGRect(origin: CGPoint(x:x,y:y), size:size)
        }
        let edge = nearestEdge(pill:pill,screen:screen,previous:previous)
        return Placement(edge:edge,card:candidate(edge))
    }
    static func nearestEdge(pill: CGRect, screen: CGRect, previous: FERBScreenEdge) -> FERBScreenEdge {
        let distances: [(FERBScreenEdge, CGFloat)] = [
            (.left, abs(pill.midX-screen.minX)), (.right, abs(screen.maxX-pill.midX)),
            (.bottom, abs(pill.midY-screen.minY)), (.top, abs(screen.maxY-pill.midY))]
        let nearest = distances.map(\.1).min() ?? 0
        // Preserve direction only on the exact bisector, not based on card room.
        let edge = distances.first(where: { $0.0 == previous && $0.1 == nearest })?.0
            ?? distances.first(where: { $0.1 == nearest })!.0
        return edge
    }

    static func connectedCard(_ card: CGRect, pill: CGRect, screen: CGRect) -> CGRect {
        let fitted = fit(card, in: screen)
        let path = NotchSurfaceShape().path(in: fitted).cgPath
        let probes = [CGPoint(x:pill.midX,y:pill.midY), CGPoint(x:pill.minX+4,y:pill.midY),
                      CGPoint(x:pill.maxX-4,y:pill.midY), CGPoint(x:pill.midX,y:pill.minY+4),
                      CGPoint(x:pill.midX,y:pill.maxY-4)]
        if probes.contains(where: { path.contains($0) }) { return fitted }
        let contact = pill.insetBy(dx:8,dy:8).intersection(screen)
        guard !contact.isNull, !contact.isEmpty else { return fitted }
        return fit(fitted.union(contact), in:screen)
    }
    static func fit(_ rect: CGRect, in screen: CGRect) -> CGRect {
        let width = min(rect.width,screen.width), height = min(rect.height,screen.height)
        return CGRect(x:min(screen.maxX-width,max(screen.minX,rect.minX)), y:min(screen.maxY-height,max(screen.minY,rect.minY)),width:width,height:height)
    }
    static func interpolate(_ start: CGRect, _ end: CGRect, progress: CGFloat) -> CGRect {
        CGRect(x:start.minX+(end.minX-start.minX)*progress,y:start.minY+(end.minY-start.minY)*progress,
               width:start.width+(end.width-start.width)*progress,height:start.height+(end.height-start.height)*progress)
    }
}

/// During a held drag, move only children inside a display-stable native canvas.
/// Moving the WindowServer frame and counter-moving the control in Core Animation
/// are separate presentations; their transient mismatch makes the grabbed pill drift.
@MainActor
enum DragSurfaceCanvas {
    static func place(window: NSWindow, presentation: NotchPresentation, control: NSView,
                      pill: CGRect, card: CGRect, canvas: CGRect, bodyVisible: Bool) {
        if window.frame != canvas { window.setFrame(canvas, display: true) }
        presentation.placeDuringDrag(size: card.size,
            origin: CGPoint(x: card.minX-canvas.minX, y: canvas.maxY-card.maxY), bodyVisible: bodyVisible)
        control.frame = CGRect(x: pill.minX-canvas.minX, y: canvas.maxY-pill.maxY,
                               width: pill.width, height: pill.height)
    }
}

/// Only an orientation change creates a finite transition. Ordinary pointer motion
/// translates both endpoints directly; it never starts or extends a chase filter.
struct DragChatMotion {
    private typealias Vector = AnimatablePair<AnimatablePair<CGFloat, CGFloat>, AnimatablePair<CGFloat, CGFloat>>
    private let spring = Spring(duration: 0.3, bounce: 0)
    private var velocity = Vector.zero
    private var initialVelocity = Vector.zero
    private static func vector(_ rect: CGRect) -> Vector {
        Vector(AnimatablePair(rect.minX, rect.minY), AnimatablePair(rect.width, rect.height))
    }
    private(set) var card: CGRect
    private var edge: FERBScreenEdge
    private var target: CGRect
    private var start: CGRect?
    private var startedAt: TimeInterval = 0
    var isAnimating: Bool { start != nil }
    init(card: CGRect, edge: FERBScreenEdge) {
        self.card = card; self.target = card; self.edge = edge
    }
    mutating func update(target: CGRect, edge: FERBScreenEdge, delta: CGPoint,
                         at time: TimeInterval, reduceMotion: Bool = false, visibleCard: CGRect? = nil) -> CGRect {
        _ = sample(at:time)
        card = card.offsetBy(dx:delta.x,dy:delta.y)
        if let currentStart = start { start = currentStart.offsetBy(dx:delta.x,dy:delta.y) }
        if edge != self.edge {
            start = visibleCard?.offsetBy(dx:delta.x,dy:delta.y) ?? card
            // Carry actual point/size velocity through a reversal, rather than
            // launching a fresh ease-out from the currently drawn geometry.
            initialVelocity = velocity
            startedAt = time
        }
        self.edge = edge
        self.target = target
        if reduceMotion { start = nil; velocity = .zero; initialVelocity = .zero }
        return sample(at:time)
    }
    mutating func sample(at time: TimeInterval) -> CGRect {
        guard let start else { card = target; velocity = .zero; return card }
        let elapsed = max(0,time-startedAt)
        if elapsed >= spring.settlingDuration {
            self.start = nil
            velocity = .zero
            card = target
            return card
        }
        let origin = Self.vector(start)
        let delta = Self.vector(target) - origin
        let value = origin + spring.value(target:delta,initialVelocity:initialVelocity,time:elapsed)
        velocity = spring.velocity(target:delta,initialVelocity:initialVelocity,time:elapsed)
        card = CGRect(x:value.first.first,y:value.first.second,
                      width:value.second.first,height:value.second.second)
        return card
    }
}
