import Foundation

// The insomnia cover's look as the owner chose it (APP-1100, for APP-985; ~/Desktop/ferb-agent-remote/spikes/x5-shield-look):
// the startup's field fully frosted (blurred, grain on top), the f written as the cover comes up, then the dot
// breathing. Everything here is a pure function of time, so the look's states are tested with an injected clock;
// Sources/FERBNotch/ShieldField.swift and InsomniaCover.swift draw it.

/// The mark at one moment (logo units, y down, as site/logo.svg).
public struct ShieldMarkFrame: Equatable, Sendable {
    /// How far above its place the dot is (logo units; 0 at home).
    public var dotLift = 0.0
    public var dotScale = 1.0
    public var dotAlpha = 1.0
    /// The dot's amber glow.
    public var glow = 0.35
    /// How much of the f is written (0-1), in hand order.
    public var reveal = 1.0
    public var fAlpha = 1.0
    /// The field's crest strength and depth: the ripple through the grain when the dot lands.
    public var energy = 0.0
    public var crest = 0.58

    public init() {}

    /// The whole mark, still (Reduce Motion, and the look once stopped).
    public static let still = ShieldMarkFrame()
}

/// Write, then Breathe (the prototype's MarkMotion `.write` as the entrance and `.breathe` at rest, unchanged).
public enum ShieldMarkMotion {
    /// The entrance: 0.4-2.2 s the f is written; at 2.2 s the dot drops in with one small bounce; the grain ripples.
    public static let writeLength = 3.6
    /// One breath of the dot at rest.
    public static let breath = 5.0

    static func smooth(_ value: Double) -> Double {
        let t = min(1, max(0, value))
        return t * t * (3 - 2 * t)
    }

    static func easeInOut(_ x: Double) -> Double {
        let t = min(1, max(0, x))
        return t < 0.5 ? 4 * t * t * t : 1 - pow(-2 * t + 2, 3) / 2
    }

    /// The dot smaller and dimmer on the out-breath, its glow brightest when full.
    static func breathe(_ f: inout ShieldMarkFrame, _ t: Double) {
        let k = (1 - cos(2 * .pi * max(0, t) / breath)) / 2
        f.dotScale = 1 - 0.12 * k
        f.dotAlpha = 1 - 0.22 * k
        f.glow = 0.12 + 0.45 * (1 - k)
    }

    /// A crest that leaves the mark when the dot lands and fades as it reaches the edge.
    static func ripple(_ f: inout ShieldMarkFrame, _ t: Double, at t0: Double) {
        guard t >= t0 else { return }
        let dt = t - t0
        f.crest = 0.04 + dt * 0.48
        f.energy = 0.55 * exp(-dt / 1.1) * smooth(dt / 0.15)
    }

    /// The mark `time` seconds after the cover came up: Write once, then Breathe for as long as it shows.
    public static func frame(_ time: Double) -> ShieldMarkFrame {
        var f = ShieldMarkFrame()
        let t = max(0, time)
        guard t < writeLength else {
            breathe(&f, t - writeLength)
            ripple(&f, t, at: 2.62)                 // the landing's ripple finishing
            return f
        }
        f.reveal = easeInOut((t - 0.4) / 1.8)
        f.fAlpha = t < 0.4 ? 0 : 1
        let drop = t - 2.2
        if drop < 0 {
            f.dotAlpha = 0
            f.dotLift = 30
        } else {
            breathe(&f, 0)
            if drop < 0.42 {
                let u = drop / 0.42
                f.dotLift = 30 * (1 - u * u)
                f.dotAlpha = smooth(drop / 0.16)
            } else if drop < 0.72 {
                let u = (drop - 0.42) / 0.30
                f.dotLift = 3.5 * 4 * u * (1 - u)     // the one small bounce
                f.glow = 0.6
            }
        }
        ripple(&f, t, at: 2.62)
        return f
    }
}

/// The frosted field's settings (APP-1100: blurrier than the prototype's σ ≈ 20 pt).
public enum ShieldFrost {
    /// The blur's strength, in points: the one constant to change it.
    public static let blurPoints = 40.0
    /// The blurred field is drawn at a quarter of the display's pixels.
    public static let blurTextureScale = 0.25
    /// The blur's sigma in the quarter-size texture's texels for a display with `pixelsPerPoint`.
    public static func blurTexels(pixelsPerPoint: Double) -> Double { blurPoints * pixelsPerPoint * blurTextureScale }
    /// The glass lifted a little, the film grain kept sharp on it (the prototype's values).
    public static let lift = 0.05
    public static let grainOnGlass = 0.30
}

