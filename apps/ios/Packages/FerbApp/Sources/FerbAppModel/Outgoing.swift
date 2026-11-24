import FerbMobile
import Foundation
import Observation

/// A file in the message box (kb PHN-690; Android's Chip): picked and copied into the phone's own storage (`path`),
/// nothing sent yet. Once it goes, `uploadId` is the Mac's name for it (given when its upload starts, so a send cut
/// short goes on where it stopped) and `onMac` says it arrived whole.
public struct Chip: Equatable, Sendable, Codable {
    public let key: String
    public let name: String
    public let size: Int64
    public let path: String
    public var uploadId: String?
    public var onMac: Bool

    public init(key: String, name: String, size: Int64, path: String, uploadId: String? = nil, onMac: Bool = false) {
        self.key = key
        self.name = name
        self.size = size
        self.path = path
        self.uploadId = uploadId
        self.onMac = onMac
    }
}

/// Where a message with files stands (kb PHN-691; Android's SendPhase).
public enum SendPhase: String, Sendable, Codable {
    /// The Mac can't be reached: it starts by itself when it can.
    case waitingLink
    /// Only the shared relay: files never go that way; it starts by itself on a direct path or his own relay.
    case waitingPath
    /// Its files are going to the Mac.
    case uploading
    /// Its files are there: the message itself is going.
    case sending
    /// He stopped it.
    case stopped
    /// FERB locked while its files went: it goes on by itself once he unlocks.
    case locked
    /// FERB closed before it was sent.
    case closed
    /// It failed, for `note`'s reason.
    case failed
}

/// A message he sent with files, held on the phone until its files are on the Mac and the message went with them
/// (kb PHN-690, PHN-691). `onMac`: files that were already on the Mac (the Files tab's "Add a message").
public struct Sending: Equatable, Sendable, Identifiable {
    public let id: String
    public let text: String
    public var chips: [Chip]
    public let onMac: [Attachment]
    public var phase: SendPhase
    public var note: Note?
    public var done: Int64
    public var total: Int64
    /// Sent in the temporary chat (the owner, 2026-11-24, ORCH-715: the same + and files as the chat): its files go the
    /// same way, then it goes with `fresh` (never the outbox); it shows only in the temporary chat, is never written on
    /// the phone, and goes when the chat ends.
    public let temporary: Bool

    public init(id: String, text: String, chips: [Chip], onMac: [Attachment], phase: SendPhase, note: Note? = nil, done: Int64 = 0,
                total: Int64 = 0, temporary: Bool = false) {
        self.id = id
        self.text = text
        self.chips = chips
        self.onMac = onMac
        self.phase = phase
        self.note = note
        self.done = done
        self.total = total
        self.temporary = temporary
    }

    public var fraction: Double { total > 0 ? min(max(Double(done) / Double(total), 0), 1) : 0 }
}

/// The message box's files and the messages that carry files (kb PHN-690, PHN-691; Android's Outgoing). A pick only
/// copies the files here and shows them in the box; nothing leaves the phone. Send makes the message a `Sending`: its
/// files go to the Mac through `Transfers` (one batch named by the message), and when they are all there the message
/// goes with their ids (`sendWith`), as FERB.app's composer sends attached files. The box and the messages under way
/// are kept in `messages.json` beside the copies, so a message survives FERB being closed; reading and writing it, and
/// every library call, happen off the main thread.
@MainActor
@Observable
public final class Outgoing {
    public private(set) var chips: [Chip] = []
    public private(set) var messages: [Sending] = []

    @ObservationIgnored private let api: PhoneApi
    @ObservationIgnored private let transfers: Transfers
    @ObservationIgnored private let local: LocalFiles
    /// A message went: the chat reads again (and the Mac may want its unlock).
    @ObservationIgnored public var onSent: ((OutboxItem) -> Void)?
    /// A temporary chat's message went: the temporary chat as the Mac answered it.
    @ObservationIgnored public var onSentTemporary: ((TemporaryChat) -> Void)?
    @ObservationIgnored private let store: Store
    @ObservationIgnored private var follow: Follow<[FileTransfer]>?
    @ObservationIgnored private var n = 0

