import Foundation

/// What a Command-V in the chat box takes from the clipboard (owner, 2026-11-17, APP-932, APP-933), decided from each
/// item's pasteboard types only, never from what they say:
///
/// - files when every item is a file or folder (one or many copied in Finder). Finder also puts their names as text
///   (and an icon picture): the files win, and the names are not typed;
/// - a picture when the clipboard holds image data and no file (a screenshot copied to the clipboard, a browser's
///   Copy Image). Rich text (RTF, RTFD, a web archive) wins over a picture of itself: a table copied from a spreadsheet
///   or a document carries one beside its text;
/// - otherwise text, as before.
public enum ClipboardContent: Equatable {
    case files
    case image
    case text
    case nothing

    /// The image types FERB reads, in the order it prefers them (PNG is attached as is; the others are made PNG).
    public static let imageTypes = ["public.png", "public.tiff", "public.jpeg", "com.compuserve.gif", "public.heic"]
    /// Text types that carry their own layout: when one is there, the clipboard is text, whatever picture it also has.
    public static let richTextTypes: Set<String> = ["public.rtf", "com.apple.flat-rtfd", "com.apple.rtfd",
                                                    "com.apple.webarchive"]
    public static let textType = "public.utf8-plain-text"

    public static func of(_ items: [[String]]) -> ClipboardContent {
        guard !items.isEmpty else { return .nothing }
        if items.allSatisfy({ $0.contains(FileDrop.fileURLType) }) { return .files }
        let types = Set(items.flatMap { $0 })
        if !types.isDisjoint(with: imageTypes), types.isDisjoint(with: richTextTypes) { return .image }
        return types.contains(textType) || types.contains("NSStringPboardType") ? .text : .nothing
    }
}

/// Files FERB itself receives for the composer: a pasted picture (APP-933), a promised file or a picture dropped on
/// FERB (APP-936). The daemon is sent a file's path, never a copy (`input_paths`), and reads it while the task runs, so
/// a received file must outlive the message:
///
/// - each arrival gets its own folder (a random name) in `root`, so its name stays the source's or a plain one
///   ("Pasted image ….png") and two never collide; the root and its folders are owner-only;
/// - one the owner removes from the composer before sending is deleted at once;
/// - one that was sent stays for its task; at FERB.app's start `sweep` removes only folders older than `keptFor`, so a
///   task still running across a restart keeps its file (macOS also clears an account's temporary folder of files left
///   untouched for days).
public final class ReceivedFiles {
    /// How long a received file is kept after it arrived (APP-936): far longer than a task runs.
    public static let keptFor: TimeInterval = 7 * 24 * 60 * 60

    public let root: URL
    private var unsent = Set<URL>()
    private let files = FileManager.default

    /// The account's own temporary folder, which the background service (same account) and its workers read too
    /// (APP-936). Not FERB's data folder: FERB's tools never read its own state (ferb_state).
    public static var defaultRoot: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("FERB attachments", isDirectory: true)
    }

    public init(root: URL = ReceivedFiles.defaultRoot) {
        self.root = root.standardizedFileURL
    }

    /// A new, empty folder of its own for one arrival.
    public func newFolder() throws -> URL {
        try files.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let folder = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try files.createDirectory(at: folder, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        return folder
    }

    /// Writes `data` as `name` in a new folder of its own and returns the file, waiting in the composer.
    public func save(_ data: Data, named name: String) throws -> URL {
        let folder = try newFolder()
        let url = folder.appendingPathComponent(Self.plainName(name)).standardizedFileURL
        do {
            try data.write(to: url, options: .withoutOverwriting)
        } catch {
            try? files.removeItem(at: folder)
            throw error
        }
        unsent.insert(url)
        return url
    }

    /// A file a source wrote into one of `newFolder`'s folders: it waits in the composer like a saved one.
    public func adopt(_ url: URL) {
        guard owns(url) else { return }
        unsent.insert(url.standardizedFileURL)
    }

    /// Whether `url` is one of FERB's received files.
    public func owns(_ url: URL) -> Bool {
        url.standardizedFileURL.path.hasPrefix(root.path + "/")
    }

    /// The files went with a message: they stay for its task.
    public func sent(_ urls: [URL]) {
        for url in urls { unsent.remove(url.standardizedFileURL) }
    }

    /// The owner took a file out of the composer: an unsent received file is deleted, and its folder once empty.
    public func discard(_ url: URL) {
        let url = url.standardizedFileURL
        guard unsent.remove(url) != nil, owns(url) else { return }
        try? files.removeItem(at: url)
        removeIfEmpty(url.deletingLastPathComponent())
    }

    /// A folder `newFolder` gave that nothing arrived in.
    public func removeIfEmpty(_ folder: URL) {
        let folder = folder.standardizedFileURL
        guard owns(folder), (try? files.contentsOfDirectory(atPath: folder.path))?.isEmpty == true else { return }
        try? files.removeItem(at: folder)
    }

    /// FERB.app started: what arrived longer than `keptFor` before `now` is removed; anything newer may belong to a
    /// task still running.
    public func sweep(now: Date = Date()) {
        guard let names = try? files.contentsOfDirectory(atPath: root.path) else { return }
        for name in names {
            let item = root.appendingPathComponent(name)
            let attributes = try? files.attributesOfItem(atPath: item.path)
            let arrived = attributes?[.modificationDate] as? Date ?? .distantPast
            if now.timeIntervalSince(arrived) > Self.keptFor { try? files.removeItem(at: item) }
        }
    }

    /// A name with no folder separator or leading dot, so it stays one plain file in its folder.
    static func plainName(_ name: String) -> String {
        var clean = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        while clean.hasPrefix(".") { clean.removeFirst() }
        return clean.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Pasted image.png" : clean
    }
}
