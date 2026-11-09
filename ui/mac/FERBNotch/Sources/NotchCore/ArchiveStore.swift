import Foundation

// APP-801: the saved chat is read tolerantly, so an archive another build saved (an older one, without a key added
// since; a newer one, with a value this build doesn't know) loses at most the element it can't read, never the whole
// history. 2026-11-08: a35676cf added `ResultCard.changed: Bool = false`; synthesized Codable ignores the default, so
// every card saved before it threw keyNotFound, the whole archive failed to decode and the owner's 11-04..11-08 chat
// was set aside unread.

/// What one tolerant read of the chat left out: each element it couldn't read, and the first few reasons.
public final class ArchiveDropReport: @unchecked Sendable {
    public private(set) var count = 0
    public private(set) var reasons: [String] = []

    func record(_ error: Error, at path: [CodingKey]) {
        count += 1
        guard reasons.count < 5 else { return }
        let place = path.map { $0.intValue.map { "[\($0)]" } ?? $0.stringValue }.joined(separator: ".")
        reasons.append(place + ": " + Self.describe(error))
    }

    /// The decoding error's kind and the key it names, never the value it read.
    private static func describe(_ error: Error) -> String {
        guard let error = error as? DecodingError else { return "unreadable" }
        switch error {
        case let .keyNotFound(key, _): return "missing \(key.stringValue)"
        case let .typeMismatch(type, _): return "not a \(type)"
        case let .valueNotFound(type, _): return "no \(type)"
        case .dataCorrupted: return "unreadable value"
        @unknown default: return "unreadable"
        }
    }
}

extension CodingUserInfoKey {
    /// The `ArchiveDropReport` a tolerant read counts into.
    static let archiveDrops = CodingUserInfoKey(rawValue: "ferb.archiveDrops")!
}

/// A value that may fail to read: it is then nil, and counted in the read's report, so only it is lost.
struct Lossy<Value: Decodable>: Decodable {
    let value: Value?

    init(from decoder: Decoder) throws {
        do {
            // Through a container, so JSONDecoder's own handling of dates, URLs and data applies.
            value = try decoder.singleValueContainer().decode(Value.self)
        } catch {
            value = nil
            (decoder.userInfo[.archiveDrops] as? ArchiveDropReport)?.record(error, at: decoder.codingPath)
        }
    }
}

extension KeyedDecodingContainer {
    /// The key's value, or `fallback` when the key is absent or null: an archive saved before the key existed.
    func value<T: Decodable>(_ key: Key, or fallback: @autoclosure () -> T) throws -> T {
        try decodeIfPresent(T.self, forKey: key) ?? fallback()
    }

    /// A nested value that can't be read is left out (and counted), never the element holding it.
    func lossy<T: Decodable>(_ type: T.Type, _ key: Key) throws -> T? {
        try decodeIfPresent(Lossy<T>.self, forKey: key)?.value
    }

    /// A list whose unreadable elements are left out (and counted); nil when the key is absent.
    func lossyList<T: Decodable>(_ type: T.Type, _ key: Key) throws -> [T]? {
        try decodeIfPresent([Lossy<T>].self, forKey: key)?.compactMap(\.value)
    }
}

/// An archive as read back: the entries this build could read, and what it left out.
public struct ArchiveRead {
    public let archive: ConversationArchive
    public let dropped: Int
    public let reasons: [String]
}

extension ConversationArchive {
    /// Reads a saved archive tolerantly (APP-801). Throws only when the data isn't an archive at all.
    public static func read(_ data: Data) throws -> ArchiveRead {
        let report = ArchiveDropReport()
        let decoder = JSONDecoder()
        decoder.userInfo[.archiveDrops] = report
        let archive = try decoder.decode(ConversationArchive.self, from: data)
        return ArchiveRead(archive: archive, dropped: report.count, reasons: report.reasons)
    }
}

/// Where FERB.app kept the chat before FERB's database held it (APP-805, APP-808), and the copies of it set aside, never
/// deleted (APP-801).
public enum ArchiveStore {
    public static let archiveKey = "FERBConversationArchive"
    /// An archive this build couldn't read at all, kept unchanged under this prefix and the time; each launch tries
    /// it again (`recover`).
    public static let unreadablePrefix = archiveKey + ".unreadable-"
    /// The original bytes of an archive read with entries left out, kept unchanged under this prefix and the time.
    public static let partialPrefix = archiveKey + ".partial-"
    /// An unreadable copy whose entries `recover` merged back, kept unchanged under this prefix and its own time.
    public static let restoredPrefix = archiveKey + ".restored-"

