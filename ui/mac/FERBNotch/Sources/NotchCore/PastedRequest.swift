import Foundation

/// Where pasted text sits in the composer draft, carried through the owner's later edits. Ranges are UTF-16 offsets,
/// as AppKit reports edits. Pasted text stays reference data (MP2) and the words the owner types stay theirs, even
/// typed inside a paste, so nothing has to ask (spec §4.7). A change no edit reported is inferred from the text; when
/// that change touches a paste, which words are pasted is no longer known and the whole draft is sent as data.
public struct PasteProvenance: Equatable {
    /// The draft the ranges describe.
    public private(set) var draft = ""
    public private(set) var pastes: [NSRange] = []
    /// A change no edit reported touched a paste, so which words are pasted is no longer known.
    public private(set) var isUnclear = false

    public init() {}

    /// `text` pasted over `selection` of `current`, the draft just before the paste.
    public mutating func paste(_ text: String, over selection: NSRange, in current: String) {
        edit(selection, with: text, in: current, pasted: true)
    }

    /// An edit the composer reports exactly: `replacement` put over `range` of `current`, the draft before it.
    /// `pasted` marks text that came from outside the owner's typing.
    public mutating func edit(_ range: NSRange, with replacement: String, in current: String, pasted: Bool = false) {
        follow(current)
        guard range.location != NSNotFound, NSMaxRange(range) <= (draft as NSString).length else {
            // The report doesn't fit the draft it describes; nothing is known about the pastes now.
            isUnclear = isUnclear || !pastes.isEmpty
            return
        }
        apply(range, with: replacement, pasted: pasted)
    }

    /// Follows a change no edit reported (dictation, a restored draft) as the smallest change that explains it.
    public mutating func follow(_ current: String) {
        guard current != draft else { return }
        let old = draft as NSString, new = current as NSString
        let shortest = min(old.length, new.length)
        var prefix = 0
        while prefix < shortest, old.character(at: prefix) == new.character(at: prefix) { prefix += 1 }
        var suffix = 0
        while suffix < shortest, old.character(at: old.length - 1 - suffix) == new.character(at: new.length - 1 - suffix) {
            suffix += 1
        }
        let range: NSRange, inserted: Int
        if prefix + suffix >= shortest {
            // A pure insertion or deletion. Across repeated characters it fits several places; take one that leaves
            // every paste whole when there is one, since the characters are the same either way.
            let removed = old.length - shortest
            let starts = max(0, shortest - suffix)...min(prefix, shortest)
            let start = starts.first { !cutsPaste(NSRange(location: $0, length: removed)) } ?? starts.lowerBound
            range = NSRange(location: start, length: removed)
            inserted = new.length - shortest
        } else {
            range = NSRange(location: prefix, length: old.length - prefix - suffix)
            inserted = new.length - prefix - suffix
        }
        // Unreported text could be pasted text moved around, so it may not change any paste: removing one whole by
        // a pure deletion is the only safe exception.
        if !pastes.isEmpty && (cutsPaste(range) || (inserted > 0 && touchesPaste(range))) { isUnclear = true }
        apply(range, with: new.substring(with: NSRange(location: range.location, length: inserted)), pasted: false)
    }

    /// Whether replacing `range` would cut into a paste rather than leave it whole or remove all of it.
    private func cutsPaste(_ range: NSRange) -> Bool {
        pastes.contains { paste in
            if range.length == 0 { return paste.location < range.location && range.location < NSMaxRange(paste) }
            return NSIntersectionRange(paste, range).length > 0 && NSUnionRange(paste, range) != range
        }
    }

    private func touchesPaste(_ range: NSRange) -> Bool {
        pastes.contains { NSIntersectionRange($0, range).length > 0 } || cutsPaste(range)
    }

    /// Replaces `range` and keeps every pasted character marked: a paste cut by the edit keeps its pieces outside it.
    private mutating func apply(_ range: NSRange, with text: String, pasted: Bool) {
        let length = (text as NSString).length, delta = length - range.length
        var kept: [NSRange] = []
        for paste in pastes {
            let before = NSRange(location: paste.location, length: max(0, min(NSMaxRange(paste), range.location) - paste.location))
            let afterStart = max(paste.location, NSMaxRange(range))
            let after = NSRange(location: afterStart + delta, length: max(0, NSMaxRange(paste) - afterStart))
            if before.length > 0 { kept.append(before) }
            if after.length > 0 { kept.append(after) }
        }
        if pasted && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            kept.append(NSRange(location: range.location, length: length))
        }
        pastes = kept.sorted { $0.location < $1.location }
        draft = (draft as NSString).replacingCharacters(in: range, with: text)
        if draft.isEmpty { self = PasteProvenance() }   // nothing is left of any paste
    }
}

/// What is sent for a draft: each paste wrapped as external data in the CLI's bracketed-paste delimiters, or the
/// whole draft when which words are pasted is no longer known.
public struct PastedRequest {
    public let markedText: String

    public init(text: String, provenance: PasteProvenance) {
        var current = provenance
        current.follow(text)
        if current.isUnclear {
            markedText = Self.wrap(text)
            return
        }
        let draft = text as NSString
        var marked = "", position = 0
        for paste in current.pastes {
            marked += draft.substring(with: NSRange(location: position, length: paste.location - position))
            marked += Self.wrap(draft.substring(with: paste))
            position = NSMaxRange(paste)
        }
        markedText = marked + draft.substring(from: position)
    }

    private static func wrap(_ text: String) -> String {
        // A clipboard escape sequence cannot forge the end of the provenance envelope.
        "\u{1b}[200~" + text.replacingOccurrences(of: "\u{1b}", with: "\\u001b") + "\u{1b}[201~"
    }
}
