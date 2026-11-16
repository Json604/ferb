import Foundation

/// A picked file copied into the app's own storage, ready to send.
public struct Staged: Equatable, Sendable {
    public let path: String
    public let name: String
    public let size: Int64

    public init(path: String, name: String, size: Int64) {
        self.path = path
        self.name = name
        self.size = size
    }
}

/// A folder the owner picked in the system's folder picker: its name and every file under it (its path inside).
/// `root` is the picker's URL, kept open (its security scope) until the folder is sent or dropped.
public struct PickedFolder: Sendable {
    public let name: String
    public let root: URL
    public let files: [PickedFile]
    let scoped: Bool
}

/// One file of a [PickedFolder]: `rel` is its path inside the folder (`Scans/2026/a.pdf`).
public struct PickedFile: Sendable {
    public let rel: String
    public let size: Int64?
    public let url: URL
}

/// Where files live on the phone (PHN-11, PHN-15, PHN-16; kb PHN-515, PHN-516; Android: model/LocalFiles.kt): the
/// app's Caches folder `files`, which no backup carries (and is marked so), every folder made with Data Protection
/// "complete unless open" (unreadable while the phone is locked, except a file being written), and which the system
/// may empty under storage pressure (a fetch then starts over: its offset is the file's length).
/// `fetched/<transfer>/<name>`: files from the Mac, kept at most [maxAge] and removed when the app locks;
/// `outgoing/<id>/<name>`: copies of what the owner picked, until sent. Nothing here hands a file to another app:
/// the screen does that (Quick Look, the share sheet) only on the owner's tap.
public final class LocalFiles: @unchecked Sendable {
    /// PHN-13, PHN-16: FETCHED_MAX_AGE_MS, 24 hours.
    public static let maxAge: TimeInterval = 24 * 60 * 60

    public let root: URL
    private let fm = FileManager.default

    public init(root: URL) {
        self.root = root
    }

    /// The phone's own: Caches/files.
    public static func forPhone() -> LocalFiles {
        LocalFiles(root: FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("files", isDirectory: true))
    }

    public var fetched: URL { root.appendingPathComponent("fetched", isDirectory: true) }
    public var outgoing: URL { root.appendingPathComponent("outgoing", isDirectory: true) }