/// Whether the look draws now, and how fast: Write at 30 frames a second, Breathe at 20 (the slow field looks the
/// same and costs a third less); nothing while the displays sleep or the screen is locked (as previews, APP-407),
/// nothing once stopped after 10 minutes (APP-982), one still frame under Reduce Motion.
public struct ShieldLookTimeline: Equatable, Sendable {
    public enum Phase: Equatable, Sendable { case writing, breathing, still, paused, stopped, dismissing }

    public static let entranceFPS = 30
    public static let restFPS = 20
    /// The field's clock wraps here, so hours of uptime keep Float precision in the shader.
    public static let fieldClockWrap = 10_000.0
    /// The field's moment under Reduce Motion (the prototype's still).
    public static let stillFieldTime = 10.0

    public let shownAt: TimeInterval
    public var reduced: Bool
    /// The displays sleep or the screen is locked.
    public var paused = false
    /// Stopped and dimmed (APP-982).
    public var stopped = false
    public var dismissedAt: TimeInterval?

    public init(shownAt: TimeInterval, reduced: Bool) {
        self.shownAt = shownAt
        self.reduced = reduced
    }

    public func phase(at now: TimeInterval) -> Phase {
        if dismissedAt != nil { return .dismissing }
        if reduced { return .still }
        if paused { return .paused }
        if stopped { return .stopped }
        return now - shownAt < ShieldMarkMotion.writeLength ? .writing : .breathing
    }

    /// Frames drawn each second now; 0: nothing is drawn (the last frame stays).
    public func framesPerSecond(at now: TimeInterval) -> Int {
        switch phase(at: now) {
        case .writing: return Self.entranceFPS
        case .breathing: return Self.restFPS
        case .still, .paused, .stopped, .dismissing: return 0
        }
    }

    public func mark(at now: TimeInterval) -> ShieldMarkFrame {
        reduced ? .still : ShieldMarkMotion.frame(now - shownAt)
    }

    public func fieldTime(at now: TimeInterval) -> Double {
        reduced ? Self.stillFieldTime : (now - shownAt).truncatingRemainder(dividingBy: Self.fieldClockWrap)
    }
}

/// Touch ID accepted (about 0.9 s): the dot glows once, the mark shrinks 6 % and fades, the text fades, then the field
/// fades to black and the cover goes. Under Reduce Motion one 0.3 s fade. The cover stays opaque throughout: the desktop
/// shows only when it is gone.
public enum ShieldDismiss {
    public static let length = 0.9
    public static let reducedLength = 0.3
    public static let glow = (start: 0.0, length: 0.16)
    public static let mark = (start: 0.12, length: 0.30, scale: 0.94)
    public static let text = (start: 0.0, length: 0.25)
    public static let field = (start: 0.15, length: 0.75)
    /// One strong ease-out for every exit.
    public static let ease = (0.23, 1.0, 0.32, 1.0)

    public static func length(reduced: Bool) -> Double { reduced ? reducedLength : length }
}

/// The cover's lines (the prototype's drafts, pending the owner's wording review).
public enum ShieldWords {
    /// "FERB is working · Locked to <first name>".
    public static func lockedTo(_ firstName: String) -> String { "FERB is working · Locked to \(firstName)" }
    public static let unlock = "Touch ID to unlock"
    public static let waiting = "Waiting for Touch ID…"

    /// The first word of the macOS account's full name, else the account's short name.
    public static func firstName(fullName: String, account: String) -> String {
        fullName.split(whereSeparator: \.isWhitespace).first.map(String.init) ?? account
    }
}

/// The startup's fixed film grain (StartupStudies' StudyGrain: 256 x 256, one LCG byte per point, seed 604).
public enum ShieldGrain {
    public static let side = 256

    public static func bytes() -> [UInt8] {
        var bytes = [UInt8](repeating: 0, count: side * side)
        var seed: UInt32 = 604
        for i in 0..<(side * side) {
            seed = seed &* 1664525 &+ 1013904223
            bytes[i] = UInt8(truncatingIfNeeded: seed >> 24)
        }
        return bytes
    }
}
