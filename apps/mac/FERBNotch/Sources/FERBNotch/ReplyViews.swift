import AppKit
import SwiftUI
import NotchCore

// Reply to a message (ReplyQuotes.swift): the reply arrow a message shows under the pointer, the two-finger sideways
// swipe that does the same, the quote above the composer and the quote in a sent message.

/// A message the owner can reply to: its quote and the model that takes it.
struct BubbleReply {
    let quote: ReplyQuote
    let model: NotchModel
}

extension NotchModel {
    /// A bubble's reply, when its entry id is known (else it offers none).
    func bubbleReply(entryID: String?, kind: String, owner: Bool, text: String) -> BubbleReply? {
        quote(entryID: entryID, kind: kind, owner: owner, text: text).map { BubbleReply(quote: $0, model: self) }
    }
}

/// Which side of the chat a message stands on; its reply arrow sits in the free space beside it.
enum BubbleSide {
    case ferb, owner
    var gutter: Edge.Set { self == .ferb ? .trailing : .leading }
    var corner: Alignment { self == .ferb ? .bottomTrailing : .bottomLeading }
}

/// A message bubble that can be replied to: the free space beside it (`gutter`) is part of it, so the pointer reaches
/// the arrow without the bubble losing its hover. The arrow shows under the pointer and grows in with a sideways swipe;
/// the quoted message lights up briefly when a quote of it is clicked. Nothing here moves the bubble.
struct QuotableBubble: ViewModifier {
    static let gutter: CGFloat = 28
    /// How far a sideways swipe goes before letting go replies.
    static let reach: CGFloat = 44

    let quote: ReplyQuote
    let side: BubbleSide
    @ObservedObject var model: NotchModel
    var cornerRadius: CGFloat = 15
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hovering = false
    @State private var pull: CGFloat = 0

    func body(content: Content) -> some View {
        let lit = model.quoteHighlight == quote.entryID
        content
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .fill(.white.opacity(lit ? 0.16 : 0))
                    .allowsHitTesting(false)
            }
            .animation(.easeOut(duration: lit ? 0.18 : 0.6), value: lit)
            .padding(side.gutter, Self.gutter)
            .overlay {
                HoverArea(pointing: false, label: "reply") { inside in
                    withAnimation(reduceMotion ? nil : .spring(duration: 0.3, bounce: 0)) { hovering = inside }
                }
            }
            .overlay(alignment: side.corner) {
                if hovering || pull > 0 {
                    ReplyArrow(model: model, quote: quote, pull: pull, reduceMotion: reduceMotion)
                        .padding(side.gutter, 3)
                        .transition(.opacity)
                }
            }
            .background(SwipeMonitor(active: hovering, reach: Self.reach, pulled: { distance in
                pull = distance
            }, released: { crossed in
                withAnimation(reduceMotion ? nil : .spring(duration: 0.35, bounce: 0)) { pull = 0 }
                if crossed { model.reply(to: quote) }
            }))
            .accessibilityAction(named: "Reply") { model.reply(to: quote) }
    }
}

/// The reply arrow: a small round button. While a swipe pulls it, it grows in with the pull and is full once letting
/// go would reply (under Reduce Motion it shows only then, at its size).
private struct ReplyArrow: View {
    @ObservedObject var model: NotchModel
    let quote: ReplyQuote
    let pull: CGFloat
    let reduceMotion: Bool
    @State private var hovered = false

    var body: some View {
        let ready = pull >= QuotableBubble.reach
        let progress = pull > 0 ? min(1, pull / QuotableBubble.reach) : 1
        Button { model.reply(to: quote) } label: {
            Image(systemName: "arrowshape.turn.up.left")
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(ready || hovered ? Palette.ink : Palette.secondary)
                .frame(width: 22, height: 22)
                .background(.white.opacity(ready ? 0.22 : HoverLook.fill(hovered: hovered)), in: Circle())
        }
        .buttonStyle(.plain)
        .cardHover($hovered, link: true, capsule: true, drawsFill: false, label: "reply")
        .scaleEffect(reduceMotion ? 1 : 0.6 + 0.4 * progress)
        .opacity(reduceMotion && pull > 0 && !ready ? 0 : progress)
        .help("Reply")
        .accessibilityLabel(quote.byOwner ? "Reply to your message" : "Reply to FERB's message")
    }
}

/// A two-finger sideways swipe over the message (while the pointer is on it): how far it went, as it goes, and whether
/// it went far enough when the fingers lifted. A swipe that starts up or down is the chat's scroll and is left alone; a
/// sideways one is taken here (the chat never scrolls sideways), its glide after the fingers lift too.
private struct SwipeMonitor: NSViewRepresentable {
    let active: Bool
    let reach: CGFloat
    let pulled: (CGFloat) -> Void
    let released: (Bool) -> Void

    func makeNSView(context: Context) -> SwipeView { SwipeView() }

    func updateNSView(_ view: SwipeView, context: Context) {
        view.reach = reach
        view.pulled = pulled
        view.released = released
        view.active = active
    }

    static func dismantleNSView(_ view: SwipeView, coordinator: ()) { view.stop() }

    final class SwipeView: NSView {
        var reach: CGFloat = 44
        var pulled: (CGFloat) -> Void = { _ in }
        var released: (Bool) -> Void = { _ in }
        var active = false { didSet { if active { start() } else if direction != .sideways { stop() } } }

