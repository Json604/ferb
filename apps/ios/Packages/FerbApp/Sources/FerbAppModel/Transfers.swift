import FerbCore
import FerbMobile
import Foundation
import Observation

public enum Way: Sendable { case fetch, send }

public enum Phase: Sendable { case running, paused, done, failed, waitingPath }

/// A finished task's own file: its task and its place among the outcome's sorted files (the result card's).
public struct TaskFile: Equatable, Sendable {
    public let taskId: String
    public let index: UInt64

    public init(taskId: String, index: UInt64) {
        self.taskId = taskId
        self.index = index
    }
}

/// One file moving either way (PHN-13, PHN-115): its bytes so far of the whole, where it is on the phone, and how it
/// stands. A file sent belongs to a `batch` (the files picked together: one progress card, "Sending 2 photos to your
/// Mac"); `forChat` batches go to the composer when they arrive. A file of a whole folder carries its `rel` path
/// inside it; a task's own file its `task`. (Android's Transfer; named apart from FerbDesign's Transfer, the card.)
public struct FileTransfer: Equatable, Identifiable, Sendable {
    public let key: String
    public let way: Way
    public var name: String
    public let local: String
    public var macPath: String? = nil
    public var done: Int64 = 0
    public var total: Int64 = 0
    public var phase: Phase = .running
    public var keyPlace: Bool = false
    public var uploadId: String? = nil
    public var problem: Note? = nil
    public var batch: String? = nil
    public var forChat: Bool = false
    public var rel: String? = nil
    public var task: TaskFile? = nil
    /// A file sent: where it is on the Mac in the Mac's words, when the library says (UploadPlace); nil: the default place.
    public var macPlace: String? = nil
    /// A file of a message he sent with files (kb PHN-691): its batch is the message; shown with it, never on the Files tab.
    public var message: Bool = false

    public init(key: String, way: Way, name: String, local: String, macPath: String? = nil, done: Int64 = 0, total: Int64 = 0,
                phase: Phase = .running, keyPlace: Bool = false, uploadId: String? = nil, problem: Note? = nil, batch: String? = nil,
                forChat: Bool = false, rel: String? = nil, task: TaskFile? = nil, message: Bool = false) {
        self.key = key
        self.way = way
        self.name = name
        self.local = local
        self.macPath = macPath
        self.done = done
        self.total = total
        self.phase = phase
        self.keyPlace = keyPlace
        self.uploadId = uploadId
        self.problem = problem
        self.batch = batch
        self.forChat = forChat
        self.rel = rel
        self.task = task
        self.message = message
    }

    public var id: String { key }
    public var fraction: Double { total > 0 ? min(max(Double(done) / Double(total), 0), 1) : 0 }
}

/// Files that move together. A whole `folder` either way (PHN-13): fetched from the Mac's listing of every file under
/// it (`skipped` key-place files left out, `cut` when the listing stopped at its cap), or sent as one folder in the
/// Mac's upload place (`group`, the id its files carry and the message's attachment). `unreadable`: files of a picked
/// folder that couldn't be read, so they weren't sent.
public struct TransferBatch: Equatable, Sendable {
    public let key: String
    public let way: Way
    public var folder: String? = nil
    public var macPath: String? = nil
    public var skipped: Int64 = 0
    public var cut: Bool = false
    public var group: String? = nil
    public var unreadable: Int = 0

    public init(key: String, way: Way, folder: String? = nil, macPath: String? = nil, skipped: Int64 = 0, cut: Bool = false,
                group: String? = nil, unreadable: Int = 0) {
        self.key = key
        self.way = way
        self.folder = folder
        self.macPath = macPath
        self.skipped = skipped
        self.cut = cut
        self.group = group
        self.unreadable = unreadable
    }
}

