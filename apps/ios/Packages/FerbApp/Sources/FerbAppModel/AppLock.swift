import Foundation
import Observation

/// The app's own lock (kb PHN-8, PHN-15): locked on a cold start; back from the background after more than
/// [lockAfter] it is locked again. When that time passes while still in the background it locks then too (the moment
/// the Mac should also be told and fetched files go, PHN-15/PHN-11), through [backgroundTimer]. Time is injected (a
/// monotonic clock in seconds), so the rule is tested without waiting. Pure. (Android: model/AppLock.kt.)
public final class AppLock {
    /// PHN-15: five minutes after the app was left.
    public static let lockAfter: Double = 5 * 60

    public private(set) var locked = true

    /// When the app last went to the background (monotonic seconds), or nil while it is in front.
    private var leftAt: Double?
    private let now: () -> Double

    public init(now: @escaping () -> Double) {
        self.now = now
    }

    /// The owner unlocked (the prompt succeeded).
    public func unlocked() { locked = false }

    /// The app went to the background. Returns when (monotonic seconds) the background lock is due.
    @discardableResult
    public func wentToBackground() -> Double {
        let t = now()
        leftAt = t
        return t + Self.lockAfter
    }

    /// The app is in front again: locked if it was away longer than the decided time.
    public func cameToFront() {
        let left = leftAt
        leftAt = nil
        if let left, now() - left > Self.lockAfter { locked = true }
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
            let sleep = self.sleep
            timer = Task { [weak self] in
                await sleep(AppLock.lockAfter + 1)
                if Task.isCancelled { return }
                self?.backgroundTimerFired()
            }
        }
        locked = lock.locked
    }

    /// The background timer's moment (called by the timer, or by a test that moved the clock).
    public func backgroundTimerFired() {
        if lock.backgroundTimer() { locked = true }
    }

    public func unlocked() {
        lock.unlocked()
        locked = false
    }

    public func lockNow() {
        lock.lock()
        locked = true
    }

    /// Seconds on a clock that never goes back and keeps counting while the phone sleeps (CLOCK_MONOTONIC on Darwin
    /// is mach_continuous_time).
    nonisolated public static func monotonic() -> Double { Double(clock_gettime_nsec_np(CLOCK_MONOTONIC)) / 1e9 }
}
