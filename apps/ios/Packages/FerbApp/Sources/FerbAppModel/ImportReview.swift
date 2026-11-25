import FerbCore
import FerbMobile
import Foundation
import Observation

/// The owner's review of what FERB learned (kb MEM-460..457, MEM-485, MEM-486): one list, each line ticked until he
/// unticks it, answered all at once (`memory.review.all`): in FERB's own question's card (five or fewer at the day's
/// first open) or in the sheet the day's note opens (the phone has no Memory page). His typed answer still works.
/// Every word (the parts' titles and lines, the groups, the notes, the buttons) is the Mac's; the phone only lays them
/// out. Memory only: nothing here is kept on the phone, and it goes when the app locks. (Android: the same.)
@MainActor
@Observable
public final class ImportReview {
    /// The review as the Mac last gave it (nil before it was read, or after the lock).
    public internal(set) var review: MemoryReview?
    /// The lines the owner unticked: the exceptions Approve all declines. Only lines still listed.
    public internal(set) var unticked: Set<String> = []

    public init() {}

    /// The list's parts as drawn now.
    public var parts: [ReviewLayout.Part] { review.map { ReviewLayout.parts($0) } ?? [] }

    /// Every line's id, in the order shown.
    public var ids: [String] { parts.flatMap(\.ids) }

    public func isTicked(_ id: String) -> Bool { !unticked.contains(id) }

    /// One tap on a line: ticked lines are approved by Approve all, unticked ones declined.
    public func toggle(_ id: String) {
        if unticked.contains(id) { unticked.remove(id) } else { unticked.insert(id) }
    }

    /// The main button's words now: Approve all, "Approve 2" while some are unticked, nil when none is ticked.
    public var approveLabel: String? {
        guard let r = review else { return nil }
        return reviewApproveLabel(review: r, ticked: UInt32(ids.filter(isTicked).count))
    }

    /// Decline all's words (the Mac's).
    public var declineLabel: String? { review.flatMap { $0.all.count > 1 ? $0.all[1].label : nil } }

    /// The line above the items (the sheet's; the card's question says it).
    public var hint: String? { review.flatMap { $0.hint.isEmpty ? nil : $0.hint } }

    func show(_ r: MemoryReview) {
        review = r
        unticked.formIntersection(ReviewLayout.parts(r).flatMap(\.ids))
    }

    func locked() {
        review = nil
        unticked = []
    }
}

/// How the list lays the review out: each part's title, its line, its groups by topic and every line (all shown:
/// Approve all answers only what he can see).
public enum ReviewLayout {
    public struct Item: Equatable, Identifiable, Sendable {
        public let id: String
        public let text: String
        /// The small line under it ("Last said …"), nil when the Mac gave none.
        public let note: String?
    }

    public struct Group: Equatable, Identifiable, Sendable {
        /// The part and the group's place in it (a group's title may be empty).
        public let id: String
        /// nil when the Mac gave none.
        public let title: String?
        public let items: [Item]
    }

    public struct Part: Equatable, Identifiable, Sendable {
        /// The part's name (always_on, still_true, offers).
        public let id: String
        public let title: String
        public let hint: String?
        public let groups: [Group]
        /// Every line's id of the part.
        public let ids: [String]
    }

    public static func parts(_ r: MemoryReview) -> [Part] {
        r.parts.map { p in
            let groups = p.groups.enumerated().compactMap { i, g -> Group? in
                if g.items.isEmpty { return nil }
                return Group(id: "\(p.part)/\(i)", title: words(g.title),
                             items: g.items.map { Item(id: $0.id, text: $0.text, note: words($0.note)) })
            }
            return Part(id: p.part, title: p.title, hint: words(p.hint), groups: groups,
                        ids: p.groups.flatMap { $0.items.map(\.id) })
        }
    }

    private static func words(_ s: String) -> String? {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}

extension AppModel {
    /// The review's card or sheet showed: the review now (its unlock asked if the Mac's window closed). The list on
    /// screen stays until the answer comes; a failure leaves it as it was.
    @discardableResult
    public func loadMemoryReview() -> Task<Void, Never> {
        busy("review") { [self] in
            if session.locked || api.link?.phase != .online { return }
            switch await unlocker.run({ await api.memoryReview() }) {
            case .ok(let r): review.show(r)
            case .failed(let e): failed(e)
            }
        }
    }

    /// Approve all (`approve`: every ticked line its part's yes, every unticked one its no) or Decline all (every line
    /// its no), in one call. The Mac's reply replaces the list (answered lines leave it); a failure leaves it as it was
    /// and says why.
    @discardableResult
    public func answerMemoryReviewAll(approve: Bool) -> Task<Void, Never> {
        busy("review", tap: true) { [self] in
            let ids = review.ids
            let yes = approve ? ids.filter(review.isTicked) : []
            let no = approve ? ids.filter { !review.isTicked($0) } : ids
            switch await unlocker.run({ await api.answerMemoryReviewAll(approve: yes, decline: no) }) {
            case .ok(let r):
                review.show(r)
                // everything answered: the Mac closes its question; the chat catches up
                if r.parts.isEmpty { refresh() }
            case .failed(let e): failed(e)
            }
        }
    }

    /// The day's note's Review (MEM-485): the review in a sheet, read now.
    public func openReview() {
        show(.review)
        loadMemoryReview()
    }
}