/// The phone's transfers (Android: model/Transfers.kt): fetches from the Mac into [LocalFiles.fetched] and uploads
/// from [LocalFiles.outgoing], each with progress, pause (the library's cancelTransfer: what arrived stays) and resume
/// where it stopped (a fetch from the file's length, an upload by its upload id), refused on the public relay before
/// anything is asked and tried once more by itself when the path carries files again ([bulkAllowed]). A batch's
/// files go one after another. Never opened by themselves: the screen opens a finished file only on the owner's tap.
///
/// iOS suspends an app soon after it leaves the screen: the shell calls [checkpoint] before its background time ends
/// (every running transfer pauses, keeping what arrived and its upload id) and [resumeAfterBackground] when the app
/// is active again.
@MainActor
@Observable
public final class Transfers {
    public private(set) var list: [FileTransfer] = []
    public private(set) var batches: [String: TransferBatch] = [:]
    /// Uploads that arrived for the composer (PHN-127: a message's attachments are this phone's finished uploads).
    public private(set) var ready: [Attachment] = []
    /// Where the composer's files are on the Mac, in the Mac's words when the library says (PHN-608); nil: the default.
    public private(set) var readyPlace: String?

    @ObservationIgnored private let api: PhoneApi
    @ObservationIgnored private let local: LocalFiles
    @ObservationIgnored private let unlocker: Unlocker
    @ObservationIgnored private var attempts: [String: String] = [:]
    /// Where a folder's file is read from, until it is copied just before it goes (memory only).
    @ObservationIgnored private var sources: [String: URL] = [:]
    /// Picked folders whose security scope stays open while their files go.
    @ObservationIgnored private var picked: [String: PickedFolder] = [:]
    /// What [checkpoint] paused, to go on with at [resumeAfterBackground].
    @ObservationIgnored private var checkpointed: Set<String> = []
    @ObservationIgnored private var n = 0

    public init(api: PhoneApi, local: LocalFiles, unlocker: Unlocker) {
        self.api = api
        self.local = local
        self.unlocker = unlocker
    }

    private func next() -> Int { n += 1; return n }
    private func update(_ key: String, _ f: (inout FileTransfer) -> Void) {
        guard let i = list.firstIndex(where: { $0.key == key }) else { return }
        f(&list[i])
    }
    private func get(_ key: String) -> FileTransfer? { list.first { $0.key == key } }
    private func setBatch(_ key: String, _ f: (inout TransferBatch) -> Void) {
        guard var b = batches[key] else { return }
        f(&b)
        batches[key] = b
    }

    // ---- fetching ---------------------------------------------------------------------------------------------------

    /// Fetches a file of the Mac; a key place's file asks for Face ID / Touch ID up front (PHN-121).
    @discardableResult
    public func fetch(_ macPath: String, name: String, size: Int64, keyPlace: Bool) -> String {
        if let existing = list.first(where: { $0.way == .fetch && $0.batch == nil && $0.macPath == macPath && $0.phase != .failed }) {
            if existing.phase == .paused || existing.phase == .waitingPath { resume(existing.key) }
            return existing.key
        }
        let key = "f\(next())-\(uptimeNanos())"
        let file = local.fetchTarget(key, name)
        list.append(FileTransfer(key: key, way: .fetch, name: name, local: file.path, macPath: macPath, total: size, keyPlace: keyPlace))
        runFetch(key)
        return key
    }

    /// A finished task's own file (its result card): the Mac names it from the task's record by its place among the
    /// outcome's files, so the phone never sends a path it read in the chat. Kb PHN-535.
    @discardableResult
    public func fetchTaskFile(_ taskId: String, index: UInt64, name: String) -> String {
        let task = TaskFile(taskId: taskId, index: index)
        if let existing = list.first(where: { $0.task == task && $0.phase != .failed }) {
            if existing.phase == .paused || existing.phase == .waitingPath { resume(existing.key) }
            return existing.key
        }
        let key = "t\(next())-\(uptimeNanos())"
        let file = local.fetchTarget(key, name)
        list.append(FileTransfer(key: key, way: .fetch, name: name, local: file.path, task: task))
        runFetch(key)
        return key
    }

