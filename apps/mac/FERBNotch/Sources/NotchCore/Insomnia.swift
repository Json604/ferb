import Foundation

// Insomnia mode, phase 1 (T-1274; owner decisions APP-980..APP-985): FERB keeps the Mac working while the owner is
// away. The daemon owns the mode and decides; FERB.app shows it (the notch's line, the menu bar item, the cover) and
// reports what happens at the cover (`insomnia.report`). Everything here is plain logic with an injected clock, so it is
// tested without a window, an event tap or the camera; Sources/FERBNotch/InsomniaCover.swift is the window side.

// MARK: - The daemon's status

/// The daemon's status object (`insomnia.status`, every insomnia method's answer, `chat.status`'s `insomnia`, the bus's
/// `insomnia.changed`). Fields it doesn't carry read as off, empty or unknown.
public struct InsomniaStatus: Equatable, Sendable {
    public struct Camera: Equatable, Sendable {
        public let on: Bool
        /// "allowed", "denied", "not_asked" or "unknown" (what FERB.app last reported).
        public let state: String
    }

    public struct Power: Equatable, Sendable {
        public let battery: Int?
        public let onBattery: Bool?
        public let lidClosed: Bool?
        /// "nominal", "fair", "serious" or "critical".
        public let thermal: String?
    }

    public struct Warning: Equatable, Sendable {
        public let id: String, kind: String, text: String
        public let at: Date?
    }

    /// A person's touch or an app FERB didn't start (APP-983): Show me, Lock my Mac and It's fine answer it.
    public struct Notice: Equatable, Sendable {
        public let id: String
        /// "touched" or "activation".
        public let kind: String
        public let at: Date?
        public let text: String
        public let app: String?
        public let count: Int
    }

    /// How the last session ended, kept 10 minutes so each device says it once.
    public struct End: Equatable, Sendable {
        public let id: String, why: String, text: String
        public let at: Date?
    }

    public let on: Bool
    /// "shield" (FERB.app covers the screen, the Mac stays unlocked) or "locked".
    public let variant: String
    /// The session's start as the daemon wrote it: what tells one session from the next.
    public let since: String
    /// "mac" or the phone's endpoint id.
    public let by: String
    /// APP-984: the one Yes to bring apps forward under the cover for this session.
    public let screenYes: Bool
    public let graceMinutes: Int
    public let working: Int
    public let endsAt: Date?
    public let shieldUp: Bool
    public let macLocked: Bool
    public let camera: Camera
    public let power: Power
    public let warnings: [Warning]
    public let notice: Notice?
    /// The daemon's own line ("Keeping your Mac working · 2 tasks running · battery 64%"); empty when off.
    public let line: String
    /// Off: FERB holds the Mac from idle sleep because a task runs.
    public let taskAwake: Bool
    public let lastEnd: End?
    /// The root helper for a closed Mac on battery (APP-986): "ready" (it answers), "absent" (not registered or not
    /// approved) or "unknown".
    public let closedLid: String

    /// The grace FERB keeps the mode on after its work is done when the daemon hasn't said (its default).
    public static let defaultGraceMinutes = 10