    /// Makes `dir` (and the root) with the protection class and the root out of backups.
    func make(_ dir: URL) {
        let attrs: [FileAttributeKey: Any] = [.protectionKey: FileProtectionType.completeUnlessOpen]
        if !fm.fileExists(atPath: root.path) {
            try? fm.createDirectory(at: root, withIntermediateDirectories: true, attributes: attrs)
            var r = root
            var v = URLResourceValues()
            v.isExcludedFromBackup = true
            try? r.setResourceValues(v)
        }
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true, attributes: attrs)
    }

    /// Gives a file the folder's protection class (the library writes fetched files itself).
    public func protect(_ path: String) {
        try? fm.setAttributes([.protectionKey: FileProtectionType.completeUnlessOpen], ofItemAtPath: path)
    }

    /// Where a fetch of `name` for transfer `key` is written (a name from the Mac never climbs out).
    public func fetchTarget(_ key: String, _ name: String) -> URL {
        let dir = fetched.appendingPathComponent(key, isDirectory: true)
        make(dir)
        return dir.appendingPathComponent(Self.safe(name))
    }

    /// Where a file of a whole folder fetched as `batch` is written: `fetched/<batch>/<folder>/<rel>`, each part safe.
    public func folderTarget(_ batch: String, _ folder: String, _ rel: String) -> URL {
        let target = fetched.appendingPathComponent(batch, isDirectory: true).appendingPathComponent(Self.safe(folder), isDirectory: true)
            .appendingPathComponent(Self.safeRel(rel))
        make(target.deletingLastPathComponent())
        return target
    }

    /// Where a file of a folder being sent waits, copied just before it goes: `outgoing/<batch>/<rel>`.
    public func sendTarget(_ batch: String, _ rel: String) -> URL {
        outgoing.appendingPathComponent(batch, isDirectory: true).appendingPathComponent(Self.safeRel(rel))
    }

    /// Copies a picked file (a URL the system's picker gave, security-scoped or a copy of its own) into `outgoing`;
    /// nil when it can't be read or is a folder. Blocking: off the main thread.
    public func stage(_ url: URL) -> Staged? {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDir), !isDir.boolValue else { return nil }
        let name = url.lastPathComponent
        let dir = outgoing.appendingPathComponent(UUID().uuidString.lowercased(), isDirectory: true)
        make(dir)
        let out = dir.appendingPathComponent(Self.safe(name))
        guard copy(url, to: out) else {
            try? fm.removeItem(at: dir)
            return nil
        }
        return Staged(path: out.path, name: name, size: size(out.path))
    }

    /// Reads a picked folder (the picker's URL): its name and files, its security scope left open for the copies made
    /// just before each file goes ([release] closes it). Nil when it can't be read. Blocking.
    public func folder(_ url: URL) -> PickedFolder? {
        let scoped = url.startAccessingSecurityScopedResource()
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue,
              let walk = fm.enumerator(at: url, includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey]) else {
            if scoped { url.stopAccessingSecurityScopedResource() }
            return nil
        }
        let base = url.standardizedFileURL.resolvingSymlinksInPath().path
        var files: [PickedFile] = []
        for case let f as URL in walk {
            let v = try? f.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
            guard v?.isRegularFile == true else { continue }
            let full = f.standardizedFileURL.resolvingSymlinksInPath().path
            guard full.hasPrefix(base + "/") else { continue }
            files.append(PickedFile(rel: String(full.dropFirst(base.count + 1)), size: v?.fileSize.map(Int64.init), url: f))
        }
        files.sort { $0.rel < $1.rel }
        return PickedFolder(name: url.lastPathComponent, root: url, files: files, scoped: scoped)
    }

    /// Closes a picked folder's security scope.
    public func release(_ folder: PickedFolder) {
        if folder.scoped { folder.root.stopAccessingSecurityScopedResource() }
    }

    /// Copies one file of a picked folder to `target` (just before it is sent); false when it can't be read. Blocking.
    public func stage(_ source: URL, to target: URL) -> Bool {
        make(target.deletingLastPathComponent())
        return copy(source, to: target)
    }

    private func copy(_ from: URL, to: URL) -> Bool {
        var ok = false
        var err: NSError?
        NSFileCoordinator().coordinate(readingItemAt: from, options: [], error: &err) { readable in
            do {
                try? fm.removeItem(at: to)
                try fm.copyItem(at: readable, to: to)
                protect(to.path)
                ok = true
            } catch {
                try? fm.removeItem(at: to)
            }
        }
        return ok && err == nil
    }

    /// Removes everything of one batch (a folder fetched or sent whole).
    public func removeBatch(_ batch: String) {
        try? fm.removeItem(at: fetched.appendingPathComponent(batch))
        try? fm.removeItem(at: outgoing.appendingPathComponent(batch))
    }

    /// Removes one transfer's file and its folder.
    public func remove(_ path: String) {
        let f = URL(fileURLWithPath: path)
        try? fm.removeItem(at: f)
        let parent = f.deletingLastPathComponent()
        let grand = parent.deletingLastPathComponent().standardizedFileURL.path
        if grand == fetched.standardizedFileURL.path || grand == outgoing.standardizedFileURL.path { try? fm.removeItem(at: parent) }
    }

    /// PHN-15: the app locked: the fetched files go (copies the owner saved or shared elsewhere are his).
    public func wipeFetched() { try? fm.removeItem(at: fetched) }

    /// Forget this Mac: everything.
    public func wipeAll() { try? fm.removeItem(at: root) }

    /// PHN-16: nothing fetched is kept longer than a day; a copy waiting to be sent goes after a day too.
    public func dropOld(now: Date = Date()) {
        for dir in [fetched, outgoing] {
            for f in (try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
            where now.timeIntervalSince(newest(f)) > Self.maxAge {
                try? fm.removeItem(at: f)
            }
        }
    }

    private func newest(_ f: URL) -> Date {
        let own = (try? fm.attributesOfItem(atPath: f.path)[.modificationDate] as? Date) ?? .distantPast
        guard let kids = try? fm.contentsOfDirectory(at: f, includingPropertiesForKeys: nil), !kids.isEmpty else { return own }
        return kids.map(newest).max() ?? own
    }

    public func size(_ path: String) -> Int64 { ((try? fm.attributesOfItem(atPath: path)[.size]) as? NSNumber)?.int64Value ?? 0 }

    public func isFile(_ path: String) -> Bool {
        var d: ObjCBool = false
        return fm.fileExists(atPath: path, isDirectory: &d) && !d.boolValue
    }

    /// A file name as given, without folders, and never `.` or `..`.
    public static func safe(_ name: String) -> String {
        var n = name
        if let i = n.lastIndex(of: "/") { n = String(n[n.index(after: i)...]) }
        if let i = n.lastIndex(of: "\\") { n = String(n[n.index(after: i)...]) }
        n = n.replacingOccurrences(of: "\u{0}", with: "").trimmingCharacters(in: .whitespacesAndNewlines)
        return n.isEmpty || n == "." || n == ".." ? "file" : String(n.prefix(200))
    }

    /// A path inside a folder: each part made [safe], so nothing climbs out.
    public static func safeRel(_ rel: String) -> String {
        let parts = rel.split(whereSeparator: { $0 == "/" || $0 == "\\" }).map { safe(String($0)) }
        return parts.isEmpty ? "file" : parts.joined(separator: "/")
    }
}