    /// The transfer of a task's file, if one was started.
    public func ofTask(_ taskId: String, index: UInt64) -> FileTransfer? { list.first { $0.task == TaskFile(taskId: taskId, index: index) } }

    /// A whole folder of the Mac, from its listing of every file under it ([Tree]): one card, its files fetched one
    /// after another into `fetched/<batch>/<folder>/…` (removed at the lock, after a day and on Forget, as every
    /// fetched file). Kb PHN-533.
    @discardableResult
    public func fetchFolder(_ tree: Tree) -> String {
        let batch = "d\(next())-\(uptimeNanos())"
        var base = tree.path
        while base.hasSuffix("/") { base.removeLast() }
        let files = tree.files.enumerated().map { i, f in
            FileTransfer(key: "\(batch).\(i)", way: .fetch, name: (f.rel as NSString).lastPathComponent,
                         local: local.folderTarget(batch, tree.name, f.rel).path, macPath: "\(base)/\(f.rel)", total: Int64(f.size),
                         phase: .paused, batch: batch, rel: f.rel)
        }
        batches[batch] = TransferBatch(key: batch, way: .fetch, folder: tree.name, macPath: tree.path, skipped: Int64(tree.skipped), cut: tree.cut)
        list += files
        runBatch(batch)
        return batch
    }

    @discardableResult
    private func runFetch(_ key: String) -> Task<Void, Never> { Task { _ = await fetchOne(key) } }

    private func fetchOne(_ key: String) async -> Bool {
        guard let t = get(key) else { return false }
        let attempt = "\(key).\(next())"
        attempts[key] = attempt
        update(key) { $0.phase = .running; $0.problem = nil }
        let listener = Heard(self, key: key, attempt: nil)
        let r: Outcome<Fetched>
        if let task = t.task {
            // the Mac takes no offset for a task's file: it starts over
            update(key) { $0.done = 0 }
            r = await unlocker.run { await api.fetchTaskFile(id: attempt, taskId: task.taskId, index: task.index, saveTo: t.local, listener: listener) }
        } else {
            let local = self.local
            r = await unlocker.withProof(t.keyPlace, .file) { signer in
                let offset = local.isFile(t.local) ? UInt64(local.size(t.local)) : 0
                return await api.fetch(id: attempt, path: t.macPath ?? "", saveTo: t.local, offset: offset, signer: signer, listener: listener)
            }
        }
        if attempts[key] != attempt { return false }
        attempts[key] = nil
        switch r {
        case .ok(let f):
            local.protect(t.local)
            update(key) { $0.phase = .done; $0.done = Int64(f.total); $0.total = Int64(f.total) }
            return true
        case .failed(let e):
            ended(key, e)
            return false
        }
    }

    // ---- sending ----------------------------------------------------------------------------------------------------

    /// Sends staged copies to the Mac, one after another, as one batch.
    @discardableResult
    public func send(_ files: [Staged], forChat: Bool) -> String {
        let batch = "b\(next())-\(uptimeNanos())"
        batches[batch] = TransferBatch(key: batch, way: .send)
        list += files.enumerated().map { i, s in
            FileTransfer(key: "\(batch).\(i)", way: .send, name: s.name, local: s.path, total: s.size, phase: .paused, batch: batch, forChat: forChat)
        }
        runBatch(batch)
        return batch
    }

    /// The files of a message he sent (kb PHN-691): one batch named by the message, each file keyed by its chip, going on
    /// from its upload id when it had one (a send cut short, even across FERB closing). Already there: it goes on.
    public func sendMessage(_ id: String, _ chips: [Chip]) {
        if list.contains(where: { $0.batch == id }) {
            if !list.contains(where: { $0.batch == id && attempts[$0.key] != nil }) { runBatch(id) }
            return
        }
        batches[id] = TransferBatch(key: id, way: .send)
        list += chips.map { c in
            FileTransfer(key: "\(id).\(c.key)", way: .send, name: c.name, local: c.path, total: c.size, phase: .paused, uploadId: c.uploadId,
                         batch: id, message: true)
        }
        runBatch(id)
    }

