import AppKit
import AVFAudio
import NotchCore

private typealias Pick = IntroCopy.Pick

/// What the owner answered on the line showing: a choice, or what they typed.
enum IntroAnswer: Equatable {
    case choice(String)
    case entry(String)
}

/// FERB.app's first-run tour (kb area app): Meet → Sign in → Permissions → Pair browsers → FERB's email →
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
    /// Where an app is on screen, for the orb's trips in Try and Pair browsers.
    let apps: IntroAppLocating
    var progress: IntroProgress
    private let clipSeconds: Double
    /// The first-launch entrance drawn over the clip (StartupArrival); nil draws none (tests).
    private let arrival: IntroArrivalShowing?
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
    /// The orb on its way to an app (Try's preview): the flight home waits for it to have arrived.
    var orbTrip: Task<Void, Never>?
    /// The orb on its way home after a preview: the next trip out leaves once it has landed.
    var orbHoming: Task<Void, Never>?

    var rpc: DaemonRPCTransport { setup.rpc }
    /// The tour is on and this beat wasn't skipped.
    var live: Bool { running && !Task.isCancelled }

    /// `clipSeconds`: the first-launch clip's length (nil: read from the bundled file).
    init(setup: SetupModel, orb: OrbControlling, sound: FerbSoundPlaying, notch: NotchModel, progress: IntroProgress,
         controls: IntroControlFeeding, apps: IntroAppLocating? = nil, sleeper: IntroSleeping? = nil,
         clipSeconds: Double? = nil, arrival: IntroArrivalShowing? = nil) {
        self.arrival = arrival
        self.setup = setup
        self.orb = orb
        self.sound = sound
        self.notch = notch
        self.progress = progress
        self.controls = controls
        self.sleeper = sleeper ?? IntroTaskSleeper()
        self.apps = apps ?? IntroAppResolver()
        self.clipSeconds = clipSeconds ?? Self.firstLaunchClipSeconds() ?? 0
    }

    /// The bundled first-launch clip's length, in seconds.
    static func firstLaunchClipSeconds() -> Double? {
        guard let url = FerbSoundFiles.files(for: .firstLaunch, subtle: false).first,
              let file = try? AVAudioFile(forReading: url), file.processingFormat.sampleRate > 0 else { return nil }
        return Double(file.length) / file.processingFormat.sampleRate
    }

    // MARK: Start, finish, skip

    /// Runs `beats` (IntroPlan.beats). A quit left `IntroProgress.resumeBeat`: when that beat is in this plan, the
    /// tour resumes there with every planned beat after it, and sign-in and permissions from before it run again only if
    /// still needed (signed out, a grant gone since). `firstRun`: a new install (Task 9 sets it), whose tour opens with
    /// the first-launch clip; a replay or a resume plays none. The running tour keeps the controller alive until it
    /// ends (`end(stoppingSound:)` is every end's cleanup).
    func start(beats: [IntroBeat], firstRun: Bool = false) {
        guard !running else { return }
        let resume = progress.resumeBeat.flatMap { beats.contains($0) ? $0 : nil }
        var plan = beats
        var ifNeeded: Set<IntroBeat> = []
        if let resume {
            let before = beats.filter { $0.rawValue < resume.rawValue && ($0 == .signIn || $0 == .permissions) }
            ifNeeded = Set(before)
            plan = before + beats.filter { $0.rawValue >= resume.rawValue }
        }
        guard !plan.isEmpty else { return }
        running = true
        ProcessInfo.processInfo.disableAutomaticTermination(Self.terminationReason)
        sound.keepWarm(true)
        orb.holdsHomecoming = true
        orb.onHomeArrival = { [weak notch] in notch?.pulseIntro() }
        notch.onIntroAction = { [weak self] action in self?.handle(action) }
        notch.introHoldsWork = true
        if firstRun && resume == nil && plan.first == .meet {
            sound.play(.firstLaunch, at: nil)
            arrival?.show(seconds: clipSeconds)
            clipEnded = false
            Task { [weak self, sleeper, clipSeconds] in
                await sleeper.sleep(seconds: clipSeconds)
                self?.endClip()
            }
        }
        run = Task { [weak self] in await self?.perform(plan, ifNeeded: ifNeeded, resuming: resume != nil) }
    }

    /// Internal interruption for repair or teardown; there is no user-facing Skip control. Everything the tour drew,
    /// played or said is gone at once; then what's still required is
    /// read again from the system and the daemon (`remainingRequired`) and `onFinished` runs.
    /// A "Do it for real" the owner already tapped still starts, as a normal task, right after; `forRepair` (the
    /// service is down, AppSetupController.repair): it goes into the composer for them to send instead. With nothing
    /// required left, setup is recorded as done, as at Home.
    func skip(forRepair: Bool = false) {
        guard running else { return }
        progress.done = true
        progress.resumeBeat = nil
        end(stoppingSound: true)
        if forRepair, let request = realTask {
            realTask = nil
            notch.addToDraft(request)
        }
        Task { [weak self] in
            guard let self else { return }
            await self.setup.refreshPermissions()
            await self.setup.refreshAI()
            self.remainingRequired = self.remaining()
            if self.remainingRequired.isEmpty { await self.setup.recordFinished() }
            self.onFinished?()
            self.startRealTask()
        }
    }

    private func perform(_ plan: [IntroBeat], ifNeeded: Set<IntroBeat>, resuming: Bool) async {
        if resuming {
            await waitForService()
            notch.onStepBehindSettings?()        // a Quit & Reopen left Settings open under the notch (APP-642)
        }
        for next in plan {
            guard live else { return }
            if ifNeeded.contains(next), !(await stillNeeded(next)) { continue }
            guard live else { return }
            beat = next
            notch.introSkippable = next.rawValue >= IntroBeat.permissions.rawValue    // APP-638
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

    /// A resumed tour first waits for the background service (APP-515): every beat reads the daemon, and a silence
    /// would read as signed out or no grants (the resume then asked for sign-in again and moved its place back there).
    /// A service that stays silent is started (macOS's "Quit & Reopen" can end it with a clean exit, which launchd
    /// doesn't restart); a running one is left alone.
    private func waitForService() async {
        var unanswered = 0
        while live, !(await setup.serviceAnswers()) {
            say(IntroCopy.gettingReady)
            unanswered += 1
            if unanswered == setup.serviceStartAfter { _ = await setup.service.start() }
            await dwell(Timing.poll)
        }
    }

    /// A beat from before the resume point, run again only when its step is missing now.
    private func stillNeeded(_ beat: IntroBeat) async -> Bool {
        switch beat {
        case .signIn:
            await setup.refreshAI()
            return !Self.connected(setup.ai)
        case .permissions:
            await setup.refreshPermissions()
            return setup.permissions == nil || !SetupModel.allowAllOrder(setup.setupRows).isEmpty
        default:
            return true
        }
    }

    private func finish() {
        progress.done = true
        progress.resumeBeat = nil
        end(stoppingSound: false)
        onFinished?()
        startRealTask()
    }

    /// The owner's own pick from the demo, as their message, now that the tour runs nothing any more; whatever they
    /// typed meanwhile stays in the composer.
    private func startRealTask() {
        guard let request = realTask else { return }
        realTask = nil
        notch.send(request)
    }

    /// Every way the tour ends: the orb, the lines and the previews go; the app may terminate again. The engine stops
    /// being kept warm; a skip also stops what's playing at once, while the natural end lets a ringing sound (Home's, or
    /// an everyday one on the shared engine) finish. This is the only place that decides it: the orb's teardown stops
    /// only its own motion.
    private func end(stoppingSound: Bool) {
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
        arrival?.hide()
        if stoppingSound { sound.stopAll() }
        notch.onIntroAction = nil
        notch.introSkippable = false
        notch.introLine = nil
        notch.introHoldsWork = false
        if notch.promptError == NotchModel.introHeldWork { notch.promptError = nil }
        notch.conversation.removePreviews()
        ProcessInfo.processInfo.enableAutomaticTermination(Self.terminationReason)
    }

    /// The required steps still missing, from the last snapshots: sign-in, then each required permission not yet on.
    func remaining() -> [String] {
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
        case .stopPreview:
            saw(.stop)
        case .skipTour:
            guard beat.map({ $0.rawValue >= IntroBeat.permissions.rawValue }) == true else { return }
            skip()
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
        arrival?.hide()
        clipEnded = true
        let waiting = clipWaiter
        clipWaiter = nil
        waiting?()
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

    /// `ferb setup --service`, which writes and loads the LaunchAgent (SetupModel.runService): nothing to do on an
    /// install already set up.
    private func installService() async -> Bool {
        await suspend { [weak self] (resume: @escaping (Bool?) -> Void) in
            guard let self else { return resume(false) }
            self.setup.runService { resume($0) }
        } ?? false
    }

    /// The Login Items OK (FERB switched off under Allow in the Background, DMN-513): System Settings opens only when
    /// the owner taps "Open Login Items" (rule 10: the tour never moves them to another app by itself), with the orb
    /// beside it from then on; the beat waits until launchd lists FERB as switched on, then sets the service up again.
    private func approveInLoginItems() async -> Bool {
        var docked = false
        sideChoices = [Pick.openLoginItems: { [weak self] in
            guard let self, self.live else { return }
            self.setup.openLoginItems()
            if !docked {
                docked = true
                self.orb.dockBesideSettings(dragging: nil, instruction: IntroCopy.loginItemsOrb)
            }
        }]
        defer { sideChoices = [:] }
        say(IntroCopy.loginItems)
        while live, let background = setup.background, await background.switchedOff() { await dwell(Timing.poll) }
        guard live else { return false }
        if docked {
            orb.hop()
            await orb.flyHome()
        }
        guard live else { return false }
        say(IntroCopy.gettingReady)
        return await installService()                 // switched on: the setup command loads the LaunchAgent now
    }

    // MARK: 8. Home

    /// FERB small in the notch again, with one line on how to bring it back. With nothing required left, setup is
    /// recorded as done (`setup.finish`, as the setup window's Done), so neither the tour nor the window comes back.
    private func home() async {
        orb.holdsHomecoming = false
        await orb.flyHome()
        guard live else { return }
        if notch.expanded { notch.collapse() }
        say(IntroCopy.home)
        await setup.refreshPermissions()
        await setup.refreshAI()
        guard live else { return }
        remainingRequired = remaining()
        if remainingRequired.isEmpty { await setup.recordFinished() }
        await dwell(Timing.home)
    }
}