    /// nil when `value` isn't an object.
    public init?(_ value: Any?) {
        guard let row = value as? [String: Any] else { return nil }
        on = flag(row["on"])
        variant = text(row["variant"]).isEmpty ? "shield" : text(row["variant"])
        since = text(row["since"])
        by = text(row["by"])
        screenYes = flag(row["screen_yes"])
        graceMinutes = int(row["grace_minutes"]) ?? Self.defaultGraceMinutes
        working = int(row["working"]) ?? 0
        endsAt = WindowTime.parse(row["ends_at"])
        shieldUp = flag(row["shield_up"])
        macLocked = flag(row["mac_locked"])
        let camera = row["camera"] as? [String: Any] ?? [:]
        self.camera = Camera(on: flag(camera["on"]), state: text(camera["state"]).isEmpty ? "unknown" : text(camera["state"]))
        let power = row["power"] as? [String: Any] ?? [:]
        self.power = Power(battery: int(power["battery"]), onBattery: optionalFlag(power["on_battery"]),
                           lidClosed: optionalFlag(power["lid_closed"]),
                           thermal: (power["thermal"] as? String).flatMap { $0.isEmpty ? nil : $0 })
        warnings = (row["warnings"] as? [[String: Any]] ?? []).compactMap { warning in
            let id = text(warning["id"]), words = text(warning["text"])
            guard !id.isEmpty, !words.isEmpty else { return nil }
            return Warning(id: id, kind: text(warning["kind"]), text: words, at: WindowTime.parse(warning["at"]))
        }
        if let notice = row["notice"] as? [String: Any], !text(notice["id"]).isEmpty {
            self.notice = Notice(id: text(notice["id"]), kind: text(notice["kind"]), at: WindowTime.parse(notice["at"]),
                                 text: text(notice["text"]), app: (notice["app"] as? String).flatMap { $0.isEmpty ? nil : $0 },
                                 count: int(notice["count"]) ?? 1)
        } else {
            notice = nil
        }
        line = text(row["line"])
        closedLid = text(row["closed_lid"]).isEmpty ? "unknown" : text(row["closed_lid"])
        taskAwake = flag(row["task_awake"])
        if let end = row["last_end"] as? [String: Any], !text(end["id"]).isEmpty {
            lastEnd = End(id: text(end["id"]), why: text(end["why"]), text: text(end["text"]), at: WindowTime.parse(end["at"]))
        } else {
            lastEnd = nil
        }
    }

    /// FERB.app raises its cover for this status: the mode is on and its variant is the cover.
    public var wantsCover: Bool { on && variant == "shield" }
}

private func text(_ value: Any?) -> String { value as? String ?? "" }
private func flag(_ value: Any?) -> Bool { (value as? NSNumber)?.boolValue ?? false }
private func optionalFlag(_ value: Any?) -> Bool? { (value as? NSNumber)?.boolValue }
private func int(_ value: Any?) -> Int? { (value as? NSNumber)?.intValue }

// MARK: - A person's input (APP-983, APP-984)

/// Whose an input event is, from the event's own source fields (x1 experiment 5, measured on the owner's Mac): a
/// person's keyboard, pointer and trackpad events carry source pid 0 and source state 1 (the HID system state); every
/// synthetic event, FERB's or another program's, marked or not, carries its poster's pid and a private source state.
public enum PersonInput {
    public enum Kind: String, Sendable { case keyboard, pointer, trackpad }

    /// The HID system state's source state id (`kCGEventSourceStateHIDSystemState`).
    public static let hidSystemState: Int64 = 1

    public static func isPerson(sourcePID: Int64, sourceStateID: Int64) -> Bool {
        sourcePID == 0 && sourceStateID == hidSystemState
    }

    /// What kind of input an event type is (CGEventType's raw values; the trackpad's gestures arrive as NSEvent types
    /// 29 and up), or nil for an event that isn't a person's touch (a null event, the tap's own notices).
    public static func kind(eventType: UInt32) -> Kind? {
        switch eventType {
        case 10, 11, 12, 14: return .keyboard          // key down, key up, modifiers, system-defined (brightness, media)
        case 1...7, 22, 25...27: return .pointer       // clicks, moves, drags, scrolling
        case 18...20, 29...40: return .trackpad        // rotate, gesture begin/end, gestures, magnify, swipes, pressure
        default: return nil
        }
    }

    /// A person's input of this kind, or nil (synthetic, or not an input).
    public static func classify(eventType: UInt32, sourcePID: Int64, sourceStateID: Int64) -> Kind? {
        guard isPerson(sourcePID: sourcePID, sourceStateID: sourceStateID) else { return nil }
        return kind(eventType: eventType)
    }
}

/// FERB.app tells the daemon of a person's input at most once every 2 seconds.
public struct InputThrottle: Equatable, Sendable {
    public static let interval: TimeInterval = 2
    public private(set) var last: Date?

    public init() {}

    /// True when a report may go now (and counts it as sent).
    public mutating func admit(_ now: Date) -> Bool {
        if let last, now.timeIntervalSince(last) < Self.interval { return false }
        last = now
        return true
    }
}

// MARK: - Whose an app's activation is

/// An app coming to the front while the cover is up is FERB's own when it is FERB.app itself, the app of a step in
/// progress, an app a recent `insomnia.expect` named (within its seconds), or the system's Touch ID/password prompt while
/// FERB.app itself is asking it. Anything else is unplanned: FERB.app reports it and takes the front back.
public struct ActivationPlan: Equatable, Sendable {
    /// The system's Touch ID/password prompt's agent (x1 experiment 7: it becomes the active app while it asks).
    public static let touchIDAgents: Set<String> = ["com.apple.LocalAuthentication.UIAgent", "com.apple.coreautha"]