    /// Stops the file of `batch` that is going (the rest wait with it).
    public func pauseBatch(_ batch: String) {
        if let t = list.first(where: { $0.batch == batch && $0.phase == .running }) { pause(t.key) }
    }

    /// Stops `batch` and forgets it and its copies.
    public func removeBatch(_ batch: String) {
        if let t = list.first(where: { $0.batch == batch }) { remove(t.key) }
    }

    /// A message's files all arrived and it was sent: its transfers go from the list (their copies already went).
    public func forgetBatch(_ batch: String) {
        list.removeAll { $0.batch == batch }
        batches[batch] = nil
    }

    /// Sends a picked folder whole (PHN-127: one folder in the Mac's upload place, each file's place inside it kept):
    /// the Mac starts the folder (`putGroup`), then each file goes with its group and path, copied here only just
    /// before it goes (a large folder never doubles its space at once). The folder rides with a message as one
    /// attachment, the group's id. Kb PHN-534.
    @discardableResult
    public func sendFolder(_ folder: PickedFolder, forChat: Bool) -> String {
        let batch = "s\(next())-\(uptimeNanos())"
        picked[batch] = folder
        let files = folder.files.enumerated().map { i, f -> FileTransfer in
            let key = "\(batch).\(i)"
            sources[key] = f.url
            return FileTransfer(key: key, way: .send, name: (f.rel as NSString).lastPathComponent, local: local.sendTarget(batch, f.rel).path,
                                total: f.size ?? 0, phase: .paused, batch: batch, forChat: forChat, rel: f.rel)
        }
        batches[batch] = TransferBatch(key: batch, way: .send, folder: folder.name)
        list += files
        runBatch(batch)
        return batch
    }

    @discardableResult
    private func runBatch(_ batch: String) -> Task<Void, Never> {
        Task {
            for t in list.filter({ $0.batch == batch }) {
                guard let cur = get(t.key), cur.phase != .done else { continue }
                let ok = cur.way == .fetch ? await fetchOne(cur.key) : await sendOne(cur.key, batch)
                if !ok && get(cur.key) != nil { return }
            }
            let all = list.filter { $0.batch == batch }
            if !all.isEmpty && all.allSatisfy({ $0.phase == .done }) {
                closeFolder(batch)
                if all[0].forChat && !all[0].message { attach(batch) }
            }
        }
    }

    private func closeFolder(_ batch: String) {
        if let f = picked.removeValue(forKey: batch) { local.release(f) }
    }

    private func sendOne(_ key: String, _ batch: String) async -> Bool {
        let b = batches[batch]
        var group = b?.group
        if let folder = b?.folder, group == nil {
            switch await unlocker.run({ await api.putGroup(name: folder) }) {
            case .ok(let g):
                group = g
                setBatch(batch) { $0.group = g }
            case .failed(let e):
                ended(key, e)
                return false
            }
        }
        guard var cur = get(key) else { return false }
        if let source = sources[key], !local.isFile(cur.local) {
            // a folder's file is copied only now; one that can't be read is left out and counted
            let local = self.local
            let target = URL(fileURLWithPath: cur.local)
            let copied = await Task.detached { local.stage(source, to: target) }.value
            if !copied {
                sources[key] = nil
                list.removeAll { $0.key == key }
                setBatch(batch) { $0.unreadable += 1 }
                return true
            }
            let size = local.size(cur.local)
            update(key) { $0.uploadId = nil; $0.total = size }
            guard let again = get(key) else { return false }
            cur = again
        }
        let attempt = "\(key).\(next())"
        attempts[key] = attempt
        update(key) { $0.phase = .running; $0.problem = nil }
        let listener = Heard(self, key: key, attempt: attempt)
        let r = await unlocker.run {
            await api.upload(id: attempt, localPath: cur.local, name: cur.name, resume: get(key)?.uploadId, group: group, rel: cur.rel, listener: listener)
        }
        if attempts[key] != attempt { return false }
        attempts[key] = nil
        switch r {
        case .ok(let u):
            update(key) { $0.phase = .done; $0.done = Int64(u.size); $0.total = Int64(u.size); $0.uploadId = u.uploadId; $0.macPlace = UploadPlace.shown(u) }
            sources[key] = nil
            local.remove(cur.local)
            return true
        case .failed(let e):
            ended(key, e)
            return false
        }
    }