    public struct Loaded {
        /// The chat read back; nil when there was none or it couldn't be read at all.
        public var archive: ConversationArchive?
        /// Saved history couldn't be set aside, so nothing may be saved over it.
        public var blocked = false
        /// What happened, for the log (counts and keys, never chat text).
        public var log: [String] = []
    }

    static func stamp(_ date: Date) -> String { ISO8601DateFormatter().string(from: date) }

    /// Keeps `data` under `key`; true once it reads back unchanged.
    static func keep(_ data: Data, as key: String, in defaults: UserDefaults) -> Bool {
        defaults.set(data, forKey: key)
        return defaults.data(forKey: key) == data
    }

    /// Reads the saved chat. An archive with entries this build can't read keeps its original bytes aside and the rest
    /// is saved back; one it can't read at all is kept aside and a new chat starts. Saved history is never written
    /// over unless its bytes were kept first.
    public static func load(_ defaults: UserDefaults, now: Date = Date()) -> Loaded {
        var loaded = Loaded()
        guard let data = defaults.data(forKey: archiveKey) else { return loaded }
        guard let read = try? ConversationArchive.read(data) else {
            let aside = unreadablePrefix + stamp(now)
            if keep(data, as: aside, in: defaults) {
                defaults.removeObject(forKey: archiveKey)
                loaded.log.append("the saved chat couldn't be read; kept unchanged as \(aside)")
            } else {
                loaded.blocked = true
                loaded.log.append("the saved chat couldn't be read or kept aside; nothing will be saved over it")
            }
            return loaded
        }
        loaded.archive = read.archive
        guard read.dropped > 0 else { return loaded }
        let aside = partialPrefix + stamp(now)
        if keep(data, as: aside, in: defaults), let rest = try? JSONEncoder().encode(read.archive.forStorage()) {
            defaults.set(rest, forKey: archiveKey)
            loaded.log.append("left out \(read.dropped) unreadable entries of the saved chat (\(read.reasons.joined(separator: "; "))); "
                              + "the original is kept as \(aside)")
        } else {
            loaded.blocked = true
            loaded.log.append("left out \(read.dropped) unreadable entries but couldn't keep the original aside; "
                              + "nothing will be saved over it")
        }
        return loaded
    }

    /// One-time recovery (APP-801): each archive kept aside as unreadable that this build can read is merged into
    /// `archive` (turns, replies and notices it doesn't have, by date; nothing of `archive` changes), the result saved,
    /// and the copy renamed to `restoredPrefix` with its bytes unchanged. A copy still unreadable stays as it is.
    /// Running it again changes nothing. Returns what happened, for the log.
    @discardableResult
    public static func recover(into archive: inout ConversationArchive, _ defaults: UserDefaults,
                               now: Date = Date()) -> [String] {
        var log: [String] = []
        let keys = defaults.dictionaryRepresentation().keys.filter { $0.hasPrefix(unreadablePrefix) }.sorted()
        for key in keys {
            guard let data = defaults.data(forKey: key), let read = try? ConversationArchive.read(data) else {
                log.append("\(key) still can't be read; left as it is")
                continue
            }
            var older = read.archive
            older.interruptRestoredDrafts()   // as its own launch would have shown them
            let added = archive.restore(older)
            if added > 0 {
                guard let saved = try? JSONEncoder().encode(archive.forStorage()), keep(saved, as: archiveKey, in: defaults)
                else {
                    log.append("couldn't save the chat with \(key)'s entries; \(key) is left as it is")
                    return log
                }
            }
            var restored = restoredPrefix + key.dropFirst(unreadablePrefix.count)
            if let there = defaults.data(forKey: restored), there != data { restored += "-" + stamp(now) }
            guard keep(data, as: restored, in: defaults) else {
                log.append("couldn't rename \(key); it stays")
                continue
            }
            defaults.removeObject(forKey: key)
            log.append("restored \(added) entries from \(key)"
                       + (read.dropped > 0 ? ", leaving out \(read.dropped) unreadable ones" : "") + "; kept as \(restored)")
        }
        return log
    }
}
