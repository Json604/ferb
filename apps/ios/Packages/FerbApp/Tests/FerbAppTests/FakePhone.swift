import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import Foundation
import Observation
import XCTest

/// A latch a fake call waits on until the test (or a cancel) lets it go.
@MainActor
final class Latch {
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var open = false
    func wait() async {
        if open { return }
        await withCheckedContinuation { waiters.append($0) }
    }
    func release() {
        open = true
        let w = waiters
        waiters = []
        w.forEach { $0.resume() }
    }
}

/// A phone for the model's tests (Android's FakePhone.kt): properties the test sets, calls it records, answers it
/// scripts. No library.
@MainActor
@Observable
final class FakePhone: PhoneApi {
    var state: PhoneState = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: nil)
    var link: LinkStatus? = FakePhone.status()

    @ObservationIgnored private var subscribers: [AsyncStream<Change>.Continuation] = []
    @ObservationIgnored var view: TranscriptView?
    @ObservationIgnored var outbox: [OutboxItem] = []
    @ObservationIgnored var calls: [String] = []
    @ObservationIgnored var answers: [(AnswerInput, Signer?)] = []
    /// What each answer call says, in turn (then ok).
    @ObservationIgnored var answerScript: [Outcome<String>] = []
    @ObservationIgnored var unlockResult: Outcome<Void> = .ok(())
    @ObservationIgnored var stopResult: Outcome<Void> = .ok(())
    @ObservationIgnored var sendWaitingFor: String?
    @ObservationIgnored var status: PhoneStatus?
    @ObservationIgnored var folders: [String: Folder] = [:]
    @ObservationIgnored var signed: [String] = []
    /// A fetch writes `fetchBytes` (half, then all) unless `fetchGate` holds it, then it is cancelled or let go.
    @ObservationIgnored var fetchBytes = Data((0..<10).map { UInt8($0) })
    @ObservationIgnored var fetchGate: Latch?
    @ObservationIgnored var fetchRefusal: FerbError?
    @ObservationIgnored var cancelled: [String] = []
    @ObservationIgnored var uploadRefusal: FerbError?
    /// An upload waits here half way until it is let go (a cancel lets it go, refused as stopped).
    @ObservationIgnored var uploadGate: Latch?
    @ObservationIgnored var sentWith: [(String, [Attachment])] = []
    @ObservationIgnored var preview: PreviewSink?
    @ObservationIgnored var previewStopped = 0
    @ObservationIgnored var settingsValues: [(String, String)] = []
    @ObservationIgnored var settingsRefusal: FerbError?
    @ObservationIgnored var tasks: Outcome<TaskPage> = .ok(TaskPage(tasks: [], more: false))
    @ObservationIgnored var mac: MacInfo?
    @ObservationIgnored var newKeys: PublicKeys?
    @ObservationIgnored var location: String?
    @ObservationIgnored var reports: [DeviceReport] = []

    func changes() -> AsyncStream<Change> {
        let (stream, cont) = AsyncStream<Change>.makeStream()
        subscribers.append(cont)
        return stream
    }

    func emit(_ c: Change) { for s in subscribers { s.yield(c) } }

    func transcript() async -> Outcome<TranscriptView?> {
        guard var v = view else { return .ok(nil) }
        v.outbox = outbox
        return .ok(v)
    }
    /// What each sync says, in turn (then ok).
    var syncScript: [Outcome<Void>] = []
    func sync() async -> Outcome<Void> {
        calls.append("sync")
        return syncScript.isEmpty ? .ok(()) : syncScript.removeFirst()
    }
    func send(_ text: String) async -> Outcome<OutboxItem> {
        calls.append("send:\(text)")
        let item = OutboxItem(id: "o\(outbox.count + 1)", text: text, createdAt: "2026-01-02T10:00:00+00:00", state: "waiting",
                              waitingFor: sendWaitingFor ?? "link", reason: nil, tried: false, attachments: [])
        outbox.append(item)
        return .ok(item)
    }
    func cancel(outboxId: String) async -> Outcome<Cancelled> {
        calls.append("cancel:\(outboxId)")
        outbox.removeAll { $0.id == outboxId }
        return .ok(Cancelled(mayHaveArrived: false))
    }
    func answer(_ approvalId: String, _ input: AnswerInput, signer: Signer?) async -> Outcome<String> {
        calls.append("answer:\(approvalId):\(signer != nil)")
        answers.append((input, signer))
        _ = signer?.sign(payload: "payload")
        return answerScript.isEmpty ? .ok("{}") : answerScript.removeFirst()
    }
    func answerHidden(_ approvalId: String, secret: SecretInput, signer: Signer) async -> Outcome<String> {
        calls.append("hidden:\(approvalId):\(secret.len())")
        secret.clear()
        guard signer.sign(payload: "sealed") != nil else { return .failed(.NotSigned(detail: "Nothing was sent.", linkCode: nil)) }
        return .ok("{}")
    }
    func stop(taskIds: [String]) async -> Outcome<Void> { calls.append("stop:\(taskIds.joined(separator: ","))"); return stopResult }
    func loadOlder(conversation: String) async -> Outcome<OlderState> { calls.append("older"); return .ok(.complete) }
    /// Hold the Mac's challenge (before the prompt) or its answer (after it) until the test opens them (PHN-641).
    var challengeHeld = false
    var answerHeld = false
    func unlock(_ signer: Signer) async -> Outcome<Void> {
        calls.append("unlock")
        while challengeHeld { try? await Task.sleep(nanoseconds: 5_000_000) }
        if case .ok = unlockResult, signer.sign(payload: "challenge") == nil { return .failed(.NotSigned(detail: "Nothing was sent.", linkCode: nil)) }
        while answerHeld { try? await Task.sleep(nanoseconds: 5_000_000) }
        return unlockResult
    }
    /// The assertions pairings carried (PHN-828), and what the pair call answers.
    @ObservationIgnored var pairedAccounts: [String] = []
    @ObservationIgnored var pairResult: Outcome<Void> = .ok(())
    @ObservationIgnored var endpoint: String? = "invented-endpoint-1"
    func pair(_ code: String, account: String) async -> Outcome<Void> {
        calls.append("pair:\(code)")
        pairedAccounts.append(account)
        return pairResult
    }
    func endpointId() async -> String? { endpoint }
    /// What binding a legacy pairing answers (PHN-828), and the assertions it carried.
    @ObservationIgnored var bindResult: Outcome<Bool> = .ok(true)
    @ObservationIgnored var bound: [String] = []
    func bindAccount(assertion: String) async -> Outcome<Bool> {
        calls.append("bind")
        bound.append(assertion)
        return bindResult
    }
    var macForgot = true
    func forget() async -> Outcome<Bool> { calls.append("forget"); return .ok(macForgot) }

    /// PHN-649: the statement path; `statementResults` answer in turn (then ok); what was sent, with its age.
    var statementOk = true
    var statementResults: [Outcome<Void>] = []
    var statements: [(String, UInt64)] = []
    func statementReady() async -> Bool { statementOk && link?.paired != false }
    func unlockWithStatement(payload: String, sig: String, ageMs: UInt64) async -> Outcome<Void> {
        calls.append("statement")
        statements.append((payload, ageMs))
        return statementResults.isEmpty ? .ok(()) : statementResults.removeFirst()
    }
    var opens: [Bool] = []
    func setOpen(_ on: Bool) { opens.append(on) }
    func lock() async -> Outcome<Bool> { calls.append("lock"); return .ok(true) }
    func unlocked() async -> Outcome<Void> { calls.append("unlocked"); return .ok(()) }
    func stopEverything() async -> Outcome<Void> { calls.append("stop-everything"); return .ok(()) }
    /// "Until I'm back" (PHN-611): what the Mac says about screen use, and what "Ask me again" gets.
    var screenUseNow = ScreenUse(active: false, since: nil, phone: nil, phoneName: nil, canOffer: true)
    func screenUse() async -> Outcome<ScreenUse> { calls.append("screen-use"); return .ok(screenUseNow) }
    func endScreenUse() async -> Outcome<ScreenUseEnd> {
        calls.append("end-screen-use")
        let was = screenUseNow.active
        screenUseNow = ScreenUse(active: false, since: nil, phone: nil, phoneName: nil, canOffer: screenUseNow.canOffer)
        return .ok(ScreenUseEnd(ended: was, by: "phone-1", screenUse: screenUseNow))
    }
    /// Insomnia mode (APP-980..985): what the Mac says, what each call gets, the camera's sink and its stops.
    var insomnia = Insomnia(on: false, variant: nil, since: nil, by: nil, screenYes: false, graceMinutes: nil, working: 0, endsAt: nil,
                            shieldUp: false, macLocked: false, camera: InsomniaCamera(on: false, state: "unknown"),
                            power: InsomniaPower(battery: nil, onBattery: nil, lidClosed: nil, thermal: nil), warnings: [], notice: nil,
                            line: "", taskAwake: false, lastEnd: nil)
    @ObservationIgnored var insomniaRefusal: FerbError?
    @ObservationIgnored var camera: CameraSink?
    @ObservationIgnored var cameraStopped = 0
    @ObservationIgnored var cameraRefusal: FerbError?
    private func withInsomnia(_ i: Insomnia) {
        insomnia = i
        status?.insomnia = i
    }
    func insomniaStatus() async -> Outcome<Insomnia> { calls.append("insomnia"); return .ok(insomnia) }
    func startInsomnia(variant: String, screenYes: Bool, signer: Signer) async -> Outcome<InsomniaStarted> {
        calls.append("insomnia-start:\(variant):\(screenYes)")
        guard let sig = signer.sign(payload: "insomnia.start") else { return .failed(.NotSigned(detail: "Nothing was sent.", linkCode: nil)) }
        signed.append(sig)
        if let e = insomniaRefusal { insomniaRefusal = nil; return .failed(e) }
        let already = insomnia.on
        if !already {
            var i = insomnia
            i.on = true
            i.variant = variant
            i.screenYes = screenYes
            i.by = "phone-1"
            i.line = "Keeping your Mac working · 1 task running · battery 64%"
            withInsomnia(i)
        }
        return .ok(InsomniaStarted(already: already, insomnia: insomnia))
    }
    func endInsomnia() async -> Outcome<InsomniaEnded> {
        calls.append("insomnia-end")
        let was = insomnia.on
        var i = insomnia
        i.on = false
        i.variant = nil
        i.notice = nil
        i.warnings = []
        i.line = ""
        withInsomnia(i)
        return .ok(InsomniaEnded(ended: was, by: "phone-1", insomnia: insomnia))
    }
    func lockMac() async -> Outcome<MacLocked> {
        calls.append("lock-mac")
        if let e = insomniaRefusal { insomniaRefusal = nil; return .failed(e) }
        var i = insomnia
        i.macLocked = true
        i.notice = nil
        withInsomnia(i)
        return .ok(MacLocked(locked: true, text: "Your Mac is locked.", insomnia: insomnia))
    }
    func dismissInsomniaNotice(_ noticeId: String) async -> Outcome<Insomnia> {
        calls.append("insomnia-fine:\(noticeId)")
        var i = insomnia
        i.notice = nil
        withInsomnia(i)
        return .ok(insomnia)
    }
    func watchCamera(sink: CameraSink) async -> Outcome<Live> {
        calls.append("camera")
        if let e = cameraRefusal { return .failed(e) }
        camera = sink
        return .ok(Live { [weak self] in Task { @MainActor in self?.cameraStopped += 1 } })
    }
    func status() async -> Outcome<PhoneStatus> {
        calls.append("status")
        return status.map { .ok($0) } ?? .failed(.Offline(detail: "no", linkCode: nil))
    }
    func lastStatus() async -> PhoneStatus? { status }
    func markReadLatest(conversation: String) async -> Outcome<Void> { calls.append("mark:\(conversation)"); return .ok(()) }
    func sendWith(_ text: String, attachments: [Attachment]) async -> Outcome<OutboxItem> {
        sentWith.append((text, attachments))
        let item = OutboxItem(id: "o\(outbox.count + 1)", text: text, createdAt: "2026-01-02T10:00:00+00:00", state: "waiting",
                              waitingFor: "link", reason: nil, tried: false, attachments: attachments)
        outbox.append(item)
        return .ok(item)
    }
    func files(path: String?, signer: Signer?, after: String?) async -> Outcome<Folder> {
        if let after {
            calls.append("files-more:\(path ?? "-"):\(after):\(signer != nil)")
            return morePages.isEmpty ? .failed(.NotFound(detail: "No more.", linkCode: nil)) : .ok(morePages.removeFirst())
        }
        calls.append("files:\(path ?? "-"):\(signer != nil)")
        if let s = signer?.sign(payload: "files:\(path ?? "")") { signed.append(s) }
        return folders[path ?? "~"].map { .ok($0) } ?? .failed(.NotFound(detail: "No such folder.", linkCode: nil))
    }
    func fetch(id: String, path: String, saveTo: String, offset: UInt64, signer: Signer?, listener: TransferListener) async -> Outcome<Fetched> {
        calls.append("fetch:\(path)@\(offset):\(signer != nil)")
        if let s = signer?.sign(payload: "get:\(path)") { signed.append(s) }
        if let r = fetchRefusal { return .failed(r) }
        let half = fetchBytes.count / 2
        if offset == 0 { try? fetchBytes.prefix(half).write(to: URL(fileURLWithPath: saveTo)) }
        listener.progress(done: UInt64(half), of: UInt64(fetchBytes.count))
        if let g = fetchGate {
            await g.wait()
            if cancelled.contains(id) { return .failed(.Cancelled(detail: "Stopped.", linkCode: "cancelled")) }
        }
        try? fetchBytes.write(to: URL(fileURLWithPath: saveTo))
        listener.progress(done: UInt64(fetchBytes.count), of: UInt64(fetchBytes.count))
        return .ok(Fetched(name: (path as NSString).lastPathComponent, savedTo: saveTo, received: UInt64(fetchBytes.count) - offset,
                           total: UInt64(fetchBytes.count)))
    }
    func upload(id: String, localPath: String, name: String, resume: String?, group: String?, rel: String?,
                listener: TransferListener) async -> Outcome<Uploaded> {
        calls.append("upload:\(name):\(resume ?? "-")")
        if let r = uploadRefusal { return .failed(r) }
        let size = ((try? FileManager.default.attributesOfItem(atPath: localPath)[.size]) as? NSNumber)?.uint64Value ?? 0
        listener.accepted(uploadId: resume ?? "up-\(name)", offset: 0)
        if let g = uploadGate {
            listener.progress(done: size / 2, of: size)
            await g.wait()
            if cancelled.contains(id) { return .failed(.Cancelled(detail: "Stopped.", linkCode: "cancelled")) }
        }
        listener.progress(done: size, of: size)
        return .ok(Uploaded(uploadId: resume ?? "up-\(name)", saved: "~/Downloads/FERB/\(name)", size: size, savedIn: nil))
    }
    func cancelTransfer(id: String) async -> Outcome<Void> {
        cancelled.append(id)
        fetchGate?.release()
        uploadGate?.release()
        return .ok(())
    }
    func tree(path: String, signer: Signer?) async -> Outcome<Tree> { .failed(.NotFound(detail: "No such folder.", linkCode: nil)) }
    func putGroup(name: String) async -> Outcome<String> { calls.append("group:\(name)"); return .ok("g-\(name)") }
    func fetchTaskFile(id: String, taskId: String, index: UInt64, saveTo: String, listener: TransferListener) async -> Outcome<Fetched> {
        .failed(.NotFound(detail: "No such file.", linkCode: nil))
    }
    @ObservationIgnored var recent = RecentFiles(files: [], skipped: 0)
    func recentFiles(limit: Int) async -> Outcome<RecentFiles> { calls.append("recent"); return .ok(recent) }
    var found: FoundFiles?
    func searchFiles(query: String, path: String?, after: String?) async -> Outcome<FoundFiles> {
        if let after {
            calls.append("search-more:\(query):\(after)")
            return moreFound.isEmpty ? .failed(.NotFound(detail: "No more.", linkCode: nil)) : .ok(moreFound.removeFirst())
        }
        return .ok(found ?? FoundFiles(path: path ?? "~", query: query, files: [], looked: 0, skipped: 0, cut: false, incomplete: false,
                                       from: 0, next: nil))
    }
    /// Paging (PHN-606): the pages "Load more" gets in turn (each asked with the page before's `next`).
    var morePages: [Folder] = []
    var moreFound: [FoundFiles] = []
    func reportDevice(_ report: DeviceReport) async -> Outcome<Bool> { reports.append(report); return .ok(true) }
    func locationWanted() async -> Outcome<String?> { .ok(location) }
    func answerLocation(requestId: String, fix: LocationFix?, whyNot: String?) async -> Outcome<Void> {
        calls.append("location:\(requestId):\(fix != nil):\(whyNot ?? "-")")
        return .ok(())
    }
    /// The work row's pictures asked for, by task, and their sinks (kb PHN-725).
    var thumbnails: [String: PreviewSink] = [:]
    var thumbnailsStopped = 0
    func watchStepThumbnail(taskId: String, worker: String, everyMs: UInt64, sink: PreviewSink) async -> Outcome<Live> {
        calls.append("step-thumbnail:\(taskId):\(worker):\(everyMs)")
        thumbnails[Thumbnails.key(taskId, worker)] = sink
        return .ok(Live { [weak self] in Task { @MainActor in self?.thumbnailsStopped += 1 } })
    }
    func watchThumbnail(taskId: String, everyMs: UInt64, sink: PreviewSink) async -> Outcome<Live> {
        calls.append("thumbnail:\(taskId):\(everyMs)")
        thumbnails[taskId] = sink
        return .ok(Live { [weak self] in Task { @MainActor in self?.thumbnailsStopped += 1 } })
    }
    func watchLive(taskId: String, sink: PreviewSink) async -> Outcome<Live> {
        calls.append("live:\(taskId)")
        preview = sink
        return .ok(Live { [weak self] in Task { @MainActor in self?.previewStopped += 1 } })
    }
    func settings() async -> Outcome<[SettingValue]> {
        calls.append("settings")
        if let r = settingsRefusal {
            settingsRefusal = nil
            return .failed(r)
        }
        return .ok(settingsValues.map { SettingValue(key: $0.0, valueJson: $0.1) })
    }
    func setSetting(key: String, valueJson: String, signer: Signer) async -> Outcome<String> {
        calls.append("set:\(key)=\(valueJson)")
        guard let sig = signer.sign(payload: "settings.set") else { return .failed(.NotSigned(detail: "Nothing was sent.", linkCode: nil)) }
        signed.append(sig)
        if let i = settingsValues.firstIndex(where: { $0.0 == key }) { settingsValues[i].1 = valueJson } else { settingsValues.append((key, valueJson)) }
        return .ok(valueJson)
    }
    func tasks(limit: Int, before: String?) async -> Outcome<TaskPage> { calls.append("tasks"); return tasks }
    /// The alerts put away through `dismissAlert`, in order.
    var dismissed: [String] = []
    func dismissAlert(id: String) async -> Outcome<Void> { calls.append("dismissAlert"); dismissed.append(id); return .ok(()) }
    func macInfo() async -> Outcome<MacInfo> { calls.append("mac"); return mac.map { .ok($0) } ?? .failed(.Offline(detail: "no", linkCode: nil)) }
    func cachedMacInfo() async -> Outcome<MacInfo?> { .ok(mac) }

    /// The review of an import as the Mac holds it; an answer takes its items out. `reviewRefusal` refuses the next call.
    @ObservationIgnored var review = MemoryReview(parts: [], count: 0)
    @ObservationIgnored var reviewRefusal: FerbError?
    @ObservationIgnored var reviewAnswers: [(part: String, ids: [String], answer: String)] = []
    func memoryReview() async -> Outcome<MemoryReview> {
        calls.append("review")
        if let e = reviewRefusal { reviewRefusal = nil; return .failed(e) }
        return .ok(review)
    }
    func answerMemoryReview(part: String, itemIds: [String], answer: String) async -> Outcome<MemoryReview> {
        calls.append("review-answer:\(part):\(itemIds.joined(separator: ",")):\(answer)")
        if let e = reviewRefusal { reviewRefusal = nil; return .failed(e) }
        reviewAnswers.append((part, itemIds, answer))
        let gone = Set(itemIds)
        review.parts = review.parts.compactMap { p in
            var p = p
            if p.part == part { p.groups = p.groups.map { g in var g = g; g.items.removeAll { gone.contains($0.id) }; return g }.filter { !$0.items.isEmpty } }
            return p.groups.isEmpty ? nil : p
        }
        review.count = UInt64(review.parts.flatMap(\.groups).flatMap(\.items).count)
        return .ok(review)
    }

    /// The temporary chat as the Mac holds it (memory only); a refusal answers the next toggle; a send adds the
    /// owner's words and, when set, FERB's `temporaryReply`.
    @ObservationIgnored var temporaryNow = FakePhone.temporaryOff
    @ObservationIgnored var temporaryRefusal: FerbError?
    @ObservationIgnored var temporaryReply: String?
    /// A temporary send waits here (its message on its way) until it is let go.
    @ObservationIgnored var temporaryGate: Latch?
    @ObservationIgnored private var temporaryCount = 0
    static let temporaryOff = TemporaryChat(on: false, id: nil, startedAt: nil, rev: 0,
                                            transcript: TranscriptView(conversation: "", items: [], questions: [], live: [], outbox: [], older: .complete))
    func temporaryChat() async -> Outcome<TemporaryChat> { calls.append("temporary"); return .ok(temporaryNow) }
    func setTemporaryChat(on: Bool) async -> Outcome<TemporaryChat> {
        calls.append("temporary-set:\(on)")
        if let e = temporaryRefusal { temporaryRefusal = nil; return .failed(e) }
        if on && !temporaryNow.on {
            temporaryCount += 1
            let id = "tmp\(temporaryCount)"
            temporaryNow = TemporaryChat(on: true, id: id, startedAt: "2026-11-23T10:00:00+05:30", rev: temporaryNow.rev + 1,
                                         transcript: TranscriptView(conversation: "temporary:\(id)", items: [], questions: [], live: [],
                                                                    outbox: [], older: .complete))
        } else if !on {
            temporaryNow = FakePhone.temporaryOff
        }
        return .ok(temporaryNow)
    }
    func sendTemporary(_ text: String) async -> Outcome<TemporaryChat> {
        calls.append("temporary-send:\(text)")
        if let g = temporaryGate { await g.wait() }
        if let e = temporaryRefusal { temporaryRefusal = nil; return .failed(e) }
        guard temporaryNow.on else { return .failed(.Refused(detail: "No temporary chat is on.", linkCode: nil)) }
        var v = temporaryNow.transcript
        let n = v.items.count
        v.items.append(FakePhone.ownerTurn("t-e\(n)", text, task: "tmp-task-\(n)"))
        if let reply = temporaryReply {
            v.items.append(.notice(notice: NoticeItem(entry: "t-e\(n + 1)", at: 0, legacy: false, reference: "t-e\(n + 1)", after: nil,
                                                      text: reply, quotes: [], cards: nil)))
        }
        temporaryNow = TemporaryChat(on: true, id: temporaryNow.id, startedAt: temporaryNow.startedAt, rev: temporaryNow.rev + 1, transcript: v)
        return .ok(temporaryNow)
    }

    /// The owner's words as the library hands a turn.
    static func ownerTurn(_ entry: String, _ text: String, task: String? = nil) -> Item {
        .turn(turn: TurnItem(entry: entry, at: 0, legacy: false, clientId: nil, requestId: "r-\(entry)", taskId: task, from: "owner",
                             taskName: nil, status: "running", statusInferred: false, notice: nil, mergedInto: nil, question: nil,
                             answeredBelow: false, answer: nil, card: nil, checklist: nil, firstReply: nil, parts: [], goal: nil,
                             words: OwnerWords(text: text, files: [], hidden: [], sealed: [], hiddenNotice: nil)))
    }

    func unregisteredKeys() async -> PublicKeys? { newKeys }
    func registerKeys(_ signer: @escaping @Sendable (KeyKind) -> Signer) async -> Outcome<Bool> {
        guard let k = newKeys else { return .ok(false) }
        guard let s = signer(.lock).sign(payload: "link.keys") else { return .failed(.NotSigned(detail: "Nothing was sent.", linkCode: nil)) }
        signed.append(s)
        calls.append("keys:\(k.strong ?? "-")")
        newKeys = nil
        return .ok(true)
    }

    static func status(_ phase: LinkPhase = .online, paired: Bool = true, path: LinkPath = .direct, code: String? = nil,
                       problem: LinkProblem? = nil) -> LinkStatus {
        LinkStatus(phase: phase, path: path, tierWords: nil, pairingCode: code, paired: paired, bulkAllowed: path != .publicRelay,
                   slow: false, ownRelayUrl: nil, ownRelayAlways: false, relay: nil, problem: problem, retryInS: nil, macOlder: false,
                   macNewer: false)
    }

    static func refused(_ linkCode: String) -> FerbError { .NeedsUnlock(detail: "The Mac wants the phone's proof.", linkCode: linkCode) }
}