    /// The batch's files go to the composer, to send with a message (`sendWith`): a folder as one, by its group.
    public func attach(_ batch: String) {
        let b = batches[batch]
        let all = list.filter { $0.batch == batch && $0.phase == .done && $0.uploadId != nil }
        let add: [Attachment]
        if let folder = b?.folder, let group = b?.group { add = [Attachment(uploadId: group, name: folder)] }
        else { add = all.map { Attachment(uploadId: $0.uploadId!, name: $0.name) } }
        for a in add where !ready.contains(where: { $0.uploadId == a.uploadId }) { ready.append(a) }
        if let place = all.compactMap(\.macPlace).first { readyPlace = place }
        list.removeAll { $0.batch == batch }
        batches[batch] = nil
        closeFolder(batch)
        local.removeBatch(batch)
    }

    public func unattach(_ uploadId: String) { ready.removeAll { $0.uploadId == uploadId } }

    /// The composer sent them.
    public func sent() { ready = [] }

    // ---- either way -----------------------------------------------------------------------------------------------

    fileprivate func progress(_ key: String, _ attempt: String?, done: UInt64, of: UInt64) {
        if let attempt, attempts[key] != attempt { return }
        guard let t = get(key), t.phase == .running else { return }
        update(key) { $0.done = Int64(done); $0.total = max(Int64(of), $0.total) }
    }

    fileprivate func accepted(_ key: String, uploadId: String, offset: UInt64) {
        // the upload id is kept whenever it arrives (a pause goes on by it); the offset only while it runs
        guard let t = get(key), t.phase != .done else { return }
        update(key) { $0.uploadId = uploadId; if t.phase == .running { $0.done = Int64(offset) } }
    }

    private func ended(_ key: String, _ e: FerbError) {
        let phase: Phase
        switch e {
        case .Cancelled, .NotSigned: phase = .paused
        case .NeedsBetterPath: phase = .waitingPath
        default: phase = .failed
        }
        let note: Note? = phase == .paused ? nil : (e.linkCode == Unlocker.pairAgain ? Note("needs_pairing_again") : AppModel.errorNote(e))
        let local = self.local
        update(key) { t in
            let have = (t.way == .fetch && t.task == nil && local.isFile(t.local)) ? local.size(t.local) : t.done
            t.phase = phase
            t.problem = note
            t.done = have
        }
    }

    /// Pauses a transfer (the one under way in a batch: the batch waits with it).
    @discardableResult
    public func pause(_ key: String) -> Task<Void, Never>? {
        guard let attempt = attempts[key] else { return nil }
        return Task { _ = await api.cancelTransfer(id: attempt) }
    }

    @discardableResult
    public func resume(_ key: String) -> Task<Void, Never>? {
        guard let t = get(key) else { return nil }
        if let batch = t.batch {
            if list.contains(where: { $0.batch == batch && attempts[$0.key] != nil }) { return nil }
            return runBatch(batch)
        }
        if attempts[key] != nil { return nil }
        return runFetch(key)
    }