    public init(api: PhoneApi, transfers: Transfers, local: LocalFiles) {
        self.api = api
        self.transfers = transfers
        self.local = local
        self.store = Store(local.root.appendingPathComponent("messages.json"))
        follow = Follow({ [weak self] () -> [FileTransfer]? in self?.transfers.list }) { [weak self] list in self?.moved(list) }
    }

    private func index(_ id: String) -> Int? { messages.firstIndex { $0.id == id } }

    private func nextKey(_ prefix: String) -> String {
        n += 1
        return "\(prefix)\(UInt64(Date().timeIntervalSince1970 * 1000))-\(n)"
    }

    // ---- the message box --------------------------------------------------------------------------------------------

    /// Picked files, already copied here: they wait in the box.
    public func pick(_ staged: [Staged]) {
        guard !staged.isEmpty else { return }
        chips += staged.map { Chip(key: nextKey("c"), name: $0.name, size: $0.size, path: $0.path) }
        save()
    }

    /// The cross on a file in the box: it goes, and so does its copy. Nothing was sent.
    public func unpick(_ key: String) {
        guard let c = chips.first(where: { $0.key == key }) else { return }
        chips.removeAll { $0.key == key }
        let local = self.local
        Task.detached { local.remove(c.path) }
        save()
    }

    /// Send with the box's files: false when the box has none (the message then goes as before). The box empties; the
    /// message shows in the chat and goes as soon as its files are on the Mac.
    @discardableResult
    public func send(_ text: String, onMac: [Attachment], temporary: Bool = false) -> Bool {
        guard !chips.isEmpty else { return false }
        let id = nextKey("m")
        let box = chips
        chips = []
        messages.append(Sending(id: id, text: text, chips: box, onMac: onMac, phase: .uploading, total: box.reduce(0) { $0 + $1.size },
                                temporary: temporary))
        start(id)
        return true
    }

    /// The temporary chat ended (its icon, on any device): its messages still on their way go, with their copies.
    public func dropTemporary() {
        for m in messages where m.temporary { remove(m.id) }
    }

    // ---- a message on its way -----------------------------------------------------------------------------------------

    /// Starts (or goes on with) a message: its files first, as far as the path allows.
    public func start(_ id: String) {
        guard let i = index(id) else { return }
        let left = messages[i].chips.filter { !$0.onMac }
        let l = api.link
        if left.isEmpty {
            deliver(id)
        } else if l?.phase != .online {
            messages[i].phase = .waitingLink
            messages[i].note = nil
        } else if l?.bulkAllowed != true {
            messages[i].phase = .waitingPath
            messages[i].note = nil
        } else {
            messages[i].phase = .uploading
            messages[i].note = nil
            transfers.sendMessage(id, left)
        }
        save()
    }

    /// The link moved: what waited for the Mac, or for a path that carries files, starts. (Files the path refused
    /// part way are tried again by `Transfers.bulkAllowed` itself.)
    public func linkChanged() {
        let l = api.link
        let started = Set(transfers.list.compactMap(\.batch))
        for m in messages {
            switch m.phase {
            case .waitingLink: if l?.phase == .online { start(m.id) }
            case .waitingPath: if l?.phase == .online && l?.bulkAllowed == true && !started.contains(m.id) { start(m.id) }
            default: break
            }
        }
    }

    /// FERB locked (PHN-15): files on their way stop (Transfers.locked); their messages go on after the unlock.
    public func locked() {
        for i in messages.indices where messages[i].phase == .uploading { messages[i].phase = .locked }
        save()
    }

    /// He unlocked: what the lock paused goes on.
    public func unlocked() {
        for m in messages where m.phase == .locked { start(m.id) }
    }

    /// Cancel while its files go: they stop where they are; the message stays, not sent, to try again or remove.
    public func cancel(_ id: String) {
        guard let i = index(id) else { return }
        messages[i].phase = .stopped
        messages[i].note = nil
        transfers.pauseBatch(id)
        save()
    }