/// A signer standing in for the prompt: it says which key it was asked for and signs, or the owner cancels.
final class FakeSigner: Signer, @unchecked Sendable {
    let kind: KeyKind
    private let cancel: Bool
    private(set) var asked = 0
    init(_ kind: KeyKind, cancel: Bool = false) {
        self.kind = kind
        self.cancel = cancel
    }
    func sign(payload: String) -> String? {
        asked += 1
        return cancel ? nil : "sig-\(kind)"
    }
}

/// The prompts as a test has them: each signer made is recorded with why.
@MainActor
final class FakePrompts: Prompts {
    var signers: [FakeSigner] = []
    var whys: [Why] = []
    var cancel = false
    var localResult = true
    private(set) var localAsked = 0
    /// Every prompt the owner saw: a signing prompt asked to sign, or the prompt alone.
    var shown: Int { signers.reduce(0) { $0 + $1.asked } + localAsked }
    func signer(_ kind: KeyKind, why: Why) -> Signer? {
        let s = FakeSigner(kind, cancel: cancel)
        signers.append(s)
        whys.append(why)
        return s
    }
    func local() async -> Bool {
        localAsked += 1
        return localResult
    }
}

/// A clock the test moves (monotonic seconds).
final class TestClock: @unchecked Sendable {
    var t: Double = 0
}

