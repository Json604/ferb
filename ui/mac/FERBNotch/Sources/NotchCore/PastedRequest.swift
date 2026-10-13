import Foundation

/// Pasted text stays data. If edits make its original range ambiguous, ask the owner about the whole draft.
public struct PastedRequest {
    public let markedText: String
    public let isEntirelyPasted: Bool
    public init(text: String, pastes: [String], editedAfterPaste: Bool = false) {
        if editedAfterPaste && !pastes.isEmpty {
            markedText = Self.wrap(text)
            isEntirelyPasted = true
            return
        }
        let retained = pastes.filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        var ranges: [Range<String.Index>] = []
        for paste in retained {
            var searchStart = text.startIndex
            var located: Range<String.Index>?
            while searchStart < text.endIndex,
                  let range = text.range(of: paste, range: searchStart..<text.endIndex) {
                if !ranges.contains(where: { $0.overlaps(range) }) { located = range; break }
                searchStart = range.upperBound
            }
            guard let located else {
                // The owner edited/replaced a paste. Never silently upgrade that text to owner authority.
                markedText = Self.wrap(text)
                isEntirelyPasted = true
                return
            }
            ranges.append(located)
        }
        ranges.sort { $0.lowerBound < $1.lowerBound }
        var position = text.startIndex
        var marked = ""
        var ownerText = ""
        for range in ranges {
            let before = String(text[position..<range.lowerBound])
            ownerText += before
            marked += before + Self.wrap(String(text[range]))
            position = range.upperBound
        }
        let remaining = String(text[position...])
        ownerText += remaining
        markedText = marked + remaining
        isEntirelyPasted = !ranges.isEmpty && ownerText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
    private static func wrap(_ text: String) -> String {
        // A clipboard escape sequence cannot forge the end of the provenance envelope.
        "\u{1b}[200~" + text.replacingOccurrences(of: "\u{1b}", with: "\\u001b") + "\u{1b}[201~"
    }
}
