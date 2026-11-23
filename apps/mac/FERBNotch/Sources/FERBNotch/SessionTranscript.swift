import AppKit
import SwiftUI
import NotchCore

/// Where a coding session's conversation stands (PHN-813, APP-1120), by the chat's own rules (APP-720, APP-806):
/// following the newest while the owner is at the bottom (TranscriptFollow tells his scrolling from growth); arrivals
/// while he is scrolled up leave the view where it is and show "New message"; earlier items come in as he scrolls near
/// the top. A reference, so scrolling never re-renders the conversation; `unreadBelow` is the only thing it publishes.
@MainActor
final class SessionTranscriptPlace: ObservableObject {
    var follow = TranscriptFollow()
    /// When FERB last moved the view itself: earlier items come in only after the owner scrolled since (APP-806).
    var ferbScrolledAt = Date.distantPast
    let marks = AnchorMarks()
    lazy var list = TranscriptList(marks: marks)
    /// Something arrived below while he was scrolled up: the "New message" control shows.
    @Published private(set) var unreadBelow = false

    /// What the list should do after it moved or grew.
    struct Step: Equatable {
        var pin = false
        var loadEarlier = false
    }

    func scrolled(offset: CGFloat, height: CGFloat, viewport: CGFloat, more: Bool, loading: Bool, at now: Date) -> Step {
        var step = Step()
        step.pin = follow.observe(offset: offset, contentHeight: height, viewport: viewport, at: now)
        // within a screen of the top, once he scrolled (or when it doesn't fill two screens): never as it opens
        if offset < viewport, more, !loading,
           (follow.ownerScrolledAt ?? .distantPast) > ferbScrolledAt || height < 2 * viewport {
            step.loadEarlier = true
        }
        if offset + viewport >= height - TranscriptFollow.bottomSlack, unreadBelow { unreadBelow = false }
        return step
    }

    /// Newer items, a card or his own words arrived: true when the view goes to the bottom (it was following), else the
    /// "New message" control shows.
    func arrived() -> Bool {
        if follow.following {
            unreadBelow = false
            return true
        }
        unreadBelow = true
        return false
    }

    /// He sent something or tapped "New message": the view follows the newest again.
    func jumped() {
        unreadBelow = false
        follow.follow()
    }

    /// FERB moves the view: offset changes until it settles aren't the owner's.
    func movedByFERB(at now: Date, animated: Bool) {
        let settle: TimeInterval = animated ? 0.3 : 0.1
        follow.scrolledByFERB(at: now, settle: settle)
        ferbScrolledAt = now.addingTimeInterval(settle)
    }
}

/// A coding session's conversation drawn by the chat's own list (APP-815's TranscriptList): rows measured once and kept,
/// the place kept when earlier items come in above, the chat's text surface for selecting and copying (APP-788), the
/// top fade, the loader and "New message" (PHN-813).
struct SessionTranscript: View {
    @ObservedObject var sessions: SessionsModel
    let agentName: String
    @StateObject private var place = SessionTranscriptPlace()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var viewport: CGFloat = 400

    /// The list's rows: the space under the fade, the loader, the items, the open cards, his words on their way, the end.
    static func rows(items: [SessionItem], cards: [SessionCard], outgoing: SessionsModel.Outgoing?,
                     loadingEarlier: Bool) -> [TranscriptRow] {
        var rows: [TranscriptRow] = [.top]
        if loadingEarlier { rows.append(.loader) }
        rows += items.map { .item(key: "session-item-\($0.seq)", text: $0.kind + "\u{1F}" + $0.text) }
        rows += cards.map { card in
            .item(key: "session-card-\(card.id)",
                  text: [card.question ?? "", card.detail, card.choices.map(\.label).joined(separator: "\u{1F}")]
                    .joined(separator: "\u{1F}"))
        }
        if let o = outgoing {
            let state: String
            switch o.state {
            case .sending: state = "sending"
            case .sent: state = "sent"
            case .notSent(let why): state = "not-sent\u{1F}" + why
            }
            rows.append(.item(key: "session-outgoing", text: o.text + "\u{1F}" + state))
        }
        return rows + [.bottom]
    }