    /// Stops and forgets a transfer (its whole batch, for files moving together) and its files on the phone.
    public func remove(_ key: String) {
        guard let t = get(key) else { return }
        let gone = t.batch.map { b in list.filter { $0.batch == b } } ?? [t]
        let keys = Set(gone.map(\.key))
        list.removeAll { keys.contains($0.key) }
        if let b = t.batch {
            batches[b] = nil
            closeFolder(b)
        }
        for g in gone {
            sources[g.key] = nil
            if let a = attempts.removeValue(forKey: g.key) { Task { _ = await api.cancelTransfer(id: a) } }
            local.remove(g.local)
        }
        if let b = t.batch { local.removeBatch(b) }
    }

    /// The path carries files again (PHN-444): each transfer that waited for it is tried once more.
    public func bulkAllowed() {
        let waiting = list.filter { $0.phase == .waitingPath }
        var seen = Set<String>()
        for b in waiting.compactMap(\.batch) where seen.insert(b).inserted { runBatch(b) }
        for t in waiting where t.batch == nil { runFetch(t.key) }
    }

    /// PHN-11, PHN-15: the app locked: fetches stop and their files go. Files being sent stay paused.
    public func locked() {
        for t in list { if let a = attempts.removeValue(forKey: t.key) { Task { _ = await api.cancelTransfer(id: a) } } }
        list = list.filter { $0.way == .send }.map { t in
            var t = t
            if t.phase == .running { t.phase = .paused }
            return t
        }
        batches = batches.filter { $0.value.way == .send }
        checkpointed = []
        local.wipeFetched()
    }

    /// PHN-11: a key place's file goes the moment its viewer closes (the owner is back in FERB).
    public func dropKeyPlaceFiles(_ keys: Set<String>) {
        let gone = list.filter { keys.contains($0.key) && $0.keyPlace }
        if gone.isEmpty { return }
        let ids = Set(gone.map(\.key))
        list.removeAll { ids.contains($0.key) }
        for g in gone { local.remove(g.local) }
    }

    /// iOS is about to suspend the app: every transfer under way pauses where it is (a fetch keeps its bytes, an
    /// upload its id), to go on at [resumeAfterBackground]. Returns when each pause was asked for.
    public func checkpoint() async {
        let running = attempts
        for (key, attempt) in running {
            checkpointed.insert(key)
            _ = await api.cancelTransfer(id: attempt)
        }
    }

    /// The app is active again: what [checkpoint] paused goes on from where it stopped.
    public func resumeAfterBackground() {
        let keys = checkpointed
        checkpointed = []
        var batchesDone = Set<String>()
        for key in keys.sorted() {
            guard let t = get(key), t.phase == .paused else { continue }
            if let b = t.batch {
                if batchesDone.insert(b).inserted { resume(key) }
            } else {
                resume(key)
            }
        }
    }

    /// Forget this Mac.
    public func clear() {
        for a in attempts.values { Task { _ = await api.cancelTransfer(id: a) } }
        attempts = [:]
        sources = [:]
        for f in picked.values { local.release(f) }
        picked = [:]
        checkpointed = []
        list = []
        batches = [:]
        ready = []
        local.wipeAll()
    }

    /// Invented transfers for previews and screenshots (nothing runs).
    public func setForPreview(_ list: [FileTransfer], batches: [TransferBatch] = []) {
        self.list = list
        self.batches = Dictionary(batches.map { ($0.key, $0) }, uniquingKeysWith: { _, b in b })
    }

    /// What one transfer hears from the library, on the link's threads: posted to the main actor.
    private final class Heard: TransferListener, @unchecked Sendable {
        private weak var owner: Transfers?
        private let key: String
        private let attempt: String?
        init(_ owner: Transfers, key: String, attempt: String?) {
            self.owner = owner
            self.key = key
            self.attempt = attempt
        }
        func progress(done: UInt64, of: UInt64) {
            let (o, key, attempt) = (owner, self.key, self.attempt)
            Task { @MainActor in o?.progress(key, attempt, done: done, of: of) }
        }
        func accepted(uploadId: String, offset: UInt64) {
            let (o, key) = (owner, self.key)
            Task { @MainActor in o?.accepted(key, uploadId: uploadId, offset: offset) }
        }
    }
}
