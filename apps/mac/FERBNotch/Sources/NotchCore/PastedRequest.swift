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
    /// Long pastes held out of the field as chips (owner, 2026-11-17, APP-934), in the order they were pasted. Each is
    /// anchored where it was pasted and follows the edits around it; on send it goes back in at its place, as a paste.
    public private(set) var held: [HeldPaste] = []

    public init() {}

    /// `text` held as a chip at `location` of `current`, the draft at the moment of the paste (any selection the paste
    /// replaced already removed). Its id is the chip's.
    @discardableResult
    public mutating func hold(_ text: String, at location: Int, in current: String, id: UUID = UUID()) -> UUID {
        follow(current)
        let place = max(0, min(location, (draft as NSString).length))
        held.append(HeldPaste(id: id, text: text, location: place))
        return id
    }

    /// The chip's cross: its text is dropped, the typed draft is unchanged.
    public mutating func removeHeld(_ id: UUID) {
        held.removeAll { $0.id == id }
    }

    /// The owner's edit of a chip's text in its editor (owner, 2026-11-25, APP-1143): what is sent from now on, at the
    /// same place and still as a paste. An emptied chip stays until its editor closes (APP-1144).
    public mutating func editHeld(_ id: UUID, to text: String) {
        guard let index = held.firstIndex(where: { $0.id == id }) else { return }
        held[index].text = text
    }

    /// The draft as sent: every held paste put back in at its place, marked as a paste exactly as if it had been pasted
    /// into the field there; the provenance returned describes that text and holds nothing.
    public func expanded(_ text: String) -> (text: String, provenance: PasteProvenance) {
        var current = self
        current.follow(text)
        var full = current.draft
        // Highest place first, so the places still to come are unchanged; at one place the later paste goes in first,
        // which leaves them in the order they were pasted.
        let order = current.held.enumerated().sorted { a, b in
            a.element.location != b.element.location ? a.element.location > b.element.location : a.offset > b.offset
        }
        for (_, paste) in order {
            let at = NSRange(location: min(paste.location, (full as NSString).length), length: 0)
            current.edit(at, with: paste.text, in: full, pasted: true)
            full = (full as NSString).replacingCharacters(in: at, with: paste.text)
        }
        current.held = []
        return (full, current)
    }

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
        // A held paste stays before text put at its own place (it was pasted before the owner typed on), moves with
        // text put or removed before it, and sits where removed text around it was.
        for index in held.indices where held[index].location > range.location {
            let place = held[index].location
            held[index].location = place >= NSMaxRange(range) ? place + delta : range.location
        }
        draft = (draft as NSString).replacingCharacters(in: range, with: text)
        if draft.isEmpty {                              // nothing is left of any paste in the field; chips stay
            pastes = []
            isUnclear = false
        }
    }
}

/// A long paste held as a chip in the composer (APP-934): its whole text and its place in the draft (UTF-16).
public struct HeldPaste: Equatable, Identifiable {
    public let id: UUID
    public internal(set) var text: String
    public internal(set) var location: Int

    public init(id: UUID, text: String, location: Int) {
        self.id = id
        self.text = text
        self.location = location
    }

    /// The chip's size line: "240 lines", or for one long line "5,000 characters".
    public var size: String {
        let lines = LongPaste.lines(in: text)
        if lines > 1 { return "\(lines.formatted()) lines" }
        let count = text.count
        return count == 1 ? "1 character" : "\(count.formatted()) characters"
    }
}

/// When pasted text is held as a chip instead of filling the field (owner, 2026-11-17, APP-934).
public enum LongPaste {
    /// The owner's to confirm: a paste longer than this many characters or lines, whichever comes first, is held as a
    /// chip; anything up to it goes into the field as before.
    public static let limit = (characters: 2_000, lines: 30)

    /// Whether `text` is held as a chip.
    public static func isLong(_ text: String) -> Bool {
        text.count > limit.characters || lines(in: text) > limit.lines
    }

    /// Lines as the field shows them; a final line break doesn't start another.
    public static func lines(in text: String) -> Int {
        guard !text.isEmpty else { return 0 }
        var lines = 1
        var index = text.startIndex
        while index < text.endIndex {
            if text[index].isNewline, text.index(after: index) < text.endIndex { lines += 1 }
            index = text.index(after: index)
        }
        return lines
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
            marked += PasteMarkers.removed(from: draft.substring(with: NSRange(location: position, length: paste.location - position)))
            marked += Self.wrap(draft.substring(with: paste))
            position = NSMaxRange(paste)
        }
        markedText = marked + PasteMarkers.removed(from: draft.substring(from: position))
    }

    private static func wrap(_ text: String) -> String {
        // Markers that leaked into the text go first (APP-713); then a clipboard escape sequence cannot forge the end
        // of the provenance envelope.
        let clean = PasteMarkers.removed(from: text)
        guard !clean.isEmpty else { return "" }
        return "\u{1b}[200~" + clean.replacingOccurrences(of: "\u{1b}", with: "\\u001b") + "\u{1b}[201~"
    }
}

/// Bracketed-paste markers in the owner's message: the composer's own (ESC[200~ … ESC[201~, `PastedRequest`) and
/// ones that leaked in as text, typically copied out of a terminal, with or without the ESC byte. Removed by the
/// daemon's rule (`untrusted.mark`, ORCH-520(3)): a leaked pair around text wherever it sits, a lone leaked marker at a
/// line's start or end. Clean-up only: nothing in the words is read (APP-713).
public enum PasteMarkers {
    private static let composer = try! NSRegularExpression(pattern: "\\x{1b}\\[20[01]~")
    private static let pair = try! NSRegularExpression(pattern: "\\[200~(.*?)\\[201~", options: [.dotMatchesLineSeparators])
    private static let edge = try! NSRegularExpression(pattern: "^(?:[ \\t]*\\[20[01]~)+|(?:\\[20[01]~[ \\t]*)+$",
                                                       options: [.anchorsMatchLines])

    /// `text` without any bracketed-paste marker; text with none comes back unchanged.
    public static func removed(from text: String) -> String {
        guard text.contains("[20") else { return text }
        // The composer's markers split the text into pieces, each cleaned of leaked ones, as the daemon does.
        let whole = text as NSString
        var pieces: [String] = [], start = 0
        for match in composer.matches(in: text, range: NSRange(location: 0, length: whole.length)) {
            pieces.append(whole.substring(with: NSRange(location: start, length: match.range.location - start)))
            start = NSMaxRange(match.range)
        }
        pieces.append(whole.substring(from: start))
        return pieces.map { piece in
            let unpaired = pair.stringByReplacingMatches(in: piece, range: NSRange(location: 0, length: (piece as NSString).length),
                                                         withTemplate: "$1")
            return edge.stringByReplacingMatches(in: unpaired, range: NSRange(location: 0, length: (unpaired as NSString).length),
                                                 withTemplate: "")
        }.joined()
    }
}