/// Everything one model test needs, in temporary places only (never the phone's own defaults or folders).
@MainActor
final class Rig {
    let phone = FakePhone()
    let clock = TestClock()
    let session: Session
    let prompts = FakePrompts()
    let root: URL
    let local: LocalFiles
    let suite = "ferb-app-test-\(UUID().uuidString)"
    let prefs: Prefs
    /// FERB's account (kb PHN-827): signed in as an invented account unless a test clears it; its Keychain item in
    /// memory, the site a fake (no network).
    let accountStore = MemoryStore()
    let http = FakeHttp()
    let web = FakeWeb()
    let account: AccountModel
    var screenLock = true
    /// Short waits, so a test sees the bounds without real seconds (Android's tests move a test clock instead).
    var timing = UnlockTiming(statementAge: 100, statementRetry: 0.02, statementTries: 6, macSlow: 0.4, recheck: 0.02, rechecks: 2)

    init() {
        let c = clock
        session = Session(now: { c.t }, sleep: { _ in try? await Task.sleep(nanoseconds: 3_600_000_000_000) })
        root = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-app-\(UUID().uuidString)", isDirectory: true)
        local = LocalFiles(root: root)
        prefs = Prefs(defaults: UserDefaults(suiteName: suite)!)
        prefs.accountDeviceId = "test-device-000001"
        try? accountStore.set("account", AccountModel.savedItem(Rig.someone, accessToken: "fa1_first", accessExpiresAt: Date().addingTimeInterval(3600),
                                                                refreshToken: "fr1_first"))
        try? accountStore.set("account.last", Data(Rig.someone.id.utf8))
        account = AccountModel(store: accountStore, http: http, web: web, prefs: prefs, version: "1.0 test", deviceName: "Invented iPhone")
    }