    public func retry(_ id: String) { start(id) }

    /// Files can't go on this path: the words go now, without them.
    @discardableResult
    public func textOnly(_ id: String) -> Task<Void, Never>? {
        guard let i = index(id), !messages[i].text.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        let m = messages[i]
        messages[i].phase = .sending
        let api = self.api
        return Task {
            if m.temporary {
                switch await Task.detached(operation: { await api.sendTemporary(m.text, attachments: m.onMac) }).value {
                case .ok(let t):
                    drop(id)
                    onSentTemporary?(t)
                case .failed(let e):
                    if let j = index(id) {
                        messages[j].phase = .waitingPath
                        messages[j].note = AppModel.errorNote(e)
                    }
                    save()
                }
                return
            }
            let r = await Task.detached { m.onMac.isEmpty ? await api.send(m.text) : await api.sendWith(m.text, attachments: m.onMac) }.value
            switch r {
            case .ok(let item):
                drop(id)
                onSent?(item)
            case .failed(let e):
                if let j = index(id) {
                    messages[j].phase = .waitingPath
                    messages[j].note = AppModel.errorNote(e)
                }
                save()
            }
        }
    }

    /// Remove: the message, its files' copies and anything of them on its way. Nothing of it is sent.
    public func remove(_ id: String) {
        transfers.removeBatch(id)
        drop(id)
    }

    private func drop(_ id: String) {
        guard let m = messages.first(where: { $0.id == id }) else { return }
        messages.removeAll { $0.id == id }
        let local = self.local
        Task.detached { for c in m.chips { local.remove(c.path) } }
        save()
    }

    /// Every file is on the Mac: the message goes with their ids.
    private func deliver(_ id: String) {
        guard let i = index(id), messages[i].phase != .sending else { return }
        messages[i].phase = .sending
        messages[i].note = nil
        messages[i].done = messages[i].total
        save()
        let m = messages[i]
        let files = m.onMac + m.chips.compactMap { c in c.uploadId.map { Attachment(uploadId: $0, name: c.name) } }
        let api = self.api
        Task {
            if m.temporary {
                // the temporary chat's message: `fresh`, never the outbox (the Mac moves its files into the temporary
                // chat's own folder and deletes them when it ends)
                switch await Task.detached(operation: { await api.sendTemporary(m.text, attachments: files) }).value {
                case .ok(let t):
                    transfers.forgetBatch(id)
                    drop(id)
                    onSentTemporary?(t)
                case .failed(let e):
                    if let j = index(id) {
                        messages[j].phase = .failed
                        messages[j].note = AppModel.errorNote(e)
                    }
                    save()
                }
                return
            }
            let r = await Task.detached { await api.sendWith(m.text, attachments: files) }.value
            switch r {
            case .ok(let item):
                transfers.forgetBatch(id)
                drop(id)
                onSent?(item)
            case .failed(let e):
                if let j = index(id) {
                    messages[j].phase = .failed
                    messages[j].note = AppModel.errorNote(e)
                }
                save()
            }
        }
    }

    /// The transfers moved: each message's progress, its files' ids, and how its files stand.
    private func moved(_ list: [FileTransfer]) {
        for m in messages {
            let mine = list.filter { $0.batch == m.id }
            if mine.isEmpty { continue }
            var changed = false
            let chips = m.chips.map { c -> Chip in
                guard let t = mine.first(where: { $0.key == "\(m.id).\(c.key)" }) else { return c }
                var c2 = c
                c2.uploadId = t.uploadId ?? c.uploadId
                c2.onMac = c.onMac || t.phase == .done
                if c2 != c { changed = true }
                return c2
            }
            let done = chips.reduce(Int64(0)) { sum, c in
                sum + (c.onMac ? c.size : (mine.first { $0.key == "\(m.id).\(c.key)" }?.done ?? 0))
            }
            // only a message whose files are going (or waiting for a path that carries them) follows its transfers;
            // one he stopped, or the lock paused, keeps its phase
            let following = m.phase == .uploading || m.phase == .waitingPath
            var phase = m.phase
            var note = m.note
            if following && !chips.allSatisfy(\.onMac) {
                if let failed = mine.first(where: { $0.phase == .failed }) {
                    phase = .failed
                    note = failed.problem
                } else if mine.contains(where: { $0.phase == .waitingPath }) {
                    phase = .waitingPath
                } else if mine.contains(where: { $0.phase == .running }) {
                    phase = .uploading
                }
            }
            guard let i = index(m.id) else { continue }
            messages[i].chips = chips
            messages[i].done = done
            messages[i].phase = phase
            messages[i].note = note
            if changed || phase != m.phase { save() }
            if following && chips.allSatisfy(\.onMac) { deliver(m.id) }
        }
    }

