import Foundation
import CoreGraphics

/// The decided sound family (notes/sound.md): no flight or landing sound.
public enum FerbSound: CaseIterable, Sendable {
    case firstLaunch, intro, summon, listeningStart, listeningEnd, done, needsYou, paused, error, notification
    case orbDrip, orbHop, orbMelt
}

public enum SoundPolicy {
    /// FERB's sounds follow the system's "Play user interface sound effects" and FERB's own feedback setting.
    public static func shouldPlay(_ s: FerbSound, uiSoundsEnabled: Bool, feedbackEnabled: Bool) -> Bool {
        uiSoundsEnabled && feedbackEnabled
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