    public struct StepApp: Equatable, Sendable {
        public let id: String
        public let bundle: String?
        public let pid: Int32?
    }

    public let ownBundle: String?
    public let ownPID: Int32
    public var step: StepApp?
    /// Apps FERB said it is about to bring forward, and until when that counts.
    public private(set) var expected: [String: Date] = [:]
    /// FERB.app is asking Touch ID or the password now.
    public var askingTouchID = false

    public init(ownBundle: String?, ownPID: Int32) {
        self.ownBundle = ownBundle
        self.ownPID = ownPID
    }

    public mutating func expect(bundle: String, seconds: Double, now: Date) {
        guard !bundle.isEmpty else { return }
        expected = expected.filter { $0.value > now }
        expected[bundle] = now.addingTimeInterval(max(0, seconds))
    }

    public enum Verdict: Equatable, Sendable {
        /// FERB.app itself: nothing to do.
        case own
        /// The app of the step in progress, or the Touch ID prompt FERB.app asked for: it keeps the front.
        case keepsFront
        /// An app FERB announced: no report; the cover takes the front back.
        case expected
        /// Nobody announced it: reported, and the cover takes the front back.
        case unplanned
    }

    public func verdict(bundle: String?, pid: Int32, now: Date) -> Verdict {
        if pid == ownPID || (bundle != nil && bundle == ownBundle) { return .own }
        if let step, (step.bundle != nil && step.bundle == bundle) || (step.pid != nil && step.pid == pid) {
            return .keepsFront
        }
        if askingTouchID, let bundle, Self.touchIDAgents.contains(bundle) { return .keepsFront }
        if let bundle, let until = expected[bundle], now <= until { return .expected }
        return .unplanned
    }
}

// MARK: - The cover's look over time (APP-982)

/// The cover shows its animated look for 10 minutes after the last event, then stops the animation and fades to
/// near-black; any event (a person's input, a notice, a step, a status change) brings it back for another 10 minutes.
/// Dimming is the cover's own drawing, never the display's backlight.
public struct ShieldLookClock: Equatable, Sendable {
    public static let animateFor: TimeInterval = 10 * 60
    public private(set) var lastEvent: Date

    public init(start: Date) { lastEvent = start }

    public mutating func event(at now: Date) { lastEvent = now }

    public func dimmed(at now: Date) -> Bool { now.timeIntervalSince(lastEvent) >= Self.animateFor }
}

/// `shield up` again every 30 seconds while the cover is up (the daemon counts it down after 90).
public struct ShieldHeartbeat: Equatable, Sendable {
    public static let every: TimeInterval = 30
    public private(set) var last: Date?

    public init() {}

    public func due(_ now: Date) -> Bool { last.map { now.timeIntervalSince($0) >= Self.every } ?? true }
    public mutating func sent(_ now: Date) { last = now }
    public mutating func reset() { last = nil }
}

// MARK: - The warning (APP-980)

/// The warning's words and the start it asks for (the contract's drafts; the owner reviews all wording before
/// shipping). The Mac's dialog and the phone's sheet say the same.
public enum InsomniaWords {
    public static let title = "Keep your Mac working while you're away?"
    public static let body = "FERB keeps your Mac awake so it can finish your tasks. It's best to leave the lid open: "
        + "your Mac stays cooler and FERB can do more. You can close it too: plugged in, FERB keeps working; on battery, "
        + "your Mac sleeps and FERB waits until you open it."
    public static let coverChoice = "Cover the screen"
    public static let coverDetail = "Your Mac stays unlocked under a cover that only your Touch ID or password removes. "
        + "If someone touches it, FERB tells your phone."
    public static let lockChoice = "Lock my Mac"
    public static let lockDetail = "Your Mac locks as usual. FERB keeps working, but can't use apps that need the screen."
    public static let screenYes = "Let FERB bring apps to the front under the cover when a step needs it"
    public static func ends(minutes: Int) -> String {
        "This ends when you're back, when you end it, or \(minutes) minutes after FERB's work is done."
    }
    public static let keepWorking = "Keep it working"
    public static let notNow = "Not now"
    /// The notch's line's button.
    public static let end = "End"
    /// The notch's line when the daemon's own line is empty.
    public static let workingLine = "Keeping your Mac working"
    /// The menu bar item, off and on.
    public static let menuStart = "Keep my Mac working…"
    public static let menuEnd = "End keeping my Mac working"
    /// What the system's Touch ID/password prompt says FERB wants ("FERB is trying to unlock your Mac's cover.").
    public static let unlockReason = "unlock your Mac's cover"
}

