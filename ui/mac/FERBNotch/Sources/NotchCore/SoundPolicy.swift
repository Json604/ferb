import Foundation
import CoreGraphics

/// The decided sound family (kb area app): no flight or landing sound.
public enum FerbSound: CaseIterable, Sendable {
    case firstLaunch, intro, summon, listeningStart, listeningEnd, done, needsYou, paused, error, notification
    case orbDrip, orbHop, orbMelt
}

/// The sounds the owner switches one by one in Settings (owner, 2026-11-01): stored as `feedback.sound.<name>`
/// booleans, absent meaning the default. The tour's sounds (the first-launch clip, the logo, the orb) share one switch.
public enum SoundSwitch: String, CaseIterable, Sendable {
    case needsYou, error, done, notification, listeningStart, listeningEnd, tour, summon, paused

    public var key: String { "feedback.sound." + rawValue }
    /// On by default: needs you, error, done, notification, listening start and end, the tour. Off: summon, paused.
    public var onByDefault: Bool { self != .summon && self != .paused }

    public init(_ s: FerbSound) {
        switch s {
        case .needsYou: self = .needsYou
        case .error: self = .error
        case .done: self = .done
        case .notification: self = .notification
        case .listeningStart: self = .listeningStart
        case .listeningEnd: self = .listeningEnd
        case .summon: self = .summon
        case .paused: self = .paused
        case .firstLaunch, .intro, .orbDrip, .orbHop, .orbMelt: self = .tour
        }
    }

    /// What Settings' Play button plays for this switch.
    public var preview: FerbSound {
        switch self {
        case .needsYou: return .needsYou
        case .error: return .error
        case .done: return .done
        case .notification: return .notification
        case .listeningStart: return .listeningStart
        case .listeningEnd: return .listeningEnd
        case .tour: return .intro
        case .summon: return .summon
        case .paused: return .paused
        }
    }
}

/// A sound that plays, and at which intensity.
public struct SoundDecision: Equatable, Sendable {
    public var subtle: Bool
    public init(subtle: Bool) { self.subtle = subtle }
}

public enum SoundPolicy {
    /// Standard or Subtle (Settings), stored as "standard" or "subtle"; absent is Standard.
    public static let intensityKey = "feedback.intensity"

    /// Whether and how a sound plays: the system's "Play user interface sound effects", FERB's master switch and the
    /// sound's own switch, at the chosen intensity. During a call (another app running microphone input; read only
    /// when everything else lets the sound play) only needs you plays, at Subtle (owner, 2026-11-01).
    public static func decide(_ s: FerbSound, uiSoundsEnabled: Bool, feedbackEnabled: Bool, switchOn: Bool, subtle: Bool,
                              onACall: () -> Bool) -> SoundDecision? {
        guard uiSoundsEnabled, feedbackEnabled, switchOn else { return nil }
        guard onACall() else { return SoundDecision(subtle: subtle) }
        return s == .needsYou ? SoundDecision(subtle: true) : nil
    }

    /// The sound an ended task or reply plays, by its status: done (confirmed or not) → done; waiting on the owner →
    /// needs you; the owner's own stop → none; anything else (failed, interrupted) → error.
    public static func outcomeSound(_ status: String) -> FerbSound? {
        switch status {
        case "done", "done_unverified": return .done
        case "needs_user", "paused": return .needsYou
        case "cancelled": return nil
        default: return .error
        }
    }

    /// Stereo position from the screen x: the screen's edges map to ±0.6, never fully hard-panned.
    public static func pan(x: CGFloat, screenWidth: CGFloat) -> Float {
        guard screenWidth > 0 else { return 0 }
        return Float(min(max((x / screenWidth - 0.5) * 1.2, -0.6), 0.6))
    }

    /// Distance from the notch lowers the gain by at most 3 dB (a screen's height away or more).
    public static func distanceGainDB(fromNotch d: CGFloat, screenHeight: CGFloat) -> Float {
        guard screenHeight > 0 else { return 0 }
        return -3 * Float(min(max(d / screenHeight, 0), 1))
    }
}
