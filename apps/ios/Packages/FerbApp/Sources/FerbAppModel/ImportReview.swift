import FerbCore
import FerbMobile
import Foundation
import Observation

/// The owner's review of what an import brought in (kb MEM-460..457): FERB's own question (`memory.review`) whose card
/// is a list he answers item by item or a part at once; his typed answer still works. Every word (the parts' titles
/// and lines, the groups, the notes, the buttons) is the Mac's; the phone only lays them out. Memory only: nothing
/// here is kept on the phone, and it goes when the app locks. (Android: the same card from the same library calls.)
@MainActor
@Observable
public final class ImportReview {
    /// The review as the Mac last gave it (nil before it was read, or after the lock).
    public internal(set) var review: MemoryReview?
    /// The groups the owner opened whole ("Show all N"), by `ReviewLayout.Group.id`.
    public internal(set) var opened: Set<String> = []

    public init() {}

    /// The card's parts as drawn now.
    public var parts: [ReviewLayout.Part] { review.map { ReviewLayout.parts($0, opened: opened) } ?? [] }

    /// "Show all N" / "Show less" on one group.
    public func toggle(_ group: String) {
        if opened.contains(group) { opened.remove(group) } else { opened.insert(group) }
    }

    func locked() {
        review = nil
        opened = []
    }
}

/// How the card lays the review out: each part's title, its line, its groups (a long group folded to its first
/// `folded` items until opened), each item's buttons (the part's answers) and the part's buttons for all of them.
public enum ReviewLayout {
    /// A group longer than this shows its first `folded` items and "Show all N".
    public static let folded = 3

    public struct Item: Equatable, Identifiable, Sendable {
        public let id: String
        public let text: String
        /// The small line under it ("Already always on", "Last said …"), nil when the Mac gave none.
        public let note: String?
        /// Its buttons: its own where the Mac gives them (Keep / Remove for one already always on, MEM-464), else the
        /// part's answers.
        public let answers: [ReviewButton]
    }

    public struct Group: Equatable, Identifiable, Sendable {
        /// The part and the group's place in it (a group's title may be empty).
        public let id: String
        /// nil when the Mac gave none.
        public let title: String?
        /// The items shown now.
        public let items: [Item]
        /// How many the group holds.
        public let total: Int
        /// The words of the button under it ("Show all N" folded, "Show less" opened), nil for a short group.
        public let toggle: String?
        /// The group's own buttons for all of its items at once (Keep all / Remove all, MEM-464), usually none.
        public let all: [ReviewButton]
        /// Every item's id of the group (shown or folded): what its "all" buttons answer.
        public let ids: [String]
    }

    public struct Part: Equatable, Identifiable, Sendable {
        /// The part's name, what an answer names (always_on, still_true, offers).
        public let id: String
        public let title: String
        public let hint: String?
        public let groups: [Group]
        /// The buttons under the part, each answering every item of it (none for a part of one item).
        public let all: [ReviewButton]
        /// Every item's id of the part (shown or folded): what an "all" button answers.
        public let ids: [String]
    }

    public static func parts(_ r: MemoryReview, opened: Set<String>) -> [Part] {
        r.parts.map { p in
            let groups = p.groups.enumerated().compactMap { i, g -> Group? in
                if g.items.isEmpty { return nil }
                let id = "\(p.part)/\(i)"
                let long = g.items.count > folded
                let open = opened.contains(id)
                let shown = long && !open ? Array(g.items.prefix(folded)) : g.items
                let items = shown.map { Item(id: $0.id, text: $0.text, note: words($0.note), answers: $0.answers.isEmpty ? p.answers : $0.answers) }
                return Group(id: id, title: words(g.title), items: items, total: g.items.count,
                             toggle: long ? (open ? L.s("show_less") : L.s("review_show_all", g.items.count)) : nil,
                             all: g.all, ids: g.items.map(\.id))
            }
            let ids = p.groups.flatMap { $0.items.map(\.id) }
            // all at once only where there is more than one (Android's MemoryReviewList, the same)
            return Part(id: p.part, title: p.title, hint: words(p.hint), groups: groups, all: ids.count > 1 ? p.all : [],
                        ids: ids)
        }
    }

    private static func words(_ s: String) -> String? {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}

extension AppModel {
    /// The review's card showed: the review now (its unlock asked if the Mac's window closed). The list on screen stays
    /// until the answer comes; a failure leaves it as it was.
    @discardableResult
    public func loadMemoryReview() -> Task<Void, Never> {
        busy("review") { [self] in
            if session.locked || api.link?.phase != .online { return }
            switch await unlocker.run({ await api.memoryReview() }) {
            case .ok(let r): review.review = r
            case .failed(let e): failed(e)
            }
        }
    }

    /// One of the review's buttons: `itemIds` (one item, or every item of the part for an "all" button) answered with
    /// `answer`. The review as the Mac has it after the answer replaces the list (answered items leave it); a failure
    /// leaves the list as it was and says why.
    @discardableResult
    public func answerMemoryReview(part: String, itemIds: [String], answer: String) -> Task<Void, Never> {
        busy("review", tap: true) { [self] in
            switch await unlocker.run({ await api.answerMemoryReview(part: part, itemIds: itemIds, answer: answer) }) {
            case .ok(let r):
                review.review = r
                // everything answered: the Mac closes its question; the chat catches up
                if r.parts.isEmpty { refresh() }
            case .failed(let e): failed(e)
            }
        }
    }
}
