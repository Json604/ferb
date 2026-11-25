import Foundation

/// FERB's review of what it learned about the owner (kb MEM-460..457, MEM-485, MEM-486): one list, grouped by topic in
/// its parts (keep in mind all the time, still true, still want help), each line ticked until he unticks it, answered
/// all at once: Approve all gives every ticked line its part's yes and every unticked one its no; Decline all gives
/// every line its no. The words, parts, groups and buttons are the daemon's (`memory.review`); the app only lays them
/// out. The chat's card (FERB's own question, capability `memory.review`, asked at the day's first open with five or
/// fewer waiting) and the Memory page's To check share one model, so ticks and answers in either show in both.
public struct MemoryReview: Equatable {
    public struct Button: Equatable, Hashable {
        public let answer: String
        public let label: String
    }

    public struct Item: Equatable, Identifiable {
        public let id: String
        public let text: String
        public let note: String
        /// Its own buttons when they differ from its part's (already always on: Keep / Remove, MEM-464).
        public var answers: [Button] = []
        public func buttons(in part: Part) -> [Button] { answers.isEmpty ? part.answers : answers }
    }

    public struct Group: Equatable {
        public let title: String
        public let items: [Item]
        /// Its own buttons for all of its items (Keep all / Remove all, MEM-464); usually none.
        public var all: [Button] = []
        public var ids: [String] { items.map(\.id) }
    }

    public struct Part: Equatable, Identifiable {
        public var id: String { part }
        public let part: String
        public let title: String
        public let hint: String
        public let groups: [Group]
        public let answers: [Button]
        public let all: [Button]
        /// Every item's id, for an answer to all of them at once.
        public var ids: [String] { groups.flatMap { $0.items.map(\.id) } }
    }

    public let parts: [Part]
    public let count: Int
    /// The whole list's buttons, at its top (MEM-486): Approve all, Decline all.
    public var all: [Button] = []
    /// The main button's words while some lines are unticked: "Approve {n}".
    public var some: String = ""
    /// The line above the items ("Untick anything that isn't right.").
    public var hint: String = ""

    public static let empty = MemoryReview(parts: [], count: 0)
    /// The question's capability: a question with it is drawn as this list.
    public static let capability = "memory.review"
    /// The chat's notes list shows this many until opened (the review list shows every line: Approve all answers
    /// only what he can see).
    public static let shownAtFirst = 3

    public init(parts: [Part], count: Int, all: [Button] = [], some: String = "", hint: String = "") {
        self.parts = parts
        self.count = count
        self.all = all
        self.some = some
        self.hint = hint
    }

    /// The daemon's answer, read leniently: a missing field is empty.
    public init(_ reply: [String: Any]) {
        func words(_ v: [String: Any], _ k: String) -> String { v[k] as? String ?? "" }
        func buttons(_ v: Any?) -> [Button] {
            (v as? [[String: Any]] ?? []).map { Button(answer: words($0, "answer"), label: words($0, "label")) }
        }
        parts = (reply["parts"] as? [[String: Any]] ?? []).map { p in
            Part(part: words(p, "part"), title: words(p, "title"), hint: words(p, "hint"),
                 groups: (p["groups"] as? [[String: Any]] ?? []).map { g in
                     Group(title: words(g, "title"),
                           items: (g["items"] as? [[String: Any]] ?? []).map {
                               Item(id: words($0, "id"), text: words($0, "text"), note: words($0, "note"),
                                    answers: buttons($0["answers"]))
                           },
                           all: buttons(g["all"]))
                 },
                 answers: buttons(p["answers"]), all: buttons(p["all"]))
        }
        count = (reply["count"] as? NSNumber)?.intValue ?? 0
        all = buttons(reply["all"])
        some = words(reply, "some")
        hint = words(reply, "hint")
    }

    /// Every line's id, in the order shown.
    public var ids: [String] { parts.flatMap(\.ids) }