/// The owner's choice in the warning.
public struct InsomniaChoice: Equatable, Sendable {
    public enum Variant: String, Sendable { case shield, locked }
    public var variant: Variant = .shield
    /// The checkbox under Cover the screen: on by default.
    public var screenYes = true

    public init(variant: Variant = .shield, screenYes: Bool = true) {
        self.variant = variant
        self.screenYes = screenYes
    }

    /// `insomnia.start`'s params: the Yes counts only with the cover (it is about apps under it).
    public var startParams: [String: Any] {
        ["variant": variant.rawValue, "screen_yes": variant == .shield && screenYes]
    }
}

// MARK: - The controller

/// What the cover's windows do (FERBNotch's InsomniaCoverWindows; tests stand in their own and nothing is ordered on
/// screen).
@MainActor
public protocol InsomniaCoverSurface: AnyObject {
    /// The cover on every display, FERB.app in front with the kiosk options, the person-input and activation watchers on.
    func raise()
    /// Everything `raise` started, undone.
    func drop()
    /// Every cover window passes clicks through (`ignoresMouseEvents`) or catches them again; the windows' numbers.
    func setClickThrough(_ on: Bool) -> [Int]
    /// The cover comes to the front again (kiosk options return).
    func takeFront()
    /// The look: animated, or stopped and near-black (APP-982).
    func show(dimmed: Bool)
    /// What FERB is doing, as the daemon's status line says it.
    func show(line: String)
    /// Touch ID or the password (the cover steps down below the system prompt while it asks); `done(true)` on success.
    func authenticate(_ done: @escaping @MainActor (Bool) -> Void)
    func startCamera()
    func stopCamera()
    /// FERB.app's camera permission: "allowed", "denied" or "not_asked".
    var cameraAccess: String { get }
}

/// FERB.app's side of insomnia mode: follows the daemon's status and bus events, runs the cover's states (up, a step's
/// click-through, Touch ID, the look's 10 minutes, the heartbeat) and sends `insomnia.report`, in order.
@MainActor
public final class InsomniaController {
    public private(set) var status: InsomniaStatus?
    public private(set) var coverUp = false
    public private(set) var dimmed = false
    public private(set) var plan: ActivationPlan
    public weak var surface: InsomniaCoverSurface?
    /// Every status that arrives (the notch's line, the menu bar item, the welcome back).
    public var onStatus: ((InsomniaStatus) -> Void)?
    public var now: () -> Date
    /// How the cover waits before sending `insomnia.end` again after the owner's unlock didn't reach the daemon (about
    /// 5 s; tests stand in their own and never wait on real time).
    public var endRetryPause: @Sendable () async -> Void = { try? await Task.sleep(for: .seconds(5)) }

    private let rpc: DaemonRPCTransport
    private let clickThrough: ClickThroughReport
    private var throttle = InputThrottle()
    private var clock: ShieldLookClock
    private var heartbeat = ShieldHeartbeat()
    /// The session the owner unlocked: its cover isn't raised again while the daemon hasn't heard the end yet.
    private var unlockedSince: String?
    private var reports: Task<Void, Never>?
    private var endTask: Task<Void, Never>?

    /// The click-through source this cover's windows are told under (`ui.click_through`, with the pointer overlay's).
    public static let clickThroughSource = "cover"

    public init(rpc: DaemonRPCTransport, clickThrough: ClickThroughReport, ownBundle: String?, ownPID: Int32,
                now: @escaping () -> Date = Date.init) {
        self.rpc = rpc
        self.clickThrough = clickThrough
        self.now = now
        plan = ActivationPlan(ownBundle: ownBundle, ownPID: ownPID)
        clock = ShieldLookClock(start: now())
    }

    public var stepID: String? { plan.step?.id }

    // MARK: The daemon's status and events

