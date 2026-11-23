import FerbCore
import FerbMobile
import Foundation

/// What the app's screens ask of the phone: one seam over [FerbPhone], so the model is tested with a fake and the
/// real one is the library (Android: model/PhoneApi.kt). `state` and `link` are observable (the real ones are
/// FerbPhone's @Observable properties); every call runs off the main thread (FerbPhone.call) and comes back typed.
@MainActor
public protocol PhoneApi: AnyObject {
    var state: PhoneState { get }
    var link: LinkStatus? { get }
    /// What changed in the core; each caller gets its own stream.
    func changes() -> AsyncStream<Change>

    /// The current conversation as the phone holds it, or nil before anything was fetched.
    func transcript() async -> Outcome<TranscriptView?>
    func sync() async -> Outcome<Void>
    func send(_ text: String) async -> Outcome<OutboxItem>
    func cancel(outboxId: String) async -> Outcome<Cancelled>
    func answer(_ approvalId: String, _ input: AnswerInput, signer: Signer?) async -> Outcome<String>
    /// A hidden answer (PHN-109, PHN-110) from a [SecretInput] the caller filled one UTF-16 unit at a time: the
    /// library empties and overwrites its copy whatever happens. Never `AnswerInput.hidden`.
    func answerHidden(_ approvalId: String, secret: SecretInput, signer: Signer) async -> Outcome<String>
    func stop(taskIds: [String]) async -> Outcome<Void>
    func loadOlder(conversation: String) async -> Outcome<OlderState>
    func unlock(_ signer: Signer) async -> Outcome<Void>
    func pair(_ code: String) async -> Outcome<Void>
    /// "Forget this Mac": `.ok(true)` when the Mac removed this phone too, `.ok(false)` when it couldn't be told.
    func forget() async -> Outcome<Bool>
    /// Paired with its lock key there: the opening prompt can sign an unlock statement before the link is up.
    func statementReady() async -> Bool
    /// The statement the opening prompt signed, handed to the Mac with its age (PHN-649).
    func unlockWithStatement(payload: String, sig: String, ageMs: UInt64) async -> Outcome<Void>
    /// The app is unlocked and in front: the device key's presence keeps the Mac's window open (PHN-648).
    func setOpen(_ on: Bool)
    /// "Try again" on the connection sheet (kb PHN-752): the link dials now instead of at its next try. Non-blocking.
    func retryLink()
    /// PHN-15: the app locked: the Mac is told and the conversation waits for the next unlock.
    func lock() async -> Outcome<Bool>
    /// The app opened by the prompt alone (the Mac out of reach): the next sync reads the conversation again.
    func unlocked() async -> Outcome<Void>
    /// "Stop everything on the Mac" (`stop.engage`): no prompt, works locked, never queued.
    func stopEverything() async -> Outcome<Void>
    /// What FERB has waiting, read now (network), and the last one pushed or read (memory).
    func status() async -> Outcome<PhoneStatus>
    /// "Until I'm back at my Mac" (PHN-610, PHN-611): whether FERB may use the Mac's screen without asking now, and the
    /// owner's "Ask me again" from the phone (both plain, allowed while locked).
    func screenUse() async -> Outcome<ScreenUse>
    func endScreenUse() async -> Outcome<ScreenUseEnd>
    /// Insomnia mode (APP-980..APP-985): read it, turn it on after the app's warning (the sensitive class: `signer` is
    /// the Face ID / Touch ID key where the phone has one), end it, Lock my Mac and It's fine (the last three plain,
    /// allowed while locked), and Show me: the Mac's camera to `sink` in memory only, never on the public relay.
    func insomniaStatus() async -> Outcome<Insomnia>
    func startInsomnia(variant: String, screenYes: Bool, signer: Signer) async -> Outcome<InsomniaStarted>
    func endInsomnia() async -> Outcome<InsomniaEnded>
    func lockMac() async -> Outcome<MacLocked>
    func dismissInsomniaNotice(_ noticeId: String) async -> Outcome<Insomnia>
    func watchCamera(sink: CameraSink) async -> Outcome<Live>
    /// The last status pushed or read. `async` like everything here that reaches the library (kb PHN-682): no caller
    /// on the main actor can wait inside it.
    func lastStatus() async -> PhoneStatus?
    func markReadLatest(conversation: String) async -> Outcome<Void>
    func sendWith(_ text: String, attachments: [Attachment]) async -> Outcome<OutboxItem>

