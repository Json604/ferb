import Foundation

/// Pair all as it happens (HAND-412, `setup.browsers_pairing`'s events): every owner profile it pairs, each with its
/// state as results land, and every profile it skipped with the owner's step. The tour and Settings show it live.
public struct PairProgress: Equatable {
    public enum State: String, Equatable, Sendable { case pairing, paired, failed, skipped }

    public struct Row: Equatable, Identifiable, Sendable {
        public var id: String { browser + "/" + folder }
        public let browser: String
        public let label: String
        public let folder: String
        public let name: String
        public var state: State
        /// Why it didn't pair and the owner's step (the daemon's words); "" while pairing or once paired.
        public var why: String
        /// FERB's Web Store page adds the extension here in one click (removed, not added, or waiting for a restart).
        public var canAdd: Bool
        public var title: String { "\(label) “\(name)”" }
    }

    public private(set) var rows: [Row] = []
    /// The owner's profiles, FERB's own left out.
    public private(set) var profiles = 0
    public private(set) var finished = false
    /// The whole reply, once `done` came (Pair all's unary reply, for SetupModel.pairAllMessage).
    public private(set) var reply: [String: Any]?

    public init() {}

    /// Profiles whose pairing page is open and not answered yet.
    public var pairing: [Row] { rows.filter { $0.state == .pairing } }
    /// Profiles Pair all opened a pairing page in.
    public var tried: Int { rows.filter { $0.state != .skipped }.count }
    /// Of those, the ones whose result has landed.
    public var landed: Int { rows.filter { $0.state == .paired || $0.state == .failed }.count }
    public var allPaired: Bool { finished && rows.allSatisfy { $0.state == .paired } }

    public mutating func apply(_ event: [String: Any]) {
        switch event["type"] as? String {
        case "plan":
            profiles = event["profiles"] as? Int ?? 0
            rows = (event["todo"] as? [[String: Any]] ?? []).compactMap { Self.row($0, .pairing) }
                + (event["skipped"] as? [[String: Any]] ?? []).compactMap { Self.row($0, .skipped) }
        case "result":
            guard let row = Self.row(event, event["paired"] as? Bool == true ? .paired : .failed) else { return }
            if let index = rows.firstIndex(where: { $0.id == row.id }) { rows[index] = row } else { rows.append(row) }
        case "done":
            finished = true
            var whole = event
            whole.removeValue(forKey: "type")
            reply = whole
        default: break
        }
    }

    /// One profile's row changed outside Pair all (added from the store page, then paired on its own).
    public mutating func set(_ id: String, state: State, why: String = "", canAdd: Bool = false) {
        guard let index = rows.firstIndex(where: { $0.id == id }) else { return }
        rows[index].state = state
        rows[index].why = why
        rows[index].canAdd = canAdd
    }

    public static func == (a: PairProgress, b: PairProgress) -> Bool {
        a.rows == b.rows && a.profiles == b.profiles && a.finished == b.finished
    }

    private static func row(_ value: [String: Any], _ state: State) -> Row? {
        guard let browser = value["browser"] as? String, let folder = value["folder"] as? String,
              !browser.isEmpty, !folder.isEmpty else { return nil }
        let why = state == .skipped ? value["step"] as? String : value["why"] as? String
        return Row(browser: browser, label: value["label"] as? String ?? browser, folder: folder,
                   name: value["name"] as? String ?? folder, state: state,
                   why: state == .paired || state == .pairing ? "" : (why ?? ""),
                   canAdd: value["can_add"] as? Bool ?? false)
    }
}
