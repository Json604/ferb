import Foundation

/// FERB's review of what an import brought in (kb MEM-460..457): one list the owner answers item by item or all at
/// once — keep in mind all the time (Approve / Decline), still true (Yes / No, not any more / Not sure), still want help
/// (Yes / Done already / No thanks). The words, parts, groups and buttons are the daemon's (`memory.review`); the app
/// only lays them out. The chat's card (FERB's own question, capability `memory.review`) and the Memory page share one
/// model, so an answer in either shows in both.
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

    public static let empty = MemoryReview(parts: [], count: 0)
    /// The question's capability: a question with it is drawn as this list.
    public static let capability = "memory.review"
    /// A group shows this many items until opened.
    public static let shownAtFirst = 3

    public init(parts: [Part], count: Int) {
        self.parts = parts
        self.count = count
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
    }

    /// "Show all 5" / "Show less": the two words the app adds (a group of more than three).
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
    private let rpc: DaemonRPCTransport
    /// Told when the daemon's list comes back with nothing left: the review's question is closed then (MEM-460).
    public var emptied: (() -> Void)?

    public init(rpc: DaemonRPCTransport, review: MemoryReview = .empty) {
        self.rpc = rpc
        self.review = review
    }

    public func load() async {
        do {
            review = MemoryReview(try await rpc.call("memory.review", params: [:]))
            problem = nil
            if review.parts.isEmpty { emptied?() }
        } catch {
            problem = error.localizedDescription
        }
    }

    /// One item's answer (`ids` of one), or all of a part's or a group's (`all`).
    public func answer(_ part: MemoryReview.Part, ids: [String], with button: MemoryReview.Button, all: Bool = false) async {
        let key = part.part + ":" + (all ? "" : ids.joined(separator: ","))
        sending.insert(key)
        defer { sending.remove(key) }
        do {
            let reply = try await rpc.call("memory.review.answer",
                                           params: ["part": part.part, "item_ids": ids, "answer": button.answer])
            review = MemoryReview(reply)
            problem = nil
            if review.parts.isEmpty { emptied?() }
        } catch {
            problem = error.localizedDescription
        }
    }

    public func isSending(_ part: MemoryReview.Part, id: String?) -> Bool {
        sending.contains(part.part + ":" + (id ?? "")) || sending.contains(part.part + ":")
    }
}