    // files (PHN-113, PHN-115, PHN-127): a paused transfer ends with `Cancelled`
    func files(path: String?, signer: Signer?, after: String?) async -> Outcome<Folder>
    func fetch(id: String, path: String, saveTo: String, offset: UInt64, signer: Signer?, listener: TransferListener) async -> Outcome<Fetched>
    func upload(id: String, localPath: String, name: String, resume: String?, group: String?, rel: String?,
                listener: TransferListener) async -> Outcome<Uploaded>
    func cancelTransfer(id: String) async -> Outcome<Void>
    /// Every file under a folder of the Mac, to fetch it whole (direct or the owner's relay only).
    func tree(path: String, signer: Signer?) async -> Outcome<Tree>
    /// Starts a folder sent from the phone: its files carry the id as `group`; the id rides with a message.
    func putGroup(name: String) async -> Outcome<String>
    /// A finished task's `index`-th file (its result card's files, the outcome's sorted artifacts).
    func fetchTaskFile(id: String, taskId: String, index: UInt64, saveTo: String, listener: TransferListener) async -> Outcome<Fetched>
    /// The Mac's recent files for this phone and a search of names (both need the Mac's unlock).
    func recentFiles(limit: Int) async -> Outcome<RecentFiles>
    func searchFiles(query: String, path: String?, after: String?) async -> Outcome<FoundFiles>

    /// The phone's own facts (T-1214, SENSE-420): sent when changed and on each new connection; memory only.
    func reportDevice(_ report: DeviceReport) async -> Outcome<Bool>
    /// The Mac's open request for one exact fix (after the owner's Yes for a task), and its answer.
    func locationWanted() async -> Outcome<String?>
    func answerLocation(requestId: String, fix: LocationFix?, whyNot: String?) async -> Outcome<Void>

    /// A task's live preview (PHN-454): frames to `sink` in memory only; [Live.stop] ends it.
    func watchLive(taskId: String, sink: PreviewSink) async -> Outcome<Live>
    /// The work row's small live picture (kb PHN-725): the same preview at most every `everyMs` (never under 2 s).
    func watchThumbnail(taskId: String, everyMs: UInt64, sink: PreviewSink) async -> Outcome<Live>
    /// A card row's own small live picture (kb PHN-726; FERB.app's RowPreview): one step's preview (`worker`).
    func watchStepThumbnail(taskId: String, worker: String, everyMs: UInt64, sink: PreviewSink) async -> Outcome<Live>

    func settings() async -> Outcome<[SettingValue]>
    func setSetting(key: String, valueJson: String, signer: Signer) async -> Outcome<String>
    func tasks(limit: Int, before: String?) async -> Outcome<TaskPage>
    /// Puts one of FERB's alerts away on every device (`attention.ack`, kb APP-1106, PHN-791).
    func dismissAlert(id: String) async -> Outcome<Void>
    func macInfo() async -> Outcome<MacInfo>
    func cachedMacInfo() async -> Outcome<MacInfo?>

    /// The owner's review of what an import brought in (kb MEM-460; both private: the Mac's unlock), and his answer to
    /// items of one part: the review as it stands after it.
    func memoryReview() async -> Outcome<MemoryReview>
    func answerMemoryReview(part: String, itemIds: [String], answer: String) async -> Outcome<MemoryReview>

    /// The temporary chat (the owner, 2026-11-23): one conversation the Mac holds in memory only, with every tool and
    /// no prior context; on and off only by the chat's dashed bubble. Read it (private: the Mac's unlock), turn it on
    /// or off (plain), and send to it; each answers the chat as it stands after the call. Nothing of it is kept on the
    /// phone: the library holds it in memory.
    func temporaryChat() async -> Outcome<TemporaryChat>
    func setTemporaryChat(on: Bool) async -> Outcome<TemporaryChat>
    func sendTemporary(_ text: String) async -> Outcome<TemporaryChat>

    /// Keys made after pairing the Mac doesn't hold (Face ID or Touch ID enrolled later), and registering them
    /// (PHN-133).
    func unregisteredKeys() async -> PublicKeys?
    func registerKeys(_ signer: @escaping @Sendable (KeyKind) -> Signer) async -> Outcome<Bool>
}

/// Listings and searches a page at a time (kb PHN-606): `after` is the page before's `next` ("Load more").
public extension PhoneApi {
    func retryLink() {}
    func files(path: String?, signer: Signer?) async -> Outcome<Folder> { await files(path: path, signer: signer, after: nil) }
    func searchFiles(query: String, path: String?) async -> Outcome<FoundFiles> { await searchFiles(query: query, path: path, after: nil) }
}

/// A live preview under way.
public final class Live: @unchecked Sendable {
    private let stopper: () -> Void
    public init(_ stop: @escaping () -> Void) { stopper = stop }
    public func stop() { stopper() }
}

/// The real phone.
@MainActor
public final class LibraryPhone: PhoneApi {
    public let phone: FerbPhone

    public init(_ phone: FerbPhone) {
        self.phone = phone
    }

    public var state: PhoneState { phone.state }
    public var link: LinkStatus? { phone.link }
    public func changes() -> AsyncStream<Change> { phone.changes() }

