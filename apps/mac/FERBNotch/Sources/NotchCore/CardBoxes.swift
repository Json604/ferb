import Foundation

/// One box per timer, reminder and schedule (APP-769, owner 2026-11-06: "why not use the same??? this is for other
/// schedules and other type of boxes too"; "Moves to the bottom"; "One box, latest run"). A box is set in one reply
/// (its event card) and delivered later (a timer's or reminder's arrival, each run of a schedule: its reminder card).
/// Within the chat a day shows, the box draws only at the last entry that carries it, so when it goes off it shows at
/// the bottom in its done or delivered state and its earlier places show nothing of it. The text beside it stays
/// (the owner's turn, whatever else FERB said there, a run's result); only the card leaves. Read from the saved
/// archive alone, so a restored chat and a restart draw the same one box.
public enum CardBoxes {
    /// The cards an entry of the chat draws: a turn's receipts and cards (its answer's, or under its task card), a
    /// late reply's turn's cards, a notice's cards.
    public static func cards(of entry: TranscriptEntry) -> [ResultCard] {
        switch entry {
        case let .turn(turn, _): return (turn.receiptCards ?? []) + (turn.cards ?? [])
        case let .reply(_, turn): return turn.cards ?? []
        case let .notice(notice): return notice.cards ?? []
        }
    }

    /// For each entry, the boxes it leaves out because a later entry of the same chat draws them.
    public static func moved(_ entries: [TranscriptEntry]) -> [UUID: Set<String>] {
        var last: [String: Int] = [:]
        for (index, entry) in entries.enumerated() {
            for box in cards(of: entry).compactMap(\.box) { last[box] = index }
        }
        var out: [UUID: Set<String>] = [:]
        for (index, entry) in entries.enumerated() {
            let gone = Set(cards(of: entry).compactMap(\.box).filter { (last[$0] ?? index) > index })
            if !gone.isEmpty { out[entry.id] = gone }
        }
        return out
    }

    /// `cards` without those whose box shows later.
    public static func shown(_ cards: [ResultCard], hiding moved: Set<String>) -> [ResultCard] {
        moved.isEmpty ? cards : cards.filter { $0.box.map { !moved.contains($0) } ?? true }
    }

    /// Whether a notice's text is said by its box: a timer's or reminder's arrival (one reminder card that isn't a
    /// schedule's run) with no quotes or proposals. Its separate bubble isn't drawn ("Your 10-second timer is up.");
    /// a reminder's own words show inside its box (`words`).
    public static func boxSays(_ notice: ConversationNotice) -> Bool {
        guard let cards = notice.cards, cards.count == 1, let card = cards.first, card.type == "reminder",
              card.kind != "schedule" else { return false }
        return notice.quotes.isEmpty && (notice.proposals ?? []).isEmpty
    }

    /// The words a reminder's box carries under its title: the reminder's message when it says more than the title.
    /// A timer's message only says its time is up, which the box says ("Done"): none.
    public static func words(_ notice: ConversationNotice) -> String? {
        guard boxSays(notice), let card = notice.cards?.first, !card.isTimer else { return nil }
        let text = notice.text.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty || text == card.title ? nil : text
    }
}
