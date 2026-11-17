import Foundation
import Observation

/// The app's own lock (kb PHN-8, PHN-15): locked on a cold start; back from the background after more than
/// [lockAfter] it is locked again. When that time passes while still in the background it locks then too (the moment
/// the Mac should also be told and fetched files go, PHN-15/PHN-11), through [backgroundTimer]. Time is injected (a
/// monotonic clock in seconds), so the rule is tested without waiting. Pure. (Android: model/AppLock.kt.)
///
/// The prompt by itself (kb PHN-615, PHN-616): each time the app arrives in front locked (a new process, or back from
/// the background, relocked or still locked) one prompt is due, taken once by [takePrompt]; leaving clears it. The
/// owner's own lock from inside ([lock]) makes none: the page waits for his tap.
public final class AppLock {
    /// PHN-15: five minutes after the app was left.
    public static let lockAfter: Double = 5 * 60

    public private(set) var locked = true

    /// The unlock prompt should appear by itself (PHN-615): the app arrived in front locked and none was shown yet.
    public private(set) var promptDue = false

    /// When the app last went to the background (monotonic seconds), or nil while it is in front.
    private var leftAt: Double?
    /// Not in front: a new process, or since [wentToBackground].
    private var away = true
    private let now: () -> Double

    public init(now: @escaping () -> Double) {
        self.now = now
    }

    /// The owner unlocked (the prompt succeeded).
    public func unlocked() {
        locked = false
        promptDue = false
    }

    /// The app went to the background. Returns when (monotonic seconds) the background lock is due.
    @discardableResult
    public func wentToBackground() -> Double {
        let t = now()
        leftAt = t
        away = true
        promptDue = false
        return t + Self.lockAfter
    }

    /// The app is in front again: locked if it was away longer than the decided time. Arriving locked makes one
    /// prompt due; being told again while already in front (Face ID's sheet closed: inactive, then active) changes
    /// nothing.
    public func cameToFront() {
        let left = leftAt
        leftAt = nil
        if let left, now() - left > Self.lockAfter { locked = true }
        if away && locked { promptDue = true }
        away = false
    }

    /// Takes the due prompt: true once per arrival, and only while still locked.
    public func takePrompt() -> Bool {
        guard promptDue, locked else { return false }
        promptDue = false
        return true
    }

    /// A taken prompt could not be shown (the app lost the screen first): due again while in front and locked.
    public func promptAgain() {
        if !away && locked { promptDue = true }
    }

    /// The background timer fired: locks if the app is still away and the time has passed. True when it locked now.
    public func backgroundTimer() -> Bool {
        guard let left = leftAt else { return false }
        if locked || now() - left < Self.lockAfter { return false }
        locked = true
        return true
    }

    /// The owner asked to lock (or the phone's passcode went away).
    public func lock() { locked = true }
}

/// The app's lock over the process's life (PHN-8, PHN-15; Android's Session in FerbApplication.kt): [AppLock]'s rule,
/// whether the app is in front as the scene says it, and the timer that locks the app when five minutes pass in the
/// background. On iOS that timer runs only while iOS lets the app run; a suspended app locks the moment it comes back
/// (cameToFront), and a new process starts locked. One per process, owned by the app shell.
@MainActor
@Observable
public final class Session {
    @ObservationIgnored public let lock: AppLock
    public private(set) var locked = true
    public private(set) var front = false
    /// The unlock prompt should appear by itself (PHN-615): the app arrived in front locked.
    public private(set) var promptDue = false
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private let sleep: @Sendable (Double) async -> Void

    /// `now`: a monotonic clock in seconds that keeps counting while the phone sleeps ([monotonic]). `sleep`: how
    /// the background timer waits (tests pass one that never returns and call [backgroundTimerFired]).
    public init(now: @escaping () -> Double = Session.monotonic,
                sleep: @escaping @Sendable (Double) async -> Void = { try? await Task.sleep(nanoseconds: UInt64($0 * 1e9)) }) {
        lock = AppLock(now: now)
        self.sleep = sleep
    }

    /// The scene became active (true) or left the screen (false).
    public func foreground(_ on: Bool) {
        front = on
        timer?.cancel()
        timer = nil
        if on {
            lock.cameToFront()
        } else {
            lock.wentToBackground()
            promptDue = false
            let sleep = self.sleep
            timer = Task { [weak self] in
                await sleep(AppLock.lockAfter + 1)
                if Task.isCancelled { return }
                self?.backgroundTimerFired()
            }
        }
        locked = lock.locked
        promptDue = lock.promptDue
    }

    /// The background timer's moment (called by the timer, or by a test that moved the clock).
    public func backgroundTimerFired() {
        if lock.backgroundTimer() { locked = true }
    }

    public func unlocked() {
        lock.unlocked()
        locked = false
        promptDue = false
    }

    /// The owner's own lock from inside the app: the lock page waits for his tap (no prompt by itself, PHN-616).
    public func lockNow() {
        lock.lock()
        locked = true
    }

    /// Takes the due prompt (once per arrival).
    public func takePrompt() -> Bool {
        let took = lock.takePrompt()
        promptDue = lock.promptDue
        return took
    }

    /// The taken prompt wasn't shown (the screen went elsewhere first): due again.
    public func promptAgain() {
        lock.promptAgain()
        promptDue = lock.promptDue
    }

    /// Seconds on a clock that never goes back and keeps counting while the phone sleeps (CLOCK_MONOTONIC on Darwin
    /// is mach_continuous_time).
    nonisolated public static func monotonic() -> Double { Double(clock_gettime_nsec_np(CLOCK_MONOTONIC)) / 1e9 }
}
