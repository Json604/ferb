import AppKit
import AVFAudio
import NotchCore

private typealias Pick = IntroCopy.Pick

/// What the owner answered on the line showing: a choice, or what they typed.
enum IntroAnswer: Equatable {
    case choice(String)
    case entry(String)
}

/// FERB.app's first-run tour (notes/setup-intro.md): Meet → Sign in → Allow all → Pair browsers → FERB's email →
/// Try → Control → Home. FERB speaks in text lines in the notch, the orb points at places on screen, every grant counts
/// only when the system confirms it, the demo looks and tells but never acts, and nothing the tour runs is real work:
/// "Do it for real" is sent as the owner's message after Home. Skippable at any point (everything it drew or said is
/// torn down at once), resumable at the beat it was on (`IntroProgress`). SetupModel is the service layer; the beats
/// are in IntroSetupBeats.swift and IntroDemoBeats.swift, the words in IntroCopy.swift.
@MainActor
final class IntroController {
    enum Timing {
        /// How long a line stays before the tour goes on by itself (the owner reads it; nothing waits on them).
        static let hello = 3.5, scan = 4.0, home = 5.0, line = 1.6
        /// A wait on something outside the app (sign-in, Login Items), read again this often.
        static let poll = 1.5
    }

    nonisolated static let terminationReason = "FERB's first-run intro"

    let setup: SetupModel
    let orb: OrbControlling
    let sound: FerbSoundPlaying
    let notch: NotchModel
    let controls: IntroControlFeeding
    let sleeper: IntroSleeping
    var progress: IntroProgress
    private let clipSeconds: Double
    /// The tour ended (finished or skipped); `remainingRequired` says what a skip left.
    var onFinished: (() -> Void)?
    private(set) var beat: IntroBeat?
    /// After a skip: the required steps still missing ("sign_in", then permission ids), for the compact setup.
    private(set) var remainingRequired: [String] = []
    private(set) var running = false

    private var run: Task<Void, Never>?
    private var clipEnded = true
    private var clipWaiter: (() -> Void)?
    /// Every wait of the tour's, ended with nothing when the tour ends: nothing is left waiting on a tour that's gone.
    private var waiters: [UUID: () -> Void] = [:]
    /// The owner's answer to the line showing.
    private var answer: ((IntroAnswer) -> Void)?
    /// Choices that act at once while the tour waits on the system (Show FERB in Finder, Restart FERB to check).
    var sideChoices: [String: () -> Void] = [:]
    /// The previewed request the owner chose "Do it for real" on: sent as their message once the tour has ended.
    var realTask: String?
    /// The last preview shown, replayed as running for practising Stop.
    var lastPreview: (request: String, events: [[String: Any]])?
    var replayID: UUID?
    var seenControls: Set<IntroControl> = []
    var controlWaiter: (() -> Void)?
    private var activation: NSObjectProtocol?

    var rpc: DaemonRPCTransport { setup.rpc }
    /// The tour is on and this beat wasn't skipped.
    var live: Bool { running && !Task.isCancelled }

    /// `clipSeconds`: the first-launch clip's length (nil: read from the bundled file).
    init(setup: SetupModel, orb: OrbControlling, sound: FerbSoundPlaying, notch: NotchModel, progress: IntroProgress,
         controls: IntroControlFeeding, sleeper: IntroSleeping? = nil, clipSeconds: Double? = nil) {
        self.setup = setup
        self.orb = orb
        self.sound = sound
        self.notch = notch
        self.progress = progress
        self.controls = controls
        self.sleeper = sleeper ?? IntroTaskSleeper()
        self.clipSeconds = clipSeconds ?? Self.firstLaunchClipSeconds() ?? 0
    }

    deinit {
        guard running else { return }
        let orb = orb, sound = sound
        Task { @MainActor in
            orb.tearDown()
            sound.keepWarm(false)
            sound.stopAll()
        }
        ProcessInfo.processInfo.enableAutomaticTermination(Self.terminationReason)
    }

    /// The bundled first-launch clip's length, in seconds.
    static func firstLaunchClipSeconds() -> Double? {
        guard let url = FerbSoundFiles.files(for: .firstLaunch, subtle: false).first,
              let file = try? AVAudioFile(forReading: url), file.processingFormat.sampleRate > 0 else { return nil }
        return Double(file.length) / file.processingFormat.sampleRate
    }

    // MARK: Start, finish, skip