    /// The main button's words with `ticked` lines ticked: Approve all, "Approve 2" while some are unticked, nil when
    /// none is (only Decline all is left).
    public func approveLabel(ticked: Int) -> String? {
        guard ticked > 0, let first = all.first else { return nil }
        if ticked >= ids.count || some.isEmpty { return first.label }
        return some.replacingOccurrences(of: "{n}", with: String(ticked))
    }

    /// "Show all 5" / "Show less": the two words the app adds (a group of more than three; the chat's notes list).
    public static func showAll(_ n: Int) -> String { "Show all \(n)" }
    public static let showLess = "Show less"
}

/// The review's one model: read from the daemon, answered through it, the answer's reply shown at once.
@MainActor
public final class MemoryReviewModel: ObservableObject {
    @Published public private(set) var review: MemoryReview = .empty
    /// Answers on their way, by part and item ("" for all of a part).
    @Published public private(set) var sending: Set<String> = []
    @Published public private(set) var problem: String?
    /// The lines the owner unticked (MEM-486): the exceptions Approve all declines. Only lines still listed count.
    @Published public private(set) var unticked: Set<String> = []
    private let rpc: DaemonRPCTransport
    /// Told when the daemon's list comes back with nothing left: the review's question is closed then (MEM-460).
    public var emptied: (() -> Void)?

    public init(rpc: DaemonRPCTransport, review: MemoryReview = .empty) {
        self.rpc = rpc
        self.review = review
    }

    public func load() async {
        do {
            show(MemoryReview(try await rpc.call("memory.review", params: [:])))
        } catch {
            problem = error.localizedDescription
        }
    }

    private func show(_ next: MemoryReview) {
        review = next
        unticked.formIntersection(next.ids)
        problem = nil
        if review.parts.isEmpty { emptied?() }
    }

    public func isTicked(_ id: String) -> Bool { !unticked.contains(id) }

    /// One tap on a line: ticked lines are approved by Approve all, unticked ones declined.
    public func toggle(_ id: String) {
        if unticked.contains(id) { unticked.remove(id) } else { unticked.insert(id) }
    }

    /// The main button's words now (nil: nothing ticked).
    public var approveLabel: String? { review.approveLabel(ticked: review.ids.filter(isTicked).count) }

    /// Approve all (every ticked line its part's yes, every unticked one its no) or Decline all (every line its no), in
    /// one call (`memory.review.all`). The reply is the list from then on.
    public func answerAll(approve: Bool) async {
        let ids = review.ids
        let yes = approve ? ids.filter(isTicked) : []
        let no = approve ? ids.filter { !isTicked($0) } : ids
        sending.insert(Self.allAnswers)
        defer { sending.remove(Self.allAnswers) }
        do {
            show(MemoryReview(try await rpc.call("memory.review.all", params: ["approve": yes, "decline": no])))
        } catch {
            problem = error.localizedDescription
        }
    }

    static let allAnswers = "*"
    public var sendingAll: Bool { sending.contains(Self.allAnswers) }

    /// The owner opened FERB (the notch by his click or shortcut, or the FERB window): the daemon's first open of the day
    /// shows what waits (MEM-485); later opens do nothing there. A failure is ignored: the next open says it again.
    public func opened() {
        Task { _ = try? await rpc.call("memory.review.daily", params: ["client": "mac"]) }
    }

    /// One item's answer (`ids` of one), or all of a part's or a group's (`all`).
    public func answer(_ part: MemoryReview.Part, ids: [String], with button: MemoryReview.Button, all: Bool = false) async {
        let key = part.part + ":" + (all ? "" : ids.joined(separator: ","))
        sending.insert(key)
        defer { sending.remove(key) }
        do {
            let reply = try await rpc.call("memory.review.answer",
                                           params: ["part": part.part, "item_ids": ids, "answer": button.answer])
            show(MemoryReview(reply))
        } catch {
            problem = error.localizedDescription
        }
    }

    public func isSending(_ part: MemoryReview.Part, id: String?) -> Bool {
        sending.contains(part.part + ":" + (id ?? "")) || sending.contains(part.part + ":")
    }
}