    var body: some View {
        let items = Dictionary(sessions.items.map { ("session-item-\($0.seq)", $0) }, uniquingKeysWith: { a, _ in a })
        let cards = Dictionary(sessions.cards.map { ("session-card-\($0.id)", $0) }, uniquingKeysWith: { a, _ in a })
        let outgoing = sessions.outgoing
        TranscriptListView(list: place.list,
                           rows: Self.rows(items: sessions.items, cards: sessions.cards, outgoing: outgoing,
                                           loadingEarlier: sessions.loadingEarlier),
                           viewport: viewport,
                           content: { [sessions, agentName] row in
                               Self.content(row, items: items, cards: cards, outgoing: outgoing, agentName: agentName,
                                            sessions: sessions)
                           },
                           scrolled: { offset, height in scrolled(offset, height) })
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { viewport = max(1, $0) }
            .mask {
                // the chat's fade at the top (ConversationTranscript)
                VStack(spacing: 0) {
                    LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom).frame(height: 24)
                    Rectangle().fill(.black)
                }
            }
            .overlay(alignment: .bottomTrailing) {
                if place.unreadBelow {
                    TranscriptJumpButton(title: "New message", systemImage: "arrow.down") {
                        place.jumped()
                        toBottom(animated: !reduceMotion)
                    }
                }
            }
            .onChange(of: sessions.items.last?.seq) { _, _ in arrived() }
            .onChange(of: sessions.cards.map(\.id)) { _, _ in arrived() }
            .onChange(of: sessions.outgoing) { old, new in
                // his own words always bring the view down to them; their state changing is growth like any other
                if let new, new.state == .sending, old?.state != .sending {
                    place.jumped()
                    toBottom(animated: false)
                } else {
                    arrived()
                }
            }
            .onAppear { place.list.toEnd(animated: false) }
    }

    private func scrolled(_ offset: CGFloat, _ height: CGFloat) {
        let step = place.scrolled(offset: offset, height: height, viewport: viewport, more: sessions.more,
                                  loading: sessions.loadingEarlier, at: Date())
        if step.pin { toBottom(animated: false) }
        if step.loadEarlier { Task { await sessions.showEarlier() } }
    }

    private func arrived() {
        // after this update's rows are in the list
        DispatchQueue.main.async { if place.arrived() { toBottom(animated: false) } }
    }

    private func toBottom(animated: Bool) {
        place.movedByFERB(at: Date(), animated: animated)
        place.list.toEnd(animated: animated)
    }

    @MainActor
    private static func content(_ row: TranscriptRow, items: [String: SessionItem], cards: [String: SessionCard],
                                outgoing: SessionsModel.Outgoing?, agentName: String, sessions: SessionsModel) -> AnyView {
        switch row {
        case .loader: return AnyView(EarlierLoader())
        case let .item(key, _):
            if let item = items[key] { return AnyView(SessionItemView(item: item)) }
            if let card = cards[key] {
                return AnyView(SessionCardView(card: card, agentName: agentName) { choice in
                    Task { await sessions.answer(card, choice) }
                })
            }
            if key == "session-outgoing", let outgoing {
                return AnyView(SessionOutgoingView(outgoing: outgoing) { Task { await sessions.tryAgain() } })
            }
            return AnyView(Color.clear)
        default: return AnyView(Color.clear)
        }
    }
}

/// His words to the session while they are on their way (PHN-812): dimmed, with a small spinner and "Sending…"; once
/// sent, his ordinary bubble until the session's record has them; not sent: dimmed, "Not sent." with the daemon's
/// reason, and Try again.
struct SessionOutgoingView: View {
    let outgoing: SessionsModel.Outgoing
    let tryAgain: () -> Void

    var body: some View {
        VStack(alignment: .trailing, spacing: 5) {
            SessionBubble(text: outgoing.text).opacity(outgoing.state == .sent ? 1 : 0.55)
            switch outgoing.state {
            case .sending:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("Sending…").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            case .sent:
                EmptyView()
            case .notSent(let why):
                VStack(alignment: .trailing, spacing: 4) {
                    Label("Not sent.", systemImage: "exclamationmark.circle").font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.red.opacity(0.9))
                    Text(why).font(.system(size: 11)).foregroundStyle(.secondary).multilineTextAlignment(.trailing)
                        .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                    Button("Try again", action: tryAgain).font(.system(size: 11))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// The owner's words in a session, on the right: the chat's text surface (select, copy), plain text only.
struct SessionBubble: View {
    let text: String
    var body: some View {
        HStack {
            Spacer(minLength: 80)
            NativeMessageText(text, size: 13)
                .padding(.horizontal, 14).padding(.vertical, 9)
                .background(.white.opacity(0.12), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
    }
}

/// A session's conversation while its first read is on its way (PHN-816, owner 2026-11-23): the shapes of its bubbles,
/// his on the right and the agent's on the left, breathing gently (still under Reduce Motion). No words: nothing is
/// claimed until the daemon answers.
struct SessionSkeleton: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    /// Each bubble: on his side (right) or the agent's, its width as a share of the row, its lines.
    static let bubbles: [(mine: Bool, width: CGFloat, lines: Int)] = [
        (true, 0.42, 1), (false, 0.78, 3), (false, 0.55, 1), (true, 0.36, 1), (false, 0.7, 2)]

    var body: some View {
        GeometryReader { geo in
            VStack(alignment: .leading, spacing: 14) {
                ForEach(Array(Self.bubbles.enumerated()), id: \.offset) { _, b in
                    HStack {
                        if b.mine { Spacer(minLength: 80) }
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .fill(.white.opacity(b.mine ? 0.12 : 0.07))
                            .frame(width: max(60, (geo.size.width - 80) * b.width), height: CGFloat(18 + 16 * b.lines))
                        if !b.mine { Spacer(minLength: 80) }
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.top, 12)
        }
        .opacity(dim ? 0.55 : 1)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { dim = true }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Opening this session…")
    }
}