    /// Runs `beats` (IntroPlan.beats) from the beat a quit left (`IntroProgress.resumeBeat`), if any. `firstRun`: a new
    /// install (Task 9 sets it), whose tour opens with the first-launch clip; a replay or a resume plays none.
    func start(beats: [IntroBeat], firstRun: Bool = false) {
        guard !running else { return }
        let resume = progress.resumeBeat
        var plan = beats
        if let resume {
            let rest = beats.filter { $0.rawValue >= resume.rawValue }
            if !rest.isEmpty { plan = rest }
        }
        guard !plan.isEmpty else { return }
        running = true
        ProcessInfo.processInfo.disableAutomaticTermination(Self.terminationReason)
        sound.keepWarm(true)
        orb.onHomeArrival = { [weak notch] in notch?.pulseIntro() }
        notch.onIntroAction = { [weak self] action in self?.handle(action) }
        notch.introHoldsWork = true
        activation = NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification,
                                                            object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { () -> Void in self?.cameBack() }
        }
        if firstRun && resume == nil && plan.first == .meet {
            sound.play(.firstLaunch, at: nil)
            clipEnded = false
            Task { [weak self, sleeper, clipSeconds] in
                await sleeper.sleep(seconds: clipSeconds)
                self?.endClip()
            }
        }
        run = Task { [weak self] in await self?.perform(plan) }
    }

    /// Skip, at any point: everything the tour drew, played or said is gone at once; then what's still required is
    /// read again from the system and the daemon (`remainingRequired`) and `onFinished` runs.
    func skip() {
        guard running else { return }
        realTask = nil
        progress.done = true
        progress.resumeBeat = nil
        end()
        Task { [weak self] in
            guard let self else { return }
            await self.setup.refreshPermissions()
            await self.setup.refreshAI()
            self.remainingRequired = self.remaining()
            self.onFinished?()
        }
    }

    private func perform(_ plan: [IntroBeat]) async {
        for next in plan {
            guard live else { return }
            beat = next
            if next != .home { progress.resumeBeat = next }
            switch next {
            case .meet: await meet()
            case .signIn: await signIn()
            case .permissions: await permissions()
            case .browsers: await browsers()
            case .identity: await identity()
            case .tryIt: await tryIt()
            case .control: await control()
            case .home: await home()
            }
        }
        guard live else { return }
        finish()
    }

    private func finish() {
        progress.done = true
        progress.resumeBeat = nil
        remainingRequired = remaining()
        end()
        onFinished?()
        // The owner's own pick from the demo, as their message, now that the tour runs nothing any more.
        if let request = realTask {
            realTask = nil
            notch.session.draft = request
            notch.startWork()
        }
    }

    /// Every way the tour ends: the orb, the sound, the lines and the previews go; the app may terminate again.
    private func end() {
        running = false
        run?.cancel()
        run = nil
        answer = nil
        controlWaiter = nil
        clipWaiter = nil
        sideChoices = [:]
        let pending = waiters
        waiters = [:]
        for resume in pending.values { resume() }
        controls.onControl = nil
        orb.onHomeArrival = nil
        orb.tearDown()
        sound.keepWarm(false)
        sound.stopAll()
        notch.onIntroAction = nil
        notch.introLine = nil
        notch.introHoldsWork = false
        if notch.promptError == NotchModel.introHeldWork { notch.promptError = nil }
        notch.conversation.removePreviews()
        if let activation { NotificationCenter.default.removeObserver(activation) }
        activation = nil
        ProcessInfo.processInfo.enableAutomaticTermination(Self.terminationReason)
    }

    /// The required steps still missing, from the last snapshots: sign-in, then each required permission not yet on.
    private func remaining() -> [String] {
        var left = Self.connected(setup.ai) ? [] : ["sign_in"]
        if setup.permissions == nil {               // the daemon didn't answer: every required row, except those on
            left += SetupModel.allowAllSequence.filter { id in
                !setup.appRows.contains { $0.id == id && $0.status == .granted }
            }
        } else {
            left += SetupModel.allowAllOrder(setup.setupRows)
        }
        return left
    }

    /// Signed in: every role uses an AI service that can be used now.
    static func connected(_ ai: AISnapshot?) -> Bool {
        guard let ai else { return false }
        let used = Set(ai.roles.values)
        return !used.isEmpty && used.allSatisfy { ai.usable($0) }
    }

    // MARK: Lines and waits

    func say(_ line: IntroLine) { notch.introLine = line }

    func dwell(_ seconds: Double) async { await sleeper.sleep(seconds: seconds) }

    /// Shows `line` and waits for the owner's answer; nil when the tour ended.
    func ask(_ line: IntroLine) async -> IntroAnswer? {
        say(line)
        return await suspend { [weak self] resume in self?.answer = { resume($0) } }
    }

    /// Shows `line()` and checks `done` every poll until it holds (nil), or the owner answers (that answer).
    func wait(showing line: @escaping () -> IntroLine, until done: @escaping () async -> Bool) async -> IntroAnswer? {
        var picked: IntroAnswer?
        answer = { picked = $0 }
        defer { answer = nil }
        while live {
            say(line())
            if await done() { return nil }
            if let picked { return picked }
            await dwell(Timing.poll)
            if let picked { return picked }
        }
        return nil
    }

    /// A wait the tour can end: `begin` gets the resume; the tour's end resumes it with nil.
    func suspend<T>(_ begin: (@escaping (T?) -> Void) -> Void) async -> T? {
        guard live else { return nil }
        let id = UUID()
        return await withCheckedContinuation { (continuation: CheckedContinuation<T?, Never>) in
            var resumed = false
            let resume: (T?) -> Void = { [weak self] value in
                guard !resumed else { return }
                resumed = true
                self?.waiters[id] = nil
                continuation.resume(returning: value)
            }
            waiters[id] = { resume(nil) }
            begin(resume)
        }
    }

    private func handle(_ action: IntroAction) {
        switch action {
        case .skip:
            skip()
        case .stopPreview:
            saw(.stop)
        case .choose(let id):
            if let act = sideChoices[id] { act(); return }
            let answered = answer
            answer = nil
            answered?(.choice(id))
        case .enter(let text):
            let answered = answer
            answer = nil
            answered?(.entry(text))
        }
    }

    private func endClip() {
        clipEnded = true
        let waiting = clipWaiter
        clipWaiter = nil
        waiting?()
    }

    /// Back from System Settings: a grant that applies only after a restart is checked (SetupModel.cameBack).
    private func cameBack() {
        guard running, beat == .permissions else { return }
        Task { await setup.cameBack() }
    }

    // MARK: 1. Meet

    /// Hello, with the sound logo after the first-launch clip has ended (never over it). The background service is
    /// installed silently meanwhile; if macOS wants the owner's OK in Login Items, the orb guides that one visit before
    /// sign-in (the daemon must run first).
    private func meet() async {
        let install = Task { [weak self] in await self?.installService() ?? false }
        if !clipEnded {
            _ = await suspend { [weak self] (resume: @escaping (Bool?) -> Void) in self?.clipWaiter = { resume(true) } }
        }
        guard live else { return }
        say(IntroCopy.hello)
        sound.play(.intro, at: nil)
        await dwell(Timing.hello)
        guard live else { return }
        if setup.serviceRunning { say(IntroCopy.gettingReady) }
        var ready = await install.value
        while live && !ready {
            if setup.serviceNeedsApproval {
                ready = await approveInLoginItems()
            } else {
                guard await ask(IntroCopy.serviceFailed(setup.serviceError)) != nil else { return }
                ready = await installService()
            }
        }
    }

    /// `ferb setup --service` and registering it with macOS (SetupModel.runService): nothing to do on an install
    /// already set up.
    private func installService() async -> Bool {
        await suspend { [weak self] (resume: @escaping (Bool?) -> Void) in
            guard let self else { return resume(false) }
            self.setup.runService { resume($0) }
        } ?? false
    }

    private func approveInLoginItems() async -> Bool {
        say(IntroCopy.loginItems)
        setup.openLoginItems()
        orb.dockBesideSettings(dragging: nil, instruction: IntroCopy.loginItemsOrb)
        while live && setup.background?.status != .enabled { await dwell(Timing.poll) }
        guard live else { return false }
        orb.hop()
        await orb.flyHome()
        guard live else { return false }
        say(IntroCopy.gettingReady)
        let ready = await setup.registerService()
        if ready { setup.serviceAction = .none }
        return ready
    }

    // MARK: 8. Home

    /// FERB small in the notch again, with one line on how to bring it back.
    private func home() async {
        if notch.expanded { notch.collapse() }
        say(IntroCopy.home)
        await dwell(Timing.home)
    }
}