        private enum Direction { case sideways, upDown }
        private var monitor: Any?
        private var direction: Direction?
        private var dx: CGFloat = 0
        private var dy: CGFloat = 0
        /// A sideways swipe ended: its glide is taken too, so it never scrolls the chat.
        private var gliding = false

        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override func isAccessibilityElement() -> Bool { false }

        override func viewWillMove(toWindow newWindow: NSWindow?) {
            super.viewWillMove(toWindow: newWindow)
            if newWindow == nil { stop() }
        }

        func start() {
            guard monitor == nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
                let scroll = Scroll(window: event.window.map(ObjectIdentifier.init), precise: event.hasPreciseScrollingDeltas,
                                    phase: event.phase, momentum: event.momentumPhase,
                                    dx: event.scrollingDeltaX, dy: event.scrollingDeltaY)
                let taken = MainActor.assumeIsolated { self?.takes(scroll) ?? false }
                return taken ? nil : event
            }
        }

        func stop() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            if direction == .sideways { pulled(0) }
            direction = nil
            gliding = false
        }

        /// What a scroll event says, read where it arrives.
        struct Scroll {
            let window: ObjectIdentifier?
            let precise: Bool
            let phase: NSEvent.Phase
            let momentum: NSEvent.Phase
            let dx: CGFloat
            let dy: CGFloat
        }

        /// Whether the swipe takes this scroll event (the chat never sees it).
        private func takes(_ event: Scroll) -> Bool {
            guard let window, event.window == ObjectIdentifier(window), event.precise else { return false }
            if event.phase.contains(.began) || event.phase.contains(.mayBegin) {
                direction = nil
                dx = 0
                dy = 0
                gliding = false
            }
            guard !event.phase.isEmpty else {
                // the glide after the fingers lift
                if gliding {
                    if event.momentum.contains(.ended) || event.momentum.contains(.cancelled) { finishGlide() }
                    return true
                }
                return false
            }
            if direction == nil {
                dx += event.dx
                dy += event.dy
                if hypot(dx, dy) > 6 { direction = abs(dx) > abs(dy) * 1.5 ? .sideways : .upDown }
            } else if direction == .sideways {
                dx += event.dx
            }
            if direction == .sideways { pulled(abs(dx)) }
            if event.phase.contains(.ended) || event.phase.contains(.cancelled) {
                let sideways = direction == .sideways
                if sideways { released(event.phase.contains(.ended) && abs(dx) >= reach) }
                direction = nil
                dx = 0
                dy = 0
                gliding = sideways
                if !active && !sideways { stop() }
                return sideways
            }
            return direction == .sideways
        }

        private func finishGlide() {
            gliding = false
            if !active { stop() }
        }
    }
}

/// The quote above the composer: who said it and its opening words on one line, and ✕. Clicking the words shows the
/// quoted message in the chat.
struct ComposerQuoteLine: View {
    let quote: ReplyQuote
    @ObservedObject var model: NotchModel
    @State private var hovered = false

    var body: some View {
        HStack(spacing: 6) {
            Button { model.showQuoted(quote) } label: {
                HStack(spacing: 5) {
                    Image(systemName: "arrowshape.turn.up.left").font(.system(size: 9.5, weight: .semibold))
                    Text(quote.speaker + ": " + quote.line).lineLimit(1).truncationMode(.tail)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Show this message")
            .accessibilityLabel("Replying to " + (quote.byOwner ? "your message: " : "FERB: ") + quote.line)
            Button { model.dropReplyQuote() } label: {
                Image(systemName: "xmark").font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(hovered ? Palette.ink : Palette.secondary)
                    .frame(width: 18, height: 18)
            }
            .buttonStyle(.plain)
            .cardHover($hovered, link: true, capsule: true, drawsFill: false, label: "quote")
            .help("Don't reply to this message · Esc")
            .accessibilityLabel("Don't reply to this message")
        }
        .font(.system(size: 11))
        .foregroundStyle(Palette.secondary)
        .padding(.leading, 10).padding(.trailing, 6).padding(.top, 6)
        .frame(height: NotchModel.replyQuoteLineHeight)
    }
}

/// The quote in a sent message, above his words: one compact line in the bubble's own rounded style; clicking it shows
/// the quoted message.
struct SentQuoteLine: View {
    let quote: ReplyQuote
    @ObservedObject var model: NotchModel
    @State private var hovered = false

    var body: some View {
        Button { model.showQuoted(quote) } label: {
            HStack(spacing: 4) {
                Image(systemName: "arrowshape.turn.up.left").font(.system(size: 9, weight: .semibold))
                Text(quote.speaker + ": " + quote.line).lineLimit(1).truncationMode(.tail)
            }
            .font(.system(size: 10.5))
            .foregroundStyle(.white.opacity(0.78))
            .padding(.horizontal, 7).padding(.vertical, 4)
            .background(.white.opacity(hovered ? 0.22 : 0.14), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        }
        .buttonStyle(.plain)
        .cardHover($hovered, link: true, cornerRadius: 9, drawsFill: false, label: "quote")
        .help("Show this message")
        .accessibilityLabel("Replying to " + (quote.byOwner ? "your message: " : "FERB: ") + quote.line)
    }
}
