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
    func pair(_ code: String) async -> Outcome<Void> { calls.append("pair:\(code)"); return .ok(()) }
    func forget() async -> Outcome<Void> { calls.append("forget"); return .ok(()) }
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
    func status() async -> Outcome<PhoneStatus> {
        calls.append("status")
        return status.map { .ok($0) } ?? .failed(.Offline(detail: "no", linkCode: nil))
    }
    func lastStatus() -> PhoneStatus? { status }
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
        listener.progress(done: size, of: size)
        return .ok(Uploaded(uploadId: resume ?? "up-\(name)", saved: "~/Downloads/FERB/\(name)", size: size, savedIn: nil))
    }
    func cancelTransfer(id: String) async -> Outcome<Void> {
        cancelled.append(id)
        fetchGate?.release()
        return .ok(())
    }
    func tree(path: String, signer: Signer?) async -> Outcome<Tree> { .failed(.NotFound(detail: "No such folder.", linkCode: nil)) }
    func putGroup(name: String) async -> Outcome<String> { calls.append("group:\(name)"); return .ok("g-\(name)") }
    func fetchTaskFile(id: String, taskId: String, index: UInt64, saveTo: String, listener: TransferListener) async -> Outcome<Fetched> {
        .failed(.NotFound(detail: "No such file.", linkCode: nil))
    }
    func recentFiles(limit: Int) async -> Outcome<RecentFiles> { calls.append("recent"); return .ok(RecentFiles(files: [], skipped: 0)) }
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
    func macInfo() async -> Outcome<MacInfo> { calls.append("mac"); return mac.map { .ok($0) } ?? .failed(.Offline(detail: "no", linkCode: nil)) }
    func cachedMacInfo() async -> Outcome<MacInfo?> { .ok(mac) }
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
    var screenLock = true
    /// Short waits, so a test sees the bounds without real seconds (Android's tests move a test clock instead).
    var timing = UnlockTiming(promptWait: 0.3, macSlow: 0.4, recheck: 0.02, rechecks: 2)

    init() {
        let c = clock
        session = Session(now: { c.t }, sleep: { _ in try? await Task.sleep(nanoseconds: 3_600_000_000_000) })
        root = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-app-\(UUID().uuidString)", isDirectory: true)
        local = LocalFiles(root: root)
        prefs = Prefs(defaults: UserDefaults(suiteName: suite)!)
    }

    func model(corpus: Corpus? = nil, locator: Locator? = nil) -> AppModel {
        let m = AppModel(api: phone, session: session, prefs: prefs, local: local, screenLock: { [weak self] in self?.screenLock ?? true },
                         corpus: corpus, locator: locator, timing: timing)
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
