import SwiftUI

// Not from the lab. Reply to a message (the owner, 2026-11-27, kb APP-1146, APP-1147: "quote any ferb's reply message,
// either slide or an on hover icon maybe. many apps have that"; "yeah my messages too"), as FERB.app does it
// (ReplyViews.swift), in the phone's one look (Android's components/Reply.kt: the same numbers and shapes). A quotable
// bubble swiped to the right follows the finger a short way with the reply arrow growing in the space it leaves; past
// `SwipeToReply.reach` a light tick says letting go replies, and letting go there does; it springs back either way.
// The quote then waits in the message box (`ReplyQuoteBar`, ✕ drops it); a sent message shows what it quotes above its
// bubble (`SentQuote`), a tap on which shows the quoted message lit up for a moment (`QuoteGlow`). The words are the
// app's.

/// A quote as a bubble or the message box shows it: who said it and its opening words on one line ("You: …",
/// "FERB: …"), what VoiceOver says of it, and the tap that shows the quoted message (nil: none).
public struct QuoteShown {
    public var line: String
    public var label: String
    public var hint: String?
    public var onTap: (() -> Void)?

    public init(line: String, label: String, hint: String? = nil, onTap: (() -> Void)? = nil) {
        self.line = line
        self.label = label
        self.hint = hint
        self.onTap = onTap
    }
}

/// What a sent message quotes, above his bubble (FERB.app's SentQuoteLine): one compact line on FERB's dark surface, in
/// the shape of the chat's small lines, so it reads on any sky; at most a bubble wide.
public struct SentQuote: View {
    let quote: QuoteShown
    public init(_ quote: QuoteShown) { self.quote = quote }
    public var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrowshape.turn.up.left.fill").font(.system(size: 11, weight: .semibold)).foregroundStyle(Ink.onDark)
            Text(quote.line).type(13.5, .medium, Ink.onDark).lineLimit(1).truncationMode(.tail)
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .background(Ink.black.opacity(0.82), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .pressable(quote.onTap)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(quote.label))
        .accessibilityHint(quote.hint.map { Text($0) } ?? Text(""))
        .accessibilityAddTraits(quote.onTap == nil ? [] : .isButton)
        .frame(maxWidth: 310, alignment: .trailing)
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// The quote waiting in the message box (inside its black card, at its top): the reply arrow and the quote's one line
/// (a tap shows the quoted message), and ✕, which drops the quote and keeps the words.
public struct ReplyQuoteBar: View {
    let quote: QuoteShown
    let dropLabel: String
    let onDrop: () -> Void

    public init(_ quote: QuoteShown, dropLabel: String, onDrop: @escaping () -> Void) {
        self.quote = quote
        self.dropLabel = dropLabel
        self.onDrop = onDrop
    }

    public var body: some View {
        HStack(spacing: 8) {
            HStack(spacing: 7) {
                Image(systemName: "arrowshape.turn.up.left.fill").font(.system(size: 12, weight: .semibold)).foregroundStyle(Ink.onDark)
                Text(quote.line).type(14, .medium, Ink.onDark).lineLimit(1).truncationMode(.tail)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 9)
            .contentShape(Rectangle())
            .pressable(quote.onTap)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text(quote.label))
            .accessibilityHint(quote.hint.map { Text($0) } ?? Text(""))
            .accessibilityAddTraits(quote.onTap == nil ? [] : .isButton)
            Image(systemName: "xmark").font(.system(size: 13, weight: .semibold)).foregroundStyle(Ink.onDark)
                .frame(width: 40, height: 40).contentShape(Rectangle())
                .pressable(onDrop, label: dropLabel)
        }
        .frame(minHeight: 40)
        .padding(.leading, 12)
        .background(Ink.darkFill, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
    }
}

/// A bubble's reply as the row hands it to the bubble (`bubbleReply` in the environment): what VoiceOver's action says
/// ("Reply") and what replying does. The owner's and FERB's bubbles take it, so the bubble itself swipes.
public struct BubbleReply {
    public var label: String
    public var onReply: () -> Void
    public init(label: String, onReply: @escaping () -> Void) {
        self.label = label
        self.onReply = onReply
    }
}

private struct BubbleReplyKey: EnvironmentKey { static let defaultValue: BubbleReply? = nil }
private struct QuoteLitKey: EnvironmentKey { static let defaultValue = false }
private struct ReplyPullShotKey: EnvironmentKey { static let defaultValue: CGFloat? = nil }

public extension EnvironmentValues {
    /// The reply of the bubble under it (nil: it offers none).
    var bubbleReply: BubbleReply? {
        get { self[BubbleReplyKey.self] }
        set { self[BubbleReplyKey.self] = newValue }
    }

    /// Holds a reply swipe that far out (the screenshots of a swipe under way; Android's `pullForShot`).
    var replyPullForShot: CGFloat? {
        get { self[ReplyPullShotKey.self] }
        set { self[ReplyPullShotKey.self] = newValue }
    }

    /// The bubble under it is the quoted message, lit up for a moment.
    var quoteLit: Bool {
        get { self[QuoteLitKey.self] }
        set { self[QuoteLitKey.self] = newValue }
    }
}

/// A bubble that can be replied to: swiped to the right (never while the chat scrolls up or down: a drag that starts
/// more up or down than sideways is the scroll's, and a leftward one is no reply), it follows the finger one to one up
/// to `reach`, then a third as fast, never past `most`, with the reply arrow growing in the space it leaves, near its
/// foot; past `reach` the arrow darkens and a light tick says letting go replies; letting go there replies, and it
/// springs back either way. Under Reduce Motion it doesn't move: the arrow shows once letting go would reply. VoiceOver
/// has the reply as an action.
public struct SwipeToReply: ViewModifier {
    /// How far a swipe goes before letting go replies (Android's ReplySwipe.reach; FERB.app's is 44 on a trackpad).
    public static let reach: CGFloat = 64
    /// The furthest the bubble follows the finger.
    public static let most: CGFloat = 96
    /// The arrow's circle at full size.
    static let arrow: CGFloat = 30

