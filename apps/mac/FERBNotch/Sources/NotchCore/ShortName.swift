import Foundation

/// A task's, schedule's, reminder's or timer's heading (T-1055, owner D32): the short name the daemon sends (`name`),
/// else the instruction's first clause cut at ~40 characters with "…", as the daemon cuts it (`names.shown_name`). The
/// full instruction shows only where the owner opens the item in the FERB window, never in a heading.
public enum ShortName {
    public static let cut = 40

    /// The heading of a daemon row: its `name`, else its `fallback` field (`goal`, `request`, `message`) cut.
    public static func of(_ row: [String: Any], fallback key: String = "goal") -> String {
        let name = (row["name"] as? String ?? "").split(whereSeparator: \.isWhitespace).joined(separator: " ")
        return name.isEmpty ? clause(row[key] as? String ?? "") : name
    }

    /// The first clause of `text` (up to a sentence end, a semicolon or colon, ", " or a spaced dash), at most `limit`
    /// characters, cut at a word with "…"; no closing punctuation. Structure only: no word is judged.
    public static func clause(_ text: String, limit: Int = cut) -> String {
        let words = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        var end = words.endIndex
        var index = words.startIndex
        while index < words.endIndex {
            let next = words.index(after: index)
            let char = words[index]
            if ".;:!?".contains(char), next < words.endIndex, words[next] == " " { end = index; break }
            if char == ",", next < words.endIndex, words[next] == " " { end = index; break }
            if char == " ", next < words.endIndex, "—–-".contains(words[next]) {
                let after = words.index(after: next)
                if after < words.endIndex, words[after] == " " { end = index; break }
            }
            index = next
        }
        let trim: (Substring) -> String = { String($0).trimmingCharacters(in: CharacterSet(charactersIn: " .,;:!?")) }
        let first = trim(words[..<end])
        guard first.count > limit else { return first }
        let head = String(first.prefix(limit - 1))
        let atWord = head.range(of: " ", options: .backwards).map { String(head[..<$0.lowerBound]) } ?? head
        return atWord.trimmingCharacters(in: CharacterSet(charactersIn: " .,;:!?(")) + "…"
    }
}