    nonisolated static let someone = AccountInfo(id: "11111111-1111-4111-8111-111111111111", email: "someone@example.com", name: "Someone",
                                     providers: ["google"])

    func model(corpus: Corpus? = nil, locator: Locator? = nil, now: (() -> Date)? = nil) -> AppModel {
        let m = AppModel(api: phone, account: account, session: session, prefs: prefs, local: local, screenLock: { [weak self] in self?.screenLock ?? true },
                         corpus: corpus, now: now ?? Date.init, locator: locator, timing: timing, steady: { [clock] in clock.t })
        m.prompts = prompts
        return m
    }

    func tearDown() {
        UserDefaults().removePersistentDomain(forName: suite)
        try? FileManager.default.removeItem(at: root)
    }
}

/// Waits (bounded) for work that hops through the main actor's queue.
@MainActor
func eventually(_ what: String = "", timeout: Double = 5, file: StaticString = #filePath, line: UInt = #line,
                _ cond: () -> Bool) async {
    let end = Date().addingTimeInterval(timeout)
    while !cond() && Date() < end { try? await Task.sleep(nanoseconds: 5_000_000) }
    XCTAssertTrue(cond(), "waited for \(what)", file: file, line: line)
}

/// Lets what is queued on the main actor run (for checks that something did NOT happen).
@MainActor
func settle() async { try? await Task.sleep(nanoseconds: 100_000_000) }