    let reply: BubbleReply
    @Environment(\.accessibilityReduceMotion) private var reduce
    @Environment(\.replyPullForShot) private var shot
    @GestureState(resetTransaction: Transaction(animation: .spring(duration: 0.35, bounce: 0.1))) private var pull = Pull.idle

    public init(_ reply: BubbleReply) { self.reply = reply }

    /// Where a drag stands: not decided yet, sideways (how far to the right), or the chat's scroll.
    enum Pull: Equatable {
        case idle, sideways(CGFloat), upDown
        var distance: CGFloat { if case .sideways(let d) = self { d } else { 0 } }
    }

    /// The first move decides (the chat never scrolls sideways): sideways only when to the right and clearly more
    /// across than up or down.
    static func decide(_ translation: CGSize) -> Pull {
        translation.width > 0 && translation.width > abs(translation.height) * 1.5 ? .sideways(translation.width) : .upDown
    }

    /// How far the bubble follows a pull of `distance`: one to one up to `reach`, then a third as much, never past `most`.
    static func follow(_ distance: CGFloat) -> CGFloat {
        guard distance > 0 else { return 0 }
        return distance <= reach ? distance : min(most, reach + (distance - reach) / 3)
    }

    /// Whether letting go of a drag that went this far replies: the rule the drag followed (sideways from its start),
    /// past the reach.
    static func replies(_ translation: CGSize) -> Bool {
        if case .sideways = decide(translation) { return translation.width >= reach }
        return false
    }

    public func body(content: Content) -> some View {
        let shown = Self.follow(shot ?? pull.distance)
        let ready = shown >= Self.reach
        let progress = min(1, shown / Self.reach)
        content
            .offset(x: reduce ? 0 : shown)
            // (after the offset: the arrow stays where the bubble was)
            .background(alignment: .bottomLeading) {
                // in the space the bubble leaves, from under it; near the foot of a tall bubble, mid-height of a short one
                if shown > 0 && (!reduce || ready) {
                    GeometryReader { box in
                        let size = Self.arrow * (reduce ? 1 : 0.6 + 0.4 * progress)
                        Image(systemName: "arrowshape.turn.up.left.fill").font(.system(size: size * 0.48, weight: .semibold))
                            .foregroundStyle(Ink.white.opacity(reduce ? 1 : progress))
                            .frame(width: size, height: size)
                            .background(Ink.black.opacity((ready ? 0.85 : 0.55) * (reduce ? 1 : progress)), in: Circle())
                            .position(x: min(shown, Self.reach) / 2, y: box.size.height - min(box.size.height / 2, 24))
                    }
                    .accessibilityHidden(true)
                }
            }
            .simultaneousGesture(
                DragGesture(minimumDistance: 10, coordinateSpace: .local)
                    .updating($pull) { v, state, _ in
                        switch state {
                        case .idle: state = Self.decide(v.translation)
                        case .sideways: state = .sideways(max(0, v.translation.width))
                        case .upDown: break
                        }
                    }
                    .onEnded { v in
                        // the same rule the drag followed: sideways from its start, past the reach where it was let go
                        if Self.replies(v.translation) { reply.onReply() }
                    }
            )
            .sensoryFeedback(.impact(weight: .light), trigger: ready) { old, new in !old && new }
            .accessibilityAction(named: Text(reply.label), reply.onReply)
    }
}

/// The quoted message lit up for a moment after a tap on a quote of it (FERB.app's `quoteHighlight`; Android's): a soft
/// white ring around the bubble and a light over it, in the bubble's own shape, coming in quickly and fading.
public struct QuoteGlow<S: Shape>: ViewModifier {
    let lit: Bool
    let shape: S
    public init(lit: Bool, shape: S) { self.lit = lit; self.shape = shape }
    public func body(content: Content) -> some View {
        content
            .background { shape.fill(Ink.white.opacity(lit ? 0.45 : 0)).padding(-5).allowsHitTesting(false) }
            .overlay { shape.fill(Ink.white.opacity(lit ? 0.16 : 0)).allowsHitTesting(false) }
            .animation(.easeOut(duration: lit ? 0.18 : 0.6), value: lit)
    }
}

/// What a bubble wears for replies: lit up as the quoted message, and swiped to reply when it offers one.
struct Quotable<S: Shape>: ViewModifier {
    let shape: S
    @Environment(\.bubbleReply) private var reply
    @Environment(\.quoteLit) private var lit
    func body(content: Content) -> some View {
        if let reply {
            content.modifier(QuoteGlow(lit: lit, shape: shape)).modifier(SwipeToReply(reply))
        } else {
            content.modifier(QuoteGlow(lit: lit, shape: shape))
        }
    }
}

public extension View {
    /// The bubbles under it reply with `reply` (nil: none) and are lit as the quoted message when `lit`.
    func bubbleReply(_ reply: BubbleReply?, lit: Bool) -> some View {
        environment(\.bubbleReply, reply).environment(\.quoteLit, lit)
    }
}