    /// A status object from the daemon (nil or not an object: nothing changes).
    public func apply(_ value: Any?) {
        guard let status = InsomniaStatus(value) else { return }
        apply(status)
    }

    public func apply(_ status: InsomniaStatus) {
        let changed = status != self.status
        self.status = status
        if !status.on {
            unlockedSince = nil
            endTask?.cancel()
            endTask = nil
            surface?.stopCamera()
        }
        if status.wantsCover && status.since != unlockedSince {
            raise()
        } else {
            lower()
        }
        if changed {
            if coverUp { wake(); surface?.show(line: status.line) }
            onStatus?(status)
        }
    }

    /// One of the insomnia bus topics; false for any other topic.
    @discardableResult
    public func receive(topic: String, data: [String: Any]) -> Bool {
        switch topic {
        case "insomnia.changed": apply(data)
        case "insomnia.step":
            let id = data["step"] as? String ?? ""
            if data["phase"] as? String == "begin" {
                beginStep(id, pid: (data["pid"] as? NSNumber)?.int32Value, bundle: data["bundle"] as? String)
            } else {
                endStep(id)
            }
        case "insomnia.expect":
            guard let bundle = data["bundle"] as? String else { return true }
            plan.expect(bundle: bundle, seconds: (data["seconds"] as? NSNumber)?.doubleValue ?? 5, now: now())
        case "insomnia.camera":
            if data["on"] as? Bool == true, status?.on == true { surface?.startCamera() } else { surface?.stopCamera() }
        default: return false
        }
        return true
    }

    // MARK: The cover

    private func raise() {
        guard !coverUp, let surface else { return }
        coverUp = true
        throttle = InputThrottle()
        clock = ShieldLookClock(start: now())
        dimmed = false
        surface.raise()
        surface.show(dimmed: false)
        surface.show(line: status?.line ?? "")
        reportShield("up")
    }

    private func lower() {
        guard coverUp else { return }
        coverUp = false
        if plan.step != nil {
            plan.step = nil
            _ = surface?.setClickThrough(false)
            clickThrough.set(Self.clickThroughSource, windows: [])
        }
        plan.askingTouchID = false
        surface?.stopCamera()
        surface?.drop()
        reportShield("down")
        heartbeat.reset()
    }

    /// Something happened at the cover: the look comes back and its 10 minutes start again (APP-982).
    private func wake() {
        clock.event(at: now())
        if dimmed {
            dimmed = false
            surface?.show(dimmed: false)
        }
    }

    /// Called about once a second while the cover is up: the heartbeat and the look's 10 minutes.
    public func tick() {
        guard coverUp else { return }
        let time = now()
        if heartbeat.due(time) { reportShield("up") }
        let dim = clock.dimmed(at: time)
        if dim != dimmed {
            dimmed = dim
            surface?.show(dimmed: dim)
        }
    }

    // MARK: A step under the cover (APP-984)

    /// FERB is about to bring `bundle` forward for real input: every cover window lets clicks through, FERB's hit tests
    /// are told to skip them (`ui.click_through`), the app's activation counts as FERB's, then `step_ready`.
    public func beginStep(_ id: String, pid: Int32?, bundle: String?) {
        guard !id.isEmpty else { return }
        guard coverUp, let surface else { return report(["event": "step_failed", "step": id, "why": "no cover is up"]) }
        // The daemon asks for a step only after the owner's Yes for it: the session's one Yes (APP-984) or his answer to
        // that step's own question (from his phone), so the cover doesn't judge which.
        plan.step = ActivationPlan.StepApp(id: id, bundle: bundle, pid: pid)
        wake()
        clickThrough.set(Self.clickThroughSource, windows: surface.setClickThrough(true))
        report(["event": "step_ready", "step": id], afterClickThrough: true)
    }

    /// The step is over: the cover catches input again and takes the front back. An end for another step changes nothing.
    public func endStep(_ id: String) {
        guard let step = plan.step, id.isEmpty || id == step.id else { return }
        plan.step = nil
        wake()
        _ = surface?.setClickThrough(false)
        clickThrough.set(Self.clickThroughSource, windows: [])
        if coverUp { surface?.takeFront() }
    }

    /// The cover's windows were made again (a display came or went): during a step their new numbers are told.
    public func coverWindowsChanged(_ windows: [Int]) {
        guard plan.step != nil else { return }
        clickThrough.set(Self.clickThroughSource, windows: windows)
    }