    public func transcript() async -> Outcome<TranscriptView?> {
        let read = await phone.call { m in
            let conv = try m.currentConversation() ?? m.conversations().first(where: \.current)?.id
            return try conv.map { try m.transcript(conversation: $0) }
        }
        // FERB's words laid out by the library (kb PHN-721..724) here, off the main actor, so the rows the screen builds
        // from this view find every layout kept (ChatModel.prepare)
        if case .ok(let v?) = read { await Task.detached(priority: .userInitiated) { ChatModel.prepare(v) }.value }
        return read
    }

    public func sync() async -> Outcome<Void> { await phone.call { try $0.sync() } }
    public func send(_ text: String) async -> Outcome<OutboxItem> { await phone.call { try $0.send(text: text) } }
    public func cancel(outboxId: String) async -> Outcome<Cancelled> { await phone.call { try $0.cancel(outboxId: outboxId) } }
    public func answer(_ approvalId: String, _ input: AnswerInput, signer: Signer?) async -> Outcome<String> {
        await phone.call { try $0.answer(approvalId: approvalId, answer: input, signer: signer) }
    }
    public func answerHidden(_ approvalId: String, secret: SecretInput, signer: Signer) async -> Outcome<String> {
        await phone.call { try $0.answerHidden(approvalId: approvalId, secret: secret, signer: signer) }
    }
    public func stop(taskIds: [String]) async -> Outcome<Void> { await phone.call { try $0.stop(taskIds: taskIds) } }
    public func loadOlder(conversation: String) async -> Outcome<OlderState> { await phone.call { try $0.loadOlder(conversation: conversation) } }
    public func unlock(_ signer: Signer) async -> Outcome<Void> { await phone.unlock(signer) }
    public func pair(_ code: String) async -> Outcome<Void> { await phone.pair(code) }
    public func forget() async -> Outcome<Bool> { await phone.forgetThisMac() }
    public func statementReady() async -> Bool { await phone.statementReady() }
    public func unlockWithStatement(payload: String, sig: String, ageMs: UInt64) async -> Outcome<Void> {
        await phone.unlockWithStatement(payload: payload, sig: sig, ageMs: ageMs)
    }
    public func setOpen(_ on: Bool) { phone.setOpen(on) }
    public func retryLink() { phone.retry() }
    public func lock() async -> Outcome<Bool> { await phone.lock() }
    public func unlocked() async -> Outcome<Void> { await phone.unlocked() }
    public func stopEverything() async -> Outcome<Void> { await phone.stopEverything() }
    public func status() async -> Outcome<PhoneStatus> { await phone.call { try $0.status() } }
    public func screenUse() async -> Outcome<ScreenUse> { await phone.call { try $0.screenUse() } }
    public func endScreenUse() async -> Outcome<ScreenUseEnd> { await phone.call { try $0.endScreenUse() } }
    public func insomniaStatus() async -> Outcome<Insomnia> { await phone.call { try $0.insomniaStatus() } }
    public func startInsomnia(variant: String, screenYes: Bool, signer: Signer) async -> Outcome<InsomniaStarted> {
        await phone.call { try $0.startInsomnia(variant: variant, screenYes: screenYes, signer: signer) }
    }
    public func endInsomnia() async -> Outcome<InsomniaEnded> { await phone.call { try $0.endInsomnia() } }
    public func lockMac() async -> Outcome<MacLocked> { await phone.call { try $0.lockMac() } }
    public func dismissInsomniaNotice(_ noticeId: String) async -> Outcome<Insomnia> {
        await phone.call { try $0.dismissInsomniaNotice(noticeId: noticeId) }
    }
    public func watchCamera(sink: CameraSink) async -> Outcome<Live> {
        await phone.call { m in
            let p = try m.watchCamera(sink: sink)
            return Live { p.stop() }
        }
    }
    public func lastStatus() async -> PhoneStatus? { await phone.lastStatus() }
    public func markReadLatest(conversation: String) async -> Outcome<Void> { await phone.call { try $0.markReadLatest(conversation: conversation) } }
    public func sendWith(_ text: String, attachments: [Attachment]) async -> Outcome<OutboxItem> {
        await phone.call { try $0.sendWith(text: text, attachments: attachments) }
    }
    public func files(path: String?, signer: Signer?, after: String?) async -> Outcome<Folder> { await phone.call { try $0.files(path: path, signer: signer, after: after) } }
    public func fetch(id: String, path: String, saveTo: String, offset: UInt64, signer: Signer?, listener: TransferListener) async -> Outcome<Fetched> {
        await phone.call { try $0.fetch(id: id, path: path, saveTo: saveTo, offset: offset, signer: signer, listener: listener) }
    }
    public func upload(id: String, localPath: String, name: String, resume: String?, group: String?, rel: String?,
                       listener: TransferListener) async -> Outcome<Uploaded> {
        await phone.call { try $0.upload(id: id, localPath: localPath, name: name, resume: resume, group: group, rel: rel, listener: listener) }
    }
    public func cancelTransfer(id: String) async -> Outcome<Void> { await phone.call { try $0.cancelTransfer(id: id) } }
    public func tree(path: String, signer: Signer?) async -> Outcome<Tree> { await phone.call { try $0.tree(path: path, signer: signer) } }
    public func putGroup(name: String) async -> Outcome<String> { await phone.call { try $0.putGroup(name: name) } }
    public func fetchTaskFile(id: String, taskId: String, index: UInt64, saveTo: String, listener: TransferListener) async -> Outcome<Fetched> {
        await phone.call { try $0.fetchTaskFile(id: id, taskId: taskId, index: index, saveTo: saveTo, listener: listener) }
    }
    public func recentFiles(limit: Int) async -> Outcome<RecentFiles> { await phone.call { try $0.recentFiles(limit: UInt32(limit)) } }
    public func searchFiles(query: String, path: String?, after: String?) async -> Outcome<FoundFiles> { await phone.call { try $0.searchFiles(query: query, path: path, after: after) } }
    public func reportDevice(_ report: DeviceReport) async -> Outcome<Bool> { await phone.call { try $0.reportDevice(report: report) } }
    public func locationWanted() async -> Outcome<String?> { await phone.call { try $0.locationWanted() } }
    public func answerLocation(requestId: String, fix: LocationFix?, whyNot: String?) async -> Outcome<Void> {
        await phone.call { try $0.answerLocation(requestId: requestId, fix: fix, whyNot: whyNot) }
    }
    public func watchLive(taskId: String, sink: PreviewSink) async -> Outcome<Live> {
        await phone.call { m in
            let p = try m.watchLive(taskId: taskId, sink: sink)
            return Live { p.stop() }
        }
    }
    public func watchThumbnail(taskId: String, everyMs: UInt64, sink: PreviewSink) async -> Outcome<Live> {
        await phone.call { m in
            let p = try m.watchThumbnail(taskId: taskId, everyMs: everyMs, sink: sink)
            return Live { p.stop() }
        }
    }
    public func watchStepThumbnail(taskId: String, worker: String, everyMs: UInt64, sink: PreviewSink) async -> Outcome<Live> {
        await phone.call { m in
            let p = try m.watchStepThumbnail(taskId: taskId, worker: worker, everyMs: everyMs, sink: sink)
            return Live { p.stop() }
        }
    }
    public func settings() async -> Outcome<[SettingValue]> { await phone.call { try $0.settings() } }
    public func setSetting(key: String, valueJson: String, signer: Signer) async -> Outcome<String> {
        await phone.call { try $0.setSetting(key: key, valueJson: valueJson, signer: signer) }
    }
    public func tasks(limit: Int, before: String?) async -> Outcome<TaskPage> { await phone.call { try $0.tasks(limit: UInt32(limit), before: before) } }
    public func dismissAlert(id: String) async -> Outcome<Void> { await phone.call { try $0.dismissAlert(id: id) } }
    public func macInfo() async -> Outcome<MacInfo> { await phone.call { try $0.macInfo() } }
    public func cachedMacInfo() async -> Outcome<MacInfo?> { await phone.call { try $0.cachedMacInfo() } }
    public func memoryReview() async -> Outcome<MemoryReview> { await phone.call { try $0.memoryReview() } }
    public func answerMemoryReview(part: String, itemIds: [String], answer: String) async -> Outcome<MemoryReview> {
        await phone.call { try $0.answerMemoryReview(part: part, itemIds: itemIds, answer: answer) }
    }
    public func temporaryChat() async -> Outcome<TemporaryChat> { await Self.laidOut(await phone.call { try $0.temporaryChat() }) }
    public func setTemporaryChat(on: Bool) async -> Outcome<TemporaryChat> {
        await Self.laidOut(await phone.call { try $0.setTemporaryChat(on: on) })
    }
    public func sendTemporary(_ text: String) async -> Outcome<TemporaryChat> {
        await Self.laidOut(await phone.call { try $0.sendTemporary(text: text) })
    }
    /// FERB's words in the temporary chat laid out off the main actor, as `transcript` does for the chat (kb PHN-682).
    private static func laidOut(_ read: Outcome<TemporaryChat>) async -> Outcome<TemporaryChat> {
        if case .ok(let t) = read, t.on {
            let v = t.transcript
            await Task.detached(priority: .userInitiated) { ChatModel.prepare(v) }.value
        }
        return read
    }
    public func unregisteredKeys() async -> PublicKeys? { await phone.unregisteredKeys() }
    public func registerKeys(_ signer: @escaping @Sendable (KeyKind) -> Signer) async -> Outcome<Bool> { await phone.registerKeys(signer) }
}