func needs(unlock: Bool, fingerprint: Bool = false, can: Bool = true, hidden: Bool = false) -> QuestionNeeds {
    QuestionNeeds(canAnswer: can, needsUnlock: unlock, hidden: hidden, fingerprint: fingerprint, challenge: "ch-1",
                  problem: can ? nil : "Answer it on your Mac.", untilBack: false)
}

func asked(_ id: String, _ n: QuestionNeeds, held: Bool = false, capability: String? = "ferb.confirm") -> StatusQuestion {
    StatusQuestion(approvalId: id, kind: "approve", capability: capability, taskId: "t-1", reason: held ? "FERB has a question" : "Pay $5?",
                   options: held ? [] : ["Yes", "No"], held: held, needs: n)
}

func phoneStatus(_ q: StatusQuestion..., results: [StatusResult] = [], stopped: Bool = false) -> PhoneStatus {
    PhoneStatus(locked: false, words: true, stopped: stopped, questions: q, running: [], results: results, screenUse: nil)
}

func emptyView() -> TranscriptView {
    TranscriptView(conversation: "c1", items: [], questions: [], live: [], outbox: [], older: .complete)
}

func questionItem(_ id: String, open: Bool = true,
                  body: String = #"{"kind":"owner_question","reason":"Go ahead?","options":["Yes","No"],"capability":"plan.ask"}"#) -> QuestionItem {
    QuestionItem(entry: "e-\(id)", approvalId: id, requestId: "r-1", taskId: "t-1", state: open ? "open" : "answered", open: open,
                 bodyJson: body, shown: "end_of_chat", row: nil)
}

func view(_ qs: QuestionItem...) -> TranscriptView {
    TranscriptView(conversation: "c1", items: [], questions: qs, live: [], outbox: [], older: .complete)
}

extension FerbError {
    var isNotSigned: Bool { if case .NotSigned = self { return true } else { return false } }
}