    // MARK: What the watchers see

    /// An event the listen-only tap saw. A person's input wakes the look and is reported (at most one every 2 s).
    public func sawInput(eventType: UInt32, sourcePID: Int64, sourceStateID: Int64) {
        guard coverUp, let kind = PersonInput.classify(eventType: eventType, sourcePID: sourcePID,
                                                      sourceStateID: sourceStateID) else { return }
        wake()
        guard throttle.admit(now()) else { return }
        report(["event": "person_input", "input": kind.rawValue, "during_step": plan.step != nil])
    }

    /// The cover itself got a click or a pointer move (the tap's report covers who it was): the look wakes.
    public func coverTouched() {
        guard coverUp else { return }
        wake()
    }

    /// An app came to the front.
    public func activated(bundle: String?, name: String?, pid: Int32) {
        guard coverUp else { return }
        switch plan.verdict(bundle: bundle, pid: pid, now: now()) {
        case .own, .keepsFront: return
        case .expected:
            if plan.step == nil { surface?.takeFront() }
        case .unplanned:
            report(["event": "activation", "app": name ?? bundle ?? "", "bundle": bundle ?? ""])
            wake()
            // During a step the step's app keeps the front; the step's end takes it back.
            if plan.step == nil { surface?.takeFront() }
        }
    }

    // MARK: Unlock

    /// The cover's Unlock: Touch ID or the password. On success the cover goes at once (the owner proved it is him,
    /// whether or not the daemon answers) and the daemon hears `insomnia.end` with `why: "back"`.
    public func unlock() {
        guard coverUp, !plan.askingTouchID, let surface else { return }
        plan.askingTouchID = true
        wake()
        surface.authenticate { [weak self] ok in self?.unlocked(ok) }
    }

    private func unlocked(_ ok: Bool) {
        plan.askingTouchID = false
        guard coverUp else { return }
        guard ok else { surface?.takeFront(); return }
        unlockedSince = status?.since
        let since = unlockedSince
        sendEnd(since: since)
        lower()
    }

    /// The first try goes in the reports' order (before `shield down`); one the daemon didn't take is tried again every
    /// few seconds while this session is still on as far as FERB.app knows.
    private func sendEnd(since: String?) {
        endTask?.cancel()
        let previous = reports
        let first = Task { [rpc] () -> [String: Any]? in
            await previous?.value
            return try? await rpc.call("insomnia.end", params: ["why": "back"])
        }
        reports = Task { _ = await first.value }
        endTask = Task { [weak self, rpc] in
            var reply = await first.value
            while reply == nil {
                guard let self, !Task.isCancelled, self.unlockedSince == since, since != nil else { return }
                await self.endRetryPause()
                guard !Task.isCancelled, self.unlockedSince == since else { return }
                reply = try? await rpc.call("insomnia.end", params: ["why": "back"])
            }
            self?.apply(reply)
        }
    }

    // MARK: The owner's own start and end

    /// The warning's Keep it working.
    public func start(_ choice: InsomniaChoice) async throws {
        apply(try await rpc.call("insomnia.start", params: choice.startParams))
    }

    /// The notch's End or the menu bar's End keeping my Mac working: the owner, at the Mac, turned it off.
    public func end() async throws {
        apply(try await rpc.call("insomnia.end", params: ["why": "turned_off"]))
    }

    // MARK: Reports

    /// The camera started, stopped or couldn't (FERB.app's camera, for the phone's Show me).
    public func cameraChanged(_ state: String) {
        report(["event": "camera", "state": state])
    }

    private func reportShield(_ state: String) {
        if state == "up" { heartbeat.sent(now()) }
        report(["event": "shield", "state": state, "camera": surface?.cameraAccess ?? "not_asked"])
    }

    /// Reports go one at a time, in the order they were made; `afterClickThrough` waits for the click-through report
    /// already scheduled, so the daemon knows the windows to skip before it hears the step is ready.
    private func report(_ params: [String: Any], afterClickThrough: Bool = false) {
        let previous = reports
        reports = Task { [rpc, clickThrough] in
            await previous?.value
            if afterClickThrough { await clickThrough.settled() }
            _ = try? await rpc.call("insomnia.report", params: params)
        }
    }

    /// Waits for the reports already made (tests).
    public func settled() async {
        await reports?.value
    }
}
