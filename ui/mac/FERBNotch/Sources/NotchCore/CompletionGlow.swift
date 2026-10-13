import Foundation

/// Two finite pulses, each followed by darkness; never an endless animation.
public enum CompletionGlow {
    public static let duration: TimeInterval = 3.6

    public static func opacity(at elapsed: TimeInterval, reduceMotion: Bool = false) -> Double {
        guard elapsed >= 0, elapsed < duration else { return 0 }
        if reduceMotion { return 0.65 * min(1, (duration - elapsed) / 0.6) }
        let position = elapsed.truncatingRemainder(dividingBy: 1.8)
        guard position < 1.12 else { return 0 }
        return pow(sin(.pi * position / 1.12), 2)
    }
}