    // ---- kept on the phone --------------------------------------------------------------------------------------------

    private struct Kept: Codable {
        struct Message: Codable {
            let id: String
            let text: String
            let phase: SendPhase
            let chips: [Chip]
            let onMac: [[String]]
        }
        let chips: [Chip]
        let messages: [Message]
    }

    /// Reads what was kept, off the main thread: the box's files, and the messages that were on their way. One that
    /// was going when FERB closed says so and waits for Try again (it goes on where its files stopped); one that waited
    /// for the Mac or a path still waits. Files whose copy is gone (a day old, PHN-16) are left out of the box; a
    /// message that lost one fails and says so.
    @discardableResult
    public func load() -> Task<Void, Never> {
        let store = self.store
        return Task {
            guard let kept = await Task.detached(operation: { store.read() }).value else { return }
            let fm = FileManager.default
            chips = kept.chips.filter { fm.fileExists(atPath: $0.path) }
            messages = kept.messages.map { k in
                let lost = k.chips.contains { !$0.onMac && !fm.fileExists(atPath: $0.path) }
                let phase: SendPhase = lost ? .failed : ([.waitingLink, .waitingPath, .stopped].contains(k.phase) ? k.phase : .closed)
                return Sending(id: k.id, text: k.text, chips: k.chips,
                               onMac: k.onMac.compactMap { $0.count == 2 ? Attachment(uploadId: $0[0], name: $0[1]) : nil },
                               phase: phase, note: lost ? Note("send_files_gone") : nil,
                               done: k.chips.filter(\.onMac).reduce(0) { $0 + $1.size }, total: k.chips.reduce(0) { $0 + $1.size })
            }
        }
    }

    private func save() {
        // a temporary chat's message is never written on the phone (memory only, as all of that chat)
        let kept = Kept(chips: chips, messages: messages.filter { !$0.temporary }.map {
            Kept.Message(id: $0.id, text: $0.text, phase: $0.phase, chips: $0.chips, onMac: $0.onMac.map { [$0.uploadId, $0.name] })
        })
        let store = self.store
        let before = lastWrite
        // each write waits for the one before it: a later save is never overwritten by an earlier one
        lastWrite = Task.detached {
            await before?.value
            await store.write(kept)
        }
    }

    @ObservationIgnored private var lastWrite: Task<Void, Never>?

    /// The file, written in the order saves were asked for (an actor: one write at a time).
    private actor Store {
        let url: URL
        init(_ url: URL) { self.url = url }

        nonisolated func read() -> Kept? {
            guard let data = try? Data(contentsOf: url) else { return nil }
            return try? JSONDecoder().decode(Kept.self, from: data)
        }

        func write(_ kept: Kept) {
            guard let data = try? JSONEncoder().encode(kept) else { return }
            try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? data.write(to: url, options: .atomic)
        }

        func delete() { try? FileManager.default.removeItem(at: url) }
    }

    /// Forget this Mac: everything.
    public func clear() {
        chips = []
        messages = []
        let store = self.store
        Task.detached { await store.delete() }
    }

    /// Invented box and messages for previews and screenshots (nothing runs).
    public func setForPreview(_ chips: [Chip], messages: [Sending] = []) {
        self.chips = chips
        self.messages = messages
    }
}
